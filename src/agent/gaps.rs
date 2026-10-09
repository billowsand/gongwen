//! 缺口台账：工作稿里「还缺什么」的清单，研究式起草围着它循环。
//!
//! 缺口由程序确定性地找出来，不靠模型自觉：
//! - 「【待核实：…】」占位——提示词早已统一了写法（`prompt.rs` 有测试锁住前缀）；
//! - 来源不明的事实——`ai_guard` 抽出的日期、数量、书名号文件名，在材料、已确认的回答、
//!   证据包与要素里都找不到。
//!
//! 来源不明的事实若在材料里「同一处」写着另一个值（同样的上下文、同类事实），就记成冲突候选
//! （[`Candidate`]），出题时作为程序给的选项（`docs/decision-modules.md` 第八节）。

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

/// `pos` 所在的小节：往前最近的二级及以下标题（去掉 `#`）。一级标题是文题，不算小节；
/// 在第一个小节之前就返回空。
pub(crate) fn section_of(text: &str, pos: usize) -> String {
    let pos = pos.min(text.len());
    let start = (0..=pos)
        .rev()
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(0);
    text[..start]
        .lines()
        .rev()
        .map(str::trim_start)
        .find(|line| line.starts_with("##"))
        .map(|line| line.trim_start_matches('#').trim().to_string())
        .unwrap_or_default()
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// 用户选择删去这一项。
    Dropped,
    /// 知识库查不到，AI 把所在句改写成不依赖这项内容的概括表述；附改写前的原句，撤回用。
    Generalized(String),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// 来源不明的事实在材料里对应的另一个值（事实冲突的候选）；旧台账没有这一项。
    #[serde(default)]
    pub(crate) candidates: Vec<Candidate>,
}

/// 事实冲突的候选：来源不明的事实在材料或资料里「同一处」写的值。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Candidate {
    pub(crate) value: String,
    /// 出处：「材料」或「《题名》」。
    pub(crate) source: String,
    /// 出处里前后一小段原文，给人对照。
    pub(crate) excerpt: String,
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
        .filter(|fact| {
            !equivalent_forms(&fact.value)
                .iter()
                .any(|form| haystack.contains(&squash(form)))
        })
        .collect()
}

static WEEKDAY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[（(]\s*(?:星期|周)[一二三四五六日天]\s*[)）]|(?:星期|周)[一二三四五六日天]")
        .expect("星期正则")
});
static CLOCK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d{1,2})[:：](\d{2})$").expect("时刻正则"));
static SPOKEN_CLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(凌晨|早上|上午|中午|下午|傍晚|晚上)?(\d{1,2})[点时](?:(\d{1,2})分|(半))?$")
        .expect("口语时刻正则")
});

/// 同一个日期或时刻的几种常见写法：材料里写「10月20日下午3点」，稿子里写
/// 「10月20日（星期二）15:00」，说的是同一件事，不该当成来源不明。
fn equivalent_forms(value: &str) -> Vec<String> {
    let mut forms = vec![value.to_string()];
    let without_weekday = WEEKDAY.replace_all(value, "").trim().to_string();
    if without_weekday != value && !without_weekday.is_empty() {
        forms.push(without_weekday);
    }
    let clock = if let Some(caps) = CLOCK.captures(value) {
        caps[1].parse::<u32>().ok().zip(caps[2].parse::<u32>().ok())
    } else if let Some(caps) = SPOKEN_CLOCK.captures(value) {
        let hour = caps[2].parse::<u32>().ok();
        let minute = match (caps.get(3), caps.get(4)) {
            (Some(m), _) => m.as_str().parse::<u32>().ok(),
            (None, Some(_)) => Some(30),
            (None, None) => Some(0),
        };
        let afternoon = matches!(
            caps.get(1).map(|m| m.as_str()),
            Some("下午" | "傍晚" | "晚上")
        );
        hour.zip(minute)
            .map(|(h, m)| (if afternoon && h < 12 { h + 12 } else { h }, m))
    } else {
        None
    };
    if let Some((hour, minute)) = clock.filter(|(h, m)| *h < 24 && *m < 60) {
        let twelve = if hour > 12 { hour - 12 } else { hour };
        let period = match hour {
            0..=5 => "凌晨",
            6..=11 => "上午",
            12 => "中午",
            13..=18 => "下午",
            _ => "晚上",
        };
        forms.push(format!("{hour}:{minute:02}"));
        forms.push(format!("{hour}：{minute:02}"));
        let tail = match minute {
            0 => String::new(),
            30 => "半".to_string(),
            m => format!("{m}分"),
        };
        for unit in ["点", "时"] {
            if minute == 30 && unit == "时" {
                forms.push(format!("{hour}时30分"));
                forms.push(format!("{period}{twelve}时30分"));
                continue;
            }
            forms.push(format!("{hour}{unit}{tail}"));
            forms.push(format!("{period}{twelve}{unit}{tail}"));
        }
    }
    forms
}

/// 去掉空白再比：「12 月 1 日」与「12月1日」算同一个。
fn squash(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_whitespace()).collect()
}

/// 缺口台账。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
            candidates: Vec::new(),
        });
    }

    /// 给还要问的来源不明事实找冲突候选。`sources` 是（出处, 原文）：材料在前，资料在后。
    pub(crate) fn attach_candidates(
        &mut self,
        sources: &[(String, String)],
        vocabulary: &[VocabularyEntry],
    ) {
        for gap in &mut self.gaps {
            if gap.kind == GapKind::Untraced
                && matches!(gap.status, GapStatus::Open | GapStatus::NoAnswer)
            {
                gap.candidates = conflict_candidates(gap, sources, vocabulary);
            }
        }
    }

    /// 正文里现有的「【待核实】」占位，全部记成要问用户的缺口（待决事项）。只认占位，不查来源
    /// 不明的事实：接受进正文的事实起草人已经过目过。
    pub(crate) fn from_placeholders(text: &str) -> Self {
        let mut ledger = Self::default();
        ledger.sync(text, text, &[]);
        for gap in &mut ledger.gaps {
            gap.status = GapStatus::NoAnswer;
        }
        ledger
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

    /// 动笔前起草人对六要素选了「先不定」的，占位按约定写成「【待核实：短名】」，已确认信息里
    /// 记着这个占位：对上的缺口记为保留待核实——不去检索、事后也不再问第二遍。
    pub(crate) fn mark_declined(&mut self, notes: &[String]) {
        for gap in &mut self.gaps {
            if matches!(gap.status, GapStatus::Open | GapStatus::NoAnswer)
                && gap.kind != GapKind::Untraced
                && notes.iter().any(|note| note.contains(&gap.literal))
            {
                gap.status = GapStatus::Skipped;
            }
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

    /// (已补全或概括, 用户已处理, 待处理)
    pub(crate) fn counts(&self) -> (usize, usize, usize) {
        let mut resolved = 0;
        let mut handled = 0;
        let mut pending = 0;
        for gap in &self.gaps {
            match gap.status {
                GapStatus::Resolved(_) | GapStatus::Generalized(_) => resolved += 1,
                GapStatus::Answered(_)
                | GapStatus::Kept
                | GapStatus::Skipped
                | GapStatus::Dropped => handled += 1,
                GapStatus::Open | GapStatus::NoAnswer => pending += 1,
            }
        }
        (resolved, handled, pending)
    }
}

/// 冲突候选最多给几个。
const MAX_CANDIDATES: usize = 2;
/// 上下文取几个字比对。
const ANCHOR_CHARS: usize = 4;
/// 上下文至少几个字才算数（太短的「于」「共」谁都对得上）。
const MIN_ANCHOR: usize = 3;
/// 摘录时事实前后各带几个字。
const EXCERPT_CHARS: usize = 14;

/// 事实前后的上下文：去掉空白，碰到换行或句末标点就停，各取 [`ANCHOR_CHARS`] 个字。
fn context(text: &str, start: usize, end: usize) -> (String, String) {
    let keep = |ch: &char| !ch.is_whitespace() || *ch == '\n';
    let stop = |ch: &char| *ch == '\n' || is_sentence_end(*ch);
    let before: Vec<char> = text[..start]
        .chars()
        .rev()
        .filter(keep)
        .take_while(|ch| !stop(ch))
        .take(ANCHOR_CHARS)
        .collect();
    let after: String = text[end..]
        .chars()
        .filter(keep)
        .take_while(|ch| !stop(ch))
        .take(ANCHOR_CHARS)
        .collect();
    (before.into_iter().rev().collect(), after)
}

/// 两段上下文对得上：以缺口那边的长度为准，比材料那边离事实最近的同样多个字。
fn anchors_match(gap: &str, source: &str, at_end: bool) -> bool {
    let wanted = gap.chars().count();
    if wanted < MIN_ANCHOR || source.chars().count() < wanted {
        return false;
    }
    let source: String = if at_end {
        source
            .chars()
            .skip(source.chars().count() - wanted)
            .collect()
    } else {
        source.chars().take(wanted).collect()
    };
    source == gap
}

/// 事实前后各带几个字的原文摘录，不跨行。
fn excerpt(text: &str, start: usize, end: usize) -> String {
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[end..].find('\n').map_or(text.len(), |i| end + i);
    let before: Vec<char> = text[line_start..start]
        .chars()
        .rev()
        .take(EXCERPT_CHARS)
        .collect();
    let after: String = text[end..line_end].chars().take(EXCERPT_CHARS).collect();
    let before: String = before.into_iter().rev().collect();
    format!(
        "…{}{}{}…",
        before.trim_start(),
        &text[start..end],
        after.trim_end()
    )
}

/// 来源不明的事实在材料里「同一处」写的其它值：同类事实（日期对日期、数量对数量），前文或后文
/// 的几个字一样。纯字面比对，不调模型；对不准的宁可不给，题照样按「来源不明」问。
fn conflict_candidates(
    gap: &Gap,
    sources: &[(String, String)],
    vocabulary: &[VocabularyEntry],
) -> Vec<Candidate> {
    let sentence = strip_citations(&gap.sentence);
    let Some(pos) = sentence.find(&gap.literal) else {
        return Vec::new();
    };
    let Some(kind) = crate::ai_guard::extract_key_facts(&gap.literal, vocabulary)
        .into_iter()
        .find(|fact| fact.value == gap.literal)
        .map(|fact| fact.kind)
    else {
        return Vec::new();
    };
    let (before, after) = context(&sentence, pos, pos + gap.literal.len());
    let same = equivalent_forms(&gap.literal)
        .iter()
        .map(|form| squash(form))
        .collect::<Vec<_>>();
    let mut found: Vec<Candidate> = Vec::new();
    for (source, text) in sources {
        let facts = crate::ai_guard::extract_key_facts(text, vocabulary);
        let values: Vec<&str> = facts
            .iter()
            .filter(|fact| fact.kind == kind)
            .map(|fact| fact.value.as_str())
            .collect();
        for value in &values {
            // 「2026年10月23日」里抽出的「23日」这类碎片不单独算。
            if values
                .iter()
                .any(|other| other.len() > value.len() && other.contains(*value))
                || same.contains(&squash(value))
                || found.iter().any(|c| squash(&c.value) == squash(value))
            {
                continue;
            }
            let hit = text.match_indices(*value).find(|(start, _)| {
                let (b, a) = context(text, *start, start + value.len());
                anchors_match(&before, &b, true) || anchors_match(&after, &a, false)
            });
            if let Some((start, _)) = hit {
                found.push(Candidate {
                    value: value.to_string(),
                    source: source.clone(),
                    excerpt: excerpt(text, start, start + value.len()),
                });
                if found.len() == MAX_CANDIDATES {
                    return found;
                }
            }
        }
    }
    found
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
    fn weekday_and_clock_spellings_count_as_the_same_fact() {
        let draft = "定于2026年10月20日（星期二）15:00召开，14:40开始签到。";
        let untraced: Vec<String> =
            untraced_facts(draft, "会议改在2026年10月20日下午3点召开。", &[])
                .into_iter()
                .map(|fact| fact.value)
                .collect();
        assert!(
            untraced.iter().all(|v| !v.contains("10月20日")),
            "{untraced:?}"
        );
        assert!(
            untraced.iter().all(|v| v != "15:00"),
            "下午3点就是15:00：{untraced:?}"
        );
        assert!(
            untraced.iter().any(|v| v == "14:40"),
            "签到时间是编的：{untraced:?}"
        );
        let forms = equivalent_forms("下午2点半");
        assert!(forms.contains(&"14:30".to_string()), "{forms:?}");
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

    fn sources(material: &str) -> Vec<(String, String)> {
        vec![("材料".to_string(), material.to_string())]
    }

    #[test]
    fn an_untraced_fact_gets_the_material_value_said_in_the_same_place() {
        let draft = "截至9月底，全市共排查火灾隐患12项，均已整改。";
        let material = "据统计，截至9月底全市共排查火灾隐患15项，出动检查人员300人。";
        let mut ledger = Ledger::default();
        ledger.sync(draft, material, &[]);
        ledger.attach_candidates(&sources(material), &[]);
        let gap = ledger
            .gaps
            .iter()
            .find(|gap| gap.literal == "12项")
            .expect("12项来源不明");
        assert_eq!(gap.candidates.len(), 1, "{:?}", gap.candidates);
        let candidate = &gap.candidates[0];
        assert_eq!(candidate.value, "15项");
        assert_eq!(candidate.source, "材料");
        assert!(
            candidate.excerpt.contains("火灾隐患15项"),
            "{}",
            candidate.excerpt
        );
        // 「300人」也是数量，但上下文对不上，不算候选。
        assert!(!gap.candidates.iter().any(|c| c.value == "300人"));

        // 出成题：材料里的值是推荐项，带出处摘录；选了就把稿里的值换掉。
        let question = crate::agent::clarify::gap_question(1, gap, &[]);
        assert!(
            question.text.contains("「12项」与材料"),
            "{}",
            question.text
        );
        let first = &question.choices[0];
        assert_eq!(first.label, "按材料：15项");
        assert!(first.recommended);
        assert!(first.detail.contains("火灾隐患15项"));
        let mut ledger = ledger.clone();
        let fixed = crate::agent::clarify::apply_gap_replies(
            draft,
            &mut ledger,
            &[question],
            &[(1, crate::agent::clarify::Reply::Choice(0))],
        );
        assert_eq!(fixed, "截至9月底，全市共排查火灾隐患15项，均已整改。");
    }

    #[test]
    fn dates_match_on_the_words_after_them_and_unrelated_facts_are_not_candidates() {
        let draft = "请各单位于10月23日前报送材料。";
        let material = "各单位须于11月1日前报送材料。会议定于12月5日召开。";
        let mut ledger = Ledger::default();
        ledger.sync(draft, material, &[]);
        ledger.attach_candidates(&sources(material), &[]);
        let values: Vec<&str> = ledger.gaps[0]
            .candidates
            .iter()
            .map(|c| c.value.as_str())
            .collect();
        assert_eq!(values, ["11月1日"]);

        // 材料里只有毫不相干的数字：不给候选，照旧按「来源不明」问。
        let mut ledger = Ledger::default();
        ledger.sync(draft, "会议定于12月5日召开。", &[]);
        ledger.attach_candidates(&sources("会议定于12月5日召开。"), &[]);
        assert!(ledger.gaps[0].candidates.is_empty());
    }

    #[test]
    fn pending_ledger_lists_every_placeholder_in_the_document_for_the_user() {
        let text =
            "一、于【待核实：排查完成时限】前完成。二、依据【待核实：上级文件依据】执行，共12起。";
        let ledger = Ledger::from_placeholders(text);
        let hints: Vec<&str> = ledger.gaps.iter().map(|gap| gap.hint.as_str()).collect();
        assert_eq!(
            hints,
            ["排查完成时限", "上级文件依据"],
            "不查来源不明的事实"
        );
        assert_eq!(ledger.needs_user().len(), 2, "可检索的占位也问用户");
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
    fn the_section_is_the_nearest_heading_below_the_title() {
        let text = "# 关于某事的函\n\n开头。\n\n## 工作安排\n\n请于【待核实：时限】前反馈。\n";
        assert_eq!(section_of(text, text.find("开头").unwrap()), "");
        assert_eq!(section_of(text, text.find("【待核实").unwrap()), "工作安排");
    }

    #[test]
    fn items_declined_before_drafting_are_not_asked_again() {
        let mut ledger = Ledger::default();
        ledger.sync(
            "请于【待核实：回复时限】前函复，联系人【待核实：联系人】。",
            "",
            &[],
        );
        ledger.mark_declined(&[
            "回复时限暂未确定：正文写「【待核实：回复时限】」，不要自己编".into(),
        ]);
        assert_eq!(ledger.gaps[0].status, GapStatus::Skipped);
        assert_eq!(ledger.needs_user(), [2]);
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
