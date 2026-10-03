//! 用研究报告的解析树提取文字，供宿主的字数统计使用。

use crate::common::ast::{Block, Inline, QuoteKind};
use crate::common::inline;

pub(super) fn body_text(markdown: &str) -> String {
    let mut text = String::new();
    for block in crate::parser::parse(markdown) {
        match block {
            Block::Heading { text: heading, .. } => {
                append_inlines(&inline::parse(&heading), &mut text)
            }
            Block::Paragraph(content)
            | Block::List { content, .. }
            | Block::Aligned { content, .. } => {
                // 独占一段的图片会排图题，段内图片没有图题。
                if let [Inline::Image { alt, .. }] = content.as_slice() {
                    text.push_str(alt);
                } else {
                    append_inlines(&content, &mut text);
                }
            }
            Block::Table { rows, caption, .. } => {
                if let Some(caption) = caption {
                    append_inlines(&inline::parse(&caption), &mut text);
                    text.push('\n');
                }
                for cell in rows.iter().flatten() {
                    append_inlines(&inline::parse(cell), &mut text);
                    text.push('\n');
                }
            }
            Block::Quote { kind, items } => {
                if let QuoteKind::Box { name, title } = kind {
                    text.push_str(&name);
                    text.push(' ');
                    append_inlines(&title, &mut text);
                    text.push('\n');
                }
                for item in items {
                    append_inlines(item.inlines(), &mut text);
                    text.push('\n');
                }
            }
            Block::CodeBlock { lang, content } => {
                if lang.as_deref() != Some("mermaid") {
                    text.push_str(&content);
                }
            }
            Block::Math(source) => append_math_text(&source, &mut text),
            Block::Marker(_) | Block::Toc | Block::Label(_) | Block::Unnumbered | Block::Empty => {}
        }
        text.push('\n');
    }
    text
}

fn append_inlines(inlines: &[Inline], text: &mut String) {
    for item in inlines {
        match item {
            Inline::Text(value) | Inline::Code(value) | Inline::Footnote(value) => {
                text.push_str(value)
            }
            Inline::Link { text: value, .. } => text.push_str(value),
            Inline::Bold(items) | Inline::Italic(items) => append_inlines(items, text),
            Inline::Math(source) | Inline::DisplayMath(source) => append_math_text(source, text),
            Inline::Image { .. }
            | Inline::CrossRef(_)
            | Inline::Citation(_)
            | Inline::TextCitation(_) => text.push(' '),
        }
    }
}

/// 公式只提取文字项；变量、数字和 LaTeX 命令不混进正文的词数。
fn append_math_text(source: &str, text: &mut String) {
    // 文字命令内保留完整的中英文；其它公式源码只计中文，不能把重音字母重复计词。
    static HAN: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\p{Han}").unwrap());
    static WORDS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\\(?:text|textrm|textbf|textit)\{([^{}]*)\}").unwrap()
    });
    let mut end = 0;
    for captures in WORDS.captures_iter(source) {
        let range = captures.get(0).unwrap().range();
        for ch in HAN.find_iter(&source[end..range.start]) {
            text.push_str(ch.as_str());
        }
        text.push(' ');
        text.push_str(&captures[1]);
        text.push(' ');
        end = range.end;
    }
    // 中文即使没有包在 \text 中，也会由中文回退字面排出。
    for ch in HAN.find_iter(&source[end..]) {
        text.push_str(ch.as_str());
    }
    text.push(' ');
}
