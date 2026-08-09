use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::services::webdav_sync::types::{CloudGroup, CloudRecord};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanSyncSnapshot {
    pub device_id: String,
    pub history_states: HashMap<String, i64>,
    pub favorite_states: HashMap<String, i64>,
    pub groups: Vec<CloudGroup>,
    #[serde(default)]
    pub tombstone_states: HashMap<String, i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanRecordBatch {
    pub collection: String,
    pub records: Vec<CloudRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanGroupBatch {
    pub groups: Vec<CloudGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanTombstoneBatch {
    pub tombstones: Vec<crate::services::database::SyncTombstone>,
}

pub fn snapshot() -> Result<LanSyncSnapshot, String> {
    let device_id = super::runtime::device_id();
    Ok(LanSyncSnapshot {
        device_id: device_id.clone(),
        history_states: crate::services::database::webdav_history_record_states()?,
        favorite_states: crate::services::database::webdav_favorite_record_states()?,
        groups: crate::services::database::webdav_list_groups(&device_id)?,
        tombstone_states: crate::services::database::sync_tombstone_states()?,
    })
}

pub fn list_history_records_since(since_updated_at: Option<i64>) -> Result<LanRecordBatch, String> {
    let device_id = super::runtime::device_id();
    let mut records = crate::services::database::webdav_list_history_records(&device_id)?;
    if let Some(since_updated_at) = since_updated_at {
        records.retain(|record| record.updated_at > since_updated_at);
    }
    let records = crate::services::database::filter_records_not_deleted(
        crate::services::database::COLLECTION_HISTORY,
        &records,
    )?;
    Ok(LanRecordBatch {
        collection: "history".to_string(),
        records,
    })
}

pub fn list_favorite_records_since(since_updated_at: Option<i64>) -> Result<LanRecordBatch, String> {
    let device_id = super::runtime::device_id();
    let mut records = crate::services::database::webdav_list_favorite_records(&device_id)?;
    if let Some(since_updated_at) = since_updated_at {
        records.retain(|record| record.updated_at > since_updated_at);
    }
    let records = crate::services::database::filter_records_not_deleted(
        crate::services::database::COLLECTION_FAVORITES,
        &records,
    )?;
    Ok(LanRecordBatch {
        collection: "favorites".to_string(),
        records,
    })
}

pub fn list_groups() -> Result<LanGroupBatch, String> {
    let device_id = super::runtime::device_id();
    let groups = crate::services::database::webdav_list_groups(&device_id)?;
    Ok(LanGroupBatch {
        groups: crate::services::database::filter_groups_not_deleted(&groups)?,
    })
}

pub fn list_tombstones_since(since_deleted_at: Option<i64>) -> Result<LanTombstoneBatch, String> {
    Ok(LanTombstoneBatch {
        tombstones: crate::services::database::list_sync_tombstones_since(since_deleted_at)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_group() -> CloudGroup {
        CloudGroup {
            name: "work".to_string(),
            icon: "ti ti-briefcase".to_string(),
            color: "#3b82f6".to_string(),
            order: 1,
            source_device_id: "dev-a".to_string(),
            created_at: 100,
            updated_at: 200,
        }
    }

    #[test]
    fn snapshot_json_round_trip_preserves_all_fields() {
        let snapshot = LanSyncSnapshot {
            device_id: "dev-a".to_string(),
            history_states: [("u1".to_string(), 100)].into_iter().collect(),
            favorite_states: [("f1".to_string(), 200)].into_iter().collect(),
            groups: vec![sample_group()],
            tombstone_states: [("t1".to_string(), 300)].into_iter().collect(),
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        let back: LanSyncSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back.device_id, "dev-a");
        assert_eq!(back.history_states.get("u1"), Some(&100));
        assert_eq!(back.favorite_states.get("f1"), Some(&200));
        assert_eq!(back.groups.len(), 1);
        assert_eq!(back.groups[0].name, "work");
        assert_eq!(back.groups[0].color, "#3b82f6");
        assert_eq!(back.tombstone_states.get("t1"), Some(&300));
    }

    #[test]
    fn snapshot_deserializes_without_tombstone_states() {
        // 旧版本客户端不发送 tombstone_states → serde(default) 应为空 map
        let json = r#"{"device_id":"dev-a","history_states":{},"favorite_states":{},"groups":[]}"#;
        let snapshot: LanSyncSnapshot = serde_json::from_str(json).unwrap();
        assert!(snapshot.tombstone_states.is_empty());
    }

    #[test]
    fn record_batch_contract_collection_and_records() {
        let batch = LanRecordBatch {
            collection: "history".to_string(),
            records: vec![],
        };
        let json = serde_json::to_string(&batch).unwrap();
        let back: LanRecordBatch = serde_json::from_str(&json).unwrap();
        assert_eq!(back.collection, "history");
        assert!(back.records.is_empty());
    }

    #[test]
    fn group_batch_and_tombstone_batch_round_trip() {
        let groups = LanGroupBatch {
            groups: vec![sample_group()],
        };
        let back: LanGroupBatch = serde_json::from_str(&serde_json::to_string(&groups).unwrap()).unwrap();
        assert_eq!(back.groups[0].order, 1);
        assert_eq!(back.groups[0].source_device_id, "dev-a");

        let tombstones = LanTombstoneBatch { tombstones: vec![] };
        let back: LanTombstoneBatch =
            serde_json::from_str(&serde_json::to_string(&tombstones).unwrap()).unwrap();
        assert!(back.tombstones.is_empty());
    }
}
