//! 粘贴转换的编排：HTML / 纯文本两条路汇到同一个结构还原。
//!
//! ```text
//! HTML：容错解析 → anydoc 文档模型 → 按文种裁剪语法 → Markdown
//! 纯文本：先看是不是已经是 Markdown，不是才往下走
//!        ↓
//! 两者都：clean_before_structure → structure::restructure（粘贴专用选项）→ clean::tidy
//! ```
//!
//! 任何一步失败都返回 `None`，调用方把事件交还编辑框原样粘贴。纯文本这条路还要
//! 多一道判断：转换结果与原文只差空白时同样返回 `None`——没有任何结构可还原时再走
//! 一遍清洗反而会动用户的字。

use crate::doc_import::{clean, structure};

use super::syntax::{self, count_markdown, looks_like_markdown, same_ignoring_whitespace};
use super::{Context, Outcome, Source, Stats, html};

pub(crate) fn convert(source: &Source, ctx: &Context) -> Option<Outcome> {
    match source {
        Source::Html { html, plain } => convert_html(html, ctx)
            .filter(|outcome| !outcome.markdown.trim().is_empty())
            .or_else(|| convert_plain(plain, ctx)),
        Source::Plain(text) => convert_plain(text, ctx),
    }
}

/// HTML 这条路：富文本里有格式线索，优先它。
fn convert_html(source: &str, ctx: &Context) -> Option<Outcome> {
    let parsed = html::parse(source).ok()?;
    let document = anydoc::html::to_document(&parsed.body, &parsed.css).ok()?;
    let mut stats = Stats {
        skipped_images: parsed.images,
        ..Stats::default()
    };
    let document = syntax::prune(document, ctx.research, &mut stats);
    let markdown = anydoc::document_to_markdown(&document);
    let markdown = restructure(&markdown, ctx, false);
    if markdown.trim().is_empty() {
        return None;
    }
    fill_counts(&markdown, &mut stats);
    Some(Outcome { markdown, stats })
}

/// 纯文本这条路：一行一段，靠编号定级（微信、记事本里复制的文字只有纯文本）。
fn convert_plain(text: &str, ctx: &Context) -> Option<Outcome> {
    // 已经是 Markdown 的内容原样粘贴：用户从别处复制现成的 Markdown 时，
    // 再走一遍清洗只会动他的结构。
    if looks_like_markdown(text) {
        return None;
    }
    let markdown = restructure(text, ctx, true);
    if markdown.trim().is_empty() {
        return None;
    }
    // 和原文只差空白：没有结构可还原，别动用户的字。
    if same_ignoring_whitespace(&markdown, text) {
        return None;
    }
    let mut stats = Stats::default();
    fill_counts(&markdown, &mut stats);
    Some(Outcome { markdown, stats })
}

/// 共用的结构还原：清洗 → 按公文版式还原标题与列表 → 收拾空行。
fn restructure(markdown: &str, ctx: &Context, lines_are_paragraphs: bool) -> String {
    let cleaned = clean::clean_before_structure(markdown);
    let options = structure::Options::paste(!ctx.has_title, lines_are_paragraphs, ctx.draft_empty);
    clean::tidy(&structure::restructure(&cleaned, &options))
}

/// 标题与列表项按最终结果数：结构还原会把「一、」这类编号段落升成标题，
/// 数模型里的块会数漏。
fn fill_counts(markdown: &str, stats: &mut Stats) {
    let (headings, list_items) = count_markdown(markdown);
    stats.headings = headings;
    stats.list_items = list_items;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> Context {
        Context {
            research: false,
            draft_empty: true,
            has_title: false,
        }
    }

    fn html(markup: &str) -> String {
        convert(
            &Source::Html {
                html: markup.to_string(),
                plain: String::new(),
            },
            &context(),
        )
        .expect("HTML 应能转换")
        .markdown
    }

    fn plain(text: &str) -> Option<String> {
        convert(&Source::Plain(text.to_string()), &context()).map(|outcome| outcome.markdown)
    }

    #[test]
    fn plain_text_numbering_becomes_headings() {
        let text = "一、总体要求\n（一）指导思想\n以高水平安全服务高质量发展。\n二、主要任务\n（一）加强巡查\n各单位要压实责任。\n";
        let markdown = plain(text).expect("多行纯文本应被识别");
        assert!(markdown.contains("## 总体要求"), "{markdown}");
        assert!(markdown.contains("### 指导思想"), "{markdown}");
        assert!(markdown.contains("## 主要任务"), "{markdown}");
        assert!(markdown.contains("### 加强巡查"), "{markdown}");
        // 编号由程序重新生成，正文里不留手打编号。
        assert!(!markdown.contains("一、总体要求"), "{markdown}");
    }

    #[test]
    fn markdown_already_written_is_left_alone() {
        assert!(plain("# 关于X的通知\n\n各单位加强巡查。").is_none());
        assert!(plain("正文\n\n| 序号 | 事项 |\n| --- | --- |\n| 1 | 报送 |").is_none());
        assert!(plain("<!-- [附件] -->\n\n附件：清单").is_none());
        assert!(plain("```\n代码\n```").is_none());
    }

    #[test]
    fn plain_text_differing_only_in_whitespace_is_left_alone() {
        // 一行一段、本来就有空行、没有编号：清洗后只差空白。
        assert!(plain("各单位加强巡查。\n\n确保安全。").is_none());
        assert!(plain("各单位加强巡查。 \n确保安全。  ").is_none());
    }

    #[test]
    fn html_that_carries_no_text_falls_back_to_the_plain_clipboard() {
        let outcome = convert(
            &Source::Html {
                html: "<p></p><script>var a = 1</script>".to_string(),
                plain: "一、总体要求\n（一）指导思想\n各单位加强巡查。\n".to_string(),
            },
            &context(),
        )
        .expect("HTML 没内容时用纯文本");
        assert!(
            outcome.markdown.contains("## 总体要求"),
            "{}",
            outcome.markdown
        );
    }

    #[test]
    fn html_pipeline_keeps_tables_and_bold() {
        let markdown = html(
            "<html><body><h1>关于X的通知</h1><p><strong>一、总体要求</strong></p>\
             <p>各单位加强巡查。</p>\
             <table><tr><th>序号</th><th>事项</th></tr><tr><td>1</td><td>报送</td></tr></table>\
             </body></html>",
        );
        assert!(markdown.contains("# 关于X的通知"), "{markdown}");
        assert!(markdown.contains("## 总体要求"), "{markdown}");
        assert!(markdown.contains("| 序号 | 事项 |"), "{markdown}");
    }
}
