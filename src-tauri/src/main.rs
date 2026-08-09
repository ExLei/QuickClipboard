#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// 判断是否应进入维护模式：环境变量 `QUICKCLIPBOARD_MAINTENANCE` 精确等于 "1"，
/// 或命令行参数中出现精确的 `--maintenance`。
fn is_maintenance_mode_requested(env_value: Option<&str>, args: &[String]) -> bool {
    env_value.map_or(false, |v| v == "1") || args.iter().any(|a| a == "--maintenance")
}

fn main() {
    let is_maintenance = is_maintenance_mode_requested(
        std::env::var("QUICKCLIPBOARD_MAINTENANCE").ok().as_deref(),
        &std::env::args().collect::<Vec<String>>(),
    );

    if is_maintenance {
        quickclipboard_lib::install_startup_panic_hook();
        #[cfg(windows)]
        quickclipboard_lib::maintenance::ensure_console();
        quickclipboard_lib::maintenance::run();
        return;
    }

    quickclipboard_lib::install_startup_panic_hook();
    quickclipboard_lib::maintenance::ensure_bat_file();
    quickclipboard_lib::run();
}

#[cfg(test)]
mod tests {
    use super::is_maintenance_mode_requested;

    #[test]
    fn no_env_no_args_is_normal_mode() {
        assert!(!is_maintenance_mode_requested(None, &[]));
    }

    #[test]
    fn env_value_one_triggers_maintenance() {
        assert!(is_maintenance_mode_requested(Some("1"), &[]));
        assert!(is_maintenance_mode_requested(Some("1"), &["--maintenance".to_string()]));
    }

    #[test]
    fn env_value_zero_does_not_trigger_maintenance() {
        assert!(!is_maintenance_mode_requested(Some("0"), &[]));
        // 但参数是独立触发条件：env="0" 时带 --maintenance 参数仍进入维护模式
        assert!(is_maintenance_mode_requested(Some("0"), &["--maintenance".to_string()]));
    }

    #[test]
    fn env_value_must_equal_one_exactly() {
        assert!(!is_maintenance_mode_requested(Some(""), &[]));
        assert!(!is_maintenance_mode_requested(Some(" 1"), &[]));
        assert!(!is_maintenance_mode_requested(Some("1 "), &[]));
        assert!(!is_maintenance_mode_requested(Some("01"), &[]));
    }

    #[test]
    fn arg_maintenance_triggers_without_env() {
        assert!(is_maintenance_mode_requested(None, &["--maintenance".to_string()]));
        assert!(is_maintenance_mode_requested(None, &["--maintenance".to_string(), "--x".to_string()]));
    }

    #[test]
    fn arg_must_match_exactly() {
        assert!(!is_maintenance_mode_requested(None, &["--maintenancex".to_string()]));
        assert!(!is_maintenance_mode_requested(None, &["--MAINTENANCE".to_string()]));
        assert!(!is_maintenance_mode_requested(None, &["maintenance".to_string()]));
    }

    #[test]
    fn unrelated_args_do_not_trigger() {
        assert!(!is_maintenance_mode_requested(None, &["--foo".to_string(), "bar".to_string()]));
    }
}
