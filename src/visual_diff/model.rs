//! 渲染文本模型：一个版本（Markdown + 可选的公文要素）展开成的「视觉块」序列。
//!
//! 模型复用 `export::parse` 的切块与编号逻辑（见 `parse_markdown_located_with_numbering`），
//! 保证与导出看到的是同一份东西：Markdown 标记已剥离、列表与序号表的编号已按
//! 解析规则生成进文本，标题上的手写编号已清掉（成文时编号由导出器再生成，
//! 因此标题与列表的「编号」天然不在比较范围内，方案规则 8 的第一半由此满足）。

use crate::export::{MarkdownBlock, TableSpan, parse_markdown_located_with_numbering};
use crate::models::{DraftInput, NumberingConfig};
use std::ops::Range;

/// 版头 / 版记里的一个公文要素字段。要素改动按方案规则 7 整体替换，不做词级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ElementField {
    /// 发文单位（红头机关标志）。
    IssuingUnit,
    /// 主送机关。
    Recipient,
    /// 抄送机关。
    CopiesTo,
    /// 发文字号（机关代字〔年份〕序号）。
    DocumentNumber,
    /// 成文日期。
    Date,
    /// 落款单位。
    SigningUnit,
    /// 密级（含保密期限）。
    Security,
}

impl ElementField {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::IssuingUnit => "发文单位",
            Self::Recipient => "主送机关",
            Self::CopiesTo => "抄送机关",
            Self::DocumentNumber => "发文字号",
            Self::Date => "成文日期",
            Self::SigningUnit => "落款单位",
            Self::Security => "密级",
        }
    }

    /// 全部要素的固定顺序；两版模型按同一顺序取出后按下标配对。
    pub(crate) fn all() -> [Self; 7] {
        [
            Self::IssuingUnit,
            Self::Recipient,
            Self::CopiesTo,
            Self::DocumentNumber,
            Self::Date,
            Self::SigningUnit,
            Self::Security,
        ]
    }
}

/// 一个视觉块：要么是 `export::parse` 的一块（附带源码字节范围，供后续
/// 联动 / 还原使用），要么是一个公文要素字段。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum VisualBlock {
    Parsed {
        block: MarkdownBlock,
        range: Range<usize>,
    },
    Element {
        field: ElementField,
        text: String,
    },
}

impl VisualBlock {
    /// 文本类块（标题 / 正文 / 列表项 / 对齐行 / 要素）的纯文本。
    pub(crate) fn text(&self) -> Option<&str> {
        match self {
            Self::Parsed { block, .. } => match block {
                MarkdownBlock::Title(text)
                | MarkdownBlock::Heading(_, text)
                | MarkdownBlock::Paragraph(text)
                | MarkdownBlock::OrderedListItem { text, .. }
                | MarkdownBlock::Aligned { text, .. } => Some(text),
                _ => None,
            },
            Self::Element { text, .. } => Some(text),
        }
    }

    /// 这一块是否是正文段落（移动注记里的「第 X 段」只数段落）。
    pub(crate) fn is_paragraph(&self) -> bool {
        matches!(
            self,
            Self::Parsed {
                block: MarkdownBlock::Paragraph(_),
                ..
            }
        )
    }
}

/// 一个版本的完整视觉模型。
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct DocumentModel {
    pub(crate) blocks: Vec<VisualBlock>,
}

impl DocumentModel {
    /// 由 Markdown 正文构建模型。编号样式取默认：两版用同一把尺子比，
    /// 差异只可能来自正文本身；序列化后的标注稿交给导出器时再按设置重新编号。
    pub(crate) fn from_markdown(markdown: &str) -> Self {
        Self::from_markdown_with_numbering(markdown, &NumberingConfig::default())
    }

    /// 与 [`from_markdown`] 相同，另按设置生成段内列表与序号表编号。
    pub(crate) fn from_markdown_with_numbering(
        markdown: &str,
        numbering: &NumberingConfig,
    ) -> Self {
        let blocks = parse_markdown_located_with_numbering(markdown, numbering)
            .into_iter()
            .map(|located| VisualBlock::Parsed {
                block: located.block,
                range: located.range,
            })
            .collect();
        Self { blocks }
    }

    /// 由公文要素 + Markdown 正文构建模型。要素排在模型最前，顺序固定
    /// （见 [`ElementField::all`]），比较时按下标配对。
    /// 第 ④ 期的版本浏览入口将提供旧版 `DraftInput` 后由它驱动要素标注。
    #[allow(dead_code)]
    pub(crate) fn from_inputs(input: &DraftInput, markdown: &str) -> Self {
        let mut model = Self::from_markdown(markdown);
        let profile = &input.profile;
        let elements = ElementField::all().into_iter().map(|field| {
            let text = match field {
                ElementField::IssuingUnit => profile.issuing_unit.trim().to_string(),
                ElementField::Recipient => profile.recipient.trim().to_string(),
                ElementField::CopiesTo => profile.copies_to.trim().to_string(),
                ElementField::DocumentNumber => {
                    let year = profile.document_year.trim();
                    let code = profile.department_code.trim();
                    let number = profile.document_number.trim();
                    if year.is_empty() && code.is_empty() && number.is_empty() {
                        String::new()
                    } else {
                        format!("{code}〔{year}〕{number}号")
                    }
                }
                ElementField::Date => input.date.trim().to_string(),
                ElementField::SigningUnit => {
                    let unit = profile.signing_unit.trim();
                    if unit.is_empty() {
                        profile.issuing_unit.trim().to_string()
                    } else {
                        unit.to_string()
                    }
                }
                ElementField::Security => {
                    let level = profile.security_level.trim();
                    let period = profile.security_period.trim();
                    if period.is_empty() {
                        level.to_string()
                    } else {
                        format!("{level}★{period}")
                    }
                }
            };
            VisualBlock::Element { field, text }
        });
        model.blocks.splice(0..0, elements);
        model
    }

    /// 模型里第 `index` 个正文段落的序号（1-based，跨区段连续数）。
    /// 移动注记「（本段由原第 X 段移来）」的 X 由此而来。
    pub(crate) fn paragraph_number(&self, index: usize) -> usize {
        self.blocks
            .iter()
            .take(index)
            .filter(|block| block.is_paragraph())
            .count()
            + 1
    }

    /// 模型的表格块访问助手。
    pub(crate) fn table(block: &VisualBlock) -> Option<(&[Vec<String>], &[TableSpan])> {
        match block {
            VisualBlock::Parsed {
                block: MarkdownBlock::Table { rows, spans, .. },
                ..
            } => Some((rows, spans)),
            _ => None,
        }
    }
}
