//! 可编辑统一 diff 的数据层：把代码 diff 的逐行变更聚成「变更块」（hunk），
//! 算出每块的已删除旧行放在哪条新行之前、哪些新行要画绿底，以及逐块还原。
//!
//! 全是纯函数，不碰界面。`diff::body_diff` 逐行比较（空行也算），行号是按 `\n` 切出的
//! 1 基源码行号，与 `TextEdit` galley 的源码行一一对应（见 `diff_gaps`）。

use crate::diff::{BlockChange, BodyDiff, ChangeKind, DiffBlock, InlineSpan};
use crate::version_link::{line_range, line_starts};
use std::ops::Range;

/// 一条已删除的旧行，画在空隙里。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeletedRow {
    /// 旧版行号（1 基）。
    pub(crate) old_line: usize,
    /// 旧行的字级片段（`Same` + `Removed`），拼起来就是整行旧文本。
    pub(crate) spans: Vec<InlineSpan>,
}

impl DeletedRow {
    pub(crate) fn text(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }
}

/// 一条新增 / 改写的新行，画绿底。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddedRow {
    /// 新版行号（0 基，便于直接当 galley 源码行下标用）。
    pub(crate) line: usize,
    /// 改写行的字级片段（`Same` + `Added`），真正改掉的字加深底色；整行新增时为空。
    pub(crate) spans: Vec<InlineSpan>,
}

/// 一个变更块：代码 diff 里夹在两段未改动内容之间的一串连续变更。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hunk {
    /// 覆盖的 `BodyDiff` 变更序号（`ChangeLinks` 按它换算到花脸稿）。
    pub(crate) changes: Range<usize>,
    /// 已删除 / 被改写的旧行，按旧版顺序；都画在 `gap_line` 之前的空隙里。
    pub(crate) deleted: Vec<DeletedRow>,
    /// 新增 / 改写后的新行。
    pub(crate) added: Vec<AddedRow>,
    /// 空隙开在新文本第几行（0 基）之前；等于总行数表示开在末尾。
    pub(crate) gap_line: usize,
    /// 旧版里本块首末行（1 基，含）；纯新增为 None。
    old_lines: Option<(usize, usize)>,
    /// 新版里本块首末行（1 基，含）；纯删除为 None。
    new_lines: Option<(usize, usize)>,
    /// 紧挨在本块前 / 后的未改动行：`(旧行号, 新行号)`，还原时据此补回分隔空行。
    prev_context: Option<(usize, usize)>,
    next_context: Option<(usize, usize)>,
}

impl Hunk {
    /// 本块在新文本里的第一行（0 基）：光标跳转、焦点判定都认它。
    pub(crate) fn first_line(&self) -> usize {
        self.added.first().map_or(self.gap_line, |row| row.line)
    }

    /// 光标在新文本第 `line` 行（0 基）时算不算落在本块上：新行本身，或纯删除
    /// 空隙紧下面那一行。
    pub(crate) fn touches_line(&self, line: usize) -> bool {
        match self.new_lines {
            Some((first, last)) => (first - 1..last).contains(&line),
            None => line == self.gap_line,
        }
    }
}

/// 源码行数（按 `\n` 切，与 galley 源码行一致）。
pub(crate) fn line_count(text: &str) -> usize {
    text.split('\n').count()
}

/// 把代码 diff 聚成变更块。`new_text` 用来算末尾空隙的行号。
pub(crate) fn hunks(body: &BodyDiff, new_text: &str) -> Vec<Hunk> {
    let total_lines = line_count(new_text);
    let mut out: Vec<Hunk> = Vec::new();
    let mut change_index = 0usize;
    let mut prev_context: Option<(usize, usize)> = None;
    let mut open: Option<Hunk> = None;
    for (index, block) in body.blocks.iter().enumerate() {
        match block {
            // 两处变更之间只隔着没动过的空行：仍算同一块。空行只是分隔，拿它把
            // 「改一段 + 删下一段」切成两块，还原要点两次，也不像 Zed 那样成块。
            DiffBlock::Unchanged(context)
                if open.is_some()
                    && context
                        .iter()
                        .all(|line| crate::diff::is_blank_line(&line.text))
                    && matches!(body.blocks.get(index + 1), Some(DiffBlock::Changed(_))) => {}
            DiffBlock::Unchanged(context) => {
                if let Some(mut hunk) = open.take() {
                    hunk.next_context = context.first().map(|line| (line.old_line, line.new_line));
                    out.push(finish(hunk, total_lines));
                }
                prev_context = context.last().map(|line| (line.old_line, line.new_line));
            }
            DiffBlock::Changed(change) => {
                let hunk = open.get_or_insert_with(|| Hunk {
                    changes: change_index..change_index,
                    deleted: Vec::new(),
                    added: Vec::new(),
                    gap_line: total_lines,
                    old_lines: None,
                    new_lines: None,
                    prev_context,
                    next_context: None,
                });
                push_change(hunk, change);
                change_index += 1;
                hunk.changes.end = change_index;
            }
        }
    }
    if let Some(hunk) = open.take() {
        out.push(finish(hunk, total_lines));
    }
    out
}

fn extend(range: &mut Option<(usize, usize)>, line: usize) {
    *range = Some(match *range {
        Some((first, last)) => (first.min(line), last.max(line)),
        None => (line, line),
    });
}

fn push_change(hunk: &mut Hunk, change: &BlockChange) {
    if change.kind != ChangeKind::Insert {
        extend(&mut hunk.old_lines, change.old_line);
        hunk.deleted.push(DeletedRow {
            old_line: change.old_line,
            spans: change.before_spans.clone(),
        });
    }
    if change.kind != ChangeKind::Delete {
        extend(&mut hunk.new_lines, change.new_line);
        hunk.added.push(AddedRow {
            line: change.new_line - 1,
            spans: if change.kind == ChangeKind::Replace {
                change.after_spans.clone()
            } else {
                Vec::new()
            },
        });
    }
}

fn finish(mut hunk: Hunk, total_lines: usize) -> Hunk {
    // 旧行照旧版顺序排：代码 diff 里改写与整删可能交错。
    hunk.deleted.sort_by_key(|row| row.old_line);
    hunk.added.sort_by_key(|row| row.line);
    hunk.gap_line = match (hunk.new_lines, hunk.next_context) {
        (Some((first, _)), _) => first - 1,
        (None, Some((_, next_new))) => next_new - 1,
        (None, None) => total_lines,
    };
    hunk
}

/// 逐块还原：把本块在新文本里的内容换回基准版那一块。返回还原后的全文，以及
/// 光标应落的字节位置（被还原块的起点）。
///
/// 一块就是「上一段未改动内容」与「下一段未改动内容」之间的整个区域——两侧各取
/// 这段区域，原样对换。不按块内各条变更的行号拼：块可能跨过没动过的空行（见
/// [`hunks`]），逐条变更的首末行在新旧两侧对不齐，拼出来会把空行挪位。
///
/// 区域边界：后面还有未改动内容时，区域是「上一段的下一行行首」到「下一段行首」，
/// 连同行尾换行一起换；后面没有了（改的是文末），区域从上一段的**行尾**起算，
/// 把两者之间的换行也算进来，文末多一个或少一个换行都能换回去。
pub(crate) fn revert(hunk: &Hunk, old: &str, new: &str) -> (String, usize) {
    let region = |text: &str, prev: Option<usize>, next: Option<usize>| -> Range<usize> {
        let starts = line_starts(text);
        let line_start = |line: usize| starts.get(line - 1).copied().unwrap_or(text.len());
        match (prev, next) {
            (prev, Some(next)) => prev.map_or(0, |prev| line_start(prev + 1))..line_start(next),
            (Some(prev), None) => {
                line_range(&starts, text, prev).map_or(text.len(), |line| line.end)..text.len()
            }
            (None, None) => 0..text.len(),
        }
    };
    let old_region = region(
        old,
        hunk.prev_context.map(|(old_line, _)| old_line),
        hunk.next_context.map(|(old_line, _)| old_line),
    );
    let new_region = region(
        new,
        hunk.prev_context.map(|(_, new_line)| new_line),
        hunk.next_context.map(|(_, new_line)| new_line),
    );
    let mut text = String::with_capacity(new.len() + old_region.len());
    text.push_str(&new[..new_region.start]);
    text.push_str(&old[old_region.clone()]);
    text.push_str(&new[new_region.end..]);
    // 光标落在被还原内容的第一个字上：文末那种区域以换行开头，跳过它。
    let skipped =
        old[old_region.clone()].len() - old[old_region].trim_start_matches(['\r', '\n']).len();
    let cursor = if hunk.next_context.is_none() && hunk.prev_context.is_some() {
        new_region.start + skipped
    } else {
        new_region.start
    };
    let cursor = cursor.min(text.len());
    (text, cursor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::body_diff;

    fn plan(old: &str, new: &str) -> Vec<Hunk> {
        hunks(&body_diff(old, new), new)
    }

    const OLD: &str = "# 标题\n\n第一段。\n\n请于八月十日前报送。\n\n多余的一段。\n\n第五段。";
    const NEW: &str = "# 标题\n\n第一段。\n\n请于八月十五日前报送。\n\n第五段。\n\n新增的结尾段。";

    #[test]
    fn consecutive_changes_group_into_one_hunk() {
        let hunks = plan(OLD, NEW);
        // 「改写 + 整删」连在一起是一块；结尾新增是另一块。
        assert_eq!(hunks.len(), 2, "{hunks:#?}");
        let first = &hunks[0];
        // 整删「多余的一段」连带删掉它后面的分隔空行：空行也是一条删除行。
        // 两段之间原有两个空行、现在只剩一个，删的是哪一个都对，这里不认位置。
        let texts: Vec<String> = first.deleted.iter().map(DeletedRow::text).collect();
        assert_eq!(texts.len(), 3, "{texts:?}");
        assert_eq!(texts[0], "请于八月十日前报送。");
        assert!(texts.contains(&"多余的一段。".to_string()), "{texts:?}");
        assert_eq!(texts.iter().filter(|text| text.is_empty()).count(), 1);
        assert_eq!(first.added.len(), 1);
        assert_eq!(first.added[0].line, 4, "改写后的行是新文本第 5 行");
        assert_eq!(first.gap_line, 4, "旧行叠在改写行上方");
        assert!(first.touches_line(4));
        let second = &hunks[1];
        assert!(second.deleted.is_empty());
        // 结尾新增一段：先是分隔空行（第 8 行），再是正文（第 9 行）。
        let lines: Vec<usize> = second.added.iter().map(|row| row.line).collect();
        assert_eq!(lines, [7, 8]);
        assert!(second.added[1].spans.is_empty(), "整行新增不做字级高亮");
    }

    #[test]
    fn a_pure_deletion_opens_its_gap_above_the_next_line() {
        let old = "甲。\n\n乙。\n\n丙。";
        let new = "甲。\n\n丙。";
        let hunks = plan(old, new);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].gap_line, 2, "空隙开在「丙」之前");
        assert!(hunks[0].touches_line(2));
        let tail = plan("甲。\n\n乙。", "甲。");
        assert_eq!(tail[0].gap_line, 1, "删的是末尾：空隙开在文末");
    }

    /// 逐块还原：还原一块之后，这一块从代码 diff 里消失，其余块原样保留。
    #[test]
    fn reverting_a_hunk_removes_only_that_hunk() {
        let hunks = plan(OLD, NEW);
        for (index, hunk) in hunks.iter().enumerate() {
            let (text, _) = revert(hunk, OLD, NEW);
            let after = plan(OLD, &text);
            assert_eq!(after.len(), hunks.len() - 1, "还原第 {index} 块：{text:?}");
            // 剩下那块的旧行 / 新行内容不变。
            let other = &hunks[1 - index];
            let remaining = &after[0];
            assert_eq!(
                remaining
                    .deleted
                    .iter()
                    .map(DeletedRow::text)
                    .collect::<Vec<_>>(),
                other
                    .deleted
                    .iter()
                    .map(DeletedRow::text)
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn reverting_every_hunk_gives_back_the_old_text() {
        let cases = [
            (OLD, NEW),
            ("甲。\n\n乙。\n\n丙。", "甲。\n\n丙。"),
            ("甲。\n\n乙。", "甲。"),
            ("甲。", "甲。\n\n乙。"),
            ("乙。\n\n甲。", "甲。"),
            ("甲。", "乙。\n\n甲。"),
            ("甲。\n乙。\n丙。", "甲。\n乙改。\n新增。\n丙。"),
            ("- 一\n- 二\n- 三", "- 一\n- 三\n- 四"),
        ];
        for (old, new) in cases {
            let mut text = new.to_string();
            // 从后往前还原：前面的块行号不受影响。
            loop {
                let hunks = plan(old, &text);
                let Some(hunk) = hunks.last() else { break };
                let (reverted, _) = revert(hunk, old, &text);
                assert_ne!(reverted, text, "还原必须有进展：{old:?} → {new:?}");
                text = reverted;
            }
            assert_eq!(text, old, "{new:?} 逐块还原回 {old:?}");
        }
    }

    #[test]
    fn the_cursor_lands_at_the_start_of_the_reverted_block() {
        let hunks = plan(OLD, NEW);
        let (text, cursor) = revert(&hunks[0], OLD, NEW);
        assert!(
            text[cursor..].starts_with("请于八月十日前报送。"),
            "{}",
            &text[cursor..]
        );
    }

    /// 线性同余发生器：测试可复现，不引入随机数依赖。
    struct Lcg(u64);

    impl Lcg {
        fn below(&mut self, n: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 33) as usize) % n.max(1)
        }
    }

    /// 随机改稿（增删改正文行、增删空行、改行尾空白、末尾换行），再按**随机顺序**
    /// 逐块还原到底：每一步都要有进展，最后与基准**逐字节**相等。代码层 diff 要是
    /// 漏看了空行或空白，这里就还原不回去。
    #[test]
    fn random_edits_revert_back_to_the_exact_base_text() {
        const POOL: [&str; 8] = [
            "## 工作目标",
            "请于八月十日前报送材料。",
            "| 项目 | 时限 |",
            "- 甲项工作",
            "**一是**加强领导。",
            "各单位要高度重视。",
            "联系人：张三。",
            "附件：1. 名单",
        ];
        let mut rng = Lcg(20_260_924);
        for round in 0..2000 {
            let mut lines: Vec<String> = Vec::new();
            for index in 0..1 + rng.below(8) {
                if index > 0 && rng.below(4) != 0 {
                    lines.push(String::new());
                }
                lines.push(format!("{}{index}", POOL[rng.below(POOL.len())]));
            }
            let old = lines.join("\n");
            for _ in 0..1 + rng.below(4) {
                let at = rng.below(lines.len() + 1);
                match rng.below(6) {
                    0 if at < lines.len() && lines.len() > 1 => {
                        lines.remove(at);
                    }
                    1 => lines.insert(at, format!("插入{}", rng.below(99))),
                    2 => lines.insert(at, String::new()),
                    3 if at < lines.len() => lines[at].push('改'),
                    4 if at < lines.len() => lines[at].push(' '),
                    _ => {}
                }
            }
            let mut new = lines.join("\n");
            if rng.below(4) == 0 {
                new.push('\n');
            }
            let mut text = new.clone();
            for _ in 0..100 {
                let hunks = plan(&old, &text);
                if hunks.is_empty() {
                    break;
                }
                let pick = rng.below(hunks.len());
                let (reverted, cursor) = revert(&hunks[pick], &old, &text);
                assert!(cursor <= reverted.len());
                assert!(
                    plan(&old, &reverted).len() < hunks.len(),
                    "第 {round} 组还原没有进展：{old:?} / {text:?} → {reverted:?}"
                );
                text = reverted;
            }
            assert_eq!(text, old, "第 {round} 组：{new:?} 逐块还原回 {old:?}");
        }
    }
}
