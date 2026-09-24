//! 标注层 → 带哨兵 Markdown：哨兵在「解析后」注入（方案 4.3）——标注稿由
//! 视觉模型重新生成，哨兵只落在最终的文字上，不会像旧实现那样插进
//! `**`、标题层级这类语法中间。导出的 PDF / Word 因此与预览同一份数据。
//!
//! 序列化要点：
//! - 删除的标题 / 列表项不排成标题 / 列表行（避免消耗自动编号，排出来
//!   还会在编号序列里冒重复号），作为普通文字行留在原位置画删除线；
//! - 表格按 GFM 逐行写回，标记只在单元格文字里；跨格合并按来源表还原；
//! - 对齐行保留居中 / 居右标记。

use super::model::VisualBlock;
use super::overlay::{BlockOverlay, Fragment, OverlayItem, RedlineOverlay, TableOverlay};
use crate::export::{
    ColumnAlign, LineAlign, MarkdownBlock, MarkdownSection, RedlineKind, TableSpan, mark_added,
    mark_deleted, table_span_at,
};

/// 一个待拼接的块：行内用 `\n`，块与块之间用 `\n\n`（相邻两张表也空行分隔，
/// 否则 GFM 会把它们并成一张）。
struct Chunk {
    lines: Vec<String>,
}

/// 把标注层序列化成带花脸稿哨兵的 Markdown，直接交给现有导出链。
/// 移动注记作为一段加框的小注排在移动段前面（方案规则 6：段首加注）。
pub(crate) fn to_marked_markdown(overlay: &RedlineOverlay) -> String {
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut align_group: Option<LineAlign> = None;
    for item in &overlay.items {
        let (overlay_block, deleted) = match item {
            OverlayItem::Block(block) => (block, false),
            OverlayItem::Deleted(block) => (block, true),
        };
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
        // 移动注记：加框小注排在移动段前面。
        if !deleted && let Some(note) = &overlay_block.note {
            lines.insert(0, mark_added(&format!("（{note}）")));
        }
        chunks.push(Chunk { lines });
    }
    chunks
        .iter()
        .map(|chunk| chunk.lines.join("\n"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn align_marker(align: LineAlign) -> &'static str {
    match align {
        LineAlign::Center => "<!-- [居中] -->",
        LineAlign::Right => "<!-- [居右] -->",
    }
}

/// 一个块产出的源码行；空返回表示这一块不落任何行（未改的 Html 等）。
fn emit_block(overlay: &BlockOverlay, deleted: bool) -> Vec<String> {
    match &overlay.block {
        VisualBlock::Element { field, .. } => {
            let marked = mark_fragments(&overlay.text);
            if marked.is_empty() {
                return Vec::new();
            }
            vec![format!("【{}】{}", field.label(), marked)]
        }
        VisualBlock::Parsed { block, .. } => match block {
            MarkdownBlock::Title(_) => {
                let marked = mark_fragments(&overlay.text);
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
                let marked = mark_fragments(&overlay.text);
                if marked.is_empty() {
                    return Vec::new();
                }
                if deleted {
                    // 删除的标题不带编号：不排成标题（编号会重复），作为普通
                    // 文字行留在原位置画删除线。
                    vec![marked]
                } else {
                    vec![format!("{} {marked}", "#".repeat(*level as usize))]
                }
            }
            MarkdownBlock::Paragraph(_) => {
                let marked = mark_fragments(&overlay.text);
                if marked.is_empty() {
                    return Vec::new();
                }
                vec![marked]
            }
            MarkdownBlock::OrderedListItem { .. } => {
                let marked = mark_fragments(&overlay.text);
                if marked.is_empty() {
                    return Vec::new();
                }
                if deleted {
                    // 删除的列表项不排成列表行，避免消耗自动编号。
                    vec![marked]
                } else {
                    vec![format!("- {marked}")]
                }
            }
            MarkdownBlock::Aligned { text, .. } => {
                let marked = if overlay.text.is_empty() {
                    mark_str(text, deleted)
                } else {
                    mark_fragments(&overlay.text)
                };
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
            // Html（含序号表等标记行）不落到纸上。
            _ => Vec::new(),
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

/// 片段序列 → 带哨兵文字。
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

fn mark_str(text: &str, deleted: bool) -> String {
    if text.is_empty() {
        return String::new();
    }
    if deleted {
        mark_deleted(text)
    } else {
        mark_added(text)
    }
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
        let marked = marked("甲段。\n\n乙段。\n\n丙段。", "甲段。\n\n丙段。\n\n乙段。");
        let readable = readable(&marked);
        assert!(
            readable.contains("[（本段由原第2段移来）]"),
            "移动注记作为加框小注落在纸上：{readable}"
        );
        assert!(readable.contains("乙段。"), "移动段正常排出：{readable}");
        assert!(!readable.contains("~乙段。~"), "原位置无删除线：{readable}");
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
