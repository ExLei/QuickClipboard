pub mod device_identity;
pub mod lan;
pub mod sync_plan;
pub mod types;

pub use types::{mode_infos, SyncTransferModeInfo};

pub fn device_id() -> String {
    device_identity::device_id()
}

pub fn lan_status() -> lan::LanRuntimeStatus {
    lan::runtime::status()
}

pub async fn lan_start_http_server(app: tauri::AppHandle) -> Result<u16, String> {
    lan::http_server::start(app, Default::default()).await
}

pub async fn lan_stop_http_server() {
    lan::http_server::stop().await
}

pub fn lan_refresh_pairing_code() -> lan::PairingCodeView {
    lan::runtime::refresh_pairing_code()
}

pub fn lan_list_paired_peers() -> Vec<lan::PairedPeerInfo> {
    lan::peer_store::list_peer_infos()
}

pub fn lan_remove_paired_peer(device_id: &str) -> Result<bool, String> {
    lan::peer_store::remove_peer(device_id)
}

pub async fn lan_pair_with_peer(base_url: String, pairing_code: String) -> Result<lan::PairedPeerInfo, String> {
    lan::http_client::pair_with_peer(base_url, pairing_code).await
}

pub fn lan_snapshot() -> Result<lan::LanSyncSnapshot, String> {
    lan::snapshot::snapshot()
}

pub async fn lan_fetch_peer_snapshot(device_id: &str) -> Result<lan::LanSyncSnapshot, String> {
    let peer = lan::peer_store::list_peers()
        .into_iter()
        .find(|peer| peer.device_id == device_id)
        .ok_or_else(|| "未找到已配对设备".to_string())?;
    let snapshot = lan::http_client::fetch_peer_snapshot(&peer).await?;
    let _ = lan::peer_store::mark_peer_seen(device_id);
    Ok(snapshot)
}

pub async fn lan_discover_peers(timeout_ms: u64) -> Result<Vec<lan::DiscoveredLanPeer>, String> {
    lan::discovery::discover(timeout_ms).await
}

pub fn lan_auto_sync_status() -> lan::LanAutoSyncStatus {
    lan::auto_sync::status()
}

pub fn lan_update_auto_sync_settings(settings: lan::LanAutoSyncSettings) -> Result<lan::LanAutoSyncSettings, String> {
    lan::auto_sync::update_settings(settings)
}

pub fn lan_notify_local_change(app: tauri::AppHandle, reason: &'static str) {
    crate::services::webdav_sync::notify_local_change(app.clone(), reason);
    lan::auto_sync::notify_local_change(app, reason);
}

pub async fn lan_start_configured_services(app: tauri::AppHandle) {
    let settings = lan::auto_sync::settings();
    if !settings.receive_enabled {
        return;
    }
    let _ = lan::http_server::start(app, Default::default()).await;
}

pub async fn lan_pull_from_peer(device_id: &str) -> Result<crate::services::webdav_sync::SyncReport, String> {
    let report = lan::pull::pull_from_peer(device_id).await?;
    let _ = lan::peer_store::mark_peer_seen(device_id);
    Ok(report)
}

pub async fn lan_push_to_peer(device_id: &str) -> Result<crate::services::webdav_sync::SyncReport, String> {
    let report = lan::push::push_to_peer(device_id).await?;
    let _ = lan::peer_store::mark_peer_seen(device_id);
    Ok(report)
}

pub async fn lan_send_file_to_peer(device_id: &str, file_path: &str) -> Result<lan::FileTransferResult, String> {
    let result = lan::transfer::send_file_to_peer(device_id, file_path).await?;
    let _ = lan::peer_store::mark_peer_seen(device_id);
    Ok(result)
}

pub async fn lan_send_file_to_peer_with_progress(
    device_id: &str,
    file_path: &str,
    transfer_id: Option<String>,
    progress: Option<lan::FileTransferProgressCallback>,
) -> Result<lan::FileTransferResult, String> {
    let result = lan::transfer::send_file_to_peer_with_progress(device_id, file_path, transfer_id, progress).await?;
    let _ = lan::peer_store::mark_peer_seen(device_id);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_infos_expose_webdav_and_lan() {
        let infos = mode_infos();
        assert_eq!(infos.len(), 2);
        assert_eq!(infos[1].mode, types::SyncMode::Lan);
        assert_eq!(infos[1].backend, types::TransferBackend::LanHttp);
    }

    #[test]
    fn device_id_passthrough_is_uuid() {
        let id = device_id();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
    }

    #[test]
    fn lan_list_paired_peers_is_empty_without_store() {
        assert!(lan_list_paired_peers().is_empty());
    }

    #[test]
    fn lan_remove_paired_peer_returns_false_without_store() {
        assert_eq!(lan_remove_paired_peer("missing"), Ok(false));
    }

    #[test]
    fn lan_update_auto_sync_settings_errors_without_store() {
        let err = lan_update_auto_sync_settings(lan::LanAutoSyncSettings::default()).unwrap_err();
        assert_eq!(err, "AppHandle 未初始化");
    }

    #[test]
    fn lan_auto_sync_status_defaults_without_store() {
        let status = lan_auto_sync_status();
        assert!(!status.settings.send_enabled);
        assert!(!status.settings.receive_enabled);
        assert!(status.last_report.is_none());
    }

    #[test]
    fn lan_refresh_pairing_code_returns_full_attempts() {
        let _guard = lan::runtime::tests::PAIRING_TEST_LOCK.lock();
        let view = lan_refresh_pairing_code();
        assert_eq!(view.pairing_code.len(), 6);
        assert_eq!(view.remaining_attempts, 5);
    }

    #[tokio::test]
    async fn lan_fetch_peer_snapshot_fails_without_paired_peer() {
        let err = lan_fetch_peer_snapshot("missing-device").await.unwrap_err();
        assert_eq!(err, "未找到已配对设备");
    }

    #[tokio::test]
    async fn lan_push_to_peer_fails_without_paired_peer() {
        let err = lan_push_to_peer("missing-device").await.unwrap_err();
        assert_eq!(err, "未找到已配对设备");
    }

    #[tokio::test]
    async fn lan_pull_from_peer_fails_without_paired_peer() {
        let err = lan_pull_from_peer("missing-device").await.unwrap_err();
        assert_eq!(err, "未找到已配对设备");
    }

    #[tokio::test]
    async fn lan_send_file_to_peer_fails_without_paired_peer() {
        let err = lan_send_file_to_peer("missing-device", "/tmp/x.txt").await.unwrap_err();
        assert_eq!(err, "未找到已配对设备");
    }
}
