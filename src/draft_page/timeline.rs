//! 版本时间轴的纯数据投影：不读库、不碰 egui。

use crate::manuscript::VersionRow;

/// 时间轴当前选中的对象。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum TimelineTarget {
    /// 起草页当前内容（尚未提交的工作区）。
    #[default]
    Working,
    /// 已提交的历史版本。
    Version(i64),
}

/// 历史版本之间的比较方式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum TimelineComparison {
    /// vN 与它的直接前一条已存在版本比较；v1 的旧侧为空。
    #[default]
    Previous,
    /// vN 与固定基准版本比较。
    FixedBaseline,
}

/// 时间轴一行的纯显示数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TimelineRowData {
    pub(crate) target: TimelineTarget,
    pub(crate) selected: bool,
    pub(crate) latest: bool,
    /// 旧侧版本号；None 表示空白稿。
    pub(crate) old_version: Option<i64>,
    /// 新侧版本号；None 表示当前工作区。
    pub(crate) new_version: Option<i64>,
    /// 固定基准无效时，旧侧已退回直接上一版。
    pub(crate) fixed_baseline_adjusted: bool,
}

/// 版本排序与每行比较方向的计算结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TimelineRows {
    pub(crate) rows: Vec<TimelineRowData>,
    /// 当前目标：输入版本列表里已不存在的历史选择回落到工作区。
    pub(crate) selected: TimelineTarget,
    /// 当前选中行实际使用的固定基准；只有固定基准模式有值。
    pub(crate) effective_fixed_baseline: Option<i64>,
    pub(crate) fixed_baseline_adjusted: bool,
}

/// 纯函数：先列工作区，再按版本号倒序列历史，并计算每行的旧 / 新版本。
///
/// 固定基准晚于目标版本或已不存在时，比较方向仍保持「旧 → 新」：退回到该目标
/// 的直接上一条已存在版本；v1 没有前一版，旧侧为空。结果通过
/// `fixed_baseline_adjusted` 明示这次回退，不偷偷把未来版本当旧版。
pub(crate) fn timeline_rows(
    versions: &[VersionRow],
    selected: TimelineTarget,
    comparison: TimelineComparison,
    fixed_baseline: Option<i64>,
) -> TimelineRows {
    let mut ascending = versions.iter().collect::<Vec<_>>();
    ascending.sort_by_key(|row| row.version_number);
    let latest = ascending.last().map(|row| row.version_number);
    let has_version = |number: i64| ascending.iter().any(|row| row.version_number == number);
    let selected = match selected {
        TimelineTarget::Working => TimelineTarget::Working,
        TimelineTarget::Version(number) if has_version(number) => TimelineTarget::Version(number),
        TimelineTarget::Version(_) => TimelineTarget::Working,
    };
    let previous = |number: i64| {
        ascending
            .iter()
            .filter(|row| row.version_number < number)
            .max_by_key(|row| row.version_number)
            .map(|row| row.version_number)
    };
    let pair_for = |target: TimelineTarget| -> (Option<i64>, Option<i64>, bool) {
        match target {
            TimelineTarget::Working => {
                let old = match comparison {
                    TimelineComparison::Previous => latest,
                    TimelineComparison::FixedBaseline => fixed_baseline
                        .filter(|number| has_version(*number))
                        .or(latest),
                };
                (old, None, false)
            }
            TimelineTarget::Version(number) => match comparison {
                TimelineComparison::Previous => (previous(number), Some(number), false),
                TimelineComparison::FixedBaseline => match fixed_baseline {
                    Some(old) if has_version(old) && old < number => {
                        (Some(old), Some(number), false)
                    }
                    Some(_) => (previous(number), Some(number), true),
                    None => (previous(number), Some(number), false),
                },
            },
        }
    };

    let mut rows = Vec::with_capacity(ascending.len() + 1);
    let (old_version, new_version, adjusted) = pair_for(TimelineTarget::Working);
    rows.push(TimelineRowData {
        target: TimelineTarget::Working,
        selected: selected == TimelineTarget::Working,
        latest: false,
        old_version,
        new_version,
        fixed_baseline_adjusted: adjusted,
    });
    for row in ascending.iter().rev() {
        let target = TimelineTarget::Version(row.version_number);
        let (old_version, new_version, fixed_baseline_adjusted) = pair_for(target);
        rows.push(TimelineRowData {
            target,
            selected: selected == target,
            latest: Some(row.version_number) == latest,
            old_version,
            new_version,
            fixed_baseline_adjusted,
        });
    }
    let selected_row = rows
        .iter()
        .find(|row| row.target == selected)
        .expect("工作区行始终存在");
    let selected_old_version = selected_row.old_version;
    let fixed_baseline_adjusted = selected_row.fixed_baseline_adjusted;
    let effective_fixed_baseline = (comparison == TimelineComparison::FixedBaseline)
        .then_some(selected_old_version)
        .flatten();
    TimelineRows {
        rows,
        selected,
        effective_fixed_baseline,
        fixed_baseline_adjusted,
    }
}

/// 起草页时间轴的持久交互状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VersionTimelineState {
    pub(crate) selected: TimelineTarget,
    pub(crate) comparison: TimelineComparison,
    pub(crate) fixed_baseline: Option<i64>,
    pub(crate) expanded: bool,
}

impl Default for VersionTimelineState {
    fn default() -> Self {
        Self {
            selected: TimelineTarget::Working,
            comparison: TimelineComparison::Previous,
            fixed_baseline: None,
            expanded: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn versions() -> Vec<VersionRow> {
        [1, 2, 3]
            .into_iter()
            .map(|version_number| VersionRow {
                version_number,
                name: format!("版本{version_number}"),
                comment: format!("注释{version_number}"),
                title: format!("标题{version_number}"),
                doc_number: String::new(),
                doc_date: String::new(),
                created_at: format!("2026-09-0{version_number}"),
                is_latest: version_number == 3,
            })
            .collect()
    }

    #[test]
    fn timeline_puts_working_first_and_history_descending_with_latest_badge() {
        let rows = timeline_rows(
            &versions(),
            TimelineTarget::Working,
            TimelineComparison::Previous,
            None,
        );
        assert_eq!(rows.selected, TimelineTarget::Working);
        assert_eq!(rows.rows[0].target, TimelineTarget::Working);
        assert!(rows.rows[0].selected);
        assert_eq!(
            rows.rows[1..]
                .iter()
                .map(|row| row.target)
                .collect::<Vec<_>>(),
            [
                TimelineTarget::Version(3),
                TimelineTarget::Version(2),
                TimelineTarget::Version(1),
            ]
        );
        assert!(rows.rows[1].latest);
        assert!(!rows.rows[2].latest);
    }

    #[test]
    fn previous_mode_pairs_each_history_row_to_its_predecessor() {
        let rows = timeline_rows(
            &versions(),
            TimelineTarget::Version(1),
            TimelineComparison::Previous,
            None,
        );
        let row = |number| {
            rows.rows
                .iter()
                .find(|row| row.target == TimelineTarget::Version(number))
                .unwrap()
        };
        assert_eq!((row(3).old_version, row(3).new_version), (Some(2), Some(3)));
        assert_eq!((row(2).old_version, row(2).new_version), (Some(1), Some(2)));
        assert_eq!((row(1).old_version, row(1).new_version), (None, Some(1)));
    }

    #[test]
    fn fixed_baseline_is_used_or_falls_back_before_the_target() {
        let versions = versions();
        let selected = timeline_rows(
            &versions,
            TimelineTarget::Version(3),
            TimelineComparison::FixedBaseline,
            Some(1),
        );
        assert_eq!(selected.effective_fixed_baseline, Some(1));
        assert!(!selected.fixed_baseline_adjusted);
        let row3 = selected
            .rows
            .iter()
            .find(|row| row.target == TimelineTarget::Version(3))
            .unwrap();
        assert_eq!((row3.old_version, row3.new_version), (Some(1), Some(3)));

        // 固定基准晚于 v2 时回退到 v1；晚于 v1 时旧侧为空白稿。
        let later = timeline_rows(
            &versions,
            TimelineTarget::Version(2),
            TimelineComparison::FixedBaseline,
            Some(3),
        );
        assert_eq!(later.effective_fixed_baseline, Some(1));
        assert!(later.fixed_baseline_adjusted);
        let row2 = later
            .rows
            .iter()
            .find(|row| row.target == TimelineTarget::Version(2))
            .unwrap();
        assert_eq!((row2.old_version, row2.new_version), (Some(1), Some(2)));

        let first = timeline_rows(
            &versions,
            TimelineTarget::Version(1),
            TimelineComparison::FixedBaseline,
            Some(2),
        );
        assert_eq!(first.effective_fixed_baseline, None);
        assert!(first.fixed_baseline_adjusted);
        let row1 = first
            .rows
            .iter()
            .find(|row| row.target == TimelineTarget::Version(1))
            .unwrap();
        assert_eq!((row1.old_version, row1.new_version), (None, Some(1)));
    }
}
