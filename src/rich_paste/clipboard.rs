//! 读剪贴板里的 HTML。
//!
//! 平台差异由 `arboard` 抹平：Windows 读 `HTML Format` 并只取
//! `StartFragment…EndFragment` 之间那一小段（**`<head>` 里的 `<style>` 拿不到**），
//! macOS 读 `public.html`，Linux 走 X11。所以下面拿到的既可能是完整页面，也可能是
//! 一段片段，解析那一步两种都要认（见 `html.rs`）。
//!
//! 读不到、不是 HTML、超过体积上限都返回 `None`：调用方据此退回纯文本粘贴，
//! 用户的剪贴板内容不会被丢掉。

use crate::file_limits::MAX_CLIPBOARD_HTML_BYTES;

/// 读当前剪贴板的 HTML；没有就返回 `None`。
pub(crate) fn read_html() -> Option<String> {
    #[cfg(test)]
    if let Some(fake) = test_html() {
        return fake;
    }
    read_platform_html()
}

/// 真正去问操作系统。测试里的采样工具直接用它（绕开线程内的替身）。
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
thread_local! {
    /// 测试替身。外层 `None` 表示没设置过、去问系统剪贴板；`Some(None)` 表示
    /// 明确「剪贴板里没有 HTML」。按线程存放，并行跑的测试互不影响。
    static TEST_HTML: std::cell::RefCell<Option<Option<String>>> =
        const { std::cell::RefCell::new(None) };
}

/// 测试用：让本线程的 [`read_html`] 返回固定内容（`None` 表示没有 HTML）。
#[cfg(test)]
pub(crate) fn set_test_html(html: Option<&str>) {
    TEST_HTML.with(|slot| *slot.borrow_mut() = Some(html.map(str::to_owned)));
}

/// 测试用：撤掉替身，恢复去问系统剪贴板。
#[cfg(test)]
pub(crate) fn clear_test_html() {
    TEST_HTML.with(|slot| *slot.borrow_mut() = None);
}

#[cfg(test)]
fn test_html() -> Option<Option<String>> {
    TEST_HTML.with(|slot| slot.borrow().clone())
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

    #[test]
    fn thread_local_override_is_what_read_html_returns() {
        // 先设一个假的，确认替身生效；再撤掉，确保状态能清干净。
        set_test_html(Some("<p>材料</p>"));
        assert_eq!(read_html().as_deref(), Some("<p>材料</p>"));
        set_test_html(None);
        assert!(read_html().is_none());
        clear_test_html();
    }
}
