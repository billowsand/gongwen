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
    ColumnAlign, LineAlign, MarkdownBlock, MarkdownSection, RedlineKind, TableSpan,
    inline_visible_char_indices, mark_added, mark_deleted, parse_align_marker,
    parse_numbered_table_marker, table_span_at,
};

/// 一个待拼接的块：行内用 `\n`；`tight_after` 为 true 时与下一个块之间也
/// 用 `\n`（列表项之间，保持是一组），否则用 `\n\n`（相邻两张表也空行
/// 分隔，否则 GFM 会把它们并成一张）。
struct Chunk {
    lines: Vec<String>,
    tight_after: bool,
}

/// 把标注层序列化成带花脸稿哨兵的 Markdown，直接交给现有导出链。
/// 移动注记作为普通括注排在移动段前面（版式小注，不带增删含义）。
pub(crate) fn to_marked_markdown(overlay: &RedlineOverlay) -> String {
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
        chunks.push(Chunk { lines, tight_after });
    }
    let mut out = String::new();
    for (index, chunk) in chunks.iter().enumerate() {
        if index > 0 {
            out.push_str(if chunks[index - 1].tight_after {
                "\n"
            } else {
                "\n\n"
            });
        }
        out.push_str(&chunk.lines.join("\n"));
    }
    out
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
                    // 段内列表：各项重新拆成列表行（编号由导出器按设置再生成）。
                    // 引导句后空一行让解析器把列表当成独立块；如果走\n 紧接
                    // 解析器会把它当成 inline list、合并进段落、圈号漂在纸面上。
                    let parts = split_fragments_at(&overlay.text, inline_items);
                    let mut lines: Vec<String> = Vec::new();
                    for (index, part) in parts.iter().enumerate() {
                        let marked = mark_fragments(part);
                        if index == 0 {
                            if !marked.is_empty() {
                                lines.push(marked);
                                lines.push(String::new());
                            }
                        } else {
                            // 各项重新生成 `1.`/`2.`/`3.`... 顺序编号：解析器按
                            // 显式编号取，不会按位置重排，这样删中间项时剩余项也
                            // 保持原编号（删项占掉的编号空缺由导出器自行处理）。
                            lines.push(format!("{}. {marked}", index));
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

/// 按纯文本字符偏移把片段序列切成若干段（段内列表拆行用）。坐标以**新版
/// 纯文本**计：Deleted 片段是旧文字、不在版面上，不推进坐标，开头若是
/// Deleted 则归入前一项（与独立列表删除项的处理一致）。否则这段 inline
/// 列表会因「前一字符被删」而把该项切到错位置、编号也错位。
fn split_fragments_at(fragments: &[Fragment], offsets: &[usize]) -> Vec<Vec<Fragment>> {
    let mut parts: Vec<Vec<Fragment>> = Vec::new();
    let mut current: Vec<Fragment> = Vec::new();
    let mut pos = 0usize;
    let mut offsets = offsets.iter().copied().peekable();
    for fragment in fragments {
        let mut rest: &str = &fragment.text;
        let is_deleted = fragment.kind == RedlineKind::Deleted;
        while !rest.is_empty() {
            // 当前字符是否正好落在某条列表项边界上（必须非 Deleted 段）。
            let absorb = if !is_deleted
                && let Some(off) = offsets.peek().copied()
                && (pos == off || pos + rest.chars().count() > off)
            {
                if pos == off {
                    parts.push(std::mem::take(&mut current));
                    offsets.next();
                } else {
                    let take = off - pos;
                    let mut chars = rest.chars();
                    let head: String = chars.by_ref().take(take).collect();
                    current.push(Fragment {
                        kind: fragment.kind,
                        text: head,
                    });
                    pos += take;
                    rest = chars.as_str();
                }
                true
            } else {
                false
            };
            if !absorb {
                let take_chars = rest.chars().count();
                let mut chars = rest.chars();
                let piece: String = chars.by_ref().take(take_chars).collect();
                let chunk = Fragment {
                    kind: fragment.kind,
                    text: piece,
                };
                // 紧贴偏移边界、且是删除：并入前一段（它属于前一项的文字）。
                if is_deleted
                    && !current.is_empty()
                    && pos == offsets.peek().copied().unwrap_or(usize::MAX)
                    && let Some(last_part) = parts.last_mut()
                {
                    last_part.push(chunk);
                } else {
                    current.push(chunk);
                }
                if !is_deleted {
                    pos += take_chars;
                }
                rest = "";
            }
        }
    }
    parts.push(current);
    parts
}

/// 文本块的标注投影：Same / Added 片段落在新版原文里（按纯文本字符 ↔
/// 原文 byte 映射取带样式的切片），Deleted 片段是旧文字、不存在于新版
/// 原文，直接以纯文本包哨兵。`**加粗**` 这类行内标记按 toggle 配对拆分：
/// open 与 close 在同一段 → 整对区间归此片段；横跨多段 → 这片只取开、
/// 下一片只取闭，**绝不重复**写出同一对加粗。
fn styled_marked(fragments: &[Fragment], raw: &str) -> String {
    if fragments.is_empty() {
        return String::new();
    }
    if fragments.iter().all(|f| f.is_same()) {
        return raw.to_string();
    }
    let plain = crate::export::plain_text(raw);
    let plain_chars = plain.chars().count();
    let toggles = bold_toggle_offsets(raw);
    let raw_boundaries: Vec<usize> = raw
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(raw.len()))
        .collect();
    let visible = inline_visible_char_indices(raw, &raw_boundaries);
    let mut starts: Vec<usize> = vec![usize::MAX; plain_chars];
    for (char_index, (byte, _)) in raw.char_indices().enumerate() {
        let end_visible = visible[char_index + 1];
        if end_visible > visible[char_index] && end_visible <= plain_chars {
            starts[end_visible - 1] = byte;
        }
    }

    // 先收集各 Same/Added 段的字节范围（Deleted 段单独先输出）。
    struct TextSlice {
        kind: RedlineKind,
        from: usize,
        to: usize,
        fallback: String,
    }
    let mut slices: Vec<TextSlice> = Vec::with_capacity(fragments.len());
    let mut offset = 0usize;
    for fragment in fragments {
        let count = fragment.char_count();
        if fragment.kind == RedlineKind::Deleted {
            continue;
        }
        if offset >= plain_chars {
            slices.push(TextSlice {
                kind: fragment.kind,
                from: raw.len(),
                to: raw.len(),
                fallback: fragment.text.clone(),
            });
        } else {
            let start_byte = starts[offset];
            if start_byte == usize::MAX {
                slices.push(TextSlice {
                    kind: fragment.kind,
                    from: raw.len(),
                    to: raw.len(),
                    fallback: fragment.text.clone(),
                });
            } else {
                let mut end_byte = if offset + count < plain_chars {
                    starts[offset + count].min(raw.len())
                } else {
                    raw.len()
                };
                if end_byte == usize::MAX {
                    end_byte = raw.len();
                }
                slices.push(TextSlice {
                    kind: fragment.kind,
                    from: start_byte,
                    to: end_byte.max(start_byte),
                    fallback: String::new(),
                });
            }
        }
        offset += count;
    }

    // 按 toggle 配对拆分加粗边界：open 字节紧接的 plain 字符所属片段
    // 拥有 open，close 同理。同一段 → 整对区间；跨段 → 拆成开闭两份。
    for pair_index in 0..toggles.len() / 2 {
        let open = toggles[pair_index * 2];
        let close = toggles[pair_index * 2 + 1];
        let owner_open = slices.iter().position(|s| s.from <= open && open < s.to);
        let owner_close = slices.iter().position(|s| s.from <= close && close < s.to);
        match (owner_open, owner_close) {
            (Some(i), Some(j)) if i == j => {
                let slice = &mut slices[i];
                slice.from = slice.from.min(open);
                slice.to = slice.to.max(close + 2);
            }
            (Some(i), Some(j)) if i < j => {
                let slice_i = &mut slices[i];
                slice_i.to = slice_i.to.max(open + 2);
                let slice_j = &mut slices[j];
                slice_j.from = slice_j.from.min(close);
            }
            _ => {}
        }
    }

    let mut out = String::new();
    // 先输出 Deleted 片段（按原顺序）。
    for fragment in fragments {
        if fragment.kind == RedlineKind::Deleted {
            out.push_str(&mark_deleted(&fragment.text));
        }
    }
    for slice in slices {
        if slice.from >= slice.to {
            out.push_str(&mark_added(&slice.fallback));
        } else {
            let piece = &raw[slice.from..slice.to.min(raw.len())];
            match slice.kind {
                RedlineKind::Same => out.push_str(piece),
                _ => out.push_str(&mark_added(piece)),
            }
        }
    }
    out
}

/// 原文里未成对转义的 `**` / `__` 标记的字节位置（出现顺序即配对方：
/// 第 1、2 个一对，第 3、4 个一对，与 `inline_atoms` 同一口径）。
fn bold_toggle_offsets(raw: &str) -> Vec<usize> {
    let mut offsets = Vec::new();
    let bytes = raw.as_bytes();
    let mut index = 0usize;
    while index + 1 < bytes.len() {
        let is_marker = (bytes[index] == b'*' && bytes[index + 1] == b'*')
            || (bytes[index] == b'_' && bytes[index + 1] == b'_');
        let marker = is_marker.then_some(2usize);
        if let Some(width) = marker {
            // 转义（`\**`）与三个以上连续星号里的不计：与 inline_atoms
            // 的 escaped_at / 成对口径保持一致即可，花脸稿由程序生成，
            // 正常只会出现成对的 `**`。
            let escaped = index > 0 && bytes[index - 1] == b'\\';
            let triple_star =
                width == 2 && bytes[index] == b'*' && bytes.get(index + 2) == Some(&b'*');
            if !escaped && !triple_star {
                offsets.push(index);
            }
            index += width;
        } else {
            index += 1;
        }
    }
    offsets
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
        // 公式被识别为受保护 token，整段增删。但文本「由」「可知。」作为 jieba
        // 词在两边相同，是 Same 片段，不会被公式的改动波及——这保证了不变式。
        let marked = marked("由$x^{2}$可知。", "由$x^{3}$可知。");
        let readable = readable(&marked);
        assert!(readable.contains("~$x^{2}$~"), "公式整段删除：{readable}");
        assert!(readable.contains("[$x^{3}$]"), "公式整段新增：{readable}");
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
