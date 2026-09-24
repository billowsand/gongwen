//! 版本对照的联动：左侧代码 diff 的变更块 ↔ 右侧花脸稿预览的块。
//!
//! 两层 diff 各算各的（方案第一节），只通过「源码字节范围」互相定位：
//! - 代码 diff（`diff::body_diff`）的每处变更带着新版源码范围，或旧版行号；
//! - 花脸稿 Markdown 的每个块带着它的来历（`visual_diff::MarkedSpan`：
//!   新版 / 旧版源码范围 → 花脸稿里的范围）。
//!
//! 预览画的是花脸稿 Markdown，它回报的点击 / 悬停范围是花脸稿里的字节位置，
//! 与用户源码对不上，必须经这张表换算，不能直接拿去跳源码。
//!
//! 全是纯函数，不碰界面与存储。

use crate::diff::{BlockChange, BodyDiff, ChangeKind, DiffBlock};
use crate::visual_diff::{MarkedSpan, SourceSide};
use std::ops::Range;

/// 一处代码变更在两版源码里的位置。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ChangeSource {
    /// 新版源码范围；纯删除为 None。
    new: Option<Range<usize>>,
    /// 旧版源码范围；纯新增为 None。
    old: Option<Range<usize>>,
    /// 纯删除在新版里没有位置：记下它在新版里的插入点（紧随其后那一块的起点），
    /// 用来把花脸稿里「旧块整删」之外的点击也能落到它上面。
    anchor: usize,
}

/// 联动表。每次代码 diff 或花脸稿重算后一起重建。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ChangeLinks {
    changes: Vec<ChangeSource>,
    spans: Vec<MarkedSpan>,
}

fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    // 空范围按一个点看：落在另一个范围里（含起点）就算相交。
    if a.is_empty() {
        return b.start <= a.start && a.start < b.end.max(b.start + 1);
    }
    if b.is_empty() {
        return a.start <= b.start && b.start < a.end;
    }
    a.start < b.end && b.start < a.end
}

/// 文本第 `line` 行（1 基）的字节范围，不含行尾换行（与 `diff` 切块口径一致）。
fn line_range(starts: &[usize], text: &str, line: usize) -> Option<Range<usize>> {
    let start = *starts.get(line.checked_sub(1)?)?;
    let raw_end = starts.get(line).map_or(text.len(), |next| next - 1);
    let end = if text[start..raw_end].ends_with('\r') {
        raw_end - 1
    } else {
        raw_end
    };
    Some(start..end)
}

fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(index, _)| index + 1))
        .collect()
}

impl ChangeLinks {
    /// `old` 是基准版正文（纯删除的变更只有旧版行号，要靠它换成字节范围）。
    pub(crate) fn new(body: &BodyDiff, old: &str, spans: &[MarkedSpan]) -> Self {
        let starts = line_starts(old);
        let mut changes: Vec<ChangeSource> = Vec::new();
        // 纯删除的插入点要等下一块才知道，先记下待补的下标。
        let mut pending_anchor: Vec<usize> = Vec::new();
        for block in &body.blocks {
            let (new_start, change) = match block {
                DiffBlock::Unchanged(context) => {
                    (context.first().map(|first| first.new_range.start), None)
                }
                DiffBlock::Changed(change) => {
                    let source = change_source(change, old, &starts);
                    (source.new.as_ref().map(|new| new.start), Some(source))
                }
            };
            if let Some(start) = new_start {
                for index in pending_anchor.drain(..) {
                    changes[index].anchor = start;
                }
            }
            if let Some(source) = change {
                if source.new.is_none() {
                    pending_anchor.push(changes.len());
                }
                changes.push(source);
            }
        }
        Self {
            changes,
            spans: spans.to_vec(),
        }
    }

    /// 变更块总数（与 `BodyDiff::changed_count` 相同）。
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.changes.len()
    }

    /// 第 `change` 处变更在花脸稿 Markdown 里对应的范围（相关各块的并集），
    /// 预览拿它当锚点高亮、滚动。找不到对应块（比如只改了空行）时返回 None。
    pub(crate) fn marked_range(&self, change: usize) -> Option<Range<usize>> {
        let source = self.changes.get(change)?;
        let mut union: Option<Range<usize>> = None;
        for span in &self.spans {
            let hit = match span.side {
                SourceSide::New => source
                    .new
                    .as_ref()
                    .is_some_and(|new| overlaps(new, &span.source)),
                SourceSide::Old => source
                    .old
                    .as_ref()
                    .is_some_and(|old| overlaps(old, &span.source)),
            };
            if hit {
                union = Some(match union {
                    Some(range) => {
                        range.start.min(span.marked.start)..range.end.max(span.marked.end)
                    }
                    None => span.marked.clone(),
                });
            }
        }
        union
    }

    /// 花脸稿里的一个范围（预览点中 / 悬停的块）对应第几处代码变更。
    /// 点在没改动的块上返回 None。
    pub(crate) fn change_at(&self, marked: &Range<usize>) -> Option<usize> {
        self.spans
            .iter()
            .filter(|span| overlaps(&span.marked, marked) || overlaps(marked, &span.marked))
            .filter_map(|span| {
                self.changes.iter().position(|change| match span.side {
                    SourceSide::New => change
                        .new
                        .as_ref()
                        .is_some_and(|new| overlaps(new, &span.source)),
                    SourceSide::Old => change
                        .old
                        .as_ref()
                        .is_some_and(|old| overlaps(old, &span.source)),
                })
            })
            .min()
    }

    /// 新版源码范围对应的花脸稿范围：左侧点中一行未改动的上下文时，右侧滚到那里。
    pub(crate) fn marked_for_new_source(&self, source: &Range<usize>) -> Option<Range<usize>> {
        self.spans
            .iter()
            .find(|span| span.side == SourceSide::New && overlaps(source, &span.source))
            .map(|span| span.marked.clone())
    }

    /// 第 `change` 处变更在新版源码里的落点：有新版范围就用它，纯删除用插入点。
    /// 「在源码中编辑」据此把光标带过去。
    pub(crate) fn new_source(&self, change: usize) -> Option<Range<usize>> {
        let source = self.changes.get(change)?;
        Some(source.new.clone().unwrap_or(source.anchor..source.anchor))
    }
}

fn change_source(change: &BlockChange, old: &str, starts: &[usize]) -> ChangeSource {
    let old_range = match change.kind {
        ChangeKind::Insert => None,
        ChangeKind::Delete | ChangeKind::Replace => line_range(starts, old, change.old_line),
    };
    ChangeSource {
        new: change.new_range.clone(),
        old: old_range,
        anchor: change.new_range.as_ref().map_or(0, |range| range.start),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::body_diff;
    use crate::redline;

    fn links(old: &str, new: &str) -> (ChangeLinks, redline::RedlineDoc) {
        let doc = redline::build(old, new);
        let body = body_diff(old, new);
        (ChangeLinks::new(&body, old, &doc.spans), doc)
    }

    fn marked_text(doc: &redline::RedlineDoc, range: Range<usize>) -> String {
        crate::export::strip_redline(&doc.markdown[range])
    }

    #[test]
    fn an_edited_paragraph_maps_both_ways() {
        let old = "第一段。\n\n请于八月十日前报送。\n\n第三段。";
        let new = "第一段。\n\n请于八月十五日前报送。\n\n第三段。";
        let (links, doc) = links(old, new);
        assert_eq!(links.len(), 1);
        let range = links.marked_range(0).expect("改过的段在花脸稿里有位置");
        let text = marked_text(&doc, range.clone());
        assert!(text.contains("报送"), "{text}");
        assert!(!text.contains("第一段"), "只圈住改过的那一段：{text}");
        // 反向：点中花脸稿里这一块，落回同一处变更。
        assert_eq!(links.change_at(&range), Some(0));
        // 点中没改的段：不是任何变更。
        let first = doc.markdown.find("第一段").unwrap();
        assert_eq!(links.change_at(&(first..first + 3)), None);
        // 左栏点中未改动的上下文行：右栏找得到同一段。
        let marked = links
            .marked_for_new_source(&(0..4))
            .expect("首段在花脸稿里");
        assert!(marked_text(&doc, marked).contains("第一段"));
    }

    #[test]
    fn a_deleted_paragraph_maps_to_its_struck_block() {
        let old = "第一段。\n\n多余的第二段。\n\n第三段。";
        let new = "第一段。\n\n第三段。";
        let (links, doc) = links(old, new);
        assert_eq!(links.len(), 1);
        let range = links.marked_range(0).expect("整删的块留在花脸稿里");
        assert_eq!(marked_text(&doc, range.clone()), "多余的第二段。");
        assert_eq!(links.change_at(&range), Some(0));
        // 纯删除在新版里的落点是紧随其后那一段的起点。
        let third = new.find("第三段").unwrap();
        assert_eq!(links.new_source(0), Some(third..third));
    }

    #[test]
    fn an_inserted_heading_and_several_changes_keep_their_order() {
        let old = "## 工作目标\n\n甲。\n\n乙。";
        let new = "## 工作目标\n\n甲改。\n\n## 保障措施\n\n乙。";
        let (links, doc) = links(old, new);
        assert_eq!(links.len(), 2);
        let first = marked_text(&doc, links.marked_range(0).unwrap());
        let second = marked_text(&doc, links.marked_range(1).unwrap());
        assert!(first.contains('甲'), "{first}");
        assert!(second.contains("保障措施"), "{second}");
        assert_eq!(links.change_at(&links.marked_range(1).unwrap()), Some(1));
    }

    #[test]
    fn line_ranges_skip_the_carriage_return() {
        let text = "甲\r\n乙\r\n丙";
        let starts = line_starts(text);
        assert_eq!(&text[line_range(&starts, text, 2).unwrap()], "乙");
        assert_eq!(&text[line_range(&starts, text, 3).unwrap()], "丙");
        assert_eq!(line_range(&starts, text, 4), None);
    }
}
