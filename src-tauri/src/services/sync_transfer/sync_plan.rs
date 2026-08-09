use std::collections::HashMap;

use crate::services::webdav_sync::types::{CloudGroup, CloudRecordMeta};

pub fn record_metas_newer_than_remote(
    local_records: Vec<CloudRecordMeta>,
    remote_states: &HashMap<String, i64>,
) -> Vec<CloudRecordMeta> {
    local_records
        .into_iter()
        .filter(|record| {
            !record.uuid.trim().is_empty()
                && remote_states
                    .get(&record.uuid)
                    .map(|remote_updated_at| record.updated_at > *remote_updated_at)
                    .unwrap_or(true)
        })
        .collect()
}

pub fn groups_newer_than_remote(
    local_groups: Vec<CloudGroup>,
    remote_groups: &[CloudGroup],
) -> Vec<CloudGroup> {
    let remote_states = remote_groups
        .iter()
        .map(|group| (group.name.as_str(), group.updated_at))
        .collect::<HashMap<_, _>>();

    local_groups
        .into_iter()
        .filter(|group| {
            !group.name.trim().is_empty()
                && remote_states
                    .get(group.name.as_str())
                    .map(|remote_updated_at| group.updated_at > *remote_updated_at)
                    .unwrap_or(true)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(uuid: &str, updated_at: i64) -> CloudRecordMeta {
        CloudRecordMeta {
            uuid: uuid.to_string(),
            updated_at,
            image_id: None,
        }
    }

    fn group(name: &str, updated_at: i64) -> CloudGroup {
        CloudGroup {
            name: name.to_string(),
            icon: String::new(),
            color: String::new(),
            order: 0,
            source_device_id: String::new(),
            created_at: 0,
            updated_at,
        }
    }

    #[test]
    fn record_metas_unknown_remote_included_regardless_of_updated_at() {
        let local = vec![meta("u1", 1)];
        let out = record_metas_newer_than_remote(local, &HashMap::new());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].uuid, "u1");
    }

    #[test]
    fn record_metas_newer_than_remote_included_exact_and_older_excluded() {
        let local = vec![
            meta("newer", 200),
            meta("equal", 100),
            meta("older", 50),
        ];
        let remote = [("newer".to_string(), 100), ("equal".to_string(), 100), ("older".to_string(), 100)]
            .into_iter()
            .collect::<HashMap<_, _>>();
        let out = record_metas_newer_than_remote(local, &remote);
        assert_eq!(out.len(), 1, "仅严格更新的记录应被推送");
        assert_eq!(out[0].uuid, "newer");
    }

    #[test]
    fn record_metas_empty_or_whitespace_uuid_never_included() {
        let local = vec![meta("", 999), meta("   ", 999), meta("u-ok", 999)];
        let out = record_metas_newer_than_remote(local, &HashMap::new());
        assert_eq!(out.len(), 1, "空 uuid 即使远端无记录也必须排除");
        assert_eq!(out[0].uuid, "u-ok");
    }

    #[test]
    fn record_metas_order_preserved() {
        let local = vec![meta("a", 2), meta("b", 3), meta("c", 4)];
        let remote = [("a".to_string(), 1), ("b".to_string(), 3), ("c".to_string(), 3)]
            .into_iter()
            .collect::<HashMap<_, _>>();
        let out = record_metas_newer_than_remote(local, &remote);
        let uuids = out.iter().map(|m| m.uuid.as_str()).collect::<Vec<_>>();
        assert_eq!(uuids, vec!["a", "c"], "输入顺序保持");
    }

    #[test]
    fn groups_newer_than_remote_unknown_included() {
        let local = vec![group("全部", 5)];
        let out = groups_newer_than_remote(local, &[]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "全部");
    }

    #[test]
    fn groups_newer_than_remote_strictly_newer_only() {
        let local = vec![group("g1", 200), group("g2", 100), group("g3", 50)];
        let remote = vec![group("g1", 100), group("g2", 100), group("g3", 100)];
        let out = groups_newer_than_remote(local, &remote);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "g1");
    }

    #[test]
    fn groups_whitespace_name_excluded() {
        let local = vec![group("", 999), group("  ", 999), group("work", 1)];
        let out = groups_newer_than_remote(local, &[]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "work");
    }

    #[test]
    fn groups_order_preserved() {
        let local = vec![group("a", 2), group("b", 3), group("c", 4)];
        let remote = vec![group("a", 1), group("b", 3), group("c", 3)];
        let out = groups_newer_than_remote(local, &remote);
        let names = out.iter().map(|g| g.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names, vec!["a", "c"]);
    }
}
