use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppLinks {
    pub website: String,
    pub github: String,
    #[serde(rename = "pdChannel", default)]
    pub pd_channel: String,
    #[serde(rename = "qqGroup1", default)]
    pub qq_group_1: String,
    #[serde(rename = "qqGroup2", default)]
    pub qq_group_2: String,
    pub bilibili: String,
    pub changelog: String,
    #[serde(rename = "releasesLatest")]
    pub releases_latest: String,
}

static LINKS: Lazy<Result<AppLinks, String>> = Lazy::new(|| {
    let json = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../src/shared/config/appLinks.json"));
    serde_json::from_str::<AppLinks>(json).map_err(|e| format!("appLinks.json 解析失败: {}", e))
});

pub fn app_links() -> Result<&'static AppLinks, String> {
    match LINKS.as_ref() {
        Ok(v) => Ok(v),
        Err(e) => Err(e.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_links_parses_embedded_config_with_renamed_fields() {
        let links = app_links().expect("appLinks.json must parse");
        assert_eq!(links.website, "https://quickclipboard.cn/");
        assert_eq!(links.github, "https://github.com/mosheng1/QuickClipboard");
        assert_eq!(links.pd_channel, "https://pd.qq.com/s/blp3j847c");
        assert_eq!(links.qq_group_1, "https://qm.qq.com/q/nUCO76MX9C");
        assert_eq!(links.qq_group_2, "https://qm.qq.com/q/O5zOi3uTuy");
        assert_eq!(links.bilibili, "https://space.bilibili.com/438982697");
        assert_eq!(links.changelog, "https://quickclipboard.cn/zh/changelog");
        assert_eq!(
            links.releases_latest,
            "https://github.com/mosheng1/QuickClipboard/releases/latest"
        );
    }

    #[test]
    fn app_links_is_cached_and_stable_across_calls() {
        let a = app_links().unwrap();
        let b = app_links().unwrap();
        assert!(std::ptr::eq(a, b));
    }
}
