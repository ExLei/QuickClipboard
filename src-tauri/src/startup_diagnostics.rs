use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::panic;
use std::path::PathBuf;
use std::sync::Once;
use std::time::{SystemTime, UNIX_EPOCH};

static STARTUP_STAGE: Lazy<RwLock<String>> =
    Lazy::new(|| RwLock::new("准备启动应用".to_string()));
static STARTUP_STATE: Lazy<RwLock<String>> =
    Lazy::new(|| RwLock::new("starting".to_string()));
static PANIC_HOOK_ONCE: Once = Once::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartupStatus {
    pid: u32,
    state: String,
    stage: String,
    updated_at_ms: u64,
}

pub fn set_startup_stage(stage: &str) {
    *STARTUP_STAGE.write() = stage.to_string();
    persist_status();
}

pub fn set_startup_stage_if_starting(stage: &str) {
    if STARTUP_STATE.read().as_str() == "starting" {
        set_startup_stage(stage);
    }
}

pub fn current_startup_stage() -> String {
    STARTUP_STAGE.read().clone()
}

pub fn mark_starting() {
    *STARTUP_STATE.write() = "starting".to_string();
    persist_status();
}

pub fn mark_ready() {
    *STARTUP_STATE.write() = "ready".to_string();
    *STARTUP_STAGE.write() = "应用已就绪".to_string();
    persist_status();
}

pub fn detect_blocking_previous_instance() -> Option<String> {
    let status = read_status()?;
    let current_pid = std::process::id();

    if status.pid == current_pid || status.state != "starting" {
        return None;
    }

    if !is_process_alive(status.pid) {
        return None;
    }

    Some(format!(
        "检测到一个可能异常卡住的旧进程，阻止了新实例正常启动。\n\n旧进程 PID：{}\n旧进程停留阶段：{}\n\n请先在任务管理器中结束该 QuickClipboard 进程，然后重新启动应用。\n\n如果问题仍然出现，请将此窗口截图反馈给开发者。",
        status.pid,
        status.stage
    ))
}

pub fn install_panic_hook() {
    PANIC_HOOK_ONCE.call_once(|| {
        let default_hook = panic::take_hook();
        panic::set_hook(Box::new(move |panic_info| {
            let startup_state = current_startup_state();
            let location = panic_info
                .location()
                .map(|loc| format!("{}:{}", loc.file(), loc.line()))
                .unwrap_or_else(|| "未知位置".to_string());

            let message = if let Some(msg) = panic_info.payload().downcast_ref::<&str>() {
                (*msg).to_string()
            } else if let Some(msg) = panic_info.payload().downcast_ref::<String>() {
                msg.clone()
            } else {
                "未知 panic".to_string()
            };

            let detail = format!(
                "启动阶段：{}\n\n异常位置：{}\n\npanic 信息：{}\n\n请将此窗口截图反馈给开发者。",
                current_startup_stage(),
                location,
                message
            );

            let suppress_dialog = should_suppress_startup_panic_dialog(
                startup_state.as_str(),
                &location,
                &message,
            );
            append_panic_log(
                startup_state.as_str(),
                &current_startup_stage(),
                &location,
                &message,
                suppress_dialog,
            );

            if startup_state == "starting" && !suppress_dialog {
                *STARTUP_STATE.write() = "panic".to_string();
                persist_status();
                show_error_dialog("QuickClipboard 启动异常", &detail);
            }

            default_hook(panic_info);

            if startup_state == "starting" && !suppress_dialog {
                std::process::exit(1);
            }
        }));
    });
}

pub fn report_startup_error(summary: &str, error: impl std::fmt::Display) {
    *STARTUP_STATE.write() = "failed".to_string();
    persist_status();
    let detail = format!(
        "启动阶段：{}\n\n{}\n{}\n\n请将此窗口截图反馈给开发者。",
        current_startup_stage(),
        summary,
        error
    );
    show_error_dialog("QuickClipboard 启动失败", &detail);
}

#[cfg(windows)]
pub fn show_error_dialog(title: &str, message: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_SYSTEMMODAL,
    };
    use windows::core::PCWSTR;

    let title_wide: Vec<u16> = format!("{title}\0").encode_utf16().collect();
    let message_wide: Vec<u16> = format!("{message}\0").encode_utf16().collect();

    unsafe {
        MessageBoxW(
            None,
            PCWSTR(message_wide.as_ptr()),
            PCWSTR(title_wide.as_ptr()),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND | MB_SYSTEMMODAL,
        );
    }
}

#[cfg(not(windows))]
pub fn show_error_dialog(title: &str, message: &str) {
    eprintln!("{title}\n{message}");
}

fn persist_status() {
    let Some(path) = status_file_path() else {
        return;
    };

    let status = StartupStatus {
        pid: std::process::id(),
        state: STARTUP_STATE.read().clone(),
        stage: current_startup_stage(),
        updated_at_ms: current_time_ms(),
    };

    if let Ok(content) = serde_json::to_vec_pretty(&status) {
        let _ = fs::write(path, content);
    }
}

fn current_startup_state() -> String {
    STARTUP_STATE.read().clone()
}

fn should_suppress_startup_panic_dialog(
    startup_state: &str,
    location: &str,
    message: &str,
) -> bool {
    if startup_state != "starting" {
        return true;
    }

    is_known_tao_reentrant_panic(location, message)
}

fn is_known_tao_reentrant_panic(location: &str, message: &str) -> bool {
    location.contains("tao-")
        && location.contains("event_loop")
        && location.contains("runner.rs")
        && (message.contains("either event handler is re-entrant")
            || message.contains("RefCell already borrowed"))
}

fn append_panic_log(
    startup_state: &str,
    startup_stage: &str,
    location: &str,
    message: &str,
    suppress_dialog: bool,
) {
    let Some(path) = panic_log_file_path() else {
        return;
    };

    let now_ms = current_time_ms();
    let mode = if suppress_dialog { "仅记录" } else { "弹窗" };
    let entry = format!(
        "[{now_ms}] 状态: {startup_state}\n阶段: {startup_stage}\n位置: {location}\npanic: {message}\n处理: {mode}\n\n"
    );

    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);

    if let Ok(mut file) = options.open(path) {
        let _ = file.write_all(entry.as_bytes());
    }
}

fn read_status() -> Option<StartupStatus> {
    let path = status_file_path()?;
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn status_file_path() -> Option<PathBuf> {
    let base_dir = dirs::data_local_dir()?.join("quickclipboard");
    fs::create_dir_all(&base_dir).ok()?;
    Some(base_dir.join("startup-status.json"))
}

fn panic_log_file_path() -> Option<PathBuf> {
    let base_dir = dirs::data_local_dir()?.join("quickclipboard");
    fs::create_dir_all(&base_dir).ok()?;
    Some(base_dir.join("startup-panic.log"))
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(windows)]
fn is_process_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    unsafe {
        let handle = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(handle) => handle,
            Err(_) => return false,
        };

        let mut exit_code = 0u32;
        let result = GetExitCodeProcess(handle, &mut exit_code).is_ok();
        let _ = CloseHandle(handle);

        result && exit_code == STILL_ACTIVE.0 as u32
    }
}

#[cfg(not(windows))]
fn is_process_alive(_pid: u32) -> bool {
    false
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::panic::AssertUnwindSafe;

    /// 串行化所有修改进程级环境变量（XDG_DATA_HOME / WEBVIEW2_*）的测试。
    pub(crate) static ENV_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    /// 在隔离的临时 XDG_DATA_HOME 下执行闭包（闭包收到隔离目录）。
    /// 仅适用于遵守 XDG 的 unix 平台：macOS/Windows 的 data_local_dir 不读该变量。
    #[cfg(all(unix, not(target_os = "macos")))]
    pub(crate) fn with_isolated_data_home<T>(f: impl FnOnce(&std::path::Path) -> T) -> T {
        struct RestoreOnDrop(Option<std::ffi::OsString>);
        impl Drop for RestoreOnDrop {
            fn drop(&mut self) {
                match &self.0 {
                    Some(v) => std::env::set_var("XDG_DATA_HOME", v),
                    None => std::env::remove_var("XDG_DATA_HOME"),
                }
            }
        }

        let _guard = ENV_LOCK.lock();
        let dir = std::env::temp_dir().join(format!("qc_app_entry_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let _restore = RestoreOnDrop(std::env::var_os("XDG_DATA_HOME"));
        std::env::set_var("XDG_DATA_HOME", &dir);
        let result = f(&dir);
        drop(_restore);
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    fn read_status_file_json() -> serde_json::Value {
        let content =
            std::fs::read_to_string(status_file_path().expect("status 文件路径可用")).expect("status 文件已写入");
        serde_json::from_str(&content).expect("status 文件是合法 JSON")
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    fn write_status(status: StartupStatus) {
        let content = serde_json::to_vec_pretty(&status).unwrap();
        std::fs::write(status_file_path().expect("status 文件路径可用"), content).unwrap();
    }

    // ---------- 纯逻辑：panic 抑制判定（全平台） ----------

    #[test]
    fn known_tao_reentrant_panic_detection() {
        let location = "cargo/registry/src/tao-0.30.0/src/event_loop/runner.rs:123";
        assert!(is_known_tao_reentrant_panic(location, "either event handler is re-entrant"));
        assert!(is_known_tao_reentrant_panic(location, "RefCell already borrowed"));
        assert!(!is_known_tao_reentrant_panic(location, "其他错误"));
        // 位置缺少任一必需片段 → false
        assert!(!is_known_tao_reentrant_panic(
            "src/event_loop/runner.rs:1",
            "either event handler is re-entrant"
        )); // 缺 tao-
        assert!(!is_known_tao_reentrant_panic(
            "tao-0.30.0/src/runner.rs:1",
            "either event handler is re-entrant"
        )); // 缺 event_loop
        assert!(!is_known_tao_reentrant_panic(
            "tao-0.30.0/src/event_loop/mod.rs:1",
            "RefCell already borrowed"
        )); // 缺 runner.rs
        // 大小写敏感：大写不匹配
        assert!(!is_known_tao_reentrant_panic(
            location,
            "EITHER EVENT HANDLER IS RE-ENTRANT"
        ));
        assert!(!is_known_tao_reentrant_panic(
            "TAO-0.30.0/src/event_loop/runner.rs:1",
            "either event handler is re-entrant"
        ));
    }

    #[test]
    fn suppress_dialog_outside_starting_state() {
        for state in ["ready", "panic", "failed"] {
            assert!(should_suppress_startup_panic_dialog(state, "任何位置", "任何消息"));
        }
    }

    #[test]
    fn suppress_dialog_starting_state_depends_on_known_panic() {
        let tao_location = "tao-0.30.0/src/event_loop/runner.rs:12";
        assert!(should_suppress_startup_panic_dialog(
            "starting",
            tao_location,
            "either event handler is re-entrant"
        ));
        assert!(should_suppress_startup_panic_dialog(
            "starting",
            tao_location,
            "RefCell already borrowed"
        ));
        assert!(!should_suppress_startup_panic_dialog("starting", "src/main.rs:10", "普通 panic"));
        assert!(!should_suppress_startup_panic_dialog("starting", tao_location, "普通 panic"));
    }

    // ---------- 启动阶段/状态机 + 状态文件持久化（Linux 等 XDG unix） ----------

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn set_stage_updates_state_and_persists_exact_json() {
        with_isolated_data_home(|_| {
            mark_starting();
            set_startup_stage("执行启动安全检查");
            assert_eq!(current_startup_stage(), "执行启动安全检查");

            let v = read_status_file_json();
            assert_eq!(v["state"], serde_json::json!("starting"));
            assert_eq!(v["stage"], serde_json::json!("执行启动安全检查"));
            assert_eq!(v["pid"], serde_json::json!(std::process::id()));
            assert!(v["updated_at_ms"].as_u64().unwrap() > 0);
        });
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn mark_ready_transitions_state_and_stage() {
        with_isolated_data_home(|_| {
            mark_ready();
            assert_eq!(current_startup_stage(), "应用已就绪");
            let v = read_status_file_json();
            assert_eq!(v["state"], serde_json::json!("ready"));
            assert_eq!(v["stage"], serde_json::json!("应用已就绪"));
        });
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn mark_starting_resets_state_to_starting() {
        with_isolated_data_home(|_| {
            mark_ready();
            mark_starting();
            let v = read_status_file_json();
            assert_eq!(v["state"], serde_json::json!("starting"));
        });
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn stage_if_starting_only_applies_while_starting() {
        with_isolated_data_home(|_| {
            mark_starting();
            set_startup_stage_if_starting("阶段A");
            assert_eq!(current_startup_stage(), "阶段A");

            mark_ready();
            set_startup_stage_if_starting("阶段B");
            assert_eq!(current_startup_stage(), "应用已就绪");
            let v = read_status_file_json();
            assert_eq!(v["stage"], serde_json::json!("应用已就绪"));
        });
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn status_timestamp_monotonic_across_persists() {
        with_isolated_data_home(|_| {
            mark_starting();
            let t1 = read_status_file_json()["updated_at_ms"].as_u64().unwrap();
            set_startup_stage("另一阶段");
            let t2 = read_status_file_json()["updated_at_ms"].as_u64().unwrap();
            assert!(t2 >= t1);
        });
    }

    // ---------- 阻塞旧进程检测（Linux 上 is_process_alive 恒 false） ----------

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn detect_returns_none_without_status_file() {
        with_isolated_data_home(|_| {
            assert_eq!(detect_blocking_previous_instance(), None);
        });
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn detect_returns_none_for_corrupt_status_file() {
        with_isolated_data_home(|_| {
            std::fs::write(status_file_path().unwrap(), "{ 不是合法 JSON !!!").unwrap();
            assert_eq!(detect_blocking_previous_instance(), None);
        });
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn detect_returns_none_for_current_pid_even_while_starting() {
        with_isolated_data_home(|_| {
            mark_starting(); // 持久化当前 pid + starting
            assert_eq!(detect_blocking_previous_instance(), None);
        });
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn detect_returns_none_for_ready_state_foreign_pid() {
        with_isolated_data_home(|_| {
            write_status(StartupStatus {
                pid: std::process::id().wrapping_add(1),
                state: "ready".to_string(),
                stage: "应用已就绪".to_string(),
                updated_at_ms: 1,
            });
            assert_eq!(detect_blocking_previous_instance(), None);
        });
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn detect_returns_none_for_starting_foreign_pid_not_alive() {
        with_isolated_data_home(|_| {
            write_status(StartupStatus {
                pid: std::process::id().wrapping_add(1),
                state: "starting".to_string(),
                stage: "构建 Tauri 应用".to_string(),
                updated_at_ms: 1,
            });
            // Linux 上 is_process_alive 恒为 false → 不构成阻塞（Windows 上此处才可能返回 Some）
            assert_eq!(detect_blocking_previous_instance(), None);
        });
    }

    // ---------- panic hook（进程内可测路径：ready/failed 状态或 tao 重入抑制） ----------

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn panic_hook_appends_log_in_ready_state_without_exit() {
        with_isolated_data_home(|_| {
            install_panic_hook();
            mark_ready();
            set_startup_stage("运行应用事件循环");
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                panic!("app_entry_test_panic_message");
            }));
            assert!(result.is_err());
            let log = std::fs::read_to_string(panic_log_file_path().unwrap()).unwrap();
            assert!(log.contains("状态: ready"));
            assert!(log.contains("阶段: 运行应用事件循环"));
            assert!(log.contains("panic: app_entry_test_panic_message"));
            assert!(log.contains("startup_diagnostics.rs"));
            assert!(log.contains("处理: 仅记录"));
        });
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn panic_hook_handles_string_payload() {
        with_isolated_data_home(|_| {
            install_panic_hook();
            mark_ready();
            let message = format!("dynamic message {}", 42);
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                panic!("{}", message);
            }));
            assert!(result.is_err());
            let log = std::fs::read_to_string(panic_log_file_path().unwrap()).unwrap();
            assert!(log.contains("panic: dynamic message 42"));
        });
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn tao_reentrant_panic_while_starting_is_suppressed_no_state_change() {
        with_isolated_data_home(|_| {
            install_panic_hook();
            mark_starting();
            set_startup_stage("执行启动安全检查");
            // 触发位置来自 fixtures/tao-panic/event_loop/runner.rs：命中 tao 重入抑制规则。
            // 若抑制失效，starting 状态非抑制 panic 会 process::exit(1) 杀死测试进程。
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                mod tao_panic_fixture {
                    include!("../fixtures/tao-panic/event_loop/runner.rs");
                }
                tao_panic_fixture::trigger_tao_like_panic();
            }));
            assert!(result.is_err());
            let log = std::fs::read_to_string(panic_log_file_path().unwrap()).unwrap();
            assert!(log.contains("状态: starting"));
            assert!(log.contains("处理: 仅记录"));
            // 状态未被改写为 panic，仍停留在 starting
            let v = read_status_file_json();
            assert_eq!(v["state"], serde_json::json!("starting"));

            // 恢复进程级全局状态：本测试故意停留在 starting，
            // 若不恢复，后续任何测试的 panic 都会触发 hook 的 process::exit(1)，杀死整个测试进程。
            *STARTUP_STATE.write() = "ready".to_string();
        });
    }

    // ---------- 启动错误上报 ----------

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn report_startup_error_persists_failed_state() {
        with_isolated_data_home(|_| {
            mark_starting();
            set_startup_stage("执行 setup：初始化数据库");
            report_startup_error("数据库初始化失败", "file is not a database");
            let v = read_status_file_json();
            assert_eq!(v["state"], serde_json::json!("failed"));
            assert_eq!(v["stage"], serde_json::json!("执行 setup：初始化数据库"));
        });
    }
}
