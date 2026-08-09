use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const CHUNK_RECORD_LIMIT: usize = 500;

#[derive(Debug, Clone)]
pub struct WebdavConfig {
    pub url: String,
    pub username: String,
    pub password: String,
    pub root_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebdavStatus {
    pub enabled: bool,
    pub configured: bool,
    pub auto_push: bool,
    pub auto_pull: bool,
    pub running: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncReport {
    pub pushed: u32,
    pub pulled: u32,
    pub errors: Vec<String>,
    pub pushed_clipboard: u32,
    pub pushed_favorites: u32,
    pub pushed_groups: u32,
    pub pulled_clipboard: u32,
    pub pulled_favorites: u32,
    pub pulled_groups: u32,
    pub pushed_items: Vec<SyncReportItem>,
    pub pulled_items: Vec<SyncReportItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncReportItem {
    pub category: String,
    pub id: String,
    pub summary: String,
    pub source_device_id: String,
    pub updated_at: i64,
}

impl CloudRecord {
    pub fn report_item(&self, category: &str) -> SyncReportItem {
        SyncReportItem {
            category: category.to_string(),
            id: self.uuid.clone(),
            summary: summarize_record(self),
            source_device_id: self.source_device_id.clone(),
            updated_at: self.updated_at,
        }
    }
}

fn summarize_record(record: &CloudRecord) -> String {
    let raw = if !record.title.trim().is_empty() {
        record.title.trim()
    } else {
        record.content.trim()
    };

    let mut summary = raw.chars().take(40).collect::<String>();
    if raw.chars().count() > 40 {
        summary.push('…');
    }
    summary
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncIndex {
    pub entries: HashMap<String, SyncIndexEntry>,
    pub next_chunk: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncIndexEntry {
    pub chunk: u32,
    pub updated_at: i64,
    pub source_device_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecordChunk {
    pub records: HashMap<String, CloudRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudRecord {
    pub uuid: String,
    pub source_device_id: String,
    #[serde(default)]
    pub is_remote: bool,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html_content: Option<String>,
    pub content_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_app: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_icon_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub char_count: Option<i64>,
    #[serde(default)]
    pub title: String,
    #[serde(default = "default_group_name")]
    pub group_name: String,
    #[serde(default)]
    pub item_order: i64,
    #[serde(default)]
    pub paste_count: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct CloudRecordMeta {
    pub uuid: String,
    pub updated_at: i64,
    pub image_id: Option<String>,
}

fn default_group_name() -> String {
    "全部".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroupList {
    pub groups: Vec<CloudGroup>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TombstoneList {
    #[serde(default)]
    pub tombstones: Vec<crate::services::database::SyncTombstone>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImageFileIndex {
    #[serde(default)]
    pub images: HashMap<String, ImageFileIndexEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImageFileIndexEntry {
    pub uploaded_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudGroup {
    pub name: String,
    pub icon: String,
    pub color: String,
    pub order: i32,
    pub source_device_id: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy)]
pub enum SyncCollection {
    History,
    Favorites,
}

impl SyncCollection {
    pub fn dir(self) -> &'static str {
        match self {
            SyncCollection::History => "history",
            SyncCollection::Favorites => "favorites",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_with_title(title: &str) -> CloudRecord {
        CloudRecord {
            uuid: "uuid-1".to_string(),
            source_device_id: "dev".to_string(),
            is_remote: false,
            content: "content-body".to_string(),
            html_content: None,
            content_type: "text".to_string(),
            image_id: None,
            source_app: None,
            source_icon_hash: None,
            char_count: None,
            title: title.to_string(),
            group_name: "全部".to_string(),
            item_order: 0,
            paste_count: 0,
            created_at: 1,
            updated_at: 2,
        }
    }

    #[test]
    fn report_item_prefers_trimmed_title_over_content() {
        let record = record_with_title("  Hello World  ");
        let item = record.report_item("clipboard");
        assert_eq!(item.category, "clipboard");
        assert_eq!(item.id, "uuid-1");
        assert_eq!(item.summary, "Hello World");
        assert_eq!(item.source_device_id, "dev");
        assert_eq!(item.updated_at, 2);
    }

    #[test]
    fn report_item_falls_back_to_trimmed_content() {
        let mut record = record_with_title("");
        record.content = "  content fallback  ".to_string();
        let item = record.report_item("favorites");
        assert_eq!(item.summary, "content fallback");
    }

    #[test]
    fn report_item_truncates_at_40_chars_with_ellipsis() {
        let record = record_with_title(&"x".repeat(41));
        let item = record.report_item("clipboard");
        assert_eq!(item.summary.chars().count(), 41);
        assert_eq!(&item.summary[..40], "x".repeat(40));
        assert!(item.summary.ends_with('…'));
    }

    #[test]
    fn report_item_exact_40_chars_gets_no_ellipsis() {
        let record = record_with_title(&"y".repeat(40));
        let item = record.report_item("clipboard");
        assert_eq!(item.summary, "y".repeat(40));
        assert!(!item.summary.ends_with('…'));
    }

    #[test]
    fn report_item_counts_multibyte_chars_not_bytes() {
        // 21 个汉字 > 40 字节，但按字符数 21 < 40，不应截断
        let title = "汉".repeat(21);
        let record = record_with_title(&title);
        let item = record.report_item("clipboard");
        assert_eq!(item.summary, title);

        // 41 个汉字按字符数截断为 40 + 省略号
        let record = record_with_title(&"汉".repeat(41));
        let item = record.report_item("clipboard");
        assert_eq!(item.summary.chars().count(), 41);
        assert_eq!(item.summary.chars().take(40).collect::<String>(), "汉".repeat(40));
        assert!(item.summary.ends_with('…'));
    }

    #[test]
    fn cloud_record_applies_serde_defaults_for_missing_fields() {
        let json = r#"{
            "uuid": "u1",
            "source_device_id": "dev",
            "content": "hello",
            "content_type": "text",
            "created_at": 1,
            "updated_at": 2
        }"#;
        let record: CloudRecord = serde_json::from_str(json).expect("minimal record must parse");
        assert_eq!(record.group_name, "全部");
        assert!(!record.is_remote);
        assert_eq!(record.item_order, 0);
        assert_eq!(record.paste_count, 0);
        assert_eq!(record.title, "");
        assert!(record.html_content.is_none());
        assert!(record.image_id.is_none());
    }

    #[test]
    fn cloud_record_preserves_explicit_group_name() {
        let json = r#"{
            "uuid": "u1",
            "source_device_id": "dev",
            "content": "hello",
            "content_type": "text",
            "group_name": "工作",
            "created_at": 1,
            "updated_at": 2
        }"#;
        let record: CloudRecord = serde_json::from_str(json).unwrap();
        assert_eq!(record.group_name, "工作");
    }

    #[test]
    fn cloud_record_serialization_omits_none_optionals() {
        let record = record_with_title("t");
        let value: serde_json::Value = serde_json::to_value(&record).unwrap();
        // 字段名保持 snake_case（无 rename_all）
        assert!(value.get("html_content").is_none());
        assert!(value.get("image_id").is_none());
        assert!(value.get("source_app").is_none());
        assert!(value.get("source_icon_hash").is_none());
        assert!(value.get("char_count").is_none());
        assert_eq!(value["group_name"], "全部");

        let mut with_image = record_with_title("t");
        with_image.image_id = Some("img-1".to_string());
        with_image.char_count = Some(5);
        let value: serde_json::Value = serde_json::to_value(&with_image).unwrap();
        assert_eq!(value["image_id"], "img-1");
        assert_eq!(value["char_count"], 5);
    }

    #[test]
    fn sync_collection_dir_mapping_is_exact() {
        assert_eq!(SyncCollection::History.dir(), "history");
        assert_eq!(SyncCollection::Favorites.dir(), "favorites");
    }

    #[test]
    fn chunk_record_limit_is_500() {
        assert_eq!(CHUNK_RECORD_LIMIT, 500);
    }

    #[test]
    fn sync_report_default_is_zeroed() {
        let report = SyncReport::default();
        assert_eq!(report.pushed, 0);
        assert_eq!(report.pulled, 0);
        assert!(report.errors.is_empty());
        assert!(report.pushed_items.is_empty());
        assert!(report.pulled_items.is_empty());
    }
}
