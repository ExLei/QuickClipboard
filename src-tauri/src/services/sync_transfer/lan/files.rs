use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::services::webdav_sync::types::CloudRecord;

pub const MAX_DIRECT_TRANSFER_FILE_SIZE: u64 = 512 * 1024 * 1024;

static RESERVED_RECEIVED_FILE_PATHS: Lazy<Mutex<HashSet<PathBuf>>> = Lazy::new(|| Mutex::new(HashSet::new()));
static RECEIVED_FILE_INDEX_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

const RECEIVED_FILE_INDEX_NAME: &str = "index.json";

#[derive(Debug, Clone)]
pub struct ReceivedFileReservation {
    pub final_path: PathBuf,
    pub temp_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceivedFileMetadata {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub source_device_id: String,
    pub source_device_name: String,
    pub received_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ReceivedFileIndex {
    #[serde(default)]
    files: HashMap<String, ReceivedFileMetadata>,
}

pub fn collect_record_image_ids(records: &[CloudRecord]) -> Vec<String> {
    let mut ids = HashSet::new();
    for record in records {
        let Some(raw) = record.image_id.as_deref() else { continue; };
        for image_id in raw.split(',').map(|item| item.trim()).filter(|item| !item.is_empty()) {
            if is_valid_image_id(image_id) {
                ids.insert(image_id.to_string());
            }
        }
    }
    ids.into_iter().collect()
}

pub fn read_image_file(image_id: &str) -> Result<Option<Vec<u8>>, String> {
    let path = image_path(image_id)?;
    if !path.exists() {
        return Ok(None);
    }
    std::fs::read(path).map(Some).map_err(|e| format!("读取局域网同步图片失败: {}", e))
}

pub fn save_image_file(image_id: &str, bytes: &[u8]) -> Result<(), String> {
    let path = image_path(image_id)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建局域网同步图片目录失败: {}", e))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("保存局域网同步图片失败: {}", e))
}

pub fn outgoing_file_info(path: &str) -> Result<(String, PathBuf, u64), String> {
    let path = PathBuf::from(path);
    let metadata = std::fs::metadata(&path).map_err(|e| format!("读取待传输文件信息失败: {}", e))?;
    if !metadata.is_file() {
        return Err("只能传输普通文件".to_string());
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "文件名无效".to_string())?
        .to_string();
    Ok((file_name, path, metadata.len()))
}

pub fn prepare_received_file(file_name: &str) -> Result<ReceivedFileReservation, String> {
    let safe_name = sanitize_file_name(file_name)?;
    let dir = received_files_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建接收文件目录失败: {}", e))?;
    let mut reserved = RESERVED_RECEIVED_FILE_PATHS
        .lock()
        .map_err(|_| "接收文件路径状态异常".to_string())?;
    let mut reserved_paths = reserved.clone();
    reserved_paths.insert(received_file_index_path()?);
    let final_path = unique_path(&dir, &safe_name, &reserved_paths);
    reserved.insert(final_path.clone());
    let temp_path = dir.join(format!(".{}.qcpart", Uuid::new_v4()));
    Ok(ReceivedFileReservation { final_path, temp_path })
}

pub fn commit_received_file(reservation: &ReceivedFileReservation) -> Result<PathBuf, String> {
    let mut reserved = RESERVED_RECEIVED_FILE_PATHS
        .lock()
        .map_err(|_| "接收文件路径状态异常".to_string())?;
    let dir = reservation
        .final_path
        .parent()
        .ok_or_else(|| "接收文件目录无效".to_string())?;
    let file_name = reservation
        .final_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "文件名无效".to_string())?;
    let final_path = if reservation.final_path.exists() {
        unique_path(dir, file_name, &reserved)
    } else {
        reservation.final_path.clone()
    };
    std::fs::rename(&reservation.temp_path, &final_path)
        .map_err(|e| format!("完成接收文件保存失败: {}", e))?;
    reserved.remove(&reservation.final_path);
    Ok(final_path)
}

pub fn discard_received_file(reservation: &ReceivedFileReservation) {
    if let Ok(mut reserved) = RESERVED_RECEIVED_FILE_PATHS.lock() {
        reserved.remove(&reservation.final_path);
    }
    let _ = std::fs::remove_file(&reservation.temp_path);
}

pub fn record_received_file(
    path: &Path,
    size: u64,
    sha256: &str,
    source_device_id: &str,
    source_device_name: &str,
) -> Result<(), String> {
    let _guard = RECEIVED_FILE_INDEX_LOCK
        .lock()
        .map_err(|_| "接收文件索引状态异常".to_string())?;
    let mut index = load_received_file_index()?;
    let path_key = received_file_path_key(path);
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("file")
        .to_string();
    index.files.insert(path_key.clone(), ReceivedFileMetadata {
        path: path_key,
        name,
        size,
        sha256: sha256.to_string(),
        source_device_id: source_device_id.trim().to_string(),
        source_device_name: source_device_name.trim().to_string(),
        received_at: chrono::Utc::now().timestamp_millis(),
    });
    save_received_file_index(&index)
}

pub fn list_received_file_metadata() -> Result<Vec<ReceivedFileMetadata>, String> {
    let _guard = RECEIVED_FILE_INDEX_LOCK
        .lock()
        .map_err(|_| "接收文件索引状态异常".to_string())?;
    let index = load_received_file_index()?;
    Ok(index.files.into_values().collect())
}

pub fn remove_received_file_metadata(path: &Path) -> Result<(), String> {
    let _guard = RECEIVED_FILE_INDEX_LOCK
        .lock()
        .map_err(|_| "接收文件索引状态异常".to_string())?;
    let mut index = load_received_file_index()?;
    let path_key = received_file_path_key(path);
    let before = index.files.len();
    index.files.remove(&path_key);
    index.files.retain(|_, item| item.path != path_key);
    if index.files.len() != before {
        save_received_file_index(&index)?;
    }
    Ok(())
}

pub fn file_name_from_transfer_path(path: &str) -> Result<String, String> {
    let raw = path
        .strip_prefix("/qc-transfer/files/")
        .ok_or_else(|| "无效的局域网传输路径".to_string())?;
    sanitize_file_name(raw)
}

pub fn image_id_from_file_path(path: &str) -> Result<String, String> {
    let raw = path
        .strip_prefix("/qc-sync/files/")
        .ok_or_else(|| "无效的局域网文件路径".to_string())?
        .strip_suffix(".png")
        .ok_or_else(|| "仅支持 png 图片文件".to_string())?;
    if !is_valid_image_id(raw) {
        return Err("无效的图片 ID".to_string());
    }
    Ok(raw.to_string())
}

pub fn received_files_dir() -> Result<PathBuf, String> {
    Ok(crate::services::get_data_directory()?.join("sync_transfer_files"))
}

pub fn is_received_file_internal(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(|name| name == RECEIVED_FILE_INDEX_NAME || name.starts_with('.') || name.ends_with(".qcpart"))
        .unwrap_or(false)
}

fn received_file_index_path() -> Result<PathBuf, String> {
    Ok(received_files_dir()?.join(RECEIVED_FILE_INDEX_NAME))
}

fn load_received_file_index() -> Result<ReceivedFileIndex, String> {
    let path = received_file_index_path()?;
    if !path.exists() {
        return Ok(ReceivedFileIndex::default());
    }
    let bytes = std::fs::read(&path)
        .map_err(|e| format!("读取接收文件索引失败: {}", e))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| format!("解析接收文件索引失败: {}", e))
}

fn save_received_file_index(index: &ReceivedFileIndex) -> Result<(), String> {
    let path = received_file_index_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建接收文件索引目录失败: {}", e))?;
    }
    let bytes = serde_json::to_vec_pretty(index)
        .map_err(|e| format!("序列化接收文件索引失败: {}", e))?;
    std::fs::write(&path, bytes)
        .map_err(|e| format!("保存接收文件索引失败: {}", e))
}

fn received_file_path_key(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string()
}

fn image_path(image_id: &str) -> Result<PathBuf, String> {
    if !is_valid_image_id(image_id) {
        return Err("无效的图片 ID".to_string());
    }
    Ok(crate::services::get_data_directory()?
        .join("clipboard_images")
        .join(format!("{}.png", image_id)))
}

fn sanitize_file_name(raw: &str) -> Result<String, String> {
    let decoded = percent_decode(raw)?;
    let name = Path::new(&decoded)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "文件名无效".to_string())?
        .trim()
        .to_string();
    if name.is_empty() || name == "." || name == ".." {
        return Err("文件名无效".to_string());
    }
    if name.contains('/') || name.contains('\\') || name.contains(':') {
        return Err("文件名包含非法字符".to_string());
    }
    Ok(name)
}

fn percent_decode(raw: &str) -> Result<String, String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err("文件名编码无效".to_string());
            }
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3])
                .map_err(|_| "文件名编码无效".to_string())?;
            let value = u8::from_str_radix(hex, 16).map_err(|_| "文件名编码无效".to_string())?;
            out.push(value);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(out).map_err(|_| "文件名编码无效".to_string())
}

fn unique_path(dir: &Path, file_name: &str, reserved: &HashSet<PathBuf>) -> PathBuf {
    let base = Path::new(file_name)
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("file");
    let ext = Path::new(file_name).extension().and_then(|ext| ext.to_str());
    let mut path = dir.join(file_name);
    let mut index = 1u32;
    while path.exists() || reserved.contains(&path) {
        let candidate = match ext {
            Some(ext) if !ext.is_empty() => format!("{} ({}).{}", base, index, ext),
            _ => format!("{} ({})", base, index),
        };
        path = dir.join(candidate);
        index = index.saturating_add(1);
    }
    path
}

fn is_valid_image_id(image_id: &str) -> bool {
    !image_id.is_empty()
        && image_id.len() <= 128
        && image_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_direct_transfer_file_size_is_512mb() {
        assert_eq!(MAX_DIRECT_TRANSFER_FILE_SIZE, 512 * 1024 * 1024);
    }

    #[test]
    fn sanitize_file_name_percent_decodes_and_strips_path_components() {
        assert_eq!(sanitize_file_name("a%20b.txt").unwrap(), "a b.txt");
        assert_eq!(sanitize_file_name("..%2Fsecret.txt").unwrap(), "secret.txt", "路径穿越被压平为文件名");
        assert_eq!(sanitize_file_name("/etc/passwd").unwrap(), "passwd");
        assert_eq!(sanitize_file_name("  spaced.txt  ").unwrap(), "spaced.txt", "首尾空白被裁剪");
    }

    #[test]
    fn sanitize_file_name_rejects_dangerous_names() {
        assert_eq!(sanitize_file_name("").unwrap_err(), "文件名无效");
        assert_eq!(sanitize_file_name("   ").unwrap_err(), "文件名无效");
        assert_eq!(sanitize_file_name(".").unwrap_err(), "文件名无效");
        assert_eq!(sanitize_file_name("..").unwrap_err(), "文件名无效");
        assert_eq!(sanitize_file_name("%2E%2E").unwrap_err(), "文件名无效", "解码后为 ..");
        assert_eq!(sanitize_file_name("a:b.txt").unwrap_err(), "文件名包含非法字符");
        assert_eq!(sanitize_file_name("%ZZ.txt").unwrap_err(), "文件名编码无效");
        assert_eq!(sanitize_file_name("a%2").unwrap_err(), "文件名编码无效");
    }

    #[test]
    fn percent_decode_handles_valid_and_invalid_escapes() {
        assert_eq!(percent_decode("abc%20def").unwrap(), "abc def");
        assert_eq!(percent_decode("100%25").unwrap(), "100%");
        assert_eq!(percent_decode("a%2Fb").unwrap(), "a/b");
        assert_eq!(percent_decode("%%").unwrap_err(), "文件名编码无效");
        assert_eq!(percent_decode("%G0").unwrap_err(), "文件名编码无效");
        assert_eq!(percent_decode("%FF").unwrap_err(), "文件名编码无效", "非 UTF-8 字节序列被拒绝");
    }

    #[test]
    fn unique_path_avoids_existing_files_with_numbered_suffix() {
        let dir = std::env::temp_dir().join(format!("qc_files_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        std::fs::write(dir.join("a (1).txt"), b"x").unwrap();
        let reserved = std::collections::HashSet::new();

        assert_eq!(unique_path(&dir, "b.txt", &reserved), dir.join("b.txt"));
        assert_eq!(unique_path(&dir, "a.txt", &reserved), dir.join("a (2).txt"));

        // 保留集合中的候选同样被跳过：c.txt 已存在且 c (1).txt 被保留 → c (2).txt
        std::fs::write(dir.join("c.txt"), b"x").unwrap();
        let mut reserved = std::collections::HashSet::new();
        reserved.insert(dir.join("c (1).txt"));
        assert_eq!(unique_path(&dir, "c.txt", &reserved), dir.join("c (2).txt"));

        // 无扩展名文件
        assert_eq!(unique_path(&dir, "plain", &reserved), dir.join("plain"));
        std::fs::write(dir.join("plain"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "plain", &reserved), dir.join("plain (1)"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn is_valid_image_id_accepts_alnum_dash_underscore_only() {
        assert!(is_valid_image_id("abc123"));
        assert!(is_valid_image_id("a-b_c"));
        assert!(is_valid_image_id(&"x".repeat(128)));
        assert!(!is_valid_image_id(""));
        assert!(!is_valid_image_id(&"x".repeat(129)));
        assert!(!is_valid_image_id("a b"));
        assert!(!is_valid_image_id("a/b"));
        assert!(!is_valid_image_id("中文"));
        assert!(!is_valid_image_id("a!b"));
    }

    #[test]
    fn collect_record_image_ids_parses_trims_filters_dedupes() {
        fn record(image_id: Option<&str>) -> CloudRecord {
            let mut r = CloudRecord {
                uuid: String::new(),
                source_device_id: String::new(),
                is_remote: false,
                content: String::new(),
                html_content: None,
                content_type: "text".to_string(),
                image_id: None,
                source_app: None,
                source_icon_hash: None,
                char_count: None,
                title: String::new(),
                group_name: "全部".to_string(),
                item_order: 0,
                paste_count: 0,
                created_at: 0,
                updated_at: 0,
            };
            r.image_id = image_id.map(|s| s.to_string());
            r
        }

        let records = vec![
            record(Some("img-1, img-2,  img-1")),
            record(Some("bad id,img-3")),
            record(None),
            record(Some("")),
        ];
        let mut ids = collect_record_image_ids(&records);
        ids.sort();
        assert_eq!(ids, vec!["img-1", "img-2", "img-3"], "逗号分隔、去重、非法 id 过滤");
    }

    #[test]
    fn file_name_from_transfer_path_strips_prefix_and_sanitizes() {
        assert_eq!(file_name_from_transfer_path("/qc-transfer/files/photo.png").unwrap(), "photo.png");
        assert_eq!(
            file_name_from_transfer_path("/qc-transfer/files/a%20b.txt").unwrap(),
            "a b.txt"
        );
        assert_eq!(file_name_from_transfer_path("/qc-transfer/files/").unwrap_err(), "文件名无效");
        assert_eq!(
            file_name_from_transfer_path("/other/path/x.txt").unwrap_err(),
            "无效的局域网传输路径"
        );
    }

    #[test]
    fn image_id_from_file_path_strips_prefix_and_suffix() {
        assert_eq!(image_id_from_file_path("/qc-sync/files/abc123.png").unwrap(), "abc123");
        assert_eq!(image_id_from_file_path("/qc-sync/files/a-b_c.png").unwrap(), "a-b_c");
        assert_eq!(
            image_id_from_file_path("/qc-sync/files/abc.jpg").unwrap_err(),
            "仅支持 png 图片文件"
        );
        assert_eq!(
            image_id_from_file_path("/qc-sync/files/bad id.png").unwrap_err(),
            "无效的图片 ID"
        );
        assert_eq!(
            image_id_from_file_path("/other/x.png").unwrap_err(),
            "无效的局域网文件路径"
        );
    }

    #[test]
    fn is_received_file_internal_detects_index_dotfiles_and_parts() {
        assert!(is_received_file_internal(Path::new("index.json")));
        assert!(is_received_file_internal(Path::new(".hidden")));
        assert!(is_received_file_internal(Path::new("photo.png.qcpart")));
        assert!(!is_received_file_internal(Path::new("normal.txt")));
        assert!(!is_received_file_internal(Path::new("index.json.bak")));
    }

    #[test]
    fn outgoing_file_info_returns_name_size_and_errors() {
        let dir = std::env::temp_dir().join(format!("qc_files_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("hello.txt");
        std::fs::write(&file, b"hello").unwrap();

        let (name, path, size) = outgoing_file_info(file.to_str().unwrap()).unwrap();
        assert_eq!(name, "hello.txt");
        assert_eq!(path, file);
        assert_eq!(size, 5);

        let err = outgoing_file_info(dir.to_str().unwrap()).unwrap_err();
        assert_eq!(err, "只能传输普通文件");

        let err = outgoing_file_info(dir.join("missing.bin").to_str().unwrap()).unwrap_err();
        assert!(err.contains("读取待传输文件信息失败"), "got: {}", err);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
