use low_memory_fltk::{ListItem, PageItem, ShowOptions, ThemeColors, UiEvent};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use tauri::AppHandle;

use crate::services::database::{query_clipboard_items, QueryParams};

const PANEL_WIDTH_LOGICAL: i32 = 420;
const PANEL_PAGE_SIZE: i64 = 25;
const MAX_PREVIEW_CHARS: usize = 120;

#[derive(Debug, Clone, Copy, Default)]
struct PanelPosition {
    logical_x: i32,
    logical_y: i32,
    physical_x: i32,
    physical_y: i32,
    physical_width: i32,
    physical_height: i32,
}

#[derive(Debug, Default)]
struct PanelPageState {
    current_page: i64,
    last_position: Option<PanelPosition>,
}

static PANEL_PAGE_STATE: Lazy<Mutex<PanelPageState>> =
    Lazy::new(|| Mutex::new(PanelPageState::default()));

pub fn init_panel(app: AppHandle) -> Result<(), String> {
    low_memory_fltk::init(move |event| match event {
        UiEvent::ItemActivated(item_id) => {
            let app = app.clone();
            std::thread::spawn(move || {
                handle_item_activated(&app, item_id);
            });
        }
        UiEvent::PageScroll(delta) => {
            std::thread::spawn(move || {
                if let Err(error) = scroll_panel_page(delta) {
                    eprintln!("低占用模式列表滚动翻页失败: {}", error);
                }
            });
        }
        UiEvent::PageSelected(page) => {
            std::thread::spawn(move || {
                if let Err(error) = jump_panel_page(page) {
                    eprintln!("低占用模式列表跳转页失败: {}", error);
                }
            });
        }
        UiEvent::Hidden => {}
    })
}

pub fn show_panel() -> Result<(), String> {
    {
        let mut state = PANEL_PAGE_STATE.lock();
        state.current_page = 0;
        state.last_position = None;
    }
    show_panel_at_current_page(None)
}

fn scroll_panel_page(delta: i32) -> Result<bool, String> {
    let (current_page, last_position) = {
        let state = PANEL_PAGE_STATE.lock();
        (state.current_page, state.last_position)
    };

    let page = load_page(current_page)?;
    if page.total_pages <= 1 {
        return Ok(false);
    }

    let next_page = if delta > 0 {
        if current_page >= page.total_pages - 1 {
            0
        } else {
            current_page + 1
        }
    } else if delta < 0 {
        if current_page <= 0 {
            page.total_pages - 1
        } else {
            current_page - 1
        }
    } else {
        current_page
    };

    if next_page == current_page {
        return Ok(false);
    }

    {
        let mut state = PANEL_PAGE_STATE.lock();
        state.current_page = next_page;
    }

    show_panel_at_current_page(last_position)?;
    Ok(true)
}

fn jump_panel_page(page: i64) -> Result<bool, String> {
    let last_position = PANEL_PAGE_STATE.lock().last_position;
    let page_data = load_page(page)?;
    let target_page = page_data.current_page;

    {
        let mut state = PANEL_PAGE_STATE.lock();
        if state.current_page == target_page {
            return Ok(false);
        }
        state.current_page = target_page;
    }

    show_panel_at_current_page(last_position)?;
    Ok(true)
}

fn show_panel_at_current_page(position_override: Option<PanelPosition>) -> Result<(), String> {
    let current_page = PANEL_PAGE_STATE.lock().current_page;
    let page = load_page(current_page)?;
    let height_logical = low_memory_fltk::preferred_height(page.items.len());
    let position = if let Some(position) = position_override {
        rebuild_position_from_existing(position, height_logical)?
    } else {
        build_position(height_logical)?
    };
    let theme = resolve_panel_theme();

    {
        let mut state = PANEL_PAGE_STATE.lock();
        state.last_position = Some(position);
    }

    low_memory_fltk::show(ShowOptions {
        items: page.items,
        footer_text: format!(
            "第 {}/{} 页 · {}-{} / {} 条",
            page.current_page + 1,
            page.total_pages.max(1),
            page.range_start,
            page.range_end,
            page.total_count
        ),
        page_items: build_page_items(page.total_pages, page.total_count),
        current_page: page.current_page,
        theme,
        x: position.logical_x,
        y: position.logical_y,
        width: PANEL_WIDTH_LOGICAL,
        height: height_logical,
        physical_x: position.physical_x,
        physical_y: position.physical_y,
        physical_width: position.physical_width,
        physical_height: position.physical_height,
    })
}

fn rebuild_position_from_existing(
    previous: PanelPosition,
    height_logical: i32,
) -> Result<PanelPosition, String> {
    let app = crate::utils::screen::get_app_handle().ok_or("APP_HANDLE 未初始化")?;
    let scale_factor =
        crate::screen::ScreenUtils::get_scale_factor_at_point(app, previous.physical_x, previous.physical_y);
    let physical_width = (PANEL_WIDTH_LOGICAL as f64 * scale_factor).round() as i32;
    let physical_height = (height_logical as f64 * scale_factor).round() as i32;
    let (physical_x, physical_y) = crate::screen::ScreenUtils::constrain_to_physical_bounds(
        app,
        previous.physical_x,
        previous.physical_y,
        physical_width,
        physical_height,
    )?;

    Ok(PanelPosition {
        logical_x: (physical_x as f64 / scale_factor).round() as i32,
        logical_y: (physical_y as f64 / scale_factor).round() as i32,
        physical_x,
        physical_y,
        physical_width,
        physical_height,
    })
}

fn build_position(height_logical: i32) -> Result<PanelPosition, String> {
    let (cursor_x, cursor_y) = crate::mouse::get_cursor_position();
    let monitor = crate::screen::ScreenUtils::get_monitor_at_cursor_global()?;
    let scale_factor = monitor.scale_factor();
    let width_physical = (PANEL_WIDTH_LOGICAL as f64 * scale_factor).round() as i32;
    let height_physical = (height_logical as f64 * scale_factor).round() as i32;
    let position = crate::utils::positioning::calculate_popup_position(
        cursor_x,
        cursor_y,
        width_physical,
        height_physical,
        &monitor,
    );
    let logical_x = (position.x as f64 / scale_factor).round() as i32;
    let logical_y = (position.y as f64 / scale_factor).round() as i32;

    Ok(PanelPosition {
        logical_x,
        logical_y,
        physical_x: position.x,
        physical_y: position.y,
        physical_width: width_physical,
        physical_height: height_physical,
    })
}

pub fn hide_panel() -> Result<(), String> {
    PANEL_PAGE_STATE.lock().last_position = None;
    low_memory_fltk::hide()
}

pub fn toggle_panel() -> Result<(), String> {
    if low_memory_fltk::is_visible() {
        hide_panel()
    } else {
        show_panel()
    }
}

pub fn is_panel_visible() -> bool {
    low_memory_fltk::is_visible()
}

pub fn is_point_in_panel(x: i32, y: i32) -> bool {
    low_memory_fltk::contains_point(x, y)
}

fn handle_item_activated(app: &AppHandle, item_id: i64) {
    use crate::services::database::get_clipboard_item_by_id;
    use crate::services::paste::paste_handler::paste_clipboard_item_with_update;
    use crate::services::system::restore_last_focus;

    if item_id <= 0 {
        return;
    }

    let _ = hide_panel();
    let _ = restore_last_focus();

    std::thread::sleep(std::time::Duration::from_millis(80));

    if let Ok(Some(item)) = get_clipboard_item_by_id(item_id) {
        if let Err(error) = paste_clipboard_item_with_update(&item) {
            eprintln!("低占用模式列表粘贴失败: {}", error);
            let _ = crate::services::notification::show_notification(
                app,
                "低占用模式",
                "列表项粘贴失败，请重试。",
            );
        }
    }
}

#[derive(Debug)]
struct PanelPage {
    items: Vec<ListItem>,
    total_count: i64,
    total_pages: i64,
    current_page: i64,
    range_start: i64,
    range_end: i64,
}

fn load_page(page: i64) -> Result<PanelPage, String> {
    let offset = page.max(0) * PANEL_PAGE_SIZE;
    let result = query_clipboard_items(QueryParams {
        offset,
        limit: PANEL_PAGE_SIZE,
        search: None,
        content_type: None,
    })?;

    let total_pages = if result.total_count == 0 {
        1
    } else {
        ((result.total_count + PANEL_PAGE_SIZE - 1) / PANEL_PAGE_SIZE).max(1)
    };

    let mut items: Vec<ListItem> = result
        .items
        .into_iter()
        .map(|item| ListItem {
            id: item.id,
            label: format_item_label(&item),
            kind_label: item_kind_label(&item.content_type).to_string(),
            is_pinned: item.is_pinned,
        })
        .collect();

    if items.is_empty() {
        items.push(ListItem {
            id: 0,
            label: "(暂无记录)".to_string(),
            kind_label: String::new(),
            is_pinned: false,
        });
    }

    let safe_total_count = result.total_count.max(0);
    let clamped_page = page.max(0).min(total_pages - 1);
    let range_start = if safe_total_count == 0 {
        0
    } else {
        clamped_page * PANEL_PAGE_SIZE + 1
    };
    let range_end = if safe_total_count == 0 {
        0
    } else {
        ((clamped_page + 1) * PANEL_PAGE_SIZE).min(safe_total_count)
    };

    Ok(PanelPage {
        items,
        total_count: safe_total_count,
        total_pages,
        current_page: clamped_page,
        range_start,
        range_end,
    })
}

fn build_page_items(total_pages: i64, total_count: i64) -> Vec<PageItem> {
    let total_pages = total_pages.max(1);
    let total_count = total_count.max(0);
    let mut items = Vec::with_capacity(total_pages as usize);

    for page_index in 0..total_pages {
        let range_start = if total_count == 0 {
            0
        } else {
            page_index * PANEL_PAGE_SIZE + 1
        };
        let range_end = if total_count == 0 {
            0
        } else {
            ((page_index + 1) * PANEL_PAGE_SIZE).min(total_count)
        };

        items.push(PageItem {
            page_index,
            label: format!("第 {} 页 · {}-{} 条", page_index + 1, range_start, range_end),
        });
    }

    items
}

fn normalize_text(text: &str) -> String {
    let mut result = String::new();
    let mut last_was_space = false;

    for c in text.chars() {
        if c.is_whitespace() {
            if !last_was_space {
                result.push(' ');
                last_was_space = true;
            }
        } else {
            result.push(c);
            last_was_space = false;
        }
    }

    result.trim().to_string()
}

fn summarize_text(text: &str) -> String {
    let text = normalize_text(text);
    if text.is_empty() {
        return "(空内容)".to_string();
    }

    let mut result = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= MAX_PREVIEW_CHARS {
            result.push('…');
            break;
        }
        result.push(ch);
    }
    result
}

fn parse_files_content(content: &str) -> Option<Vec<String>> {
    if !content.starts_with("files:") {
        return None;
    }

    let json_str = &content[6..];
    let parsed: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let files = parsed.get("files")?.as_array()?;

    let names: Vec<String> = files
        .iter()
        .filter_map(|file| file.get("name").and_then(|name| name.as_str()).map(|name| name.to_string()))
        .collect();

    if names.is_empty() { None } else { Some(names) }
}

fn summarize_named_items(names: &[String], unit: &str) -> String {
    if names.is_empty() {
        return format!("(空{})", unit);
    }

    if names.len() == 1 {
        return summarize_text(&names[0]);
    }

    let first = summarize_text(&names[0]);
    let second = summarize_text(&names[1]);
    if names.len() == 2 {
        return format!("{} · {}", first, second);
    }

    format!("{} · {} 等 {} 个{}", first, second, names.len(), unit)
}

fn format_item_label(item: &crate::services::database::ClipboardItem) -> String {
    match item.content_type.as_str() {
        "text" | "link" | "rich_text" => summarize_text(&item.content),
        "image" => {
            if let Some(names) = parse_files_content(&item.content) {
                summarize_named_items(&names, "张图片")
            } else {
                "图片".to_string()
            }
        }
        "file" => {
            if let Some(names) = parse_files_content(&item.content) {
                summarize_named_items(&names, "个文件")
            } else {
                let filename = item
                    .content
                    .split(['/', '\\'])
                    .last()
                    .unwrap_or("文件");
                summarize_text(filename)
            }
        }
        _ => summarize_text(&item.content),
    }
}

fn item_kind_label(content_type: &str) -> &'static str {
    match content_type {
        "text" => "文本",
        "link" => "链接",
        "rich_text" => "富文",
        "image" => "图片",
        "file" => "文件",
        _ => "其他",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PanelThemeKind {
    Light,
    DarkModern,
    DarkClassic,
}

fn resolve_panel_theme() -> ThemeColors {
    match resolve_panel_theme_kind() {
        PanelThemeKind::Light => ThemeColors {
            window_bg: (255, 255, 255),
            panel_bg: (243, 244, 246),
            footer_bg: (229, 231, 235),
            hover_bg: blend_rgba((243, 244, 246), (59, 130, 246, 31)),
            border: blend_rgba((243, 244, 246), (17, 24, 39, 56)),
            accent: (59, 130, 246),
            window_border: (71, 85, 105),
            text: (17, 24, 39),
            footer_text: (107, 114, 128),
        },
        PanelThemeKind::DarkModern => ThemeColors {
            window_bg: (17, 24, 39),
            panel_bg: (31, 41, 55),
            footer_bg: (45, 51, 66),
            hover_bg: blend_rgba((31, 41, 55), (96, 165, 250, 46)),
            border: blend_rgba((31, 41, 55), (255, 255, 255, 56)),
            accent: (59, 130, 246),
            window_border: (229, 231, 235),
            text: (229, 231, 235),
            footer_text: (203, 213, 225),
        },
        PanelThemeKind::DarkClassic => ThemeColors {
            window_bg: (30, 30, 30),
            panel_bg: (42, 42, 42),
            footer_bg: (51, 51, 51),
            hover_bg: blend_rgba((42, 42, 42), (74, 137, 220, 46)),
            border: blend_rgba((42, 42, 42), (255, 255, 255, 56)),
            accent: (74, 137, 220),
            window_border: (224, 224, 224),
            text: (224, 224, 224),
            footer_text: (199, 199, 199),
        },
    }
}

/// 纯决策函数：主题字符串 + 深色样式 + 系统深色标志 → 面板主题种类。
/// 不触碰任何全局状态，便于测试注入 system_dark 双侧。
fn resolve_theme_kind_from(theme: &str, dark_theme_style: &str, system_dark: bool) -> PanelThemeKind {
    let theme = theme.trim();

    if theme == "dark" {
        return if dark_theme_style == "modern" {
            PanelThemeKind::DarkModern
        } else {
            PanelThemeKind::DarkClassic
        };
    }

    if theme == "auto" && system_dark {
        return if dark_theme_style == "modern" {
            PanelThemeKind::DarkModern
        } else {
            PanelThemeKind::DarkClassic
        };
    }

    PanelThemeKind::Light
}

fn resolve_panel_theme_kind() -> PanelThemeKind {
    let settings = crate::get_settings();
    resolve_theme_kind_from(&settings.theme, &settings.dark_theme_style, is_system_dark_mode())
}

fn blend_rgba(base: (u8, u8, u8), overlay: (u8, u8, u8, u8)) -> (u8, u8, u8) {
    let alpha = overlay.3 as f32 / 255.0;
    let blend_channel = |base_channel: u8, overlay_channel: u8| -> u8 {
        ((base_channel as f32 * (1.0 - alpha)) + (overlay_channel as f32 * alpha)).round() as u8
    };

    (
        blend_channel(base.0, overlay.0),
        blend_channel(base.1, overlay.1),
        blend_channel(base.2, overlay.2),
    )
}

#[cfg(target_os = "windows")]
fn is_system_dark_mode() -> bool {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let personalize = hkcu.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    let Ok(key) = personalize else {
        return false;
    };

    let value: Result<u32, _> = key.get_value("AppsUseLightTheme");
    matches!(value, Ok(0))
}

#[cfg(not(target_os = "windows"))]
fn is_system_dark_mode() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::database::connection::test_support::{
        ensure_isolated_settings_env, SettingsGuard, TestDb, TEST_ENV_LOCK,
    };
    use crate::services::database::ClipboardItem;
    use crate::services::update_settings;

    fn make_item(content_type: &str, content: &str) -> ClipboardItem {
        ClipboardItem {
            id: 1,
            uuid: None,
            favorite_id: None,
            source_device_id: None,
            is_remote: false,
            content: content.to_string(),
            html_content: None,
            content_type: content_type.to_string(),
            image_id: None,
            item_order: 0,
            is_pinned: false,
            paste_count: 0,
            source_app: None,
            source_icon_hash: None,
            char_count: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    fn seed_clipboard(db: &TestDb, count: i64) {
        for i in 0..count {
            db.exec(
                "INSERT INTO clipboard (content, content_type, item_order, created_at, updated_at) VALUES (?1, 'text', ?2, 1, 1)",
                &[&format!("item-{}", i), &(i + 1)],
            );
        }
    }

    #[test]
    fn normalize_text_collapses_whitespace_runs_and_trims() {
        assert_eq!(normalize_text("a  b\t\n c"), "a b c");
        assert_eq!(normalize_text("  hello   world  "), "hello world");
        assert_eq!(normalize_text("\t\n "), "");
        // U+00A0 (NBSP) 属于 Unicode White_Space，同样会被折叠为单个空格
        assert_eq!(normalize_text("a\u{00A0}b"), "a b");
        assert_eq!(normalize_text("a\u{00A0}\u{00A0}b"), "a b");
    }

    #[test]
    fn summarize_text_returns_placeholder_for_empty_and_normalizes() {
        assert_eq!(summarize_text(""), "(空内容)");
        assert_eq!(summarize_text("   \n "), "(空内容)");
        assert_eq!(summarize_text("  a   b "), "a b");
    }

    #[test]
    fn summarize_text_truncates_at_120_chars_with_ellipsis() {
        // 契约值：MAX_PREVIEW_CHARS = 120（用字面量固定，防止常量被改动后测试自洽）
        let exactly_120 = "a".repeat(120);
        assert_eq!(summarize_text(&exactly_120), exactly_120, "正好 120 字符不加省略号");

        let over_120 = "a".repeat(121);
        let expected = format!("{}…", "a".repeat(120));
        assert_eq!(summarize_text(&over_120), expected, "超过 120 字符截断并追加省略号");
        assert_eq!(expected.chars().count(), 121);

        // 按字符（而非字节）计数：多字节字符同样在 120 字符处截断
        let chinese = "中".repeat(121);
        let expected_cn = format!("{}…", "中".repeat(120));
        assert_eq!(summarize_text(&chinese), expected_cn);
    }

    #[test]
    fn parse_files_content_extracts_names_from_files_json() {
        let content = "files:{\"files\":[{\"name\":\"a.png\"},{\"name\":\"b.jpg\"}]}";
        assert_eq!(
            parse_files_content(content),
            Some(vec!["a.png".to_string(), "b.jpg".to_string()])
        );
    }

    #[test]
    fn parse_files_content_rejects_invalid_or_missing_files_json() {
        assert_eq!(parse_files_content("files:not-json"), None, "非法 JSON → None");
        assert_eq!(parse_files_content("files:{\"files\":[]}"), None, "空数组 → None");
        assert_eq!(
            parse_files_content("files:{\"foo\":[{\"name\":\"a\"}]}"),
            None,
            "缺少 files 字段 → None"
        );
        assert_eq!(
            parse_files_content("files:{\"files\":[{\"path\":\"a\"}]}"),
            None,
            "条目缺 name 字段 → 全部过滤 → None"
        );
        assert_eq!(
            parse_files_content("text:{\"files\":[{\"name\":\"a\"}]}"),
            None,
            "不以 files: 开头 → None"
        );
        assert_eq!(parse_files_content("files:"), None, "空 JSON 尾部 → None");
    }

    #[test]
    fn summarize_named_items_formats_counts_and_units() {
        assert_eq!(summarize_named_items(&[], "个文件"), "(空个文件)");
        assert_eq!(
            summarize_named_items(&["only.txt".to_string()], "个文件"),
            "only.txt"
        );
        assert_eq!(
            summarize_named_items(&["a.png".to_string(), "b.jpg".to_string()], "张图片"),
            "a.png · b.jpg"
        );
        // 既有行为（quirk）：3 项以上时固定格式为“等 {n} 个{unit}”，
        // unit 直接拼在硬编码的“个”之后：unit="个文件" → “个个文件”，
        // unit="张图片" → “个张图片”。
        assert_eq!(
            summarize_named_items(
                &["a.txt".to_string(), "b.txt".to_string(), "c.txt".to_string()],
                "个文件"
            ),
            "a.txt · b.txt 等 3 个个文件"
        );
        assert_eq!(
            summarize_named_items(
                &["a.png".to_string(), "b.png".to_string(), "c.png".to_string()],
                "张图片"
            ),
            "a.png · b.png 等 3 个张图片"
        );
    }

    #[test]
    fn format_item_label_dispatches_by_content_type() {
        assert_eq!(format_item_label(&make_item("text", "  hi   there ")), "hi there");
        assert_eq!(format_item_label(&make_item("link", "  hi   there ")), "hi there");
        assert_eq!(format_item_label(&make_item("rich_text", "  hi   there ")), "hi there");
        assert_eq!(format_item_label(&make_item("unknown_type", "  hi   there ")), "hi there");
    }

    #[test]
    fn format_item_label_summarizes_long_text_with_ellipsis() {
        let content = "a".repeat(125);
        let expected = format!("{}…", "a".repeat(120));
        assert_eq!(format_item_label(&make_item("text", &content)), expected);
    }

    #[test]
    fn format_item_label_handles_image_content() {
        let with_files = make_item(
            "image",
            "files:{\"files\":[{\"name\":\"a.png\"},{\"name\":\"b.jpg\"}]}",
        );
        assert_eq!(format_item_label(&with_files), "a.png · b.jpg");

        let plain = make_item("image", "some-raw-bytes");
        assert_eq!(format_item_label(&plain), "图片");
    }

    #[test]
    fn format_item_label_handles_file_content() {
        let with_files = make_item(
            "file",
            "files:{\"files\":[{\"name\":\"a.txt\"},{\"name\":\"b.txt\"},{\"name\":\"c.txt\"}]}",
        );
        assert_eq!(format_item_label(&with_files), "a.txt · b.txt 等 3 个个文件");

        assert_eq!(
            format_item_label(&make_item("file", "C:\\Users\\me\\report.pdf")),
            "report.pdf",
            "Windows 路径取最后一段"
        );
        assert_eq!(
            format_item_label(&make_item("file", "/tmp/dir/doc.txt")),
            "doc.txt",
            "POSIX 路径取最后一段"
        );
    }

    #[test]
    fn item_kind_label_maps_exact_content_types() {
        assert_eq!(item_kind_label("text"), "文本");
        assert_eq!(item_kind_label("link"), "链接");
        assert_eq!(item_kind_label("rich_text"), "富文");
        assert_eq!(item_kind_label("image"), "图片");
        assert_eq!(item_kind_label("file"), "文件");
        assert_eq!(item_kind_label("anything-else"), "其他");
        assert_eq!(item_kind_label(""), "其他");
    }

    #[test]
    fn build_page_items_computes_exact_labels_and_ranges() {
        // PageItem 未实现 PartialEq，按 (page_index, label) 逐项断言
        // 空数据：单页且范围 0-0
        let items: Vec<(i64, String)> = build_page_items(1, 0)
            .into_iter()
            .map(|item| (item.page_index, item.label))
            .collect();
        assert_eq!(items, vec![(0, "第 1 页 · 0-0 条".to_string())]);

        // total_pages 至少为 1
        let items: Vec<(i64, String)> = build_page_items(0, 0)
            .into_iter()
            .map(|item| (item.page_index, item.label))
            .collect();
        assert_eq!(items, vec![(0, "第 1 页 · 0-0 条".to_string())]);

        let items: Vec<(i64, String)> = build_page_items(2, 26)
            .into_iter()
            .map(|item| (item.page_index, item.label))
            .collect();
        assert_eq!(
            items,
            vec![
                (0, "第 1 页 · 1-25 条".to_string()),
                (1, "第 2 页 · 26-26 条".to_string()),
            ]
        );

        let items: Vec<(i64, String)> = build_page_items(3, 75)
            .into_iter()
            .map(|item| (item.page_index, item.label))
            .collect();
        assert_eq!(
            items,
            vec![
                (0, "第 1 页 · 1-25 条".to_string()),
                (1, "第 2 页 · 26-50 条".to_string()),
                (2, "第 3 页 · 51-75 条".to_string()),
            ]
        );
    }

    #[test]
    fn blend_rgba_blends_exact_channels() {
        assert_eq!(
            blend_rgba((243, 244, 246), (59, 130, 246, 0)),
            (243, 244, 246),
            "alpha=0 时完全保留底色"
        );
        assert_eq!(
            blend_rgba((243, 244, 246), (59, 130, 246, 255)),
            (59, 130, 246),
            "alpha=255 时完全覆盖为叠色"
        );
        // Light 主题 hover_bg: blend((243,244,246), (59,130,246,31))
        assert_eq!(blend_rgba((243, 244, 246), (59, 130, 246, 31)), (221, 230, 246));
        // Light 主题 border: blend((243,244,246), (17,24,39,56))
        assert_eq!(blend_rgba((243, 244, 246), (17, 24, 39, 56)), (193, 196, 201));
        // 50% 半透明黑覆盖白 → 128
        assert_eq!(blend_rgba((0, 0, 0), (255, 255, 255, 128)), (128, 128, 128));
    }

    #[test]
    fn load_page_empty_database_yields_placeholder_page() {
        let _guard = TEST_ENV_LOCK.lock();
        let db = TestDb::new();
        let page = load_page(0).expect("空库加载第 0 页应成功");
        assert_eq!(page.total_count, 0);
        assert_eq!(page.total_pages, 1);
        assert_eq!(page.current_page, 0);
        assert_eq!(page.range_start, 0);
        assert_eq!(page.range_end, 0);
        assert_eq!(page.items.len(), 1, "空库应展示占位项");
        assert_eq!(page.items[0].id, 0);
        assert_eq!(page.items[0].label, "(暂无记录)");
        assert_eq!(page.items[0].kind_label, "");
    }

    #[test]
    fn load_page_single_full_page_ranges() {
        let _guard = TEST_ENV_LOCK.lock();
        let db = TestDb::new();
        seed_clipboard(&db, 25);
        let page = load_page(0).expect("加载第 0 页应成功");
        assert_eq!(page.total_count, 25);
        assert_eq!(page.total_pages, 1);
        assert_eq!(page.current_page, 0);
        assert_eq!(page.range_start, 1);
        assert_eq!(page.range_end, 25);
        assert_eq!(page.items.len(), 25);
    }

    #[test]
    fn load_page_multi_page_ranges_and_clamping() {
        let _guard = TEST_ENV_LOCK.lock();
        let db = TestDb::new();
        seed_clipboard(&db, 26);

        let page0 = load_page(0).expect("第 0 页");
        assert_eq!(page0.total_count, 26);
        assert_eq!(page0.total_pages, 2);
        assert_eq!(page0.current_page, 0);
        assert_eq!(page0.range_start, 1);
        assert_eq!(page0.range_end, 25);
        assert_eq!(page0.items.len(), 25);

        let page1 = load_page(1).expect("第 1 页");
        assert_eq!(page1.current_page, 1);
        assert_eq!(page1.range_start, 26);
        assert_eq!(page1.range_end, 26);
        assert_eq!(page1.items.len(), 1);

        // 负页码钳制到第 0 页
        let clamped_neg = load_page(-3).expect("负页码");
        assert_eq!(clamped_neg.current_page, 0);
        assert_eq!(clamped_neg.range_start, 1);
        assert_eq!(clamped_neg.range_end, 25);
        assert_eq!(clamped_neg.items.len(), 25);

        // 超界页码钳制到最后一页；偏移超出数据范围时展示占位项
        let clamped_high = load_page(99).expect("超界页码");
        assert_eq!(clamped_high.current_page, 1);
        assert_eq!(clamped_high.range_start, 26);
        assert_eq!(clamped_high.range_end, 26);
        assert_eq!(clamped_high.items.len(), 1);
        assert_eq!(clamped_high.items[0].id, 0);
        assert_eq!(clamped_high.items[0].label, "(暂无记录)");
    }

    #[test]
    fn theme_resolution_dark_theme_follows_dark_style() {
        let _guard = TEST_ENV_LOCK.lock();
        ensure_isolated_settings_env();
        let _restore = SettingsGuard(crate::services::get_settings());

        let mut settings = crate::services::get_settings();
        settings.theme = "dark".to_string();
        settings.dark_theme_style = "modern".to_string();
        update_settings(settings).expect("更新设置");
        assert_eq!(resolve_panel_theme_kind(), PanelThemeKind::DarkModern);

        let mut settings = crate::services::get_settings();
        settings.theme = "dark".to_string();
        settings.dark_theme_style = "classic".to_string();
        update_settings(settings).expect("更新设置");
        assert_eq!(resolve_panel_theme_kind(), PanelThemeKind::DarkClassic);

        // theme 值两侧空白会被 trim
        let mut settings = crate::services::get_settings();
        settings.theme = " dark ".to_string();
        settings.dark_theme_style = "modern".to_string();
        update_settings(settings).expect("更新设置");
        assert_eq!(resolve_panel_theme_kind(), PanelThemeKind::DarkModern);
    }

    #[test]
    fn theme_resolution_light_and_auto_follow_system_dark_mode() {
        let _guard = TEST_ENV_LOCK.lock();
        ensure_isolated_settings_env();
        let _restore = SettingsGuard(crate::services::get_settings());

        let mut settings = crate::services::get_settings();
        settings.theme = "light".to_string();
        update_settings(settings).expect("更新设置");
        assert_eq!(resolve_panel_theme_kind(), PanelThemeKind::Light);

        // auto 双侧注入：纯决策函数不依赖平台实时 is_system_dark_mode()
        assert_eq!(
            resolve_theme_kind_from("auto", "modern", true),
            PanelThemeKind::DarkModern,
            "auto + 系统深色 → dark 样式"
        );
        assert_eq!(
            resolve_theme_kind_from("auto", "classic", true),
            PanelThemeKind::DarkClassic
        );
        assert_eq!(
            resolve_theme_kind_from("auto", "modern", false),
            PanelThemeKind::Light,
            "auto + 系统浅色 → Light"
        );
        // light 不受系统深色影响；dark 不看系统深色标志；两侧空白 trim
        assert_eq!(
            resolve_theme_kind_from("light", "modern", true),
            PanelThemeKind::Light
        );
        assert_eq!(
            resolve_theme_kind_from("dark", "classic", false),
            PanelThemeKind::DarkClassic
        );
        assert_eq!(
            resolve_theme_kind_from(" auto ", "modern", true),
            PanelThemeKind::DarkModern,
            "theme 两侧空白被 trim"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn system_dark_mode_is_false_on_non_windows() {
        assert!(!is_system_dark_mode());
    }
}
