//! 标题计数：标题层级编号、正文标题与居中标记。
//!
//! 由 src/export/mod.rs 拆分而来：本文件是模块 `export::headings`，与其它子模块共享
//! `export` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::export::{
    MarkdownSection, circled_number, legacy_attachment_label, number_to_chinese,
    parse_section_marker,
};
use crate::models::{HeadingNumbering, ListNumbering, NumberingConfig};
use regex::Regex;
use std::sync::OnceLock;

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

/// 按编号样式生成一个标题编号前缀。`第X章`/`第X节`/`第X部分` 类样式在编号后
/// 补一个全角空格与标题文字分隔，其余样式（如 `一、`、`1.`）直接与标题文字相连。
pub(crate) fn render_heading_number(style: HeadingNumbering, number: usize) -> String {
    match style {
        HeadingNumbering::Chinese => format!("{}、", number_to_chinese(number)),
        HeadingNumbering::ChineseParen => format!("（{}）", number_to_chinese(number)),
        HeadingNumbering::FullDigitParen => format!("（{number}）"),
        HeadingNumbering::HalfDigitParen => format!("({number})"),
        HeadingNumbering::DecimalDot => format!("{number}."),
        HeadingNumbering::ChapterDigit => format!("第{number}章　"),
        HeadingNumbering::ChapterChinese => format!("第{}章　", number_to_chinese(number)),
        HeadingNumbering::Part => format!("第{}部分　", number_to_chinese(number)),
        HeadingNumbering::SectionDigit => format!("第{number}节　"),
    }
}

/// 按编号样式生成一个列表项编号前缀。
pub(crate) fn render_list_number(style: ListNumbering, number: usize) -> String {
    match style {
        ListNumbering::Circled => circled_number(number),
        ListNumbering::HalfParen => format!("({number})"),
        ListNumbering::FullParen => format!("（{number}）"),
        ListNumbering::DecimalDot => format!("{number}."),
    }
}

/// 公文正文各级标题的编号：默认 一、 →（一）→ 1. →（1）。DOCX 导出与界面预览共用，
/// 保证预览里看到的编号就是导出后的编号。
pub(crate) fn official_heading_text(
    level: u8,
    text: &str,
    counters: &mut [usize; 4],
    numbering: &NumberingConfig,
) -> Option<String> {
    official_heading_prefix(level, counters, numbering).map(|prefix| format!("{prefix}{text}"))
}

/// 只生成公文标题编号前缀。实时排版编辑器不能把自动编号真正写进
/// Markdown，因此用这个共用函数在屏幕上叠加，导出时仍由同一套计数器生成。
pub(crate) fn official_heading_prefix(
    level: u8,
    counters: &mut [usize; 4],
    numbering: &NumberingConfig,
) -> Option<String> {
    let style = numbering.heading(level)?;
    match level {
        2 => {
            counters[0] += 1;
            counters[1..].fill(0);
            Some(render_heading_number(style, counters[0]))
        }
        3 => {
            counters[1] += 1;
            counters[2..].fill(0);
            Some(render_heading_number(style, counters[1]))
        }
        4 => {
            counters[2] += 1;
            counters[3] = 0;
            Some(render_heading_number(style, counters[2]))
        }
        5 => {
            counters[3] += 1;
            Some(render_heading_number(style, counters[3]))
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
    numbering: NumberingConfig,
    expecting_title: bool,
    in_attachment: bool,
    legacy_attachment: bool,
    /// 最近一次 `next` 处理的行是否为正式标题（文档标题或附件正式标题），
    /// 需要按方正小标宋二号居中渲染，与预览/导出一致。
    centered_title: bool,
    /// 最近一次 `next` 处理的行如果是带编号的标题，它折算后的公文层级。
    /// 旧格式附件里 `#` 的个数比实际层级多一层，导航窗格的缩进按这个值算，
    /// 才不会把附件里的「一、」画得比正文的「一、」深一层。
    numbered_level: Option<u8>,
}

impl Default for HeadingCounters {
    fn default() -> Self {
        Self::with_numbering(NumberingConfig::default())
    }
}

impl HeadingCounters {
    pub(crate) fn with_numbering(numbering: NumberingConfig) -> Self {
        Self {
            levels: [0; 4],
            numbering,
            expecting_title: true,
            in_attachment: false,
            legacy_attachment: false,
            centered_title: false,
            numbered_level: None,
        }
    }

    pub(crate) fn next(&mut self, line: &str) -> Option<String> {
        self.centered_title = false;
        self.numbered_level = None;
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
        let prefix = official_heading_prefix(level as u8, &mut self.levels, &self.numbering);
        if prefix.is_some() {
            self.numbered_level = Some(level as u8);
        }
        prefix
    }

    /// 最近一次 `next` 处理的行如果排出了编号，它在公文里的实际层级：
    /// 2 是「一、」，3 是「（一）」，4 是「1.」，5 是「（1）」。没排出编号时为 None。
    pub(crate) fn numbered_level(&self) -> Option<u8> {
        self.numbered_level
    }

    /// 最近一次 `next` 处理的行是否为正式标题（方正小标宋二号居中渲染）。
    pub(crate) fn centered_title(&self) -> bool {
        self.centered_title
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_every_heading_numbering_style() {
        assert_eq!(render_heading_number(HeadingNumbering::Chinese, 3), "三、");
        assert_eq!(
            render_heading_number(HeadingNumbering::ChineseParen, 3),
            "（三）"
        );
        assert_eq!(
            render_heading_number(HeadingNumbering::FullDigitParen, 3),
            "（3）"
        );
        assert_eq!(
            render_heading_number(HeadingNumbering::HalfDigitParen, 3),
            "(3)"
        );
        assert_eq!(render_heading_number(HeadingNumbering::DecimalDot, 3), "3.");
        assert_eq!(
            render_heading_number(HeadingNumbering::ChapterDigit, 3),
            "第3章　"
        );
        assert_eq!(
            render_heading_number(HeadingNumbering::ChapterChinese, 3),
            "第三章　"
        );
        assert_eq!(
            render_heading_number(HeadingNumbering::Part, 3),
            "第三部分　"
        );
        assert_eq!(
            render_heading_number(HeadingNumbering::SectionDigit, 3),
            "第3节　"
        );
    }

    #[test]
    fn renders_every_list_numbering_style() {
        assert_eq!(render_list_number(ListNumbering::Circled, 3), "③");
        assert_eq!(render_list_number(ListNumbering::HalfParen, 3), "(3)");
        assert_eq!(render_list_number(ListNumbering::FullParen, 3), "（3）");
        assert_eq!(render_list_number(ListNumbering::DecimalDot, 3), "3.");
    }

    #[test]
    fn official_heading_prefix_follows_custom_config() {
        let numbering = NumberingConfig {
            heading1: HeadingNumbering::ChapterDigit,
            heading2: HeadingNumbering::DecimalDot,
            heading3: HeadingNumbering::ChineseParen,
            heading4: HeadingNumbering::FullDigitParen,
            ..Default::default()
        };
        let mut counters = [0usize; 4];
        assert_eq!(
            official_heading_prefix(2, &mut counters, &numbering).as_deref(),
            Some("第1章　")
        );
        assert_eq!(
            official_heading_prefix(3, &mut counters, &numbering).as_deref(),
            Some("1.")
        );
        assert_eq!(
            official_heading_prefix(4, &mut counters, &numbering).as_deref(),
            Some("（一）")
        );
        assert_eq!(
            official_heading_prefix(5, &mut counters, &numbering).as_deref(),
            Some("（1）")
        );
    }
}
