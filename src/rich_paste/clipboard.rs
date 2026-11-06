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
///
/// 测试里**默认不碰系统剪贴板**：宿主机器上碰巧复制了富文本时，依赖粘贴的 UI
/// 测试就会读到它，结果随开发机状态时灵时不灵（真实剪贴板是进程外的全局状态，
/// 测试之间也没法互不干扰）。要 HTML 的测试必须显式 [`set_test_html`] 替身；
/// 没设置过与 `set_test_html(None)` 一样返回 `None`。
pub(crate) fn read_html() -> Option<String> {
    #[cfg(test)]
    {
        test_html().flatten()
    }
    #[cfg(not(test))]
    {
        read_platform_html()
    }
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
    /// 测试替身。外层 `None` 表示没设置过（等同「剪贴板里没有 HTML」，**不读系统
    /// 剪贴板**）；`Some(None)` 是显式声明没有 HTML；`Some(Some(..))` 是给的假内容。
    /// 按线程存放，并行跑的测试互不影响。
    static TEST_HTML: std::cell::RefCell<Option<Option<String>>> =
        const { std::cell::RefCell::new(None) };
}

/// 测试用：让本线程的 [`read_html`] 返回固定内容（`None` 表示没有 HTML）。
#[cfg(test)]
pub(crate) fn set_test_html(html: Option<&str>) {
    TEST_HTML.with(|slot| *slot.borrow_mut() = Some(html.map(str::to_owned)));
}

/// 测试用：撤掉替身，本线程的 [`read_html`] 回到「没有 HTML」。
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

    /// 测试里 `read_html()` 只认替身：宿主机剪贴板里有什么都不该影响测试结果。
    #[test]
    fn read_html_never_reads_the_system_clipboard_in_tests() {
        // 没设置过替身：直接 None，不去问 arboard（测试进程没有可依赖的剪贴板）。
        clear_test_html();
        assert!(
            read_html().is_none(),
            "未设置替身时 read_html 不能去读系统剪贴板"
        );
        // `set_test_html(None)` 是显式「没有 HTML」，语义同上。
        set_test_html(None);
        assert!(read_html().is_none());
        clear_test_html();
    }

    #[test]
    fn thread_local_override_is_what_read_html_returns() {
        // 设置假的就返回假的；撤掉后回到 None（不会回落去读系统剪贴板）。
        set_test_html(Some("<p>材料</p>"));
        assert_eq!(read_html().as_deref(), Some("<p>材料</p>"));
        set_test_html(None);
        assert!(read_html().is_none());
        clear_test_html();
        assert!(read_html().is_none(), "撤掉替身后不应再返回内容");
    }
}
