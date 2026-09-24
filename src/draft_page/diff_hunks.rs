//! 可编辑统一 diff 的数据层：把代码 diff 的逐行变更聚成「变更块」（hunk），
//! 算出每块的已删除旧行放在哪条新行之前、哪些新行要画绿底，以及逐块还原。
//!
//! 全是纯函数，不碰界面。`diff::body_diff` 按非空行比较，行号是按 `\n` 切出的
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
    for block in &body.blocks {
        match block {
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
pub(crate) fn revert(hunk: &Hunk, old: &str, new: &str) -> (String, usize) {
    let old_starts = line_starts(old);
    let new_starts = line_starts(new);
    let old_span = |first: usize, last: usize| -> Option<Range<usize>> {
        let start = line_range(&old_starts, old, first)?.start;
        let end = line_range(&old_starts, old, last)?.end;
        Some(start..end)
    };
    let new_line = |line: usize| line_range(&new_starts, new, line);
    let old_line = |line: usize| line_range(&old_starts, old, line);
    let splice = |range: Range<usize>, insert: &str| -> (String, usize) {
        let mut text = String::with_capacity(new.len() + insert.len());
        text.push_str(&new[..range.start]);
        text.push_str(insert);
        text.push_str(&new[range.end..]);
        (text, range.start)
    };
    match (hunk.old_lines, hunk.new_lines) {
        // 改写（可能夹着增删）：新文本里这一块整段换回旧文本那一块。
        (Some((old_first, old_last)), Some((new_first, new_last))) => {
            let (Some(old_range), Some(first), Some(last)) = (
                old_span(old_first, old_last),
                new_line(new_first),
                new_line(new_last),
            ) else {
                return (new.to_string(), 0);
            };
            splice(first.start..last.end, &old[old_range])
        }
        // 纯新增：删掉这几行，连同把它们和后文隔开的空行。
        (None, Some((new_first, new_last))) => {
            let (Some(first), Some(last)) = (new_line(new_first), new_line(new_last)) else {
                return (new.to_string(), 0);
            };
            let range = match (hunk.next_context, hunk.prev_context) {
                (Some((_, next)), _) => {
                    first.start..new_line(next).map_or(new.len(), |line| line.start)
                }
                (None, Some((_, prev))) => new_line(prev).map_or(0, |line| line.end)..last.end,
                (None, None) => 0..new.len(),
            };
            splice(range, "")
        }
        // 纯删除：在原位置插回旧行，分隔空行照旧版原样补上。
        (Some((old_first, old_last)), None) => {
            let Some(old_range) = old_span(old_first, old_last) else {
                return (new.to_string(), 0);
            };
            let chunk = &old[old_range.clone()];
            match (hunk.next_context, hunk.prev_context) {
                (Some((next_old, next_new)), _) => {
                    let separator_end = old_line(next_old).map_or(old.len(), |line| line.start);
                    let separator = &old[old_range.end..separator_end];
                    let at = new_line(next_new).map_or(new.len(), |line| line.start);
                    splice(at..at, &format!("{chunk}{separator}"))
                }
                (None, Some((prev_old, prev_new))) => {
                    let separator_start = old_line(prev_old).map_or(0, |line| line.end);
                    let separator = &old[separator_start..old_range.start];
                    let at = new_line(prev_new).map_or(new.len(), |line| line.end);
                    let (text, _) = splice(at..at, &format!("{separator}{chunk}"));
                    (text, at + separator.len())
                }
                (None, None) => (chunk.to_string(), 0),
            }
        }
        (None, None) => (new.to_string(), 0),
    }
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
        assert_eq!(first.deleted.len(), 2);
        assert_eq!(first.deleted[0].text(), "请于八月十日前报送。");
        assert_eq!(first.deleted[1].text(), "多余的一段。");
        assert_eq!(first.added.len(), 1);
        assert_eq!(first.added[0].line, 4, "改写后的行是新文本第 5 行");
        assert_eq!(first.gap_line, 4, "旧行叠在改写行上方");
        assert!(first.touches_line(4));
        let second = &hunks[1];
        assert!(second.deleted.is_empty());
        assert_eq!(second.added[0].line, 8);
        assert!(second.added[0].spans.is_empty(), "整行新增不做字级高亮");
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
}
