use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const LABEL_PREFIX: &str = "transfer-shelf-";
pub const TASK_PROGRESS_EVENT: &str = "transfer-shelf-task-progress";
pub const STATE_CHANGED_EVENT: &str = "transfer-shelf-state-changed";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelfFileInfo {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    pub exists: bool,
    pub icon: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelfSummary {
    pub id: String,
    pub label: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelfSendTarget {
    pub peer_id: String,
    pub path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelfCloudUploadTarget {
    pub path: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelfSendError {
    pub peer_id: String,
    pub path: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelfFileProgress {
    pub path: String,
    pub sent_bytes: u64,
    pub total_bytes: u64,
    pub total: usize,
    pub done: usize,
    pub failed: usize,
    pub status: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelfSendTaskPayload {
    pub shelf_id: String,
    pub status: String,
    pub total: usize,
    pub done: usize,
    pub failed: usize,
    pub sent_bytes: u64,
    pub total_bytes: u64,
    pub current_path: Option<String>,
    pub current_file_name: Option<String>,
    pub errors: Vec<ShelfSendError>,
    pub file_progresses: Vec<ShelfFileProgress>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelfStateSnapshot {
    pub id: String,
    pub name: String,
    pub files: Vec<ShelfFileInfo>,
    pub selected_peer_ids: Vec<String>,
}

pub fn label_for(id: &str) -> String {
    format!("{}{}", LABEL_PREFIX, id)
}

pub fn describe_path(path: &str) -> ShelfFileInfo {
    let normalized_path = normalize_shell_path(path);
    let effective_path = if normalized_path != path && std::fs::metadata(&normalized_path).is_ok() {
        normalized_path.as_str()
    } else {
        path
    };
    let path_buf = PathBuf::from(effective_path);
    let metadata = std::fs::metadata(&path_buf).ok();
    let name = Path::new(effective_path)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(effective_path)
        .to_string();
    let icon = metadata
        .as_ref()
        .filter(|value| value.is_file())
        .and_then(|_| crate::utils::icon::get_file_icon_base64(effective_path)
            .or_else(|| crate::utils::icon::get_file_icon_base64(path)));

    ShelfFileInfo {
        path: effective_path.to_string(),
        name,
        size: metadata.as_ref().map(|value| value.len()).unwrap_or(0),
        is_dir: metadata.as_ref().map(|value| value.is_dir()).unwrap_or(false),
        exists: metadata.is_some(),
        icon,
    }
}

#[cfg(windows)]
fn normalize_shell_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!("\\\\{}", rest)
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        path.to_string()
    }
}

#[cfg(not(windows))]
fn normalize_shell_path(path: &str) -> String {
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static DIR_SEQ: AtomicUsize = AtomicUsize::new(0);

    fn unique_temp_path() -> std::path::PathBuf {
        let seq = DIR_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "qc_shelf_types_test_{}_{}",
            std::process::id(),
            seq
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        dir
    }

    // ---- WR-27: label_for ----
    #[test]
    fn label_for_prefixes_id_with_shelf_marker() {
        assert_eq!(label_for("42"), "transfer-shelf-42");
        assert_eq!(label_for(""), "transfer-shelf-");
        assert_eq!(label_for("a b"), "transfer-shelf-a b");
    }

    // ---- WR-28: normalize_shell_path ----
    #[cfg(windows)]
    #[test]
    fn normalize_shell_path_strips_win32_extended_prefixes() {
        assert_eq!(normalize_shell_path(r"\\?\UNC\server\share"), r"\\server\share");
        assert_eq!(normalize_shell_path(r"\\?\C:\dir\file.txt"), r"C:\dir\file.txt");
        assert_eq!(normalize_shell_path(r"C:\plain.txt"), r"C:\plain.txt");
    }

    #[cfg(not(windows))]
    #[test]
    fn normalize_shell_path_is_identity_on_non_windows() {
        assert_eq!(normalize_shell_path(r"\\?\UNC\server\share"), r"\\?\UNC\server\share");
        assert_eq!(normalize_shell_path(r"C:\foo"), r"C:\foo");
    }

    // ---- WR-29: describe_path ----
    #[test]
    fn describe_path_reports_existing_file_metadata() {
        let dir = unique_temp_path();
        let file = dir.join("doc.txt");
        std::fs::write(&file, b"abc").expect("写临时文件");
        let info = describe_path(file.to_str().unwrap());
        assert!(info.exists);
        assert!(!info.is_dir);
        assert_eq!(info.size, 3, "size = 文件字节数");
        assert_eq!(info.name, "doc.txt");
        assert_eq!(info.path, file.to_str().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn describe_path_reports_missing_file_with_lexical_name() {
        let path = "/nonexistent-qc-xyz/abc.txt";
        let info = describe_path(path);
        assert!(!info.exists);
        assert!(!info.is_dir);
        assert_eq!(info.size, 0, "缺失文件 size=0");
        assert_eq!(info.name, "abc.txt", "file_name 是纯词法结果");
        assert_eq!(info.path, path);
    }

    #[test]
    fn describe_path_falls_back_to_full_path_when_no_file_name() {
        let path = "/nonexistent-qc-xyz/..";
        let info = describe_path(path);
        assert!(!info.exists);
        assert_eq!(info.name, path, "无 file_name 时回退为整个路径");
    }

    #[test]
    fn describe_path_reports_directory() {
        let dir = unique_temp_path();
        let info = describe_path(dir.to_str().unwrap());
        assert!(info.exists);
        assert!(info.is_dir);
        assert_eq!(info.name, dir.file_name().unwrap().to_str().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

