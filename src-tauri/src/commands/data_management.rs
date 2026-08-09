use serde::Deserialize;

#[derive(Deserialize)]
pub struct ChangePathPayload {
    #[serde(alias = "new_path", alias = "newPath")]
    new_path: String,
    #[serde(default = "default_source_only")]
    mode: String,
}

#[derive(Deserialize)]
pub struct ResetPathPayload {
    #[serde(default = "default_source_only")]
    mode: String,
}

#[derive(Deserialize)]
pub struct CheckTargetPayload {
    #[serde(alias = "target_path", alias = "targetPath")]
    target_path: String,
}

fn default_source_only() -> String {
    "source_only".to_string()
}

#[derive(Deserialize)]
pub struct ExportPayload {
    #[serde(alias = "target_path", alias = "targetPath")]
    target_path: String,
}

#[derive(Deserialize)]
pub struct ImportPayload {
    #[serde(alias = "zip_path", alias = "zipPath")]
    zip_path: String,
    mode: String,
}

#[tauri::command]
pub fn dm_get_current_storage_path() -> Result<String, String> {
    let path = crate::services::data_management::get_current_storage_dir()?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn dm_get_default_storage_path() -> Result<String, String> {
    let path = crate::services::data_management::get_default_data_dir()?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn dm_check_target_has_data(payload: CheckTargetPayload) -> Result<crate::services::data_management::TargetDataInfo, String> {
    let path = std::path::PathBuf::from(payload.target_path);
    crate::services::data_management::check_target_has_data(&path)
}

#[tauri::command]
pub fn dm_change_storage_path(app: tauri::AppHandle, payload: ChangePathPayload) -> Result<String, String> {
    let path = std::path::PathBuf::from(payload.new_path);
    let new_dir = crate::services::data_management::change_storage_dir(path, &payload.mode)?;

    Ok(new_dir.to_string_lossy().to_string())
}

#[tauri::command]
pub fn dm_reset_storage_path_to_default(app: tauri::AppHandle, payload: ResetPathPayload) -> Result<String, String> {
    let dir = crate::services::data_management::reset_storage_dir_to_default(&payload.mode)?;

    Ok(dir.to_string_lossy().to_string())
}

#[tauri::command]
pub fn dm_export_data_zip(payload: ExportPayload) -> Result<String, String> {
    let path = std::path::PathBuf::from(payload.target_path);
    let out = crate::services::data_management::export_data_zip(path)?;
    Ok(out.to_string_lossy().to_string())
}

#[tauri::command]
pub fn dm_import_data_zip(payload: ImportPayload) -> Result<String, String> {
    let zip = std::path::PathBuf::from(payload.zip_path);
    let result = crate::services::data_management::import_data_zip(zip, &payload.mode)?;
    Ok(result)
}

#[tauri::command]
pub fn dm_reset_all_data(app: tauri::AppHandle) -> Result<String, String> {
    let path = crate::services::data_management::reset_all_data()?;
    Ok(path)
}

#[tauri::command]
pub fn dm_list_backups() -> Result<Vec<crate::services::data_management::BackupInfo>, String> {
    crate::services::data_management::list_backups()
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_path_payload_accepts_both_key_spellings_and_defaults_mode() {
        let p1: ChangePathPayload = serde_json::from_str(r#"{"new_path": "/tmp/a"}"#).unwrap();
        assert_eq!(p1.new_path, "/tmp/a");
        assert_eq!(p1.mode, "source_only");

        let p2: ChangePathPayload =
            serde_json::from_str(r#"{"newPath": "/tmp/b", "mode": "move"}"#).unwrap();
        assert_eq!(p2.new_path, "/tmp/b");
        assert_eq!(p2.mode, "move");
    }

    #[test]
    fn check_target_payload_accepts_both_key_spellings() {
        let p1: CheckTargetPayload = serde_json::from_str(r#"{"target_path": "/tmp/x"}"#).unwrap();
        assert_eq!(p1.target_path, "/tmp/x");
        let p2: CheckTargetPayload = serde_json::from_str(r#"{"targetPath": "/tmp/y"}"#).unwrap();
        assert_eq!(p2.target_path, "/tmp/y");
    }

    #[test]
    fn export_payload_accepts_both_key_spellings() {
        let p1: ExportPayload = serde_json::from_str(r#"{"target_path": "/tmp/out.zip"}"#).unwrap();
        assert_eq!(p1.target_path, "/tmp/out.zip");
        let p2: ExportPayload = serde_json::from_str(r#"{"targetPath": "/tmp/out2.zip"}"#).unwrap();
        assert_eq!(p2.target_path, "/tmp/out2.zip");
    }

    #[test]
    fn import_payload_accepts_both_key_spellings() {
        let p1: ImportPayload =
            serde_json::from_str(r#"{"zip_path": "/tmp/in.zip", "mode": "replace"}"#).unwrap();
        assert_eq!(p1.zip_path, "/tmp/in.zip");
        assert_eq!(p1.mode, "replace");
        let p2: ImportPayload =
            serde_json::from_str(r#"{"zipPath": "/tmp/in2.zip", "mode": "merge"}"#).unwrap();
        assert_eq!(p2.zip_path, "/tmp/in2.zip");
        assert_eq!(p2.mode, "merge");
    }

    #[test]
    fn import_payload_requires_mode_field() {
        // 与 ChangePathPayload/ResetPathPayload 不同，ImportPayload.mode 无默认值，必须显式提供
        assert!(serde_json::from_str::<ImportPayload>(r#"{"zip_path": "/tmp/in.zip"}"#).is_err());
    }

    #[test]
    fn reset_path_payload_defaults_mode_to_source_only() {
        let p: ResetPathPayload = serde_json::from_str("{}").unwrap();
        assert_eq!(p.mode, "source_only");
    }
}
