//! 标题计数：标题层级编号、正文标题与居中标记。
//!
//! 由 src/export/mod.rs 拆分而来：本文件是模块 `export::headings`，与其它子模块共享
//! `export` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use std::sync::{OnceLock};
use regex::Regex;
use crate::export::{MarkdownSection, parse_section_marker, number_to_chinese, legacy_attachment_label};

/// 与 mdx 公文转换保持一致：标题编号由导出器统一生成，先清掉模型或人工写入的旧编号。
pub(crate) fn clean_heading_number(text: &str) -> String {
    const PATTERNS: &[&str] = &[
        r"^附录\s*[A-Za-z0-9]+(?:[.\-][A-Za-z0-9]+)*\s*[、.．:：]?\s*",
        r"^(?i:appendix)\s*[A-Za-z0-9]+(?:[.\-][A-Za-z0-9]+)*\s*[、.．:：]?\s*",
        r"^第[一二三四五六七八九十百零\d]+[章节条部分]\s*[、.．]?\s*",
        r"^[（(][一二三四五六七八九十百零]+[）)]\s*[、.．]?\s*",
        r"^[一二三四五六七八九十百零]+[、,.．]\s*",
        r"^[（(]\d+[）)]\s*[、.．]?\s*",
        r"^\(\d+\)\s*[、.．]?\s*",
        r"^\d+(?:\.\d+)+[.．]?\s*",
        r"^\d+[.．、]\s+",
        r"^\d+\s+",
    ];
    static REGEXES: OnceLock<Vec<Regex>> = OnceLock::new();
    let mut cleaned = text.to_string();
    for regex in REGEXES.get_or_init(|| {
        PATTERNS
            .iter()
            .map(|pattern| Regex::new(pattern).expect("valid heading pattern"))
            .collect()
    }) {
        cleaned = regex.replace(&cleaned, "").to_string();
    }
    cleaned.trim().to_string()
}

/// 公文正文各级标题的编号：一、 →（一）→ 1. →（1）。DOCX 导出与界面预览共用，
/// 保证预览里看到的编号就是导出后的编号。
pub(crate) fn official_heading_text(
    level: u8,
    text: &str,
    counters: &mut [usize; 4],
) -> Option<String> {
    official_heading_prefix(level, counters).map(|prefix| format!("{prefix}{text}"))
}

/// 只生成公文标题编号前缀。实时排版编辑器不能把自动编号真正写进
/// Markdown，因此用这个共用函数在屏幕上叠加，导出时仍由同一套计数器生成。
pub(crate) fn official_heading_prefix(level: u8, counters: &mut [usize; 4]) -> Option<String> {
    match level {
        2 => {
            counters[0] += 1;
            counters[1..].fill(0);
            Some(format!("{}、", number_to_chinese(counters[0])))
        }
        3 => {
            counters[1] += 1;
            counters[2..].fill(0);
            Some(format!("（{}）", number_to_chinese(counters[1])))
        }
        4 => {
            counters[2] += 1;
            counters[3] = 0;
            Some(format!("{}.", counters[2]))
        }
        5 => {
            counters[3] += 1;
            Some(format!("({})", counters[3]))
        }
        _ => None,
    }
}

/// 逐行推进标题计数器并返回该行叠加的编号前缀，供实时排版编辑器使用。
/// 规则与导出器（docx/latex）和预览完全一致：
/// - 区段标记（正文/附件）处切换区段并重置计数器；
/// - 正文第一个 `#` 与每个附件标记后的 `#` 都是正式标题；
/// - 正文和附件的 `##` 及以下使用完全相同的编号层级；
/// - 非标题行返回 None 且不推进计数器。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HeadingCounters {
    levels: [usize; 4],
    expecting_title: bool,
    in_attachment: bool,
    legacy_attachment: bool,
    /// 最近一次 `next` 处理的行是否为正式标题（文档标题或附件正式标题），
    /// 需要按方正小标宋二号居中渲染，与预览/导出一致。
    centered_title: bool,
}

impl Default for HeadingCounters {

    fn default() -> Self {
        Self {
            levels: [0; 4],
            expecting_title: true,
            in_attachment: false,
            legacy_attachment: false,
            centered_title: false,
        }
    }
}

impl HeadingCounters {

    pub(crate) fn next(&mut self, line: &str) -> Option<String> {
        self.centered_title = false;
        if let Some(section) = parse_section_marker(line) {
            self.levels = [0; 4];
            self.expecting_title = true;
            self.in_attachment = section == MarkdownSection::Attachment;
            self.legacy_attachment = false;
            return None;
        }
        let trimmed = line.trim_start();
        let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
        let is_heading = (1..=6).contains(&hashes)
            && trimmed
                .as_bytes()
                .get(hashes)
                .is_some_and(u8::is_ascii_whitespace);
        if !is_heading {
            return None;
        }
        if hashes == 1 {
            let text = trimmed[hashes..].trim();
            if self.in_attachment && legacy_attachment_label(text).is_some() {
                self.levels = [0; 4];
                self.expecting_title = true;
                self.legacy_attachment = true;
                return None;
            }
            self.centered_title = self.expecting_title;
            self.expecting_title = false;
            self.legacy_attachment = false;
            return None;
        }
        if self.legacy_attachment && self.expecting_title && hashes == 2 {
            self.centered_title = true;
            self.expecting_title = false;
            return None;
        }
        let level = if self.legacy_attachment {
            hashes - 1
        } else {
            hashes
        };
        official_heading_prefix(level as u8, &mut self.levels)
    }

    /// 最近一次 `next` 处理的行是否为正式标题（方正小标宋二号居中渲染）。
    pub(crate) fn centered_title(&self) -> bool {
        self.centered_title
    }
}
