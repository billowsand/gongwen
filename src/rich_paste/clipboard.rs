//! 读剪贴板里的 HTML。
//!
//! 平台差异由 `arboard` 抹平：Windows 读 `HTML Format` 并只取
//! `StartFragment…EndFragment` 之间那一小段（**`<head>` 里的 `<style>` 拿不到**），
//! macOS 读 `public.html`，Linux 优先走 Wayland data-control，可回落到 X11。所以下面拿到的既可能是完整页面，也可能是
//! 一段片段，解析那一步两种都要认（见 `html.rs`）。
//!
//! 读不到、不是 HTML、超过体积上限都返回 `None`：调用方据此退回纯文本粘贴，
//! 用户的剪贴板内容不会被丢掉。

use crate::file_limits::MAX_CLIPBOARD_HTML_BYTES;

/// 在后台读取同一次外部粘贴的文字及 HTML 副本。
pub(crate) fn read_external() -> Result<super::Source, String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|error| format!("无法访问系统剪贴板：{error}"))?;
    let plain = clipboard.get_text();
    if let Some(html) = clipboard.get().html().ok().and_then(accept) {
        return Ok(super::Source::Html {
            html,
            plain: plain.unwrap_or_default(),
        });
    }
    plain.map(super::Source::Plain).map_err(|error| {
        if matches!(error, arboard::Error::ContentNotAvailable) {
            "系统剪贴板没有可粘贴的文字或富文本。".to_string()
        } else {
            format!("无法读取系统剪贴板：{error}")
        }
    })
}

/// 真正去问操作系统。测试里的采样工具直接用它。
#[cfg(test)]
pub(crate) fn read_platform_html() -> Option<String> {
    let html = arboard::Clipboard::new().ok()?.get().html().ok()?;
    accept(html)
}

/// 体积与空内容过滤。超过上限当没有富文本，退回纯文本。
fn accept(html: String) -> Option<String> {
    if html.len() as u64 > MAX_CLIPBOARD_HTML_BYTES || html.trim().is_empty() {
        return None;
    }
    Some(html)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_and_empty_fragments_count_as_no_html() {
        assert!(accept(String::new()).is_none());
        assert!(accept("  \n".to_string()).is_none());
        assert!(accept("<p>正文</p>".to_string()).is_some());
        let too_large = "x".repeat(MAX_CLIPBOARD_HTML_BYTES as usize + 1);
        assert!(accept(too_large).is_none());
    }
}
