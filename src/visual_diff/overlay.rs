//! 标注层：视觉 diff 的产物。不改动新版文本本身，只附着
//! `(文字, Same/Deleted/Added)` 片段、整删块与移动注记，三处消费方
//! （序列化导出、预览、一致性测试）读同一份数据。

use super::model::VisualBlock;
use crate::export::{RedlineKind, TableSpan};

/// 一段同类文字；`Deleted` 的旧文字留在原位画删除线，`Added` 的新文字就地加框。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fragment {
    pub(crate) kind: RedlineKind,
    pub(crate) text: String,
}

impl Fragment {
    pub(crate) fn same(text: impl Into<String>) -> Self {
        Self {
            kind: RedlineKind::Same,
            text: text.into(),
        }
    }

    pub(crate) fn deleted(text: impl Into<String>) -> Self {
        Self {
            kind: RedlineKind::Deleted,
            text: text.into(),
        }
    }

    pub(crate) fn added(text: impl Into<String>) -> Self {
        Self {
            kind: RedlineKind::Added,
            text: text.into(),
        }
    }

    pub(crate) fn is_same(&self) -> bool {
        self.kind == RedlineKind::Same
    }

    /// 片段的字符数（归并规则按字数统计）。
    pub(crate) fn char_count(&self) -> usize {
        self.text.chars().count()
    }
}

/// 表格一行的标注。`deleted` 为 true 表示这是一行旧表里的已删行
/// （整行删除线）；否则是新表的一行，`source_row` 指向该行在来源表里的
/// 下标（跨格合并序列化时按来源表查 spans）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowOverlay {
    pub(crate) deleted: bool,
    pub(crate) source_row: usize,
    pub(crate) cells: Vec<Vec<Fragment>>,
}

/// 一张配对表格的标注：行序列（含整删行）+ 新旧两表各自的合并单元格。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TableOverlay {
    pub(crate) columns: usize,
    pub(crate) rows: Vec<RowOverlay>,
    /// 新表的合并单元格（整行合并的分组行等）。
    pub(crate) spans: Vec<TableSpan>,
    /// 旧表的合并单元格，存在整删行时用于序列化。
    pub(crate) old_spans: Vec<TableSpan>,
    /// 列数变化整表替换时，前 `split_after` 行属于旧表（列数不同，必须拆成
    /// 两张独立的 GFM 表，否则解析器会把它们并成一张）。
    pub(crate) split_after: Option<usize>,
}

/// 一个块的标注：`text` 是文本类块的片段序列（标题、正文、列表项正文、
/// 要素值）；`table` 是表格块的逐格标注；`note` 是移动注记等附注。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BlockOverlay {
    pub(crate) block: VisualBlock,
    pub(crate) text: Vec<Fragment>,
    pub(crate) table: Option<TableOverlay>,
    pub(crate) note: Option<String>,
}

impl BlockOverlay {
    /// 整块均未改动（片段全是 `Same`、无表格标注、无注记）。
    #[allow(dead_code)]
    pub(crate) fn is_unchanged(&self) -> bool {
        if self.note.is_some() || self.table.is_some() {
            return false;
        }
        self.text.iter().all(Fragment::is_same)
    }
}

/// 标注层条目：新版的一个块，或插在块间的一个整删旧块。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum OverlayItem {
    Block(BlockOverlay),
    Deleted(BlockOverlay),
}

/// 一份完整标注层，按新版阅读顺序排列。
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RedlineOverlay {
    pub(crate) items: Vec<OverlayItem>,
}

impl RedlineOverlay {
    /// 两版是否完全一致（没有任何标注）。
    #[allow(dead_code)]
    pub(crate) fn is_empty(&self) -> bool {
        self.items.iter().all(|item| match item {
            OverlayItem::Block(block) => block.is_unchanged(),
            OverlayItem::Deleted(_) => false,
        })
    }

    /// 调试与测试用的可读形式：删除记为 ~…~、新增记为 […]。
    #[cfg(test)]
    pub(crate) fn readable(&self) -> String {
        fn frag_text(fragments: &[Fragment]) -> String {
            fragments
                .iter()
                .map(|fragment| match fragment.kind {
                    crate::export::RedlineKind::Same => fragment.text.clone(),
                    crate::export::RedlineKind::Deleted => format!("~{}~", fragment.text),
                    crate::export::RedlineKind::Added => format!("[{}]", fragment.text),
                })
                .collect()
        }
        self.items
            .iter()
            .map(|item| {
                let overlay = match item {
                    OverlayItem::Block(block) => block,
                    OverlayItem::Deleted(block) => block,
                };
                let body = if let Some(table) = &overlay.table {
                    table
                        .rows
                        .iter()
                        .map(|row| {
                            let cells = row
                                .cells
                                .iter()
                                .map(|cell| frag_text(cell))
                                .collect::<Vec<_>>()
                                .join(" | ");
                            format!("| {cells} |")
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    frag_text(&overlay.text)
                };
                match &overlay.note {
                    Some(note) => format!("{body}（注：{note}）"),
                    None => body,
                }
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}
