//! 更新检查：读取 GitHub Releases 的最新发布，并按语义化版本比较先后。
//!
//! 网络请求与解析分开：`fetch_latest` 只负责取回响应文本，`parse_latest_release`
//! 只负责解析，两者都能单独测试。请求只在用户点「检查更新」时发一次，不轮询。

use anyhow::{Context, Result, anyhow};
use std::cmp::Ordering;
use std::time::Duration;

/// 默认的更新检查地址：项目在 GitHub 上的最新发布接口。
pub(crate) const DEFAULT_UPDATE_CHECK_URL: &str =
    "https://api.github.com/repos/billowsand/gongwen/releases/latest";

/// 一次更新检查的结果形式。
#[derive(Debug, Clone)]
pub(crate) struct ReleaseInfo {
    /// 版本号，已去掉 `v` 前缀，例如 `0.9.5`。
    pub(crate) version: String,
    /// 发布页地址，供用户在浏览器里打开。
    pub(crate) url: String,
}

/// 关于窗「检查更新」的状态。只在本次运行中保留，不写入配置。
#[derive(Debug, Clone, Default)]
pub(crate) enum UpdateCheckState {
    /// 还没检查过，或用户重新打开关于窗。
    #[default]
    Idle,
    /// 后台线程正在请求。
    Checking,
    /// 当前已经是最新版本。
    Latest,
    /// 发现比当前版本更新的发布。
    Available(ReleaseInfo),
    /// 请求或解析失败，内容直接给用户看。
    Failed(String),
}

/// 解析 GitHub `releases/latest` 的响应。
///
/// 只认 `tag_name` 与 `html_url` 两个字段；缺任何一个都按畸形响应处理，
/// 免得把空链接或空版本当成「已是最新」。
pub(crate) fn parse_latest_release(json: &str) -> Result<ReleaseInfo> {
    let value: serde_json::Value =
        serde_json::from_str(json).context("更新服务返回的不是有效 JSON")?;
    let tag = value
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .ok_or_else(|| anyhow!("更新服务响应缺少有效的 tag_name 字段"))?;
    let url = value
        .get("html_url")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| anyhow!("更新服务响应缺少有效的 html_url 字段"))?;
    let version = tag
        .strip_prefix('v')
        .or_else(|| tag.strip_prefix('V'))
        .unwrap_or(tag)
        .trim()
        .to_string();
    if version.is_empty() {
        return Err(anyhow!("更新服务返回的版本号为空"));
    }
    Ok(ReleaseInfo {
        version,
        url: url.to_string(),
    })
}

/// 比较两个版本号，返回 `current` 相对 `latest` 的先后。
///
/// 口径：只比较 `major.minor.patch` 三段数字，缺段按 0 补齐；非法数字段按 0
/// 处理，保证异常标签不会让排序崩溃。预发布后缀（`-beta.1`）与构建元数据
/// （`+build`）不参与主版本比较：主版本相同时，带预发布后缀的视为更旧，
/// 两个预发布之间按字典序比较。真正的版本发布不使用预发布标签，这套简化
/// 足够判断「有没有新版本」。
pub(crate) fn compare_versions(current: &str, latest: &str) -> Ordering {
    let current = split_version(current);
    let latest = split_version(latest);
    for index in 0..3 {
        match current.core[index].cmp(&latest.core[index]) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    match (current.pre_release, latest.pre_release) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(left), Some(right)) => left.cmp(right),
    }
}

struct VersionParts<'a> {
    core: [u64; 3],
    pre_release: Option<&'a str>,
}

fn split_version(value: &str) -> VersionParts<'_> {
    let value = value.trim();
    let value = value
        .strip_prefix('v')
        .or_else(|| value.strip_prefix('V'))
        .unwrap_or(value);
    // 构建元数据不参与比较。
    let value = value.split_once('+').map_or(value, |(left, _)| left);
    let (core, pre_release) = match value.split_once('-') {
        Some((core, pre)) if !pre.trim().is_empty() => (core, Some(pre)),
        _ => (value, None),
    };
    let mut numbers = [0u64; 3];
    for (slot, part) in numbers.iter_mut().zip(core.split('.')) {
        *slot = part.trim().parse().unwrap_or(0);
    }
    VersionParts {
        core: numbers,
        pre_release,
    }
}

/// 取回更新服务的响应正文。超时固定为调用方给的值（关于窗用 5 秒）。
pub(crate) fn fetch_latest(url: &str, timeout: Duration) -> Result<String> {
    // 走应用统一的 HTTP 客户端，代理设置（直连 / 跟随系统 / 自定义）才生效。
    let client = crate::net::client(url, timeout.as_secs().max(1))
        .map_err(|error| anyhow!("无法创建更新检查请求：{error:#}"))?;
    let response = client
        .get(url)
        // GitHub API 拒绝没有 User-Agent 的请求。
        .header(
            reqwest::header::USER_AGENT,
            concat!("gongwen-assistant/", env!("CARGO_PKG_VERSION")),
        )
        .send()
        .map_err(|error| {
            if error.is_timeout() {
                anyhow!("请求超时（{} 秒）", timeout.as_secs())
            } else if error.is_connect() {
                anyhow!("连接不上更新服务，请检查网络或更新检查地址")
            } else {
                anyhow!("请求更新服务失败：{error}")
            }
        })?;
    let response = response.error_for_status().map_err(|error| {
        anyhow!(
            "更新服务返回错误状态：{}",
            error
                .status()
                .map(|status| status.to_string())
                .unwrap_or_else(|| error.to_string())
        )
    })?;
    response
        .text()
        .context("读取更新服务响应失败，可能是网络中断")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_normal_release_response() {
        let json = r#"{
            "tag_name": "v0.9.5",
            "html_url": "https://github.com/billowsand/gongwen/releases/tag/v0.9.5",
            "name": "v0.9.5"
        }"#;
        let info = parse_latest_release(json).expect("正常响应应能解析");
        assert_eq!(info.version, "0.9.5");
        assert_eq!(
            info.url,
            "https://github.com/billowsand/gongwen/releases/tag/v0.9.5"
        );
    }

    #[test]
    fn accepts_a_tag_without_the_v_prefix() {
        let json = r#"{"tag_name":"1.2.3","html_url":"https://example.com/1.2.3"}"#;
        assert_eq!(parse_latest_release(json).unwrap().version, "1.2.3");
    }

    #[test]
    fn rejects_a_response_without_tag_name() {
        let json = r#"{"html_url":"https://example.com/release"}"#;
        let error = parse_latest_release(json).expect_err("缺 tag_name 应报错");
        assert!(format!("{error:#}").contains("tag_name"));
    }

    #[test]
    fn rejects_a_response_without_html_url() {
        let json = r#"{"tag_name":"v0.9.5"}"#;
        let error = parse_latest_release(json).expect_err("缺 html_url 应报错");
        assert!(format!("{error:#}").contains("html_url"));
    }

    #[test]
    fn rejects_malformed_json() {
        let error = parse_latest_release("not json").expect_err("畸形 JSON 应报错");
        assert!(format!("{error:#}").contains("JSON"));
    }

    #[test]
    fn compares_major_minor_and_patch() {
        assert_eq!(compare_versions("0.9.5", "1.0.0"), Ordering::Less);
        assert_eq!(compare_versions("1.0.0", "0.9.9"), Ordering::Greater);
        assert_eq!(compare_versions("0.9.5", "0.10.0"), Ordering::Less);
        assert_eq!(compare_versions("0.10.0", "0.9.5"), Ordering::Greater);
        assert_eq!(compare_versions("0.9.5", "0.9.6"), Ordering::Less);
        assert_eq!(compare_versions("0.9.6", "0.9.5"), Ordering::Greater);
        assert_eq!(compare_versions("0.9.5", "0.9.5"), Ordering::Equal);
    }

    #[test]
    fn tolerates_a_leading_v_and_missing_segments() {
        assert_eq!(compare_versions("v1.2.3", "1.2.3"), Ordering::Equal);
        assert_eq!(compare_versions("1.2", "1.2.0"), Ordering::Equal);
        assert_eq!(compare_versions("1", "1.0.1"), Ordering::Less);
    }

    #[test]
    fn treats_pre_releases_as_older_than_the_final_release() {
        assert_eq!(compare_versions("1.2.3-beta.1", "1.2.3"), Ordering::Less);
        assert_eq!(compare_versions("1.2.3", "1.2.3-beta.1"), Ordering::Greater);
        assert_eq!(
            compare_versions("1.2.3-beta.1", "1.2.3-beta.2"),
            Ordering::Less
        );
    }
}
