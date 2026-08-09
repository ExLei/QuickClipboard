//! 数据库模块特征化测试（characterization tests）。
//!
//! 行为契约来源：
//! - `docs/architecture/modules/clipboard_pipeline.md`（数据库行为表 b_delete_writes_tombstone 等）
//! - 契约文件 `docs/architecture/modules/database.md` 缺失，其余行为按源码提取
//!   （connection.rs / clipboard.rs / favorites.rs / groups.rs / tombstones.rs，含行号证据）。
//!
//! 测试基础设施：全库 SQLite 是单连接（`DB_CONNECTION` 全局），所有触碰数据库的测试
//! 必须串行 —— 统一使用 `connection::test_support::{TEST_ENV_LOCK, TestDb}`（与同仓
//! 其他测试共用同一把锁与临时库管理）。断言全部为精确值/精确语义。

use super::*;
use super::connection::MAX_CONTENT_LENGTH;
use super::connection::test_support::{TestDb, TEST_ENV_LOCK};
use crate::services::database::connection::{close_database, with_connection};
use crate::services::sync_transfer::device_id;
use crate::services::webdav_sync::types::{CloudGroup, CloudRecord, CloudRecordMeta};
use rusqlite::params;
use std::collections::HashMap;
use uuid::Uuid;

/// 持锁运行测试闭包（每个测试一个全新临时数据库，Drop 时自动 close + 清理）。
fn with_test_db<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let _guard = TEST_ENV_LOCK.lock();
    let _db = TestDb::new();
    f()
}

/// 轮询等待条件成立（用于异步 char_count 回填等）。
fn wait_until(timeout_ms: u64, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    cond()
}


/// 便捷查询：执行无参 SQL 并把行映射为 Vec<T>（内部已持全局连接锁）。
fn query_all<T, F>(sql: &str, f: F) -> Vec<T>
where
    F: Fn(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
{
    with_connection(|conn| {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], f)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    })
    .unwrap()
}


// ---------- 种子数据辅助 ----------

/// 插入一条剪贴板行（item_order = 当前 max+1，char_count 按内容计算）。
fn seed_clipboard(
    content: &str,
    content_type: &str,
    updated_at: i64,
    is_pinned: bool,
    uuid: Option<&str>,
) -> i64 {
    with_connection(|conn| {
        let next_order: i64 = conn
            .query_row("SELECT COALESCE(MAX(item_order), 0) + 1 FROM clipboard", [], |r| {
                r.get(0)
            })
            .unwrap_or(1);
        conn.execute(
            "INSERT INTO clipboard
                (content, html_content, content_type, image_id, item_order, is_pinned, uuid,
                 created_at, updated_at, char_count)
             VALUES (?1, NULL, ?2, NULL, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                content,
                content_type,
                next_order,
                is_pinned as i64,
                uuid,
                updated_at,
                updated_at,
                content.chars().count() as i64
            ],
        )?;
        Ok(conn.last_insert_rowid())
    })
    .unwrap()
}

/// 插入一条剪贴板行，char_count 显式为 NULL（触发懒计算/回填路径）。
fn seed_clipboard_null_char_count(content: &str, content_type: &str) -> i64 {
    with_connection(|conn| {
        conn.execute(
            "INSERT INTO clipboard (content, content_type, item_order, created_at, updated_at, char_count)
             VALUES (?1, ?2, 1, 1, 1, NULL)",
            params![content, content_type],
        )?;
        Ok(conn.last_insert_rowid())
    })
    .unwrap()
}

/// 插入一条收藏（item_order = 当前 max+1）。
fn seed_favorite(id: &str, title: &str, content: &str, group: &str, updated_at: i64) {
    with_connection(|conn| {
        let next_order: i64 = conn
            .query_row("SELECT COALESCE(MAX(item_order), 0) + 1 FROM favorites", [], |r| {
                r.get(0)
            })
            .unwrap_or(1);
        conn.execute(
            "INSERT INTO favorites (id, title, content, content_type, group_name, item_order, created_at, updated_at, char_count)
             VALUES (?1, ?2, ?3, 'text', ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                title,
                content,
                group,
                next_order,
                updated_at,
                updated_at,
                content.chars().count() as i64
            ],
        )?;
        Ok(())
    })
    .unwrap()
}

fn seed_group(name: &str, icon: &str, color: &str, order: i32) {
    with_connection(|conn| {
        conn.execute(
            "INSERT INTO groups (name, icon, color, order_index, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 1, 1)",
            params![name, icon, color, order],
        )?;
        Ok(())
    })
    .unwrap()
}

fn clipboard_row_count() -> i64 {
    with_connection(|conn| conn.query_row("SELECT COUNT(*) FROM clipboard", [], |r| r.get(0))).unwrap()
}

fn favorites_row_count() -> i64 {
    with_connection(|conn| conn.query_row("SELECT COUNT(*) FROM favorites", [], |r| r.get(0))).unwrap()
}

fn tombstone_deleted_at(collection: &str, item_id: &str) -> Option<i64> {
    with_connection(|conn| {
        Ok(conn
            .query_row(
                "SELECT deleted_at FROM sync_tombstones WHERE collection = ?1 AND item_id = ?2",
                params![collection, item_id],
                |r| r.get(0),
            )
            .ok())
    })
    .unwrap()
}

/// 在全局连接上记录 tombstone（包装 pub(crate) 的 *_in_conn 函数）。
fn record_tombstone(collection: &str, item_id: &str, source_device_id: &str, deleted_at: i64) {
    with_connection(|conn| {
        record_sync_tombstone_in_conn(conn, collection, item_id, source_device_id, deleted_at)?;
        Ok(())
    })
    .unwrap()
}

fn make_record(uuid: &str, content: &str, updated_at: i64) -> CloudRecord {
    CloudRecord {
        uuid: uuid.to_string(),
        source_device_id: "dev-r".to_string(),
        is_remote: false,
        content: content.to_string(),
        html_content: None,
        content_type: "text".to_string(),
        image_id: None,
        source_app: None,
        source_icon_hash: None,
        char_count: None,
        title: String::new(),
        group_name: "全部".to_string(),
        item_order: 1,
        paste_count: 0,
        created_at: updated_at,
        updated_at,
    }
}

// =====================================================================
// connection.rs
// =====================================================================

/// b_db_init_pragmas: init_database 设置 WAL/synchronous NORMAL/foreign_keys ON/
/// cache_size 10000/temp_store MEMORY，建表与索引。
#[test]
fn init_database_sets_pragmas_schema() {
    with_test_db(|| {
        with_connection(|conn| {
            let journal: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
            let fk: i64 = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).unwrap();
            let sync: i64 = conn.query_row("PRAGMA synchronous", [], |r| r.get(0)).unwrap();
            let cache: i64 = conn.query_row("PRAGMA cache_size", [], |r| r.get(0)).unwrap();
            let temp_store: i64 = conn.query_row("PRAGMA temp_store", [], |r| r.get(0)).unwrap();
            assert_eq!(journal, "wal", "journal_mode 应为 wal");
            assert_eq!(fk, 1, "foreign_keys 应为 ON");
            assert_eq!(sync, 1, "synchronous 应为 NORMAL(=1)");
            assert_eq!(cache, 10000);
            assert_eq!(temp_store, 2, "temp_store 应为 MEMORY(=2)");
            for table in ["clipboard", "clipboard_data", "favorites", "groups", "sync_tombstones"] {
                let n: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                        params![table],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(n, 1, "表 {} 应存在", table);
            }
            for idx in [
                "idx_clipboard_order",
                "idx_clipboard_data_unique",
                "idx_clipboard_uuid_unique",
                "idx_favorites_group",
                "idx_sync_tombstones_deleted_at",
            ] {
                let n: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                        params![idx],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(n, 1, "索引 {} 应存在", idx);
            }
            Ok(())
        })
        .unwrap();
    });
}

/// b_db_uninitialized_error: 未初始化时 with_connection 返回裸字符串 "数据库未初始化"；
/// 闭包错误包装为 "数据库操作失败: <e>"。
#[test]
fn uninitialized_db_returns_bare_error_and_closure_errors_are_wrapped() {
    let _guard = TEST_ENV_LOCK.lock();
    // 确保全局连接为 None
    close_database();
    let err = get_clipboard_count().unwrap_err();
    assert_eq!(err, "数据库未初始化", "未初始化必须返回裸错误字符串");

    // 初始化后，闭包内的 SQL 错误应被包装
    let _db = TestDb::new();
    let wrapped = with_connection(|conn| {
        conn.query_row("SELECT x FROM no_such_table", [], |r| r.get::<_, i64>(0))
            .map(|_| ())
    })
    .unwrap_err();
    assert_eq!(
        wrapped, "数据库操作失败: no such table: no_such_table",
        "闭包错误应被精确包装，实际: {}",
        wrapped
    );
}

/// b_clipboard_order_migration: 旧式 item_order（ASC）在 create_tables 时迁移为 DESC 全局序号。
/// 迁移条件：存在负 item_order 或 MAX(item_order) < COUNT(*)。
#[test]
fn migrate_clipboard_order_reassigns_asc_orders_to_desc() {
    let _guard = TEST_ENV_LOCK.lock();
    let dir = std::env::temp_dir().join(format!("qc_db_test_{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("quickclipboard.db");
    {
        // 用旧式最小 schema 预置数据（含 item_order 0,1,2 与 updated_at 递增）
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE clipboard (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                content TEXT NOT NULL,
                content_type TEXT NOT NULL DEFAULT 'text',
                item_order INTEGER NOT NULL DEFAULT 0,
                is_pinned INTEGER NOT NULL DEFAULT 0,
                uuid TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
             );
             INSERT INTO clipboard (content, content_type, item_order, is_pinned, created_at, updated_at)
             VALUES ('a','text',0,0,100,100), ('b','text',1,0,200,200), ('c','text',2,0,300,300);",
        )
        .unwrap();
    }
    init_database(db_path.to_str().unwrap()).unwrap();
    let orders: Vec<(String, i64)> = query_all("SELECT content, item_order FROM clipboard ORDER BY id", |r| Ok((r.get(0)?, r.get(1)?)));
    // 迁移按 (is_pinned DESC, item_order ASC, updated_at DESC) 排序后赋 count-i：
    // a(idx0)->3, b(idx1)->2, c(idx2)->1
    assert_eq!(orders, vec![("a".to_string(), 3), ("b".to_string(), 2), ("c".to_string(), 1)]);

    // 迁移后特征消失（MAX >= COUNT 且无负数）-> 再跑一次不再改动
    with_connection(|conn| {
        connection::migrate_clipboard_order(conn);
        Ok(())
    })
    .unwrap();
    let orders2: Vec<i64> = query_all("SELECT item_order FROM clipboard ORDER BY id", |r| r.get(0));
    assert_eq!(orders2, vec![3, 2, 1]);
    close_database();
    let _ = std::fs::remove_dir_all(&dir);
}

/// b_favorites_global_order_migration: user_version<1 时一次性把收藏迁移为全局序号，
/// 迁移后 user_version=1 且重复初始化不再改变。
#[test]
fn favorites_global_order_migration_runs_once() {
    let _guard = TEST_ENV_LOCK.lock();
    let dir = std::env::temp_dir().join(format!("qc_db_test_{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("quickclipboard.db");
    init_database(db_path.to_str().unwrap()).unwrap();
    // 三条收藏全部 item_order=0（旧式非全局序号）
    seed_favorite("f1", "t1", "c1", "全部", 100);
    seed_favorite("f2", "t2", "c2", "全部", 200);
    seed_favorite("f3", "t3", "c3", "全部", 300);
    // 回退 user_version，模拟旧库
    with_connection(|conn| {
        conn.execute_batch("PRAGMA user_version = 0")?;
        Ok(())
    })
    .unwrap();
    close_database();
    init_database(db_path.to_str().unwrap()).unwrap();

    let orders: Vec<(String, i64)> = query_all("SELECT id, item_order FROM favorites ORDER BY id", |r| Ok((r.get(0)?, r.get(1)?)));
    // ORDER BY item_order DESC, updated_at DESC, created_at DESC, id ASC
    // 全部 order=0 -> f3(300) idx0->3, f2(200) idx1->2, f1(100) idx2->1
    assert_eq!(
        orders,
        vec![("f1".to_string(), 1), ("f2".to_string(), 2), ("f3".to_string(), 3)]
    );
    let uv: i32 = with_connection(|conn| conn.query_row("PRAGMA user_version", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(uv, 1, "迁移后 user_version 应为 1");
    // 重复初始化不再重排
    close_database();
    init_database(db_path.to_str().unwrap()).unwrap();
    let orders2: Vec<i64> = query_all("SELECT item_order FROM favorites ORDER BY id", |r| r.get(0));
    assert_eq!(orders2, vec![1, 2, 3]);
    close_database();
    let _ = std::fs::remove_dir_all(&dir);
}

/// b_favorites_auto_title_cleanup: 图片/文件类型且标题等于自动生成标题的收藏，init 时清空标题。
#[test]
fn init_clears_auto_generated_titles_for_file_and_image_favorites() {
    let _guard = TEST_ENV_LOCK.lock();
    let dir = std::env::temp_dir().join(format!("qc_db_test_{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("quickclipboard.db");
    init_database(db_path.to_str().unwrap()).unwrap();
    let long_content = "X".repeat(60);
    let auto_title = format!("{}...", &long_content[..50]);
    with_connection(|conn| {
        conn.execute_batch(&format!(
            "INSERT INTO favorites (id, title, content, content_type, group_name, item_order, created_at, updated_at)
             VALUES ('f-img', '{}', '{}', 'image', '全部', 1, 1, 1),
                    ('f-file', '{}', '{}', 'file', '全部', 2, 1, 1),
                    ('f-custom', 'My Title', '{}', 'image', '全部', 3, 1, 1),
                    ('f-text', '{}', '{}', 'text', '全部', 4, 1, 1);",
            auto_title, long_content, auto_title, long_content, long_content, long_content, long_content
        ))?;
        Ok(())
    })
    .unwrap();

    close_database();
    init_database(db_path.to_str().unwrap()).unwrap();

    let titles: Vec<(String, String)> = query_all("SELECT id, title FROM favorites ORDER BY id", |r| Ok((r.get(0)?, r.get(1)?)));
    let map: HashMap<_, _> = titles.into_iter().collect();
    assert_eq!(map.get("f-img").unwrap(), "", "图片类型自动标题应被清空");
    assert_eq!(map.get("f-file").unwrap(), "", "文件类型自动标题应被清空");
    assert_eq!(map.get("f-custom").unwrap(), "My Title", "自定义标题应保留");
    assert_eq!(map.get("f-text").unwrap(), &long_content, "文本类型不清理");
    close_database();
    let _ = std::fs::remove_dir_all(&dir);
}

// =====================================================================
// clipboard.rs
// =====================================================================

/// b_delete_writes_tombstone: 删除剪贴板项记录 tombstone（item_id=uuid），
/// 删除行与 clipboard_data，无 uuid 时 tombstone 用 id 字符串。
#[test]
fn delete_item_records_tombstone_and_removes_row_and_data() {
    with_test_db(|| {
        let id = seed_clipboard("hello", "text", 100, false, Some("u1"));
        save_clipboard_data_items(
            "clipboard",
            &id.to_string(),
            &[ClipboardDataSeed {
                format_name: "text/plain".into(),
                raw_data: b"hello".to_vec(),
                is_primary: true,
                format_order: 0,
            }],
        )
        .unwrap();

        let before = chrono::Local::now().timestamp();
        delete_clipboard_item(id).unwrap();
        let after = chrono::Local::now().timestamp();

        assert_eq!(clipboard_row_count(), 0);
        assert!(
            get_clipboard_data_items("clipboard", &id.to_string()).unwrap().is_empty(),
            "clipboard_data 应随行删除"
        );
        let (tombstone_item, tombstone_src, deleted_at): (String, String, i64) =
            with_connection(|conn| {
                conn.query_row(
                    "SELECT item_id, source_device_id, deleted_at FROM sync_tombstones
                     WHERE collection = 'history' AND item_id = 'u1'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
            })
            .unwrap();
        assert_eq!(tombstone_item, "u1");
        assert_eq!(tombstone_src, device_id());
        assert!(deleted_at >= before && deleted_at <= after, "deleted_at 应在操作时间窗内");

        // 无 uuid 的行：tombstone item_id = id 字符串
        let id2 = seed_clipboard("x", "text", 200, false, None);
        delete_clipboard_item(id2).unwrap();
        assert!(
            tombstone_deleted_at("history", &id2.to_string()).is_some(),
            "无 uuid 行 tombstone 用 id 字符串"
        );
        assert_eq!(tombstone_deleted_at("history", &format!("uuid-{}", id2)), None);
    });
}

/// b_delete_writes_tombstone（引用保护）: 图片仍被其他行引用时不被标记清理。
#[test]
fn delete_item_with_image_id_keeps_referencing_row() {
    with_test_db(|| {
        let img = format!("tstimg_{}", Uuid::new_v4());
        let id = with_connection(|conn| {
            conn.execute(
                "INSERT INTO clipboard (content, content_type, image_id, item_order, created_at, updated_at)
                 VALUES ('pic', 'image', ?1, 1, 1, 1)",
                params![&img],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .unwrap();
        let id2 = with_connection(|conn| {
            conn.execute(
                "INSERT INTO clipboard (content, content_type, image_id, item_order, created_at, updated_at)
                 VALUES ('pic2', 'image', ?1, 2, 1, 1)",
                params![&img],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .unwrap();
        delete_clipboard_item(id).unwrap();
        // 删除只影响目标行；引用行保留
        assert_eq!(clipboard_row_count(), 1);
        let left: String = with_connection(|conn| {
            conn.query_row("SELECT content FROM clipboard WHERE id = ?1", params![id2], |r| {
                r.get(0)
            })
        })
        .unwrap();
        assert_eq!(left, "pic2");
    });
}

/// b_tombstone_blocks_remote: lan_upsert_history_records 在 tombstone.deleted_at >=
/// record.updated_at 时跳过记录（不插入/不更新，不出现在 changed 回执）。
/// 边界：updated_at == deleted_at 也跳过；updated_at == deleted_at + 1 才应用。
#[test]
fn tombstone_blocks_remote_upsert_with_boundary() {
    with_test_db(|| {
        record_tombstone(COLLECTION_HISTORY, "u1", "dev-x", 5000);

        // updated_at == deleted_at -> 跳过
        let changed = lan_upsert_history_records(&[make_record("u1", "blocked-eq", 5000)]).unwrap();
        assert!(changed.is_empty());
        assert_eq!(clipboard_row_count(), 0);

        // updated_at < deleted_at -> 跳过
        let changed = lan_upsert_history_records(&[make_record("u1", "blocked-lt", 4999)]).unwrap();
        assert!(changed.is_empty());
        assert_eq!(clipboard_row_count(), 0);

        // updated_at == deleted_at + 1 -> 应用（边界另一侧）
        let changed = lan_upsert_history_records(&[make_record("u1", "applied", 5001)]).unwrap();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].uuid, "u1");
        let (content, is_remote): (String, i64) = with_connection(|conn| {
            conn.query_row(
                "SELECT content, is_remote FROM clipboard WHERE uuid = 'u1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
        })
        .unwrap();
        assert_eq!(content, "applied");
        assert_eq!(is_remote, 1, "远端记录应标记 is_remote=1");
    });
}

/// b_repair_overrides_tombstone: webdav_repair_history_records 无视 tombstone 应用记录，
/// updated_at = max(record.updated_at, now, tombstone.deleted_at+1)，清除 tombstone，is_remote=1。
#[test]
fn repair_overrides_tombstone_and_clears_it() {
    with_test_db(|| {
        record_tombstone(COLLECTION_HISTORY, "u1", "dev-x", 5000);

        let before = chrono::Utc::now().timestamp();
        let changed = webdav_repair_history_records(&[make_record("u1", "repaired", 1000)]).unwrap();
        let after = chrono::Utc::now().timestamp();

        assert_eq!(changed.len(), 1, "修复模式必须返回应用结果");
        let (content, is_remote, updated_at): (String, i64, i64) = with_connection(|conn| {
            conn.query_row(
                "SELECT content, is_remote, updated_at FROM clipboard WHERE uuid = 'u1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
        })
        .unwrap();
        assert_eq!(content, "repaired");
        assert_eq!(is_remote, 1);
        assert!(
            updated_at >= before && updated_at <= after,
            "restored updated_at 应在调用时间窗内"
        );
        assert!(updated_at >= 5001, "必须 >= tombstone.deleted_at + 1");
        assert_eq!(
            tombstone_deleted_at(COLLECTION_HISTORY, "u1"),
            None,
            "修复后 tombstone 应被清除"
        );
    });
}

/// b_history_limit_trim: limit_clipboard_history 按 (is_pinned DESC, item_order DESC,
/// updated_at DESC) 保留前 N 条，删除其余行及其 clipboard_data。
#[test]
fn limit_history_trims_beyond_top_n_pinned_first() {
    with_test_db(|| {
        let keep_data = vec![ClipboardDataSeed {
            format_name: "text/plain".into(),
            raw_data: b"keep".to_vec(),
            is_primary: true,
            format_order: 0,
        }];
        let drop_data = vec![ClipboardDataSeed {
            format_name: "text/plain".into(),
            raw_data: b"drop".to_vec(),
            is_primary: true,
            format_order: 0,
        }];
        // 置顶行(updated_at 1000) + 非置顶 item_order 1..5 (updated_at 100..500)
        let pinned = seed_clipboard("pinned", "text", 1000, true, None);
        let o1 = seed_clipboard("o1", "text", 100, false, None);
        let o2 = seed_clipboard("o2", "text", 200, false, None);
        seed_clipboard("o3", "text", 300, false, None);
        let o4 = seed_clipboard("o4", "text", 400, false, None);
        let o5 = seed_clipboard("o5", "text", 500, false, None);
        save_clipboard_data_items("clipboard", &pinned.to_string(), &keep_data).unwrap();
        save_clipboard_data_items("clipboard", &o5.to_string(), &keep_data).unwrap();
        save_clipboard_data_items("clipboard", &o1.to_string(), &drop_data).unwrap();
        save_clipboard_data_items("clipboard", &o2.to_string(), &drop_data).unwrap();

        limit_clipboard_history(3).unwrap();

        assert_eq!(clipboard_row_count(), 3, "保留 1 置顶 + 2 最新非置顶");
        let kept: Vec<String> = query_all("SELECT content FROM clipboard", |r| r.get(0));
        let mut kept_sorted = kept.clone();
        kept_sorted.sort();
        assert_eq!(kept_sorted, vec!["o4", "o5", "pinned"]);
        // 被删行的 clipboard_data 清除；保留行的仍在
        assert!(get_clipboard_data_items("clipboard", &o1.to_string()).unwrap().is_empty());
        assert!(get_clipboard_data_items("clipboard", &o2.to_string()).unwrap().is_empty());
        assert_eq!(get_clipboard_data_items("clipboard", &o5.to_string()).unwrap().len(), 1);
        assert_eq!(get_clipboard_data_items("clipboard", &pinned.to_string()).unwrap().len(), 1);
    });
}

/// b_history_limit_trim: max_count >= 999999 跳过修剪；max_count=0 清空全部。
#[test]
fn limit_history_skips_at_huge_limit_and_empties_at_zero() {
    with_test_db(|| {
        seed_clipboard("a", "text", 100, false, None);
        seed_clipboard("b", "text", 200, false, None);
        limit_clipboard_history(999999).unwrap();
        assert_eq!(clipboard_row_count(), 2, ">=999999 必须跳过修剪");

        limit_clipboard_history(0).unwrap();
        assert_eq!(clipboard_row_count(), 0, "limit=0 时 SQL LIMIT 0 语义：全部删除");
    });
}

/// b_clear_history_writes_tombstones: 清空历史为每行记录 tombstone 并删除 clipboard_data。
#[test]
fn clear_history_writes_tombstones_and_clears_data() {
    with_test_db(|| {
        let id1 = seed_clipboard("a", "text", 100, false, Some("u1"));
        seed_clipboard("b", "text", 200, false, Some("u2"));
        save_clipboard_data_items(
            "clipboard",
            &id1.to_string(),
            &[ClipboardDataSeed {
                format_name: "text/plain".into(),
                raw_data: b"a".to_vec(),
                is_primary: true,
                format_order: 0,
            }],
        )
        .unwrap();

        clear_clipboard_history().unwrap();

        assert_eq!(clipboard_row_count(), 0);
        assert!(get_clipboard_data_items("clipboard", &id1.to_string()).unwrap().is_empty());
        assert!(tombstone_deleted_at(COLLECTION_HISTORY, "u1").is_some());
        assert!(tombstone_deleted_at(COLLECTION_HISTORY, "u2").is_some());
    });
}

/// b_upsert_lww_skip_newer: 已存在行 updated_at >= 记录时跳过（LWW），
/// 且当 tombstone 比行旧时跳过路径会清除 tombstone。
#[test]
fn upsert_skips_when_existing_row_newer_and_clears_older_tombstone() {
    with_test_db(|| {
        // 已有行 updated_at=5000
        let changed = lan_upsert_history_records(&[make_record("u1", "first", 5000)]).unwrap();
        assert_eq!(changed.len(), 1);
        // 旧 tombstone (3000 < 行 5000)
        record_tombstone(COLLECTION_HISTORY, "u1", "dev-x", 3000);

        // 记录比行旧 -> 跳过，且 tombstone 被清除
        let changed = lan_upsert_history_records(&[make_record("u1", "stale", 4000)]).unwrap();
        assert!(changed.is_empty(), "旧记录不得回执为 changed");
        let content: String = with_connection(|conn| {
            conn.query_row("SELECT content FROM clipboard WHERE uuid = 'u1'", [], |r| r.get(0))
        })
        .unwrap();
        assert_eq!(content, "first", "旧记录不得覆盖新行");
        assert_eq!(
            tombstone_deleted_at(COLLECTION_HISTORY, "u1"),
            None,
            "比行旧的 tombstone 在 skip 路径应被清除"
        );

        // 记录比行新 -> 应用
        let changed = lan_upsert_history_records(&[make_record("u1", "newer", 6000)]).unwrap();
        assert_eq!(changed.len(), 1);
        let content: String = with_connection(|conn| {
            conn.query_row("SELECT content FROM clipboard WHERE uuid = 'u1'", [], |r| r.get(0))
        })
        .unwrap();
        assert_eq!(content, "newer");
    });
}

/// b_search_truncation: 搜索时 >1600 字节的文本按关键词截断（关键词位于摘要开头附近），
/// 返回 char_count 懒计算值；total_count 反映过滤；非文本类型不截断。
#[test]
fn query_truncates_around_keyword_and_backfills_char_count() {
    with_test_db(|| {
        let content = format!("{}needle{}", "A".repeat(3000), "B".repeat(3000));
        let id = seed_clipboard_null_char_count(&content, "text");

        let result = query_clipboard_items(QueryParams {
            offset: 0,
            limit: 50,
            search: Some("needle".to_string()),
            content_type: None,
        })
        .unwrap();

        assert_eq!(result.total_count, 1);
        assert_eq!(result.items.len(), 1);
        let excerpt = &result.items[0].content;
        assert!(excerpt.len() <= MAX_CONTENT_LENGTH as usize);
        // "..."(3) + 16 个前文字符后即关键词
        assert!(excerpt.starts_with("..."));
        assert_eq!(&excerpt[19..25], "needle", "关键词应位于摘要开头 16 字符处");
        assert!(excerpt.ends_with("..."));
        // char_count 懒计算（行内返回）
        assert_eq!(result.items[0].char_count, Some(6006));
        // 异步回填最终落到数据库
        let backfilled = wait_until(2000, || {
            with_connection(|conn| {
                conn.query_row(
                    "SELECT char_count FROM clipboard WHERE id = ?1",
                    params![id],
                    |r| r.get::<_, Option<i64>>(0),
                )
            })
            .unwrap_or(None)
            .is_some()
        });
        assert!(backfilled, "char_count 应被异步回填到数据库");
    });
}

/// b_search_truncation（非搜索路径 + 类型过滤）: 无搜索时按截断字符串处理；
/// 非文本类型永不截断；content_type 过滤生效。
#[test]
fn query_plain_truncation_and_content_type_filter() {
    with_test_db(|| {
        let long_text = "Z".repeat(3000);
        let id_text = seed_clipboard(&long_text, "text", 100, false, None);
        let id_image = seed_clipboard(&long_text, "image", 200, false, None);

        // 无搜索：truncate_string（截断点 1550 + 固定后缀）
        let result = query_clipboard_items(QueryParams::default()).unwrap();
        let text_item = result.items.iter().find(|i| i.id == id_text).unwrap();
        assert!(text_item.content.starts_with(&"Z".repeat(1550)));
        assert!(text_item.content.ends_with("...(内容过长已截断)"));
        let image_item = result.items.iter().find(|i| i.id == id_image).unwrap();
        assert_eq!(image_item.content, long_text, "非文本类型不得截断");

        // content_type 过滤
        let result = query_clipboard_items(QueryParams {
            offset: 0,
            limit: 50,
            search: None,
            content_type: Some("image".to_string()),
        })
        .unwrap();
        assert_eq!(result.total_count, 1);
        assert_eq!(result.items[0].id, id_image);

        // 搜索无命中
        let result = query_clipboard_items(QueryParams {
            offset: 0,
            limit: 50,
            search: Some("needle-absent".to_string()),
            content_type: None,
        })
        .unwrap();
        assert_eq!(result.total_count, 0);
        assert!(result.items.is_empty());
        assert!(!result.has_more);
    });
}

/// b_pin_bucket_ordering: toggle_pin 在置顶/非置顶桶之间移动，item_order = 目标桶 max+1；
/// 返回新状态；不存在的 id 报错。
#[test]
fn toggle_pin_moves_between_buckets_with_max_plus_one() {
    with_test_db(|| {
        let id_a = seed_clipboard("a", "text", 100, false, None);
        let id_b = seed_clipboard("b", "text", 200, false, None);

        let pinned = toggle_pin_clipboard_item(id_b).unwrap();
        assert_eq!(pinned, true);
        let (p, order): (i64, i64) = with_connection(|conn| {
            conn.query_row(
                "SELECT is_pinned, item_order FROM clipboard WHERE id = ?1",
                params![id_b],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
        })
        .unwrap();
        assert_eq!((p, order), (1, 1), "置顶桶 max=0 -> item_order=1");

        // 再置顶一条 -> order = max(pinned)+1 = 2
        let pinned2 = toggle_pin_clipboard_item(id_a).unwrap();
        assert_eq!(pinned2, true);
        let order_a: i64 = with_connection(|conn| {
            conn.query_row(
                "SELECT item_order FROM clipboard WHERE id = ?1",
                params![id_a],
                |r| r.get(0),
            )
        })
        .unwrap();
        assert_eq!(order_a, 2);

        // 取消置顶 -> 非置顶桶（此时为空）max+1 = 1
        let unpinned = toggle_pin_clipboard_item(id_a).unwrap();
        assert_eq!(unpinned, false);
        let (p, order): (i64, i64) = with_connection(|conn| {
            conn.query_row(
                "SELECT is_pinned, item_order FROM clipboard WHERE id = ?1",
                params![id_a],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
        })
        .unwrap();
        assert_eq!((p, order), (0, 1));

        // 不存在的 id -> Err（query_row 未命中 → QueryReturnedNoRows）
        assert_eq!(
            toggle_pin_clipboard_item(99999).unwrap_err(),
            "数据库操作失败: Query returned no rows"
        );
    });
}

/// b_pin_bucket_ordering: 查询按 is_pinned DESC, item_order DESC, updated_at DESC 排序；
/// 跨桶移动是 no-op；桶内移动按 reorder 语义交换。
#[test]
fn query_orders_pinned_first_and_cross_bucket_move_is_noop() {
    with_test_db(|| {
        let id_a = seed_clipboard("a", "text", 100, false, None); // order 1
        let id_b = seed_clipboard("b", "text", 200, false, None); // order 2
        let id_c = seed_clipboard("c", "text", 300, false, None); // order 3
        let id_d = seed_clipboard("d", "text", 400, true, None); // pinned order 4 (max+1)

        let result = query_clipboard_items(QueryParams::default()).unwrap();
        let ids: Vec<i64> = result.items.iter().map(|i| i.id).collect();
        assert_eq!(ids, vec![id_d, id_c, id_b, id_a], "置顶优先，再按 item_order DESC");

        // 跨桶移动：from pinned(d) to unpinned(a) -> Ok 且顺序不变
        move_clipboard_item_by_id(id_d, id_a).unwrap();
        let orders: Vec<(i64, i64, i64)> = query_all("SELECT id, is_pinned, item_order FROM clipboard ORDER BY id", |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)));
        assert_eq!(
            orders,
            vec![(id_a, 0, 1), (id_b, 0, 2), (id_c, 0, 3), (id_d, 1, 4)],
            "跨桶移动不得重排"
        );

        // 桶内移动：把 c(order3, idx1) 移到 a(order1, idx2)
        // 全序 [d(p1), c(3), b(2), a(1)] -> from_idx=1, to_idx=3
        // from_idx < to_idx: items[2..=3] order+1 -> b:2->3, a:1->2; c 取 target_order=1
        move_clipboard_item_by_id(id_c, id_a).unwrap();
        let orders: Vec<(i64, i64)> = query_all("SELECT id, item_order FROM clipboard WHERE is_pinned = 0 ORDER BY id", |r| Ok((r.get(0)?, r.get(1)?)));
        assert_eq!(orders, vec![(id_a, 2), (id_b, 3), (id_c, 1)]);
        // 新排序：d(p1), b(3), a(2), c(1)
        let result = query_clipboard_items(QueryParams::default()).unwrap();
        let ids: Vec<i64> = result.items.iter().map(|i| i.id).collect();
        assert_eq!(ids, vec![id_d, id_b, id_a, id_c]);
    });
}

/// b_position_count_uuid: 计数、0 基位置、uuid 生成幂等与按 uuid 反查。
#[test]
fn position_count_and_uuid_helpers() {
    with_test_db(|| {
        seed_clipboard("a", "text", 100, false, None);
        let mid = seed_clipboard("b", "text", 200, false, None);
        seed_clipboard("c", "text", 300, false, None);

        assert_eq!(get_clipboard_count().unwrap(), 3);
        let pos = get_clipboard_item_position(mid).unwrap();
        assert_eq!(pos, Some(1), "c(0), b(1), a(2) 的 0 基位置");

        let uuid1 = ensure_clipboard_item_uuid(mid).unwrap();
        assert!(!uuid1.trim().is_empty());
        let uuid2 = ensure_clipboard_item_uuid(mid).unwrap();
        assert_eq!(uuid1, uuid2, "uuid 生成必须幂等");
        assert_eq!(get_clipboard_item_id_by_uuid(&uuid1).unwrap(), Some(mid));
        assert_eq!(get_clipboard_item_id_by_uuid("no-such-uuid").unwrap(), None);
        assert_eq!(
            ensure_clipboard_item_uuid(99999).unwrap_err(),
            "生成 uuid 失败",
            "不存在的行：UPDATE 0 行后回读 None → 报生成失败"
        );
    });
}

/// b_paste_counts: 粘贴计数 +1；批量 +1；空输入 no-op；不存在 id 不报错。
#[test]
fn paste_count_increments_single_batch_and_empty() {
    with_test_db(|| {
        let id1 = seed_clipboard("a", "text", 100, false, None);
        let id2 = seed_clipboard("b", "text", 200, false, None);

        increment_paste_count(id1).unwrap();
        increment_paste_counts(&[id1, id2]).unwrap();
        increment_paste_counts(&[]).unwrap();
        increment_paste_count(99999).unwrap(); // 不存在 id 静默

        let counts: Vec<(i64, i64)> = query_all("SELECT id, paste_count FROM clipboard ORDER BY id", |r| Ok((r.get(0)?, r.get(1)?)));
        assert_eq!(counts, vec![(id1, 2), (id2, 1)]);
    });
}

/// b_clipboard_data_upsert: raw 格式按 (target_kind, target_id, format_name) upsert，
/// 查询按 format_order ASC, id ASC；删除按 target / 按 kind。
#[test]
fn clipboard_data_upsert_get_delete() {
    with_test_db(|| {
        // 空输入 -> Ok 且不写库
        save_clipboard_data_items("clipboard", "1", &[]).unwrap();
        assert_eq!(get_clipboard_data_items("clipboard", "1").unwrap().len(), 0);

        let seeds = vec![
            ClipboardDataSeed {
                format_name: "text/plain".into(),
                raw_data: b"abc".to_vec(),
                is_primary: true,
                format_order: 1,
            },
            ClipboardDataSeed {
                format_name: "text/html".into(),
                raw_data: b"<b>x</b>".to_vec(),
                is_primary: false,
                format_order: 0,
            },
        ];
        save_clipboard_data_items("clipboard", "1", &seeds).unwrap();
        let items = get_clipboard_data_items("clipboard", "1").unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].format_name, "text/html", "format_order ASC 优先");
        assert_eq!(items[0].raw_data, b"<b>x</b>");
        assert_eq!(items[0].is_primary, false);
        assert_eq!(items[1].format_name, "text/plain");
        assert_eq!(items[1].is_primary, true);

        // 重复保存同 format_name -> upsert 覆盖，不新增
        save_clipboard_data_items(
            "clipboard",
            "1",
            &[ClipboardDataSeed {
                format_name: "text/plain".into(),
                raw_data: b"zzz".to_vec(),
                is_primary: true,
                format_order: 5,
            }],
        )
        .unwrap();
        let items = get_clipboard_data_items("clipboard", "1").unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].raw_data, b"zzz");
        assert_eq!(items[1].format_order, 5);

        // 按 target 删除
        delete_clipboard_data_items("clipboard", "1").unwrap();
        assert_eq!(get_clipboard_data_items("clipboard", "1").unwrap().len(), 0);
        // 按 kind 删除
        save_clipboard_data_items("clipboard", "9", &seeds).unwrap();
        delete_clipboard_data_items_by_kind("clipboard").unwrap();
        assert_eq!(get_clipboard_data_items("clipboard", "9").unwrap().len(), 0);
    });
}

/// b_update_clears_raw_formats: 内容或 html 变化时清除 raw 格式；未变化时保留；
/// 不存在 id 返回错误。
#[test]
fn update_item_clears_raw_formats_only_on_change() {
    with_test_db(|| {
        let id = seed_clipboard("old", "text", 100, false, None);
        let seed = || vec![ClipboardDataSeed {
            format_name: "text/plain".into(),
            raw_data: b"old".to_vec(),
            is_primary: true,
            format_order: 0,
        }];
        save_clipboard_data_items("clipboard", &id.to_string(), &seed()).unwrap();

        // 内容变化 -> 清除
        update_clipboard_item(id, "new".into(), None).unwrap();
        assert!(get_clipboard_data_items("clipboard", &id.to_string()).unwrap().is_empty());

        // 内容未变 -> 保留
        save_clipboard_data_items("clipboard", &id.to_string(), &seed()).unwrap();
        update_clipboard_item(id, "new".into(), None).unwrap();
        assert_eq!(get_clipboard_data_items("clipboard", &id.to_string()).unwrap().len(), 1);

        // html 变化 -> 清除
        save_clipboard_data_items("clipboard", &id.to_string(), &seed()).unwrap();
        update_clipboard_item(id, "new".into(), Some("<p>new</p>".into())).unwrap();
        assert!(get_clipboard_data_items("clipboard", &id.to_string()).unwrap().is_empty());

        // html 相同 -> 保留
        save_clipboard_data_items("clipboard", &id.to_string(), &seed()).unwrap();
        update_clipboard_item(id, "new".into(), Some("<p>new</p>".into())).unwrap();
        assert_eq!(get_clipboard_data_items("clipboard", &id.to_string()).unwrap().len(), 1);

        // 不存在 id -> Err（query_row 未命中 → QueryReturnedNoRows）
        assert_eq!(
            update_clipboard_item(99999, "x".into(), None).unwrap_err(),
            "数据库操作失败: Query returned no rows"
        );
    });
}

/// b_pagination_has_more: 分页 has_more 边界与空库分页。
#[test]
fn query_pagination_has_more_boundaries() {
    with_test_db(|| {
        // 空库
        let result = query_clipboard_items(QueryParams::default()).unwrap();
        assert_eq!(result.total_count, 0);
        assert!(result.items.is_empty());
        assert!(!result.has_more);

        for i in 0..5 {
            seed_clipboard(&format!("c{}", i), "text", 100 + i, false, None);
        }
        // offset=0 limit=2 -> 2 条，has_more=true
        let r = query_clipboard_items(QueryParams {
            offset: 0,
            limit: 2,
            search: None,
            content_type: None,
        })
        .unwrap();
        assert_eq!(r.items.len(), 2);
        assert_eq!(r.has_more, true);
        assert_eq!(r.offset, 0);
        assert_eq!(r.limit, 2);
        // offset=4 limit=2 -> 1 条，has_more=false
        let r = query_clipboard_items(QueryParams {
            offset: 4,
            limit: 2,
            search: None,
            content_type: None,
        })
        .unwrap();
        assert_eq!(r.items.len(), 1);
        assert_eq!(r.has_more, false);
        assert_eq!(r.total_count, 5);
    });
}

/// b_webdav_records: 无 uuid 行用 id 字符串兜底；无 source_device_id 用请求方
/// device_id；own-records 只含本设备行；states 只含非空 uuid。
#[test]
fn webdav_records_uuid_and_device_fallbacks() {
    with_test_db(|| {
        let id1 = seed_clipboard("a", "text", 100, false, None);
        seed_clipboard("b", "text", 200, false, Some("u2"));
        with_connection(|conn| {
            conn.execute(
                "INSERT INTO clipboard (content, content_type, item_order, uuid, source_device_id, created_at, updated_at)
                 VALUES ('c', 'text', 3, 'u3', 'dev-other', 300, 300)",
                [],
            )?;
            Ok(())
        })
        .unwrap();

        let records = webdav_list_history_records("dev-local").unwrap();
        let rec1 = records.iter().find(|r| r.content == "a").unwrap();
        assert_eq!(rec1.uuid, id1.to_string(), "无 uuid 行用 id 字符串");
        assert_eq!(rec1.source_device_id, "dev-local", "无 device 行用请求方 device_id");
        let rec3 = records.iter().find(|r| r.content == "c").unwrap();
        assert_eq!(rec3.source_device_id, "dev-other", "有 device 行保留原值");

        let own = webdav_list_own_history_records("dev-local").unwrap();
        let contents: Vec<&str> = own.iter().map(|r| r.content.as_str()).collect();
        assert!(contents.contains(&"a") && contents.contains(&"b"));
        assert!(!contents.contains(&"c"), "其他设备行不得出现在 own 列表");

        let states = webdav_history_record_states().unwrap();
        assert_eq!(states.len(), 2, "只统计非空 uuid（u2、u3）");
        assert_eq!(states.get("u2"), Some(&200));
        assert_eq!(states.get("u3"), Some(&300));
        assert!(!states.contains_key(&id1.to_string()), "无 uuid 行不得进入 states");
    });
}

/// b_webdav_signature: webdav_local_sync_parts_signature 输出 "count:max(updated_at)"。
#[test]
fn webdav_sync_signature_counts_and_max() {
    with_test_db(|| {
        let sig = webdav_local_sync_parts_signature().unwrap();
        assert_eq!(
            sig,
            WebdavLocalSyncSignature {
                clipboard: "0:0".into(),
                favorites: "0:0".into(),
                groups: "0:0".into(),
                tombstones: "0:0".into(),
            }
        );

        seed_clipboard("a", "text", 100, false, None);
        seed_clipboard("b", "text", 500, false, None);
        seed_favorite("f1", "t", "c", "全部", 300);
        seed_group("g1", "ti ti-folder", "#dc2626", 1);
        record_tombstone(COLLECTION_FAVORITES, "f-x", "dev-x", 700);
        record_tombstone(COLLECTION_GROUPS, "g-x", "dev-x", 600);

        let sig = webdav_local_sync_parts_signature().unwrap();
        assert_eq!(sig.clipboard, "2:500");
        assert_eq!(sig.favorites, "1:300");
        assert_eq!(sig.groups, "1:1", "groups 用 groups 表 updated_at");
        assert_eq!(sig.tombstones, "2:700", "tombstones 用 max(deleted_at)");
    });
}

// =====================================================================
// favorites.rs
// =====================================================================

/// b_favorite_dedup: add_clipboard_to_favorites 重复调用返回同一收藏（不重复插入），
/// 新收藏 id=剪贴板 uuid、group='全部'、title=''、复制内容/char_count/raw 格式。
#[test]
fn add_clipboard_to_favorites_dedupes_on_second_call() {
    with_test_db(|| {
        let id = with_connection(|conn| {
            conn.execute(
                "INSERT INTO clipboard (content, html_content, content_type, item_order, uuid, created_at, updated_at, char_count)
                 VALUES ('clip-content', '<p>clip</p>', 'rich_text', 1, 'clip-uuid-1', 100, 100, 12)",
                [],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .unwrap();
        save_clipboard_data_items(
            "clipboard",
            &id.to_string(),
            &[ClipboardDataSeed {
                format_name: "text/plain".into(),
                raw_data: b"clip-content".to_vec(),
                is_primary: true,
                format_order: 0,
            }],
        )
        .unwrap();

        let fav1 = add_clipboard_to_favorites(id, None).unwrap();
        assert_eq!(fav1.id, "clip-uuid-1", "收藏 id 使用剪贴板 uuid");
        assert_eq!(fav1.group_name, "全部");
        assert_eq!(fav1.title, "");
        assert_eq!(fav1.content, "clip-content");
        assert_eq!(fav1.html_content.as_deref(), Some("<p>clip</p>"));
        assert_eq!(fav1.content_type, "rich_text");
        assert_eq!(fav1.char_count, Some(12), "char_count 从剪贴板行复制");
        assert_eq!(fav1.paste_count, 0);

        // raw 格式复制为 target_kind='favorite'
        let raw = get_clipboard_data_items("favorite", &fav1.id).unwrap();
        assert_eq!(raw.len(), 1);
        assert_eq!(raw[0].format_name, "text/plain");
        assert_eq!(raw[0].raw_data, b"clip-content");

        // 第二次调用 -> 同一收藏，不重复插入
        let fav2 = add_clipboard_to_favorites(id, None).unwrap();
        assert_eq!(fav2.id, fav1.id);
        assert_eq!(favorites_row_count(), 1);
    });
}

/// b_favorites_query: query_favorites 的 group/search/content_type 过滤、分页、
/// 搜索关键词截断、计数（含 '全部' 语义）。
#[test]
fn query_favorites_filters_group_search_type_and_truncates() {
    with_test_db(|| {
        seed_favorite("f1", "Needle doc", "hello", "work", 100);
        seed_favorite("f2", "other", "needle in hay", "全部", 200);
        seed_favorite("f3", "pic", "img-content", "work", 300);
        // f4: 长文本触发搜索截断
        with_connection(|conn| {
            conn.execute(
                "INSERT INTO favorites (id, title, content, content_type, group_name, item_order, created_at, updated_at, char_count)
                 VALUES ('f4', 't', ?1, 'text', '全部', 4, 400, 400, NULL)",
                params![format!("{}needle{}", "N".repeat(3000), "M".repeat(3000))],
            )?;
            Ok(())
        })
        .unwrap();

        // group 过滤
        let r = query_favorites(FavoritesQueryParams {
            offset: 0,
            limit: 50,
            group_name: Some("work".into()),
            search: None,
            content_type: None,
        })
        .unwrap();
        assert_eq!(r.total_count, 2);
        assert!(r.items.iter().all(|i| i.group_name == "work"));

        // '全部' = 不过滤
        let r = query_favorites(FavoritesQueryParams {
            offset: 0,
            limit: 50,
            group_name: Some("全部".into()),
            search: None,
            content_type: None,
        })
        .unwrap();
        assert_eq!(r.total_count, 4);

        // search 命中 title 或 content
        let r = query_favorites(FavoritesQueryParams {
            offset: 0,
            limit: 50,
            group_name: None,
            search: Some("needle".into()),
            content_type: None,
        })
        .unwrap();
        assert_eq!(r.total_count, 3, "f1(title) + f2(content) + f4(content)");
        // 搜索截断：f4 内容 ≤1600 字节且含关键词
        let f4 = r.items.iter().find(|i| i.id == "f4").unwrap();
        assert!(f4.content.len() <= MAX_CONTENT_LENGTH as usize);
        assert!(f4.content.contains("needle"));
        // char_count 懒计算
        assert_eq!(f4.char_count, Some(6006));

        // content_type 过滤
        let r = query_favorites(FavoritesQueryParams {
            offset: 0,
            limit: 50,
            group_name: None,
            search: None,
            content_type: Some("image".into()),
        })
        .unwrap();
        assert_eq!(r.total_count, 0, "无 image 收藏");
        // 分页 has_more
        let r = query_favorites(FavoritesQueryParams {
            offset: 0,
            limit: 2,
            group_name: None,
            search: None,
            content_type: None,
        })
        .unwrap();
        assert_eq!(r.items.len(), 2);
        assert_eq!(r.has_more, true);

        // 计数
        assert_eq!(get_favorites_count(None).unwrap(), 4);
        assert_eq!(get_favorites_count(Some("全部".into())).unwrap(), 4);
        assert_eq!(get_favorites_count(Some("work".into())).unwrap(), 2);
        assert_eq!(get_favorites_count(Some("nonexistent".into())).unwrap(), 0);
    });
}

/// b_favorites_crud: add_favorite 序号/字符数；update_favorite 内容/分组/raw 格式清理；
/// move_favorite_to_group；delete_favorite/delete_favorites 记录 tombstone。
#[test]
fn favorites_crud_orders_counts_update_move_delete() {
    with_test_db(|| {
        // add_favorite
        let f = add_favorite("Title".into(), "你好world".into(), None).unwrap();
        assert_eq!(f.group_name, "全部");
        assert_eq!(f.content_type, "text");
        assert_eq!(f.char_count, Some(7), "chars().count() 按码点计");
        assert_eq!(f.item_order, 1);
        assert!(!f.id.is_empty());
        let f2 = add_favorite("T2".into(), "x".into(), Some("work".into())).unwrap();
        assert_eq!(f2.item_order, 2, "item_order = max+1");
        assert_eq!(f2.group_name, "work");

        // 字符数计数边界：空内容 -> Some(0)
        let f3 = add_favorite("empty".into(), String::new(), None).unwrap();
        assert_eq!(f3.char_count, Some(0));

        // update_favorite：内容变化 -> char_count 重算、raw 格式清除
        save_clipboard_data_items(
            "favorite",
            &f.id,
            &[ClipboardDataSeed {
                format_name: "text/plain".into(),
                raw_data: b"hello".to_vec(),
                is_primary: true,
                format_order: 0,
            }],
        )
        .unwrap();
        let updated =
            update_favorite(f.id.clone(), "T2".into(), "新内容".into(), None, None).unwrap();
        assert_eq!(updated.content, "新内容");
        assert_eq!(updated.char_count, Some(3));
        assert!(get_clipboard_data_items("favorite", &f.id).unwrap().is_empty());

        // 内容未变 -> raw 格式保留
        save_clipboard_data_items(
            "favorite",
            &f.id,
            &[ClipboardDataSeed {
                format_name: "text/plain".into(),
                raw_data: b"hello".to_vec(),
                is_primary: true,
                format_order: 0,
            }],
        )
        .unwrap();
        update_favorite(f.id.clone(), "T2".into(), "新内容".into(), None, None).unwrap();
        assert_eq!(get_clipboard_data_items("favorite", &f.id).unwrap().len(), 1);

        // 分组变化
        let updated = update_favorite(
            f.id.clone(),
            "T2".into(),
            "新内容".into(),
            Some("dev".into()),
            None,
        )
        .unwrap();
        assert_eq!(updated.group_name, "dev");

        // 不存在 id -> Err（optional() 未命中 → QueryReturnedNoRows）
        assert_eq!(
            update_favorite("no-such".into(), "t".into(), "c".into(), None, None).unwrap_err(),
            "数据库操作失败: Query returned no rows"
        );

        // move_favorite_to_group
        move_favorite_to_group(f2.id.clone(), "dev".into()).unwrap();
        let g: String = with_connection(|conn| {
            conn.query_row(
                "SELECT group_name FROM favorites WHERE id = ?1",
                params![&f2.id],
                |r| r.get(0),
            )
        })
        .unwrap();
        assert_eq!(g, "dev");
        // 同组移动 -> Ok 且不报错
        move_favorite_to_group(f2.id.clone(), "dev".into()).unwrap();

        // delete_favorite -> tombstone
        delete_favorite(f3.id.clone()).unwrap();
        assert!(tombstone_deleted_at(COLLECTION_FAVORITES, &f3.id).is_some());
        assert_eq!(favorites_row_count(), 2);

        // delete_favorites 批量（含重复 id 去重）
        delete_favorites(&[f.id.clone(), f.id.clone(), f2.id.clone()]).unwrap();
        assert_eq!(favorites_row_count(), 0);
    });
}

/// b_move_reorder（favorites）: move_favorite_item 桶内重排语义。
#[test]
fn move_favorite_reorders_within_bucket() {
    with_test_db(|| {
        let f1 = add_favorite("a".into(), "a".into(), None).unwrap(); // order 1
        let f2 = add_favorite("b".into(), "b".into(), None).unwrap(); // order 2
        let f3 = add_favorite("c".into(), "c".into(), None).unwrap(); // order 3
        // 从 idx0(f3) 移到 idx2(f1): items[1..=2] order+1 -> f2:2->3, f1:1->2; f3 取 target=1
        move_favorite_item(f3.id.clone(), f1.id.clone()).unwrap();
        let orders: HashMap<String, i64> = query_all("SELECT id, item_order FROM favorites", |r| Ok((r.get(0)?, r.get(1)?)))
            .into_iter()
            .collect();
        assert_eq!(orders.get(&f1.id), Some(&2), "f1 应移到目标位置");
        assert_eq!(orders.get(&f2.id), Some(&3), "f2 应后移一位");
        assert_eq!(orders.get(&f3.id), Some(&1), "f3 应取目标位置 order");
        // 相同 id -> Ok no-op
        move_favorite_item(f1.id.clone(), f1.id.clone()).unwrap();
        // 不存在的 id -> Err（InvalidParameterName 携带具体 id）
        assert_eq!(
            move_favorite_item("no-such".into(), f1.id.clone()).unwrap_err(),
            "数据库操作失败: Invalid parameter name: ID no-such 不存在"
        );
    });
}

// =====================================================================
// groups.rs
// =====================================================================

/// b_group_protection: delete_group('全部') 报错，无 tombstone，收藏不动。
#[test]
fn delete_group_all_is_protected() {
    with_test_db(|| {
        seed_favorite("f1", "t", "c", "全部", 100);
        let err = delete_group("全部".into()).unwrap_err();
        assert!(err.contains("不能删除"), "应报错: {}", err);
        assert!(tombstone_deleted_at(COLLECTION_GROUPS, "全部").is_none());
        let g: String = with_connection(|conn| {
            conn.query_row("SELECT group_name FROM favorites WHERE id = 'f1'", [], |r| r.get(0))
        })
        .unwrap();
        assert_eq!(g, "全部", "收藏不得被移动");
    });
}

/// b_group_delete_reassigns: delete_group 记录 tombstone、收藏改回 '全部'、分组行删除；
/// 不存在的分组删除不记录 tombstone（exists=0）。
#[test]
fn delete_group_reassigns_favorites_and_records_tombstone() {
    with_test_db(|| {
        add_group("work".into(), "ti ti-folder".into(), "#dc2626".into()).unwrap();
        seed_favorite("f1", "t", "c", "work", 100);
        seed_favorite("f2", "t2", "c2", "全部", 200);

        delete_group("work".into()).unwrap();

        assert!(tombstone_deleted_at(COLLECTION_GROUPS, "work").is_some());
        let groups_count: i64 = with_connection(|conn| {
            conn.query_row("SELECT COUNT(*) FROM groups WHERE name = 'work'", [], |r| r.get(0))
        })
        .unwrap();
        assert_eq!(groups_count, 0);
        let g: String = with_connection(|conn| {
            conn.query_row("SELECT group_name FROM favorites WHERE id = 'f1'", [], |r| r.get(0))
        })
        .unwrap();
        assert_eq!(g, "全部", "收藏应改回 '全部'");

        // 不存在的分组：Ok，无 tombstone
        delete_group("ghost".into()).unwrap();
        assert!(tombstone_deleted_at(COLLECTION_GROUPS, "ghost").is_none());
    });
}

/// b_group_rename_cascade: update_group 重命名级联收藏；重命名为已存在名报错；
/// 同名更新 Ok。
#[test]
fn update_group_rename_cascades_and_rejects_duplicates() {
    with_test_db(|| {
        add_group("work".into(), "ti ti-folder".into(), "#dc2626".into()).unwrap();
        add_group("other".into(), "ti ti-folder".into(), "#dc2626".into()).unwrap();
        seed_favorite("f1", "t", "c", "work", 100);

        // 重命名 -> 收藏级联
        let g = update_group(
            "work".into(),
            "dev".into(),
            "ti ti-star".into(),
            "#00ff00".into(),
        )
        .unwrap();
        assert_eq!(g.name, "dev");
        assert_eq!(g.icon, "ti ti-star");
        assert_eq!(g.color, "#00ff00");
        let fav_group: String = with_connection(|conn| {
            conn.query_row("SELECT group_name FROM favorites WHERE id = 'f1'", [], |r| r.get(0))
        })
        .unwrap();
        assert_eq!(fav_group, "dev", "重命名必须级联收藏");
        assert_eq!(g.item_count, 1);

        // 重命名为已存在 -> Err
        let err = update_group("dev".into(), "other".into(), "".into(), "".into()).unwrap_err();
        assert!(err.contains("已存在"), "应报错: {}", err);
        // 分组仍存在且收藏未变
        let fav_group: String = with_connection(|conn| {
            conn.query_row("SELECT group_name FROM favorites WHERE id = 'f1'", [], |r| r.get(0))
        })
        .unwrap();
        assert_eq!(fav_group, "dev");

        // 同名更新 -> Ok
        let g = update_group("dev".into(), "dev".into(), "".into(), "".into()).unwrap();
        assert_eq!(g.name, "dev");
    });
}

/// b_groups_crud: add_group 序号/颜色/图标规范化、重名报错；get_all_groups 排序与
/// item_count；reorder_groups。
#[test]
fn add_group_normalizes_and_reorders() {
    with_test_db(|| {
        let g1 = add_group("g1".into(), "".into(), "0".into()).unwrap();
        assert_eq!(g1.icon, "ti ti-folder", "空图标回退默认");
        assert_eq!(g1.color, "#dc2626", "空/0 颜色回退默认");
        assert_eq!(g1.order, 1);
        assert_eq!(g1.item_count, 0);
        let g2 = add_group("g2".into(), " ti ti-star ".into(), "#FF0000".into()).unwrap();
        assert_eq!(g2.icon, "ti ti-star");
        assert_eq!(g2.color, "#ff0000", "颜色转小写");
        assert_eq!(g2.order, 2);

        // 重名 -> Err
        let err = add_group("g1".into(), "".into(), "".into()).unwrap_err();
        assert!(err.contains("已存在"));

        // get_all_groups 按 (order_index, name) 排序，item_count 统计收藏
        seed_favorite("f1", "t", "c", "g2", 100);
        let groups = get_all_groups().unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "g1");
        assert_eq!(groups[1].name, "g2");
        assert_eq!(groups[1].item_count, 1);

        // reorder_groups
        reorder_groups(vec![("g2".into(), 0), ("g1".into(), 1)]).unwrap();
        let groups = get_all_groups().unwrap();
        assert_eq!(groups[0].name, "g2");
        assert_eq!(groups[0].order, 0);
        assert_eq!(groups[1].name, "g1");
        assert_eq!(groups[1].order, 1);
    });
}

/// b_groups_crud（webdav 侧）: webdav_list_groups 规范化图标/颜色与 device 兜底；
/// lan_save_groups 插入并规范化颜色，LWW 跳过相同 updated_at。
#[test]
fn webdav_list_and_save_groups_normalize_icon_color_device() {
    with_test_db(|| {
        seed_group("g1", "", "", 1);
        let groups = webdav_list_groups("dev-local").unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "g1");
        assert_eq!(groups[0].icon, "ti ti-folder", "空图标回退");
        assert_eq!(groups[0].color, "#dc2626", "空颜色回退");
        assert_eq!(groups[0].source_device_id, "dev-local", "无 device 用请求方");

        // lan_save_groups 插入远端分组（颜色规范化 + tombstone 检查通过）
        let incoming = CloudGroup {
            name: "remote-g".into(),
            icon: "".into(),
            color: "#ABC".into(),
            order: 2,
            source_device_id: "dev-r".into(),
            created_at: 1,
            updated_at: 1,
        };
        let changed = lan_save_groups(&[incoming]).unwrap();
        assert_eq!(changed.len(), 1);
        let groups = webdav_list_groups("dev-local").unwrap();
        let rg = groups.iter().find(|g| g.name == "remote-g").unwrap();
        assert_eq!(rg.icon, "ti ti-folder");
        assert_eq!(rg.color, "#aabbcc", "3 位 hex 扩展并小写");
        assert_eq!(rg.source_device_id, "dev-r");
        // 相同 updated_at 的重复推送 -> skip（不回执）
        let incoming = CloudGroup {
            name: "remote-g".into(),
            icon: "".into(),
            color: "#ABC".into(),
            order: 2,
            source_device_id: "dev-r".into(),
            created_at: 1,
            updated_at: 1,
        };
        let changed = lan_save_groups(&[incoming]).unwrap();
        assert!(changed.is_empty(), "LWW：旧/相同 updated_at 不应用");
    });
}

// =====================================================================
// tombstones.rs
// =====================================================================

/// b_tombstone_monotonic_upsert: record_sync_tombstone 只允许 deleted_at 单调不回退；
/// 空 collection/item_id 为 no-op。
#[test]
fn tombstone_record_is_monotonic() {
    with_test_db(|| {
        record_tombstone(COLLECTION_HISTORY, "u1", "dev-x", 100);
        assert_eq!(tombstone_deleted_at(COLLECTION_HISTORY, "u1"), Some(100));
        // 更新为更新值
        record_tombstone(COLLECTION_HISTORY, "u1", "dev-y", 200);
        assert_eq!(tombstone_deleted_at(COLLECTION_HISTORY, "u1"), Some(200));
        let (src, created): (String, i64) = with_connection(|conn| {
            conn.query_row(
                "SELECT source_device_id, created_at FROM sync_tombstones
                 WHERE collection = 'history' AND item_id = 'u1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
        })
        .unwrap();
        assert_eq!(src, "dev-y");
        assert_eq!(created, 200, "created_at 与 deleted_at 同步更新");
        // 旧值不回退
        record_tombstone(COLLECTION_HISTORY, "u1", "dev-z", 150);
        assert_eq!(tombstone_deleted_at(COLLECTION_HISTORY, "u1"), Some(200));
        // 空值 no-op
        record_tombstone("  ", "u1", "dev-z", 999);
        record_tombstone(COLLECTION_HISTORY, "", "dev-z", 999);
        assert_eq!(tombstone_deleted_at(COLLECTION_HISTORY, "u1"), Some(200));
    });
}

/// b_tombstone_lww_local_edit_wins: apply_sync_tombstones 只删除 updated_at <= deleted_at
/// 的行；更新的本地编辑获胜（保留）；组 '全部' 永不删除；未知 collection 忽略。
#[test]
fn apply_tombstones_respects_lww_and_protects_all_group() {
    with_test_db(|| {
        // 历史：行比 tombstone 新 -> 保留
        seed_clipboard("local-new", "text", 5000, false, Some("u1"));
        let t = SyncTombstone {
            collection: COLLECTION_HISTORY.into(),
            item_id: "u1".into(),
            source_device_id: "dev-x".into(),
            deleted_at: 4000,
            created_at: 4000,
        };
        let report = apply_sync_tombstones(&[t]).unwrap();
        assert_eq!(report.history, 0, "更新的本地编辑必须保留");
        assert_eq!(clipboard_row_count(), 1);

        // 历史：tombstone 更新 -> 删除行与 raw 数据
        let id = seed_clipboard("old", "text", 3000, false, Some("u2"));
        save_clipboard_data_items(
            "clipboard",
            &id.to_string(),
            &[ClipboardDataSeed {
                format_name: "text/plain".into(),
                raw_data: b"old".to_vec(),
                is_primary: true,
                format_order: 0,
            }],
        )
        .unwrap();
        let t = SyncTombstone {
            collection: COLLECTION_HISTORY.into(),
            item_id: "u2".into(),
            source_device_id: "dev-x".into(),
            deleted_at: 4000,
            created_at: 4000,
        };
        let report = apply_sync_tombstones(&[t]).unwrap();
        assert_eq!(report.history, 1);
        assert_eq!(clipboard_row_count(), 1);
        assert!(get_clipboard_data_items("clipboard", &id.to_string()).unwrap().is_empty());

        // 收藏同理
        seed_favorite("f1", "t", "c", "全部", 3000);
        let t = SyncTombstone {
            collection: COLLECTION_FAVORITES.into(),
            item_id: "f1".into(),
            source_device_id: "dev-x".into(),
            deleted_at: 4000,
            created_at: 4000,
        };
        let report = apply_sync_tombstones(&[t]).unwrap();
        assert_eq!(report.favorites, 1);
        assert_eq!(favorites_row_count(), 0);

        // 组 '全部' 受保护（updated_at=1 远早于 tombstone，仍不得删除）
        seed_group("全部", "", "", 1);
        let t = SyncTombstone {
            collection: COLLECTION_GROUPS.into(),
            item_id: "全部".into(),
            source_device_id: "dev-x".into(),
            deleted_at: 999999,
            created_at: 999999,
        };
        let report = apply_sync_tombstones(&[t]).unwrap();
        assert_eq!(report.groups, 0);
        let n: i64 = with_connection(|conn| {
            conn.query_row("SELECT COUNT(*) FROM groups WHERE name = '全部'", [], |r| r.get(0))
        })
        .unwrap();
        assert_eq!(n, 1, "'全部' 分组不得被 tombstone 删除");

        // 未知 collection 忽略
        let t = SyncTombstone {
            collection: "unknown".into(),
            item_id: "x".into(),
            source_device_id: "dev-x".into(),
            deleted_at: 1,
            created_at: 1,
        };
        let report = apply_sync_tombstones(&[t]).unwrap();
        assert_eq!(report.total(), 0);

        // 空输入 -> 默认报告
        let report = apply_sync_tombstones(&[]).unwrap();
        assert_eq!(report.total(), 0);
    });
}

/// b_tombstone_apply_report: 组 tombstone 删除时收藏改回 '全部'，报告计数正确。
#[test]
fn apply_group_tombstone_reassigns_favorites() {
    with_test_db(|| {
        // 组 updated_at=1（远早于 tombstone.deleted_at=5000）-> 组被删除
        seed_group("work", "", "", 1);
        seed_favorite("f1", "t", "c", "work", 100);
        let t = SyncTombstone {
            collection: COLLECTION_GROUPS.into(),
            item_id: "work".into(),
            source_device_id: "dev-x".into(),
            deleted_at: 5000,
            created_at: 5000,
        };
        let report = apply_sync_tombstones(&[t]).unwrap();
        assert_eq!(report.groups, 1);
        assert_eq!(report.total(), 1);
        let g: String = with_connection(|conn| {
            conn.query_row("SELECT group_name FROM favorites WHERE id = 'f1'", [], |r| r.get(0))
        })
        .unwrap();
        assert_eq!(g, "全部", "组删除时收藏必须改回 '全部'");
    });
}

/// b_tombstone_since_and_states: list_sync_tombstones_since 过滤与 ASC 排序；
/// sync_tombstone_states 键 "collection:item_id"。
#[test]
fn list_sync_tombstones_since_is_filtered_and_ordered() {
    with_test_db(|| {
        record_tombstone(COLLECTION_HISTORY, "u1", "dev-x", 100);
        record_tombstone(COLLECTION_FAVORITES, "f1", "dev-x", 300);
        record_tombstone(COLLECTION_GROUPS, "g1", "dev-x", 200);

        let all = list_sync_tombstones_since(None).unwrap();
        let dels: Vec<i64> = all.iter().map(|t| t.deleted_at).collect();
        assert_eq!(dels, vec![100, 200, 300], "按 deleted_at ASC");

        let since = list_sync_tombstones_since(Some(150)).unwrap();
        let dels: Vec<i64> = since.iter().map(|t| t.deleted_at).collect();
        assert_eq!(dels, vec![200, 300], "只返回 deleted_at > since");
        assert_eq!(since[0].collection, COLLECTION_GROUPS);

        let states = sync_tombstone_states().unwrap();
        assert_eq!(states.get(&tombstone_state_key(COLLECTION_HISTORY, "u1")), Some(&100));
        assert_eq!(states.get("history:u1"), Some(&100));
        assert_eq!(states.len(), 3);
    });
}

/// b_upsert_tombstones_monotonic: upsert_sync_tombstones 只接受更新的 deleted_at，
/// 跳过空 collection/item_id，changed 只含应用项。
#[test]
fn upsert_sync_tombstones_is_monotonic() {
    with_test_db(|| {
        let mk = |collection: &str, item: &str, deleted_at: i64| SyncTombstone {
            collection: collection.into(),
            item_id: item.into(),
            source_device_id: "dev-x".into(),
            deleted_at,
            created_at: deleted_at,
        };
        // 空输入
        let changed = upsert_sync_tombstones(&[]).unwrap();
        assert!(changed.is_empty());

        // 应用新 tombstone
        let changed = upsert_sync_tombstones(&[mk(COLLECTION_HISTORY, "u1", 100)]).unwrap();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].deleted_at, 100);

        // 更新的值应用
        let changed = upsert_sync_tombstones(&[mk(COLLECTION_HISTORY, "u1", 200)]).unwrap();
        assert_eq!(changed.len(), 1);
        assert_eq!(tombstone_deleted_at(COLLECTION_HISTORY, "u1"), Some(200));

        // 更旧的值跳过
        let changed = upsert_sync_tombstones(&[mk(COLLECTION_HISTORY, "u1", 150)]).unwrap();
        assert!(changed.is_empty());
        assert_eq!(tombstone_deleted_at(COLLECTION_HISTORY, "u1"), Some(200));

        // 空 collection/item_id 跳过
        let changed = upsert_sync_tombstones(&[mk("", "x", 1), mk(COLLECTION_HISTORY, "", 1)]).unwrap();
        assert!(changed.is_empty());
    });
}

/// b_restored_record_updated_at: 修复模式时间 = max(updated_at, now, deleted_at+1)；
/// 无 tombstone 时 = max(updated_at, now)。边界两侧断言。
#[test]
fn restored_record_updated_at_semantics() {
    let before = chrono::Utc::now().timestamp();
    let r1 = restored_record_updated_at(1000, None);
    let r2 = restored_record_updated_at(1000, Some(2000));
    let after = chrono::Utc::now().timestamp();
    assert!(r1 >= before && r1 <= after, "无 tombstone: max(updated_at, now)");
    assert!(r2 >= before && r2 <= after);
    assert!(r2 >= 2001, "有 tombstone: 必须 >= deleted_at + 1");
    // updated_at 主导分支
    let far_future = after + 100_000_000;
    let r3 = restored_record_updated_at(far_future, Some(1));
    assert_eq!(r3, far_future, "updated_at 主导时精确返回");
}

/// b_sync_filter_pure: 纯过滤函数（按 tombstone states 过滤 record/group/meta、
/// 筛选比远端新的 tombstone、状态键格式）。
#[test]
fn sync_filter_pure_functions() {
    let mut states = HashMap::new();
    states.insert("history:u1".to_string(), 100i64);
    states.insert("groups:g1".to_string(), 200i64);

    // filter_record_metas_not_deleted_by_states
    let metas = vec![
        CloudRecordMeta { uuid: "u1".into(), updated_at: 50, image_id: None },
        CloudRecordMeta { uuid: "u1".into(), updated_at: 100, image_id: None },
        CloudRecordMeta { uuid: "u1".into(), updated_at: 101, image_id: None },
        CloudRecordMeta { uuid: "u2".into(), updated_at: 50, image_id: None },
    ];
    let kept = filter_record_metas_not_deleted_by_states(COLLECTION_HISTORY, metas, &states);
    let kept_uuids: Vec<i64> = kept.iter().map(|m| m.updated_at).collect();
    assert_eq!(kept_uuids, vec![101, 50], "deleted_at < updated_at 才保留");

    // filter_groups_not_deleted_by_states
    let mk_group = |name: &str, updated_at: i64| CloudGroup {
        name: name.into(),
        icon: "".into(),
        color: "".into(),
        order: 1,
        source_device_id: "d".into(),
        created_at: 1,
        updated_at,
    };
    let groups = vec![
        mk_group("g1", 199),
        mk_group("g1", 200),
        mk_group("g1", 201),
        mk_group("g2", 50),
    ];
    let kept = filter_groups_not_deleted_by_states(groups, &states);
    let kept_updates: Vec<i64> = kept.iter().map(|g| g.updated_at).collect();
    assert_eq!(kept_updates, vec![201, 50]);

    // filter_records_not_deleted（走 DB states）
    with_test_db(|| {
        record_tombstone(COLLECTION_HISTORY, "u1", "dev-x", 100);
        let records = vec![
            make_record("u1", "old", 50),
            make_record("u1", "eq", 100),
            make_record("u1", "new", 101),
            make_record("u2", "none", 50),
        ];
        let kept = filter_records_not_deleted(COLLECTION_HISTORY, &records).unwrap();
        let kept_content: Vec<&str> = kept.iter().map(|r| r.content.as_str()).collect();
        assert_eq!(kept_content, vec!["new", "none"]);
    });

    // tombstones_newer_than_remote
    let mk_t = |collection: &str, item: &str, deleted_at: i64| SyncTombstone {
        collection: collection.into(),
        item_id: item.into(),
        source_device_id: "d".into(),
        deleted_at,
        created_at: deleted_at,
    };
    let tombstones = vec![
        mk_t(COLLECTION_HISTORY, "u1", 150),
        mk_t(COLLECTION_HISTORY, "u1", 100),
        mk_t(COLLECTION_HISTORY, "u2", 50),
    ];
    let kept = tombstones_newer_than_remote(tombstones, &states);
    let kept_dels: Vec<i64> = kept.iter().map(|t| t.deleted_at).collect();
    assert_eq!(kept_dels, vec![150, 50], "远端更旧或无远端状态的保留");

    // tombstone_state_key
    assert_eq!(tombstone_state_key("a", "b"), "a:b");
    assert_eq!(tombstone_state_key("history", "u1"), "history:u1");
}

// =====================================================================
// models.rs
// =====================================================================

/// b_pagination_has_more（纯逻辑）: has_more = offset + items.len() < total_count。
#[test]
fn paginated_result_has_more_is_exact() {
    let page: PaginatedResult<i64> = PaginatedResult::new(50, vec![1; 50], 0, 50);
    assert_eq!(page.has_more, false);
    assert_eq!(page.total_count, 50);
    let page: PaginatedResult<i64> = PaginatedResult::new(51, vec![1; 50], 0, 50);
    assert_eq!(page.has_more, true);
    let page: PaginatedResult<i64> = PaginatedResult::new(50, Vec::new(), 50, 50);
    assert_eq!(page.has_more, false, "最后一页恰好取完");
    let page: PaginatedResult<i64> = PaginatedResult::new(61, vec![1; 10], 50, 50);
    assert_eq!(page.has_more, true);
    let empty: PaginatedResult<i64> = PaginatedResult::new(0, Vec::new(), 0, 50);
    assert_eq!(empty.has_more, false);

    // 默认查询参数
    let d = QueryParams::default();
    assert_eq!((d.offset, d.limit), (0, 50));
    assert_eq!(d.search, None);
    assert_eq!(d.content_type, None);
    let d = FavoritesQueryParams::default();
    assert_eq!((d.offset, d.limit), (0, 50));
}
