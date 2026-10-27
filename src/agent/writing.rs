//! 固定阶段的公文写作任务与执行入口。

use crate::models::TemplateKind;
use serde::{Deserialize, Serialize};

mod assemble;
mod execution;
pub(crate) mod store;
use execution::chunks;
pub(crate) use execution::run;

#[cfg(test)]
mod tests;

const MAX_SOURCE_BYTES: usize = 16 * 1024 * 1024;
const MAX_TOTAL_SOURCE_BYTES: usize = 64 * 1024 * 1024;
const MAX_CHUNK_BYTES: usize = 3_200;
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
            Self::Collect => "分节备料",
            Self::Write => "分节写作",
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
    pub status: String,
    #[serde(default)]
    pub manually_edited: bool,
    #[serde(default)]
    pub review_suggestion: Option<String>,
    #[serde(default)]
    pub review_parts: Vec<String>,
    #[serde(default)]
    pub review_input_hash: String,
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
    Failed,
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
            topic_chunk_bytes: MAX_CHUNK_BYTES,
            chunk_bytes: MAX_CHUNK_BYTES,
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
                if c.end > s.text.len()
                    || c.start > c.end
                    || !s.text.is_char_boundary(c.start)
                    || !s.text.is_char_boundary(c.end)
                    || s.text.get(c.start..c.end) != Some(c.quote.as_str())
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

    /// 分节备料的请求指纹：只随文种、主题与本节标题、要点、字数变化，与章节顺序无关。
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
    }

    /// 这一块在给定请求下的处理状态；没有记录即为待处理。
    fn chunk_state(
        &self,
        source: &SourceSnapshot,
        section_id: Option<&str>,
        query: &str,
        start: usize,
        end: usize,
    ) -> Option<ChunkStatus> {
        let hash = chunk_hash(source, start, end);
        self.chunks
            .iter()
            .find(|c| {
                c.source_id == source.id
                    && c.section_id.as_deref() == section_id
                    && c.query == query
                    && c.chunk_hash == hash
            })
            .map(|c| c.status)
    }

    pub(crate) fn coverage(&self) -> Coverage {
        self.coverage_for(None)
    }
    pub(crate) fn section_coverage(&self, section_id: &str) -> Coverage {
        self.coverage_for(Some(section_id))
    }
    fn coverage_for(&self, section_id: Option<&str>) -> Coverage {
        let section = section_id.and_then(|id| self.sections.iter().find(|s| s.id == id));
        let (block_size, query) = match section {
            Some(section) => (section.chunk_bytes.max(512), self.section_query(section)),
            None => (self.topic_chunk_bytes, self.topic_query()),
        };
        let mut coverage = Coverage::default();
        for source in self
            .sources
            .iter()
            .filter(|s| s.role == SourceRole::Content)
        {
            for (start, end, _) in chunks(&source.text, block_size) {
                coverage.total += 1;
                match self.chunk_state(source, section_id, &query, start, end) {
                    Some(ChunkStatus::Extracted | ChunkStatus::NoRelevantContent) => {
                        coverage.done += 1
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
            self.topic_directory.clear();
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
        self.topic_directory.clear();
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
            "{}已修改：提纲请复核；未经人工修改的章节会在下次分节写作时按新要求重写",
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
                let query = self.topic_query();
                for source in content {
                    for (start, end, _) in chunks(&source.text, self.topic_chunk_bytes) {
                        anyhow::ensure!(
                            matches!(
                                self.chunk_state(source, None, &query, start, end),
                                Some(ChunkStatus::Extracted | ChunkStatus::NoRelevantContent)
                            ),
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
                    for (start, end, _) in chunks(&source.text, self.topic_chunk_bytes) {
                        anyhow::ensure!(
                            self.chunk_state(source, None, "", start, end)
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
                let content: Vec<_> = self
                    .sources
                    .iter()
                    .filter(|s| s.role == SourceRole::Content)
                    .collect();
                anyhow::ensure!(!content.is_empty(), "没有内容材料");
                for section in &self.sections {
                    let query = self.section_query(section);
                    for source in &content {
                        for (start, end, _) in chunks(&source.text, section.chunk_bytes.max(512)) {
                            anyhow::ensure!(
                                matches!(
                                    self.chunk_state(
                                        source,
                                        Some(section.id.as_str()),
                                        &query,
                                        start,
                                        end
                                    ),
                                    Some(ChunkStatus::Extracted | ChunkStatus::NoRelevantContent)
                                ),
                                "章节「{}」尚有原文块未成功处理",
                                section.title
                            );
                        }
                    }
                }
            }
            Stage::Review => {
                self.ready_for(Stage::Write)?;
                anyhow::ensure!(
                    !self.sections.is_empty()
                        && self
                            .sections
                            .iter()
                            .all(|s| !s.markdown.trim().is_empty() && s.status != "写作中"),
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
pub(crate) struct Coverage {
    pub total: usize,
    pub done: usize,
    pub failed: usize,
    pub pending: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Action {
    Extract,
    Plan,
    ReviseOutline(String),
    Collect,
    Write,
    RewriteSection(usize, String),
    Polish,
}

/// 原文块的身份：原文快照校验值加块的起止位置。
fn chunk_hash(source: &SourceSnapshot, start: usize, end: usize) -> String {
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
    MAX_CHUNK_BYTES
}
