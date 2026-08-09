pub mod app_links;
pub mod cf_html;
pub mod html;
pub mod icon;
pub mod image;
pub mod mouse;
pub mod positioning;
pub mod screen;
pub mod system;
pub mod text;

/// `WebviewWindowBuilder::drag_and_drop` 仅在 Windows 上可用（tao 扩展 API，
/// tauri-runtime 的 `WindowBuilder::drag_and_drop` 带 `#[cfg(windows)]`）。
/// 其他平台原样透传，保持构建链可跨平台编译。
pub trait WindowDragAndDropExt {
    fn drag_and_drop_cfg(self, enabled: bool) -> Self;
}

impl<'a, R: tauri::Runtime, M: tauri::Manager<R>> WindowDragAndDropExt
    for tauri::WebviewWindowBuilder<'a, R, M>
{
    #[cfg(windows)]
    fn drag_and_drop_cfg(self, enabled: bool) -> Self {
        self.drag_and_drop(enabled)
    }

    #[cfg(not(windows))]
    fn drag_and_drop_cfg(self, _enabled: bool) -> Self {
        self
    }
}

pub use html::truncate_html;
pub use image::{get_image_dimensions, is_image_file};
pub use screen::init_screen_utils;
pub use system::get_text_scale_factor;
pub use text::{is_textual_content_type, truncate_around_keyword, truncate_string};

#[cfg(test)]
mod tests {
    use super::*;

    // 编译期契约：trait 必须由具体类型实现（删除 trait / impl / 改签名 → 编译失败即红）
    fn assert_impl<T: WindowDragAndDropExt>() {}

    #[test]
    fn webview_window_builder_implements_drag_and_drop_ext() {
        assert_impl::<
            tauri::WebviewWindowBuilder<
                '_,
                tauri::test::MockRuntime,
                tauri::App<tauri::test::MockRuntime>,
            >,
        >();
    }

    // 非 Windows：透传分支编译 + build 不 panic。
    // 注意：MockRuntime 无法观察窗口配置（dispatcher title() 恒返回 ""、builder 配置被丢弃），
    // 恒等语义由编译期 assert_impl + 结构强制（move-only builder 返回 Self）保证，此处仅冒烟。
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn drag_and_drop_cfg_compiles_and_builds_on_non_windows() {
        use tauri::test::mock_app;

        let app = mock_app();
        let build = |label: &str, with_cfg: bool| {
            let mut b = tauri::WebviewWindowBuilder::new(
                &app,
                label,
                tauri::WebviewUrl::App("index.html".into()),
            );
            if with_cfg {
                b = b.drag_and_drop_cfg(true);
            }
            b.build()
        };

        build("smoke-direct", false).expect("直接 build 应成功");
        build("smoke-cfg", true).expect("经透传 build 应成功");
    }

    // Windows：转发分支编译 + build 不 panic（true/false 双值）。
    // 注意：MockRuntime 的 drag_and_drop 是 no-op，转发值无法在 mock 下观察；
    // 转发正确性需真 Windows（tauri-runtime-wry）集成测试，此处仅守卫编译路径与分支存在。
    #[cfg(windows)]
    #[test]
    fn drag_and_drop_cfg_compiles_and_builds_on_windows() {
        use tauri::test::mock_app;

        let app = mock_app();
        let _false_branch = tauri::WebviewWindowBuilder::new(
            &app,
            "fwd-cfg",
            tauri::WebviewUrl::App("index.html".into()),
        )
        .drag_and_drop_cfg(false)
        .build()
        .expect("经转发 build 应成功");
        let _true_branch = tauri::WebviewWindowBuilder::new(
            &app,
            "fwd-cfg-true",
            tauri::WebviewUrl::App("index.html".into()),
        )
        .drag_and_drop_cfg(true)
        .build()
        .expect("true 分支 build 应成功");
    }
}
