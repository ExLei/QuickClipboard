// WebView2 环境变量安全检查

// 危险参数列表：(参数模式, 描述)
const DANGEROUS_PATTERNS: &[(&str, &str)] = &[
    // DevTools 相关
    ("--auto-open-devtools-for-tabs", "自动打开开发者工具"),
    ("--remote-debugging-port", "远程调试端口"),
    ("--remote-debugging-pipe", "远程调试管道"),
    ("--remote-debugging-address", "远程调试地址"),
    // 安全策略绕过
    ("--disable-web-security", "禁用网页安全策略"),
    ("--disable-site-isolation-trials", "禁用站点隔离"),
    ("--allow-running-insecure-content", "允许运行不安全内容"),
    ("--disable-features=IsolateOrigins", "禁用源隔离"),
    // 扩展注入
    ("--load-extension", "加载外部扩展"),
    ("--disable-extensions-except", "扩展白名单绕过"),
    // 用户数据篡改
    ("--user-data-dir", "自定义用户数据目录"),
    // 沙箱绕过
    ("--disable-gpu-sandbox", "禁用 GPU 沙箱"),
    ("--no-sandbox", "禁用沙箱"),
    ("--disable-setuid-sandbox", "禁用 setuid 沙箱"),
];

// 检查 WebView2 环境变量中是否包含危险参数
fn check_dangerous_webview2_args() -> Option<String> {
    let args = std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").ok()?;
    let args_lower = args.to_lowercase();
    
    let detected: Vec<String> = DANGEROUS_PATTERNS
        .iter()
        .filter(|(pattern, _)| args_lower.contains(&pattern.to_lowercase()))
        .map(|(pattern, desc)| format!("• {} ({})", pattern, desc))
        .collect();
    
    if detected.is_empty() {
        return None;
    }
    
    Some(format!(
        "环境变量: WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS\n\n检测到的危险参数:\n{}",
        detected.join("\n")
    ))
}

// 显示安全警告对话框
#[cfg(windows)]
fn show_security_warning(warning: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_OK, MB_ICONWARNING};
    use windows::core::PCWSTR;
    
    let title: Vec<u16> = "安全警告 - QuickClipboard\0".encode_utf16().collect();
    let message: Vec<u16> = format!(
        "检测到可能影响应用安全的环境变量配置：\n\n{}\n\n\
        为保护您的数据安全，应用将退出。\n\n\
        如需正常使用，请移除相关环境变量后重新启动。\0",
        warning
    ).encode_utf16().collect();
    
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONWARNING,
        );
    }
}

// 执行 WebView2 安全检查
// 如果检测到危险参数，显示警告对话框并退出程序
pub fn check_webview_security() {
    #[cfg(all(not(debug_assertions), windows))]
    {
        if let Some(warning) = check_dangerous_webview2_args() {
            show_security_warning(&warning);
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 临时设置/清除 WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS 后执行闭包，结束后恢复。
    /// 与其它修改进程级环境变量的测试共享 ENV_LOCK 串行化。
    fn with_args<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
        struct RestoreOnDrop(Option<std::ffi::OsString>);
        impl Drop for RestoreOnDrop {
            fn drop(&mut self) {
                match &self.0 {
                    Some(v) => std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", v),
                    None => std::env::remove_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"),
                }
            }
        }

        let _guard = crate::startup_diagnostics::tests::ENV_LOCK.lock();
        let _restore = RestoreOnDrop(std::env::var_os("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"));
        match value {
            Some(v) => std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", v),
            None => std::env::remove_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"),
        }
        f()
    }

    #[test]
    fn no_env_var_no_warning() {
        with_args(None, || assert_eq!(check_dangerous_webview2_args(), None));
    }

    #[test]
    fn empty_env_var_no_warning() {
        with_args(Some(""), || assert_eq!(check_dangerous_webview2_args(), None));
    }

    #[test]
    fn benign_args_no_warning() {
        with_args(Some("--disable-gpu --enable-logging --lang=zh-CN"), || {
            assert_eq!(check_dangerous_webview2_args(), None);
        });
    }

    #[test]
    fn single_dangerous_pattern_detected_with_exact_format() {
        with_args(Some("--no-sandbox"), || {
            let warning = check_dangerous_webview2_args().expect("应检测到危险参数");
            assert_eq!(
                warning,
                "环境变量: WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS\n\n检测到的危险参数:\n• --no-sandbox (禁用沙箱)"
            );
        });
    }

    #[test]
    fn detection_is_case_insensitive() {
        with_args(Some("--REMOTE-DEBUGGING-PORT=9222"), || {
            let warning = check_dangerous_webview2_args().expect("应检测到危险参数");
            assert_eq!(
                warning,
                "环境变量: WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS\n\n检测到的危险参数:\n• --remote-debugging-port (远程调试端口)"
            );
        });
    }

    #[test]
    fn all_dangerous_patterns_are_listed() {
        // 黄金表：数量 + 每个 pattern 的完整 warning 字面量，与生产 DANGEROUS_PATTERNS 解耦
        const GOLDEN: &[(&str, &str)] = &[
            ("--auto-open-devtools-for-tabs", "自动打开开发者工具"),
            ("--remote-debugging-port", "远程调试端口"),
            ("--remote-debugging-pipe", "远程调试管道"),
            ("--remote-debugging-address", "远程调试地址"),
            ("--disable-web-security", "禁用网页安全策略"),
            ("--disable-site-isolation-trials", "禁用站点隔离"),
            ("--allow-running-insecure-content", "允许运行不安全内容"),
            ("--disable-features=IsolateOrigins", "禁用源隔离"),
            ("--load-extension", "加载外部扩展"),
            ("--disable-extensions-except", "扩展白名单绕过"),
            ("--user-data-dir", "自定义用户数据目录"),
            ("--disable-gpu-sandbox", "禁用 GPU 沙箱"),
            ("--no-sandbox", "禁用沙箱"),
            ("--disable-setuid-sandbox", "禁用 setuid 沙箱"),
        ];
        assert_eq!(
            DANGEROUS_PATTERNS.len(),
            GOLDEN.len(),
            "生产危险参数数量与黄金表不一致"
        );
        for (pattern, desc) in GOLDEN {
            with_args(Some(pattern), || {
                let warning = check_dangerous_webview2_args().expect("应检测到危险参数");
                let expected = format!(
                    "环境变量: WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS\n\n检测到的危险参数:\n• {} ({})",
                    pattern, desc
                );
                assert_eq!(warning, expected, "pattern {:?} 的 warning 与黄金表不符", pattern);
            });
        }
    }

    #[test]
    fn multiple_dangerous_patterns_all_listed() {
        with_args(Some("--disable-web-security --no-sandbox"), || {
            let warning = check_dangerous_webview2_args().expect("应检测到危险参数");
            assert!(warning.contains("• --disable-web-security (禁用网页安全策略)"));
            assert!(warning.contains("• --no-sandbox (禁用沙箱)"));
        });
    }

    #[test]
    fn substring_prefix_matches_are_detected() {
        // 匹配语义是子串包含：危险模式作为其它参数的前缀时同样命中
        with_args(Some("--user-data-dirX"), || {
            assert!(check_dangerous_webview2_args().is_some());
        });
        with_args(Some("--no-sandboxed-foo"), || {
            assert!(check_dangerous_webview2_args().is_some());
        });
    }

    #[test]
    fn partial_flag_overlap_is_not_detected() {
        // 与危险模式名称部分重叠但并非子串包含的良性参数不误报
        with_args(Some("--user-agent=foo"), || {
            assert_eq!(check_dangerous_webview2_args(), None);
        });
        with_args(Some("--disable-gpu"), || {
            assert_eq!(check_dangerous_webview2_args(), None);
        });
        with_args(Some("--remote-debugging"), || {
            assert_eq!(check_dangerous_webview2_args(), None);
        });
    }
}
