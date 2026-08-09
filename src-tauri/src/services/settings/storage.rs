use super::model::{
    AppSettings, SETTINGS_MIGRATION_VERSION_V1, SETTINGS_MIGRATION_VERSION_V2,
    SETTINGS_MIGRATION_VERSION_V3,
};
use std::{env, fs, path::PathBuf};

pub struct SettingsStorage;

impl SettingsStorage {
    fn migrate_settings(settings: &mut AppSettings) -> bool {
        let mut migrated = false;
        let migration_version = settings.settings_migration_version.unwrap_or(0);

        if migration_version < SETTINGS_MIGRATION_VERSION_V1 {
            settings.image_preview = true;
            settings.text_preview = true;
            settings.file_preview = true;
            settings.settings_migration_version = Some(SETTINGS_MIGRATION_VERSION_V1);
            migrated = true;
        }

        if migration_version < SETTINGS_MIGRATION_VERSION_V2 {
            settings.settings_migration_version = Some(SETTINGS_MIGRATION_VERSION_V2);
            migrated = true;
        }

        if migration_version < SETTINGS_MIGRATION_VERSION_V3 {
            let _ = settings.normalize_app_filter_blocklist();
            settings.settings_migration_version = Some(SETTINGS_MIGRATION_VERSION_V3);
            migrated = true;
        }

        migrated
    }

    fn is_portable_mode() -> bool {
        if crate::services::is_portable_build() {
            return true;
        }
        env::current_exe()
            .ok()
            .and_then(|exe| {
                exe.parent()
                    .map(|p| p.join("portable.flag").exists() || p.join("portable.txt").exists())
            })
            .unwrap_or(false)
    }

    fn get_data_dir() -> Result<PathBuf, String> {
        if Self::is_portable_mode() {
            let exe_dir = env::current_exe()
                .map_err(|e| e.to_string())?
                .parent()
                .ok_or("无法获取执行目录")?
                .to_path_buf();
            return Ok(exe_dir.join("data"));
        }

        Ok(dirs::data_local_dir()
            .ok_or("无法获取数据目录")?
            .join("quickclipboard"))
    }

    pub fn get_settings_path() -> Result<PathBuf, String> {
        let dir = Self::get_data_dir()?;
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(dir.join("settings.json"))
    }

    pub fn load() -> Result<AppSettings, String> {
        let path = Self::get_settings_path()?;

        if !path.exists() {
            return Ok(AppSettings::default());
        }

        let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let has_legacy_lan_sync_settings = content.contains("\"lanSync");
        let mut settings: AppSettings =
            serde_json::from_str(&content).map_err(|e| e.to_string())?;
        let had_legacy_webdav_password = !settings.webdav_password.is_empty();
        if had_legacy_webdav_password {
            if !settings.webdav_url.trim().is_empty() && !settings.webdav_username.trim().is_empty()
            {
                if let Err(e) = crate::services::secure_credentials::set_webdav_password(
                    &settings.webdav_url,
                    &settings.webdav_username,
                    &settings.webdav_password,
                ) {
                    eprintln!("迁移 WebDAV 密码到系统凭据库失败: {}", e);
                }
            }
            settings.webdav_password.clear();
        }
        let normalized = settings.normalize_app_filter_blocklist();
        let migrated = Self::migrate_settings(&mut settings)
            || normalized
            || has_legacy_lan_sync_settings
            || had_legacy_webdav_password;

        if migrated {
            let _ = Self::save(&settings);
        }

        Ok(settings)
    }

    pub fn exists() -> Result<bool, String> {
        let path = Self::get_settings_path()?;
        Ok(path.exists())
    }

    pub fn save(settings: &AppSettings) -> Result<(), String> {
        let path = Self::get_settings_path()?;
        let content = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
        fs::write(&path, content).map_err(|e| e.to_string())
    }

    pub fn get_data_directory(settings: &AppSettings) -> Result<PathBuf, String> {
        if settings.use_custom_storage {
            if let Some(ref path) = settings.custom_storage_path {
                let custom_dir = PathBuf::from(path);
                fs::create_dir_all(&custom_dir).map_err(|e| e.to_string())?;
                return Ok(custom_dir);
            }
        }

        let dir = Self::get_data_dir()?;
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::database::connection::test_support::TEST_ENV_LOCK;
    use parking_lot::Mutex;

    // 所有读写真实 settings.json 的测试共享同一把锁，保证即使测试并行执行也不会互相踩踏。
    static SETTINGS_FILE_LOCK: Mutex<()> = Mutex::new(());

    fn lock() -> parking_lot::MutexGuard<'static, ()> {
        SETTINGS_FILE_LOCK.lock()
    }

    // RAII 备份/还原真实的 settings.json（测试前备份，测试结束（含 panic 展开）后还原）。
    struct SettingsFileGuard {
        backup: Option<Vec<u8>>,
    }

    impl SettingsFileGuard {
        fn new() -> Self {
            Self {
                backup: fs::read(SettingsStorage::get_settings_path().expect("settings path")).ok(),
            }
        }
    }

    impl Drop for SettingsFileGuard {
        fn drop(&mut self) {
            let path = SettingsStorage::get_settings_path().expect("settings path");
            match &self.backup {
                Some(bytes) => {
                    let _ = fs::write(&path, bytes);
                }
                None => {
                    let _ = fs::remove_file(&path);
                }
            }
        }
    }

    // ---------- migrate_settings ----------

    #[test]
    fn migrates_from_absent_version_through_v3() {
        let mut s = AppSettings::default();
        s.settings_migration_version = None;
        s.image_preview = false;
        s.text_preview = false;
        s.file_preview = false;
        s.app_filter_mode = "whitelist".to_string();
        s.app_filter_list = vec!["x.exe".to_string()];

        assert!(SettingsStorage::migrate_settings(&mut s));
        assert_eq!(
            s.settings_migration_version,
            Some(SETTINGS_MIGRATION_VERSION_V3)
        );
        assert!(s.image_preview && s.text_preview && s.file_preview);
        assert_eq!(s.app_filter_mode, "blacklist");
        assert!(s.app_filter_list.is_empty());
    }

    #[test]
    fn does_not_reapply_v1_when_starting_from_v1() {
        let mut s = AppSettings::default();
        s.settings_migration_version = Some(SETTINGS_MIGRATION_VERSION_V1);
        s.image_preview = false;

        assert!(SettingsStorage::migrate_settings(&mut s));
        assert_eq!(
            s.settings_migration_version,
            Some(SETTINGS_MIGRATION_VERSION_V3)
        );
        assert!(!s.image_preview, "V1 步进不应被重新执行");
    }

    #[test]
    fn migrates_v2_legacy_filter_to_v3() {
        let mut s = AppSettings::default();
        s.settings_migration_version = Some(SETTINGS_MIGRATION_VERSION_V2);
        s.app_filter_blocklist = vec![];
        s.app_filter_list = vec!["chrome.exe".to_string()];

        assert!(SettingsStorage::migrate_settings(&mut s));
        assert_eq!(
            s.settings_migration_version,
            Some(SETTINGS_MIGRATION_VERSION_V3)
        );
        assert_eq!(s.app_filter_blocklist, vec!["chrome.exe".to_string()]);
        assert!(s.app_filter_list.is_empty());
    }

    #[test]
    fn noop_when_already_at_v3() {
        let mut s = AppSettings::default();
        s.settings_migration_version = Some(SETTINGS_MIGRATION_VERSION_V3);

        assert!(!SettingsStorage::migrate_settings(&mut s));
        assert_eq!(
            s.settings_migration_version,
            Some(SETTINGS_MIGRATION_VERSION_V3)
        );
    }

    #[test]
    fn v3_migration_skips_normalize_when_version_already_v3() {
        let mut s = AppSettings::default();
        s.settings_migration_version = Some(SETTINGS_MIGRATION_VERSION_V3);
        s.app_filter_mode = "whitelist".to_string();

        assert!(!SettingsStorage::migrate_settings(&mut s));
        assert_eq!(s.app_filter_mode, "whitelist");
    }

    // ---------- load / save / exists / paths ----------

    #[test]
    fn load_returns_defaults_when_file_missing() {
        let _lock = lock();
        let _file = SettingsFileGuard::new();
        let path = SettingsStorage::get_settings_path().unwrap();
        let _ = fs::remove_file(&path);

        let settings = SettingsStorage::load().unwrap();
        assert_eq!(settings.history_limit, 100);
        assert_eq!(
            settings.settings_migration_version,
            Some(SETTINGS_MIGRATION_VERSION_V3)
        );
        assert!(!path.exists(), "缺失文件时 load 不应创建 settings.json");
    }

    #[test]
    fn load_errors_on_invalid_json() {
        let _lock = lock();
        let _file = SettingsFileGuard::new();
        let path = SettingsStorage::get_settings_path().unwrap();
        fs::write(&path, "{not json").unwrap();

        let err = SettingsStorage::load().expect_err("无效 JSON 必须返回 Err");
        assert!(!err.is_empty());
    }

    #[test]
    fn load_parses_camel_case_json_and_upgrades_old_files() {
        let _lock = lock();
        let _file = SettingsFileGuard::new();
        let path = SettingsStorage::get_settings_path().unwrap();
        fs::write(
            &path,
            r#"{"historyLimit": 200, "theme": "dark", "soundVolume": 70.0}"#,
        )
        .unwrap();

        let settings = SettingsStorage::load().unwrap();
        assert_eq!(settings.history_limit, 200);
        assert_eq!(settings.theme, "dark");
        assert_eq!(settings.sound_volume, 70.0);
        assert_eq!(
            settings.settings_migration_version,
            Some(SETTINGS_MIGRATION_VERSION_V3)
        );
        assert!(
            settings.image_preview,
            "无版本号的旧文件应被升级到 V3 并强制预览开启"
        );
    }

    #[test]
    fn load_rewrites_file_when_legacy_lan_sync_settings_present() {
        let _lock = lock();
        let _file = SettingsFileGuard::new();
        let path = SettingsStorage::get_settings_path().unwrap();
        // fixture 已带 settingsMigrationVersion=3 + appFilterMode=blacklist：
        // 迁移与规范化都不会触发，lanSync 字段成为唯一可能触发重写的因素。
        fs::write(
            &path,
            r#"{"settingsMigrationVersion": 3, "appFilterMode": "blacklist", "lanSyncTransferEnabled": true, "historyLimit": 123}"#,
        )
        .unwrap();

        let settings = SettingsStorage::load().unwrap();
        assert_eq!(settings.history_limit, 123);
        assert_eq!(
            settings.settings_migration_version,
            Some(SETTINGS_MIGRATION_VERSION_V3),
            "V3 版本不应被迁移改写"
        );
        assert_eq!(settings.app_filter_mode, "blacklist");
        let content = fs::read_to_string(&path).unwrap();
        assert!(
            !content.contains("lanSync"),
            "旧 lanSync 字段应从文件中移除"
        );
        assert!(content.contains("\"historyLimit\": 123"));
        assert!(
            content.contains("\"settingsMigrationVersion\": 3"),
            "非 lanSync 的字段必须原样保留"
        );
    }

    #[test]
    fn load_clears_legacy_webdav_password_and_rewrites_file() {
        let _lock = lock();
        let _file = SettingsFileGuard::new();
        let path = SettingsStorage::get_settings_path().unwrap();
        fs::write(
            &path,
            r#"{"webdavUrl": "https://dav.example.com", "webdavUsername": "alice", "webdavPassword": "s3cret"}"#,
        )
        .unwrap();

        let settings = SettingsStorage::load().unwrap();
        assert_eq!(settings.webdav_password, "");
        assert_eq!(settings.webdav_url, "https://dav.example.com");
        assert_eq!(settings.webdav_username, "alice");
        let content = fs::read_to_string(&path).unwrap();
        assert!(
            !content.contains("webdavPassword"),
            "迁移后明文密码不得留在文件中"
        );
        assert!(content.contains("\"webdavUrl\""));
    }

    #[test]
    fn load_normalizes_legacy_whitelist_mode_from_file() {
        let _lock = lock();
        let _file = SettingsFileGuard::new();
        let path = SettingsStorage::get_settings_path().unwrap();
        fs::write(
            &path,
            r#"{"appFilterMode": "whitelist", "appFilterList": ["a.exe"]}"#,
        )
        .unwrap();

        let settings = SettingsStorage::load().unwrap();
        assert_eq!(settings.app_filter_mode, "blacklist");
        assert!(settings.app_filter_list.is_empty());
        assert!(
            settings.app_filter_blocklist.is_empty(),
            "whitelist 不应被反转为 blocklist"
        );
        let content = fs::read_to_string(&path).unwrap();
        assert!(
            !content.contains("appFilterMode"),
            "规范化后旧字段不应留在文件中"
        );
    }

    #[test]
    fn load_does_not_rewrite_file_when_already_current() {
        let _lock = lock();
        let _file = SettingsFileGuard::new();
        let path = SettingsStorage::get_settings_path().unwrap();
        // 显式携带黑名单模式 + V3：解析后无需迁移/规范化 → 不得重写文件。
        let content =
            r#"{"historyLimit": 555, "settingsMigrationVersion": 3, "appFilterMode": "blacklist"}"#;
        fs::write(&path, content).unwrap();

        let settings = SettingsStorage::load().unwrap();
        assert_eq!(settings.history_limit, 555);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            content,
            "已是最新版本且无需规范化时不应重写文件"
        );
    }

    #[test]
    fn load_normalizes_missing_app_filter_mode_and_rewrites() {
        let _lock = lock();
        let _file = SettingsFileGuard::new();
        let path = SettingsStorage::get_settings_path().unwrap();
        // 空对象字段级缺省：appFilterMode 缺省为空串 → load 强制为 blacklist 并重写文件。
        let content = r#"{"historyLimit": 555, "settingsMigrationVersion": 3}"#;
        fs::write(&path, content).unwrap();

        let settings = SettingsStorage::load().unwrap();
        assert_eq!(settings.history_limit, 555);
        assert_eq!(settings.app_filter_mode, "blacklist");
        let rewritten = fs::read_to_string(&path).unwrap();
        assert_ne!(
            rewritten, content,
            "缺省 appFilterMode 触发规范化 → 应重写文件"
        );
        assert!(rewritten.contains("\"historyLimit\": 555"));
    }

    #[test]
    fn save_then_load_roundtrips_exact_settings() {
        let _lock = lock();
        let _file = SettingsFileGuard::new();
        let mut s = AppSettings::default();
        s.history_limit = 321;
        s.theme = "dark".to_string();
        s.visible_optional_tabs = vec!["favorites".to_string()];

        SettingsStorage::save(&s).unwrap();
        let loaded = SettingsStorage::load().unwrap();
        assert_eq!(
            serde_json::to_value(&loaded).unwrap(),
            serde_json::to_value(&s).unwrap()
        );
        let content = fs::read_to_string(SettingsStorage::get_settings_path().unwrap()).unwrap();
        assert!(content.contains("\"historyLimit\": 321"));
    }

    #[test]
    fn exists_reflects_settings_file_presence() {
        let _lock = lock();
        let _file = SettingsFileGuard::new();
        let path = SettingsStorage::get_settings_path().unwrap();
        let _ = fs::remove_file(&path);

        assert!(!SettingsStorage::exists().unwrap());
        SettingsStorage::save(&AppSettings::default()).unwrap();
        assert!(SettingsStorage::exists().unwrap());
    }

    #[test]
    fn settings_path_points_to_settings_json_under_data_dir() {
        let path = SettingsStorage::get_settings_path().unwrap();
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("settings.json")
        );
        assert!(
            path.parent().unwrap().is_dir(),
            "get_settings_path 应创建数据目录"
        );
    }

    #[test]
    fn data_directory_default_and_custom() {
        let custom_dir = env::temp_dir().join(format!("qc-settings-test-{}", std::process::id()));
        let mut custom = AppSettings::default();
        custom.use_custom_storage = true;
        custom.custom_storage_path = Some(custom_dir.to_string_lossy().to_string());

        let dir = SettingsStorage::get_data_directory(&custom).unwrap();
        assert_eq!(dir, custom_dir);
        assert!(dir.is_dir(), "自定义存储目录应被创建");
        let _ = fs::remove_dir_all(&custom_dir);

        let default_settings = AppSettings::default();
        let dir = SettingsStorage::get_data_directory(&default_settings).unwrap();
        assert!(dir.is_dir());

        let mut none_path = AppSettings::default();
        none_path.use_custom_storage = true;
        let fallback = SettingsStorage::get_data_directory(&none_path).unwrap();
        assert_eq!(fallback, dir, "use_custom_storage 无路径时应回退默认目录");
    }

    // ---------- state (全局设置状态) ----------
    // state.rs 的更新会写真实 settings.json，因此与 storage 测试共用文件锁。

    #[test]
    fn update_with_mutates_global_and_persists() {
        let _lock = lock();
        let _env = TEST_ENV_LOCK.lock();
        let _file = SettingsFileGuard::new();
        let original = super::super::state::get_settings();

        let result = super::super::state::update_with(|s| s.history_limit = 4242);
        assert!(result.is_ok());
        assert_eq!(super::super::state::get_settings().history_limit, 4242);
        let content = fs::read_to_string(SettingsStorage::get_settings_path().unwrap()).unwrap();
        assert!(content.contains("\"historyLimit\": 4242"));

        super::super::state::update_settings(original).unwrap();
    }

    #[test]
    fn update_settings_replaces_global_and_persists() {
        let _lock = lock();
        let _env = TEST_ENV_LOCK.lock();
        let _file = SettingsFileGuard::new();
        let original = super::super::state::get_settings();

        let mut fresh = AppSettings::default();
        fresh.theme = "dark".to_string();
        fresh.history_limit = 777;
        super::super::state::update_settings(fresh.clone()).unwrap();

        let current = super::super::state::get_settings();
        assert_eq!(current.theme, "dark");
        assert_eq!(current.history_limit, 777);
        let content = fs::read_to_string(SettingsStorage::get_settings_path().unwrap()).unwrap();
        assert!(content.contains("\"historyLimit\": 777"));

        super::super::state::update_settings(original).unwrap();
    }

    #[test]
    fn state_data_directory_honors_custom_storage_path() {
        let _lock = lock();
        let _env = TEST_ENV_LOCK.lock();
        let _file = SettingsFileGuard::new();
        let original = super::super::state::get_settings();
        let custom_dir = env::temp_dir().join(format!("qc-state-custom-{}", std::process::id()));

        super::super::state::update_with(|s| {
            s.use_custom_storage = true;
            s.custom_storage_path = Some(custom_dir.to_string_lossy().to_string());
        })
        .unwrap();
        let dir = super::super::state::get_data_directory().unwrap();
        assert_eq!(dir, custom_dir);
        let _ = fs::remove_dir_all(&custom_dir);

        super::super::state::update_settings(original).unwrap();
    }
}
