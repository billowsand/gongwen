//! 固定阶段的公文写作任务与执行入口。

use crate::models::TemplateKind;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

mod assemble;
mod execution;
pub(crate) mod store;
use execution::chunk_ranges;
pub(crate) use execution::{ONE_SHOT_MAX_CHARS, run};

#[cfg(test)]
mod tests;

const MAX_SOURCE_BYTES: usize = 16 * 1024 * 1024;
const MAX_TOTAL_SOURCE_BYTES: usize = 64 * 1024 * 1024;
/// 单块原文的安全上限。块宽平时按模型窗口算（约半个窗口，见执行层 `extract_tokens`）。
const MAX_CHUNK_BYTES: usize = 512 * 1024;
/// 新任务的初始块宽，约是 32k 窗口的一半；开始提炼时按实际窗口重算。
const DEFAULT_CHUNK_BYTES: usize = 44_000;
/// 旧版任务存盘时没有块宽字段，那时固定按 3200 字节切块，读回按它算才对得上旧的块记录。
const LEGACY_CHUNK_BYTES: usize = 3_200;
const MAX_RETRIES: usize = 2;
const MAX_CALLS_PER_ACTION: usize = 400;
const MAX_TASK_SECONDS: u64 = 3 * 60 * 60;
/// 统稿历史最多留几份，再早的人工统稿丢弃。
const MAX_FINAL_HISTORY: usize = 20;

/// 能按章节拼接成文的文种。会议议程、会议纪要有固定骨架，电话记录单如实记录来电，
/// 都不适合分节写作再拼接。
pub(crate) fn supports(kind: TemplateKind) -> bool {
    !matches!(
        kind,
        TemplateKind::PhoneRecord | TemplateKind::MeetingAgenda | TemplateKind::MeetingMinutes
    )
}

/// 没有风格样稿时的默认写法；换文种时按新文种重写这几行，人工改过的保留。
pub(crate) fn default_style(kind: TemplateKind) -> String {
    format!(
        "默认写法（未从样稿学习）：\n{}",
        crate::prompt::style_guide(kind)
    )
}

/// 没有结构范文时的默认篇章框架；口径同 [`default_style`]。
pub(crate) fn default_framework(kind: TemplateKind) -> String {
    format!(
        "默认篇章框架（未从范文提炼）：\n{}",
        crate::prompt::kind_rules(kind)
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Stage {
    Prepare,
    Extract,
    Outline,
    Collect,
    Write,
    Review,
}

impl Stage {
    pub(crate) const ALL: [Self; 6] = [
        Self::Prepare,
        Self::Extract,
        Self::Outline,
        Self::Collect,
        Self::Write,
        Self::Review,
    ];
    pub(crate) fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Prepare => "素材准备",
            Self::Extract => "提炼素材",
            Self::Outline => "确认提纲",
            Self::Collect => "各节素材",
            Self::Write => "成文",
            Self::Review => "统稿审阅",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum SourceRole {
    Content,
    Style,
    Framework,
}
impl SourceRole {
    pub(crate) const ALL: [Self; 3] = [Self::Content, Self::Style, Self::Framework];
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Content => "内容材料",
            Self::Style => "风格样稿",
            Self::Framework => "结构范文",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SourceSnapshot {
    pub id: String,
    pub title: String,
    pub role: SourceRole,
    pub text: String,
    pub hash: String,
}
impl SourceSnapshot {
    pub(crate) fn new(
        title: impl Into<String>,
        role: SourceRole,
        text: impl Into<String>,
    ) -> anyhow::Result<Self> {
        let text = text.into();
        let title = title.into();
        anyhow::ensure!(
            !text.trim().is_empty(),
            "材料正文为空，请重新选择或补充内容"
        );
        anyhow::ensure!(
            !title.trim().is_empty() && title.len() <= 1024,
            "材料名称不能为空且不能超过 1024 字节"
        );
        anyhow::ensure!(text.len() <= MAX_SOURCE_BYTES, "单份材料超过 16 MiB 上限");
        let hash = digest(text.as_bytes());
        Ok(Self {
            id: uuid::Uuid::new_v4().to_string(),
            title,
            role,
            text,
            hash,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MaterialCard {
    pub id: String,
    pub source_id: String,
    pub quote: String,
    pub point: String,
    pub section_id: Option<String>,
    pub start: usize,
    pub end: usize,
    #[serde(default)]
    pub verification: Verification,
    #[serde(default)]
    pub status: String,
    /// 产生这张卡片时的请求指纹（主题或本节要求）；与当前指纹不同即为过期，保留不用。
    #[serde(default)]
    pub query: String,
    /// 依据是否已在原文里逐字定位。是：`quote` 就是原文 `start..end` 的片段；否：`quote` 是模型
    /// 概括的依据，`start..end` 只标出它出自哪一段原文。旧卡片都是逐字定位的，缺省为是。
    #[serde(default = "located_default")]
    pub located: bool,
}

fn located_default() -> bool {
    true
}

impl Default for MaterialCard {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            source_id: String::new(),
            quote: String::new(),
            point: String::new(),
            section_id: None,
            start: 0,
            end: 0,
            verification: Verification::Unverified,
            status: String::new(),
            query: String::new(),
            located: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(crate) enum Verification {
    #[default]
    Unverified,
    Supported,
    Conflict,
    Insufficient,
}

/// 章节状态。旧任务 JSON 里存的是中文字符串，读写都按原字符串兼容，
/// 界面文案统一由 [`SectionStatus::label`] 给出。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SectionStatus {
    #[default]
    Idle,
    Collecting,
    Collected,
    CollectFailed,
    Writing,
    Done,
    ManualDone,
    ReviewReady,
}

impl SectionStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Idle => "",
            Self::Collecting => "备料进行中",
            Self::Collected => "备料完成",
            Self::CollectFailed => "备料未完成",
            Self::Writing => "写作中",
            Self::Done => "已完成",
            Self::ManualDone => "人工已完成",
            Self::ReviewReady => "已有统稿建议，待审阅",
        }
    }
}

impl Serialize for SectionStatus {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.label())
    }
}

impl<'de> Deserialize<'de> for SectionStatus {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            "备料进行中" => Self::Collecting,
            "备料完成" => Self::Collected,
            "备料未完成" => Self::CollectFailed,
            "写作中" => Self::Writing,
            "已完成" => Self::Done,
            "人工已完成" => Self::ManualDone,
            "已有统稿建议，待审阅" => Self::ReviewReady,
            // 旧值或空串：当作尚未推进，用户可以继续操作，不假装已完成。
            _ => Self::Idle,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(crate) struct Section {
    pub id: String,
    pub title: String,
    pub points: String,
    pub target_chars: usize,
    pub markdown: String,
    pub locked: bool,
    pub cards: Vec<MaterialCard>,
    #[serde(default)]
    pub input_hash: String,
    #[serde(default)]
    pub status: SectionStatus,
    #[serde(default)]
    pub manually_edited: bool,
    #[serde(default)]
    pub review_suggestion: Option<String>,
    #[serde(default)]
    pub review_parts: Vec<String>,
    #[serde(default)]
    pub review_input_hash: String,
    /// 审阅时选了「保留原稿」：建议仍保留、可随时改回，拼全文用本节正文。
    #[serde(default)]
    pub review_kept_draft: bool,
    /// 上次给本节分配素材时的指纹（[`WritingTask::assignment_query`]）；与当前一致即已备料。
    #[serde(default)]
    pub assigned_query: String,
    /// 锁定或人工章节的统稿建议：单独存放，拼全文仍用本节正文，采纳与否由用户决定。
    #[serde(default)]
    pub advisory_suggestion: Option<String>,
    #[serde(default)]
    pub advisory_parts: Vec<String>,
    #[serde(default)]
    pub advisory_input_hash: String,
    #[serde(default)]
    pub write_parts: Vec<String>,
    #[serde(default)]
    pub merge_state: Option<MergeState>,
    #[serde(default)]
    pub revision_history: Vec<String>,
    #[serde(default)]
    pub rewrite_request: Option<String>,
    #[serde(default)]
    pub rewrite_baseline: String,
    #[serde(default)]
    pub rewrite_in_progress: bool,
    #[serde(default = "default_chunk_bytes")]
    pub chunk_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MergeState {
    input_hash: String,
    parts: Vec<String>,
    next: Vec<String>,
    level: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ChunkProgress {
    pub source_id: String,
    pub chunk_hash: String,
    pub role: SourceRole,
    pub section_id: Option<String>,
    pub start: usize,
    pub end: usize,
    pub status: ChunkStatus,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub output: String,
    /// 处理这一块时的请求指纹；风格与结构样稿只随原文变化，指纹为空。
    #[serde(default)]
    pub query: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ChunkStatus {
    Pending,
    Extracted,
    NoRelevantContent,
    /// 模型处理失败，保留原因等待重试或人工处置。
    Failed,
    /// 用户人工确认这块没有相关内容：计入覆盖，但单独显示为「人工跳过」，可撤销。
    SkippedByUser,
}

impl ChunkStatus {
    /// 这一块是否已经处理完（成功或人工确认），可以计入覆盖。
    pub(crate) fn counts_as_done(self) -> bool {
        matches!(
            self,
            Self::Extracted | Self::NoRelevantContent | Self::SkippedByUser
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WritingTask {
    pub id: String,
    pub kind: TemplateKind,
    pub topic: String,
    pub target_chars: usize,
    pub stage: Stage,
    pub sources: Vec<SourceSnapshot>,
    pub style_md: String,
    pub framework_md: String,
    pub topic_cards: Vec<MaterialCard>,
    #[serde(default)]
    pub topic_directory: String,
    #[serde(default)]
    pub style_manual: bool,
    #[serde(default)]
    pub framework_manual: bool,
    /// 风格 / 框架归纳与主题观点目录的输入指纹：输入没变就复用上次结果，不重复问模型。
    #[serde(default)]
    pub style_summary_input: String,
    #[serde(default)]
    pub framework_summary_input: String,
    #[serde(default)]
    pub topic_directory_input: String,
    pub sections: Vec<Section>,
    #[serde(default)]
    pub section_history: Vec<Section>,
    pub outline_confirmed: bool,
    pub final_markdown: String,
    pub status: String,
    pub error: Option<String>,
    #[serde(default)]
    pub chunks: Vec<ChunkProgress>,
    #[serde(default)]
    pub calls_used: usize,
    #[serde(default)]
    pub usage: crate::agent::backend::UsageTotals,
    #[serde(skip)]
    usage_prefix: usize,
    #[serde(default)]
    pub started_at_unix: Option<u64>,
    #[serde(default)]
    pub stale_reason: Option<String>,
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub resume_action: Option<Action>,
    #[serde(default = "default_chunk_bytes")]
    pub topic_chunk_bytes: usize,
    #[serde(default = "default_chunk_bytes")]
    pub chunk_bytes: usize,
    #[serde(default)]
    pub review_notes: String,
    #[serde(default)]
    pub review_group_notes: Vec<String>,
    #[serde(default)]
    pub review_group_hash: String,
    /// 统稿页人工改过全文；上游变化时保留并提示，重新统稿前存入统稿历史。
    #[serde(default)]
    pub final_manual: bool,
    #[serde(default)]
    pub final_history: Vec<String>,
}
impl Default for WritingTask {
    fn default() -> Self {
        Self::new(TemplateKind::default())
    }
}
impl WritingTask {
    pub(crate) fn new(kind: TemplateKind) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            kind,
            topic: String::new(),
            target_chars: 1000,
            stage: Stage::Prepare,
            sources: vec![],
            style_md: String::new(),
            framework_md: String::new(),
            topic_cards: vec![],
            topic_directory: String::new(),
            style_manual: false,
            framework_manual: false,
            style_summary_input: String::new(),
            framework_summary_input: String::new(),
            topic_directory_input: String::new(),
            sections: vec![],
            section_history: vec![],
            outline_confirmed: false,
            final_markdown: String::new(),
            status: "待开始".into(),
            error: None,
            chunks: vec![],
            calls_used: 0,
            usage: Default::default(),
            usage_prefix: 0,
            started_at_unix: None,
            stale_reason: None,
            version: 1,
            resume_action: None,
            topic_chunk_bytes: DEFAULT_CHUNK_BYTES,
            chunk_bytes: DEFAULT_CHUNK_BYTES,
            review_notes: String::new(),
            review_group_notes: Vec::new(),
            review_group_hash: String::new(),
            final_manual: false,
            final_history: Vec::new(),
        }
    }
    pub(crate) fn source_text(&self, source_id: &str) -> Option<&str> {
        self.sources
            .iter()
            .find(|s| s.id == source_id)
            .map(|s| s.text.as_str())
    }
    pub(crate) fn validate_sources(&self) -> Vec<SourceIssue> {
        let mut out = Vec::new();
        for card in self.topic_cards.iter().chain(
            self.sections
                .iter()
                .flat_map(|section| section.cards.iter()),
        ) {
            if self
                .sources
                .iter()
                .any(|source| source.id == card.source_id && source.role != SourceRole::Content)
            {
                out.push(SourceIssue {
                    source_id: card.source_id.clone(),
                    message: "素材卡不能将风格样稿或结构范文当作事实依据".into(),
                });
            }
            if !self
                .sources
                .iter()
                .any(|source| source.id == card.source_id)
            {
                out.push(SourceIssue {
                    source_id: card.source_id.clone(),
                    message: format!("卡片「{}」引用的来源已移除", card.point),
                });
            }
        }
        for s in &self.sources {
            if digest(s.text.as_bytes()) != s.hash {
                out.push(SourceIssue {
                    source_id: s.id.clone(),
                    message: "原文快照校验值不匹配".into(),
                });
            }
            for c in self
                .topic_cards
                .iter()
                .chain(self.sections.iter().flat_map(|x| x.cards.iter()))
                .filter(|c| c.source_id == s.id)
            {
                // 逐字定位的卡片必须与原文片段完全一致；概括的依据只核对所标出处在原文范围内，
                // 不冒充原句。事实是否有出处由交付前对全文的关键事实核对把关。
                if c.end > s.text.len()
                    || c.start > c.end
                    || !s.text.is_char_boundary(c.start)
                    || !s.text.is_char_boundary(c.end)
                    || (c.located && s.text.get(c.start..c.end) != Some(c.quote.as_str()))
                {
                    out.push(SourceIssue {
                        source_id: s.id.clone(),
                        message: format!("卡片「{}」原句与定位不匹配", c.point),
                    });
                }
            }
        }
        out
    }
    /// 主题提炼的请求指纹。主题变了，旧块与旧卡片留着但不再算作当前成果，改回即恢复。
    pub(crate) fn topic_query(&self) -> String {
        digest(format!("topic|{}", self.topic.trim()).as_bytes())
    }

    /// 本节要求的指纹：只随文种、主题与本节标题、要点、字数变化，与章节顺序无关。
    pub(crate) fn section_query(&self, section: &Section) -> String {
        digest(
            format!(
                "section|{:?}|{}|{}|{}|{}",
                self.kind,
                self.topic.trim(),
                section.title,
                section.points,
                section.target_chars
            )
            .as_bytes(),
        )
    }

    /// 分配素材的指纹：本节要求加当前主题卡片集合。章节改了、材料重新提炼了，都要重新分配。
    pub(crate) fn assignment_query(&self, section: &Section) -> String {
        let mut ids: Vec<&str> = self
            .current_topic_cards()
            .into_iter()
            .map(|card| card.id.as_str())
            .collect();
        ids.sort_unstable();
        digest(format!("assign|{}|{}", self.section_query(section), ids.join(",")).as_bytes())
    }

    /// 本节是否已按当前提纲与当前素材分配过素材（可以一条也没分到）。
    pub(crate) fn section_assigned(&self, section: &Section) -> bool {
        section.assigned_query == self.assignment_query(section)
    }

    /// 按当前主题有效的主题素材卡。
    pub(crate) fn current_topic_cards(&self) -> Vec<&MaterialCard> {
        let query = self.topic_query();
        self.topic_cards
            .iter()
            .filter(|card| card.query == query)
            .collect()
    }

    /// 按本节当前要求有效的分节素材卡。
    pub(crate) fn current_section_cards<'a>(&self, section: &'a Section) -> Vec<&'a MaterialCard> {
        let query = self.section_query(section);
        section
            .cards
            .iter()
            .filter(|card| card.query == query)
            .collect()
    }

    /// 采纳锁定或人工稿的统稿建议：原稿进本节历史，正文换成建议并仍算人工稿。
    pub(crate) fn accept_advisory(&mut self, index: usize) -> bool {
        let Some(advice) = self
            .sections
            .get_mut(index)
            .and_then(|section| section.advisory_suggestion.take())
        else {
            return false;
        };
        let section = &mut self.sections[index];
        if !section.markdown.trim().is_empty() {
            section.revision_history.push(section.markdown.clone());
        }
        section.markdown = advice;
        section.manually_edited = true;
        section.status = SectionStatus::ManualDone;
        section.advisory_parts.clear();
        section.advisory_input_hash.clear();
        self.refresh_final_if_generated();
        self.stale_reason = Some("已采纳统稿建议到人工稿，请重新核对后审阅".into());
        true
    }

    /// 审阅时逐节选用统稿建议或原稿；建议本身保留，可随时改回。
    pub(crate) fn choose_review(&mut self, index: usize, keep_draft: bool) -> bool {
        let Some(section) = self.sections.get_mut(index) else {
            return false;
        };
        if section.review_suggestion.is_none() || section.review_kept_draft == keep_draft {
            return false;
        }
        section.review_kept_draft = keep_draft;
        self.refresh_final_if_generated();
        true
    }

    /// 按逐节选择重新拼全文；人工改过的统稿先存入统稿历史，可恢复。
    pub(crate) fn reassemble_final(&mut self) {
        if self.final_manual {
            self.archive_final();
        }
        self.final_manual = false;
        self.final_markdown = assemble::assemble(self);
    }

    /// 按当前各节正文重新拼全文；人工改过统稿时不动它，避免覆盖用户输入。
    pub(crate) fn refresh_final_if_generated(&mut self) {
        if !self.final_manual {
            self.final_markdown = assemble::assemble(self);
        }
    }

    /// 本次全文实际用到的章节素材的核对统计（只算当前指纹有效的卡片）。
    pub(crate) fn verification_stats(&self) -> VerificationStats {
        let mut stats = VerificationStats::default();
        for section in &self.sections {
            for card in self.current_section_cards(section) {
                stats.add(card.verification);
            }
        }
        stats
    }

    pub(crate) fn section_verification_stats(&self, section_id: &str) -> VerificationStats {
        let mut stats = VerificationStats::default();
        if let Some(section) = self
            .sections
            .iter()
            .find(|section| section.id == section_id)
        {
            for card in self.current_section_cards(section) {
                stats.add(card.verification);
            }
        }
        stats
    }

    /// 冲突与依据不足的卡片连同材料标题，供交付前软提醒列表与跳回原文。
    pub(crate) fn verification_issue_cards(&self) -> Vec<(String, MaterialCard)> {
        let mut issues = Vec::new();
        for section in &self.sections {
            for card in self.current_section_cards(section) {
                if !matches!(
                    card.verification,
                    Verification::Conflict | Verification::Insufficient
                ) {
                    continue;
                }
                let title = self
                    .sources
                    .iter()
                    .find(|source| source.id == card.source_id)
                    .map_or_else(String::new, |source| source.title.clone());
                issues.push((title, card.clone()));
            }
        }
        issues
    }

    /// 旧版任务的块与卡片没有指纹；旧版每次改输入都会清空成果，留下的必然对应当前输入。
    pub(crate) fn adopt_legacy_records(&mut self) {
        let topic = self.topic_query();
        let sections: Vec<_> = self
            .sections
            .iter()
            .map(|section| (section.id.clone(), self.section_query(section)))
            .collect();
        let query_of = |section_id: Option<&str>| match section_id {
            None => Some(topic.clone()),
            Some(id) => sections
                .iter()
                .find(|(section, _)| section == id)
                .map(|(_, query)| query.clone()),
        };
        for chunk in self
            .chunks
            .iter_mut()
            .filter(|chunk| chunk.role == SourceRole::Content && chunk.query.is_empty())
        {
            if let Some(query) = query_of(chunk.section_id.as_deref()) {
                chunk.query = query;
            }
        }
        for card in self.topic_cards.iter_mut().filter(|c| c.query.is_empty()) {
            card.query.clone_from(&topic);
        }
        for (section, (_, query)) in self.sections.iter_mut().zip(&sections) {
            for card in section.cards.iter_mut().filter(|c| c.query.is_empty()) {
                card.query.clone_from(query);
            }
        }
        // 逐节重读原文那一版备好的章节：素材已在本节，视为已分配，不必再问一次模型。
        let adopted: Vec<_> = self
            .sections
            .iter()
            .map(|section| {
                (section.assigned_query.is_empty()
                    && !self.current_section_cards(section).is_empty())
                .then(|| self.assignment_query(section))
            })
            .collect();
        for (section, query) in self.sections.iter_mut().zip(adopted) {
            if let Some(query) = query {
                section.assigned_query = query;
            }
        }
    }

    /// 块状态索引，键与 [`chunk_hash`] 记录的块身份一致。覆盖率与就绪判断都走它，
    /// 不再对每一块线性扫描 `chunks`。
    fn chunk_index(&self) -> HashMap<(&str, Option<&str>, &str, &str), ChunkStatus> {
        self.chunks
            .iter()
            .map(|chunk| {
                (
                    (
                        chunk.source_id.as_str(),
                        chunk.section_id.as_deref(),
                        chunk.query.as_str(),
                        chunk.chunk_hash.as_str(),
                    ),
                    chunk.status,
                )
            })
            .collect()
    }

    /// 这一块在给定请求下的处理状态；没有记录即为待处理。
    fn chunk_status(
        index: &HashMap<(&str, Option<&str>, &str, &str), ChunkStatus>,
        source: &SourceSnapshot,
        section_id: Option<&str>,
        query: &str,
        start: usize,
        end: usize,
    ) -> Option<ChunkStatus> {
        let hash = chunk_hash(source, start, end);
        index
            .get(&(source.id.as_str(), section_id, query, hash.as_str()))
            .copied()
    }

    /// 主题提炼的覆盖情况（只算内容材料）。
    pub(crate) fn coverage(&self) -> Coverage {
        let sources: Vec<_> = self
            .sources
            .iter()
            .filter(|s| s.role == SourceRole::Content)
            .collect();
        self.coverage_of(&sources, None, self.topic_chunk_bytes, &self.topic_query())
    }

    /// 单份材料的处理进度（素材准备页逐份显示用）；样式与结构样稿按空指纹统计。
    pub(crate) fn source_coverage(&self, source_id: &str) -> Coverage {
        let Some(source) = self.sources.iter().find(|s| s.id == source_id) else {
            return Coverage::default();
        };
        if source.role == SourceRole::Content {
            self.coverage_of(&[source], None, self.topic_chunk_bytes, &self.topic_query())
        } else {
            self.coverage_of(&[source], None, self.topic_chunk_bytes, "")
        }
    }

    fn coverage_of(
        &self,
        sources: &[&SourceSnapshot],
        section_id: Option<&str>,
        block_size: usize,
        query: &str,
    ) -> Coverage {
        let index = self.chunk_index();
        let mut coverage = Coverage::default();
        for source in sources {
            for (start, end) in chunk_ranges(&source.text, block_size) {
                coverage.total += 1;
                match Self::chunk_status(&index, source, section_id, query, start, end) {
                    Some(status) if status.counts_as_done() => {
                        coverage.done += 1;
                        if status == ChunkStatus::SkippedByUser {
                            coverage.skipped += 1;
                        }
                    }
                    Some(ChunkStatus::Failed) => coverage.failed += 1,
                    _ => {}
                }
            }
        }
        coverage.pending = coverage
            .total
            .saturating_sub(coverage.done + coverage.failed);
        coverage
    }

    /// 人工确认一块没有相关内容：计入覆盖，但保留为可撤销的独立状态，绝不记成模型判定。
    pub(crate) fn mark_chunk_skipped(
        &mut self,
        source: &SourceSnapshot,
        section_id: Option<&str>,
        query: &str,
        start: usize,
        end: usize,
    ) -> bool {
        let progress = ChunkProgress {
            source_id: source.id.clone(),
            chunk_hash: chunk_hash(source, start, end),
            role: source.role,
            section_id: section_id.map(str::to_string),
            start,
            end,
            status: ChunkStatus::SkippedByUser,
            error: None,
            output: String::new(),
            query: query.to_string(),
        };
        if let Some(index) = self.chunks.iter().position(|chunk| {
            chunk.source_id == progress.source_id
                && chunk.section_id == progress.section_id
                && chunk.query == progress.query
                && chunk.chunk_hash == progress.chunk_hash
        }) {
            if self.chunks[index].status == ChunkStatus::SkippedByUser {
                return false;
            }
            self.chunks[index] = progress;
        } else {
            self.chunks.push(progress);
        }
        self.resume_action = None;
        true
    }

    /// 撤销人工跳过，让这一块回到待处理。
    pub(crate) fn unskip_chunk(
        &mut self,
        source: &SourceSnapshot,
        section_id: Option<&str>,
        query: &str,
        start: usize,
        end: usize,
    ) -> bool {
        let hash = chunk_hash(source, start, end);
        let before = self.chunks.len();
        self.chunks.retain(|chunk| {
            !(chunk.source_id == source.id
                && chunk.section_id.as_deref() == section_id
                && chunk.query == query
                && chunk.chunk_hash == hash
                && chunk.status == ChunkStatus::SkippedByUser)
        });
        if self.chunks.len() != before {
            self.resume_action = None;
            true
        } else {
            false
        }
    }

    /// 文种变了：风格 / 框架若是旧文种的默认文字就换新，人工改过的保留；
    /// 提纲需要重新确认，已有成果不删除。
    pub(crate) fn sync_kind(&mut self, kind: TemplateKind) -> bool {
        if self.kind == kind {
            return false;
        }
        let old = self.kind;
        self.kind = kind;
        let has_style = self
            .sources
            .iter()
            .any(|source| source.role == SourceRole::Style);
        let has_framework = self
            .sources
            .iter()
            .any(|source| source.role == SourceRole::Framework);
        if !self.style_manual && !has_style {
            self.style_md = default_style(kind);
            self.style_summary_input.clear();
        }
        if !self.framework_manual && !has_framework {
            self.framework_md = default_framework(kind);
            self.framework_summary_input.clear();
        }
        self.clear_topic_directory();
        self.outline_changed();
        self.stale_reason = Some(format!(
            "文种已从「{}」改为「{}」：默认写法与框架已按新文种更新，提纲需重新确认；已读的材料照常复用",
            old.label(),
            kind.label()
        ));
        true
    }

    fn clear_topic_directory(&mut self) {
        self.topic_directory.clear();
        self.topic_directory_input.clear();
    }

    /// 有没有已经花过模型调用的成果；没有时改输入无须确认。
    pub(crate) fn has_results(&self) -> bool {
        !self.chunks.is_empty() || !self.sections.is_empty() || !self.final_markdown.is_empty()
    }

    pub(crate) fn add_source(&mut self, source: SourceSnapshot) {
        let content = source.role == SourceRole::Content;
        self.sources.push(source);
        self.sources_changed(content);
    }
    /// 移除材料只删它自己的块与卡片，其他材料已完成的提炼与备料照常复用。
    pub(crate) fn remove_source(&mut self, id: &str) -> bool {
        let Some(index) = self.sources.iter().position(|s| s.id == id) else {
            return false;
        };
        let content = self.sources.remove(index).role == SourceRole::Content;
        self.chunks.retain(|chunk| chunk.source_id != id);
        self.topic_cards.retain(|card| card.source_id != id);
        for section in &mut self.sections {
            section.cards.retain(|card| card.source_id != id);
        }
        self.sources_changed(content);
        true
    }
    /// 移除这份材料会丢掉的素材卡数与已完成块数，供确认框说明。
    pub(crate) fn source_impact(&self, id: &str) -> (usize, usize) {
        let cards = self
            .topic_cards
            .iter()
            .chain(self.sections.iter().flat_map(|s| s.cards.iter()))
            .filter(|card| card.source_id == id)
            .count();
        let chunks = self
            .chunks
            .iter()
            .filter(|chunk| chunk.source_id == id && chunk.status != ChunkStatus::Pending)
            .count();
        (cards, chunks)
    }
    fn sources_changed(&mut self, content: bool) {
        self.resume_action = None;
        if content {
            self.clear_topic_directory();
            self.outline_changed();
        } else {
            self.drafts_changed();
        }
        self.stale_reason = Some("材料已变化：新增部分需要提炼与备料，已完成的块会直接复用".into());
    }

    /// 改主题：已完成的提炼按旧主题指纹保留，在重新提炼完成前改回原主题即可恢复。
    pub(crate) fn set_topic(&mut self, topic: String) {
        if topic == self.topic {
            return;
        }
        self.topic = topic;
        self.clear_topic_directory();
        self.outline_changed();
        self.stage = Stage::Prepare;
        self.stale_reason =
            Some("主题已修改：主题素材与各节备料需按新主题重新提炼；原主题的成果暂时保留".into());
    }

    /// 目标篇幅只影响拟纲与写作，不作废已读的材料。
    pub(crate) fn target_changed(&mut self) {
        self.stale_reason = Some("目标篇幅已变化：可在提纲页调整各节字数，或重新生成提纲".into());
    }

    /// 风格或框架改了：不必重读材料；提纲待复核，未经人工修改的章节下次写作时按新要求重写。
    pub(crate) fn style_changed(&mut self, style: bool) {
        if style {
            self.style_manual = true;
        } else {
            self.framework_manual = true;
        }
        self.drafts_changed();
        self.stale_reason = Some(format!(
            "{}已修改：提纲请复核；未经人工修改的章节会在下次成文时按新要求重写",
            if style {
                "写作风格"
            } else {
                "篇章框架"
            }
        ));
    }

    /// 提纲变化：需要重新确认。各节备料按本节指纹判断是否仍然有效，不删除。
    pub(crate) fn outline_changed(&mut self) {
        self.resume_action = None;
        self.outline_confirmed = false;
        if self.stage.index() > Stage::Outline.index() {
            self.stage = Stage::Outline;
        }
        self.drafts_changed();
    }

    /// 章节正文变了：AI 统稿建议与检查清单作废；人工改过的统稿保留并提示，不悄悄清空。
    pub(crate) fn drafts_changed(&mut self) {
        self.review_notes.clear();
        self.review_group_notes.clear();
        self.review_group_hash.clear();
        for section in &mut self.sections {
            section.review_suggestion = None;
            section.review_parts.clear();
            section.review_input_hash.clear();
            section.review_kept_draft = false;
            // 锁定 / 人工稿的建议基于改动前的正文，采纳会盖掉新的修改，一并作废。
            section.advisory_suggestion = None;
            section.advisory_parts.clear();
            section.advisory_input_hash.clear();
        }
        if self.final_manual && !self.final_markdown.trim().is_empty() {
            self.stale_reason = Some(
                "统稿之后章节有变化；你修改过的统稿已保留，可重新统稿（当前稿会存入统稿历史）"
                    .into(),
            );
        } else {
            self.final_markdown.clear();
            self.final_manual = false;
        }
    }

    /// 删除章节：正文与素材移入历史提纲，可恢复为新的一节。
    pub(crate) fn remove_section(&mut self, id: &str) {
        if let Some(index) = self.sections.iter().position(|s| s.id == id) {
            let removed = self.sections.remove(index);
            self.section_history.push(removed);
            self.outline_changed();
        }
    }

    /// 把当前人工统稿存入历史，供重新统稿后恢复。
    pub(crate) fn archive_final(&mut self) {
        let text = self.final_markdown.trim();
        if text.is_empty() || self.final_history.last().is_some_and(|last| last == text) {
            return;
        }
        self.final_history.push(text.to_string());
        if self.final_history.len() > MAX_FINAL_HISTORY {
            self.final_history.remove(0);
        }
    }

    /// 把统稿历史中的一份放回统稿页；当前人工稿先存档。
    pub(crate) fn restore_final(&mut self, index: usize) {
        let Some(text) = self.final_history.get(index).cloned() else {
            return;
        };
        if self.final_manual {
            self.archive_final();
        }
        self.final_markdown = text;
        self.final_manual = true;
        self.resume_action = None;
    }
    pub(crate) fn confirm_outline(&mut self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.sections.is_empty(), "提纲至少需要一个正文单元");
        anyhow::ensure!(
            self.sections.iter().all(|s| !s.title.trim().is_empty()),
            "章节标题不能为空"
        );
        self.outline_confirmed = true;
        self.stale_reason = None;
        self.stage = Stage::Collect;
        Ok(())
    }
    pub(crate) fn ready_for(&self, stage: Stage) -> anyhow::Result<()> {
        match stage {
            Stage::Prepare => (),
            Stage::Extract => {
                anyhow::ensure!(!self.topic.trim().is_empty(), "请先填写主题");
                anyhow::ensure!(
                    self.sources
                        .iter()
                        .any(|source| source.role == SourceRole::Content
                            && !source.text.trim().is_empty()),
                    "请先添加至少一份内容材料"
                );
            }
            Stage::Outline => {
                let content: Vec<_> = self
                    .sources
                    .iter()
                    .filter(|s| s.role == SourceRole::Content)
                    .collect();
                anyhow::ensure!(!content.is_empty(), "请先准备内容材料");
                let index = self.chunk_index();
                let query = self.topic_query();
                for source in content {
                    for (start, end) in chunk_ranges(&source.text, self.topic_chunk_bytes) {
                        anyhow::ensure!(
                            Self::chunk_status(&index, source, None, &query, start, end)
                                .is_some_and(ChunkStatus::counts_as_done),
                            "内容材料「{}」尚未完整提炼",
                            source.title
                        );
                    }
                }
                for source in self
                    .sources
                    .iter()
                    .filter(|s| s.role != SourceRole::Content)
                {
                    for (start, end) in chunk_ranges(&source.text, self.topic_chunk_bytes) {
                        anyhow::ensure!(
                            Self::chunk_status(&index, source, None, "", start, end)
                                == Some(ChunkStatus::Extracted),
                            "样稿「{}」尚未提炼完成",
                            source.title
                        );
                    }
                }
            }
            Stage::Collect => anyhow::ensure!(self.outline_confirmed, "请先确认提纲"),
            Stage::Write => {
                anyhow::ensure!(self.outline_confirmed, "请先确认提纲");
                anyhow::ensure!(
                    self.sources
                        .iter()
                        .any(|source| source.role == SourceRole::Content),
                    "没有内容材料"
                );
                // 各节素材从已提炼的主题素材里分配，不再逐节重读原文。
                for section in &self.sections {
                    anyhow::ensure!(
                        self.section_assigned(section),
                        "章节「{}」尚未分配素材",
                        section.title
                    );
                }
            }
            Stage::Review => {
                self.ready_for(Stage::Write)?;
                anyhow::ensure!(
                    !self.sections.is_empty()
                        && self
                            .sections
                            .iter()
                            .all(|s| !s.markdown.trim().is_empty()
                                && s.status != SectionStatus::Writing),
                    "请先完成各节正文"
                );
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceIssue {
    pub source_id: String,
    pub message: String,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct VerificationStats {
    pub checked: usize,
    pub conflict: usize,
    pub insufficient: usize,
    pub unverified: usize,
}

impl VerificationStats {
    fn add(&mut self, verification: Verification) {
        match verification {
            Verification::Supported => self.checked += 1,
            Verification::Conflict => self.conflict += 1,
            Verification::Insufficient => self.insufficient += 1,
            Verification::Unverified => self.unverified += 1,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Coverage {
    pub total: usize,
    pub done: usize,
    pub failed: usize,
    pub pending: usize,
    /// 其中由用户人工确认「无相关内容」的块数，单独显示，不与模型判定混同。
    pub skipped: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Action {
    Extract,
    Plan,
    ReviseOutline(String),
    Collect,
    Write,
    RewriteSection(usize, String),
    /// 只重试一个失败的原文块；`section_id` 为空表示主题提炼的块。
    RetryChunk(ChunkRef),
    Polish,
}

/// 原文块的身份：只重试这一块时使用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ChunkRef {
    pub source_id: String,
    pub section_id: Option<String>,
    pub role: SourceRole,
    pub start: usize,
    pub end: usize,
}

/// 原文块的身份：原文快照校验值加块的起止位置。
pub(crate) fn chunk_hash(source: &SourceSnapshot, start: usize, end: usize) -> String {
    digest(format!("{}:{}:{}", source.hash, start, end).as_bytes())
}

fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn default_chunk_bytes() -> usize {
    LEGACY_CHUNK_BYTES
}
