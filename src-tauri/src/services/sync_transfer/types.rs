use serde::{Deserialize, Serialize};

/// 同步/传输入口支持的工作模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    #[serde(rename = "webdav")]
    WebDav,
    Lan,
}

/// 具体的数据传输后端。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferBackend {
    WebDavStore,
    LanHttp,
}

/// 同步/传输页面逐步开放的能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncTransferFeature {
    Records,
    Files,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncTransferModeInfo {
    pub mode: SyncMode,
    pub backend: TransferBackend,
    pub features: Vec<SyncTransferFeature>,
    pub available: bool,
}

pub fn mode_infos() -> Vec<SyncTransferModeInfo> {
    vec![
        SyncTransferModeInfo {
            mode: SyncMode::WebDav,
            backend: TransferBackend::WebDavStore,
            features: vec![SyncTransferFeature::Records, SyncTransferFeature::Files],
            available: true,
        },
        SyncTransferModeInfo {
            mode: SyncMode::Lan,
            backend: TransferBackend::LanHttp,
            features: vec![SyncTransferFeature::Records, SyncTransferFeature::Files],
            available: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_infos_exact_two_modes() {
        let infos = mode_infos();
        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].mode, SyncMode::WebDav);
        assert_eq!(infos[0].backend, TransferBackend::WebDavStore);
        assert_eq!(infos[0].features, vec![SyncTransferFeature::Records, SyncTransferFeature::Files]);
        assert!(infos[0].available);
        assert_eq!(infos[1].mode, SyncMode::Lan);
        assert_eq!(infos[1].backend, TransferBackend::LanHttp);
        assert_eq!(infos[1].features, vec![SyncTransferFeature::Records, SyncTransferFeature::Files]);
        assert!(infos[1].available);
    }

    #[test]
    fn sync_mode_serde_names_are_snake_case() {
        assert_eq!(serde_json::to_string(&SyncMode::WebDav).unwrap(), "\"webdav\"");
        assert_eq!(serde_json::to_string(&SyncMode::Lan).unwrap(), "\"lan\"");
        assert_eq!(serde_json::from_str::<SyncMode>("\"webdav\"").unwrap(), SyncMode::WebDav);
        assert_eq!(serde_json::from_str::<SyncMode>("\"lan\"").unwrap(), SyncMode::Lan);
    }

    #[test]
    fn transfer_backend_serde_names_are_snake_case() {
        // serde 的 snake_case 按驼峰边界拆分：WebDavStore → web_dav_store
        assert_eq!(
            serde_json::to_string(&TransferBackend::WebDavStore).unwrap(),
            "\"web_dav_store\""
        );
        assert_eq!(serde_json::to_string(&TransferBackend::LanHttp).unwrap(), "\"lan_http\"");
        assert_eq!(
            serde_json::from_str::<TransferBackend>("\"web_dav_store\"").unwrap(),
            TransferBackend::WebDavStore
        );
        assert_eq!(
            serde_json::from_str::<TransferBackend>("\"lan_http\"").unwrap(),
            TransferBackend::LanHttp
        );
    }

    #[test]
    fn mode_info_json_round_trip() {
        let info = &mode_infos()[1];
        let json = serde_json::to_string(info).unwrap();
        let back: SyncTransferModeInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.mode, SyncMode::Lan);
        assert_eq!(back.backend, TransferBackend::LanHttp);
        assert_eq!(back.features.len(), 2);
        assert!(back.available);
    }
}
