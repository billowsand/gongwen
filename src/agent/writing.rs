//! 固定阶段的公文写作任务与执行入口。

use crate::models::TemplateKind;
use serde::{Deserialize, Serialize};

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
    pub(crate) fn coverage(&self) -> Coverage {
        self.coverage_for(None, false)
    }
    pub(crate) fn section_coverage(&self, section_id: &str) -> Coverage {
        self.coverage_for(Some(section_id), false)
    }
    fn coverage_for(&self, section_id: Option<&str>, include_sections: bool) -> Coverage {
        let mut total = 0;
        let mut done = 0;
        let mut failed = 0;
        for source in self
            .sources
            .iter()
            .filter(|s| s.role == SourceRole::Content)
        {
            let block_size = section_id
                .and_then(|id| self.sections.iter().find(|section| section.id == id))
                .map_or(self.topic_chunk_bytes, |section| {
                    section.chunk_bytes.max(512)
                });
            for (start, end, _) in chunks(&source.text, block_size) {
                if let Some(section_id) = section_id {
                    total += 1;
                    let hash = digest(format!("{}:{}:{}", source.hash, start, end).as_bytes());
                    if let Some(c) = self.chunks.iter().find(|c| {
                        c.source_id == source.id
                            && c.section_id.as_deref() == Some(section_id)
                            && c.chunk_hash == hash
                    }) {
                        if matches!(
                            c.status,
                            ChunkStatus::Extracted | ChunkStatus::NoRelevantContent
                        ) {
                            done += 1
                        } else if c.status == ChunkStatus::Failed {
                            failed += 1
                        }
                    }
                } else {
                    total += 1;
                    let hash = digest(format!("{}:{}:{}", source.hash, start, end).as_bytes());
                    if let Some(c) = self.chunks.iter().find(|c| {
                        c.source_id == source.id && c.section_id.is_none() && c.chunk_hash == hash
                    }) {
                        if matches!(
                            c.status,
                            ChunkStatus::Extracted | ChunkStatus::NoRelevantContent
                        ) {
                            done += 1
                        } else if c.status == ChunkStatus::Failed {
                            failed += 1
                        }
                    }
                    for section in self.sections.iter().filter(|_| include_sections) {
                        total += 1;
                        if let Some(c) = self.chunks.iter().find(|c| {
                            c.source_id == source.id
                                && c.section_id.as_deref() == Some(section.id.as_str())
                                && c.chunk_hash == hash
                        }) {
                            if matches!(
                                c.status,
                                ChunkStatus::Extracted | ChunkStatus::NoRelevantContent
                            ) {
                                done += 1
                            } else if c.status == ChunkStatus::Failed {
                                failed += 1
                            }
                        }
                    }
                }
            }
        }
        Coverage {
            total,
            done,
            failed,
            pending: total.saturating_sub(done + failed),
        }
    }
    pub(crate) fn add_source(&mut self, source: SourceSnapshot) {
        self.sources.push(source);
        self.invalidate_from(Stage::Extract);
    }
    pub(crate) fn remove_source(&mut self, id: &str) -> bool {
        let old = self.sources.len();
        self.sources.retain(|s| s.id != id);
        if old != self.sources.len() {
            self.invalidate_from(Stage::Extract);
            true
        } else {
            false
        }
    }
    pub(crate) fn invalidate_from(&mut self, stage: Stage) {
        self.resume_action = None;
        self.review_notes.clear();
        self.review_group_notes.clear();
        self.review_group_hash.clear();
        self.stage = Stage::ALL[self.stage.index().min(stage.index())];
        self.stale_reason = Some(format!("{}及后续成果依赖输入已变化", stage.label()));
        if stage.index() <= Stage::Collect.index() {
            self.chunks.retain(|chunk| chunk.section_id.is_none());
            for section in &mut self.sections {
                section.input_hash.clear();
                section.write_parts.clear();
                section.rewrite_in_progress = false;
                section.review_suggestion = None;
                section.review_parts.clear();
                section.review_input_hash.clear();
            }
        }
        match stage {
            Stage::Prepare => {
                self.topic_cards.clear();
                self.topic_directory.clear();
                self.outline_confirmed = false;
                self.final_markdown.clear();
                self.chunks.clear()
            }
            Stage::Extract => {
                self.topic_cards.clear();
                self.topic_directory.clear();
                self.outline_confirmed = false;
                self.chunks.clear();
                self.sections.iter_mut().for_each(|s| {
                    s.cards.clear();
                    s.status = "待备料".into();
                });
                self.final_markdown.clear()
            }
            Stage::Outline => {
                self.outline_confirmed = false;
                self.sections.iter_mut().for_each(|s| {
                    s.cards.clear();
                    s.status = "待确认".into();
                });
                self.final_markdown.clear()
            }
            Stage::Collect => {
                self.sections.iter_mut().for_each(|s| {
                    s.cards.clear();
                    s.status = "待备料".into();
                });
                self.final_markdown.clear()
            }
            Stage::Write | Stage::Review => {
                self.final_markdown.clear();
                for section in &mut self.sections {
                    section.review_suggestion = None;
                    section.review_parts.clear();
                    section.review_input_hash.clear();
                }
            }
        }
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
                for source in content {
                    for (start, end, _) in chunks(&source.text, self.topic_chunk_bytes) {
                        let hash = digest(format!("{}:{}:{}", source.hash, start, end).as_bytes());
                        anyhow::ensure!(
                            self.chunks.iter().any(|c| c.source_id == source.id
                                && c.section_id.is_none()
                                && c.chunk_hash == hash
                                && matches!(
                                    c.status,
                                    ChunkStatus::Extracted | ChunkStatus::NoRelevantContent
                                )),
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
                        let hash = digest(format!("{}:{}:{}", source.hash, start, end).as_bytes());
                        anyhow::ensure!(
                            self.chunks.iter().any(|c| c.source_id == source.id
                                && c.chunk_hash == hash
                                && c.status == ChunkStatus::Extracted),
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
                    for source in &content {
                        for (start, end, _) in chunks(&source.text, section.chunk_bytes.max(512)) {
                            let hash =
                                digest(format!("{}:{}:{}", source.hash, start, end).as_bytes());
                            anyhow::ensure!(
                                self.chunks.iter().any(|c| c.source_id == source.id
                                    && c.section_id.as_deref() == Some(section.id.as_str())
                                    && c.chunk_hash == hash
                                    && matches!(
                                        c.status,
                                        ChunkStatus::Extracted | ChunkStatus::NoRelevantContent
                                    )),
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
