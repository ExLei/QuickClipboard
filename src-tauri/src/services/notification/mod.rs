use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

fn should_show_startup_notification(settings: &crate::services::AppSettings) -> bool {
    settings.show_startup_notification
}

// 无自定义快捷键时回退到默认 Shift+Space
fn startup_notification_shortcut(toggle_shortcut: &str) -> String {
    if toggle_shortcut.is_empty() {
        "Shift+Space".to_string()
    } else {
        toggle_shortcut.to_string()
    }
}

fn startup_notification_body(shortcut: &str) -> String {
    format!("QuickClipboard已启动\n按 {} 打开剪贴板窗口", shortcut)
}

pub fn show_startup_notification(app: &AppHandle) -> Result<(), String> {
    let settings = crate::services::get_settings();

    if !should_show_startup_notification(&settings) {
        return Ok(());
    }

    let shortcut = startup_notification_shortcut(&settings.toggle_shortcut);

    let notification_body = startup_notification_body(&shortcut);

    app.notification()
        .builder()
        .title("QuickClipboard")
        .body(&notification_body)
        .show()
        .map_err(|e| format!("显示通知失败: {}", e))?;

    Ok(())
}

// 显示通用消息通知
#[allow(dead_code)]
pub fn show_notification(
    app: &AppHandle,
    title: &str,
    body: &str,
) -> Result<(), String> {
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| format!("显示通知失败: {}", e))?;
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_notification_falls_back_to_shift_space() {
        assert_eq!(startup_notification_shortcut(""), "Shift+Space");
        assert_eq!(startup_notification_shortcut("  "), "  ");
        assert_eq!(startup_notification_shortcut("Ctrl+Shift+A"), "Ctrl+Shift+A");
    }

    #[test]
    fn startup_notification_body_embeds_shortcut_verbatim() {
        let body = startup_notification_body("Ctrl+Shift+A");
        assert!(body.starts_with("QuickClipboard已启动\n"));
        assert!(body.contains("按 Ctrl+Shift+A 打开剪贴板窗口"));
        assert_eq!(body, "QuickClipboard已启动\n按 Ctrl+Shift+A 打开剪贴板窗口");
    }

    #[test]
    fn startup_notification_gate_follows_setting() {
        let mut settings = crate::services::AppSettings::default();
        assert!(should_show_startup_notification(&settings));
        settings.show_startup_notification = false;
        assert!(!should_show_startup_notification(&settings));
    }
}

