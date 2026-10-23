//! 粘贴转换的编排：HTML / 纯文本两条路汇到同一个结构还原。
//!
//! ```text
//! HTML：容错解析 → anydoc 文档模型 → 按文种裁剪语法 → Markdown
//! 纯文本：先看是不是已经是 Markdown，不是才往下走
//!        ↓
//! 两者都：按所在章节平移样式标题 → clean_before_structure
//!        → structure::restructure_with_report（粘贴专用选项）→ clean::tidy
//!        → 列表整形
//! ```
//!
//! 任何一步失败都返回 `None`，调用方把事件交还编辑框原样粘贴。纯文本这条路还要
//! 多一道判断：转换结果与原文只差空白时同样返回 `None`——没有任何结构可还原时再走
//! 一遍清洗反而会动用户的字。
//!
//! 粘在句子中间时走 [`convert_inline`]：多行内容压成一行行内文字，不建结构。
//! AI 侧栏的材料走 [`to_material`]：只要 HTML → Markdown 与语法裁剪，不做结构还原。

use crate::doc_import::{clean, structure};

use super::syntax::{self, count_markdown, looks_like_markdown, same_ignoring_whitespace};
use super::{Context, Outcome, Source, Stats, gaps, headings, html, lists};

pub(crate) fn convert(source: &Source, ctx: &Context) -> Option<Outcome> {
    // 复制的本来就是 Markdown（VS Code、Typora 源码模式这类编辑器同时给一份 HTML）：
    // 走 HTML 会把 `## ` 转义、几行并成一段。纯文本像 Markdown 就原样粘贴。
    if looks_like_markdown(source.plain_text()) {
        return None;
    }
    match source {
        Source::Html { html, plain } => convert_html(html, ctx)
            .filter(|outcome| !outcome.markdown.trim().is_empty())
            .or_else(|| convert_plain(plain, ctx)),
        Source::Plain(text) => convert_plain(text, ctx),
    }
}

/// HTML 这条路：富文本里有格式线索，优先它。
fn convert_html(source: &str, ctx: &Context) -> Option<Outcome> {
    let parsed = html::parse(source, ctx.research()).ok()?;
    let document = anydoc::html::to_document(&parsed.body, &parsed.css).ok()?;
    let mut stats = Stats {
        skipped_images: parsed.images,
        boxes: parsed.boxes,
        default_boxes: parsed.default_boxes,
        ..Stats::default()
    };
    let document = syntax::prune(document, ctx.research(), &mut stats);
    let markdown = anydoc::document_to_markdown(&document);
    let (markdown, jumps) = restructure(&markdown, ctx, false);
    if markdown.trim().is_empty() {
        return None;
    }
    stats.number_jumps = jumps;
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
    let (markdown, jumps) = restructure(text, ctx, true);
    if markdown.trim().is_empty() {
        return None;
    }
    // 和原文只差空白：没有结构可还原，别动用户的字。
    if same_ignoring_whitespace(&markdown, text) {
        return None;
    }
    let mut stats = Stats {
        number_jumps: jumps,
        ..Stats::default()
    };
    fill_counts(&markdown, &mut stats);
    Some(Outcome { markdown, stats })
}

/// 粘在句子中间：转成一行行内文字。
pub(crate) fn convert_inline(source: &Source, ctx: &Context) -> Option<Outcome> {
    match source {
        Source::Html { html, plain } => inline_html(html, ctx).or_else(|| inline_plain(plain)),
        Source::Plain(text) => inline_plain(text),
    }
}

fn inline_html(source: &str, ctx: &Context) -> Option<Outcome> {
    let parsed = html::parse(source, false).ok()?;
    let document = anydoc::html::to_document(&parsed.body, &parsed.css).ok()?;
    let mut stats = Stats {
        skipped_images: parsed.images,
        ..Stats::default()
    };
    let document = syntax::prune(document, ctx.research(), &mut stats);
    // 表格没法塞进一行：交给编辑框原样粘贴。
    if syntax::has_table(&document) {
        return None;
    }
    let markdown = inline_text(&html::restore_boxes(&anydoc::document_to_markdown(
        &document,
    )));
    if markdown.is_empty() {
        return None;
    }
    Some(Outcome { markdown, stats })
}

fn inline_plain(text: &str) -> Option<Outcome> {
    if looks_like_markdown(text) {
        return None;
    }
    let markdown = inline_text(text);
    if markdown.is_empty() {
        return None;
    }
    Some(Outcome {
        markdown,
        stats: Stats::default(),
    })
}

/// AI 侧栏的材料：剪贴板 HTML → Markdown。
///
/// 材料要的是**原文**：只做格式转换与语法裁剪（研究报告那套规则即可，材料不分文种），
/// 不做编号重排、不剥版头版记、不认文档标题、不整理列表；图片本版不入库，按未导入跳过。
pub(crate) fn to_material(source: &str) -> Option<String> {
    let parsed = html::parse(source, false).ok()?;
    let document = anydoc::html::to_document(&parsed.body, &parsed.css).ok()?;
    let document = syntax::prune(document, true, &mut Stats::default());
    // `html.rs` 在文本节点里护过字段分隔空格，这里要换回来，否则非字符 U+FDD0 会进材料。
    let markdown = gaps::restore(&html::restore_boxes(&anydoc::document_to_markdown(
        &document,
    )));
    (!markdown.trim().is_empty()).then_some(markdown)
}

/// 大段粘贴作为侧栏材料时的正文：有 HTML 就用它转出来的 Markdown（表格是管道表），
/// 没有或转换失败就用剪贴板里的纯文本。与起草页一致，读失败一律退回原文。
pub(crate) fn material_text(plain: &str) -> String {
    // 复制的本来就是 Markdown（VS Code 一类编辑器同时给一份 HTML）：走 HTML 会转义成 `\## `。
    if looks_like_markdown(plain) {
        return plain.to_string();
    }
    super::clipboard::read_html()
        .and_then(|html| to_material(&html))
        .unwrap_or_else(|| plain.to_string())
}

/// 多行 Markdown / 纯文本压成一行行内文字：去掉标题、列表、引用的记号，
/// **保留**原文编号文字（「一、」这时是正文的一部分）与加粗；段落之间直接相连，
/// 中文不加空格、西文之间一个空格。
fn inline_text(text: &str) -> String {
    let mut pieces: Vec<String> = Vec::new();
    let mut fenced = false;
    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        let line = strip_inline_marker(trimmed).trim();
        if line.is_empty() {
            continue;
        }
        pieces.push(line.to_string());
    }
    // 字段之间的分隔空格同样护住再换回来：HTML 那条路的文字里已经带着占位符，
    // 不换回来就会把非字符 U+FDD0 写进正文。
    let joined = gaps::protect(&join_inline(&pieces));
    gaps::restore(clean::normalize_spaces(&joined).trim())
}

/// 去掉标题（`#`）、无序列表（`- `）与引用（`> `）的记号；有序列表开头的数字就是
/// 原文编号，留着。
fn strip_inline_marker(line: &str) -> &str {
    if let Some(rest) = line.strip_prefix("> ") {
        return strip_inline_marker(rest);
    }
    // 文框首行（`[!专栏] 标题`）：行内只要标题。
    if let Some(rest) = line.strip_prefix("[!")
        && let Some(end) = rest.find(']')
    {
        return rest[end + 1..].trim_start();
    }
    let hashes = line.bytes().take_while(|&byte| byte == b'#').count();
    if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
        return &line[hashes..];
    }
    if let Some(rest) = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))
    {
        return rest;
    }
    if line.starts_with("<!--") && line.ends_with("-->") {
        return "";
    }
    line
}

fn join_inline(pieces: &[String]) -> String {
    let mut out = String::new();
    for piece in pieces {
        let needs_space = out
            .chars()
            .last()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
            && piece.starts_with(|ch: char| ch.is_ascii_alphanumeric());
        if needs_space {
            out.push(' ');
        }
        out.push_str(piece);
    }
    out
}

/// 共用的结构还原：平移样式标题 → 清洗 → 按公文版式还原标题与列表 → 收拾空行 →
/// 列表整形。
///
/// 字段之间的分隔空格（「张某　联系电话：」）先护住、最后换回来，见 `gaps`。HTML 那条
/// 路在 `html.rs` 逐个文本节点护过了，这里再护一遍对它没有影响。
///
/// 返回整理好的 Markdown 与原文编号跳号（原文编号对）。
fn restructure(
    markdown: &str,
    ctx: &Context,
    lines_are_paragraphs: bool,
) -> (String, Vec<(String, String)>) {
    let shifted = headings::shift(markdown, ctx);
    let protected = gaps::protect(&shifted);
    let cleaned = clean::clean_before_structure(&protected);
    let options = structure::Options::paste(!ctx.has_title, lines_are_paragraphs, ctx.draft_empty);
    let (restructured, report) = structure::restructure_with_report(&cleaned, &options);
    let shaped = lists::shape(&clean::tidy(&restructured), ctx.kind);
    let jumps = report
        .jumps()
        .iter()
        .map(|jump| (jump.from.clone(), jump.to.clone()))
        .collect();
    // 文框首行的占位留到最后：清洗会把 `[!案例]` 的半角感叹号转成全角。
    (html::restore_boxes(&gaps::restore(&shaped)), jumps)
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
    use crate::models::TemplateKind;

    fn context() -> Context {
        Context {
            kind: TemplateKind::OfficialLetter,
            draft_empty: true,
            has_title: false,
            section_level: None,
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

    #[test]
    fn dotted_headings_keep_their_levels() {
        let markdown = html(
            "<html><body><h2>1 总体要求</h2><p>正文。</p>\
             <h2>1.1 指导思想</h2><p>正文。</p>\
             <h2>1.1.1 基本原则</h2><p>正文。</p></body></html>",
        );
        assert!(markdown.contains("## 总体要求"), "{markdown}");
        assert!(markdown.contains("### 指导思想"), "{markdown}");
        assert!(markdown.contains("#### 基本原则"), "{markdown}");
    }

    #[test]
    fn number_jumps_are_reported_in_the_status() {
        let outcome = convert(
            &Source::Plain("一、总体要求\n正文。\n三、主要任务\n正文。".to_string()),
            &context(),
        )
        .expect("纯文本应被识别");
        assert_eq!(
            outcome.stats.number_jumps,
            [("一、".to_string(), "三、".to_string())]
        );
        let notice = outcome.stats.notice();
        assert!(notice.contains("原文编号有跳号（一、→三、）"), "{notice}");
    }

    fn research_context() -> Context {
        Context {
            kind: TemplateKind::ResearchReport,
            draft_empty: true,
            has_title: false,
            section_level: None,
        }
    }

    #[test]
    fn research_boxes_default_to_column_name_and_warn() {
        let outcome = convert(
            &Source::Html {
                html: "<html><body><aside><p><b>某市做法</b></p>\
                       <p>先把台账建起来，再按季度核对。</p></aside></body></html>"
                    .to_string(),
                plain: String::new(),
            },
            &research_context(),
        )
        .expect("研究报告应能转换");
        assert!(
            outcome.markdown.contains("> [!专栏] 某市做法"),
            "{}",
            outcome.markdown
        );
        assert_eq!(outcome.stats.boxes, 1);
        assert_eq!(outcome.stats.default_boxes, 1);
        assert!(
            outcome
                .stats
                .notice()
                .contains("识别出 1 个文框，名称暂写「专栏」"),
            "{}",
            outcome.stats.notice()
        );

        // 名称从标题里认出来的，不再提示改名称。
        let named = convert(
            &Source::Html {
                html: "<html><body><aside><p><b>案例2：某县做法</b></p>\
                       <p>各县可结合实际借鉴。</p></aside></body></html>"
                    .to_string(),
                plain: String::new(),
            },
            &research_context(),
        )
        .expect("研究报告应能转换");
        assert!(
            named.markdown.contains("> [!案例] 某县做法"),
            "{}",
            named.markdown
        );
        assert_eq!(named.stats.default_boxes, 0);
        assert!(
            !named.stats.notice().contains("识别出"),
            "{}",
            named.stats.notice()
        );
    }

    #[test]
    fn non_research_documents_get_no_boxes() {
        let outcome = convert(
            &Source::Html {
                html: "<html><body><aside><p><b>某市做法</b></p>\
                       <p>先把台账建起来，再按季度核对。</p></aside></body></html>"
                    .to_string(),
                plain: String::new(),
            },
            &context(),
        )
        .expect("公文应能转换");
        assert!(!outcome.markdown.contains("[!"), "{}", outcome.markdown);
        assert_eq!(outcome.stats.boxes, 0);
    }

    #[test]
    fn material_keeps_tables_and_leaves_numbering_alone() {
        let markdown = to_material(
            "<html><body><h3>一、基本情况</h3><p>说明。</p>\
             <table><tr><th>序号</th><th>事项</th></tr>\
             <tr><td>1</td><td>隐患排查</td></tr></table></body></html>",
        )
        .expect("材料应能转换");
        assert!(markdown.contains("| 序号 | 事项 |"), "{markdown}");
        // 材料是原文：不把「一、」升成标题。
        assert!(markdown.contains("一、基本情况"), "{markdown}");
        assert!(!markdown.contains("## 基本情况"), "{markdown}");
    }

    #[test]
    fn material_text_falls_back_to_the_plain_clipboard() {
        crate::rich_paste::clipboard::set_test_html(None);
        assert_eq!(material_text("原文"), "原文");
        crate::rich_paste::clipboard::set_test_html(Some(
            "<html><body><table><tr><th>甲</th><th>乙</th></tr>\
             <tr><td>1</td><td>2</td></tr></table></body></html>",
        ));
        let text = material_text("甲\t乙");
        assert!(text.contains("| 甲 | 乙 |"), "{text}");
        crate::rich_paste::clipboard::clear_test_html();
    }

    /// 材料里不能有占位符，复制的现成 Markdown 不能被转义。
    #[test]
    fn material_keeps_field_gaps_and_verbatim_markdown() {
        let markdown =
            to_material("<p>联系人：张某　联系电话：123</p><p>二段</p>").expect("材料应能转换");
        assert!(!markdown.contains('\u{FDD0}'), "{markdown:?}");
        assert!(
            markdown.contains("联系人：张某　联系电话：123"),
            "{markdown}"
        );

        crate::rich_paste::clipboard::set_test_html(Some(
            "<div><div><span>## 标题</span></div><div><span>正文**加粗**</span></div></div>",
        ));
        assert_eq!(
            material_text("## 标题\n正文**加粗**"),
            "## 标题\n正文**加粗**"
        );
        crate::rich_paste::clipboard::clear_test_html();
    }

    /// 微信公众号白底 `section` 里的「一、总体要求」是章节，不是文框。
    #[test]
    fn white_sections_with_numbered_titles_are_not_boxes() {
        let outcome = convert(
            &Source::Html {
                html: "<section style=\"background-color: rgb(255, 255, 255)\">\
                       <p><strong>一、总体要求</strong></p><p>各单位细化措施。</p></section>\
                       <div style=\"background: #f2f2f2\">\
                       <p><strong>（一）指导思想</strong></p><p>坚持问题导向。</p></div>"
                    .to_string(),
                plain: String::new(),
            },
            &research_context(),
        )
        .expect("研究报告应能转换");
        assert!(!outcome.markdown.contains("[!"), "{}", outcome.markdown);
        assert!(
            outcome.markdown.contains("## 总体要求"),
            "{}",
            outcome.markdown
        );
        assert_eq!(outcome.stats.boxes, 0);
    }

    #[test]
    fn inline_conversion_flattens_markup_but_keeps_numbering() {
        let outcome = convert_inline(
            &Source::Html {
                html: "<html><body><h2>一、总体要求</h2><p>各单位加强巡查。</p>\
                       <ul><li>第一项</li><li>第二项</li></ul></body></html>"
                    .to_string(),
                plain: String::new(),
            },
            &context(),
        )
        .expect("行内内容应能转换");
        assert_eq!(outcome.markdown, "一、总体要求各单位加强巡查。第一项第二项");
        assert!(
            outcome
                .stats
                .inline_notice()
                .starts_with("已按行内格式粘贴")
        );
    }

    #[test]
    fn inline_conversion_gives_up_on_tables() {
        assert!(
            convert_inline(
                &Source::Html {
                    html: "<html><body><p>甲</p>\
                           <table><tr><th>序号</th><th>事项</th></tr>\
                           <tr><td>1</td><td>报送</td></tr></table></body></html>"
                        .to_string(),
                    plain: String::new(),
                },
                &context(),
            )
            .is_none()
        );
    }

    #[test]
    fn inline_plain_text_joins_lines() {
        let outcome = convert_inline(
            &Source::Plain("各有关单位：\n一、总体要求\n（一）指导思想".to_string()),
            &context(),
        )
        .expect("多行纯文本应能压成一行");
        assert_eq!(outcome.markdown, "各有关单位：一、总体要求（一）指导思想");
    }

    /// 行内粘贴也要保住字段分隔空格，而且占位符不能漏进正文。
    #[test]
    fn inline_conversion_keeps_field_gaps_without_placeholders() {
        for source in [
            Source::Html {
                html: "<p>联系人：张某　联系电话：123</p><p>地址：某路</p>".to_string(),
                plain: String::new(),
            },
            Source::Plain("联系人：张某　联系电话：123\n地址：某路".to_string()),
        ] {
            let markdown = convert_inline(&source, &context())
                .expect("多行内容应能压成一行")
                .markdown;
            assert!(!markdown.contains('\u{FDD0}'), "{markdown:?}");
            assert!(
                markdown.starts_with("联系人：张某　联系电话：123"),
                "{markdown}"
            );
        }
    }
}
