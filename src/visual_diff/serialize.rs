//! 标注层 → 带哨兵 Markdown：哨兵在「解析后」注入（方案 4.3）——标注稿由
//! 视觉模型重新生成，哨兵只落在最终的文字上，不会像旧实现那样插进
//! `**`、标题层级这类语法中间。导出的 PDF / Word 因此与预览同一份数据。
//!
//! 序列化要点：
//! - 删除的标题 / 列表项不排成标题 / 列表行（避免消耗自动编号，排出来
//!   还会在编号序列里冒重复号），作为普通文字行留在原位置画删除线；
//! - 独立列表项用单换行拼接、按原起始号写 `N.`，保持是一组；删除的列表
//!   项折进前一项的同一行，不打断组；
//! - 表格按 GFM 逐行写回，标记只在单元格文字里；跨格合并按来源表还原；
//! - 对齐行保留居中 / 居右标记；研究报告的区段标记（摘要、参考文献等）
//!   原样写回，mdx 靠它们分区；
//! - 文本块的标注投影回带样式的原文：加粗边界不被哨兵切断。

use super::model::VisualBlock;
use super::overlay::{BlockOverlay, Fragment, OverlayItem, RedlineOverlay, TableOverlay};
use crate::export::{
    ColumnAlign, LineAlign, MarkdownBlock, MarkdownSection, REDLINE_ADD_CLOSE, REDLINE_ADD_OPEN,
    REDLINE_DEL_CLOSE, REDLINE_DEL_OPEN, RedlineKind, TableSpan, inline_char_spans, mark_added,
    mark_deleted, parse_align_marker, parse_numbered_table_marker, table_span_at,
};
use std::ops::Range;

/// 一个待拼接的块：行内用 `\n`；`tight_after` 为 true 时与下一个块之间也
/// 用 `\n`（列表项之间，保持是一组），否则用 `\n\n`（相邻两张表也空行
/// 分隔，否则 GFM 会把它们并成一张）。
struct Chunk {
    lines: Vec<String>,
    tight_after: bool,
    /// 这一块由哪些标注条目写成：通常一条；删除的列表项折进前一项时会多一条。
    sources: Vec<(SourceSide, Range<usize>, bool)>,
}

/// 标注稿里一个块的来历：它在花脸稿 Markdown 里的字节范围、来自哪一版的
/// 哪段源码，以及这一块有没有标注。
///
/// 花脸稿 Markdown 是重新生成的，字节位置与用户源码对不上；版本对照要在
/// 「预览里点中的块」与「代码 diff 里的变更块」之间互跳，就靠这张表转换
/// （方案第一节：两层 diff 只通过源码字节范围互相定位）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MarkedSpan {
    /// 在花脸稿 Markdown 里的字节范围。
    pub(crate) marked: Range<usize>,
    pub(crate) side: SourceSide,
    /// 在 `side` 那一版正文源码里的字节范围。
    pub(crate) source: Range<usize>,
    /// 这一块带不带增删标注（或移动注记）。
    pub(crate) changed: bool,
}

/// 标注稿里的块来自哪一版：新版的块，或插在块间的整删旧块。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceSide {
    New,
    Old,
}

/// 把标注层序列化成带花脸稿哨兵的 Markdown，直接交给现有导出链。
/// 移动注记作为普通括注排在移动段前面（版式小注，不带增删含义）。
#[cfg(test)]
pub(crate) fn to_marked_markdown(overlay: &RedlineOverlay) -> String {
    to_marked_markdown_with_spans(overlay).0
}

/// 同 [`to_marked_markdown`]，另外返回每个块的来历表（见 [`MarkedSpan`]）。
pub(crate) fn to_marked_markdown_with_spans(overlay: &RedlineOverlay) -> (String, Vec<MarkedSpan>) {
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut align_group: Option<LineAlign> = None;
    for item in &overlay.items {
        let (overlay_block, deleted) = match item {
            OverlayItem::Block(block) => (block, false),
            OverlayItem::Deleted(block) => (block, true),
        };
        let is_list_item = matches!(
            overlay_block.block,
            VisualBlock::Parsed {
                block: MarkdownBlock::OrderedListItem { .. },
                ..
            }
        );
        let is_aligned = !deleted
            && matches!(
                overlay_block.block,
                VisualBlock::Parsed {
                    block: MarkdownBlock::Aligned { .. },
                    ..
                }
            );
        if !is_aligned {
            align_group = None;
        }
        let mut lines = emit_block(overlay_block, deleted);
        if lines.is_empty() {
            continue;
        }
        let source = match &overlay_block.block {
            VisualBlock::Parsed { range, .. } => Some((
                if deleted {
                    SourceSide::Old
                } else {
                    SourceSide::New
                },
                range.clone(),
                deleted || overlay_block.note.is_some() || !overlay_block.is_unchanged(),
            )),
            VisualBlock::Element { .. } => None,
        };
        // 连续同向的对齐行合并进同一个居中 / 居右区。
        if is_aligned
            && let VisualBlock::Parsed {
                block: MarkdownBlock::Aligned { align, .. },
                ..
            } = &overlay_block.block
            && align_group != Some(*align)
        {
            lines.insert(0, align_marker(*align).to_string());
            align_group = Some(*align);
        }
        // 移动注记：普通括注行排在移动段前面。
        if !deleted && let Some(note) = &overlay_block.note {
            lines.insert(0, format!("（{note}）"));
        }
        // 删除的列表项折进前一项的同一行：插在两项中间会把组打断，重编
        // 号全乱。前一项不是列表项时只能自成一行（罕见：组首被删）。
        if deleted && is_list_item {
            let marked = lines.join("\n");
            if let Some(last) = chunks.last_mut()
                && last.tight_after
            {
                last.lines.last_mut().expect("列表块有行").push_str(&marked);
                last.sources.extend(source);
                continue;
            }
        }
        // 列表项之间单换行：空行隔开会被解析器当成新的一组，编号全变 1。
        let tight_after = !deleted && is_list_item;
        if let Some(last) = chunks.last_mut()
            && last.tight_after
        {
            // 上一块是列表项、这一块不是：先关掉它的 tight 标记。
            last.tight_after = false;
        }
        chunks.push(Chunk {
            lines,
            tight_after,
            sources: source.into_iter().collect(),
        });
    }
    let mut out = String::new();
    let mut spans = Vec::new();
    for (index, chunk) in chunks.iter().enumerate() {
        if index > 0 {
            out.push_str(if chunks[index - 1].tight_after {
                "\n"
            } else {
                "\n\n"
            });
        }
        let start = out.len();
        out.push_str(&chunk.lines.join("\n"));
        for (side, source, changed) in &chunk.sources {
            spans.push(MarkedSpan {
                marked: start..out.len(),
                side: *side,
                source: source.clone(),
                changed: *changed,
            });
        }
    }
    (out, spans)
}

fn align_marker(align: LineAlign) -> &'static str {
    match align {
        LineAlign::Center => "<!-- [居中] -->",
        LineAlign::Right => "<!-- [居右] -->",
    }
}

/// 一个块产出的源码行；空返回表示这一块不落任何行（未改的 Html 标记行、
/// 已删除的 Html 等）。
fn emit_block(overlay: &BlockOverlay, deleted: bool) -> Vec<String> {
    let raw = overlay.block.raw_text().unwrap_or_default();
    let inline_items: &[usize] = match &overlay.block {
        VisualBlock::Parsed { inline_items, .. } => inline_items,
        _ => &[],
    };
    match &overlay.block {
        VisualBlock::Element { .. } => {
            // 要素标注落在版头 / 版记的原位（方案规则 7），那是第 ② 期
            // 预览与导出绘制的事；本期不进正文，避免误接成正文行。
            Vec::new()
        }
        VisualBlock::Parsed { block, .. } => match block {
            MarkdownBlock::Title(_) => {
                let marked = styled_marked(&overlay.text, raw);
                if marked.is_empty() {
                    return Vec::new();
                }
                if deleted {
                    // 删除的文档标题不排成标题，作为普通文字行画删除线。
                    vec![marked]
                } else {
                    vec![format!("# {marked}")]
                }
            }
            MarkdownBlock::Heading(level, _) => {
                let marked = styled_marked(&overlay.text, raw);
                if marked.is_empty() {
                    return Vec::new();
                }
                if deleted {
                    // 删除的标题不带编号：不排成标题（编号会重复），作为
                    // 普通文字行留在原位置画删除线。
                    vec![marked]
                } else {
                    vec![format!("{} {marked}", "#".repeat(*level as usize))]
                }
            }
            MarkdownBlock::Paragraph(_) => {
                if !deleted && !inline_items.is_empty() {
                    // 段内列表：各项重新拆成列表行（编号由导出器按设置再生成），
                    // 引导句与列表行用单换行紧接——空行隔开解析器就不再认作
                    // 段内列表，圈号会丢。
                    let parts = project(&overlay.text, raw, inline_items);
                    let mut lines: Vec<String> = Vec::new();
                    for (index, marked) in parts.into_iter().enumerate() {
                        if index == 0 {
                            if !marked.is_empty() {
                                lines.push(marked);
                            }
                        } else {
                            lines.push(format!("1. {marked}"));
                        }
                    }
                    return lines;
                }
                let marked = styled_marked(&overlay.text, raw);
                if marked.is_empty() {
                    return Vec::new();
                }
                vec![marked]
            }
            MarkdownBlock::OrderedListItem { number, .. } => {
                let marked = styled_marked(&overlay.text, raw);
                if marked.is_empty() {
                    return Vec::new();
                }
                if deleted {
                    // 删除的列表项不排成列表行，避免消耗自动编号；调用方
                    // 会把它折进前一项的同一行。
                    vec![marked]
                } else {
                    // 按原起始号写显式序号：解析器只认组首的序号，后续
                    // 项的编号按位次顺延，与模型里的 number 一致。
                    vec![format!("{number}. {marked}")]
                }
            }
            MarkdownBlock::Aligned { .. } => {
                let marked = styled_marked(&overlay.text, raw);
                if marked.is_empty() {
                    return Vec::new();
                }
                vec![marked]
            }
            MarkdownBlock::Image { alt, src } => {
                if deleted {
                    let label = if alt.trim().is_empty() {
                        format!("［图片：{src}］")
                    } else {
                        format!("［图片：{alt}］")
                    };
                    vec![mark_deleted(&label)]
                } else {
                    vec![format!("![{alt}]({src})")]
                }
            }
            MarkdownBlock::Table {
                aligns, numbered, ..
            } => {
                let Some(table) = &overlay.table else {
                    return Vec::new();
                };
                let mut lines: Vec<String> = Vec::new();
                if *numbered && !deleted {
                    lines.push("<!-- [序号表] -->".to_string());
                }
                if let Some(split_after) = table.split_after {
                    // 列数变化整表替换：旧表、新表各自独立，空行分隔。
                    let (old_rows, new_rows) = table.rows.split_at(split_after);
                    let old_part = TableOverlay {
                        columns: old_rows.first().map_or(0, |row| row.cells.len()),
                        rows: old_rows.to_vec(),
                        spans: Vec::new(),
                        old_spans: table.old_spans.clone(),
                        split_after: None,
                    };
                    let new_part = TableOverlay {
                        columns: new_rows.first().map_or(0, |row| row.cells.len()),
                        rows: new_rows.to_vec(),
                        spans: table.spans.clone(),
                        old_spans: Vec::new(),
                        split_after: None,
                    };
                    lines.extend(emit_table(&old_part, &aligns[..old_part.columns], true));
                    lines.push(String::new());
                    lines.extend(emit_table(&new_part, aligns, false));
                } else {
                    lines.extend(emit_table(table, aligns, deleted));
                }
                lines
            }
            MarkdownBlock::Marker(section) => {
                let line = match section {
                    MarkdownSection::Body => "<!-- [正文] -->",
                    MarkdownSection::Attachment => "<!-- [附件] -->",
                };
                vec![line.to_string()]
            }
            // Html 原样写回：研究报告的区段标记（摘要 / 参考文献 / 目录）、
            // <div> 块都靠它们分区。两类标记行除外——序号表 / 居中居右的
            // 标记由表格、对齐行的序列化负责，写两遍会乱。删除的 Html 不写。
            MarkdownBlock::Html(line) => {
                if deleted
                    || parse_numbered_table_marker(line)
                    || parse_align_marker(line).is_some()
                {
                    Vec::new()
                } else {
                    vec![line.clone()]
                }
            }
        },
    }
}

/// 表格行序列：表头 + 分隔行 + 数据行（含整删行），行内逐格标注。
fn emit_table(table: &TableOverlay, aligns: &[ColumnAlign], deleted: bool) -> Vec<String> {
    let spans: &[TableSpan] = if deleted {
        &table.old_spans
    } else {
        &table.spans
    };
    let mut lines: Vec<String> = Vec::new();
    for (index, row) in table.rows.iter().enumerate() {
        let cells = row
            .cells
            .iter()
            .enumerate()
            .map(|(column, cell)| cell_markdown(spans, row, column, cell))
            .collect::<Vec<_>>()
            .join("|");
        lines.push(format!("|{cells}|"));
        if index == 0 {
            // 分隔行：按列对齐写最简形式，冒号决定对齐，与导出器一致。
            let separator = (0..table.columns)
                .map(|column| match aligns.get(column) {
                    Some(ColumnAlign::Left) => ":---",
                    Some(ColumnAlign::Center) => ":---:",
                    Some(ColumnAlign::Right) => "---:",
                    // Auto 与缺省都不写冒号：与解析器写回前的源码一致。
                    Some(ColumnAlign::Auto) | None => "---",
                })
                .collect::<Vec<_>>()
                .join("|");
            // 与常规源码排版一致：分隔行每格带空格（`| --- | --- |`）。
            let padded = separator
                .split('|')
                .map(|cell| format!(" {cell} "))
                .collect::<Vec<_>>()
                .join("|");
            lines.push(format!("|{padded}|"));
        }
    }
    lines
}

/// 一格的 Markdown：跨格合并按来源表还原（横向续格空着，纵向续格写 ^^）。
fn cell_markdown(
    spans: &[TableSpan],
    row: &super::overlay::RowOverlay,
    column: usize,
    cell: &[Fragment],
) -> String {
    if let Some(span) = table_span_at(spans, row.source_row, column)
        && (span.row != row.source_row || span.column != column)
    {
        if span.row != row.source_row {
            return " ^^ ".to_string();
        }
        return String::new();
    }
    let marked = mark_fragments(cell);
    if marked.is_empty() {
        return " ".to_string();
    }
    format!(" {marked} ")
}

/// 片段序列 → 带哨兵文字（无样式投影的格子 / 图片标签用）。
fn mark_fragments(fragments: &[Fragment]) -> String {
    fragments
        .iter()
        .map(|fragment| match fragment.kind {
            RedlineKind::Same => fragment.text.clone(),
            RedlineKind::Deleted => mark_deleted(&fragment.text),
            RedlineKind::Added => mark_added(&fragment.text),
        })
        .collect()
}

/// 文本块的标注投影：把纯文本上算出的片段投影回带样式的原文。
///
/// 坐标系是**新版纯文本**：Same / Added 片段逐字对到原文里的可见字符，
/// 取它带转义的原文切片（[`inline_char_spans`]），加粗不照搬原文里的
/// `**`，而是按每个字的加粗状态重新开合；Deleted 片段是旧文字，不在新版
/// 原文里，按纯文本转义后包哨兵，不推进坐标。
///
/// 哨兵只在「加粗已闭合」时开合，所以 `**` 永远不会跨过哨兵——导出器先按
/// 哨兵切块、再在块内配对加粗，跨块的 `**` 会配错。
///
/// `splits` 是新版纯文本里的切点（段内列表各项的起点），在切点处另起一段
/// 输出；位于切点上的 Deleted 片段归前一段（被删的项折进前一项末尾，不占
/// 编号）。没有切点时恒返回一段。
fn project(fragments: &[Fragment], raw: &str, splits: &[usize]) -> Vec<String> {
    if fragments.is_empty() {
        return vec![String::new()];
    }
    if splits.is_empty() && fragments.iter().all(Fragment::is_same) {
        return vec![raw.to_string()];
    }
    let spans = inline_char_spans(raw);
    let mut writer = MarkWriter::default();
    let mut parts: Vec<String> = Vec::new();
    let mut splits = splits.iter().copied().peekable();
    let mut pos = 0usize;
    for fragment in fragments {
        if fragment.kind == RedlineKind::Deleted {
            writer.set_kind(RedlineKind::Deleted);
            writer.push_plain(&fragment.text);
            continue;
        }
        for ch in fragment.text.chars() {
            while splits.next_if(|split| *split <= pos).is_some() {
                parts.push(writer.finish());
            }
            writer.set_kind(fragment.kind);
            match spans.get(pos) {
                Some((source, visible, bold)) if *visible == ch => {
                    writer.set_bold(*bold);
                    writer.out.push_str(&raw[source.clone()]);
                }
                // 对不上号（不该发生）：退回纯文本，宁可丢样式也不丢字。
                _ => writer.push_plain(&ch.to_string()),
            }
            pos += 1;
        }
    }
    parts.push(writer.finish());
    parts
}

/// 逐字写出带哨兵、带加粗的 Markdown；维护当前哨兵类型与加粗开合。
#[derive(Default)]
struct MarkWriter {
    out: String,
    kind: Option<RedlineKind>,
    bold: bool,
}

impl MarkWriter {
    fn set_bold(&mut self, bold: bool) {
        if self.bold != bold {
            self.out.push_str("**");
            self.bold = bold;
        }
    }

    fn set_kind(&mut self, kind: RedlineKind) {
        let current = self.kind.unwrap_or(RedlineKind::Same);
        if current == kind {
            return;
        }
        self.set_bold(false);
        self.close_mark();
        match kind {
            RedlineKind::Same => {}
            RedlineKind::Deleted => self.out.push(REDLINE_DEL_OPEN),
            RedlineKind::Added => self.out.push(REDLINE_ADD_OPEN),
        }
        self.kind = Some(kind);
    }

    fn close_mark(&mut self) {
        match self.kind.take() {
            Some(RedlineKind::Deleted) => self.out.push(REDLINE_DEL_CLOSE),
            Some(RedlineKind::Added) => self.out.push(REDLINE_ADD_CLOSE),
            _ => {}
        }
    }

    /// 纯文本（旧文字、兜底字符）：关掉加粗，行内标记字符转义。
    ///
    /// 公式（`$…$` / `$$…$$`）原样写：公式源码不是 Markdown，转义了下标 `_`、
    /// 命令 `\` 就变成字面字符，纸上印出「w_i」。
    fn push_plain(&mut self, text: &str) {
        self.set_bold(false);
        let mut rest = text;
        while !rest.is_empty() {
            let (plain, formula) = match super::tokenize::find_formula(rest) {
                Some(range) => (&rest[..range.start], &rest[range.clone()]),
                None => (rest, ""),
            };
            for ch in plain.chars() {
                if matches!(ch, '*' | '_' | '`' | '\\') {
                    self.out.push('\\');
                }
                self.out.push(ch);
            }
            self.out.push_str(formula);
            rest = &rest[plain.len() + formula.len()..];
        }
    }

    fn finish(&mut self) -> String {
        self.set_bold(false);
        self.close_mark();
        std::mem::take(&mut self.out)
    }
}

/// 单段投影（标题、正文、列表项、对齐行）。
fn styled_marked(fragments: &[Fragment], raw: &str) -> String {
    project(fragments, raw, &[]).concat()
}

#[cfg(test)]
mod tests {
    use super::super::compare::diff_documents;
    use super::super::model::DocumentModel;
    use super::*;
    use crate::export::{REDLINE_ADD_OPEN, REDLINE_DEL_OPEN, strip_redline};

    fn marked(old: &str, new: &str) -> String {
        let overlay = diff_documents(
            &DocumentModel::from_markdown(old),
            &DocumentModel::from_markdown(new),
        );
        to_marked_markdown(&overlay)
    }

    fn readable(marked: &str) -> String {
        marked
            .chars()
            .map(|ch| match ch {
                REDLINE_DEL_OPEN | crate::export::REDLINE_DEL_CLOSE => '~',
                REDLINE_ADD_OPEN => '[',
                crate::export::REDLINE_ADD_CLOSE => ']',
                other => other,
            })
            .collect()
    }

    #[test]
    fn an_unchanged_document_round_trips_byte_for_byte() {
        let text = "第一段。\n\n| 事项 | 时限 |\n| --- | --- |\n| 备案 | 8月 |\n\n第二段。";
        let marked = marked(text, text);
        assert_eq!(marked, text, "未改动的文档必须原样往返");
    }

    #[test]
    fn marks_survive_the_export_parse_cycle() {
        // 序列化产出必须能被导出解析器原样读回：标记不破坏语法。
        let marked = marked(
            "## 报送内容\n\n同意你单位关于报送的请示。",
            "## 报送要求\n\n同意你单位关于开展检查的请示。",
        );
        let blocks = crate::export::parse_markdown(&marked);
        assert!(
            blocks
                .iter()
                .any(|block| matches!(block, MarkdownBlock::Heading(2, _))),
            "标题必须是标题：{marked}"
        );
        assert!(
            blocks
                .iter()
                .any(|block| matches!(block, MarkdownBlock::Paragraph(_))),
            "正文必须是段落：{marked}"
        );
    }

    #[test]
    fn a_deleted_heading_is_a_marked_line_not_a_heading() {
        let marked = marked("## 甲\n\n## 乙\n\n正文。", "## 甲\n\n正文。");
        let readable = readable(&marked);
        assert!(readable.contains("~乙~"), "删除的标题画删除线：{readable}");
        assert!(
            !readable.lines().any(|line| line.starts_with("## ~")),
            "删除的标题不能排成标题行（编号会重复）：{readable}"
        );
        // 重新解析后不能多出标题块。
        let blocks = crate::export::parse_markdown(&marked);
        let headings = blocks
            .iter()
            .filter(|block| matches!(block, MarkdownBlock::Heading(..)))
            .count();
        assert_eq!(headings, 1, "只剩新版的甲标题：{marked}");
    }

    #[test]
    fn a_deleted_list_item_is_a_marked_line_not_a_list_item() {
        let marked = marked("- 甲。\n- 乙。", "- 甲。");
        let readable = readable(&marked);
        assert!(
            readable.contains("~乙。~"),
            "删除的列表项画删除线：{readable}"
        );
        assert!(
            !readable.lines().any(|line| line.starts_with("- ~")),
            "删除的列表项不能排成列表行：{readable}"
        );
    }

    #[test]
    fn table_grid_survives_marking() {
        let old = "| 事项 | 时限 |\n| --- | --- |\n| 备案 | 8月10日 |";
        let new = "| 事项 | 时限 |\n| --- | --- |\n| 备案 | 9月15日 |";
        let marked = marked(old, new);
        let readable = readable(&marked);
        assert!(
            readable.contains("~8月10日~[9月15日]"),
            "格内标注：{readable}"
        );
        assert!(readable.contains("| --- | --- |"), "分隔行在：{readable}");
        assert_eq!(marked.lines().count(), 3, "表格三行紧挨着：{marked}");
        // 哨兵剥掉后必须是干净的新表格。
        let clean = strip_redline(&marked);
        let blocks = crate::export::parse_markdown(&clean);
        assert!(
            blocks
                .iter()
                .any(|block| matches!(block, MarkdownBlock::Table { .. })),
            "剥掉哨兵后仍是表格：{clean}"
        );
    }

    #[test]
    fn a_whole_table_replace_keeps_two_tables_apart() {
        let old = "| 事项 | 时限 |\n| --- | --- |\n| 备案 | 8月 |";
        let new = "| 事项 | 时限 | 责任人 |\n| --- | --- | --- |\n| 备案 | 8月 | 张三 |";
        let marked = marked(old, new);
        let clean = strip_redline(&marked);
        let blocks = crate::export::parse_markdown(&clean);
        let tables = blocks
            .iter()
            .filter(|block| matches!(block, MarkdownBlock::Table { .. }))
            .count();
        assert_eq!(tables, 2, "整表替换必须是两张独立的表：{clean}");
    }

    #[test]
    fn the_new_text_is_recovered_by_dropping_deleted_fragments() {
        // 剥离标注还原新版文本：去掉 Deleted 片段与哨兵，剩下的必须与新版
        // 纯文本一致（strip_redline 只摘哨兵，删掉的旧字仍占版面，这是花脸稿
        // 比定稿长的原因，不是提取新文本的手段）。
        let new = "## 报送要求\n\n同意你单位关于开展检查的请示。\n\n- 新增项。\n\n第三段。";
        let overlay = diff_documents(
            &DocumentModel::from_markdown("## 报送内容\n\n同意你单位关于报送的请示。"),
            &DocumentModel::from_markdown(new),
        );
        let mut recovered = String::new();
        for item in &overlay.items {
            let block = match item {
                OverlayItem::Block(block) => block,
                OverlayItem::Deleted(_) => continue,
            };
            for fragment in &block.text {
                if fragment.kind != RedlineKind::Deleted {
                    recovered.push_str(&fragment.text);
                }
            }
            if let Some(table) = &block.table {
                for row in &table.rows {
                    if row.deleted {
                        continue;
                    }
                    for cell in &row.cells {
                        for fragment in cell {
                            if fragment.kind != RedlineKind::Deleted {
                                recovered.push_str(&fragment.text);
                            }
                        }
                    }
                }
            }
        }
        // 期望值 = 新版按导出解析口径剥掉 Markdown 标记后的各块文字
        // （标注层比较的正是不带 `##`、`- ` 这类语法的渲染文本）。
        let expected: String = crate::export::parse_markdown(new)
            .iter()
            .filter_map(|block| match block {
                MarkdownBlock::Title(text)
                | MarkdownBlock::Heading(_, text)
                | MarkdownBlock::Paragraph(text)
                | MarkdownBlock::OrderedListItem { text, .. }
                | MarkdownBlock::Aligned { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let normalized: String = recovered.split_whitespace().collect();
        let expected: String = expected.split_whitespace().collect();
        assert_eq!(normalized, expected, "还原的新文本：{recovered}");
    }

    #[test]
    fn a_move_note_is_emitted_beside_the_paragraph() {
        let old = "第一段内容比较长一些用于识别。\n\n第二段内容也比较长用于识别移动。\n\n第三段内容同样足够长可以识别。";
        let new = "第三段内容同样足够长可以识别。\n\n第一段内容比较长一些用于识别。\n\n第二段内容也比较长用于识别移动。";
        let marked = marked(old, new);
        let readable = readable(&marked);
        assert!(
            readable.contains("（本段由原第3段移来）"),
            "移动注记作为括注落在纸上：{readable}"
        );
        assert!(
            !readable.contains("[（本段"),
            "移动注记不带新增框：{readable}"
        );
        assert!(
            readable.contains("第三段内容"),
            "移动段正常排出：{readable}"
        );
        assert!(!readable.contains('~'), "原位置无删除线：{readable}");
    }

    #[test]
    fn aligned_lines_keep_their_markers() {
        let old = "<!-- [居中] -->\n旧行内容";
        let new = "<!-- [居中] -->\n新行内容";
        let marked = marked(old, new);
        let readable = readable(&marked);
        assert!(
            readable.contains("<!-- [居中] -->"),
            "居中标记在：{readable}"
        );
        assert!(
            readable.contains("~旧~[新]行内容"),
            "对齐行就地标注：{readable}"
        );
    }

    #[test]
    fn formula_marks_stay_inside_the_paragraph() {
        let marked = marked("由$x^{2}$可知。", "由$x^{3}$可知。");
        let readable = readable(&marked);
        assert!(
            readable.contains("~$x^{2}$~[$x^{3}$]"),
            "公式整体删旧插新：{readable}"
        );
        assert!(!readable.contains('\n'), "不拆出额外行：{readable}");
    }

    #[test]
    fn a_deleted_formula_is_written_verbatim() {
        // 第 ③ 期测试 F7：删除侧按纯文本转义，公式里的下标 `_`、命令 `\`
        // 被转义成字面字符，纸上印出「w_i」。公式要原样写，公式外照常转义。
        let marked = marked(
            r"其中$w_i$为权重，a_b 与 $\sum_{i} x_i$。",
            r"其中$\omega_i$为权重。",
        );
        let readable = readable(&marked);
        assert!(
            readable.contains("$w_i$") && !readable.contains(r"w\_i"),
            "删除侧公式原样：{readable}"
        );
        assert!(
            readable.contains(r"$\sum_{i} x_i$"),
            "删除侧公式里的命令与下标原样：{readable}"
        );
        assert!(
            readable.contains(r"a\_b"),
            "公式外的下划线照常转义：{readable}"
        );
    }

    #[test]
    fn a_marked_title_keeps_its_chunks_for_the_run_builder() {
        // 标题改动也就地标注：序列化后的标题行带哨兵块，导出器的 marked_runs
        // 据此生成删除线 / 边框 run；行内加粗标记仍被剥成纯文本。
        let marked = marked("# 关于报送的函", "# 关于**开展检查**的函");
        let blocks = crate::export::parse_markdown(&marked);
        let title = blocks
            .iter()
            .find_map(|block| match block {
                MarkdownBlock::Title(text) => Some(text),
                _ => None,
            })
            .expect("标题在");
        let chunks = crate::export::redline_chunks(title);
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.kind == RedlineKind::Deleted && chunk.text.contains("报送")),
            "标题里的删除块在：{chunks:?}"
        );
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.kind == RedlineKind::Added && chunk.text.contains("开展检查")),
            "标题里的新增块在：{chunks:?}"
        );
    }
}
