use super::processor::ProcessedContent;
use crate::services::database::connection::with_connection;
use crate::services::database::clipboard::limit_clipboard_history;
use crate::services::database::ClipboardDataSeed;
use crate::services::settings::get_settings;
use rusqlite::params;
use chrono;
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Copy)]
struct DuplicateClipboardItem {
    id: i64,
    is_pinned: i64,
}

// 计算文本字符数
fn calculate_char_count(content: &str, content_type: &str) -> Option<i64> {
    if content_type.contains("text") || content_type.contains("rich_text") {
        let count = content.chars().count() as i64;
        if count > 0 {
            Some(count)
        } else {
            None
        }
    } else {
        None
    }
}

pub fn store_clipboard_item(content: ProcessedContent) -> Result<i64, String> {
    let settings = get_settings();
    
    if !settings.save_images && is_image_type(&content.content_type) {
        return Err("已禁止保存图片".to_string());
    }
    
    let result = with_connection(|conn| {
        let tx = conn.unchecked_transaction()?;
        let now = chrono::Local::now().timestamp();

        if let Some(duplicate) = find_duplicate_item(&content, &tx)? {
            let clipboard_id = refresh_duplicate_item(&content, &tx, duplicate, now)?;
            tx.commit()?;
            return Ok(clipboard_id);
        }

        let new_order = next_item_order(&tx, 0, None)?;
        let char_count = calculate_char_count(&content.content, &content.content_type);
        let uuid = Uuid::new_v4().to_string();
        
        tx.execute(
            "INSERT INTO clipboard (content, html_content, content_type, image_id, item_order, source_app, source_icon_hash, char_count, uuid, source_device_id, is_remote, created_at, updated_at) 
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                &content.content,
                content.html_content.as_deref(),
                &content.content_type,
                content.image_id.as_deref(),
                new_order,
                content.source_app.as_deref(),
                content.source_icon_hash.as_deref(),
                char_count,
                uuid,
                Option::<&str>::None,
                0,
                now,
                now
            ],
        )?;

        let clipboard_id = tx.last_insert_rowid();

        if !content.raw_formats.is_empty() {
            let target_id = clipboard_id.to_string();
            save_clipboard_data_items_with_conn(&tx, "clipboard", &target_id, &content.raw_formats)?;
        }

        tx.commit()?;
        Ok(clipboard_id)
    });
    
    match result {
        Ok(id) => {
            let _ = limit_clipboard_history(settings.history_limit);
            Ok(id)
        },
        Err(e) => Err(e),
    }
}

// 智能去重
fn find_duplicate_item(
    content: &ProcessedContent,
    conn: &rusqlite::Connection,
) -> Result<Option<DuplicateClipboardItem>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, content, content_type, is_pinned
         FROM clipboard 
         ORDER BY updated_at DESC, id DESC
         LIMIT 100"
    )?;
    
    let recent_items = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,      // id
            row.get::<_, String>(1)?,   // content
            row.get::<_, String>(2)?,   // content_type
            row.get::<_, i64>(3)?,      // is_pinned
        ))
    })?;
    
    for item in recent_items {
        let (db_id, db_content, db_type, is_pinned) = item?;

        let is_text_same = if is_text_type(&content.content_type) && is_text_type(&db_type) {
            content.content == db_content
        } else if is_file_type(&content.content_type) && is_file_type(&db_type) {
            compare_file_contents(&content.content, &db_content)
        } else {
            false
        };
        
        if !is_text_same {
            continue;
        }

        return Ok(Some(DuplicateClipboardItem {
            id: db_id,
            is_pinned,
        }));
    }
    
    Ok(None)
}

fn refresh_duplicate_item(
    content: &ProcessedContent,
    conn: &rusqlite::Connection,
    duplicate: DuplicateClipboardItem,
    now: i64,
) -> Result<i64, rusqlite::Error> {
    let new_order = next_item_order(conn, duplicate.is_pinned, Some(duplicate.id))?;
    let char_count = calculate_char_count(&content.content, &content.content_type);

    let rows = conn.execute(
        "UPDATE clipboard
         SET content = ?1,
             html_content = ?2,
             content_type = ?3,
             image_id = ?4,
             item_order = ?5,
             source_app = ?6,
             source_icon_hash = ?7,
             char_count = ?8,
             updated_at = ?9
         WHERE id = ?10",
        params![
            &content.content,
            content.html_content.as_deref(),
            &content.content_type,
            content.image_id.as_deref(),
            new_order,
            content.source_app.as_deref(),
            content.source_icon_hash.as_deref(),
            char_count,
            now,
            duplicate.id,
        ],
    )?;

    if rows == 0 {
        return Err(rusqlite::Error::QueryReturnedNoRows);
    }

    let target_id = duplicate.id.to_string();
    conn.execute(
        "DELETE FROM clipboard_data WHERE target_kind = 'clipboard' AND target_id = ?1",
        params![target_id],
    )?;

    if !content.raw_formats.is_empty() {
        save_clipboard_data_items_with_conn(conn, "clipboard", &target_id, &content.raw_formats)?;
    }

    Ok(duplicate.id)
}

fn next_item_order(
    conn: &rusqlite::Connection,
    is_pinned: i64,
    exclude_id: Option<i64>,
) -> Result<i64, rusqlite::Error> {
    let max_order: i64 = if let Some(exclude_id) = exclude_id {
        conn.query_row(
            "SELECT COALESCE(MAX(item_order), 0) FROM clipboard WHERE is_pinned = ?1 AND id <> ?2",
            params![is_pinned, exclude_id],
            |row| row.get(0),
        )?
    } else {
        conn.query_row(
            "SELECT COALESCE(MAX(item_order), 0) FROM clipboard WHERE is_pinned = ?1",
            params![is_pinned],
            |row| row.get(0),
        )?
    };

    Ok(max_order + 1)
}

fn save_clipboard_data_items_with_conn(
    conn: &rusqlite::Connection,
    target_kind: &str,
    target_id: &str,
    items: &[ClipboardDataSeed],
) -> Result<(), rusqlite::Error> {
    if items.is_empty() {
        return Ok(());
    }

    let now = chrono::Local::now().timestamp();
    for item in items {
        conn.execute(
            "INSERT INTO clipboard_data (
                target_kind, target_id, format_name, raw_data,
                is_primary, format_order, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(target_kind, target_id, format_name)
             DO UPDATE SET
                raw_data = excluded.raw_data,
                is_primary = excluded.is_primary,
                format_order = excluded.format_order,
                updated_at = excluded.updated_at",
            params![
                target_kind,
                target_id,
                item.format_name,
                item.raw_data,
                if item.is_primary { 1 } else { 0 },
                item.format_order,
                now,
                now,
            ],
        )?;
    }

    Ok(())
}


fn is_text_type(content_type: &str) -> bool {
    content_type.starts_with("text") || content_type.contains("rich_text") || content_type.contains("link")
}

fn is_file_type(content_type: &str) -> bool {
    content_type.contains("image") || content_type.contains("file")
}

fn is_image_type(content_type: &str) -> bool {
    content_type.contains("image")
}

// 比较文件内容
fn compare_file_contents(content1: &str, content2: &str) -> bool {
    if !content1.starts_with("files:") || !content2.starts_with("files:") {
        return content1 == content2;
    }
    
    let Ok(json1) = serde_json::from_str::<Value>(&content1[6..]) else { return false };
    let Ok(json2) = serde_json::from_str::<Value>(&content2[6..]) else { return false };
    
    extract_file_paths(&json1) == extract_file_paths(&json2)
}

// 从 JSON 提取并排序文件路径
fn extract_file_paths(json: &Value) -> Vec<String> {
    let mut paths: Vec<String> = json["files"]
        .as_array()
        .into_iter()
        .flat_map(|files| files.iter())
        .filter_map(|file| file["path"].as_str().map(String::from))
        .collect();
    
    paths.sort();
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::database::connection::test_support::{SettingsGuard, TestDb, TEST_ENV_LOCK};
    use crate::services::database::connection::with_connection;
    use crate::services::settings::AppSettings;

    fn mk_text(content: &str) -> ProcessedContent {
        ProcessedContent {
            content: content.to_string(),
            html_content: None,
            content_type: "text".to_string(),
            image_id: None,
            source_app: None,
            source_icon_hash: None,
            raw_formats: Vec::new(),
        }
    }

    fn mk_text_with_raw(content: &str, raw: Vec<ClipboardDataSeed>) -> ProcessedContent {
        ProcessedContent {
            content: content.to_string(),
            html_content: None,
            content_type: "text".to_string(),
            image_id: None,
            source_app: None,
            source_icon_hash: None,
            raw_formats: raw,
        }
    }

    fn mk_image() -> ProcessedContent {
        ProcessedContent {
            content: "files:{\"files\":[{\"path\":\"clipboard_images/abc.png\",\"name\":\"abc.png\",\"size\":1,\"is_directory\":false,\"file_type\":\"PNG\"}],\"operation\":\"copy\"}"
                .to_string(),
            html_content: None,
            content_type: "image".to_string(),
            image_id: Some("abc".to_string()),
            source_app: None,
            source_icon_hash: None,
            raw_formats: Vec::new(),
        }
    }

    fn count_clipboard() -> i64 {
        with_connection(|conn| conn.query_row("SELECT COUNT(*) FROM clipboard", [], |r| r.get(0)))
            .expect("count")
    }

    fn clip_row(id: i64) -> (
        String,
        String,
        Option<String>,
        i64,
        Option<i64>,
        i64,
        i64,
        Option<String>,
        i64,
        i64,
    ) {
        with_connection(|conn| {
            conn.query_row(
                "SELECT content, content_type, image_id, item_order, char_count, is_pinned, is_remote, uuid, created_at, updated_at FROM clipboard WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                    ))
                },
            )
        })
        .expect("读取行")
    }

    fn clip_data(target_id: &str) -> Vec<(String, Vec<u8>, bool, i64)> {
        with_connection(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT format_name, raw_data, is_primary, format_order FROM clipboard_data WHERE target_kind = 'clipboard' AND target_id = ?1 ORDER BY format_order",
                )
                .unwrap();
            let rows = stmt
                .query_map(params![target_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, bool>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            Ok(rows)
        })
        .expect("读取 raw 格式")
    }

    // b_store_new_text —— DB 落库契约部分（事件/声音/LAN 通知需 Tauri AppHandle，无法单测）
    #[test]
    fn store_text_inserts_row_with_exact_contract_fields() {
        let _guard = TEST_ENV_LOCK.lock();
        let _db = TestDb::new();

        let id = store_clipboard_item(mk_text("abc")).expect("存储应成功");
        let (content, content_type, image_id, item_order, char_count, is_pinned, is_remote, uuid, created_at, updated_at) =
            clip_row(id);

        assert_eq!(content, "abc");
        assert!(content_type.starts_with("text"));
        assert_eq!(content_type, "text");
        assert_eq!(image_id, None);
        assert_eq!(item_order, 1, "首条记录 item_order=1");
        assert_eq!(char_count, Some(3), "'abc' 的字符数=3");
        assert_eq!(is_pinned, 0);
        assert_eq!(is_remote, 0, "本地捕获 is_remote=0");
        let uuid = uuid.expect("uuid 非空");
        assert!(!uuid.is_empty());
        assert!(uuid::Uuid::parse_str(&uuid).is_ok(), "uuid 是合法 v4 uuid");
        assert_eq!(created_at, updated_at);
        assert!(created_at > 0);

        // 第二条记录 item_order 递增
        let id2 = store_clipboard_item(mk_text("def")).expect("存储应成功");
        assert_ne!(id, id2);
        assert_eq!(clip_row(id2).3, 2);
        assert_eq!(count_clipboard(), 2);
    }

    // b_store_new_text —— 多字节字符按码点计数（错误语义：Unicode）
    #[test]
    fn store_unicode_text_counts_code_points_not_bytes() {
        let _guard = TEST_ENV_LOCK.lock();
        let _db = TestDb::new();
        // '你好ab' = 4 个码点、8 个 UTF-8 字节
        let id = store_clipboard_item(mk_text("你好ab")).expect("存储应成功");
        assert_eq!(clip_row(id).4, Some(4), "字符数按 chars() 计（4 码点），而非字节数（8 字节）");
    }

    // b_duplicate_refresh
    #[test]
    fn duplicate_text_refresh_keeps_single_row_and_bumps_order() {
        let _guard = TEST_ENV_LOCK.lock();
        let _db = TestDb::new();

        let first = store_clipboard_item(mk_text("abc")).expect("存储应成功");
        let second = store_clipboard_item(mk_text("xyz")).expect("存储应成功");
        // 把首条记录的时间戳推到过去，使刷新后的 updated_at 可被严格区分
        // （updated_at 是秒级时间戳，同一秒内的两次写入无法用 > 区分）
        with_connection(|conn| {
            conn.execute("UPDATE clipboard SET updated_at = 1 WHERE id = ?1", params![first])?;
            Ok(())
        })
        .expect("重置 updated_at");
        let old_updated = clip_row(first).9;
        assert_eq!(old_updated, 1);

        // 再次复制 'abc'：返回同一 id，不插入新行
        let refreshed = store_clipboard_item(mk_text("abc")).expect("刷新应成功");
        assert_eq!(refreshed, first, "重复项刷新返回原 id");
        assert_eq!(count_clipboard(), 2, "不产生新行");

        let (content, _ct, _img, order, char_count, _pin, _remote, _uuid, _created, updated) = clip_row(first);
        assert_eq!(content, "abc");
        assert_eq!(order, 3, "刷新后 item_order 升到未置顶区最前（max+1=3）");
        assert_eq!(char_count, Some(3));
        assert!(updated > old_updated, "updated_at 被刷新（1 → 当前秒）");
        // 另一条记录不受影响
        assert_eq!(clip_row(second).3, 2);
    }

    // b_duplicate_refresh —— raw 格式整体替换
    #[test]
    fn duplicate_refresh_replaces_raw_formats() {
        let _guard = TEST_ENV_LOCK.lock();
        let _db = TestDb::new();

        let seed1 = vec![ClipboardDataSeed {
            format_name: "CF_UNICODETEXT".to_string(),
            raw_data: b"v1".to_vec(),
            is_primary: true,
            format_order: 0,
        }];
        let id = store_clipboard_item(mk_text_with_raw("abc", seed1)).expect("存储应成功");
        assert_eq!(clip_data(&id.to_string()).len(), 1);

        let seed2 = vec![
            ClipboardDataSeed {
                format_name: "CF_UNICODETEXT".to_string(),
                raw_data: b"v2".to_vec(),
                is_primary: true,
                format_order: 0,
            },
            ClipboardDataSeed {
                format_name: "CF_TEXT".to_string(),
                raw_data: b"t2".to_vec(),
                is_primary: false,
                format_order: 1,
            },
        ];
        let id2 = store_clipboard_item(mk_text_with_raw("abc", seed2)).expect("刷新应成功");
        assert_eq!(id2, id);

        let rows = clip_data(&id.to_string());
        assert_eq!(rows.len(), 2, "旧 raw 格式被 DELETE 后整体重插");
        assert_eq!(rows[0], ("CF_UNICODETEXT".to_string(), b"v2".to_vec(), true, 0));
        assert_eq!(rows[1], ("CF_TEXT".to_string(), b"t2".to_vec(), false, 1));
    }

    // b_duplicate_beyond_window
    #[test]
    fn duplicate_older_than_100_rows_inserts_new_row() {
        let _guard = TEST_ENV_LOCK.lock();
        let db = TestDb::new();
        // 放大 history_limit，避免 store 后的自动裁剪干扰本测试
        let original = get_settings();
        let enlarged = AppSettings {
            history_limit: 1000,
            ..original.clone()
        };
        crate::services::update_settings(enlarged).expect("更新设置");
        let _settings_guard = SettingsGuard(original);

        // 先插入 'X'（id 最小 → 按 updated_at DESC, id DESC 排在第 101 位）
        db.exec(
            "INSERT INTO clipboard (content, content_type, item_order, uuid, is_remote, created_at, updated_at) VALUES ('X', 'text', 1, 'ux0', 0, 1000, 1000)",
            &[],
        );
        for i in 1..=100 {
            db.exec(
                "INSERT INTO clipboard (content, content_type, item_order, uuid, is_remote, created_at, updated_at) VALUES (?1, 'text', ?2, ?3, 0, 1000, 1000)",
                &[&format!("row-{}", i), &(i + 1), &format!("u{}", i)],
            );
        }
        assert_eq!(count_clipboard(), 101);

        // 再次复制 'X'：窗口外找不到重复 → 新行
        let new_id = store_clipboard_item(mk_text("X")).expect("存储应成功");
        assert_eq!(count_clipboard(), 102, "新行插入，旧行保留");
        let old_id = db
            .query_row("SELECT id FROM clipboard WHERE uuid = 'ux0'", &[], |r| r.get::<_, i64>(0))
            .unwrap();
        assert_ne!(new_id, old_id, "重复项在窗口外 → 新行 id");
        let x_count = db
            .query_row("SELECT COUNT(*) FROM clipboard WHERE content = 'X'", &[], |r| r.get::<_, i64>(0))
            .unwrap();
        assert_eq!(x_count, 2, "新旧两条 'X' 共存");
    }

    // b_images_disabled —— DB 层契约（吞错/无事件/无声音需完整 worker 环境）
    #[test]
    fn images_disabled_rejects_image_content_exactly() {
        let _guard = TEST_ENV_LOCK.lock();
        let _db = TestDb::new();

        let original = get_settings();
        let disabled = AppSettings {
            save_images: false,
            ..original.clone()
        };
        crate::services::update_settings(disabled).expect("更新设置");
        let _settings_guard = SettingsGuard(original);

        let err = store_clipboard_item(mk_image()).expect_err("save_images=false 必须拒绝图片");
        assert_eq!(err, "已禁止保存图片");
        assert_eq!(count_clipboard(), 0, "不落库");

        // 文本仍然可存
        store_clipboard_item(mk_text("abc")).expect("文本不受 save_images 影响");
        assert_eq!(count_clipboard(), 1);
    }

    // 错误语义：类型不匹配的内容不视为重复
    #[test]
    fn text_and_file_contents_are_not_duplicates_even_with_same_string() {
        let _guard = TEST_ENV_LOCK.lock();
        let _db = TestDb::new();
        let id1 = store_clipboard_item(mk_text("abc")).expect("存储应成功");
        let file_like = ProcessedContent {
            content: "abc".to_string(),
            html_content: None,
            content_type: "file".to_string(),
            image_id: None,
            source_app: None,
            source_icon_hash: None,
            raw_formats: Vec::new(),
        };
        let id2 = store_clipboard_item(file_like).expect("存储应成功");
        assert_ne!(id1, id2);
        assert_eq!(count_clipboard(), 2);
    }

    // 错误语义：files: JSON 解析失败 → 不判重（compare_file_contents 返回 false）
    #[test]
    fn malformed_files_json_never_deduplicates() {
        assert!(!compare_file_contents("files:not-json", "files:not-json"));
        assert!(!compare_file_contents("files:{\"files\":[]}", "files:{\"oops\""));
    }

    #[test]
    fn file_duplicate_compares_sorted_path_sets() {
        let a = "files:{\"files\":[{\"path\":\"b.txt\"},{\"path\":\"a.txt\"}]}";
        let b = "files:{\"files\":[{\"path\":\"a.txt\"},{\"path\":\"b.txt\"}],\"operation\":\"copy\"}";
        assert!(compare_file_contents(a, b), "路径集合相同则判重（顺序无关）");
        let c = "files:{\"files\":[{\"path\":\"a.txt\"},{\"path\":\"c.txt\"}]}";
        assert!(!compare_file_contents(a, c), "路径集合不同不判重");
        // 非 files: 前缀退化为字符串相等
        assert!(compare_file_contents("plain", "plain"));
        assert!(!compare_file_contents("plain", "plain2"));
    }

    // b_pin_bucket_ordering —— 存储侧：置顶后新捕获落在未置顶桶
    #[test]
    fn new_captures_get_order_in_unpinned_bucket_even_when_pinned_exists() {
        let _guard = TEST_ENV_LOCK.lock();
        let db = TestDb::new();
        // 预置一条置顶记录（order 5）
        db.exec(
            "INSERT INTO clipboard (content, content_type, item_order, is_pinned, uuid, is_remote, created_at, updated_at) VALUES ('pinned', 'text', 5, 1, 'up', 0, 1000, 1000)",
            &[],
        );

        let id_a = store_clipboard_item(mk_text("a")).expect("存储应成功");
        assert_eq!(clip_row(id_a).3, 1, "未置顶桶为空 → 新记录 order=1");

        // 置顶新记录 → 置顶桶 max+1 = 6
        let pinned = crate::services::database::toggle_pin_clipboard_item(id_a).expect("置顶应成功");
        assert!(pinned);
        let (_, _, _, order_a, _, is_pinned_a, ..) = clip_row(id_a);
        assert_eq!(is_pinned_a, 1);
        assert_eq!(order_a, 6);

        // 再捕获 → 未置顶桶再次从 1 开始
        let id_b = store_clipboard_item(mk_text("b")).expect("存储应成功");
        assert_eq!(clip_row(id_b).3, 1);
        let id_c = store_clipboard_item(mk_text("c")).expect("存储应成功");
        assert_eq!(clip_row(id_c).3, 2);
    }

    // 错误语义：置顶记录被排除在 max(order) 计算之外（refresh 时）
    #[test]
    fn refresh_excludes_own_id_from_order_computation() {
        let _guard = TEST_ENV_LOCK.lock();
        let _db = TestDb::new();
        let id = store_clipboard_item(mk_text("abc")).expect("存储应成功");
        store_clipboard_item(mk_text("xyz")).expect("存储应成功");
        // 刷新 'abc'：max(other)=2 → 3
        let again = store_clipboard_item(mk_text("abc")).expect("刷新应成功");
        assert_eq!(again, id);
        assert_eq!(clip_row(id).3, 3);
    }
}

