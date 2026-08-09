use once_cell::sync::Lazy;
use uuid::Uuid;

const DEVICE_ID_KEY: &str = "sync_transfer_device_id";
const LEGACY_SYNC_TRANSFER_LAN_DEVICE_ID_KEY: &str = "sync_transfer_lan_device_id";

static DEVICE_ID: Lazy<String> = Lazy::new(load_or_create_device_id);

pub fn device_id() -> String {
    DEVICE_ID.clone()
}

fn load_or_create_device_id() -> String {
    if let Some(id) = stored_device_id(DEVICE_ID_KEY) {
        return id;
    }

    if let Some(id) = stored_device_id(LEGACY_SYNC_TRANSFER_LAN_DEVICE_ID_KEY) {
        let _ = crate::services::store::set(DEVICE_ID_KEY, &id);
        return id;
    }

    let id = Uuid::new_v4().to_string();
    let _ = crate::services::store::set(DEVICE_ID_KEY, &id);
    id
}

fn stored_device_id(key: &str) -> Option<String> {
    crate::services::store::get::<String>(key)
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_uuid_v4(value: &str) -> bool {
        let bytes = value.as_bytes();
        bytes.len() == 36
            && bytes[8] == b'-'
            && bytes[13] == b'-'
            && bytes[18] == b'-'
            && bytes[23] == b'-'
            && bytes[14] == b'4'
            && bytes[19].is_ascii_hexdigit()
            && (bytes[19] == b'8' || bytes[19] == b'9' || bytes[19] == b'a' || bytes[19] == b'b')
            && value.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
    }

    #[test]
    fn device_id_is_uuid_v4_format() {
        let id = device_id();
        assert!(!id.is_empty());
        assert!(is_uuid_v4(&id), "device_id 必须是 UUID v4: {}", id);
        // 进程内稳定：两次调用返回同一 id（跨进程持久化依赖 store，未覆盖）
        assert_eq!(device_id(), id);
    }

    #[test]
    fn stored_device_id_filters_empty_values() {
        // store 未初始化（无 AppHandle）→ 任何 key 都返回 None
        assert_eq!(stored_device_id(DEVICE_ID_KEY), None);
        assert_eq!(stored_device_id(LEGACY_SYNC_TRANSFER_LAN_DEVICE_ID_KEY), None);
    }
}
