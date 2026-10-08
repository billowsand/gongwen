//! 审校共用的正文与标题边界；屏蔽保持字节长度，提示仍能定位到源码。

use crate::export::{self, MarkdownBlock, MarkdownSection};
use std::ops::Range;

pub(super) const DOCUMENT_KINDS: &[&str] = &[
    "决议", "决定", "命令", "公报", "公告", "通告", "意见", "通知", "通报", "报告", "请示", "批复",
    "议案", "函", "纪要", "方案",
];

pub(super) fn claimed_kind(title: &str) -> Option<&'static str> {
    DOCUMENT_KINDS
        .iter()
        .copied()
        .filter(|suffix| title.trim().ends_with(suffix))
        .max_by_key(|suffix| suffix.len())
}

/// 不取附件、块引文和代码中的一级标题；正文标记可以切回正文。
pub(crate) fn main_title(markdown: &str) -> Option<(String, Range<usize>)> {
    let mut in_body = true;
    for located in export::parse_markdown_located(markdown) {
        match located.block {
            MarkdownBlock::Marker(section) => in_body = section == MarkdownSection::Body,
            MarkdownBlock::Title(text) if in_body => {
                return Some((text, located.range));
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn declared_kind(markdown: &str) -> Option<&'static str> {
    main_title(markdown).and_then(|(title, _)| claimed_kind(&title))
}

fn blank(bytes: &mut [u8], range: Range<usize>) {
    for byte in &mut bytes[range] {
        if !matches!(*byte, b'\n' | b'\r') {
            *byte = b' ';
        }
    }
}

/// 标题、表格、块引文、附件和非正文区均不参与本稿的语句判断。
/// 单/双中文引号、日文引号内的直接引文同样保护；不屏蔽整段普通叙述。
pub(crate) fn body_text(markdown: &str) -> String {
    let mut bytes = markdown.as_bytes().to_vec();
    let mut in_body = true;
    for located in export::parse_markdown_located(markdown) {
        let keep = match located.block {
            MarkdownBlock::Marker(section) => {
                in_body = section == MarkdownSection::Body;
                false
            }
            MarkdownBlock::Paragraph(_) | MarkdownBlock::OrderedListItem { .. } => in_body,
            _ => false,
        };
        if !keep {
            blank(&mut bytes, located.range);
        }
    }
    // 研究报告扩展区段也不应进入公文正文检查，遇到 [正文] 才重新进入。
    let mut offset = 0;
    in_body = true;
    for line in markdown.split_inclusive('\n') {
        if let Some(section) = export::parse_research_marker(line.trim()) {
            in_body = matches!(
                section,
                export::ResearchSection::Body | export::ResearchSection::Part
            );
        }
        if !in_body {
            blank(&mut bytes, offset..offset + line.len());
        }
        offset += line.len();
    }
    // 保持掩码的 UTF-8 合法性：每个被保护字符的全部字节一起换为空格。
    let mut stack: Vec<(char, usize)> = Vec::new();
    let text = String::from_utf8(bytes).expect("整块屏蔽保留 UTF-8");
    let mut bytes = text.as_bytes().to_vec();
    for (index, ch) in text.char_indices() {
        let closing = match ch {
            '“' => Some('”'),
            '‘' => Some('’'),
            '「' => Some('」'),
            '『' => Some('』'),
            _ => None,
        };
        if let Some(closing) = closing {
            stack.push((closing, index));
        } else if stack.last().is_some_and(|(closing, _)| *closing == ch) {
            let (_, start) = stack.pop().expect("有对应开引号");
            blank(&mut bytes, start..index + ch.len_utf8());
        }
    }
    String::from_utf8(bytes).expect("引文屏蔽保留 UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_preserve_offsets_and_resume_after_attachments() {
        let text = "# 测试报告\n\n主文。\n\n> 请予批复。\n\n<!-- [附件] -->\n\n# 附件请示\n\n附件。\n\n<!-- [正文] -->\n\n续写正文。\n";
        let body = body_text(text);
        assert_eq!(body.len(), text.len());
        assert!(!body.contains("请予批复"));
        assert!(!body.contains("附件。"));
        assert!(body.contains("主文。"));
        assert!(body.contains("续写正文。"));
        assert_eq!(declared_kind(text), Some("报告"));
        let offset = text.find("续写正文").unwrap();
        assert_eq!(&body[offset..offset + "续写正文".len()], "续写正文");
    }

    #[test]
    fn an_attachment_title_cannot_determine_the_document_kind() {
        assert_eq!(declared_kind("<!-- [附件] -->\n\n# 附件请示"), None);
        assert_eq!(declared_kind("> # 引文请示\n\n# 情况报告   "), Some("报告"));
    }
}
