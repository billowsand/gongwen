//! 检查器埋点：谁的建议被采纳，谁的被忽略。
//!
//! 立这一层的理由很直接：**不度量就没法判断一个检查器该留还是该关**。公文场景
//! 里一个天天误报的规则，用户的反应不是忽略它，是把整个校对关掉——那比没有这条
//! 规则更糟（`proofread_rules` 开头写的就是这个取舍）。有了采纳率，这个判断有据
//! 可依，而不是凭印象。
//!
//! 只记用户的**动作**，不记「出现过多少条」：
//!
//! - 出现数会被每轮重新校验反复累加，去重成本高，含义还模糊——用户没处理不等于
//!   不认可，可能只是没看完；
//! - 采纳与忽略是明确的表态，且一条建议只会表态一次，天然不重复。
//!
//! 所以采纳率 = 采纳 / (采纳 + 忽略)。分母小的时候不作数，界面上要说清楚。
//!
//! 数据只留在本机，单独存一个文件而不进 `config.json`：埋点每点一次就变，塞进
//! 配置会让配置版本历史被这类噪音淹没，回看「我上周改过什么设置」时全是计数。

use crate::revision::RevisionSource;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 判定「这个检查器是不是在帮倒忙」至少需要这么多次表态。样本太小时，
/// 一次误报就能把采纳率打到 50%，据此下结论只会误伤。
pub const MIN_SAMPLES: u32 = 8;

/// 采纳率低于这个值就该考虑停用。三成是经验线：十条里有七条要手动划掉，
/// 用户下一步就是关掉整个功能。
pub const LOW_ADOPTION: f32 = 0.3;

/// 一个来源的累计表态。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceStat {
    pub accepted: u32,
    pub ignored: u32,
    /// 采纳之后又撤销的次数。这一笔会把 `accepted` 减回去——那次采纳最终没有留下，
    /// 算成「采纳过」只会让采纳率虚高，而这个指标存在的意义正是别骗自己。
    /// 单独记一笔是因为「点下去才发现不对」比直接忽略更能说明这条规则误导人。
    pub undone: u32,
    /// 模型改写被闸门拦下的次数。规则来源恒为 0。
    pub gate_rejected: u32,
}

impl SourceStat {
    /// 用户表态的总次数。
    pub fn decisions(&self) -> u32 {
        self.accepted + self.ignored
    }

    /// 采纳率。表态次数为 0 时返回 `None`——没有数据就不要给数字，
    /// 显示「0%」会被当成「很差」。
    pub fn adoption(&self) -> Option<f32> {
        let total = self.decisions();
        (total > 0).then(|| self.accepted as f32 / total as f32)
    }

    /// 样本够多且采纳率过低。这是「建议停用」的唯一判据。
    pub fn is_underperforming(&self) -> bool {
        self.decisions() >= MIN_SAMPLES && self.adoption().is_some_and(|rate| rate < LOW_ADOPTION)
    }
}

/// 全部来源的埋点。键是来源编号：词表条目号、规则号或检查器 id。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Metrics {
    pub sources: BTreeMap<String, SourceStat>,
    /// 有没有未落盘的改动。不进 JSON——它描述的是内存状态。
    #[serde(skip)]
    dirty: bool,
}

impl Metrics {
    pub fn get(&self, key: &str) -> SourceStat {
        self.sources.get(key).copied().unwrap_or_default()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn entry(&mut self, key: &str) -> &mut SourceStat {
        self.dirty = true;
        self.sources.entry(key.to_string()).or_default()
    }

    pub fn record_accept(&mut self, source: &RevisionSource) {
        self.entry(source.key()).accepted += 1;
    }

    pub fn record_ignore(&mut self, source: &RevisionSource) {
        self.entry(source.key()).ignored += 1;
    }

    /// 撤销一次采纳。`accepted` 一并回退：那次采纳最终没有留下，算成「采纳过」
    /// 会让采纳率虚高，而这个指标存在的意义正是别骗自己。
    pub fn record_undo(&mut self, source: &RevisionSource) {
        let stat = self.entry(source.key());
        stat.undone += 1;
        stat.accepted = stat.accepted.saturating_sub(1);
    }

    pub fn record_gate_rejection(&mut self, task: &str, count: u32) {
        if count == 0 {
            return;
        }
        self.entry(task).gate_rejected += count;
    }

    /// 采纳率过低、值得提请用户停用的来源，按采纳率从低到高排。
    pub fn underperforming(&self) -> Vec<(&str, SourceStat)> {
        let mut rows: Vec<_> = self
            .sources
            .iter()
            .filter(|(_, stat)| stat.is_underperforming())
            .map(|(key, stat)| (key.as_str(), *stat))
            .collect();
        rows.sort_by(|a, b| {
            a.1.adoption()
                .unwrap_or(1.0)
                .total_cmp(&b.1.adoption().unwrap_or(1.0))
                .then(b.1.decisions().cmp(&a.1.decisions()))
        });
        rows
    }

    pub fn clear(&mut self) {
        self.sources.clear();
        self.dirty = true;
    }

    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }
}

fn path() -> anyhow::Result<std::path::PathBuf> {
    Ok(crate::storage::config_dir()?.join("metrics.json"))
}

/// 读埋点。文件不存在、损坏或读不动都返回空表——埋点是辅助信息，
/// 任何一种失败都不该妨碍用户写公文。
pub fn load() -> Metrics {
    let Ok(path) = path() else {
        return Metrics::default();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// 落盘。失败静默：同上，这不是用户需要处理的错误。
pub fn save(metrics: &mut Metrics) {
    if !metrics.is_dirty() {
        return;
    }
    let Ok(path) = path() else { return };
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(text) = serde_json::to_string_pretty(metrics) else {
        return;
    };
    if std::fs::write(path, text).is_ok() {
        metrics.mark_saved();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lexicon(id: &str) -> RevisionSource {
        RevisionSource::Lexicon {
            entry_id: id.to_string(),
        }
    }

    #[test]
    fn adoption_is_none_before_any_decision() {
        let metrics = Metrics::default();
        assert!(metrics.get("TYP-001").adoption().is_none());
    }

    #[test]
    fn accepting_and_ignoring_move_the_rate() {
        let mut metrics = Metrics::default();
        for _ in 0..3 {
            metrics.record_accept(&lexicon("TYP-001"));
        }
        metrics.record_ignore(&lexicon("TYP-001"));
        let stat = metrics.get("TYP-001");
        assert_eq!(stat.decisions(), 4);
        assert_eq!(stat.adoption(), Some(0.75));
    }

    #[test]
    fn undo_takes_back_the_acceptance() {
        // 采纳后又撤销，说明这条把人误导了；留在采纳里会让采纳率虚高。
        let mut metrics = Metrics::default();
        metrics.record_accept(&lexicon("TYP-002"));
        metrics.record_undo(&lexicon("TYP-002"));
        let stat = metrics.get("TYP-002");
        assert_eq!(stat.accepted, 0);
        assert_eq!(stat.undone, 1);
        assert!(stat.adoption().is_none(), "抵消后不该再有采纳率");
    }

    #[test]
    fn undo_never_underflows() {
        let mut metrics = Metrics::default();
        metrics.record_undo(&lexicon("TYP-003"));
        assert_eq!(metrics.get("TYP-003").accepted, 0);
    }

    #[test]
    fn a_small_sample_never_counts_as_underperforming() {
        // 三次全忽略，采纳率 0，但样本太小，不该据此建议停用。
        let mut metrics = Metrics::default();
        for _ in 0..3 {
            metrics.record_ignore(&lexicon("TYP-004"));
        }
        assert!(!metrics.get("TYP-004").is_underperforming());
        assert!(metrics.underperforming().is_empty());
    }

    #[test]
    fn a_consistently_ignored_rule_is_flagged() {
        let mut metrics = Metrics::default();
        for _ in 0..MIN_SAMPLES {
            metrics.record_ignore(&lexicon("TYP-005"));
        }
        assert!(metrics.get("TYP-005").is_underperforming());
        let flagged = metrics.underperforming();
        assert_eq!(flagged.len(), 1);
        assert_eq!(flagged[0].0, "TYP-005");
    }

    #[test]
    fn gate_rejections_are_counted_separately_from_decisions() {
        let mut metrics = Metrics::default();
        metrics.record_gate_rejection("MDL-GRAMMAR", 5);
        let stat = metrics.get("MDL-GRAMMAR");
        assert_eq!(stat.gate_rejected, 5);
        assert_eq!(stat.decisions(), 0, "闸门拦截不是用户表态");
    }

    #[test]
    fn zero_rejections_do_not_create_an_entry() {
        let mut metrics = Metrics::default();
        metrics.record_gate_rejection("MDL-GRAMMAR", 0);
        assert!(metrics.sources.is_empty());
        assert!(!metrics.is_dirty(), "什么都没发生就不该标记待落盘");
    }
}
