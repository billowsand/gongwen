//! 缺口台账：工作稿里「还缺什么」的清单，研究式起草围着它循环。
//!
//! 缺口由程序确定性地找出来，不靠模型自觉：
//! - 「【待核实：…】」占位——提示词早已统一了写法（`prompt.rs` 有测试锁住前缀）；
//! - 来源不明的事实——`ai_guard` 抽出的日期、数量、书名号文件名，在材料、已确认的回答、
//!   证据包与要素里都找不到。

use super::evidence::{CITATION, strip_citations};
use crate::ai_guard::{FactKind, FactToken};
use crate::models::VocabularyEntry;
use regex::Regex;
use std::ops::Range;
use std::sync::LazyLock;

/// 「【待核实】」「【待核实：会议时间】」。冒号全角半角都认，提示可以为空。
static PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"【待核实[：:]?([^】]*)】").expect("占位正则"));

/// 句末标点。
fn is_sentence_end(ch: char) -> bool {
    matches!(ch, '。' | '！' | '？' | '；' | '!' | '?')
}

/// 把正文切成句子（字节区间）。按行、再按句末标点切；句号后紧跟的引用标记 `[K3]`
/// 归前一句——模型常把标记写在句号后面。首尾空白不计入。
pub(crate) fn sentence_spans(text: &str) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let mut line_start = 0usize;
    for line in text.split('\n') {
        let mut start = 0usize;
        let mut iter = line.char_indices().peekable();
        while let Some((index, ch)) = iter.next() {
            if !is_sentence_end(ch) {
                continue;
            }
            let mut end = index + ch.len_utf8();
            if let Some(found) = CITATION.find(&line[end..])
                && found.start() == 0
            {
                end += found.end();
                while iter.peek().is_some_and(|(next, _)| *next < end) {
                    iter.next();
                }
            }
            push_trimmed(line, line_start, start..end, &mut spans);
            start = end;
        }
        push_trimmed(line, line_start, start..line.len(), &mut spans);
        line_start += line.len() + 1;
    }
    spans
}

fn push_trimmed(line: &str, line_start: usize, range: Range<usize>, out: &mut Vec<Range<usize>>) {
    let raw = &line[range.clone()];
    let lead = raw.len() - raw.trim_start().len();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return;
    }
    let start = line_start + range.start + lead;
    out.push(start..start + trimmed.len());
}

/// 包含 `pos` 的那一句；找不到时退回所在行。
pub(crate) fn sentence_at(text: &str, pos: usize) -> Range<usize> {
    sentence_spans(text)
        .into_iter()
        .find(|span| span.start <= pos && pos < span.end)
        .unwrap_or_else(|| {
            let start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
            let end = text[pos..].find('\n').map_or(text.len(), |i| pos + i);
            start..end
        })
}

/// 正文里的一处占位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Placeholder {
    /// 占位全文，如「【待核实：会议时间】」。
    pub(crate) literal: String,
    /// 冒号后的提示，如「会议时间」；可能为空。
    pub(crate) hint: String,
    pub(crate) span: Range<usize>,
}

pub(crate) fn find_placeholders(text: &str) -> Vec<Placeholder> {
    PLACEHOLDER
        .captures_iter(text)
        .map(|caps| {
            let whole = caps.get(0).expect("整段匹配");
            Placeholder {
                literal: whole.as_str().to_string(),
                hint: caps[1].trim().to_string(),
                span: whole.range(),
            }
        })
        .collect()
}

/// 缺口的类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GapKind {
    /// 政策依据、上级表述、背景情况、通行做法：可以去知识库找。
    Retrievable,
    /// 本次的时间、地点、人员、数额：只有用户知道。**不去检索**——硬搜只会把旧稿里的
    /// 旧日期找回来当成新事实，正好踩「旧稿事实不得沿用」的红线。
    NeedsUser,
    /// 稿里出现了、却找不到出处的事实（模型可能编的）。
    Untraced,
}

impl GapKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Retrievable => "可检索",
            Self::NeedsUser => "需你提供",
            Self::Untraced => "来源不明",
        }
    }
}

/// 缺口的处理状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GapStatus {
    Open,
    /// 用证据补全了，附证据编号。
    Resolved(Vec<usize>),
    /// 检索到头也没找到。
    NoAnswer,
    /// 用户给了答案。
    Answered(String),
    /// 用户确认原文无误。
    Kept,
    /// 用户选择保留待核实。
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Gap {
    pub(crate) id: usize,
    pub(crate) kind: GapKind,
    /// 给人看的缺什么：占位提示，或来源不明的那个事实。
    pub(crate) hint: String,
    /// 在正文里的字面：占位全文，或事实原文。按它定位、按它替换。
    pub(crate) literal: String,
    /// 所在句（当前版本）。
    pub(crate) sentence: String,
    pub(crate) status: GapStatus,
    /// 已检索的次数。
    pub(crate) attempts: u8,
    /// 检索时用过的检索词。
    pub(crate) queries: Vec<String>,
    /// 检索返回过的资料（证据键）；再搜回来的全是旧资料，就算检索到头了。
    pub(crate) seen_chunks: Vec<String>,
}

/// 「【待核实：X】」该交给知识库还是交给用户。
///
/// 先看是不是只有用户知道的本次安排；「核对原文」「示意」这类明说要查资料的除外。
/// 判不出来的默认去检索：检索不到的最后也会出题问用户，不会丢。
pub(crate) fn classify(hint: &str, sentence: &str) -> GapKind {
    const RETRIEVE_FIRST: [&str; 3] = ["原文", "示意", "依据"];
    const USER_ONLY: [&str; 34] = [
        "时间",
        "日期",
        "截止",
        "期限",
        "时限",
        "几月",
        "几日",
        "地点",
        "会场",
        "参加",
        "参会",
        "人员",
        "名单",
        "联系人",
        "电话",
        "手机",
        "邮箱",
        "金额",
        "经费",
        "预算",
        "资金",
        "数额",
        "人数",
        "负责人",
        "责任单位",
        "牵头单位",
        "承办单位",
        "主办单位",
        "文号",
        "编号",
        "报送",
        "受理",
        "地址",
        "联系",
    ];
    if RETRIEVE_FIRST.iter().any(|word| hint.contains(word)) {
        return GapKind::Retrievable;
    }
    // 「吨数」「户数」「次数」：本次工作的具体数量。
    let counts_this_work = hint.chars().count() >= 2 && hint.ends_with('数');
    if counts_this_work || USER_ONLY.iter().any(|word| hint.contains(word)) {
        return GapKind::NeedsUser;
    }
    // 提示为空时看所在句：句子里在讲时间地点人员，多半也是本次安排。
    if hint.is_empty() && USER_ONLY.iter().any(|word| sentence.contains(word)) {
        return GapKind::NeedsUser;
    }
    GapKind::Retrievable
}

/// 稿里来源不明的事实：日期时间、数量、书名号文件名，在 `sources` 里都找不到。
///
/// 单位与人名不算：它们来自用户维护的标准词库，本身就是出处。
pub(crate) fn untraced_facts(
    text: &str,
    sources: &str,
    vocabulary: &[VocabularyEntry],
) -> Vec<FactToken> {
    let clean = strip_citations(text);
    let haystack = squash(sources);
    let facts: Vec<FactToken> = crate::ai_guard::extract_key_facts(&clean, vocabulary)
        .into_iter()
        .filter(|fact| {
            matches!(
                fact.kind,
                FactKind::DateTime | FactKind::Number | FactKind::Document
            )
        })
        .collect();
    // 「2026年10月23日」里还会再抽出「23日」「10月」「2026年」。这些碎片单独去找出处，
    // 随便哪段证据里有个「23日」就会被误认作有出处（实测踩过），所以只留最长的那个。
    let whole: Vec<String> = facts.iter().map(|fact| fact.value.clone()).collect();
    facts
        .into_iter()
        .filter(|fact| {
            !whole
                .iter()
                .any(|other| other.len() > fact.value.len() && other.contains(&fact.value))
        })
        .filter(|fact| !haystack.contains(&squash(&fact.value)))
        .collect()
}

/// 去掉空白再比：「12 月 1 日」与「12月1日」算同一个。
fn squash(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_whitespace()).collect()
}

/// 缺口台账。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Ledger {
    pub(crate) gaps: Vec<Gap>,
    next_id: usize,
}

impl Ledger {
    /// 对照当前工作稿刷新台账：新出现的占位与来源不明事实入账，已在账上的更新所在句。
    /// 返回新增条数。
    pub(crate) fn sync(
        &mut self,
        text: &str,
        sources: &str,
        vocabulary: &[VocabularyEntry],
    ) -> usize {
        let mut added = 0;
        for placeholder in find_placeholders(text) {
            let span = sentence_at(text, placeholder.span.start);
            let sentence = text[span].to_string();
            if let Some(gap) = self
                .gaps
                .iter_mut()
                .find(|gap| gap.kind != GapKind::Untraced && gap.literal == placeholder.literal)
            {
                // 同一个占位补过了却还在：别处还有一处，重新打开。
                if matches!(gap.status, GapStatus::Resolved(_)) {
                    gap.status = GapStatus::Open;
                }
                gap.sentence = sentence;
                continue;
            }
            let kind = classify(&placeholder.hint, &sentence);
            let hint = if placeholder.hint.is_empty() {
                "待核实内容".to_string()
            } else {
                placeholder.hint.clone()
            };
            self.push(kind, hint, placeholder.literal, sentence);
            added += 1;
        }
        for fact in untraced_facts(text, sources, vocabulary) {
            let Some(pos) = text.find(&fact.value) else {
                continue;
            };
            let sentence = text[sentence_at(text, pos)].to_string();
            if let Some(gap) = self
                .gaps
                .iter_mut()
                .find(|gap| gap.kind == GapKind::Untraced && gap.literal == fact.value)
            {
                gap.sentence = sentence;
                continue;
            }
            self.push(GapKind::Untraced, fact.value.clone(), fact.value, sentence);
            added += 1;
        }
        added
    }

    fn push(&mut self, kind: GapKind, hint: String, literal: String, sentence: String) {
        self.next_id += 1;
        self.gaps.push(Gap {
            id: self.next_id,
            kind,
            hint,
            literal,
            sentence,
            status: GapStatus::Open,
            attempts: 0,
            queries: Vec::new(),
            seen_chunks: Vec::new(),
        });
    }

    /// 核验时发现引用不支持的事实：直接记为「找不到出处」，交用户确认。
    pub(crate) fn add_unsupported(&mut self, value: &str, sentence: &str) {
        if let Some(gap) = self
            .gaps
            .iter_mut()
            .find(|gap| gap.kind == GapKind::Untraced && gap.literal == value)
        {
            gap.status = GapStatus::NoAnswer;
            gap.sentence = sentence.to_string();
            return;
        }
        self.push(
            GapKind::Untraced,
            value.to_string(),
            value.to_string(),
            sentence.to_string(),
        );
        if let Some(gap) = self.gaps.last_mut() {
            gap.status = GapStatus::NoAnswer;
        }
    }

    pub(crate) fn get(&self, id: usize) -> Option<&Gap> {
        self.gaps.iter().find(|gap| gap.id == id)
    }

    pub(crate) fn get_mut(&mut self, id: usize) -> Option<&mut Gap> {
        self.gaps.iter_mut().find(|gap| gap.id == id)
    }

    /// 这一轮该去知识库找的：仍然开着、不是只有用户知道的、检索次数没用完。
    pub(crate) fn retrieval_targets(&self, max_attempts: u8) -> Vec<usize> {
        self.gaps
            .iter()
            .filter(|gap| {
                gap.status == GapStatus::Open
                    && gap.kind != GapKind::NeedsUser
                    && gap.attempts < max_attempts
            })
            .map(|gap| gap.id)
            .collect()
    }

    /// 要出题问用户的：需你提供的、检索到头的、来源不明且没核实到的。
    pub(crate) fn needs_user(&self) -> Vec<usize> {
        self.gaps
            .iter()
            .filter(|gap| match gap.status {
                GapStatus::NoAnswer => true,
                GapStatus::Open => gap.kind == GapKind::NeedsUser,
                _ => false,
            })
            .map(|gap| gap.id)
            .collect()
    }

    /// (已补全, 用户已处理, 待处理)
    pub(crate) fn counts(&self) -> (usize, usize, usize) {
        let mut resolved = 0;
        let mut handled = 0;
        let mut pending = 0;
        for gap in &self.gaps {
            match gap.status {
                GapStatus::Resolved(_) => resolved += 1,
                GapStatus::Answered(_) | GapStatus::Kept | GapStatus::Skipped => handled += 1,
                GapStatus::Open | GapStatus::NoAnswer => pending += 1,
            }
        }
        (resolved, handled, pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(text: &str) -> Vec<&str> {
        sentence_spans(text)
            .into_iter()
            .map(|span| &text[span])
            .collect()
    }

    #[test]
    fn sentences_split_on_punctuation_and_keep_trailing_citations() {
        let text = "# 标题\n  一、各地要加强巡查。已覆盖全部林区。[K3] 请于月底前报送！\n";
        assert_eq!(
            texts(text),
            [
                "# 标题",
                "一、各地要加强巡查。",
                "已覆盖全部林区。[K3]",
                "请于月底前报送！"
            ]
        );
        let at = text.find("覆盖").unwrap();
        assert_eq!(&text[sentence_at(text, at)], "已覆盖全部林区。[K3]");
    }

    #[test]
    fn placeholders_are_found_with_or_without_hints() {
        let text = "于【待核实：排查完成时限】前完成，共清运【待核实：吨数】吨【待核实】。";
        let found = find_placeholders(text);
        let hints: Vec<_> = found.iter().map(|p| p.hint.as_str()).collect();
        assert_eq!(hints, ["排查完成时限", "吨数", ""]);
        assert_eq!(found[0].literal, "【待核实：排查完成时限】");
        assert_eq!(&text[found[1].span.clone()], "【待核实：吨数】");
    }

    #[test]
    fn this_works_specifics_go_to_the_user_not_the_knowledge_base() {
        assert_eq!(classify("排查完成时限", ""), GapKind::NeedsUser);
        assert_eq!(classify("会议地点", ""), GapKind::NeedsUser);
        assert_eq!(classify("吨数", ""), GapKind::NeedsUser);
        assert_eq!(classify("牵头单位", ""), GapKind::NeedsUser);
        // 实测踩过：报送方式是本次安排，去知识库查只会白跑两趟。
        assert_eq!(classify("报送方式和受理部门", ""), GapKind::NeedsUser);
        assert_eq!(classify("上级文件依据", ""), GapKind::Retrievable);
        assert_eq!(classify("核对原文", ""), GapKind::Retrievable);
        assert_eq!(classify("示意约××，须核实替换", ""), GapKind::Retrievable);
        assert_eq!(classify("近年火灾情况", ""), GapKind::Retrievable);
        assert_eq!(classify("", "会议时间定于【待核实】"), GapKind::NeedsUser);
    }

    #[test]
    fn facts_found_in_sources_are_not_untraced() {
        let text = "请于2026年12月1日前完成排查，共投入经费300万元[K2]。";
        let sources = "材料：12 月 1 日前完成；证据：投入经费300万元";
        let facts = untraced_facts(text, sources, &[]);
        let values: Vec<_> = facts.iter().map(|fact| fact.value.as_str()).collect();
        assert!(!values.iter().any(|v| v.contains("300")), "{values:?}");
        assert!(
            values.iter().any(|v| v.contains("2026")),
            "年份没有出处就该列出：{values:?}"
        );
    }

    #[test]
    fn fragments_of_a_longer_fact_are_not_tracked_on_their_own() {
        let facts = untraced_facts("请于2026年10月23日前报送。", "", &[]);
        let values: Vec<_> = facts.iter().map(|fact| fact.value.as_str()).collect();
        assert_eq!(values, ["2026年10月23日"], "{values:?}");
    }

    #[test]
    fn ledger_tracks_gaps_across_rounds() {
        let mut ledger = Ledger::default();
        let text = "一、于【待核实：排查完成时限】前完成。二、依据【待核实：上级文件依据】执行。";
        assert_eq!(ledger.sync(text, "", &[]), 2);
        assert_eq!(ledger.gaps[0].kind, GapKind::NeedsUser);
        assert_eq!(ledger.gaps[1].kind, GapKind::Retrievable);
        assert_eq!(ledger.retrieval_targets(2), [2]);
        assert_eq!(ledger.needs_user(), [1]);
        // 再同步一次不重复入账。
        assert_eq!(ledger.sync(text, "", &[]), 0);

        ledger.get_mut(2).unwrap().status = GapStatus::Resolved(vec![3]);
        let filled = "一、于【待核实：排查完成时限】前完成。二、依据《森林防火条例》[K3]执行。";
        // 文件名有 [K3] 但证据文本没传进来：作为来源不明入账。
        assert_eq!(ledger.sync(filled, "", &[]), 1);
        assert_eq!(ledger.gaps[2].kind, GapKind::Untraced);
        // 证据里有，就不算来源不明。
        let mut fresh = Ledger::default();
        fresh.sync(filled, "《森林防火条例》", &[]);
        assert!(fresh.gaps.iter().all(|gap| gap.kind != GapKind::Untraced));
        assert_eq!(ledger.counts(), (1, 0, 2));
    }

    #[test]
    fn a_resolved_placeholder_that_still_appears_elsewhere_reopens() {
        let mut ledger = Ledger::default();
        ledger.sync("依据【待核实：核对原文】。", "", &[]);
        ledger.gaps[0].status = GapStatus::Resolved(vec![1]);
        ledger.sync("依据甲[K1]。又依据【待核实：核对原文】。", "", &[]);
        assert_eq!(ledger.gaps[0].status, GapStatus::Open);
        assert_eq!(ledger.gaps[0].sentence, "又依据【待核实：核对原文】。");
    }
}
