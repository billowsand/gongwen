//! 工具测试用的假环境：假知识库、假稿件库、按配置回话的假模型。

use super::store::ManuscriptDoc;
use super::*;
use crate::agent::backend::Completion;
use crate::agent::backend::ModelRole;
use crate::lmstudio::StreamDelta;
use crate::models::{ManuscriptStatus, VocabularyCategory};
use std::cell::RefCell;

pub(crate) struct FakeKb {
    pub(crate) chunks: Vec<RetrievedChunk>,
    pub(crate) docs: Vec<(i64, String, TemplateKind, String)>,
}

impl KnowledgeSearch for FakeKb {
    fn enabled(&self) -> bool {
        true
    }

    fn search(&self, query: &str) -> anyhow::Result<(Vec<RetrievedChunk>, Vec<String>)> {
        Ok((
            self.chunks
                .iter()
                .filter(|chunk| query.chars().any(|c| chunk.text.contains(c)))
                .cloned()
                .collect(),
            Vec::new(),
        ))
    }

    fn read(&self, doc_id: i64) -> anyhow::Result<Option<(String, String)>> {
        Ok(self
            .docs
            .iter()
            .find(|doc| doc.0 == doc_id)
            .map(|doc| (doc.1.clone(), doc.3.clone())))
    }

    fn list(&self, kind: Option<TemplateKind>) -> anyhow::Result<Vec<(i64, String, TemplateKind)>> {
        Ok(self
            .docs
            .iter()
            .filter(|doc| kind.is_none_or(|k| k == doc.2))
            .map(|doc| (doc.0, doc.1.clone(), doc.2))
            .collect())
    }
}

pub(crate) struct FakeManuscripts {
    pub(crate) docs: Vec<ManuscriptDoc>,
}

impl ManuscriptSource for FakeManuscripts {
    fn search(
        &self,
        filter: &crate::manuscript::ManuscriptFilter,
        limit: usize,
    ) -> anyhow::Result<Vec<ManuscriptDoc>> {
        Ok(self
            .docs
            .iter()
            .filter(|doc| filter.keyword.is_empty() || doc.title.contains(&filter.keyword))
            .filter(|doc| filter.kind.is_none_or(|k| k == doc.kind))
            .filter(|doc| filter.status.is_none_or(|s| s == doc.status))
            .take(limit)
            .cloned()
            .collect())
    }

    fn read(&self, id: i64, version: Option<i64>) -> anyhow::Result<Option<ManuscriptDoc>> {
        Ok(self
            .docs
            .iter()
            .find(|doc| doc.id == id)
            .map(|doc| ManuscriptDoc {
                version,
                ..doc.clone()
            }))
    }

    fn versions(&self, id: i64) -> anyhow::Result<Vec<crate::manuscript::VersionRow>> {
        Ok(if id == 1 {
            vec![crate::manuscript::VersionRow {
                version_number: 1,
                name: "初稿".into(),
                comment: String::new(),
                title: "关于做好秋季森林防火工作的通知".into(),
                doc_number: String::new(),
                doc_date: "2025-09-01".into(),
                created_at: "2025-09-01 10:00".into(),
                is_latest: true,
            }]
        } else {
            Vec::new()
        })
    }
}

/// 按配置回话的假模型，同时记下收到的提示词。
pub(crate) struct EchoModel {
    pub(crate) reply: String,
    pub(crate) prompts: RefCell<Vec<(ModelRole, String)>>,
}

impl ModelBackend for EchoModel {
    fn complete(
        &self,
        role: ModelRole,
        _system: &str,
        user: &str,
        on_delta: &mut dyn FnMut(StreamDelta<'_>),
    ) -> anyhow::Result<Completion> {
        self.prompts.borrow_mut().push((role, user.to_string()));
        on_delta(StreamDelta::Content(&self.reply));
        Ok(Completion {
            content: self.reply.clone(),
            truncated: false,
        })
    }

    fn cancelled(&self) -> bool {
        false
    }
}

fn unit(
    code: &str,
    parent: &str,
    canonical: &str,
    external: &str,
    aliases: &[&str],
) -> VocabularyEntry {
    VocabularyEntry {
        category: VocabularyCategory::Unit,
        code: code.into(),
        parent: parent.into(),
        canonical: canonical.into(),
        external_name: external.into(),
        aliases: aliases.iter().map(|a| a.to_string()).collect(),
        ..VocabularyEntry::default()
    }
}

pub(crate) struct Fixture {
    pub(crate) board: Board,
    pub(crate) config: AppConfig,
    pub(crate) vocabulary: Vec<VocabularyEntry>,
    pub(crate) skill: Skill,
    pub(crate) kb: FakeKb,
    pub(crate) manuscripts: FakeManuscripts,
    pub(crate) model: EchoModel,
    pub(crate) events: Vec<Event>,
}

impl Fixture {
    pub(crate) fn new(document: &str) -> Self {
        let mut skill = crate::agent::skill::parse(
            "test",
            "---\nname: 测试\n---\n## 提炼\n请提炼：{input}\n",
            "测试",
        )
        .expect("测试技能");
        skill.tools = ids().into_iter().map(str::to_string).collect();
        let mut vocabulary = vec![
            unit("01", "", "市应急管理局", "", &["市应急局"]),
            unit("0101", "01", "市应急管理局办公室", "", &["应急局办公室"]),
            unit("02", "", "市林业和草原局", "市林草局", &["林草局"]),
        ];
        vocabulary.push(VocabularyEntry {
            category: VocabularyCategory::Person,
            canonical: "张三".into(),
            position: "副局长".into(),
            unit: "01".into(),
            phone: "13800000000".into(),
            ..VocabularyEntry::default()
        });
        let board = Board {
            document: document.to_string(),
            workspace: document.to_string(),
            ..Board::default()
        };
        Self {
            board,
            config: AppConfig::default(),
            vocabulary,
            skill,
            kb: FakeKb {
                chunks: vec![crate::agent::evidence::tests_support::chunk(
                    7,
                    "森林防火条例",
                    "第十条 各地应当建立防火责任制。",
                )],
                docs: vec![(
                    3,
                    "森林防火条例".into(),
                    TemplateKind::PlainDocument,
                    "第一条 ……\n第十条 ……".into(),
                )],
            },
            manuscripts: FakeManuscripts {
                docs: vec![ManuscriptDoc {
                    id: 1,
                    title: "关于做好秋季森林防火工作的通知".into(),
                    kind: TemplateKind::PlainDocument,
                    status: ManuscriptStatus::Published,
                    doc_number: String::new(),
                    doc_date: "2025-09-01".into(),
                    draft: crate::models::DraftInput::default(),
                    markdown: "# 关于做好秋季森林防火工作的通知\n\n各地要加强巡查。\n".into(),
                    version: None,
                }],
            },
            model: EchoModel {
                reply: "模型的回答".into(),
                prompts: RefCell::new(Vec::new()),
            },
            events: Vec::new(),
        }
    }

    pub(crate) fn call(&mut self, id: &str, args: Value) -> Result<ToolOutput, String> {
        let env = Env {
            config: &self.config,
            vocabulary: &self.vocabulary,
            kb: &self.kb,
            manuscripts: &self.manuscripts,
            model: &self.model,
            skill: &self.skill,
        };
        let mut events = Vec::new();
        let result = {
            let mut emit = |event| events.push(event);
            let mut ctx = ToolCtx {
                board: &mut self.board,
                env: &env,
                emit: &mut emit,
            };
            call(id, &mut ctx, &args)
        };
        self.events.extend(events);
        result
    }
}
