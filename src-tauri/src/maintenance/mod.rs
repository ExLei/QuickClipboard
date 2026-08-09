mod ui;

use rusqlite::Connection;
use std::path::PathBuf;

use ui::{App, run_tui};

pub fn ensure_bat_file() {
    let exe_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return,
    };
    let bat_path = match exe_path.parent() {
        Some(dir) => dir.join("maintenance-mode.bat"),
        None => return,
    };
    if bat_path.exists() {
        return;
    }
    let bat_content = "@echo off\r\n\
setlocal EnableDelayedExpansion\r\n\
\r\n\
pushd \"%~dp0\"\r\n\
\r\n\
set \"EXE=\"\r\n\
\r\n\
if exist \"%~dp0QuickClipboard.exe\" (\r\n\
    set \"EXE=%~dp0QuickClipboard.exe\"\r\n\
)\r\n\
if not defined EXE (\r\n\
    if exist \"%~dp0quickclipboard.exe\" (\r\n\
        set \"EXE=%~dp0quickclipboard.exe\"\r\n\
    )\r\n\
)\r\n\
if not defined EXE (\r\n\
    if exist \"%~dp0target\\debug\\QuickClipboard.exe\" (\r\n\
        set \"EXE=%~dp0target\\debug\\QuickClipboard.exe\"\r\n\
    )\r\n\
)\r\n\
if not defined EXE (\r\n\
    if exist \"%~dp0target\\release\\QuickClipboard.exe\" (\r\n\
        set \"EXE=%~dp0target\\release\\QuickClipboard.exe\"\r\n\
    )\r\n\
)\r\n\
\r\n\
if not defined EXE (\r\n\
    echo [ERROR] QuickClipboard.exe not found\r\n\
    echo Please place this .bat next to QuickClipboard.exe\r\n\
    pause\r\n\
    exit /b 1\r\n\
)\r\n\
\r\n\
set QUICKCLIPBOARD_MAINTENANCE=1\r\n\
start \"\" \"%EXE%\" --maintenance\r\n\
\r\n\
popd\r\n\
exit /b 0\r\n\
";
    let _ = std::fs::write(&bat_path, bat_content);
}

#[cfg(windows)]
pub fn ensure_console() {
    use windows::Win32::System::Console::AllocConsole;
    unsafe {
        AllocConsole().ok();
    }
}

#[cfg(not(windows))]
pub fn ensure_console() {}

fn find_data_dir() -> Option<PathBuf> {
    let settings = crate::services::settings::storage::SettingsStorage::load()
        .unwrap_or_else(|e| {
            eprintln!("警告: 读取设置失败，将使用默认设置: {}", e);
            crate::services::AppSettings::default()
        });

    crate::services::settings::storage::SettingsStorage::get_data_directory(&settings)
        .map_err(|e| eprintln!("错误: 获取数据目录失败: {}", e))
        .ok()
}

pub fn run() {
    println!("QuickClipboard 维护模式");
    println!("正在启动...\n");

    let data_dir = match find_data_dir() {
        Some(d) => d,
        None => {
            eprintln!("错误: 无法确定数据目录");
            eprintln!("请检查程序安装是否正确。");
            wait_for_key();
            return;
        }
    };

    let db_path = data_dir.join("quickclipboard.db");

    if !db_path.exists() {
        eprintln!("错误: 数据库文件不存在");
        eprintln!("路径: {}", db_path.display());
        eprintln!("请确保 QuickClipboard 至少运行过一次以生成数据库。");
        wait_for_key();
        return;
    }

    let db = match Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("错误: 无法打开数据库: {}", e);
            wait_for_key();
            return;
        }
    };

    let _ = db.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;",
    );

    let table_exists: bool = db
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='clipboard'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false);

    if !table_exists {
        eprintln!("错误: 数据库中没有 clipboard 表");
        eprintln!("数据库可能已损坏，或尚未初始化。");
        wait_for_key();
        return;
    }

    let app = App {
        db,
        items: Vec::new(),
        total_count: 0,
        current_page: 0,
        page_size: 30,
        search_query: String::new(),
        is_searching: false,
        fav_items: Vec::new(),
        fav_total: 0,
        fav_page: 0,
        fav_search: String::new(),
        fav_is_searching: false,
        groups: Vec::new(),
        current_tab: ui::Tab::Clipboard,
        table_state: ratatui::widgets::TableState::new().with_selected(Some(0)),
        scroll_state: ratatui::widgets::ScrollbarState::new(1),
        screen: ui::Screen::List,
        status_message: String::new(),
        should_quit: false,
    };

    if let Err(e) = run_tui(app) {
        eprintln!("\n维护模式运行出错: {}", e);
        wait_for_key();
    }

    println!("已退出维护模式。");
}

#[cfg(windows)]
fn wait_for_key() {
    use std::io::Read;
    eprintln!("\n按 Enter 键退出...");
    let _ = std::io::stdin().read(&mut [0u8; 1]);
}

#[cfg(not(windows))]
fn wait_for_key() {
    use std::io::Read;
    eprintln!("\n按 Enter 键退出...");
    let _ = std::io::stdin().read(&mut [0u8; 1]);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 与 ensure_bat_file 中写出的内容逐字节一致（同一字面量结构）。
    const EXPECTED_BAT_CONTENT: &str = "@echo off\r\n\
setlocal EnableDelayedExpansion\r\n\
\r\n\
pushd \"%~dp0\"\r\n\
\r\n\
set \"EXE=\"\r\n\
\r\n\
if exist \"%~dp0QuickClipboard.exe\" (\r\n\
    set \"EXE=%~dp0QuickClipboard.exe\"\r\n\
)\r\n\
if not defined EXE (\r\n\
    if exist \"%~dp0quickclipboard.exe\" (\r\n\
        set \"EXE=%~dp0quickclipboard.exe\"\r\n\
    )\r\n\
)\r\n\
if not defined EXE (\r\n\
    if exist \"%~dp0target\\debug\\QuickClipboard.exe\" (\r\n\
        set \"EXE=%~dp0target\\debug\\QuickClipboard.exe\"\r\n\
    )\r\n\
)\r\n\
if not defined EXE (\r\n\
    if exist \"%~dp0target\\release\\QuickClipboard.exe\" (\r\n\
        set \"EXE=%~dp0target\\release\\QuickClipboard.exe\"\r\n\
    )\r\n\
)\r\n\
\r\n\
if not defined EXE (\r\n\
    echo [ERROR] QuickClipboard.exe not found\r\n\
    echo Please place this .bat next to QuickClipboard.exe\r\n\
    pause\r\n\
    exit /b 1\r\n\
)\r\n\
\r\n\
set QUICKCLIPBOARD_MAINTENANCE=1\r\n\
start \"\" \"%EXE%\" --maintenance\r\n\
\r\n\
popd\r\n\
exit /b 0\r\n\
";

    #[test]
    fn ensure_bat_file_creates_when_missing_and_never_overwrites() {
        let bat_path = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .join("maintenance-mode.bat");

        // 分支 1：文件已存在 → 不得覆盖
        let marker = "MARKER-SENTINEL\n";
        std::fs::write(&bat_path, marker).unwrap();
        ensure_bat_file();
        assert_eq!(std::fs::read_to_string(&bat_path).unwrap(), marker);

        // 分支 2：文件缺失 → 创建，内容与产品定义逐字节一致
        std::fs::remove_file(&bat_path).unwrap();
        ensure_bat_file();
        let content = std::fs::read_to_string(&bat_path).unwrap();
        assert_eq!(content, EXPECTED_BAT_CONTENT);

        // 清理，避免污染构建目录
        let _ = std::fs::remove_file(&bat_path);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn find_data_dir_defaults_to_data_local_dir() {
        // 套件中其它模块的测试支撑会在测试二进制旁放置 portable.flag（隔离数据目录），
        // 使 settings 层解析为 <exe>/data；此时断言 portable 语义，否则断言 XDG 语义。
        // （with_isolated_data_home 内部已持有 ENV_LOCK）
        let portable_active = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|p| p.join("portable.flag").exists()))
            .unwrap_or(false);
        crate::startup_diagnostics::tests::with_isolated_data_home(|data_home| {
            let dir = find_data_dir().expect("应能确定数据目录");
            if portable_active {
                let exe_dir = std::env::current_exe()
                    .expect("current_exe")
                    .parent()
                    .expect("exe 父目录")
                    .to_path_buf();
                assert_eq!(dir, exe_dir.join("data"), "portable 模式下数据目录 = <exe>/data");
            } else {
                assert_eq!(dir, data_home.join("quickclipboard"));
            }
            assert!(dir.exists());
        });
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn find_data_dir_honors_custom_storage_setting() {
        use crate::services::database::connection::test_support::{SettingsGuard, TEST_ENV_LOCK};
        // with_isolated_data_home 内部持有 ENV_LOCK；此处只取 TEST_ENV_LOCK，避免死锁。
        let _lock = TEST_ENV_LOCK.lock();
        crate::startup_diagnostics::tests::with_isolated_data_home(|data_home| {
            let custom = data_home.join("custom-storage");
            // settings 可能已被套件中其它测试提前加载（Lazy 全局），
            // 因此通过 API 写入而非注入 settings.json。
            let original = crate::services::get_settings();
            let mut redirected = original.clone();
            redirected.use_custom_storage = true;
            redirected.custom_storage_path = Some(custom.to_string_lossy().to_string());
            crate::services::update_settings(redirected).expect("重定向数据目录");
            let _guard = SettingsGuard(original);

            let dir = find_data_dir().expect("应能确定数据目录");
            assert_eq!(dir, custom);
            assert!(dir.exists());
        });
    }
}
