//! 修订建议总线：词表、文档规则与模型的统一产物。
//!
//! 立这一层的理由，是 AI 的落地形态原本只有「整篇提案，全接受或全放弃」
//! （见 `app::ai_workbench`）。用户真正要的是「这句话的『截止』该改成
//! 『截至』」——逐条看、逐条确认、逐条撤销。整篇替换既没法逐条归因，也让每加
//! 一个小检查器就得新做一套界面。
//!
//! 所以先定契约：**不管建议来自词表、文档规则还是模型，产物都是 [`Revision`]。**
//! 界面只认它，后面接小模型检查器时一行界面代码都不用改。
//!
//! 两条硬约束，来自公文场景本身：
//!
//! 1. **偏移量永远由程序算。** 模型给的字节位置一概不采信——小模型算 offset
//!    必错。模型只回「改后的整句」，span 由调用方对原句和改后句求差得到。
//! 2. **采纳即自检。** 每落地一条建议就用确定性规则复扫一遍，引入了新的必错
//!    就自动回滚。这样模型的幻觉会被词表自己挡掉，而不是靠提示词里写十遍
//!    「不许编造」。

use crate::proofread::{Level, ProofNote};
use std::collections::BTreeSet;
use std::ops::Range;

/// 锚点上下文取多少个字符。取字符不取字节：12 个汉字足以区分同一个词在不同
/// 句子里的两次出现，再长则正文稍作改动就锚不上了。
const CONTEXT_CHARS: usize = 12;

/// 同一段文字在全篇出现多少次以上就放弃重定位。满篇都是「工作」这两个字时，
/// 靠上下文也认不准，与其猜错位置改坏正文，不如置灰让用户自己看。
const MAX_CANDIDATES: usize = 512;

pub type RevisionId = u64;

/// 建议的来源。决定界面上的来源标签，也决定「永久忽略」的键怎么拼。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevisionSource {
    /// 校对词表命中，`entry_id` 是词表条目编号。
    Lexicon { entry_id: String },
    /// 文档级规则命中（标题规范、文种越界、序号层级等）。
    DocRule { rule_id: String },
    /// 模型检查器产出，`task` 是检查类型（语病、称谓……）。
    ///
    /// 与规则来源分开的实际作用在 [`RevisionSet::begin_rule_pass`]：规则每轮
    /// 校验都重算，模型建议只重定位——跑一轮要几秒到几十秒，不能跟词表一样
    /// 每次校验都推倒重来。
    Model { task: String },
}

impl RevisionSource {
    /// 确定性来源。这类建议每轮校验都重算，不需要跨轮保留。
    pub fn is_rule(&self) -> bool {
        matches!(self, Self::Lexicon { .. } | Self::DocRule { .. })
    }

    /// 忽略记录挂靠用的稳定键的前半段。
    pub fn key(&self) -> &str {
        match self {
            Self::Lexicon { entry_id } => entry_id,
            Self::DocRule { rule_id } => rule_id,
            Self::Model { task } => task,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Lexicon { .. } => "词表",
            Self::DocRule { .. } => "规则",
            Self::Model { .. } => "模型",
        }
    }
}

/// 建议的当前状态。
///
/// 刻意没有分「拒绝」和「忽略」两档：界面上两者是同一个按钮、同一种后果，
/// 分开只会让用户猜它们有什么区别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionState {
    /// 待确认。
    Pending,
    /// 用户按下「忽略」，本篇内不再出现。
    Ignored,
    /// 正文改动后锚不回去了，只展示不可采纳。
    Stale,
}

/// 抗漂移锚。
///
/// 没有它，逐条修订就不能用：`revalidate` 不是每次击键都跑，用户看着建议改了
/// 几个字，`span` 就已经偏了；此时点「采纳」会按旧偏移量切开正文，改坏的地方
/// 还未必在视野里。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    /// 命中的原文。
    pub before: String,
    /// 紧邻其前的若干字符。
    pub prefix: String,
    /// 紧邻其后的若干字符。
    pub suffix: String,
}

impl Anchor {
    pub fn new(text: &str, span: &Range<usize>) -> Self {
        let before = text.get(span.clone()).unwrap_or_default().to_string();
        let prefix: String = text
            .get(..span.start)
            .unwrap_or_default()
            .chars()
            .rev()
            .take(CONTEXT_CHARS)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let suffix: String = text
            .get(span.end..)
            .unwrap_or_default()
            .chars()
            .take(CONTEXT_CHARS)
            .collect();
        Self {
            before,
            prefix,
            suffix,
        }
    }

    /// 在当前正文里重新找到这条建议指向的位置。
    ///
    /// 判定顺序：原位仍是原文 → 全篇只有这一处同文 → 上下文得分唯一最高的一处
    /// → 上下文完全吻合的若干处里离原位最近的一处。都不成立就返回 `None`，
    /// 由调用方置为 [`RevisionState::Stale`]。
    ///
    /// 关键是**并列最高分即视为无法判定**（除非满分，那说明正文里真有一模一样
    /// 的重复段落，取最近的才对）。宁可置灰让用户自己看，也不能猜——猜错的代价
    /// 是把正文改坏，而且改坏的地方未必在视野里。
    pub fn relocate(&self, text: &str, hint: &Range<usize>) -> Option<Range<usize>> {
        if self.before.is_empty() {
            return None;
        }
        if text.get(hint.clone()) == Some(self.before.as_str()) {
            return Some(hint.clone());
        }
        // (得分, 距原位的字节距离, 起点)
        let mut candidates: Vec<(u8, usize, usize)> = Vec::new();
        let mut cursor = 0usize;
        while let Some(found) = text[cursor..].find(&self.before) {
            let start = cursor + found;
            cursor = start + self.before.len();
            if candidates.len() >= MAX_CANDIDATES {
                return None;
            }
            candidates.push((
                self.score_at(text, start),
                start.abs_diff(hint.start),
                start,
            ));
        }
        let single = candidates.len() == 1;
        let best_score = candidates.iter().map(|(score, ..)| *score).max()?;
        let mut winners: Vec<_> = candidates
            .into_iter()
            .filter(|(score, ..)| *score == best_score)
            .collect();
        // 全篇仅此一处同文，没有别的候选可混淆，直接认它。
        if single {
            let (_, _, start) = winners.first()?;
            return Some(*start..start + self.before.len());
        }
        // 多处同文而上下文一点都对不上，无从分辨。
        if best_score == 0 {
            return None;
        }
        if winners.len() > 1 {
            // 只有前后文都完全吻合时，「取最近的一处」才是有依据的选择。
            if best_score < 2 {
                return None;
            }
            winners.sort_by_key(|(_, distance, _)| *distance);
        }
        let (_, _, start) = winners.first()?;
        Some(*start..start + self.before.len())
    }

    /// 前后文各吻合记 1 分，满分 2 分。上下文为空视为吻合——正文开头结尾的命中
    /// 本来就没有前文或后文可比。
    fn score_at(&self, text: &str, start: usize) -> u8 {
        let end = start + self.before.len();
        let prefix_ok = self.prefix.is_empty()
            || text
                .get(..start)
                .is_some_and(|head| head.ends_with(self.prefix.as_str()));
        let suffix_ok = self.suffix.is_empty()
            || text
                .get(end..)
                .is_some_and(|tail| tail.starts_with(self.suffix.as_str()));
        u8::from(prefix_ok) + u8::from(suffix_ok)
    }
}

/// 一条待确认的修订建议。
#[derive(Debug, Clone)]
pub struct Revision {
    pub id: RevisionId,
    /// 正文字节范围。每轮重定位后刷新。
    pub span: Range<usize>,
    pub anchor: Anchor,
    /// 可直接写回正文的替换文本；`None` 表示只提示、不给改法（语病、结构问题
    /// 常见——「删去『通过』或『使』」这类操作说明拿去替换只会改坏正文）。
    pub after: Option<String>,
    /// 给用户看的理由。没有理由的建议不予展示：用户无法判断就等于无法确认。
    pub reason: String,
    pub source: RevisionSource,
    /// 分类标签，直接沿用词表与规则里的分组名（错别字、称谓规范……）。
    pub group: String,
    pub severity: Level,
    /// 规则来源恒为 1.0；模型来源低于 1.0，界面据此标出「模型判断，需人工复核」。
    pub confidence: f32,
    pub state: RevisionState,
}

impl Revision {
    pub fn before(&self) -> &str {
        &self.anchor.before
    }

    /// 能不能一键采纳。
    pub fn is_actionable(&self) -> bool {
        self.state == RevisionState::Pending && self.after.is_some()
    }

    /// 忽略记录挂靠的稳定键：来源 + 原文。不能只用来源编号——同一条词表规则
    /// 在一篇稿子里可能命中多处，用户忽略的是「这一处」还是「这类写法」，
    /// 按原文区分才对得上。
    pub fn ignore_key(&self) -> String {
        format!("{}\u{1}{}", self.source.key(), self.anchor.before)
    }
}

/// 模型检查器交上来的一条建议。
///
/// 形状定在契约这一侧，而不是各检查器自己定：`span` 必须是**检查器算好的**
/// 全文字节范围——模型只回改后的整句，偏移量由检查器对原句求差得出，这条
/// 规矩写在这里才拦得住后来者顺手把模型给的位置传进来。
#[derive(Debug, Clone)]
pub struct ModelSuggestion {
    pub span: Range<usize>,
    pub before: String,
    pub after: String,
    pub reason: String,
    /// 检查器 id。进 [`RevisionSource::Model`]，也是忽略键的前半段。
    pub task: String,
    /// 分组名，与词表的分组并列显示。
    pub group: String,
}

/// 已采纳的一条建议，供撤销。
#[derive(Debug, Clone)]
struct AppliedRevision {
    /// 落地之后这段文字在正文里的范围。
    span: Range<usize>,
    before: String,
    after: String,
    label: String,
}

/// 一篇稿子的修订建议集合。
#[derive(Debug, Default)]
pub struct RevisionSet {
    items: Vec<Revision>,
    applied: Vec<AppliedRevision>,
    ignored: BTreeSet<String>,
    next_id: RevisionId,
}

impl RevisionSet {
    pub fn items(&self) -> &[Revision] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 待确认且给得出改法的条数——「采纳全部必错」按钮的计数就是它的子集。
    pub fn pending_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.state == RevisionState::Pending)
            .count()
    }

    pub fn actionable_ids(&self, severity: Level) -> Vec<RevisionId> {
        self.items
            .iter()
            .filter(|item| item.is_actionable() && item.severity == severity)
            .map(|item| item.id)
            .collect()
    }

    /// 模型来源的待确认条数。复核完成后的状态栏只该报这个数，不能报总数——
    /// 把词表命中也算进「复核发现」会让人以为模型查出了它根本没查的东西。
    pub fn model_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| !item.source.is_rule() && item.state == RevisionState::Pending)
            .count()
    }

    /// 锚不回正文、只能等重新校验的条数。
    pub fn stale_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.state == RevisionState::Stale)
            .count()
    }

    /// 本次会话已采纳的条数。
    pub fn applied_count(&self) -> usize {
        self.applied.len()
    }

    pub fn last_applied_label(&self) -> Option<&str> {
        self.applied.last().map(|item| item.label.as_str())
    }

    /// 换稿、清空正文时整体复位。忽略记录跟着这一篇走，一并清掉。
    pub fn clear(&mut self) {
        self.items.clear();
        self.applied.clear();
        self.ignored.clear();
    }

    /// 丢掉全部规则来源的建议，为新一轮确定性校验腾位置；模型来源的保留，
    /// 只重定位——它们跑一次要花几秒到几十秒，不能每次校验都重来。
    pub fn begin_rule_pass(&mut self, text: &str) {
        self.items.retain(|item| !item.source.is_rule());
        self.relocate_all(text);
    }

    /// 把一批确定性检查结果收进来。`permanent` 是配置里的永久忽略键。
    pub fn push_notes(
        &mut self,
        notes: Vec<ProofNote>,
        rule: bool,
        text: &str,
        permanent: &[String],
    ) {
        for note in notes {
            let anchor = Anchor::new(text, &note.span);
            if anchor.before.is_empty() {
                continue;
            }
            let source = if rule {
                RevisionSource::DocRule {
                    rule_id: note.entry_id.clone(),
                }
            } else {
                RevisionSource::Lexicon {
                    entry_id: note.entry_id.clone(),
                }
            };
            let key = format!("{}\u{1}{}", source.key(), anchor.before);
            if self.ignored.contains(&key) || permanent.contains(&key) {
                continue;
            }
            let id = self.next_id;
            self.next_id += 1;
            self.items.push(Revision {
                id,
                span: note.span,
                anchor,
                after: note.replacement,
                reason: note.message,
                source,
                group: note.group,
                severity: note.level,
                confidence: 1.0,
                state: RevisionState::Pending,
            });
        }
    }

    /// 一轮收集结束：按位置、级别排序，位置相同的去重。
    pub fn end_rule_pass(&mut self) {
        self.sort_items();
    }

    fn sort_items(&mut self) {
        self.items.sort_by(|a, b| {
            a.span
                .start
                .cmp(&b.span.start)
                .then(a.severity.cmp(&b.severity))
                .then(a.source.key().cmp(b.source.key()))
        });
        self.items
            .dedup_by(|a, b| a.span == b.span && a.reason == b.reason);
    }

    /// 用一轮模型复核的结果整批替换模型来源的建议。
    ///
    /// 整批换而不是增量并：一轮复核就是对当前正文的一次完整判断，留着上一轮的
    /// 残余只会让用户看见早已不成立的建议。规则来源的不受影响。
    ///
    /// 模型建议一律记「疑似」档：小模型的判断没有词表那种确定性，给「必错」
    /// 就意味着它会进「采纳全部必错」的批量口子，那是不该给模型的权限。
    /// 返回**因正文对不上而丢弃**的条数。复核跑一轮要几秒到几十秒，用户完全
    /// 可能在这期间改了稿子；改动之前的部分一改，后面所有 span 就都偏了。这种
    /// 丢弃必须报到界面上——静默吞掉结果比丢结果本身更糟，用户会以为模型什么
    /// 都没查出来。
    pub fn replace_model(
        &mut self,
        suggestions: Vec<ModelSuggestion>,
        text: &str,
        permanent: &[String],
    ) -> usize {
        self.items.retain(|item| item.source.is_rule());
        let mut dropped = 0usize;
        for item in suggestions {
            let anchor = Anchor::new(text, &item.span);
            // 检查器算的 span 必须真能切回这段原文，否则要么是它算错了，要么是
            // 正文在复核期间被改过。两种情况都不能按这个位置下刀。
            if anchor.before.is_empty() || anchor.before != item.before {
                dropped += 1;
                continue;
            }
            let source = RevisionSource::Model {
                task: item.task.clone(),
            };
            let key = format!("{}\u{1}{}", source.key(), anchor.before);
            if self.ignored.contains(&key) || permanent.contains(&key) {
                continue;
            }
            let id = self.next_id;
            self.next_id += 1;
            self.items.push(Revision {
                id,
                span: item.span,
                anchor,
                after: Some(item.after),
                reason: item.reason,
                source,
                group: item.group,
                severity: Level::Suspect,
                confidence: 0.7,
                state: RevisionState::Pending,
            });
        }
        self.sort_items();
        dropped
    }

    /// 正文改动后重新锚定全部建议，锚不上的置灰。
    pub fn relocate_all(&mut self, text: &str) {
        for item in &mut self.items {
            match item.anchor.relocate(text, &item.span) {
                Some(span) => {
                    item.span = span;
                    if item.state == RevisionState::Stale {
                        item.state = RevisionState::Pending;
                    }
                }
                None => item.state = RevisionState::Stale,
            }
        }
    }

    /// 本篇忽略一条。
    pub fn ignore(&mut self, id: RevisionId) -> Option<String> {
        let item = self.items.iter_mut().find(|item| item.id == id)?;
        item.state = RevisionState::Ignored;
        let key = item.ignore_key();
        self.ignored.insert(key.clone());
        self.items.retain(|item| item.id != id);
        Some(key)
    }

    /// 采纳一条建议。
    ///
    /// `verify` 拿到 (改前全文, 改后全文)，由调用方用确定性规则判断这次替换有没有
    /// 引入新问题；返回 `Err` 则整条放弃，正文一个字都不动。
    pub fn apply<F>(
        &mut self,
        id: RevisionId,
        text: &mut String,
        verify: F,
    ) -> Result<String, String>
    where
        F: FnOnce(&str, &str) -> Result<(), String>,
    {
        let index = self
            .items
            .iter()
            .position(|item| item.id == id)
            .ok_or_else(|| "该建议已不在列表中。".to_string())?;
        let item = &self.items[index];
        let after = item
            .after
            .clone()
            .ok_or_else(|| "这条只作提示，没有可直接替换的写法。".to_string())?;
        // 采纳这一刻再锚一次：从上次校验到现在，用户完全可能已经改过正文。
        let span = item
            .anchor
            .relocate(text, &item.span)
            .ok_or_else(|| "原文已改动，该建议无法定位，请重新校验。".to_string())?;
        let before = item.anchor.before.clone();
        let label = format!("{before} → {after}");

        let mut candidate = String::with_capacity(text.len() + after.len());
        candidate.push_str(&text[..span.start]);
        candidate.push_str(&after);
        candidate.push_str(&text[span.end..]);
        verify(text, &candidate)?;

        *text = candidate;
        self.applied.push(AppliedRevision {
            span: span.start..span.start + after.len(),
            before,
            after,
            label: label.clone(),
        });
        self.items.remove(index);
        self.relocate_all(text);
        Ok(label)
    }

    /// 撤销最近一次采纳。正文在那之后被改过就拒绝撤销——盲目写回会覆盖用户
    /// 自己的修改。
    pub fn undo_last(&mut self, text: &mut String) -> Result<String, String> {
        let last = self
            .applied
            .last()
            .ok_or_else(|| "本次还没有采纳过任何建议。".to_string())?;
        if text.get(last.span.clone()) != Some(last.after.as_str()) {
            return Err("那一处正文已被改动，不再撤销，以免覆盖你自己的修改。".into());
        }
        let last = self.applied.pop().expect("刚确认过非空");
        text.replace_range(last.span.clone(), &last.before);
        self.relocate_all(text);
        Ok(last.label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(
        entry_id: &str,
        span: Range<usize>,
        level: Level,
        replacement: Option<&str>,
    ) -> ProofNote {
        ProofNote {
            entry_id: entry_id.into(),
            group: "错别字".into(),
            level,
            message: "建议改为「部署」".into(),
            span,
            replacement: replacement.map(str::to_string),
        }
    }

    fn set_with(text: &str, notes: Vec<ProofNote>) -> RevisionSet {
        let mut set = RevisionSet::default();
        set.begin_rule_pass(text);
        set.push_notes(notes, false, text, &[]);
        set.end_rule_pass();
        set
    }

    #[test]
    fn applying_a_revision_replaces_only_that_span() {
        let mut text = String::from("按上级布署办理。");
        let start = text.find("布署").expect("样本含命中");
        let mut set = set_with(
            &text,
            vec![note(
                "TYP-001",
                start..start + 6,
                Level::MustFix,
                Some("部署"),
            )],
        );
        let id = set.items()[0].id;
        let label = set
            .apply(id, &mut text, |_, _| Ok(()))
            .expect("应当采纳成功");
        assert_eq!(text, "按上级部署办理。");
        assert_eq!(label, "布署 → 部署");
        assert!(set.is_empty(), "采纳后该条应离开待确认列表");
    }

    #[test]
    fn verify_failure_leaves_the_text_untouched() {
        // 采纳即自检的核心保证：闸门不过，正文一个字都不能动。
        let mut text = String::from("按上级布署办理。");
        let start = text.find("布署").expect("样本含命中");
        let mut set = set_with(
            &text,
            vec![note(
                "TYP-001",
                start..start + 6,
                Level::MustFix,
                Some("部署"),
            )],
        );
        let id = set.items()[0].id;
        let error = set
            .apply(id, &mut text, |_, _| Err("引入新问题".into()))
            .expect_err("闸门应当拦下");
        assert_eq!(error, "引入新问题");
        assert_eq!(text, "按上级布署办理。");
        assert_eq!(set.pending_count(), 1, "被拦下的建议应当留在列表里");
    }

    #[test]
    fn undo_restores_the_original_wording() {
        let mut text = String::from("按上级布署办理。");
        let start = text.find("布署").expect("样本含命中");
        let mut set = set_with(
            &text,
            vec![note(
                "TYP-001",
                start..start + 6,
                Level::MustFix,
                Some("部署"),
            )],
        );
        let id = set.items()[0].id;
        set.apply(id, &mut text, |_, _| Ok(()))
            .expect("应当采纳成功");
        set.undo_last(&mut text).expect("应当撤销成功");
        assert_eq!(text, "按上级布署办理。");
        assert_eq!(set.applied_count(), 0);
    }

    #[test]
    fn undo_refuses_after_the_user_edited_that_spot() {
        let mut text = String::from("按上级布署办理。");
        let start = text.find("布署").expect("样本含命中");
        let mut set = set_with(
            &text,
            vec![note(
                "TYP-001",
                start..start + 6,
                Level::MustFix,
                Some("部署"),
            )],
        );
        let id = set.items()[0].id;
        set.apply(id, &mut text, |_, _| Ok(()))
            .expect("应当采纳成功");
        text = String::from("按上级安排办理。");
        assert!(set.undo_last(&mut text).is_err(), "改过之后不该盲目写回");
        assert_eq!(text, "按上级安排办理。");
    }

    #[test]
    fn anchor_follows_text_inserted_before_it() {
        // 用户在命中位置之前插入一段话——建议必须跟着挪，而不是按旧偏移改坏正文。
        let text = "按上级布署办理。";
        let start = text.find("布署").expect("样本含命中");
        let anchor = Anchor::new(text, &(start..start + 6));
        let edited = String::from("经研究决定，现予明确。按上级布署办理。");
        let moved = anchor
            .relocate(&edited, &(start..start + 6))
            .expect("应当重新锚上");
        assert_eq!(&edited[moved], "布署");
    }

    #[test]
    fn anchor_picks_the_occurrence_whose_context_matches() {
        // 同一个词出现两次，靠前后文区分是哪一处。
        let text = "上级布署已明确，下级布署尚未落实。";
        let second = text.rfind("布署").expect("样本含两处命中");
        let anchor = Anchor::new(text, &(second..second + 6));
        // 在两处之间插入文字，旧偏移量整体失效。
        let edited = text.replace("已明确", "已经明确并逐项分解到岗");
        let moved = anchor
            .relocate(&edited, &(second..second + 6))
            .expect("应当重新锚上");
        assert!(
            edited[..moved.start].ends_with("下级"),
            "锚到的应是后一处，实际前文为 {:?}",
            &edited[..moved.start]
        );
    }

    #[test]
    fn anchor_gives_up_when_candidates_tie_on_a_partial_match() {
        // 两处同文，前文都对得上、后文都对不上——得分并列且不满分，谁是原来那一处
        // 无从判断。此时必须置灰：挑一个改，改错的地方还未必在用户视野里。
        let pad = |ch: char| std::iter::repeat_n(ch, CONTEXT_CHARS).collect::<String>();
        let original = format!("{}布署{}", pad('甲'), pad('乙'));
        let start = original.find("布署").expect("样本含命中");
        let anchor = Anchor::new(&original, &(start..start + 6));

        // 后文整段被改写，且同样的前文出现了两次。开头再加一句，让原位偏移
        // 也失效——否则原位恰好还是「布署」，第一级判定就直接命中了。
        let edited = format!("前言。\n{a}布署{b}{a}布署{b}", a = pad('甲'), b = pad('丁'));
        assert!(
            anchor.relocate(&edited, &(start..start + 6)).is_none(),
            "并列得分且不满分时应当放弃重定位"
        );
    }

    #[test]
    fn anchor_gives_up_when_the_original_wording_is_gone() {
        let text = "按上级布署办理。";
        let anchor = Anchor::new(text, &(9..15));
        assert!(anchor.relocate("按上级部署办理。", &(9..15)).is_none());
    }

    #[test]
    fn ignored_entries_do_not_come_back_next_pass() {
        let text = String::from("按上级布署办理。");
        let start = text.find("布署").expect("样本含命中");
        let notes = || {
            vec![note(
                "TYP-001",
                start..start + 6,
                Level::MustFix,
                Some("部署"),
            )]
        };
        let mut set = set_with(&text, notes());
        let id = set.items()[0].id;
        set.ignore(id).expect("应当记下忽略");
        set.begin_rule_pass(&text);
        set.push_notes(notes(), false, &text, &[]);
        set.end_rule_pass();
        assert!(set.is_empty(), "忽略过的条目不该在下一轮校验里复活");
    }

    #[test]
    fn permanent_ignore_list_is_honoured() {
        let text = String::from("按上级布署办理。");
        let start = text.find("布署").expect("样本含命中");
        let mut set = RevisionSet::default();
        set.begin_rule_pass(&text);
        set.push_notes(
            vec![note(
                "TYP-001",
                start..start + 6,
                Level::MustFix,
                Some("部署"),
            )],
            false,
            &text,
            &["TYP-001\u{1}布署".to_string()],
        );
        set.end_rule_pass();
        assert!(set.is_empty());
    }

    #[test]
    fn hint_only_notes_are_listed_but_not_actionable() {
        let text = String::from("通过这次整治，使形势好转。");
        let start = text.find("通过").expect("样本含命中");
        let mut set = set_with(
            &text,
            vec![note("GRM-001", start..start + 6, Level::Suspect, None)],
        );
        assert_eq!(set.pending_count(), 1);
        assert!(!set.items()[0].is_actionable());
        let id = set.items()[0].id;
        let mut text2 = text.clone();
        assert!(set.apply(id, &mut text2, |_, _| Ok(())).is_err());
        assert_eq!(text2, text);
    }
}
