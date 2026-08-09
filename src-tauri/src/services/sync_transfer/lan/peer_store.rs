use serde::{Deserialize, Serialize};

const PAIRED_PEERS_KEY: &str = "sync_transfer_lan_paired_peers";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedPeer {
    pub device_id: String,
    pub device_name: String,
    pub base_url: String,
    pub peer_token: String,
    pub paired_at_ms: i64,
    pub last_seen_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedPeerInfo {
    pub device_id: String,
    pub device_name: String,
    pub base_url: String,
    pub paired_at_ms: i64,
    pub last_seen_at_ms: Option<i64>,
}

impl PairedPeer {
    pub fn new(device_id: String, device_name: String, base_url: String, peer_token: String) -> Self {
        Self {
            device_id,
            device_name,
            base_url,
            peer_token,
            paired_at_ms: chrono::Utc::now().timestamp_millis(),
            last_seen_at_ms: None,
        }
    }

    pub fn info(&self) -> PairedPeerInfo {
        PairedPeerInfo {
            device_id: self.device_id.clone(),
            device_name: self.device_name.clone(),
            base_url: self.base_url.clone(),
            paired_at_ms: self.paired_at_ms,
            last_seen_at_ms: self.last_seen_at_ms,
        }
    }
}

pub fn list_peers() -> Vec<PairedPeer> {
    let peers = crate::services::store::get::<Vec<PairedPeer>>(PAIRED_PEERS_KEY).unwrap_or_default();
    let original_len = peers.len();
    let peers = dedupe_peers(peers);
    if peers.len() != original_len {
        let _ = save_peers(&peers);
    }
    peers
}

pub fn list_peer_infos() -> Vec<PairedPeerInfo> {
    list_peers().into_iter().map(|peer| peer.info()).collect()
}

pub fn save_peers(peers: &[PairedPeer]) -> Result<(), String> {
    crate::services::store::set(PAIRED_PEERS_KEY, &peers.to_vec())
}

pub fn upsert_peer(peer: PairedPeer) -> Result<(), String> {
    let mut peers = list_peers();
    peers.retain(|item| !same_peer_identity(item, &peer));
    peers.push(peer);
    save_peers(&peers)
}

fn dedupe_peers(peers: Vec<PairedPeer>) -> Vec<PairedPeer> {
    let mut out: Vec<PairedPeer> = Vec::new();
    for peer in peers {
        if let Some(index) = out.iter().position(|item| same_peer_identity(item, &peer)) {
            out[index] = peer;
        } else {
            out.push(peer);
        }
    }
    out
}

fn same_peer_identity(left: &PairedPeer, right: &PairedPeer) -> bool {
    if left.device_id == right.device_id {
        return true;
    }
    let left_base_url = normalized_base_url(&left.base_url);
    !left_base_url.is_empty() && left_base_url == normalized_base_url(&right.base_url)
}

fn normalized_base_url(base_url: &str) -> String {
    base_url.trim().trim_end_matches('/').to_ascii_lowercase()
}

pub fn mark_peer_seen(device_id: &str) -> Result<(), String> {
    let mut peers = list_peers();
    let Some(peer) = peers.iter_mut().find(|peer| peer.device_id == device_id) else {
        return Ok(());
    };
    peer.last_seen_at_ms = Some(chrono::Utc::now().timestamp_millis());
    save_peers(&peers)
}

pub fn remove_peer(device_id: &str) -> Result<bool, String> {
    let mut peers = list_peers();
    let before = peers.len();
    peers.retain(|peer| peer.device_id != device_id);
    if peers.len() == before {
        return Ok(false);
    }
    save_peers(&peers)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(device_id: &str, base_url: &str) -> PairedPeer {
        PairedPeer {
            device_id: device_id.to_string(),
            device_name: format!("dev-{}", device_id),
            base_url: base_url.to_string(),
            peer_token: format!("tok-{}", device_id),
            paired_at_ms: 1000,
            last_seen_at_ms: None,
        }
    }

    #[test]
    fn normalized_base_url_trims_lowercases_strips_trailing_slash() {
        assert_eq!(normalized_base_url("  HTTP://EXAMPLE.COM:35691/// "), "http://example.com:35691");
        assert_eq!(normalized_base_url("http://192.168.1.5"), "http://192.168.1.5");
        assert_eq!(normalized_base_url(""), "");
    }

    #[test]
    fn same_peer_identity_matches_device_id_or_normalized_base_url() {
        assert!(same_peer_identity(&peer("a", "http://x:1"), &peer("a", "http://y:2")));
        assert!(same_peer_identity(
            &peer("a", "http://X:1/"),
            &peer("b", "HTTP://x:1")
        ), "不同 device_id 但 base_url 规范化后相同 → 同一身份");
        assert!(!same_peer_identity(&peer("a", "http://x:1"), &peer("b", "http://x:2")));
        assert!(!same_peer_identity(&peer("a", ""), &peer("b", "")), "空 base_url 不构成身份");
    }

    #[test]
    fn dedupe_peers_keeps_last_occurrence_per_identity() {
        let peers = vec![
            peer("a", "http://x:1"),
            peer("a", "http://x:1"),
            peer("b", "http://y:1"),
        ];
        let out = dedupe_peers(peers);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].device_id, "a");
        assert_eq!(out[1].device_id, "b");
    }

    #[test]
    fn dedupe_peers_by_normalized_base_url_keeps_last() {
        let peers = vec![peer("a", "http://same:1"), peer("b", "HTTP://SAME:1/")];
        let out = dedupe_peers(peers);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].device_id, "b", "后出现的条目胜出");
    }

    #[test]
    fn paired_peer_new_sets_paired_at_and_no_last_seen() {
        let p = PairedPeer::new("d1".to_string(), "name".to_string(), "http://x".to_string(), "tok".to_string());
        assert_eq!(p.device_id, "d1");
        assert_eq!(p.device_name, "name");
        assert_eq!(p.base_url, "http://x");
        assert_eq!(p.peer_token, "tok");
        assert!(p.paired_at_ms > 0);
        assert_eq!(p.last_seen_at_ms, None);
    }

    #[test]
    fn paired_peer_info_drops_token_and_copies_fields() {
        let p = PairedPeer {
            device_id: "d1".to_string(),
            device_name: "name".to_string(),
            base_url: "http://x".to_string(),
            peer_token: "secret-token".to_string(),
            paired_at_ms: 123,
            last_seen_at_ms: Some(456),
        };
        let info = p.info();
        assert_eq!(info.device_id, "d1");
        assert_eq!(info.device_name, "name");
        assert_eq!(info.base_url, "http://x");
        assert_eq!(info.paired_at_ms, 123);
        assert_eq!(info.last_seen_at_ms, Some(456));
    }

    // ---- 未初始化 store（无 AppHandle）的确定性契约 ----

    #[test]
    fn list_peers_is_empty_without_store() {
        assert!(list_peers().is_empty());
    }

    #[test]
    fn list_peer_infos_is_empty_without_store() {
        assert!(list_peer_infos().is_empty());
    }

    #[test]
    fn save_peers_errors_without_store() {
        assert_eq!(save_peers(&[]), Err("AppHandle 未初始化".to_string()));
    }

    #[test]
    fn upsert_peer_errors_without_store() {
        assert_eq!(
            upsert_peer(peer("a", "http://x:1")),
            Err("AppHandle 未初始化".to_string())
        );
    }

    #[test]
    fn mark_peer_seen_is_noop_without_peers() {
        assert_eq!(mark_peer_seen("missing-device"), Ok(()));
    }

    #[test]
    fn remove_peer_returns_false_without_peers() {
        assert_eq!(remove_peer("missing-device"), Ok(false));
    }
}
