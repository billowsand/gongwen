//! 附件与标题：附件横排标记、标题层级与编号。
//!
//! 由 src/export/latex.rs 拆分而来：本文件是模块 `export::latex::attachments`，与其它子模块共享
//! `export::latex` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::export::latex::{marked_heading_tex, marked_tex_escape};
use crate::export::table::requires_landscape;
use crate::export::title::{self, TitlePlan};
use crate::export::{MarkdownBlock, MarkdownSection, official_heading_prefix};
use crate::models::NumberingConfig;

/// 每个附件只要有一张表在竖页中横向过密，就将整个附件（而非仅表格）改为横页。
pub(crate) fn attachment_landscape_flags(blocks: &[MarkdownBlock]) -> Vec<bool> {
    let mut flags = Vec::new();
    let mut seen_document_title = false;
    let mut current_attachment = None;

    for block in blocks {
        match block {
            MarkdownBlock::Title(_) if !seen_document_title && current_attachment.is_none() => {
                seen_document_title = true
            }
            MarkdownBlock::Marker(MarkdownSection::Attachment) => {
                flags.push(false);
                current_attachment = Some(flags.len() - 1);
            }
            MarkdownBlock::Marker(MarkdownSection::Body) => current_attachment = None,
            MarkdownBlock::Table { rows, spans, .. } => {
                if let Some(index) = current_attachment
                    && requires_landscape(rows, spans)
                {
                    flags[index] = true;
                }
            }
            _ => {}
        }
    }
    flags
}

pub(crate) fn attachment_document_title_to_tex(text: &str) -> String {
    // 附件标识位于第一行，正式标题置于第三行；用固定正文行距留出第二行。
    // 标题改动也就地标注（花脸稿哨兵在解析后注入，这里走标注感知的转义）。
    // 排布与主标题同一套：多出 1–2 字横向压缩，再多按分词均衡换行。
    let plain = crate::export::plain_text(text);
    let body = match title::title_plan(&plain, title::chars_per_line()) {
        TitlePlan::SingleLine => marked_tex_escape(text),
        TitlePlan::Compressed => format!(
            "\\scalebox{{{}}}[1]{{{}}}",
            title::compressed_scale_percent(&plain) as f64 / 100.0,
            marked_tex_escape(text)
        ),
        TitlePlan::Wrapped(lines) => crate::export::redline_slice_lines(text, &lines)
            .iter()
            .map(|line| marked_tex_escape(line))
            .collect::<Vec<_>>()
            .join("\\\\"),
    };
    format!(
        "\\vspace{{\\BodyBaselineSkip}}\n{{\\centering\\bs\\enbt\\zihao{{2}}\\setlength{{\\baselineskip}}{{\\BodyBaselineSkip}} {body}\\par}}"
    )
}

pub(crate) fn target_tex_section<'a>(
    section: MarkdownSection,
    body: &'a mut Vec<String>,
    attachments: &'a mut Vec<String>,
) -> &'a mut Vec<String> {
    match section {
        MarkdownSection::Body => body,
        MarkdownSection::Attachment => attachments,
    }
}

pub(crate) fn official_heading_to_tex(
    level: u8,
    text: &str,
    counters: &mut [usize; 4],
    numbering: &NumberingConfig,
) -> Option<String> {
    // 标题文字走标注感知的转义：花脸稿里删除 / 新增按块注宏；编号前缀
    // 经 marked_heading_tex——新增标题整体加框时编号并进框（规则 8）。
    let number = official_heading_prefix(level, counters, numbering)?;
    let escaped = marked_heading_tex(&number, text);
    let rendered = match level {
        2 => format!("\\noindent\\hspace*{{2em}}{{\\heiti\\enheiti {escaped}}}\\par"),
        3 => format!("\\noindent\\hspace*{{2em}}{{\\kai\\enkai {escaped}}}\\par"),
        4 | 5 => format!("\\noindent\\hspace*{{2em}}{{\\fs\\textbf{{{escaped}}}}}\\par"),
        _ => return None,
    };
    Some(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_title_wraps_at_words_like_the_main_title() {
        let tex = attachment_document_title_to_tex(
            "关于进一步加强全市基层治理体系和治理能力现代化建设的实施方案附件材料汇编",
        );
        assert!(tex.contains("\\\\"), "长标题应分行：{tex}");
    }

    #[test]
    fn attachment_title_compresses_small_overflow() {
        // 一行 20 字宽，21 个汉字只超一字。
        let tex = attachment_document_title_to_tex("一二三四五六七八九十一二三四五六七八九十一");
        assert!(tex.contains("\\scalebox"), "{tex}");
        assert!(!tex.contains("\\\\"), "{tex}");
    }
}
