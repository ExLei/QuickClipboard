use std::sync::Arc;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileTransferResult {
    pub saved: bool,
    pub path: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTransferProgress {
    pub transfer_id: String,
    pub device_id: String,
    pub file_path: String,
    pub file_name: String,
    pub sent_bytes: u64,
    pub total_bytes: u64,
    pub status: String,
}

pub type FileTransferProgressCallback = Arc<dyn Fn(FileTransferProgress) + Send + Sync + 'static>;

#[derive(Clone)]
pub struct FileTransferProgressReporter {
    transfer_id: String,
    device_id: String,
    file_path: String,
    file_name: String,
    total_bytes: u64,
    callback: FileTransferProgressCallback,
}

impl FileTransferProgressReporter {
    pub fn new(
        transfer_id: String,
        device_id: String,
        file_path: String,
        file_name: String,
        total_bytes: u64,
        callback: FileTransferProgressCallback,
    ) -> Self {
        Self {
            transfer_id,
            device_id,
            file_path,
            file_name,
            total_bytes,
            callback,
        }
    }

    pub fn emit(&self, status: &str, sent_bytes: u64) {
        (self.callback)(FileTransferProgress {
            transfer_id: self.transfer_id.clone(),
            device_id: self.device_id.clone(),
            file_path: self.file_path.clone(),
            file_name: self.file_name.clone(),
            sent_bytes,
            total_bytes: self.total_bytes,
            status: status.to_string(),
        });
    }
}

pub async fn send_file_to_peer(device_id: &str, file_path: &str) -> Result<FileTransferResult, String> {
    send_file_to_peer_with_progress(device_id, file_path, None, None).await
}

pub async fn send_file_to_peer_with_progress(
    device_id: &str,
    file_path: &str,
    transfer_id: Option<String>,
    progress: Option<FileTransferProgressCallback>,
) -> Result<FileTransferResult, String> {
    let peer = super::peer_store::list_peers()
        .into_iter()
        .find(|peer| peer.device_id == device_id)
        .ok_or_else(|| "未找到已配对设备".to_string())?;
    let (file_name, path, size) = super::files::outgoing_file_info(file_path)?;
    let reporter = progress.map(|callback| {
        FileTransferProgressReporter::new(
            transfer_id.unwrap_or_else(|| format!("{}:{}", device_id, file_path)),
            device_id.to_string(),
            path.to_string_lossy().to_string(),
            file_name.clone(),
            size,
            callback,
        )
    });
    super::http_client::send_peer_file_stream(&peer, &file_name, path, size, reporter).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    #[tokio::test]
    async fn send_file_to_peer_fails_when_peer_not_paired() {
        let err = send_file_to_peer_with_progress("missing-device", "/tmp/whatever.txt", None, None)
            .await
            .unwrap_err();
        assert_eq!(err, "未找到已配对设备");
    }

    #[test]
    fn progress_reporter_emits_exact_fields() {
        let seen = Arc::new(Mutex::new(None));
        let callback: FileTransferProgressCallback = {
            let seen = seen.clone();
            Arc::new(move |progress| {
                *seen.lock() = Some(progress);
            })
        };

        let reporter = FileTransferProgressReporter::new(
            "t-1".to_string(),
            "dev-9".to_string(),
            "/tmp/a.bin".to_string(),
            "a.bin".to_string(),
            1024,
            callback,
        );
        reporter.emit("sending", 512);

        let event = seen.lock().take().expect("回调必须被调用");
        assert_eq!(event.transfer_id, "t-1");
        assert_eq!(event.device_id, "dev-9");
        assert_eq!(event.file_path, "/tmp/a.bin");
        assert_eq!(event.file_name, "a.bin");
        assert_eq!(event.sent_bytes, 512);
        assert_eq!(event.total_bytes, 1024);
        assert_eq!(event.status, "sending");
    }

    #[test]
    fn progress_reporter_reports_completion_status() {
        let seen = Arc::new(Mutex::new(None));
        let callback: FileTransferProgressCallback = {
            let seen = seen.clone();
            Arc::new(move |progress| {
                *seen.lock() = Some(progress);
            })
        };
        let reporter = FileTransferProgressReporter::new(
            "t-2".to_string(),
            "dev-9".to_string(),
            "p".to_string(),
            "n".to_string(),
            100,
            callback,
        );
        reporter.emit("done", 100);
        let event = seen.lock().take().unwrap();
        assert_eq!(event.status, "done");
        assert_eq!(event.sent_bytes, 100);
    }

    #[test]
    fn progress_reporter_echoes_transfer_id_passed_at_construction() {
        // 注意：本测试只覆盖 emit 透传 —— 传入的 transfer_id 原样出现在回调事件里。
        // `send_file_to_peer_with_progress` 中 `transfer_id.unwrap_or_else(||
        // format!("{}:{}", device_id, file_path))` 的默认值 fallback 分支
        // 需要已配对设备 + 真实文件 + 网络流，无法在单元测试中触发，未覆盖。
        let seen = Arc::new(Mutex::new(None));
        let callback: FileTransferProgressCallback = {
            let seen = seen.clone();
            Arc::new(move |progress| {
                *seen.lock() = Some(progress);
            })
        };
        let reporter = FileTransferProgressReporter::new(
            format!("{}:{}", "dev-9", "/tmp/x.bin"),
            "dev-9".to_string(),
            "/tmp/x.bin".to_string(),
            "x.bin".to_string(),
            0,
            callback,
        );
        reporter.emit("sending", 0);
        let event = seen.lock().take().unwrap();
        assert_eq!(event.transfer_id, "dev-9:/tmp/x.bin");
    }
}
