use super::models::{ClipboardDataItem, ClipboardDataSeed, ClipboardItem, PaginatedResult, QueryParams};
use super::connection::{with_connection, MAX_CONTENT_LENGTH};
use crate::services::webdav_sync::types::{CloudRecord, CloudRecordMeta};
use crate::utils::{is_textual_content_type, truncate_string, truncate_around_keyword, truncate_html};
use rusqlite::{params, OptionalExtension};
use std::collections::{HashMap, HashSet};
use chrono;
use uuid::Uuid;

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

pub fn save_clipboard_data_items(
    target_kind: &str,
    target_id: &str,
    items: &[ClipboardDataSeed],
) -> Result<(), String> {
    if items.is_empty() {
        return Ok(());
    }

    with_connection(|conn| {
        let now = chrono::Local::now().timestamp();
        let tx = conn.unchecked_transaction()?;

        for item in items {
            tx.execute(
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

        tx.commit()?;
        Ok(())
    })
}

fn get_clipboard_data_items_by_target(
    target_kind: &str,
    target_id: &str,
) -> Result<Vec<ClipboardDataItem>, String> {
    with_connection(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, target_kind, target_id, format_name, raw_data, is_primary, format_order, created_at, updated_at
             FROM clipboard_data
             WHERE target_kind = ?1 AND target_id = ?2
             ORDER BY format_order ASC, id ASC",
        )?;

        let items = stmt
            .query_map(params![target_kind, target_id], |row| {
                Ok(ClipboardDataItem {
                    id: row.get(0)?,
                    target_kind: row.get(1)?,
                    target_id: row.get(2)?,
                    format_name: row.get(3)?,
                    raw_data: row.get(4)?,
                    is_primary: row.get::<_, i64>(5)? != 0,
                    format_order: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(items)
    })
}

pub fn get_clipboard_data_items(
    target_kind: &str,
    target_id: &str,
) -> Result<Vec<ClipboardDataItem>, String> {
    get_clipboard_data_items_by_target(target_kind, target_id)
}

pub fn delete_clipboard_data_items(target_kind: &str, target_id: &str) -> Result<(), String> {
    with_connection(|conn| {
        conn.execute(
            "DELETE FROM clipboard_data WHERE target_kind = ?1 AND target_id = ?2",
            params![target_kind, target_id],
        )?;
        Ok(())
    })
}

pub fn delete_clipboard_data_items_by_kind(target_kind: &str) -> Result<(), String> {
    with_connection(|conn| {
        conn.execute(
            "DELETE FROM clipboard_data WHERE target_kind = ?1",
            params![target_kind],
        )?;
        Ok(())
    })
}

// 异步更新缺失的字符数
pub fn update_missing_char_counts(items: Vec<(i64, String, String)>) {
    if items.is_empty() { return; }
    
    std::thread::spawn(move || {
        let _ = with_connection(|conn| {
            for (id, content, content_type) in items {
                if let Some(char_count) = calculate_char_count(&content, &content_type) {
                    conn.execute(
                        "UPDATE clipboard SET char_count = ?1 WHERE id = ?2",
                        params![char_count, id],
                    )?;
                }
            }
            Ok(())
        });
    });
}

// 按逗号拆分图片ID
fn split_image_ids(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim())
        .filter(|x| !x.is_empty())
        .map(|x| x.to_string())
        .collect()
}

// 检查图片ID是否仍被 clipboard 或 favorites 引用
fn is_image_id_referenced(conn: &rusqlite::Connection, image_id: &str) -> Result<bool, rusqlite::Error> {
    let exact = image_id;
    let p1 = format!("{},%", image_id);
    let p2 = format!("%,{},%", image_id);
    let p3 = format!("%,{}", image_id);

    let q = |table: &str| -> Result<bool, rusqlite::Error> {
        let sql = format!(
            "SELECT EXISTS(SELECT 1 FROM {} WHERE image_id = ?1 OR image_id LIKE ?2 OR image_id LIKE ?3 OR image_id LIKE ?4)",
            table
        );
        let exists: i64 = conn.query_row(&sql, params![exact, p1, p2, p3], |row| row.get(0))?;
        Ok(exists != 0)
    };

    Ok(q("clipboard")? || q("favorites")?)
}

// 删除图片文件
fn delete_image_files(image_ids: Vec<String>) -> Result<(), String> {
    if image_ids.is_empty() { return Ok(()); }
    let data_dir = crate::services::get_data_directory()?;
    let images_dir = data_dir.join("clipboard_images");
    for iid in image_ids {
        let p = images_dir.join(format!("{}.png", iid));
        if p.exists() {
            let _ = std::fs::remove_file(&p);
        }
    }
    Ok(())
}

// 分页查询剪贴板历史
pub fn query_clipboard_items(params: QueryParams) -> Result<PaginatedResult<ClipboardItem>, String> {
    let search_keyword = params.search.clone();
    let has_filter = search_keyword.as_ref().map(|s| !s.trim().is_empty()).unwrap_or(false)
        || params.content_type.as_ref().map(|t| t != "all").unwrap_or(false);
    
    with_connection(|conn| {
        let mut where_clauses = vec![];
        let mut query_params: Vec<Box<dyn rusqlite::ToSql>> = vec![];
        
        if let Some(ref search) = search_keyword {
            if !search.trim().is_empty() {
                where_clauses.push("content LIKE ?");
                let search_pattern = format!("%{}%", search);
                query_params.push(Box::new(search_pattern));
            }
        }
        
        if let Some(ref content_type) = params.content_type {
            if content_type != "all" {
                where_clauses.push("content_type LIKE ?");
                let pattern = format!("%{}%", content_type);
                query_params.push(Box::new(pattern));
            }
        }
        
        let where_clause = if where_clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", where_clauses.join(" AND "))
        };
        
        let total_count: i64 = if has_filter {
            let count_sql = format!("SELECT COUNT(*) FROM clipboard {}", where_clause);
            let count_params: Vec<Box<dyn rusqlite::ToSql>> = query_params.iter().map(|p| {
                let val: Box<dyn rusqlite::ToSql> = match p.as_ref().to_sql() {
                    Ok(rusqlite::types::ToSqlOutput::Borrowed(rusqlite::types::ValueRef::Text(s))) => {
                        Box::new(String::from_utf8_lossy(s).to_string())
                    }
                    _ => Box::new("")
                };
                val
            }).collect();
            conn.query_row(
                &count_sql,
                rusqlite::params_from_iter(count_params.iter().map(|p| p.as_ref())),
                |row| row.get(0)
            )?
        } else {
            conn.query_row("SELECT COUNT(*) FROM clipboard", [], |row| row.get(0))?
        };
        
        if total_count == 0 {
            return Ok(PaginatedResult::new(0, vec![], params.offset, params.limit));
        }
        
        let query_sql = format!(
            "SELECT id, uuid, source_device_id, is_remote, content, html_content, content_type, image_id, item_order, is_pinned, paste_count, source_app, source_icon_hash, created_at, updated_at, char_count,
                    (SELECT id FROM favorites WHERE source_clipboard_uuid = clipboard.uuid LIMIT 1) AS favorite_id
             FROM clipboard 
             {} 
             ORDER BY is_pinned DESC, item_order DESC, updated_at DESC 
             LIMIT ? OFFSET ?",
            where_clause
        );
        
        query_params.push(Box::new(params.limit));
        query_params.push(Box::new(params.offset));
        
        let mut stmt = conn.prepare(&query_sql)?;

        let mut items_to_update: Vec<(i64, String, String)> = vec![];
        
        let items = stmt.query_map(
            rusqlite::params_from_iter(query_params.iter().map(|p| p.as_ref())),
            |row| {
                let id: i64 = row.get(0)?;
                let uuid: Option<String> = row.get(1)?;
                let source_device_id: Option<String> = row.get(2)?;
                let is_remote: i64 = row.get(3)?;
                let content: String = row.get(4)?;
                let html_content: Option<String> = row.get(5)?;
                let content_type: String = row.get(6)?;
                let char_count: Option<i64> = row.get(15)?;
                
                let (truncated_content, truncated_html) = if is_textual_content_type(&content_type) {
                    let truncated_content = if content.len() > MAX_CONTENT_LENGTH {
                        if let Some(ref keyword) = search_keyword {
                            if !keyword.trim().is_empty() {
                                truncate_around_keyword(content.clone(), keyword, MAX_CONTENT_LENGTH)
                            } else {
                                truncate_string(content.clone(), MAX_CONTENT_LENGTH)
                            }
                        } else {
                            truncate_string(content.clone(), MAX_CONTENT_LENGTH)
                        }
                    } else {
                        content.clone()
                    };
                    
                    let truncated_html = html_content.map(|h| {
                        if h.len() > MAX_CONTENT_LENGTH {
                            truncate_html(h, MAX_CONTENT_LENGTH)
                        } else {
                            h
                        }
                    });
                    
                    (truncated_content, truncated_html)
                } else {
                    (content.clone(), html_content)
                };

                let needs_char_count = content_type.contains("text") || content_type.contains("rich_text");
                let final_char_count = if char_count.is_none() && needs_char_count && !content.is_empty() {
                    Some(content.chars().count() as i64)
                } else {
                    char_count
                };
                
                Ok((ClipboardItem {
                    id,
                    uuid,
                    favorite_id: row.get(16)?,
                    source_device_id,
                    is_remote: is_remote != 0,
                    content: truncated_content,
                    html_content: truncated_html,
                    content_type: content_type.clone(),
                    image_id: row.get(7)?,
                    item_order: row.get(8)?,
                    is_pinned: row.get::<_, i64>(9)? != 0,
                    paste_count: row.get(10)?,
                    source_app: row.get(11)?,
                    source_icon_hash: row.get(12)?,
                    char_count: final_char_count,
                    created_at: row.get(13)?,
                    updated_at: row.get(14)?,
                }, char_count.is_none() && needs_char_count, id, content, content_type))
            }
        )?
        .collect::<Result<Vec<_>, _>>()?;
        
        let mut result_items = vec![];
        for (item, needs_update, id, content, content_type) in items {
            if needs_update {
                items_to_update.push((id, content, content_type));
            }
            result_items.push(item);
        }

        if !items_to_update.is_empty() {
            update_missing_char_counts(items_to_update);
        }
        
        Ok(PaginatedResult::new(total_count, result_items, params.offset, params.limit))
    })
}

pub fn webdav_list_history_records(device_id: &str) -> Result<Vec<CloudRecord>, String> {
    with_connection(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, uuid, source_device_id, is_remote, content, html_content, content_type,
                    image_id, item_order, is_pinned, paste_count, source_app, source_icon_hash,
                    char_count, created_at, updated_at
             FROM clipboard
             ORDER BY item_order DESC, updated_at DESC, id DESC",
        )?;

        let rows = stmt.query_map([], |row| {
            let id: i64 = row.get(0)?;
            let uuid_opt: Option<String> = row.get(1)?;
            let uuid = uuid_opt.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| id.to_string());
            let source_device_id = row
                .get::<_, Option<String>>(2)?
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| device_id.to_string());

            Ok(CloudRecord {
                uuid,
                source_device_id,
                is_remote: row.get::<_, i64>(3)? != 0,
                content: row.get(4)?,
                html_content: row.get(5)?,
                content_type: row.get(6)?,
                image_id: row.get(7)?,
                item_order: row.get(8)?,
                paste_count: row.get(10)?,
                source_app: row.get(11)?,
                source_icon_hash: row.get(12)?,
                char_count: row.get(13)?,
                title: String::new(),
                group_name: "全部".to_string(),
                created_at: row.get(14)?,
                updated_at: row.get(15)?,
            })
        })?;

        Ok(rows.filter_map(|row| row.ok()).collect())
    })
}

pub fn webdav_list_history_record_metas() -> Result<Vec<CloudRecordMeta>, String> {
    with_connection(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, uuid, updated_at, image_id
             FROM clipboard
             ORDER BY item_order DESC, updated_at DESC, id DESC",
        )?;

        let rows = stmt.query_map([], |row| {
            let id: i64 = row.get(0)?;
            let uuid_opt: Option<String> = row.get(1)?;
            let uuid = uuid_opt.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| id.to_string());
            Ok(CloudRecordMeta {
                uuid,
                updated_at: row.get(2)?,
                image_id: row.get(3)?,
            })
        })?;

        Ok(rows.filter_map(|row| row.ok()).collect())
    })
}

pub fn webdav_get_history_record_by_uuid(uuid: &str, device_id: &str) -> Result<Option<CloudRecord>, String> {
    with_connection(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, uuid, source_device_id, is_remote, content, html_content, content_type,
                    image_id, item_order, is_pinned, paste_count, source_app, source_icon_hash,
                    char_count, created_at, updated_at
             FROM clipboard
             WHERE uuid = ?1 OR ((uuid IS NULL OR uuid = '') AND id = ?2)
             LIMIT 1",
        )?;
        let id = uuid.parse::<i64>().ok();
        let record = stmt.query_row(params![uuid, id], |row| {
            let id: i64 = row.get(0)?;
            let uuid_opt: Option<String> = row.get(1)?;
            let uuid = uuid_opt.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| id.to_string());
            let source_device_id = row
                .get::<_, Option<String>>(2)?
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| device_id.to_string());

            Ok(CloudRecord {
                uuid,
                source_device_id,
                is_remote: row.get::<_, i64>(3)? != 0,
                content: row.get(4)?,
                html_content: row.get(5)?,
                content_type: row.get(6)?,
                image_id: row.get(7)?,
                item_order: row.get(8)?,
                paste_count: row.get(10)?,
                source_app: row.get(11)?,
                source_icon_hash: row.get(12)?,
                char_count: row.get(13)?,
                title: String::new(),
                group_name: "全部".to_string(),
                created_at: row.get(14)?,
                updated_at: row.get(15)?,
            })
        }).optional()?;

        Ok(record)
    })
}

pub fn webdav_list_own_history_records(device_id: &str) -> Result<Vec<CloudRecord>, String> {
    with_connection(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, uuid, source_device_id, is_remote, content, html_content, content_type,
                    image_id, item_order, is_pinned, paste_count, source_app, source_icon_hash,
                    char_count, created_at, updated_at
             FROM clipboard
             WHERE source_device_id IS NULL OR source_device_id = '' OR source_device_id = ?1
             ORDER BY item_order DESC, updated_at DESC, id DESC",
        )?;

        let rows = stmt.query_map(params![device_id], |row| {
            let id: i64 = row.get(0)?;
            let uuid_opt: Option<String> = row.get(1)?;
            let uuid = uuid_opt.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| id.to_string());

            Ok(CloudRecord {
                uuid,
                source_device_id: device_id.to_string(),
                is_remote: row.get::<_, i64>(3)? != 0,
                content: row.get(4)?,
                html_content: row.get(5)?,
                content_type: row.get(6)?,
                image_id: row.get(7)?,
                item_order: row.get(8)?,
                paste_count: row.get(10)?,
                source_app: row.get(11)?,
                source_icon_hash: row.get(12)?,
                char_count: row.get(13)?,
                title: String::new(),
                group_name: "全部".to_string(),
                created_at: row.get(14)?,
                updated_at: row.get(15)?,
            })
        })?;

        Ok(rows.filter_map(|row| row.ok()).collect())
    })
}

pub fn webdav_history_record_states() -> Result<HashMap<String, i64>, String> {
    with_connection(|conn| {
        let mut stmt = conn.prepare(
            "SELECT uuid, updated_at FROM clipboard WHERE uuid IS NOT NULL AND uuid != ''",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;
        let mut states = HashMap::new();
        for row in rows {
            let (uuid, updated_at) = row?;
            states.insert(uuid, updated_at);
        }
        Ok(states)
    })
}

pub fn lan_upsert_history_records(records: &[CloudRecord]) -> Result<Vec<CloudRecord>, String> {
    upsert_history_records(records, false)
}

pub fn webdav_repair_history_records(records: &[CloudRecord]) -> Result<Vec<CloudRecord>, String> {
    upsert_history_records(records, true)
}

fn upsert_history_records(records: &[CloudRecord], ignore_tombstones: bool) -> Result<Vec<CloudRecord>, String> {
    if records.is_empty() {
        return Ok(Vec::new());
    }

    with_connection(|conn| {
        let tx = conn.unchecked_transaction()?;
        let mut changed = Vec::new();

        for record in records {
            if record.uuid.trim().is_empty() {
                continue;
            }
            let tombstone_deleted_at = super::tombstones::sync_tombstone_deleted_at_in_conn(
                &tx,
                super::tombstones::COLLECTION_HISTORY,
                &record.uuid,
            )?;
            if !ignore_tombstones && tombstone_deleted_at.map(|value| value >= record.updated_at).unwrap_or(false) {
                continue;
            }
            let restored_updated_at = if ignore_tombstones {
                super::tombstones::restored_record_updated_at(record.updated_at, tombstone_deleted_at)
            } else {
                record.updated_at
            };

            let existing = tx
                .query_row(
                    "SELECT COALESCE(source_device_id, ''), updated_at, content, html_content, content_type,
                            image_id, item_order, paste_count, source_app, source_icon_hash, char_count, created_at
                     FROM clipboard WHERE uuid = ?1 LIMIT 1",
                    params![record.uuid],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, Option<String>>(5)?,
                            row.get::<_, i64>(6)?,
                            row.get::<_, i64>(7)?,
                            row.get::<_, Option<String>>(8)?,
                            row.get::<_, Option<String>>(9)?,
                            row.get::<_, Option<i64>>(10)?,
                            row.get::<_, i64>(11)?,
                        ))
                    },
                )
                .optional()?;

            if let Some((
                source_device_id,
                updated_at,
                content,
                html_content,
                content_type,
                image_id,
                item_order,
                paste_count,
                source_app,
                source_icon_hash,
                char_count,
                created_at,
            )) = existing {
                let same = source_device_id == record.source_device_id
                    && updated_at == restored_updated_at
                    && content == record.content
                    && html_content == record.html_content
                    && content_type == record.content_type
                    && image_id == record.image_id
                    && item_order == record.item_order
                    && paste_count == record.paste_count
                    && source_app == record.source_app
                    && source_icon_hash == record.source_icon_hash
                    && char_count == record.char_count
                    && created_at == record.created_at;

                if updated_at >= restored_updated_at || same {
                    if tombstone_deleted_at.map(|deleted_at| deleted_at < updated_at).unwrap_or(false) {
                        super::tombstones::delete_sync_tombstone_in_conn(
                            &tx,
                            super::tombstones::COLLECTION_HISTORY,
                            &record.uuid,
                        )?;
                    }
                    continue;
                }

                tx.execute(
                    "UPDATE clipboard SET
                        source_device_id = ?1,
                        is_remote = 1,
                        content = ?2,
                        html_content = ?3,
                        content_type = ?4,
                        image_id = ?5,
                        item_order = ?6,
                        paste_count = ?7,
                        source_app = ?8,
                        source_icon_hash = ?9,
                        char_count = ?10,
                        created_at = ?11,
                        updated_at = ?12
                     WHERE uuid = ?13",
                    params![
                        record.source_device_id,
                        record.content,
                        record.html_content,
                        record.content_type,
                        record.image_id,
                        record.item_order,
                        record.paste_count,
                        record.source_app,
                        record.source_icon_hash,
                        record.char_count,
                        record.created_at,
                        restored_updated_at,
                        record.uuid,
                    ],
                )?;
                if tombstone_deleted_at.map(|deleted_at| deleted_at < restored_updated_at).unwrap_or(false) {
                    super::tombstones::delete_sync_tombstone_in_conn(
                        &tx,
                        super::tombstones::COLLECTION_HISTORY,
                        &record.uuid,
                    )?;
                }
                let mut changed_record = record.clone();
                changed_record.updated_at = restored_updated_at;
                changed.push(changed_record);
                continue;
            }

            tx.execute(
                "INSERT INTO clipboard (
                    uuid, source_device_id, is_remote, content, html_content, content_type,
                    image_id, item_order, is_pinned, paste_count, source_app, source_icon_hash,
                    char_count, created_at, updated_at
                 ) VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    record.uuid,
                    record.source_device_id,
                    record.content,
                    record.html_content,
                    record.content_type,
                    record.image_id,
                    record.item_order,
                    record.paste_count,
                    record.source_app,
                    record.source_icon_hash,
                    record.char_count,
                    record.created_at,
                    restored_updated_at,
                ],
            )?;
            if tombstone_deleted_at.map(|deleted_at| deleted_at < restored_updated_at).unwrap_or(false) {
                super::tombstones::delete_sync_tombstone_in_conn(
                    &tx,
                    super::tombstones::COLLECTION_HISTORY,
                    &record.uuid,
                )?;
            }
            let mut changed_record = record.clone();
            changed_record.updated_at = restored_updated_at;
            changed.push(changed_record);
        }

        tx.commit()?;
        Ok(changed)
    })
}


// 获取剪贴板总数
pub fn get_clipboard_count() -> Result<i64, String> {
    with_connection(|conn| {
        conn.query_row("SELECT COUNT(*) FROM clipboard", [], |row| row.get(0))
    })
}

pub fn get_clipboard_item_position(id: i64) -> Result<Option<i64>, String> {
    with_connection(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id FROM clipboard ORDER BY is_pinned DESC, item_order DESC, updated_at DESC",
        )?;

        let ids = stmt
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(ids.iter().position(|item_id| *item_id == id).map(|index| index as i64))
    })
}

// 根据ID获取剪贴板项（完整内容，不截断）
pub fn get_clipboard_item_by_id(id: i64) -> Result<Option<ClipboardItem>, String> {
    get_clipboard_item_by_id_with_limit(id, None)
}

pub fn ensure_clipboard_item_uuid(id: i64) -> Result<String, String> {
    let maybe_uuid: Option<String> = with_connection(|conn| {
        let existing: Option<Option<String>> = conn
            .query_row(
                "SELECT uuid FROM clipboard WHERE id = ?1 LIMIT 1",
                params![id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?;

        let existing = existing.flatten();

        if let Some(uuid) = existing.clone().filter(|u| !u.trim().is_empty()) {
            return Ok(Some(uuid));
        }

        let new_uuid = Uuid::new_v4().to_string();
        conn.execute(
            "UPDATE clipboard SET uuid = ?1 WHERE id = ?2 AND (uuid IS NULL OR uuid = '')",
            params![new_uuid, id],
        )?;

        let uuid: Option<Option<String>> = conn
            .query_row(
                "SELECT uuid FROM clipboard WHERE id = ?1 LIMIT 1",
                params![id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?;

        Ok(uuid.flatten())
    })?;

    maybe_uuid
        .filter(|u| !u.trim().is_empty())
        .ok_or_else(|| "生成 uuid 失败".to_string())
}

pub fn get_clipboard_item_id_by_uuid(uuid: &str) -> Result<Option<i64>, String> {
    with_connection(|conn| {
        conn.query_row(
            "SELECT id FROM clipboard WHERE uuid = ?1 LIMIT 1",
            params![uuid],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.into())
    })
}

// 根据ID获取剪贴板项（指定截断长度）
pub fn get_clipboard_item_by_id_with_limit(id: i64, max_content_length: Option<usize>) -> Result<Option<ClipboardItem>, String> {
    with_connection(|conn| {
        conn.query_row(
            "SELECT id, uuid, source_device_id, is_remote, content, html_content, content_type, image_id, item_order, is_pinned, paste_count, source_app, source_icon_hash, created_at, updated_at, char_count,
                    (SELECT id FROM favorites WHERE source_clipboard_uuid = clipboard.uuid LIMIT 1) AS favorite_id
             FROM clipboard WHERE id = ?",
            params![id],
            |row| {
                let uuid: Option<String> = row.get(1)?;
                let source_device_id: Option<String> = row.get(2)?;
                let is_remote: i64 = row.get(3)?;
                let content: String = row.get(4)?;
                let html_content: Option<String> = row.get(5)?;
                let content_type: String = row.get(6)?;
                let char_count: Option<i64> = row.get(15)?;
                let final_content = if let Some(max_len) = max_content_length {
                    let is_text_type = is_textual_content_type(&content_type);
                    if is_text_type && content.len() > max_len {
                        truncate_string(content.clone(), max_len)
                    } else {
                        content.clone()
                    }
                } else {
                    content.clone()
                };
                
                // 计算字符数
                let final_char_count = if char_count.is_none() && (content_type.contains("text") || content_type.contains("rich_text")) && !content.is_empty() {
                    Some(content.chars().count() as i64)
                } else {
                    char_count
                };
                
                Ok(ClipboardItem {
                    id: row.get(0)?,
                    uuid,
                    favorite_id: row.get(16)?,
                    source_device_id,
                    is_remote: is_remote != 0,
                    content: final_content,
                    html_content,
                    content_type,
                    image_id: row.get(7)?,
                    item_order: row.get(8)?,
                    is_pinned: row.get::<_, i64>(9)? != 0,
                    paste_count: row.get(10)?,
                    source_app: row.get(11)?,
                    source_icon_hash: row.get(12)?,
                    char_count: final_char_count,
                    created_at: row.get(13)?,
                    updated_at: row.get(14)?,
                })
            }
        )
        .optional()
        .map_err(|e| e.into())
    })
}

pub fn increment_paste_count(id: i64) -> Result<(), String> {
    with_connection(|conn| {
        conn.execute(
            "UPDATE clipboard SET paste_count = paste_count + 1 WHERE id = ?",
            params![id],
        )?;
        Ok(())
    })
}

pub fn increment_paste_counts(ids: &[i64]) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }

    with_connection(|conn| {
        let tx = conn.unchecked_transaction()?;
        for id in ids {
            tx.execute(
                "UPDATE clipboard SET paste_count = paste_count + 1 WHERE id = ?",
                params![id],
            )?;
        }
        tx.commit()?;
        Ok(())
    })
}

// 限制剪贴板历史数量（删除超出限制的旧记录）
pub fn limit_clipboard_history(max_count: u64) -> Result<(), String> {
    if max_count >= 999999 {
        return Ok(());
    }
    
    let (images_to_delete, deleted_ids): (Vec<String>, Vec<i64>) = with_connection(|conn| {
        let sql_ids = "SELECT image_id FROM clipboard WHERE id NOT IN (SELECT id FROM clipboard ORDER BY is_pinned DESC, item_order DESC, updated_at DESC LIMIT ?1) AND image_id IS NOT NULL AND image_id <> ''";
        let mut stmt = conn.prepare(sql_ids)?;
        let ids_iter = stmt.query_map(params![max_count], |row| row.get::<_, String>(0))?;
        let mut set: HashSet<String> = HashSet::new();
        for r in ids_iter {
            if let Ok(s) = r {
                for iid in split_image_ids(&s) {
                    set.insert(iid);
                }
            }
        }
        drop(stmt);

        let mut delete_ids_stmt = conn.prepare(
            "SELECT id FROM clipboard WHERE id NOT IN (
                SELECT id FROM clipboard ORDER BY is_pinned DESC, item_order DESC, updated_at DESC LIMIT ?1
            )",
        )?;
        let deleted_ids = delete_ids_stmt
            .query_map(params![max_count], |row| row.get::<_, i64>(0))?
            .filter_map(|r| r.ok())
            .collect::<Vec<_>>();

        conn.execute(
            "DELETE FROM clipboard WHERE id NOT IN (
                SELECT id FROM clipboard ORDER BY is_pinned DESC, item_order DESC, updated_at DESC LIMIT ?1
            )",
            params![max_count],
        )?;

        let mut to_delete = Vec::new();
        for iid in set.into_iter() {
            if !is_image_id_referenced(conn, &iid)? {
                to_delete.push(iid);
            }
        }
        Ok((to_delete, deleted_ids))
    })?;

    for id in deleted_ids {
        let _ = delete_clipboard_data_items("clipboard", &id.to_string());
    }

    delete_image_files(images_to_delete)
}

// 删除单个剪贴板项
pub fn delete_clipboard_item(id: i64) -> Result<(), String> {
    let images_to_delete: Vec<String> = with_connection(|conn| {
        let item: Option<(Option<String>, Option<String>)> = conn
            .query_row(
                "SELECT image_id, uuid FROM clipboard WHERE id = ?",
                params![id],
                |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()?;
        let Some((image_ids, uuid)) = item else {
            return Ok(Vec::new());
        };
        let deleted_at = chrono::Local::now().timestamp();
        let tombstone_id = uuid.filter(|value| !value.trim().is_empty()).unwrap_or_else(|| id.to_string());
        super::tombstones::record_sync_tombstone_in_conn(
            conn,
            super::tombstones::COLLECTION_HISTORY,
            &tombstone_id,
            &crate::services::sync_transfer::device_id(),
            deleted_at,
        )?;

        conn.execute("DELETE FROM clipboard WHERE id = ?1", params![id])?;

        let mut to_delete = Vec::new();
        if let Some(ids) = image_ids {
            for iid in split_image_ids(&ids) {
                if !is_image_id_referenced(conn, &iid)? {
                    to_delete.push(iid);
                }
            }
        }
        Ok(to_delete)
    })?;

    let _ = delete_clipboard_data_items("clipboard", &id.to_string());
    delete_image_files(images_to_delete)
}

pub fn delete_clipboard_items(ids: &[i64]) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }

    let unique_ids: Vec<i64> = ids
        .iter()
        .copied()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    let images_to_delete: Vec<String> = with_connection(|conn| {
        let mut image_id_set: HashSet<String> = HashSet::new();
        let mut tombstone_ids = Vec::new();
        for id in &unique_ids {
            let item: Option<(Option<String>, Option<String>)> = conn
                .query_row(
                    "SELECT image_id, uuid FROM clipboard WHERE id = ?",
                    params![id],
                    |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?)),
                )
                .optional()?;

            if let Some((image_ids, uuid)) = item {
                if let Some(image_ids) = image_ids {
                    for image_id in split_image_ids(&image_ids) {
                        image_id_set.insert(image_id);
                    }
                }
                tombstone_ids.push(uuid.filter(|value| !value.trim().is_empty()).unwrap_or_else(|| id.to_string()));
            }
        }

        let tx = conn.unchecked_transaction()?;
        let deleted_at = chrono::Local::now().timestamp();
        let local_device_id = crate::services::sync_transfer::device_id();
        for uuid in &tombstone_ids {
            super::tombstones::record_sync_tombstone_in_conn(
                &tx,
                super::tombstones::COLLECTION_HISTORY,
                uuid,
                &local_device_id,
                deleted_at,
            )?;
        }
        for id in &unique_ids {
            tx.execute("DELETE FROM clipboard WHERE id = ?1", params![id])?;
        }
        tx.commit()?;

        let mut to_delete = Vec::new();
        for image_id in image_id_set {
            if !is_image_id_referenced(conn, &image_id)? {
                to_delete.push(image_id);
            }
        }

        Ok(to_delete)
    })?;

    for id in &unique_ids {
        let _ = delete_clipboard_data_items("clipboard", &id.to_string());
    }
    delete_image_files(images_to_delete)
}

// 清空所有剪贴板历史
pub fn clear_clipboard_history() -> Result<(), String> {
    let images_to_delete: Vec<String> = with_connection(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, image_id, uuid FROM clipboard",
        )?;
        let ids_iter = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        let mut set: HashSet<String> = HashSet::new();
        let mut tombstone_ids = Vec::new();
        for r in ids_iter {
            if let Ok((id, image_ids, uuid)) = r {
                if let Some(image_ids) = image_ids {
                    for iid in split_image_ids(&image_ids) {
                        set.insert(iid);
                    }
                }
                tombstone_ids.push(uuid.filter(|value| !value.trim().is_empty()).unwrap_or_else(|| id.to_string()));
            }
        }
        drop(stmt);

        let tx = conn.unchecked_transaction()?;
        let deleted_at = chrono::Local::now().timestamp();
        let local_device_id = crate::services::sync_transfer::device_id();
        for uuid in tombstone_ids {
            super::tombstones::record_sync_tombstone_in_conn(
                &tx,
                super::tombstones::COLLECTION_HISTORY,
                &uuid,
                &local_device_id,
                deleted_at,
            )?;
        }
        tx.execute("DELETE FROM clipboard", [])?;
        tx.commit()?;

        let mut to_delete = Vec::new();
        for iid in set.into_iter() {
            if !is_image_id_referenced(conn, &iid)? {
                to_delete.push(iid);
            }
        }
        Ok(to_delete)
    })?;

    let _ = delete_clipboard_data_items_by_kind("clipboard");
    delete_image_files(images_to_delete)
}

// 排序逻辑
fn reorder_items(conn: &rusqlite::Connection, from_idx: usize, to_idx: usize, items: &[(i64, i64)]) -> Result<(), rusqlite::Error> {
    if from_idx == to_idx { return Ok(()); }
    
    let tx = conn.unchecked_transaction()?;
    let now = chrono::Local::now().timestamp();
    let moved_id = items[from_idx].0;
    let target_order = items[to_idx].1;

    if from_idx < to_idx {
        for i in (from_idx + 1)..=to_idx {
            tx.execute("UPDATE clipboard SET item_order = item_order + 1, updated_at = ?1 WHERE id = ?2", params![now, items[i].0])?;
        }
    } else {
        for i in to_idx..from_idx {
            tx.execute("UPDATE clipboard SET item_order = item_order - 1, updated_at = ?1 WHERE id = ?2", params![now, items[i].0])?;
        }
    }
    tx.execute("UPDATE clipboard SET item_order = ?1, updated_at = ?2 WHERE id = ?3", params![target_order, now, moved_id])?;
    tx.commit()
}

// 移动剪贴板项到顶部（非置顶区的顶部）
pub fn move_clipboard_item_to_top(id: i64) -> Result<(), String> {
    with_connection(|conn| {
        let now = chrono::Local::now().timestamp();
        let max_order: i64 = conn.query_row(
            "SELECT COALESCE(MAX(item_order), 0) FROM clipboard WHERE is_pinned = 0",
            [],
            |row| row.get(0)
        ).unwrap_or(0);
        
        conn.execute(
            "UPDATE clipboard SET item_order = ?1, updated_at = ?2 WHERE id = ?3 AND is_pinned = 0",
            params![max_order + 1, now, id],
        )?;
        Ok(())
    })
}

// 移动剪贴板项
pub fn move_clipboard_item_by_id(from_id: i64, to_id: i64) -> Result<(), String> {
    if from_id == to_id { return Ok(()); }

    with_connection(|conn| {
        let from_pinned: i64 = conn.query_row(
            "SELECT is_pinned FROM clipboard WHERE id = ?",
            params![from_id], |row| row.get(0)
        )?;
        let to_pinned: i64 = conn.query_row(
            "SELECT is_pinned FROM clipboard WHERE id = ?",
            params![to_id], |row| row.get(0)
        )?;
        
        if from_pinned != to_pinned {
            return Ok(());
        }
        
        let items: Vec<(i64, i64)> = conn.prepare("SELECT id, item_order FROM clipboard ORDER BY is_pinned DESC, item_order DESC, updated_at DESC")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;

        let from_idx = items.iter().position(|(id, _)| *id == from_id)
            .ok_or_else(|| rusqlite::Error::InvalidParameterName(format!("ID {} 不存在", from_id)))?;
        let to_idx = items.iter().position(|(id, _)| *id == to_id)
            .ok_or_else(|| rusqlite::Error::InvalidParameterName(format!("ID {} 不存在", to_id)))?;
        
        reorder_items(conn, from_idx, to_idx, &items)
    })
}

// 更新剪贴板项的内容
pub fn update_clipboard_item(
    id: i64,
    content: String,
    html_content: Option<String>,
) -> Result<(), String> {
    let should_clear_raw_formats = with_connection(|conn| {
        let (old_content, old_html_content): (String, Option<String>) = conn.query_row(
            "SELECT content, html_content FROM clipboard WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;

        let content_changed = old_content != content;
        let html_changed = html_content
            .as_ref()
            .map(|new_html| old_html_content.as_deref() != Some(new_html.as_str()))
            .unwrap_or(false);

        let now = chrono::Local::now().timestamp();
        let rows = if let Some(ref html_content) = html_content {
            conn.execute(
                "UPDATE clipboard SET content = ?1, html_content = ?2, updated_at = ?3 WHERE id = ?4",
                params![&content, html_content, now, id],
            )?
        } else {
            conn.execute(
                "UPDATE clipboard SET content = ?1, updated_at = ?2 WHERE id = ?3",
                params![&content, now, id],
            )?
        };
        if rows == 0 {
            Err(rusqlite::Error::QueryReturnedNoRows)
        } else {
            Ok(content_changed || html_changed)
        }
    }).map_err(|e| if e.contains("QueryReturnedNoRows") {
        format!("剪贴板项不存在: {}", id)
    } else { e })?;

    if should_clear_raw_formats {
        delete_clipboard_data_items("clipboard", &id.to_string())?;
    }

    Ok(())
}

// 切换剪贴板项的置顶状态（置顶时放到置顶区第一位，取消置顶时移到非置顶区第一位）
pub fn toggle_pin_clipboard_item(id: i64) -> Result<bool, String> {
    with_connection(|conn| {
        let current_pinned: i64 = conn.query_row(
            "SELECT is_pinned FROM clipboard WHERE id = ?", params![id], |row| row.get(0)
        )?;
        
        let now = chrono::Local::now().timestamp();
        if current_pinned == 0 {
            let max_pinned_order: i64 = conn.query_row(
                "SELECT COALESCE(MAX(item_order), 0) FROM clipboard WHERE is_pinned = 1", [], |row| row.get(0)
            ).unwrap_or(0);
            conn.execute("UPDATE clipboard SET is_pinned = 1, item_order = ?1, updated_at = ?2 WHERE id = ?3", params![max_pinned_order + 1, now, id])?;
            Ok(true)
        } else {
            let max_order: i64 = conn.query_row(
                "SELECT COALESCE(MAX(item_order), 0) FROM clipboard WHERE is_pinned = 0", [], |row| row.get(0)
            ).unwrap_or(0);
            conn.execute("UPDATE clipboard SET is_pinned = 0, item_order = ?1, updated_at = ?2 WHERE id = ?3", params![max_order + 1, now, id])?;
            Ok(false)
        }
    })
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::database::connection::test_support::{TestDb, TEST_ENV_LOCK};
    use crate::services::webdav_sync::types::CloudRecord;

    fn seed_clip_row(db: &TestDb, uuid: &str, content: &str, item_order: i64, updated_at: i64) -> i64 {
        db.exec(
            "INSERT INTO clipboard (content, content_type, item_order, uuid, is_remote, created_at, updated_at) VALUES (?1, 'text', ?2, ?3, 0, ?4, ?4)",
            &[&content, &item_order, &uuid, &updated_at],
        )
    }

    fn seed_clip_data(db: &TestDb, target_id: &str, format_name: &str, raw: &[u8]) {
        db.exec(
            "INSERT INTO clipboard_data (target_kind, target_id, format_name, raw_data, is_primary, format_order, created_at, updated_at) VALUES ('clipboard', ?1, ?2, ?3, 1, 0, 1000, 1000)",
            &[&target_id, &format_name, &raw],
        );
    }

    fn mk_record(uuid: &str, content: &str, updated_at: i64) -> CloudRecord {
        CloudRecord {
            uuid: uuid.to_string(),
            source_device_id: "remote-dev".to_string(),
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
            created_at: 500,
            updated_at,
        }
    }

    // b_delete_writes_tombstone
    #[test]
    fn delete_writes_tombstone_and_removes_row_data_and_unreferenced_image() {
        let _guard = TEST_ENV_LOCK.lock();
        let db = TestDb::new();
        let img_rel = db.create_fake_image("deadbeef");
        let img_abs = db.data_dir().join(&img_rel);
        assert!(img_abs.exists());

        let id = seed_clip_row(&db, "u1", "abc", 1, 1000);
        db.exec("UPDATE clipboard SET image_id = 'deadbeef' WHERE id = ?1", &[&id]);
        seed_clip_data(&db, &id.to_string(), "CF_UNICODETEXT", b"abc");

        delete_clipboard_item(id).expect("删除应成功");

        assert_eq!(db.count("clipboard"), 0, "行被删除");
        assert_eq!(db.count("clipboard_data"), 0, "raw 格式被删除");
        let (collection, item_id, device, deleted_at): (String, String, String, i64) = db
            .query_row(
                "SELECT collection, item_id, source_device_id, deleted_at FROM sync_tombstones",
                &[],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .expect("应有墓碑");
        assert_eq!(collection, "history");
        assert_eq!(item_id, "u1", "墓碑 item_id = 行 uuid");
        assert_eq!(device, crate::services::sync_transfer::device_id());
        let now = chrono::Local::now().timestamp();
        assert!((now - deleted_at).abs() <= 5, "deleted_at 为删除时刻");
        assert!(!img_abs.exists(), "无引用图片文件被删除");
    }

    #[test]
    fn delete_keeps_image_file_when_favorite_still_references_it() {
        let _guard = TEST_ENV_LOCK.lock();
        let db = TestDb::new();
        let img_rel = db.create_fake_image("stillalive");
        let img_abs = db.data_dir().join(&img_rel);
        let id = seed_clip_row(&db, "u1", "abc", 1, 1000);
        db.exec("UPDATE clipboard SET image_id = 'stillalive' WHERE id = ?1", &[&id]);
        db.exec(
            "INSERT INTO favorites (id, title, content, content_type, image_id, group_name, item_order, created_at, updated_at) VALUES ('f1', '', 'abc', 'text', 'stillalive', '全部', 1, 1000, 1000)",
            &[],
        );

        delete_clipboard_item(id).expect("删除应成功");
        assert!(img_abs.exists(), "被收藏引用的图片文件应保留");
    }

    // b_tombstone_blocks_remote（含边界：>= 判定与反向边界）
    #[test]
    fn tombstone_blocks_remote_upsert_until_record_is_newer() {
        let _guard = TEST_ENV_LOCK.lock();
        let db = TestDb::new();
        let id = seed_clip_row(&db, "u1", "local", 1, 1000);
        delete_clipboard_item(id).expect("删除应成功");
        let (deleted_at,): (i64,) = db
            .query_row("SELECT deleted_at FROM sync_tombstones", &[], |r| Ok((r.get(0)?,)))
            .expect("应有墓碑");

        // 边界：updated_at == deleted_at → 拦截（>= 判定）
        let eq_record = mk_record("u1", "remote-eq", deleted_at);
        let changed = lan_upsert_history_records(&[eq_record]).expect("upsert 不报错");
        assert!(changed.is_empty(), "updated_at == deleted_at 必须被墓碑拦截");
        assert_eq!(db.count("clipboard"), 0);

        // 边界：updated_at < deleted_at → 拦截
        let old_record = mk_record("u1", "remote-old", deleted_at - 100);
        let changed = lan_upsert_history_records(&[old_record]).expect("upsert 不报错");
        assert!(changed.is_empty());
        assert_eq!(db.count("clipboard"), 0);

        // 反向边界：updated_at = deleted_at + 1 → 生效，墓碑清除
        let newer_record = mk_record("u1", "remote-new", deleted_at + 1);
        let changed = lan_upsert_history_records(&[newer_record]).expect("upsert 不报错");
        assert_eq!(changed.len(), 1);
        let (content, is_remote, source_device): (String, i64, String) = db
            .query_row(
                "SELECT content, is_remote, source_device_id FROM clipboard WHERE uuid = 'u1'",
                &[],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("应插入远端行");
        assert_eq!(content, "remote-new");
        assert_eq!(is_remote, 1, "远端记录 is_remote=1");
        assert_eq!(source_device, "remote-dev");
        assert_eq!(db.count("sync_tombstones"), 0, "记录比墓碑新 → 墓碑清除");
    }

    // b_repair_overrides_tombstone
    #[test]
    fn repair_overrides_tombstone_with_restored_timestamp() {
        let _guard = TEST_ENV_LOCK.lock();
        let db = TestDb::new();
        let id = seed_clip_row(&db, "u1", "local", 1, 1000);
        delete_clipboard_item(id).expect("删除应成功");
        let (deleted_at,): (i64,) = db
            .query_row("SELECT deleted_at FROM sync_tombstones", &[], |r| Ok((r.get(0)?,)))
            .expect("应有墓碑");

        // 记录比墓碑旧 → repair 仍应用，updated_at = max(record, now, deleted_at+1)
        let record = mk_record("u1", "restored", deleted_at - 100);
        let changed = webdav_repair_history_records(&[record]).expect("repair 应成功");
        assert_eq!(changed.len(), 1);
        let restored_at = changed[0].updated_at;
        assert!(restored_at >= deleted_at + 1, "restored = max(record, now, deleted_at+1)");
        let (content, is_remote, updated_at): (String, i64, i64) = db
            .query_row(
                "SELECT content, is_remote, updated_at FROM clipboard WHERE uuid = 'u1'",
                &[],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("应插入恢复行");
        assert_eq!(content, "restored");
        assert_eq!(is_remote, 1);
        assert_eq!(updated_at, restored_at, "落库 updated_at = 返回的 restored 值");
        assert_eq!(db.count("sync_tombstones"), 0, "恢复后墓碑清除");
    }

    #[test]
    fn repair_exact_max_semantics_when_record_is_newest() {
        let _guard = TEST_ENV_LOCK.lock();
        let db = TestDb::new();
        let id = seed_clip_row(&db, "u2", "local", 1, 1000);
        delete_clipboard_item(id).expect("删除应成功");
        let (deleted_at,): (i64,) = db
            .query_row("SELECT deleted_at FROM sync_tombstones", &[], |r| Ok((r.get(0)?,)))
            .expect("应有墓碑");

        // record.updated_at 远大于 now 和 deleted_at → restored 精确等于 record.updated_at
        let far_future = deleted_at + 1_000_000;
        let record = mk_record("u2", "future", far_future);
        let changed = webdav_repair_history_records(&[record]).expect("repair 应成功");
        assert_eq!(changed[0].updated_at, far_future, "max(record, now, deleted_at+1) = record");
    }

    // b_history_limit_trim
    #[test]
    fn history_limit_trims_beyond_top_n_pinned_first() {
        let _guard = TEST_ENV_LOCK.lock();
        let db = TestDb::new();
        // 置顶 1 条 + 未置顶 3 条（order 3/2/1，updated_at 3000/2000/1000）
        db.exec(
            "INSERT INTO clipboard (content, content_type, item_order, is_pinned, uuid, is_remote, created_at, updated_at, image_id) VALUES ('pinned', 'text', 10, 1, 'up', 0, 3000, 3000, 'keep-pinned')",
            &[],
        );
        let id_u3 = seed_clip_row(&db, "u3", "row-3", 3, 3000);
        let id_u2 = seed_clip_row(&db, "u2", "row-2", 2, 2000);
        let id_u1 = seed_clip_row(&db, "u1", "row-1", 1, 1000);
        seed_clip_data(&db, &id_u3.to_string(), "CF_UNICODETEXT", b"3");
        seed_clip_data(&db, &id_u2.to_string(), "CF_UNICODETEXT", b"2");
        seed_clip_data(&db, &id_u1.to_string(), "CF_UNICODETEXT", b"1");

        limit_clipboard_history(3).expect("trim 应成功");

        assert_eq!(db.count("clipboard"), 3, "保留 top-3");
        let remaining: String = db
            .query_row(
                "SELECT GROUP_CONCAT(uuid, ',') FROM (SELECT uuid FROM clipboard ORDER BY is_pinned DESC, item_order DESC, updated_at DESC)",
                &[],
                |r| r.get(0),
            )
            .expect("剩余行");
        assert_eq!(remaining, "up,u3,u2", "置顶优先，然后 order DESC");
        // 被删行的 raw 格式一并删除
        let orphan = db
            .query_row(
                "SELECT COUNT(*) FROM clipboard_data WHERE target_kind = 'clipboard' AND target_id = ?1",
                &[&id_u1.to_string()],
                |r| r.get::<_, i64>(0),
            )
            .expect("孤儿检查");
        assert_eq!(orphan, 0);
        assert!(db.query_row("SELECT id FROM clipboard WHERE uuid = 'u1'", &[], |r| r.get::<_, i64>(0)).is_none());
    }

    #[test]
    fn history_limit_deletes_unreferenced_image_files_but_keeps_favorite_referenced() {
        let _guard = TEST_ENV_LOCK.lock();
        let db = TestDb::new();
        let img_dead = db.create_fake_image("trimmed");
        let img_alive = db.create_fake_image("favkept");
        let dead_abs = db.data_dir().join(&img_dead);
        let alive_abs = db.data_dir().join(&img_alive);

        db.exec(
            "INSERT INTO clipboard (content, content_type, item_order, uuid, is_remote, created_at, updated_at, image_id) VALUES ('d', 'text', 1, 'ud', 0, 1000, 1000, 'trimmed')",
            &[],
        );
        db.exec(
            "INSERT INTO clipboard (content, content_type, item_order, uuid, is_remote, created_at, updated_at, image_id) VALUES ('k', 'text', 2, 'uk', 0, 1000, 1000, 'favkept')",
            &[],
        );
        db.exec(
            "INSERT INTO favorites (id, title, content, content_type, image_id, group_name, item_order, created_at, updated_at) VALUES ('f1', '', 'k', 'text', 'favkept', '全部', 1, 1000, 1000)",
            &[],
        );

        limit_clipboard_history(1).expect("trim 应成功");
        assert!(!dead_abs.exists(), "无引用图片被删除");
        assert!(alive_abs.exists(), "收藏引用的图片保留");
    }

    // 边界：history_limit >= 999999 完全跳过裁剪 —— 用 1,000,001 行验证“不删任何行”，
    // 否则（去掉跳过）会裁到 top-999999。
    #[test]
    fn history_limit_skips_trimming_at_999999() {
        let _guard = TEST_ENV_LOCK.lock();
        let db = TestDb::new();
        db.exec(
            "WITH RECURSIVE cnt(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM cnt WHERE x < 1000001) \
             INSERT INTO clipboard (content, content_type, item_order, uuid, is_remote, created_at, updated_at) \
             SELECT 'x', 'text', x, 'u' || x, 0, 1, 1 FROM cnt",
            &[],
        );
        assert_eq!(db.count("clipboard"), 1_000_001);
        limit_clipboard_history(999999).expect("应跳过");
        assert_eq!(db.count("clipboard"), 1_000_001, "999999 上限必须跳过裁剪");
    }

    // b_search_truncation
    #[test]
    fn search_truncates_around_keyword_and_filters_total_count() {
        let _guard = TEST_ENV_LOCK.lock();
        let db = TestDb::new();
        let long = format!("{}needle{}", "前".repeat(3000), "后".repeat(3000));
        let id = seed_clip_row(&db, "u1", &long, 1, 1000);
        db.exec("UPDATE clipboard SET char_count = NULL WHERE id = ?1", &[&id]);
        seed_clip_row(&db, "u2", "other stuff", 2, 2000);

        let page = query_clipboard_items(QueryParams {
            offset: 0,
            limit: 50,
            search: Some("needle".to_string()),
            content_type: None,
        })
        .expect("查询应成功");

        assert_eq!(page.total_count, 1, "total_count 反映过滤");
        let item = &page.items[0];
        // 与工具函数逐字节一致（keyword 居中截断）
        assert_eq!(item.content, truncate_around_keyword(long.clone(), "needle", 1600));
        assert!(item.content.len() <= 1600);
        assert!(item.content.contains("needle"));
        // char_count 为 NULL 时由查询就地计算（懒回填在异步线程）
        assert_eq!(item.char_count, Some(long.chars().count() as i64));

        // 无搜索：两条都在
        let all = query_clipboard_items(QueryParams::default()).expect("查询应成功");
        assert_eq!(all.total_count, 2);

        // 等异步回填完成，避免把过期 (id, content) 写入后续测试的数据库
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let done = db
                .query_row("SELECT char_count FROM clipboard WHERE id = ?1", &[&id], |r| r.get::<_, Option<i64>>(0))
                .expect("读取 char_count")
                .is_some();
            if done || std::time::Instant::now() > deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn non_textual_content_is_never_truncated() {
        let _guard = TEST_ENV_LOCK.lock();
        let db = TestDb::new();
        let long_files = format!("files:{}", "x".repeat(3000));
        seed_clip_row(&db, "u1", &long_files, 1, 1000);
        db.exec("UPDATE clipboard SET content_type = 'file' WHERE uuid = 'u1'", &[]);

        let page = query_clipboard_items(QueryParams::default()).expect("查询应成功");
        let item = &page.items[0];
        assert_eq!(item.content_type, "file");
        assert_eq!(item.content.len(), long_files.len(), "非文本类型永不截断");
        assert!(item.content.len() > 1600);
    }

    // b_pin_bucket_ordering
    #[test]
    fn pin_toggle_moves_bucket_and_query_orders_pinned_first() {
        let _guard = TEST_ENV_LOCK.lock();
        let db = TestDb::new();
        let id_a = seed_clip_row(&db, "ua", "a", 1, 1000);
        let id_b = seed_clip_row(&db, "ub", "b", 2, 2000);
        let id_c = seed_clip_row(&db, "uc", "c", 3, 3000);

        assert!(toggle_pin_clipboard_item(id_b).expect("置顶应成功"), "返回 true = 已置顶");
        let (is_pinned, item_order): (i64, i64) = db
            .query_row(
                "SELECT is_pinned, item_order FROM clipboard WHERE id = ?1",
                &[&id_b],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("读取");
        assert_eq!(is_pinned, 1);
        assert_eq!(item_order, 1, "置顶桶空 → order = max(pinned)+1 = 1");

        // 查询顺序：is_pinned DESC, item_order DESC, updated_at DESC
        let page = query_clipboard_items(QueryParams::default()).expect("查询应成功");
        let ids: Vec<i64> = page.items.iter().map(|i| i.id).collect();
        assert_eq!(ids, vec![id_b, id_c, id_a], "置顶优先，未置顶按 order DESC");
    }

    #[test]
    fn cross_bucket_move_is_noop_and_move_to_top_bumps_order() {
        let _guard = TEST_ENV_LOCK.lock();
        let db = TestDb::new();
        let id_a = seed_clip_row(&db, "ua", "a", 1, 1000);
        let id_b = seed_clip_row(&db, "ub", "b", 2, 2000);
        assert!(toggle_pin_clipboard_item(id_b).expect("置顶应成功"));

        // 跨桶移动：Ok 且不重排
        move_clipboard_item_by_id(id_b, id_a).expect("跨桶移动应 Ok");
        let (a_pin, a_order): (i64, i64) = db
            .query_row("SELECT is_pinned, item_order FROM clipboard WHERE id = ?1", &[&id_a], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("读取");
        let (b_pin, b_order): (i64, i64) = db
            .query_row("SELECT is_pinned, item_order FROM clipboard WHERE id = ?1", &[&id_b], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("读取");
        assert_eq!((a_pin, a_order), (0, 1), "未置顶项未被重排");
        assert_eq!((b_pin, b_order), (1, 1), "置顶项未被重排");

        // 取消置顶 → 未置顶桶 max+1
        assert!(!toggle_pin_clipboard_item(id_b).expect("取消置顶应成功"), "返回 false = 已取消");
        let (_, order): (i64, i64) = db
            .query_row("SELECT is_pinned, item_order FROM clipboard WHERE id = ?1", &[&id_b], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("读取");
        assert_eq!(order, 2, "未置顶桶 max(1)+1 = 2");

        // move to top → max(unpinned)+1
        move_clipboard_item_to_top(id_a).expect("移到顶部应成功");
        let (_, order_a): (i64, i64) = db
            .query_row("SELECT is_pinned, item_order FROM clipboard WHERE id = ?1", &[&id_a], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("读取");
        assert_eq!(order_a, 3, "move to top = max(unpinned)+1 = 3");
    }

    // 错误语义：DB 未初始化时查询报错
    #[test]
    fn queries_fail_with_clear_error_when_db_closed() {
        let _guard = TEST_ENV_LOCK.lock();
        crate::services::database::connection::close_database();
        let err = query_clipboard_items(QueryParams::default()).expect_err("应报错");
        assert_eq!(err, "数据库未初始化");
    }
}
