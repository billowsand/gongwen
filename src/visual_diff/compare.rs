//! 比较规则：模型 → 标注层（方案第四节 4.2）。
//!
//! 结构：先把两版切成章节（规则 1），章节内把连续正文段拼成文字流比较
//! （规则 2，段落边界是软分隔），流内用词级 token（规则 3），结果按归并
//! 阈值整形（规则 4），未能对齐的删 / 增块按相似度与位置配成移动（规则 6），
//! 公文要素整体替换（规则 7），表格逐格 / 整行 / 整表（规则 9），公式作为
//! 受保护 token 天然整体比较（规则 10）。

use super::model::{DocumentModel, ElementField, VisualBlock};
use super::overlay::{
    BlockOverlay, Fragment, OverlayItem, RedlineOverlay, RowOverlay, TableOverlay,
};
use super::tokenize::{Token, tokenize};
use crate::export::{MarkdownBlock, RedlineKind, TableSpan};
use similar::{Algorithm, DiffTag, capture_diff_slices_deadline};
use std::collections::HashMap;

/// 归并阈值（方案第八节默认值）：夹缝 ≤ 2 字吸收进改动。
const MERGE_GAP_CHARS: usize = 2;
/// 分句改动超过一半就整句删旧插新（1/2）。
const SENTENCE_CHANGE_NUM: usize = 1;
const SENTENCE_CHANGE_DEN: usize = 2;
/// 一个分句里改动片段 ≥ 3 段就整句删旧插新。
const SENTENCE_FRAGMENT_LIMIT: usize = 3;
/// 未能对齐的删 / 增块相似度 ≥ 0.8 配成移动。
const MOVE_SIMILARITY: f32 = 0.8;
/// 段落模糊锚定（就地改写）的最低相似度：低于它才考虑一删一增。
const FUZZY_ANCHOR_SIMILARITY: f32 = 0.6;
/// 锚定要求的最低公共字符数：「甲段。」「丙段。」这类短段 Dice 相似度虚高，
/// 公共字符太少说明只是用词相近，不是同一段的改写。
const MIN_ANCHOR_COMMON_CHARS: usize = 4;
/// 列表项配对成「修改」的最低相似度；低于它宁可一删一增。
const LIST_PAIR_SIMILARITY: f32 = 0.4;
/// 段落锚定做全局加权 LCS 的规模上限（旧段数 × 新段数）。超过它——典型是没有
/// 标题、整篇几千段落在同一个 run 里——先把一字不差的段按 Myers 锚住，只在
/// 锚点之间的夹缝里做加权 LCS；否则 2000 段就要算 400 万次相似度，十几秒。
/// 上限以内走原来的全局算法，结果逐字节不变。
const GLOBAL_ANCHOR_LIMIT: usize = 250_000;

#[cfg(test)]
thread_local! {
    /// 测试用：临时改写 [`GLOBAL_ANCHOR_LIMIT`]，让同一份输入分别走两条路径比对。
    pub(crate) static ANCHOR_LIMIT_OVERRIDE: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

fn anchor_limit() -> usize {
    #[cfg(test)]
    if let Some(limit) = ANCHOR_LIMIT_OVERRIDE.with(std::cell::Cell::get) {
        return limit;
    }
    GLOBAL_ANCHOR_LIMIT
}

/// 比较两个版本，产出附着在新版上的标注层。方向：old 是基准版，new 是目标版。
pub(crate) fn diff_documents(old: &DocumentModel, new: &DocumentModel) -> RedlineOverlay {
    let mut overlay = RedlineOverlay::default();
    diff_elements(old, new, &mut overlay);
    let old_sections = split_sections(old);
    let new_sections = split_sections(new);
    for (old_section, new_section) in align_sections(old, new, &old_sections, &new_sections) {
        diff_section(old, new, old_section, new_section, &mut overlay);
    }
    overlay
}

/// Myers diff（similar 封装）：操作序列带 `tag()` / `old_range()` / `new_range()`。
fn diff_ops<T: Eq + std::hash::Hash>(old: &[T], new: &[T]) -> Vec<similar::DiffOp> {
    capture_diff_slices_deadline(Algorithm::Myers, old, new, None)
}

// ── 公文要素（规则 7）────────────────────────────────────────────────────────

fn diff_elements(old: &DocumentModel, new: &DocumentModel, overlay: &mut RedlineOverlay) {
    for field in ElementField::all() {
        let old_text = element_value(old, field);
        let new_text = element_value(new, field);
        if old_text == new_text {
            if !new_text.trim().is_empty() {
                overlay.items.push(OverlayItem::Block(BlockOverlay {
                    block: VisualBlock::Element {
                        field,
                        text: new_text,
                    },
                    text: Vec::new(),
                    table: None,
                    note: None,
                }));
            }
            continue;
        }
        // 字段变化：旧值删除线、新值加框，整体替换不做词级。
        if !old_text.trim().is_empty() {
            overlay.items.push(OverlayItem::Deleted(BlockOverlay {
                block: VisualBlock::Element {
                    field,
                    text: old_text.clone(),
                },
                text: vec![Fragment::deleted(old_text)],
                table: None,
                note: None,
            }));
        }
        if !new_text.trim().is_empty() {
            overlay.items.push(OverlayItem::Block(BlockOverlay {
                block: VisualBlock::Element {
                    field,
                    text: new_text.clone(),
                },
                text: vec![Fragment::added(new_text)],
                table: None,
                note: None,
            }));
        }
    }
}

fn element_value(model: &DocumentModel, field: ElementField) -> String {
    model
        .blocks
        .iter()
        .find_map(|block| match block {
            VisualBlock::Element { field: found, text } if *found == field => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

// ── 章节对齐（规则 1）────────────────────────────────────────────────────────

/// 一章 = 一个标题（或文首无标题的引子）+ 它之后的正文块，到下一个标题或
/// 区段标记为止。附件标记切章：附件里的标题编号从头再数，与导出口径一致。
#[derive(Debug)]
struct Section {
    heading: Option<usize>,
    blocks: Vec<usize>,
}

fn split_sections(model: &DocumentModel) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    let mut current = Section {
        heading: None,
        blocks: Vec::new(),
    };
    for (index, block) in model.blocks.iter().enumerate() {
        let opens_section = matches!(
            block,
            VisualBlock::Parsed {
                block: MarkdownBlock::Title(_)
                    | MarkdownBlock::Heading(..)
                    | MarkdownBlock::Marker(_),
                ..
            }
        );
        if opens_section {
            sections.push(std::mem::replace(
                &mut current,
                Section {
                    heading: Some(index),
                    blocks: Vec::new(),
                },
            ));
        } else {
            current.blocks.push(index);
        }
    }
    sections.push(current);
    sections
}

/// 章的排序键：标题文字（剥空白）；编号与层级不参与（规则 8 / 格式不标注）。
fn section_key(model: &DocumentModel, section: &Section) -> Option<String> {
    section.heading.and_then(|index| {
        model.blocks[index]
            .text()
            .map(normalize_key)
            .filter(|key| !key.is_empty())
    })
}

/// 章节 LCS 对齐：标题文字相同的章配成一对；配不上的按出现顺序两两配对
/// （标题改一字也由此进词级比较），多出来的单边删除 / 新增。
fn align_sections<'a>(
    old: &'a DocumentModel,
    new: &'a DocumentModel,
    old_sections: &'a [Section],
    new_sections: &'a [Section],
) -> Vec<(Option<&'a Section>, Option<&'a Section>)> {
    let old_keys: Vec<Option<String>> = old_sections
        .iter()
        .map(|section| section_key(old, section))
        .collect();
    let new_keys: Vec<Option<String>> = new_sections
        .iter()
        .map(|section| section_key(new, section))
        .collect();
    let ops = diff_ops(&old_keys, &new_keys);
    let mut pairs = Vec::new();
    let mut old_pending: Vec<usize> = Vec::new();
    let mut new_pending: Vec<usize> = Vec::new();
    for op in ops {
        match op.tag() {
            DiffTag::Equal => {
                flush(
                    old_sections,
                    new_sections,
                    &mut old_pending,
                    &mut new_pending,
                    &mut pairs,
                );
                for offset in 0..op.old_range().len() {
                    pairs.push((
                        Some(&old_sections[op.old_range().start + offset]),
                        Some(&new_sections[op.new_range().start + offset]),
                    ));
                }
            }
            DiffTag::Delete => old_pending.extend(op.old_range()),
            DiffTag::Insert => new_pending.extend(op.new_range()),
            DiffTag::Replace => {
                old_pending.extend(op.old_range());
                new_pending.extend(op.new_range());
            }
        }
    }
    flush(
        old_sections,
        new_sections,
        &mut old_pending,
        &mut new_pending,
        &mut pairs,
    );
    pairs
}

/// 配不上的章节按出现顺序两两配对，多出来的单边。
fn flush<'a>(
    old_sections: &'a [Section],
    new_sections: &'a [Section],
    old_pending: &mut Vec<usize>,
    new_pending: &mut Vec<usize>,
    pairs: &mut Vec<(Option<&'a Section>, Option<&'a Section>)>,
) {
    let count = old_pending.len().max(new_pending.len());
    for offset in 0..count {
        pairs.push((
            old_pending.get(offset).map(|index| &old_sections[*index]),
            new_pending.get(offset).map(|index| &new_sections[*index]),
        ));
    }
    old_pending.clear();
    new_pending.clear();
}

// ── 章节内比较 ────────────────────────────────────────────────────────────────

fn diff_section(
    old: &DocumentModel,
    new: &DocumentModel,
    old_section: Option<&Section>,
    new_section: Option<&Section>,
    overlay: &mut RedlineOverlay,
) {
    // 标题本身的增删改（文字级；编号不在模型里，天然不参与比较）。
    match (
        old_section.and_then(|section| section.heading),
        new_section.and_then(|section| section.heading),
    ) {
        (Some(old_head), Some(new_head)) => {
            let old_text = old.blocks[old_head].text().unwrap_or_default();
            let new_text = new.blocks[new_head].text().unwrap_or_default();
            let fragments = diff_texts(old_text, new_text);
            overlay.items.push(OverlayItem::Block(BlockOverlay {
                block: new.blocks[new_head].clone(),
                text: fragments,
                table: None,
                note: None,
            }));
        }
        (Some(old_head), None) => push_deleted_block(overlay, old.blocks[old_head].clone()),
        (None, Some(new_head)) => {
            let text = new.blocks[new_head].text().unwrap_or_default();
            overlay.items.push(OverlayItem::Block(BlockOverlay {
                block: new.blocks[new_head].clone(),
                text: vec![Fragment::added(text)],
                table: None,
                note: None,
            }));
        }
        (None, None) => {}
    }
    let old_blocks: &[usize] = old_section
        .map(|section| section.blocks.as_slice())
        .unwrap_or(&[]);
    let new_blocks: &[usize] = new_section
        .map(|section| section.blocks.as_slice())
        .unwrap_or(&[]);
    overlay
        .items
        .extend(align_content(old, new, old_blocks, new_blocks));
}

// ── 正文块对齐 ────────────────────────────────────────────────────────────────

/// 内容块切成段：连续正文段合成一个「文字流段」，列表项、表格、图片、
/// 对齐行各自成段。完全相同的段先 LCS 锚定，锚不上的整段进变更组。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegKind {
    Stream,
    List,
    Table,
    Other,
}

#[derive(Debug)]
struct Segment {
    kind: SegKind,
    blocks: Vec<usize>,
}

fn segment_kind(block: &VisualBlock) -> Option<SegKind> {
    match block {
        VisualBlock::Parsed { block, .. } => Some(match block {
            MarkdownBlock::Paragraph(_) => SegKind::Stream,
            MarkdownBlock::OrderedListItem { .. } => SegKind::List,
            MarkdownBlock::Table { .. } => SegKind::Table,
            _ => SegKind::Other,
        }),
        VisualBlock::Element { .. } => None,
    }
}

fn build_segments(model: &DocumentModel, blocks: &[usize]) -> Vec<Segment> {
    let mut segments: Vec<Segment> = Vec::new();
    for &index in blocks {
        let Some(kind) = segment_kind(&model.blocks[index]) else {
            continue;
        };
        if kind == SegKind::Stream
            && let Some(last) = segments.last_mut()
            && last.kind == SegKind::Stream
        {
            last.blocks.push(index);
            continue;
        }
        segments.push(Segment {
            kind,
            blocks: vec![index],
        });
    }
    segments
}

fn segment_key(model: &DocumentModel, segment: &Segment) -> String {
    let text: String = segment
        .blocks
        .iter()
        .filter_map(|index| model.blocks[*index].text())
        .collect();
    match segment.kind {
        SegKind::Stream | SegKind::List => normalize_key(&text),
        // 表格 / 图片 / 对齐行整体作键：内容不同就不锚定，进变更组配对。
        // 表格没有 text()，键用全部格文字拼成（图片、对齐行用 text）。
        SegKind::Table => {
            let cells: String = segment
                .blocks
                .iter()
                .filter_map(|index| DocumentModel::table(&model.blocks[*index]))
                .flat_map(|(rows, _)| rows.iter().flatten())
                .cloned()
                .collect();
            format!("Table:{}", normalize_key(&cells))
        }
        SegKind::Other => format!("Other:{}", normalize_key(&text)),
    }
}

fn align_content(
    old: &DocumentModel,
    new: &DocumentModel,
    old_blocks: &[usize],
    new_blocks: &[usize],
) -> Vec<OverlayItem> {
    let old_segments = build_segments(old, old_blocks);
    let new_segments = build_segments(new, new_blocks);
    let old_keys: Vec<String> = old_segments.iter().map(|s| segment_key(old, s)).collect();
    let new_keys: Vec<String> = new_segments.iter().map(|s| segment_key(new, s)).collect();
    let ops = diff_ops(&old_keys, &new_keys);

    let mut items = Vec::new();
    let mut old_pending: Vec<usize> = Vec::new();
    let mut new_pending: Vec<usize> = Vec::new();
    fn flush(
        old: &DocumentModel,
        new: &DocumentModel,
        old_segments: &[Segment],
        new_segments: &[Segment],
        old_pending: &mut Vec<usize>,
        new_pending: &mut Vec<usize>,
        items: &mut Vec<OverlayItem>,
    ) {
        if old_pending.is_empty() && new_pending.is_empty() {
            return;
        }
        let old_run: Vec<usize> = old_pending
            .iter()
            .flat_map(|index| old_segments[*index].blocks.iter().copied())
            .collect();
        let new_run: Vec<usize> = new_pending
            .iter()
            .flat_map(|index| new_segments[*index].blocks.iter().copied())
            .collect();
        items.extend(resolve_run(old, new, &old_run, &new_run));
        old_pending.clear();
        new_pending.clear();
    }
    for op in ops {
        match op.tag() {
            DiffTag::Equal => {
                flush(
                    old,
                    new,
                    &old_segments,
                    &new_segments,
                    &mut old_pending,
                    &mut new_pending,
                    &mut items,
                );
                for offset in 0..op.new_range().len() {
                    let segment = &new_segments[op.new_range().start + offset];
                    for &index in &segment.blocks {
                        items.push(same_block_item(new.blocks[index].clone()));
                    }
                }
            }
            DiffTag::Delete => old_pending.extend(op.old_range()),
            DiffTag::Insert => new_pending.extend(op.new_range()),
            DiffTag::Replace => {
                old_pending.extend(op.old_range());
                new_pending.extend(op.new_range());
            }
        }
    }
    flush(
        old,
        new,
        &old_segments,
        &new_segments,
        &mut old_pending,
        &mut new_pending,
        &mut items,
    );
    items
}

fn same_block_item(block: VisualBlock) -> OverlayItem {
    let table = DocumentModel::table(&block)
        .map(|(rows, spans)| whole_table_overlay(rows, spans, RedlineKind::Same));
    let text = block
        .text()
        .map(|text| vec![Fragment::same(text)])
        .unwrap_or_default();
    OverlayItem::Block(BlockOverlay {
        block,
        text,
        table,
        note: None,
    })
}

fn push_deleted_block(overlay: &mut RedlineOverlay, block: VisualBlock) {
    let (text, table) = deleted_block_overlay(&block);
    overlay.items.push(OverlayItem::Deleted(BlockOverlay {
        block,
        text,
        table,
        note: None,
    }));
}

/// 整删块的标注：文本类整块删除线；表格整表逐格删除线。
fn deleted_block_overlay(block: &VisualBlock) -> (Vec<Fragment>, Option<TableOverlay>) {
    if let Some((rows, spans)) = DocumentModel::table(block) {
        return (
            Vec::new(),
            Some(whole_table_overlay(rows, spans, RedlineKind::Deleted)),
        );
    }
    let text = block
        .text()
        .map(|text| vec![Fragment::deleted(text)])
        .unwrap_or_default();
    (text, None)
}

fn whole_table_overlay(
    rows: &[Vec<String>],
    spans: &[TableSpan],
    kind: RedlineKind,
) -> TableOverlay {
    let fragment = |text: &str| match kind {
        RedlineKind::Deleted => Fragment::deleted(text),
        RedlineKind::Added => Fragment::added(text),
        RedlineKind::Same => Fragment::same(text),
    };
    TableOverlay {
        columns: rows.first().map_or(0, Vec::len),
        rows: rows
            .iter()
            .enumerate()
            .map(|(source_row, row)| RowOverlay {
                deleted: kind == RedlineKind::Deleted,
                source_row,
                cells: row.iter().map(|cell| vec![fragment(cell)]).collect(),
            })
            .collect(),
        spans: if kind == RedlineKind::Deleted {
            Vec::new()
        } else {
            spans.to_vec()
        },
        old_spans: if kind == RedlineKind::Deleted {
            spans.to_vec()
        } else {
            Vec::new()
        },
        split_after: None,
    }
}

// ── 变更组：移动识别 + 文字流 + 配对块 ────────────────────────────────────────

/// 处理一对锚不上的段组（旧组 vs 新组）：移动识别先拿走相似段，剩余正文段
/// 按锚段切成夹缝、同一夹缝内拼文字流比较，列表项 / 表格各自配对，最后剩下
/// 的旧块成整删、新块成整增；输出按「新组位置优先、旧组在所在夹缝里按比例
/// 插空」排定先后。
fn resolve_run(
    old: &DocumentModel,
    new: &DocumentModel,
    old_blocks: &[usize],
    new_blocks: &[usize],
) -> Vec<OverlayItem> {
    let mut used_old = vec![false; old_blocks.len()];
    let mut used_new = vec![false; new_blocks.len()];
    let mut items: Vec<(f64, usize, OverlayItem)> = Vec::new();
    let mut order = 0usize;
    let push = |position: f64,
                order: &mut usize,
                items: &mut Vec<(f64, usize, OverlayItem)>,
                item: OverlayItem| {
        items.push((position, *order, item));
        *order += 1;
    };

    // 1. 段落配对（规则 6 与移动识别的最小集合原则）：先对 run 内的段落做
    //    一次 LCS——相似度 ≥ 0.6 即视为锚定，LCS 上的段就是「没动 / 就地
    //    改写」（词级标注，不加注记）；LCS 之外、相似度 ≥ 0.8 的删 / 增配对
    //    才算移动，新位置带「（本段由原第 X 段移来）」注记；剩下的进文字流
    //    比较。这样在最前面插一段、拆分合并，都不会让后面的段落被误标移动。
    let old_para: Vec<usize> = old_blocks
        .iter()
        .enumerate()
        .filter(|(_, index)| old.blocks[**index].is_paragraph())
        .map(|(pos, _)| pos)
        .collect();
    let new_para: Vec<usize> = new_blocks
        .iter()
        .enumerate()
        .filter(|(_, index)| new.blocks[**index].is_paragraph())
        .map(|(pos, _)| pos)
        .collect();
    let texts = |side_old: bool, pos: usize| -> &str {
        let block_index = if side_old {
            old_blocks[old_para[pos]]
        } else {
            new_blocks[new_para[pos]]
        };
        let model = if side_old { old } else { new };
        model.blocks[block_index].text().unwrap_or_default()
    };
    // 相似度：规模小时一次算满矩阵（原算法）；规模大时按需算、记住算过的。
    let global = old_para.len() * new_para.len() <= anchor_limit();
    let matrix: Option<Vec<Vec<f32>>> = global.then(|| {
        (0..old_para.len())
            .map(|a| {
                (0..new_para.len())
                    .map(|b| similarity(texts(true, a), texts(false, b)))
                    .collect()
            })
            .collect()
    });
    let cache: std::cell::RefCell<HashMap<(usize, usize), f32>> = Default::default();
    let sim = |a: usize, b: usize| -> f32 {
        if let Some(matrix) = &matrix {
            return matrix[a][b];
        }
        *cache
            .borrow_mut()
            .entry((a, b))
            .or_insert_with(|| similarity(texts(true, a), texts(false, b)))
    };
    // 短段相似（Dice 虚高）与包含关系（拆分 / 合并的「第二段。」含于
    // 「第一段第二段。」）都不锚定：后者交给文字流比较，能得出干净的
    // 拆分 / 合并结果。完全相同的段始终是锚。
    let can_anchor = |a: usize, b: usize| -> bool {
        if sim(a, b) < FUZZY_ANCHOR_SIMILARITY {
            return false;
        }
        let old_text = texts(true, a);
        let new_text = texts(false, b);
        if normalize_key(old_text) == normalize_key(new_text) {
            return true;
        }
        common_chars(old_text, new_text) >= MIN_ANCHOR_COMMON_CHARS
            && !containment(old_text, new_text)
    };
    let anchor_score =
        |a: usize, b: usize| -> f32 { if can_anchor(a, b) { sim(a, b) } else { 0.0 } };
    let anchored = if global {
        lcs_pairs(old_para.len(), new_para.len(), anchor_score)
    } else {
        let old_keys: Vec<String> = (0..old_para.len())
            .map(|a| normalize_key(texts(true, a)))
            .collect();
        let new_keys: Vec<String> = (0..new_para.len())
            .map(|b| normalize_key(texts(false, b)))
            .collect();
        gapped_lcs_pairs(&old_keys, &new_keys, &anchor_score)
    };
    let mut anchored_old: Vec<usize> = Vec::new();
    let mut anchored_new: Vec<usize> = Vec::new();
    // 锚段在组内的块序号（旧, 新），两侧都单调递增：第 4 步按它切夹缝，
    // 整删块也按它定位。
    let mut anchors: Vec<(usize, usize)> = Vec::new();
    for (a, b) in anchored {
        let oi = old_para[a];
        let ni = new_para[b];
        used_old[oi] = true;
        used_new[ni] = true;
        anchored_old.push(a);
        anchored_new.push(b);
        anchors.push((oi, ni));
        let old_text = old.blocks[old_blocks[oi]].text().unwrap_or_default();
        let new_text = new.blocks[new_blocks[ni]].text().unwrap_or_default();
        let fragments = if normalize_key(old_text) == normalize_key(new_text) {
            vec![Fragment::same(new_text)]
        } else {
            diff_texts(old_text, new_text)
        };
        push(
            new_position(new, new_blocks, ni),
            &mut order,
            &mut items,
            OverlayItem::Block(BlockOverlay {
                block: new.blocks[new_blocks[ni]].clone(),
                text: fragments,
                table: None,
                note: None,
            }),
        );
    }
    // 1b. 移动：LCS 之外的删 / 增段，相似度 ≥ 0.8 的按最优配对，新位置带
    //     移来注记；一字未动的只留注记。
    let mut candidates: Vec<(f32, usize, usize)> = Vec::new();
    // 标记表代替 `contains`：长稿里锚点成千上万，线性查找会让这一步变成立方。
    let mut is_anchored_old = vec![false; old_para.len()];
    let mut is_anchored_new = vec![false; new_para.len()];
    for &a in &anchored_old {
        is_anchored_old[a] = true;
    }
    for &b in &anchored_new {
        is_anchored_new[b] = true;
    }
    // 下标一律是 `old_para` / `new_para` 里的序号（与 `sim`、`texts` 同一套），
    // 换成组内块序号只在取块时做一次：组里夹着表格、列表项时两者不相等，
    // 混用会比错段落甚至越界。
    for (a, _) in is_anchored_old
        .iter()
        .enumerate()
        .filter(|(_, anchored)| !**anchored)
    {
        for (b, _) in is_anchored_new
            .iter()
            .enumerate()
            .filter(|(_, anchored)| !**anchored)
        {
            let score = sim(a, b);
            let old_text = texts(true, a);
            let new_text = texts(false, b);
            let identical = normalize_key(old_text) == normalize_key(new_text);
            if score >= MOVE_SIMILARITY && (identical || !containment(old_text, new_text)) {
                candidates.push((score, a, b));
            }
        }
    }
    candidates.sort_by(|x, y| y.0.total_cmp(&x.0));
    for (_, a, b) in candidates {
        let oi = old_para[a];
        let ni = new_para[b];
        if used_old[oi] || used_new[ni] {
            continue;
        }
        used_old[oi] = true;
        used_new[ni] = true;
        let old_index = old_blocks[oi];
        let new_index = new_blocks[ni];
        let old_text = old.blocks[old_index].text().unwrap_or_default();
        let new_text = new.blocks[new_index].text().unwrap_or_default();
        let identical = normalize_key(old_text) == normalize_key(new_text);
        let fragments = if identical {
            vec![Fragment::same(new_text)]
        } else {
            diff_texts(old_text, new_text)
        };
        let note = format!("本段由原第{}段移来", old.paragraph_number(old_index));
        push(
            new_position(new, new_blocks, ni),
            &mut order,
            &mut items,
            OverlayItem::Block(BlockOverlay {
                block: new.blocks[new_index].clone(),
                text: fragments,
                table: None,
                note: Some(note),
            }),
        );
    }

    // 2. 表格配对：按出现顺序两两配对，列数不同整表删旧插新。
    pair_tables(
        old,
        new,
        old_blocks,
        new_blocks,
        &mut used_old,
        &mut used_new,
        &mut items,
        &mut order,
    );

    // 3. 列表项配对：相似度 ≥ 0.4 的按最高分配对，做正文词级比较。
    let list_pairs = best_pairs(
        old,
        new,
        old_blocks,
        new_blocks,
        &used_old,
        &used_new,
        |block| {
            matches!(
                block,
                VisualBlock::Parsed {
                    block: MarkdownBlock::OrderedListItem { .. },
                    ..
                }
            )
        },
        LIST_PAIR_SIMILARITY,
    );
    for (oi, ni) in list_pairs {
        used_old[oi] = true;
        used_new[ni] = true;
        let old_text = old.blocks[old_blocks[oi]].text().unwrap_or_default();
        let new_text = new.blocks[new_blocks[ni]].text().unwrap_or_default();
        push(
            new_position(new, new_blocks, ni),
            &mut order,
            &mut items,
            OverlayItem::Block(BlockOverlay {
                block: new.blocks[new_blocks[ni]].clone(),
                text: diff_texts(old_text, new_text),
                table: None,
                note: None,
            }),
        );
    }

    // 3b. 对齐行配对：相似度 ≥ 0.4 的按最高分配对做词级比较；配不上的
    //     整行删除 / 新增（居中 / 居右行常常整行改写）。
    let aligned_pairs = best_pairs(
        old,
        new,
        old_blocks,
        new_blocks,
        &used_old,
        &used_new,
        |block| {
            matches!(
                block,
                VisualBlock::Parsed {
                    block: MarkdownBlock::Aligned { .. },
                    ..
                }
            )
        },
        LIST_PAIR_SIMILARITY,
    );
    for (oi, ni) in aligned_pairs {
        used_old[oi] = true;
        used_new[ni] = true;
        let old_text = old.blocks[old_blocks[oi]].text().unwrap_or_default();
        let new_text = new.blocks[new_blocks[ni]].text().unwrap_or_default();
        push(
            new_position(new, new_blocks, ni),
            &mut order,
            &mut items,
            OverlayItem::Block(BlockOverlay {
                block: new.blocks[new_blocks[ni]].clone(),
                text: diff_texts(old_text, new_text),
                table: None,
                note: None,
            }),
        );
    }

    // 4. 剩余正文段拼文字流比较（规则 2：段落边界是软分隔）。只在同一个
    //    夹缝（相邻两个锚段之间）里拼：远处删一段、别处增一段互不相干，
    //    各自整删 / 整增留在原位，不拼成一处整句替换。
    let placer = OldPlacer::new(old, new, old_blocks, new_blocks, anchors);
    let old_pool: Vec<(usize, usize)> = old_blocks
        .iter()
        .enumerate()
        .filter(|(i, index)| !used_old[*i] && old.blocks[**index].is_paragraph())
        .map(|(i, index)| (i, *index))
        .collect();
    let new_pool: Vec<(usize, usize)> = new_blocks
        .iter()
        .enumerate()
        .filter(|(i, index)| !used_new[*i] && new.blocks[**index].is_paragraph())
        .map(|(i, index)| (i, *index))
        .collect();
    let (mut old_from, mut new_from) = (0usize, 0usize);
    while old_from < old_pool.len() || new_from < new_pool.len() {
        // 两池都按组内序号递增，夹缝号也随之递增：取两侧最小的夹缝号，
        // 把落在这个夹缝里的段各切一截。
        let gap = old_pool
            .get(old_from)
            .map(|&(oi, _)| placer.old_gap(oi))
            .into_iter()
            .chain(new_pool.get(new_from).map(|&(ni, _)| placer.new_gap(ni)))
            .min()
            .unwrap_or_default();
        let old_to = old_from
            + old_pool[old_from..]
                .iter()
                .take_while(|&&(oi, _)| placer.old_gap(oi) == gap)
                .count();
        let new_to = new_from
            + new_pool[new_from..]
                .iter()
                .take_while(|&&(ni, _)| placer.new_gap(ni) == gap)
                .count();
        let old_slice = &old_pool[old_from..old_to];
        let new_slice = &new_pool[new_from..new_to];
        old_from = old_to;
        new_from = new_to;
        let stream = stream_diff(old, new, old_slice, new_slice);
        // 旧池的正文段全部参与了文字流比较：整删的由 stream 报回，部分删除的
        // 已经画进新段片段，不论哪种都不再走第 5 步的整删块。
        for (oi, _) in old_slice {
            used_old[*oi] = true;
        }
        for (pool_index, fragments) in stream.new_fragments {
            let (ni, block_index) = new_slice[pool_index];
            used_new[ni] = true;
            push(
                new_position(new, new_blocks, ni),
                &mut order,
                &mut items,
                OverlayItem::Block(BlockOverlay {
                    block: new.blocks[block_index].clone(),
                    text: fragments,
                    table: None,
                    note: None,
                }),
            );
        }
        for pool_index in stream.deleted {
            let (oi, block_index) = old_slice[pool_index];
            let block = old.blocks[block_index].clone();
            let (text, table) = deleted_block_overlay(&block);
            push(
                placer.position(oi),
                &mut order,
                &mut items,
                OverlayItem::Deleted(BlockOverlay {
                    block,
                    text,
                    table,
                    note: None,
                }),
            );
        }
    }

    // 5. 其余未配对的块（图片、对齐行等）：旧的整体删除、新的整体新增。
    for (oi, &old_index) in old_blocks.iter().enumerate() {
        if used_old[oi] {
            continue;
        }
        used_old[oi] = true;
        let block = old.blocks[old_index].clone();
        let (text, table) = deleted_block_overlay(&block);
        push(
            placer.position(oi),
            &mut order,
            &mut items,
            OverlayItem::Deleted(BlockOverlay {
                block,
                text,
                table,
                note: None,
            }),
        );
    }
    for (ni, &new_index) in new_blocks.iter().enumerate() {
        if used_new[ni] {
            continue;
        }
        used_new[ni] = true;
        push(
            new_position(new, new_blocks, ni),
            &mut order,
            &mut items,
            added_block_item(new.blocks[new_index].clone()),
        );
    }

    items.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    items.into_iter().map(|(_, _, item)| item).collect()
}

fn added_block_item(block: VisualBlock) -> OverlayItem {
    if let Some((rows, spans)) = DocumentModel::table(&block) {
        return OverlayItem::Block(BlockOverlay {
            block: block.clone(),
            text: Vec::new(),
            table: Some(whole_table_overlay(rows, spans, RedlineKind::Added)),
            note: None,
        });
    }
    let text = block
        .text()
        .map(|text| vec![Fragment::added(text)])
        .unwrap_or_default();
    OverlayItem::Block(BlockOverlay {
        block,
        text,
        table: None,
        note: None,
    })
}

/// 组内第 ni 个块在新组文字流里的起始位置（字符数）。
fn new_position(new: &DocumentModel, new_blocks: &[usize], ni: usize) -> f64 {
    new_blocks[..ni]
        .iter()
        .map(|&index| block_weight(&new.blocks[index]))
        .sum::<usize>() as f64
}

/// 旧块在新组坐标里的定位器。锚段把两侧切成一一对应的夹缝（第 k 个夹缝在
/// 第 k 个锚段之前），旧块只在自己所在夹缝的新侧范围里按字数比例插空：
/// 整删块落在原来的两个邻段之间，不会被远处的增删带偏。
struct OldPlacer {
    /// 旧组块的流坐标前缀和（长度 = 块数 + 1）。
    old_prefix: Vec<usize>,
    /// 新组块的流坐标前缀和（长度 = 块数 + 1）。
    new_prefix: Vec<usize>,
    /// 锚段的组内块序号（旧, 新），两侧都单调递增。
    anchors: Vec<(usize, usize)>,
}

impl OldPlacer {
    fn new(
        old: &DocumentModel,
        new: &DocumentModel,
        old_blocks: &[usize],
        new_blocks: &[usize],
        anchors: Vec<(usize, usize)>,
    ) -> Self {
        let prefix = |model: &DocumentModel, blocks: &[usize]| {
            let mut sums = vec![0usize];
            for &index in blocks {
                sums.push(
                    sums.last().copied().unwrap_or_default() + block_weight(&model.blocks[index]),
                );
            }
            sums
        };
        Self {
            old_prefix: prefix(old, old_blocks),
            new_prefix: prefix(new, new_blocks),
            anchors,
        }
    }

    /// 组内第 oi 个旧块所在的夹缝号（它前面有几个锚段）。
    fn old_gap(&self, oi: usize) -> usize {
        self.anchors.partition_point(|&(a, _)| a < oi)
    }

    /// 组内第 ni 个新块所在的夹缝号。
    fn new_gap(&self, ni: usize) -> usize {
        self.anchors.partition_point(|&(_, b)| b < ni)
    }

    /// 组内第 oi 个旧块的位置：夹缝两侧的旧 / 新流坐标范围按比例换算。
    /// 夹缝的新侧为空时排在后一个锚段之前（前一个锚段至少占 1）。
    fn position(&self, oi: usize) -> f64 {
        let gap = self.old_gap(oi);
        let previous = gap.checked_sub(1).map(|k| self.anchors[k]);
        let next = self.anchors.get(gap).copied();
        let old_lo = previous.map_or(0, |(a, _)| self.old_prefix[a + 1]);
        let new_lo = previous.map_or(0, |(_, b)| self.new_prefix[b + 1]);
        let old_hi = next.map_or(*self.old_prefix.last().unwrap_or(&0), |(a, _)| {
            self.old_prefix[a]
        });
        let new_hi = next.map_or(*self.new_prefix.last().unwrap_or(&0), |(_, b)| {
            self.new_prefix[b]
        });
        let fraction = if old_hi > old_lo {
            (self.old_prefix[oi] - old_lo) as f64 / (old_hi - old_lo) as f64
        } else {
            0.0
        };
        if new_hi > new_lo {
            new_lo as f64 + fraction * (new_hi - new_lo) as f64
        } else {
            new_lo as f64 - 0.5 + fraction * 0.5
        }
    }
}

/// 块的「流坐标权重」：文本类取字数，表格取全部格文字数，其余取 1。
fn block_weight(block: &VisualBlock) -> usize {
    match DocumentModel::table(block) {
        Some((rows, _)) => rows.iter().flatten().map(|cell| cell.chars().count()).sum(),
        None => block
            .text()
            .map(|text| text.chars().count())
            .unwrap_or(1)
            .max(1),
    }
}

/// 新旧两组里未配对的表格按出现顺序两两配对；列数相同逐格词级，
/// 列数不同整表删旧插新（旧行全删线 + 新行全框，同一张 overlay 表达）。
#[allow(clippy::too_many_arguments)]
fn pair_tables(
    old: &DocumentModel,
    new: &DocumentModel,
    old_blocks: &[usize],
    new_blocks: &[usize],
    used_old: &mut [bool],
    used_new: &mut [bool],
    items: &mut Vec<(f64, usize, OverlayItem)>,
    order: &mut usize,
) {
    let is_table = |block: &VisualBlock| {
        matches!(
            block,
            VisualBlock::Parsed {
                block: MarkdownBlock::Table { .. },
                ..
            }
        )
    };
    let old_tables: Vec<usize> = old_blocks
        .iter()
        .enumerate()
        .filter(|(i, index)| !used_old[*i] && is_table(&old.blocks[**index]))
        .map(|(i, _)| i)
        .collect();
    let new_tables: Vec<usize> = new_blocks
        .iter()
        .enumerate()
        .filter(|(i, index)| !used_new[*i] && is_table(&new.blocks[**index]))
        .map(|(i, _)| i)
        .collect();
    for (offset, oi) in old_tables.iter().enumerate() {
        let Some(&ni) = new_tables.get(offset) else {
            continue;
        };
        used_old[*oi] = true;
        used_new[ni] = true;
        let overlay = diff_tables(&old.blocks[old_blocks[*oi]], &new.blocks[new_blocks[ni]]);
        items.push((
            new_position(new, new_blocks, ni),
            *order,
            OverlayItem::Block(BlockOverlay {
                block: new.blocks[new_blocks[ni]].clone(),
                text: Vec::new(),
                table: Some(overlay),
                note: None,
            }),
        ));
        *order += 1;
    }
}

/// 二分 LCS 的最大分数版：`score(a, b)` ≤ 0 表示不配对，其余为配对分
/// （相似度）。返回配对的下标对（按旧侧顺序）。分数最大化让 LCS 在
/// 「乙~乙′（改写）」和「丙~甲（模板相同的不同段）」之间选前者。
fn lcs_pairs(n: usize, m: usize, score: impl Fn(usize, usize) -> f32) -> Vec<(usize, usize)> {
    const EPS: f32 = 1e-6;
    let mut dp = vec![vec![0f32; m + 1]; n + 1];
    for i in 1..=n {
        for j in 1..=m {
            dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            let s = score(i - 1, j - 1);
            if s > 0.0 {
                dp[i][j] = dp[i][j].max(dp[i - 1][j - 1] + s);
            }
        }
    }
    let mut pairs = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 && j > 0 {
        let s = score(i - 1, j - 1);
        if s > 0.0 && (dp[i][j] - (dp[i - 1][j - 1] + s)).abs() < EPS {
            pairs.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if (dp[i][j] - dp[i - 1][j]).abs() < EPS {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    pairs.reverse();
    pairs
}

/// 大规模的段落锚定：先用 Myers 把规范化后一字不差、且允许锚定的段对齐，再在
/// 相邻两个锚点之间的夹缝里跑加权 LCS（[`lcs_pairs`]）。完全相同的段是加权 LCS
/// 里分数最高的配对，绝大多数情况下与全局算法得出同一组锚点，只是不再对全篇
/// 两两算相似度。
fn gapped_lcs_pairs(
    old_keys: &[String],
    new_keys: &[String],
    score: &impl Fn(usize, usize) -> f32,
) -> Vec<(usize, usize)> {
    // 夹缝 `old_start..old_end` × `new_start..new_end` 里的加权 LCS。
    let gap = |old_start: usize, old_end: usize, new_start: usize, new_end: usize| {
        lcs_pairs(old_end - old_start, new_end - new_start, |a, b| {
            score(old_start + a, new_start + b)
        })
        .into_iter()
        .map(move |(a, b)| (old_start + a, new_start + b))
    };
    let mut pairs = Vec::new();
    let (mut gap_old, mut gap_new) = (0usize, 0usize);
    for op in diff_ops(old_keys, new_keys) {
        if op.tag() != DiffTag::Equal {
            continue;
        }
        for (a, b) in op.old_range().zip(op.new_range()) {
            // 相同却不许锚定的段（如空段）留在夹缝里，交给加权 LCS 处理。
            if score(a, b) <= 0.0 {
                continue;
            }
            pairs.extend(gap(gap_old, a, gap_new, b));
            pairs.push((a, b));
            gap_old = a + 1;
            gap_new = b + 1;
        }
    }
    pairs.extend(gap(gap_old, old_keys.len(), gap_new, new_keys.len()));
    pairs
}

/// 同类块按相似度贪心配对（列表项用），返回 (旧组序号, 新组序号)。
#[allow(clippy::too_many_arguments)]
fn best_pairs(
    old: &DocumentModel,
    new: &DocumentModel,
    old_blocks: &[usize],
    new_blocks: &[usize],
    used_old: &[bool],
    used_new: &[bool],
    kind: fn(&VisualBlock) -> bool,
    threshold: f32,
) -> Vec<(usize, usize)> {
    let mut candidates: Vec<(f32, usize, usize)> = Vec::new();
    for (oi, &old_index) in old_blocks.iter().enumerate() {
        if used_old[oi] || !kind(&old.blocks[old_index]) {
            continue;
        }
        let old_text = old.blocks[old_index].text().unwrap_or_default();
        for (ni, &new_index) in new_blocks.iter().enumerate() {
            if used_new[ni] || !kind(&new.blocks[new_index]) {
                continue;
            }
            let score = similarity(old_text, new.blocks[new_index].text().unwrap_or_default());
            if score >= threshold {
                candidates.push((score, oi, ni));
            }
        }
    }
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut pairs = Vec::new();
    let mut old_taken = vec![false; old_blocks.len()];
    let mut new_taken = vec![false; new_blocks.len()];
    for (_, oi, ni) in candidates {
        if old_taken[oi] || new_taken[ni] {
            continue;
        }
        old_taken[oi] = true;
        new_taken[ni] = true;
        pairs.push((oi, ni));
    }
    pairs
}

// ── 表格（规则 9）────────────────────────────────────────────────────────────

/// 表格配对：行按整行文字 LCS 对齐，配上的行逐格词级，配不上的整行增删；
/// 列数不同整表删旧插新（旧行全删线 + 新行全框排在同一张 overlay 里）。
fn diff_tables(old_block: &VisualBlock, new_block: &VisualBlock) -> TableOverlay {
    let (old_rows, old_spans) = DocumentModel::table(old_block).unwrap_or((&[], &[]));
    let (new_rows, new_spans) = DocumentModel::table(new_block).unwrap_or((&[], &[]));
    let columns = new_rows.first().map_or(0, Vec::len);
    let old_columns = old_rows.first().map_or(0, Vec::len);
    if columns == 0 || old_columns != columns {
        let split_after = old_rows.len();
        let mut rows = whole_table_overlay(old_rows, old_spans, RedlineKind::Deleted).rows;
        rows.extend(whole_table_overlay(new_rows, new_spans, RedlineKind::Added).rows);
        return TableOverlay {
            columns,
            rows,
            spans: Vec::new(),
            old_spans: old_spans.to_vec(),
            split_after: Some(split_after),
        };
    }
    let old_keys: Vec<String> = old_rows
        .iter()
        .map(|row| normalize_key(&row.concat()))
        .collect();
    let new_keys: Vec<String> = new_rows
        .iter()
        .map(|row| normalize_key(&row.concat()))
        .collect();
    let ops = diff_ops(&old_keys, &new_keys);
    let mut rows: Vec<RowOverlay> = Vec::new();
    for op in ops {
        match op.tag() {
            DiffTag::Equal => {
                for source_row in op.new_range() {
                    rows.push(RowOverlay {
                        deleted: false,
                        source_row,
                        cells: new_rows[source_row]
                            .iter()
                            .map(|cell| vec![Fragment::same(cell.as_str())])
                            .collect(),
                    });
                }
            }
            DiffTag::Delete => {
                for source_row in op.old_range() {
                    rows.push(RowOverlay {
                        deleted: true,
                        source_row,
                        cells: old_rows[source_row]
                            .iter()
                            .map(|cell| vec![Fragment::deleted(cell.as_str())])
                            .collect(),
                    });
                }
            }
            DiffTag::Insert => {
                for source_row in op.new_range() {
                    rows.push(RowOverlay {
                        deleted: false,
                        source_row,
                        cells: new_rows[source_row]
                            .iter()
                            .map(|cell| vec![Fragment::added(cell.as_str())])
                            .collect(),
                    });
                }
            }
            DiffTag::Replace => {
                let old_range = op.old_range();
                let new_range = op.new_range();
                let new_len = new_range.len();
                for (offset, source_row) in new_range.enumerate() {
                    if offset < old_range.len() {
                        let old_cells = &old_rows[old_range.start + offset];
                        rows.push(RowOverlay {
                            deleted: false,
                            source_row,
                            cells: new_rows[source_row]
                                .iter()
                                .enumerate()
                                .map(|(column, cell)| diff_texts(&old_cells[column], cell))
                                .collect(),
                        });
                    } else {
                        rows.push(RowOverlay {
                            deleted: false,
                            source_row,
                            cells: new_rows[source_row]
                                .iter()
                                .map(|cell| vec![Fragment::added(cell.as_str())])
                                .collect(),
                        });
                    }
                }
                // 旧行比新行多出来的部分整行删除，排在新行之后。
                let extra_start = old_range.start + new_len;
                for (offset, row) in old_rows[extra_start..old_range.end].iter().enumerate() {
                    rows.push(RowOverlay {
                        deleted: true,
                        source_row: extra_start + offset,
                        cells: row
                            .iter()
                            .map(|cell| vec![Fragment::deleted(cell.as_str())])
                            .collect(),
                    });
                }
            }
        }
    }
    TableOverlay {
        columns,
        rows,
        spans: new_spans.to_vec(),
        old_spans: old_spans.to_vec(),
        split_after: None,
    }
}

// ── 文字流比较（规则 2、3、4、10）────────────────────────────────────────────

struct StreamOutcome {
    /// 新池里每个正文段的片段（下标对齐 new_pool）。
    new_fragments: Vec<(usize, Vec<Fragment>)>,
    /// 整段被删的旧池下标。
    deleted: Vec<usize>,
}

/// 把若干正文段拼成一条字符流做词级 diff，再按新版段落切回去。段落边界是
/// 软分隔：拆分 / 合并（文字没变）天然不产生标记。整段被删的旧段返回为
/// 整删块；部分删除的文字留在原位，画进相邻新段的片段序列里。
fn stream_diff(
    old: &DocumentModel,
    new: &DocumentModel,
    old_pool: &[(usize, usize)],
    new_pool: &[(usize, usize)],
) -> StreamOutcome {
    if new_pool.is_empty() {
        return StreamOutcome {
            new_fragments: Vec::new(),
            deleted: (0..old_pool.len()).collect(),
        };
    }
    if old_pool.is_empty() {
        return StreamOutcome {
            new_fragments: new_pool
                .iter()
                .enumerate()
                .map(|(index, (_, block_index))| {
                    let text = new.blocks[*block_index].text().unwrap_or_default();
                    (index, vec![Fragment::added(text)])
                })
                .collect(),
            deleted: Vec::new(),
        };
    }
    // 拆分 / 合并的快速通道：两池文字流拼接后完全相同（仅段落边界不同），
    // 整池视为未改动。逐 token 比较会把挪过边界的句读标点标成增删。
    let old_concat: String = old_pool
        .iter()
        .map(|(_, index)| old.blocks[*index].text().unwrap_or_default())
        .collect();
    let new_concat: String = new_pool
        .iter()
        .map(|(_, index)| new.blocks[*index].text().unwrap_or_default())
        .collect();
    if normalize_key(&old_concat) == normalize_key(&new_concat) {
        return StreamOutcome {
            new_fragments: new_pool
                .iter()
                .enumerate()
                .map(|(index, (_, block_index))| {
                    let text = new.blocks[*block_index].text().unwrap_or_default();
                    (index, vec![Fragment::same(text)])
                })
                .collect(),
            deleted: Vec::new(),
        };
    }
    let old_texts: Vec<&str> = old_pool
        .iter()
        .map(|(_, index)| old.blocks[*index].text().unwrap_or_default())
        .collect();
    let new_texts: Vec<&str> = new_pool
        .iter()
        .map(|(_, index)| new.blocks[*index].text().unwrap_or_default())
        .collect();
    let flat_old = flatten_tokens(&old_texts);
    let flat_new = flatten_tokens(&new_texts);
    let old_keys: Vec<String> = flat_old
        .tokens
        .iter()
        .map(|token| token.key.clone())
        .collect();
    let new_keys: Vec<String> = flat_new
        .tokens
        .iter()
        .map(|token| token.key.clone())
        .collect();
    let ops = diff_ops(&old_keys, &new_keys);

    let mut fragments: Vec<Vec<Fragment>> = vec![Vec::new(); new_pool.len()];
    let mut deleted: Vec<usize> = Vec::new();
    let mut pending_inline: Vec<Fragment> = Vec::new();

    // 把「上一个 Delete 操作遗留在原位」的片段先挂到本操作第一个新 token 所属段。
    let flush_pending =
        |fragments: &mut Vec<Vec<Fragment>>, pending: &mut Vec<Fragment>, owner: usize| {
            for fragment in pending.drain(..) {
                push_fragment(&mut fragments[owner], fragment);
            }
        };

    for op in ops {
        match op.tag() {
            DiffTag::Equal | DiffTag::Insert => {
                if !pending_inline.is_empty() {
                    let owner = flat_new.owner[op.new_range().start];
                    flush_pending(&mut fragments, &mut pending_inline, owner);
                }
                let kind = if op.tag() == DiffTag::Equal {
                    RedlineKind::Same
                } else {
                    RedlineKind::Added
                };
                let mut run_owner: Option<usize> = None;
                for index in op.new_range() {
                    let owner = flat_new.owner[index];
                    if run_owner != Some(owner) {
                        run_owner = Some(owner);
                    }
                    push_fragment(
                        &mut fragments[owner],
                        Fragment {
                            kind,
                            text: flat_new.tokens[index].text.clone(),
                        },
                    );
                }
            }
            DiffTag::Delete => {
                collect_deleted_run(&flat_old, op.old_range(), &mut deleted, &mut pending_inline);
            }
            DiffTag::Replace => {
                if !pending_inline.is_empty() {
                    let owner = flat_new.owner[op.new_range().start];
                    flush_pending(&mut fragments, &mut pending_inline, owner);
                }
                let text: String = op
                    .old_range()
                    .map(|index| flat_old.tokens[index].text.as_str())
                    .collect();
                let owner = flat_new.owner[op.new_range().start];
                push_fragment(&mut fragments[owner], Fragment::deleted(text));
                for index in op.new_range() {
                    let owner = flat_new.owner[index];
                    push_fragment(
                        &mut fragments[owner],
                        Fragment::added(flat_new.tokens[index].text.clone()),
                    );
                }
            }
        }
    }
    // 流尾的部分删除：挂到最后一个非空新段（没有则第一个）。
    if !pending_inline.is_empty() {
        let owner = fragments
            .iter()
            .rposition(|list| !list.is_empty())
            .unwrap_or(0);
        flush_pending(&mut fragments, &mut pending_inline, owner);
    }

    let new_fragments = fragments
        .into_iter()
        .enumerate()
        .map(|(index, list)| {
            let mut list = merge_fragments(list);
            apply_merge_rules(&mut list);
            (index, list)
        })
        .collect();
    StreamOutcome {
        new_fragments,
        deleted,
    }
}

/// 平铺的 token 流：每段切词后首尾相接，`owner` 记每个 token 属于第几段，
/// `seg_start` 记每段第一个 token 的流下标。
struct FlatTokens {
    tokens: Vec<Token>,
    owner: Vec<usize>,
    seg_start: Vec<usize>,
}

fn flatten_tokens(texts: &[&str]) -> FlatTokens {
    let mut flat = FlatTokens {
        tokens: Vec::new(),
        owner: Vec::new(),
        seg_start: Vec::new(),
    };
    for (index, text) in texts.iter().enumerate() {
        flat.seg_start.push(flat.tokens.len());
        for token in tokenize(text) {
            flat.tokens.push(token);
            flat.owner.push(index);
        }
    }
    flat
}

/// 处理一个 Delete 操作：整段被覆盖的进 `deleted`（整删块），段边缘的
/// 部分覆盖留在原位（`pending_inline`，由调用方挂到下一个新段上）。
fn collect_deleted_run(
    flat_old: &FlatTokens,
    range: std::ops::Range<usize>,
    deleted: &mut Vec<usize>,
    pending_inline: &mut Vec<Fragment>,
) {
    let mut cursor = range.start;
    for (segment, &start) in flat_old.seg_start.iter().enumerate() {
        let end = flat_old
            .seg_start
            .get(segment + 1)
            .copied()
            .unwrap_or(flat_old.tokens.len());
        if cursor >= range.end {
            break;
        }
        if end <= cursor || start >= range.end {
            continue;
        }
        let piece_start = cursor.max(start);
        let piece_end = range.end.min(end);
        let text: String = (piece_start..piece_end)
            .map(|index| flat_old.tokens[index].text.as_str())
            .collect();
        if piece_start == start && piece_end == end {
            // 整段覆盖：旧段成整删块。
            deleted.push(segment);
        } else if !text.is_empty() {
            push_fragment(pending_inline, Fragment::deleted(text));
        }
        cursor = piece_end;
    }
}

/// 片段追加：相邻同类合并，避免碎块。
fn push_fragment(list: &mut Vec<Fragment>, fragment: Fragment) {
    if fragment.text.is_empty() {
        return;
    }
    if let Some(last) = list.last_mut()
        && last.kind == fragment.kind
    {
        last.text.push_str(&fragment.text);
        return;
    }
    list.push(fragment);
}

/// 相邻同类合并后的片段序列。
fn merge_fragments(list: Vec<Fragment>) -> Vec<Fragment> {
    let mut out: Vec<Fragment> = Vec::new();
    for fragment in list {
        push_fragment(&mut out, fragment);
    }
    out
}

/// 词级比较两段文字（标题、列表项、移动段、表格单元格共用），含归并规则。
fn diff_texts(old_text: &str, new_text: &str) -> Vec<Fragment> {
    let old_tokens = tokenize(old_text);
    let new_tokens = tokenize(new_text);
    let old_keys: Vec<String> = old_tokens.iter().map(|token| token.key.clone()).collect();
    let new_keys: Vec<String> = new_tokens.iter().map(|token| token.key.clone()).collect();
    let ops = diff_ops(&old_keys, &new_keys);
    let mut fragments = Vec::new();
    for op in ops {
        match op.tag() {
            DiffTag::Equal => {
                let text: String = op
                    .new_range()
                    .map(|index| new_tokens[index].text.as_str())
                    .collect();
                push_fragment(&mut fragments, Fragment::same(text));
            }
            DiffTag::Delete => {
                let text: String = op
                    .old_range()
                    .map(|index| old_tokens[index].text.as_str())
                    .collect();
                push_fragment(&mut fragments, Fragment::deleted(text));
            }
            DiffTag::Insert => {
                let text: String = op
                    .new_range()
                    .map(|index| new_tokens[index].text.as_str())
                    .collect();
                push_fragment(&mut fragments, Fragment::added(text));
            }
            DiffTag::Replace => {
                let old_part: String = op
                    .old_range()
                    .map(|index| old_tokens[index].text.as_str())
                    .collect();
                let new_part: String = op
                    .new_range()
                    .map(|index| new_tokens[index].text.as_str())
                    .collect();
                push_fragment(&mut fragments, Fragment::deleted(old_part));
                push_fragment(&mut fragments, Fragment::added(new_part));
            }
        }
    }
    apply_merge_rules(&mut fragments);
    fragments
}

// ── 归并规则（规则 4）────────────────────────────────────────────────────────

/// 夹缝吸收：两个改动片段之间 ≤ 2 个未改动字，吸收进改动合成一段。
///
/// 做法是把「改动 + 短夹缝 + 改动 …」连成一个区域，整体改写成「删旧
/// 文字、插新文字」：旧文字 = 区域里去掉 Added 的部分，新文字 = 去掉
/// Deleted 的部分。这样不论夹缝两侧是删 / 增、删 / 删还是增 / 增，不变式
/// （去 Added 得旧文、去 Deleted 得新文）都按构造成立。
///
/// 只有同时含删和增的区域才吸收：纯删（「请各~有关~单位~认真~组织」）或
/// 纯增本来就清楚，硬并成「删一整串、再加回夹缝」反而难读。改写后再按
/// 词剥掉两侧相同的首尾，免得夹缝在删、增两边各印一遍。
fn absorb_gaps(fragments: &mut Vec<Fragment>) {
    let mut out: Vec<Fragment> = Vec::with_capacity(fragments.len());
    let mut index = 0usize;
    while index < fragments.len() {
        if fragments[index].is_same() {
            push_fragment(&mut out, fragments[index].clone());
            index += 1;
            continue;
        }
        // 区域：从这个改动起，吞掉后面「短夹缝 + 改动」的组合。
        let mut end = index;
        loop {
            while end < fragments.len() && !fragments[end].is_same() {
                end += 1;
            }
            let gap_then_change = end + 1 < fragments.len()
                && fragments[end].char_count() <= MERGE_GAP_CHARS
                && !fragments[end + 1].is_same();
            if !gap_then_change {
                break;
            }
            end += 1;
        }
        let region = &fragments[index..end];
        let has_gap = region.iter().any(Fragment::is_same);
        let mixed = region.iter().any(|f| f.kind == RedlineKind::Deleted)
            && region.iter().any(|f| f.kind == RedlineKind::Added);
        if has_gap && mixed {
            let old_text: String = region
                .iter()
                .filter(|f| f.kind != RedlineKind::Added)
                .map(|f| f.text.as_str())
                .collect();
            let new_text: String = region
                .iter()
                .filter(|f| f.kind != RedlineKind::Deleted)
                .map(|f| f.text.as_str())
                .collect();
            for fragment in replacement(&old_text, &new_text) {
                push_fragment(&mut out, fragment);
            }
        } else {
            for fragment in region {
                push_fragment(&mut out, fragment.clone());
            }
        }
        index = end;
    }
    *fragments = out;
}

/// 「删 old、插 new」，按词剥掉两边相同的首尾（剥下来的是未改动文字）。
fn replacement(old_text: &str, new_text: &str) -> Vec<Fragment> {
    let old_tokens = tokenize(old_text);
    let new_tokens = tokenize(new_text);
    let prefix = old_tokens
        .iter()
        .zip(&new_tokens)
        .take_while(|(a, b)| a.text == b.text)
        .count();
    let max_suffix = old_tokens.len().min(new_tokens.len()) - prefix;
    let suffix = old_tokens
        .iter()
        .rev()
        .zip(new_tokens.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a.text == b.text)
        .count();
    let join = |tokens: &[Token]| -> String { tokens.iter().map(|t| t.text.as_str()).collect() };
    [
        Fragment::same(join(&new_tokens[..prefix])),
        Fragment::deleted(join(&old_tokens[prefix..old_tokens.len() - suffix])),
        Fragment::added(join(&new_tokens[prefix..new_tokens.len() - suffix])),
        Fragment::same(join(&new_tokens[new_tokens.len() - suffix..])),
    ]
    .into_iter()
    .filter(|fragment| !fragment.text.is_empty())
    .collect()
}

/// 删、增文字完全相同的相邻片段（「~要~[要]」）抵消成未改动。
fn cancel_identical_pairs(fragments: &mut Vec<Fragment>) {
    let mut out: Vec<Fragment> = Vec::with_capacity(fragments.len());
    for fragment in fragments.drain(..) {
        if let Some(last) = out.last()
            && !last.is_same()
            && !fragment.is_same()
            && last.kind != fragment.kind
            && last.text == fragment.text
        {
            let same = Fragment::same(out.pop().expect("刚取过").text);
            push_fragment(&mut out, same);
            continue;
        }
        push_fragment(&mut out, fragment);
    }
    *fragments = out;
}

/// 分句切点：句读字符（，。；：！？）。
fn is_sentence_break(ch: char) -> bool {
    matches!(
        ch,
        '，' | '。' | '；' | '：' | '！' | '？' | ',' | ';' | ':' | '!' | '?'
    )
}

/// 把片段在句读处切开：未改动的 Same 片段常常跨过好几个句号，不切开就
/// 会把整段划进一个分句区，归并阈值必然被触发（整段替换的退化）。
fn split_at_sentence_breaks(fragments: Vec<Fragment>) -> Vec<Fragment> {
    let mut out = Vec::with_capacity(fragments.len());
    for fragment in fragments {
        let mut piece = String::new();
        for ch in fragment.text.chars() {
            piece.push(ch);
            if is_sentence_break(ch) {
                out.push(Fragment {
                    kind: fragment.kind,
                    text: std::mem::take(&mut piece),
                });
            }
        }
        if !piece.is_empty() {
            out.push(Fragment {
                kind: fragment.kind,
                text: piece,
            });
        }
    }
    out
}

/// 分句归并：按句读（，。；：！？）切分，改动字数过半或改动片段 ≥ 3 段，
/// 整个分句改为「删旧句、插新句」。删旧句的文字由 `Deleted` 与 `Same`
/// 片段拼回，插新句由 `Same` 与 `Added` 片段拼成。
fn replace_heavy_sentences(fragments: &mut Vec<Fragment>) {
    let mut regions: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    for (index, fragment) in fragments.iter().enumerate() {
        let ends_sentence = fragment.text.chars().last().is_some_and(is_sentence_break);
        if ends_sentence {
            regions.push((start, index + 1));
            start = index + 1;
        }
    }
    if start < fragments.len() {
        regions.push((start, fragments.len()));
    }
    for (begin, end) in regions.into_iter().rev() {
        let region = &fragments[begin..end];
        let total: usize = region.iter().map(Fragment::char_count).sum();
        if total == 0 {
            continue;
        }
        // 改动字数按删 / 增两侧较大值计：整 token 替换（如日期）两侧各算一遍
        // 会把改动率虚增到超过一半，误触发整句替换。
        let deleted: usize = region
            .iter()
            .filter(|fragment| fragment.kind == RedlineKind::Deleted)
            .map(Fragment::char_count)
            .sum();
        let added: usize = region
            .iter()
            .filter(|fragment| fragment.kind == RedlineKind::Added)
            .map(Fragment::char_count)
            .sum();
        let changed = deleted.max(added);
        let pieces = region.iter().filter(|fragment| !fragment.is_same()).count();
        let heavy = changed * SENTENCE_CHANGE_DEN > total * SENTENCE_CHANGE_NUM
            || pieces >= SENTENCE_FRAGMENT_LIMIT;
        if !heavy {
            continue;
        }
        let old_text: String = region
            .iter()
            .filter(|fragment| fragment.kind != RedlineKind::Added)
            .map(|fragment| fragment.text.as_str())
            .collect();
        let new_text: String = region
            .iter()
            .filter(|fragment| fragment.kind != RedlineKind::Deleted)
            .map(|fragment| fragment.text.as_str())
            .collect();
        fragments.splice(begin..end, sentence_replacement(&old_text, &new_text));
    }
}

/// 整句「删旧句、插新句」，但两句共有的句首 / 句尾标点留作未改动：旧句
/// 「基于本地模型的。」换成「。」时，只删前面的字，句号不该删了再加一遍
/// ——那样排出来是一个单独套框的「。」，还可能被挤到下一行行首。
fn sentence_replacement(old_text: &str, new_text: &str) -> Vec<Fragment> {
    let is_mark = |ch: char| is_sentence_break(ch) || ch.is_whitespace();
    let old_chars: Vec<char> = old_text.chars().collect();
    let new_chars: Vec<char> = new_text.chars().collect();
    let prefix = old_chars
        .iter()
        .zip(&new_chars)
        .take_while(|(a, b)| a == b && is_mark(**a))
        .count();
    let max_suffix = old_chars.len().min(new_chars.len()) - prefix;
    let suffix = old_chars
        .iter()
        .rev()
        .zip(new_chars.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b && is_mark(**a))
        .count();
    let text = |chars: &[char]| chars.iter().collect::<String>();
    [
        Fragment::same(text(&new_chars[..prefix])),
        Fragment::deleted(text(&old_chars[prefix..old_chars.len() - suffix])),
        Fragment::added(text(&new_chars[prefix..new_chars.len() - suffix])),
        Fragment::same(text(&new_chars[new_chars.len() - suffix..])),
    ]
    .into_iter()
    .filter(|fragment| !fragment.text.is_empty())
    .collect()
}

fn apply_merge_rules(fragments: &mut Vec<Fragment>) {
    cancel_identical_pairs(fragments);
    absorb_gaps(fragments);
    // 句读切分在夹缝吸收之后做：夹缝复制出的文字也要参与分句判定。
    let split = split_at_sentence_breaks(std::mem::take(fragments));
    *fragments = split;
    replace_heavy_sentences(fragments);
    let merged = merge_fragments(std::mem::take(fragments));
    *fragments = merged;
    cancel_identical_pairs(fragments);
}

// ── 相似度与归一化 ────────────────────────────────────────────────────────────

/// 两段文字共有的字符数（字符多重集交集）。
fn common_chars(a: &str, b: &str) -> usize {
    let mut counts: HashMap<char, usize> = HashMap::new();
    for ch in a.chars().filter(|ch| !ch.is_whitespace()) {
        *counts.entry(ch).or_default() += 1;
    }
    let mut common = 0usize;
    for ch in b.chars().filter(|ch| !ch.is_whitespace()) {
        let entry = counts.entry(ch).or_default();
        if *entry > 0 {
            *entry -= 1;
            common += 1;
        }
    }
    common
}

/// 两段文字是否存在包含关系（短的是长的子串）：拆分 / 合并的 signature，
/// 这类配对既不锚定也不算移动，交给文字流比较。
fn containment(a: &str, b: &str) -> bool {
    let (long, short) = if a.chars().count() >= b.chars().count() {
        (a, b)
    } else {
        (b, a)
    };
    !short.is_empty() && long.contains(short)
}

/// 两段文字的相似度：字符 Dice 系数（移动识别与块配对用）。
fn similarity(a: &str, b: &str) -> f32 {
    let a_len = a.chars().filter(|ch| !ch.is_whitespace()).count();
    let b_len = b.chars().filter(|ch| !ch.is_whitespace()).count();
    if a_len == 0 && b_len == 0 {
        return 1.0;
    }
    if a_len == 0 || b_len == 0 {
        return 0.0;
    }
    2.0 * common_chars(a, b) as f32 / (a_len + b_len) as f32
}

/// 比较键：剥掉空白（规则 5：空格差异不标；标点保留，改了照标）。
fn normalize_key(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_whitespace()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(old: &str, new: &str) -> RedlineOverlay {
        diff_documents(
            &DocumentModel::from_markdown(old),
            &DocumentModel::from_markdown(new),
        )
    }

    #[test]
    fn identical_documents_produce_an_empty_overlay() {
        let overlay = diff("第一段。\n\n第二段。", "第一段。\n\n第二段。");
        assert!(overlay.is_empty());
    }

    #[test]
    fn a_word_level_change_marks_only_that_word() {
        let overlay = diff(
            "同意你单位关于报送的请示。",
            "同意你单位关于开展检查的请示。",
        );
        assert_eq!(overlay.readable(), "同意你单位关于~报送~[开展检查]的请示。");
    }

    #[test]
    fn changing_a_date_replaces_it_whole() {
        // 规则 3：日期是整 token，「10日」改「15日」整体删旧插新。
        let overlay = diff("请于8月10日前报送。", "请于8月15日前报送。");
        assert_eq!(overlay.readable(), "请于~8月10日~[8月15日]前报送。");
    }

    #[test]
    fn changing_a_formula_replaces_it_whole() {
        // 规则 10：公式不拆。
        let overlay = diff("由$x^{2}$可知。", "由$x^{3}$可知。");
        assert_eq!(overlay.readable(), "由~$x^{2}$~[$x^{3}$]可知。");
    }

    #[test]
    fn a_tiny_gap_between_changes_is_absorbed() {
        // 规则 4：夹缝 ≤ 2 字吸收进改动，两处改动连成一片。
        let overlay = diff("加强组织领导。", "加强组织领导和统筹协调。");
        assert_eq!(overlay.readable(), "加强组织领导[和统筹协调]。");
    }

    #[test]
    fn a_sentence_with_many_fragments_is_replaced_whole() {
        // 规则 4：一个分句里改动片段 ≥ 3 段，整句删旧插新（避免红绿交错）；
        // 两句共有的句尾标点不删了再加。
        let overlay = diff("甲、乙、丙工作。", "A、乙、B工作。");
        assert_eq!(overlay.readable(), "~甲、乙、丙工作~[A、乙、B工作]。");
    }

    #[test]
    fn a_heavily_rewritten_sentence_is_replaced_whole() {
        // 规则 4：分句改动过半（按删 / 增两侧较大值计），整句删旧插新；
        // 共有的句尾标点留作未改动。
        let overlay = diff("要认真履行职责。", "务必严格履行自身职责。");
        assert_eq!(
            overlay.readable(),
            "~要认真履行职责~[务必严格履行自身职责]。"
        );
    }

    #[test]
    fn a_light_sentence_stays_word_level() {
        let overlay = diff(
            "同意你单位关于报送的请示。",
            "同意你单位关于开展检查的请示。",
        );
        assert!(
            overlay.readable().contains("~报送~[开展检查]"),
            "轻改仍应词级：{}",
            overlay.readable()
        );
    }

    #[test]
    fn paragraph_split_and_merge_stay_unmarked() {
        // 规则 2：段落边界是软分隔——文字流拼接后完全相同的拆分 / 合并
        // 不产生任何标记。
        let overlay = diff("第一段。第二段。", "第一段。\n\n第二段。");
        assert!(
            overlay.is_empty(),
            "拆分不应产生标记：{}",
            overlay.readable()
        );
        let overlay = diff("第一段。\n\n第二段。", "第一段。第二段。");
        assert!(
            overlay.is_empty(),
            "合并不应产生标记：{}",
            overlay.readable()
        );
    }

    #[test]
    fn a_split_with_punctuation_change_stays_word_level() {
        // 拆分点挪动了句读（旧段无句号、新段补句号）：就地锚定后按词级标注
        // 补出的标点，不整段替换、不出移动注记。
        let overlay = diff(
            "第一段第二段。",
            "第一段。

第二段。",
        );
        let readable = overlay.readable();
        assert!(!readable.contains("移来"), "拆分不该出移动注记：{readable}");
        assert!(
            !readable.starts_with('~') && !readable.starts_with('['),
            "不该整段删旧插新：{readable}"
        );
    }

    #[test]
    fn a_moved_paragraph_gets_a_note_instead_of_marks() {
        // 规则 6：整段移动，新位置正常排出 + 注记，原位置不再画删除线。
        // 用真实长度的段落：LCS 锚住没动的两段，只有真正挪动的段配成移动。
        let old = "第一段内容比较长一些用于识别。\n\n第二段内容也比较长用于识别移动。\n\n第三段内容同样足够长可以识别。";
        let new = "第三段内容同样足够长可以识别。\n\n第一段内容比较长一些用于识别。\n\n第二段内容也比较长用于识别移动。";
        let overlay = diff(old, new);
        let readable = overlay.readable();
        assert!(
            readable.contains("第三段内容同样足够长可以识别。（注：本段由原第3段移来）"),
            "移动段应带注记：{readable}"
        );
        assert_eq!(
            readable.matches("移来").count(),
            1,
            "只有真正挪动的段才有注记：{readable}"
        );
        assert!(!readable.contains('~'), "原位置不该再画删除线：{readable}");
    }

    #[test]
    fn a_moved_and_edited_paragraph_shows_inner_marks() {
        // 乙就地改写（无注记），丙挪动（注记）：两者的判定都只看相对位置
        // 与文字，不看「第几个槽」。
        let old = "甲段内容。\n\n乙段内容比较长。\n\n丙段内容。";
        let new = "甲段内容。\n\n丙段内容。\n\n乙段新内容比较长。";
        let overlay = diff(old, new);
        let readable = overlay.readable();
        // 一种改写一种挪动时只有一个移动注记（哪段携带取决于 LCS 对齐，
        // 两种对齐都只有一个注记）；挪动的段不再画整段删除线。
        assert_eq!(
            readable.matches("移来").count(),
            1,
            "恰好一个移动注记：{readable}"
        );
        assert!(
            !readable.contains("~丙段内容。~"),
            "挪动的段不该有整段删除线：{readable}"
        );
    }

    #[test]
    fn an_in_place_rewrite_is_not_called_a_move() {
        // 就地改写（同位置槽）只标词级，不加移动注记。
        let overlay = diff("请各单位按时报送材料。", "请各单位按时报送完整材料。");
        let readable = overlay.readable();
        assert!(
            !readable.contains("移来"),
            "就地改写不该有移动注记：{readable}"
        );
        assert!(readable.contains("[完整]"), "改写部分应加框：{readable}");
    }

    #[test]
    fn heading_text_changes_are_marked_in_place() {
        // 规则 8：标题只比文字；编号程序生成，不出现在标注里。
        let overlay = diff("## 报送内容\n\n正文。", "## 报送要求\n\n正文。");
        assert_eq!(overlay.readable(), "报送~内容~[要求]\n\n正文。");
    }

    #[test]
    fn an_added_heading_is_whole_added() {
        let overlay = diff("## 甲\n\n正文。", "## 甲\n\n## 乙\n\n正文。");
        assert!(overlay.readable().contains("[乙]"), "新增标题整项加框");
    }

    #[test]
    fn a_deleted_heading_keeps_its_text_without_a_number() {
        let overlay = diff("## 甲\n\n## 乙\n\n正文。", "## 甲\n\n正文。");
        let readable = overlay.readable();
        assert!(
            readable.contains("~乙~"),
            "删除的标题文字画删除线：{readable}"
        );
        // 标注层里没有编号，序列化时才决定不排成标题（见 serialize.rs）。
    }

    #[test]
    fn list_item_renumbering_is_not_marked() {
        // 规则 8：列表编号程序生成，中间插入一项只标那一项。
        let overlay = diff("- 甲。\n- 乙。", "- 甲。\n- 新增项。\n- 乙。");
        let readable = overlay.readable();
        assert!(readable.contains("[新增项。]"), "新增项应加框：{readable}");
        assert!(
            !readable.contains("~乙。~"),
            "顺移的乙项不该画删除线：{readable}"
        );
    }

    #[test]
    fn table_cells_are_compared_word_by_word() {
        // 规则 9：逐格词级。
        let old = "| 事项 | 时限 |\n| --- | --- |\n| 备案 | 8月10日 |";
        let new = "| 事项 | 时限 |\n| --- | --- |\n| 备案 | 9月15日 |";
        let readable = diff(old, new).readable();
        assert!(
            readable.contains("~8月10日~[9月15日]"),
            "单元格整体日期替换：{readable}"
        );
        assert!(readable.contains("| 事项 | 时限 |"), "表头原样：{readable}");
    }

    #[test]
    fn table_rows_added_and_deleted_are_marked_whole_rows() {
        let header = "| 事项 | 时限 |\n| --- | --- |\n";
        let added = diff(
            &(header.to_string() + "| 甲 | 8月 |"),
            &(header.to_string() + "| 甲 | 8月 |\n| 乙 | 9月 |"),
        );
        let readable = added.readable();
        assert!(
            readable.contains("| [乙] | [9月] |"),
            "新增整行加框：{readable}"
        );
        let deleted = diff(
            &(header.to_string() + "| 甲 | 8月 |\n| 乙 | 9月 |"),
            &(header.to_string() + "| 甲 | 8月 |"),
        );
        let readable = deleted.readable();
        assert!(
            readable.contains("| ~乙~ | ~9月~ |"),
            "删除整行画删除线：{readable}"
        );
    }

    #[test]
    fn a_table_with_a_different_column_count_is_replaced_whole() {
        let old = "| 事项 | 时限 |\n| --- | --- |\n| 备案 | 8月 |";
        let new = "| 事项 | 时限 | 责任人 |\n| --- | --- | --- |\n| 备案 | 8月 | 张三 |";
        let readable = diff(old, new).readable();
        assert!(
            readable.contains("~备案~") && readable.contains("[张三]"),
            "列数变化整表删旧插新：{readable}"
        );
    }

    #[test]
    fn an_element_field_change_replaces_the_value_whole() {
        // 规则 7：要素整体替换，不做词级。
        let mut old_input = crate::models::DraftInput::default();
        old_input.profile.recipient = "市财政局。".into();
        let mut new_input = old_input.clone();
        new_input.profile.recipient = "市发展改革委。".into();
        let overlay = diff_documents(
            &DocumentModel::from_inputs(&old_input, "正文。"),
            &DocumentModel::from_inputs(&new_input, "正文。"),
        );
        let readable = overlay.readable();
        assert!(
            readable.contains("~市财政局。~") && readable.contains("[市发展改革委。]"),
            "要素旧值删除线、新值加框：{readable}"
        );
        assert!(!readable.contains("~市财政~"), "要素不做词级：{readable}");
    }

    #[test]
    fn punctuation_changes_are_marked() {
        // 规则 5：标点是真实文字，改了照标。
        let overlay = diff("加强领导，统筹兼顾。", "加强领导；统筹兼顾。");
        assert_eq!(overlay.readable(), "加强领导~，~[；]统筹兼顾。");
    }

    #[test]
    fn a_deleted_paragraph_stays_as_a_deleted_block() {
        let overlay = diff("第一段。\n\n多余的第二段。", "第一段。");
        let readable = overlay.readable();
        assert_eq!(readable, "第一段。\n\n~多余的第二段。~");
    }

    #[test]
    fn a_whole_new_paragraph_is_added() {
        let overlay = diff("第一段。", "第一段。\n\n新增的第二段。");
        let readable = overlay.readable();
        assert_eq!(readable, "第一段。\n\n[新增的第二段。]");
    }
}
