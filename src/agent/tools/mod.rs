//! 工具层：原子操作（`docs/ai-agent-workbench.md` 16.2、16.3）。
//!
//! 一个工具只做一件事，输入输出都是 JSON，流程可以直接调（`tool:` 步骤），算子也用它们拼招式。
//! 每个工具有权限级别；技能只能用自己在 `tools:` 里声明过的工具。**改文档的工具只能改工作稿**，
//! 正文只有用户接受提案这一个入口（红线 1）。
//!
//! 分组：
//! - `doc`：当前稿件，只读；
//! - `workspace`：工作稿，读写；
//! - `store`：知识库与稿件库；
//! - `vocab`：标准词库与规范；
//! - `check`：确定性检查；
//! - `calc`：计算与文本；
//! - `http`：外部系统接口（`http.call`，只查询，只能调设置里配好的接口）；
//! - `interact`：模型、选择题、留言。

mod calc;
mod check;
mod doc;
mod http;
mod interact;
mod store;
mod vocab;
mod workspace;

pub(crate) use interact::ASSIST_SYSTEM;
#[cfg(test)]
pub(crate) use store::ManuscriptDoc;
pub(crate) use store::{ManuscriptSource, SqliteManuscripts};

use super::api::{ApiSecrets, ApiStore};
use super::backend::ModelBackend;
use super::board::Board;
use super::clarify::Question;
use super::engine::Event;
use super::evidence::EvidenceDoc;
use super::skill::Skill;
use crate::models::{AppConfig, LmStudioConfig, RagConfig, TemplateKind, VocabularyEntry};
use crate::rag::RetrievedChunk;
use serde_json::{Map, Value};

/// 工具的权限级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Permission {
    /// 只读：读稿件、检索、查词库、计算。
    Read,
    /// 检查：确定性，不改任何东西。
    Check,
    /// 写 AI 工作稿。工作稿不是正文，落地要用户接受。
    WriteWorkspace,
    /// 访问外部系统（只查询）。
    External,
    /// 问用户：出选择题，流程挂起等回答。
    AskUser,
}

impl Permission {
    pub(crate) fn icon(self) -> &'static str {
        match self {
            Self::Read => "⌕",
            Self::Check => "✓",
            Self::WriteWorkspace => "✎",
            Self::External => "⇄",
            Self::AskUser => "?",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Read => "只读",
            Self::Check => "检查",
            Self::WriteWorkspace => "写工作稿",
            Self::External => "外部调用",
            Self::AskUser => "问用户",
        }
    }
}

/// 一次工具调用留下的记录，任务流里显示成一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolUse {
    pub(crate) tool: &'static str,
    pub(crate) permission: Permission,
    pub(crate) summary: String,
}

impl ToolUse {
    pub(crate) fn new(tool: &'static str, permission: Permission, summary: String) -> Self {
        Self {
            tool,
            permission,
            summary,
        }
    }

    pub(crate) fn line(&self) -> String {
        format!("{} {}", self.permission.icon(), self.summary)
    }
}

/// 知识库（只读）。测试时换成假知识库。
pub(crate) trait KnowledgeSearch {
    /// 知识库没启用时不检索，可检索的缺口直接记为「知识库无答案」。
    fn enabled(&self) -> bool;
    /// 返回 (片段, 降级说明)。
    fn search(&self, query: &str) -> anyhow::Result<(Vec<RetrievedChunk>, Vec<String>)>;
    /// 一篇文档的 (标题, 全文)。
    fn read(&self, _doc_id: i64) -> anyhow::Result<Option<(String, String)>> {
        Ok(None)
    }
    /// 文档列表：(id, 标题, 文种)。
    fn list(
        &self,
        _kind: Option<TemplateKind>,
    ) -> anyhow::Result<Vec<(i64, String, TemplateKind)>> {
        Ok(Vec::new())
    }
}

/// 本机知识库，检索与知识库页的检索、问答走同一个 `rag::retrieve`。
pub(crate) struct RagSearch {
    pub(crate) enabled: bool,
    pub(crate) rag: RagConfig,
    pub(crate) chat: LmStudioConfig,
    pub(crate) kind_filter: Option<TemplateKind>,
}

impl KnowledgeSearch for RagSearch {
    fn enabled(&self) -> bool {
        self.enabled
    }

    fn search(&self, query: &str) -> anyhow::Result<(Vec<RetrievedChunk>, Vec<String>)> {
        let db_path = crate::storage::manuscript_db_path()?;
        let outcome =
            crate::rag::retrieve(&self.rag, &self.chat, &db_path, query, self.kind_filter)?;
        Ok((outcome.chunks, outcome.warnings))
    }

    fn read(&self, doc_id: i64) -> anyhow::Result<Option<(String, String)>> {
        let db_path = crate::storage::manuscript_db_path()?;
        crate::knowledge::KnowledgeStore::open(&db_path)?.get_doc_content(doc_id)
    }

    fn list(&self, kind: Option<TemplateKind>) -> anyhow::Result<Vec<(i64, String, TemplateKind)>> {
        let db_path = crate::storage::manuscript_db_path()?;
        Ok(crate::knowledge::KnowledgeStore::open(&db_path)?
            .list_docs(kind)?
            .into_iter()
            .map(|row| (row.id, row.title, row.kind))
            .collect())
    }
}

/// 工具运行的环境：配置、词库、数据源、模型、当前技能。
pub(crate) struct Env<'a> {
    pub(crate) config: &'a AppConfig,
    pub(crate) vocabulary: &'a [VocabularyEntry],
    pub(crate) kb: &'a dyn KnowledgeSearch,
    pub(crate) manuscripts: &'a dyn ManuscriptSource,
    pub(crate) model: &'a dyn ModelBackend,
    pub(crate) skill: &'a Skill,
    /// 设置里配好的数据接口与密钥。
    pub(crate) apis: &'a ApiStore,
    pub(crate) secrets: &'a ApiSecrets,
}

/// 一次工具调用的上下文。
pub(crate) struct ToolCtx<'a, 'e> {
    pub(crate) board: &'a mut Board,
    pub(crate) env: &'a Env<'e>,
    pub(crate) emit: &'a mut dyn FnMut(Event),
}

impl ToolCtx<'_, '_> {
    /// 检查类工具的默认对象：给了 `text` 用它，否则工作稿，工作稿为空就用正文。
    pub(crate) fn text_arg(&self, args: &Map<String, Value>) -> String {
        if let Some(text) = args.get("text").and_then(Value::as_str) {
            return text.to_string();
        }
        if self.board.workspace.trim().is_empty() {
            self.board.document.clone()
        } else {
            self.board.workspace.clone()
        }
    }
}

/// 工具的一个输入。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Input {
    pub(crate) name: &'static str,
    pub(crate) required: bool,
    pub(crate) doc: &'static str,
}

pub(crate) const fn required(name: &'static str, doc: &'static str) -> Input {
    Input {
        name,
        required: true,
        doc,
    }
}

pub(crate) const fn optional(name: &'static str, doc: &'static str) -> Input {
    Input {
        name,
        required: false,
        doc,
    }
}

/// 工具的产出。
#[derive(Debug, Clone, Default)]
pub(crate) struct ToolOutput {
    pub(crate) value: Value,
    /// 过程里那一行的人话。
    pub(crate) summary: String,
    /// 资料类工具的产出；步骤写了 `evidence: true`（或工具默认）时并入证据包。
    pub(crate) evidence: Vec<EvidenceDoc>,
    /// 资料默认进证据包。旧稿这类只作写法参考的，由步骤写 `evidence: false` 关掉。
    pub(crate) evidence_by_default: bool,
    /// 问用户：流程在这里挂起。
    pub(crate) suspend: Option<Vec<Question>>,
}

impl ToolOutput {
    pub(crate) fn new(value: Value, summary: impl Into<String>) -> Self {
        Self {
            value,
            summary: summary.into(),
            ..Self::default()
        }
    }
}

pub(crate) trait Tool: Sync {
    fn id(&self) -> &'static str;
    fn permission(&self) -> Permission;
    /// 一句话：能做什么。给技能作者与模型看。
    fn description(&self) -> &'static str;
    fn inputs(&self) -> &'static [Input];
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String>;
}

/// 全部工具。
pub(crate) fn all() -> Vec<&'static dyn Tool> {
    let mut tools: Vec<&'static dyn Tool> = Vec::new();
    tools.extend(doc::TOOLS);
    tools.extend(workspace::TOOLS);
    tools.extend(store::TOOLS);
    tools.extend(vocab::TOOLS);
    tools.extend(check::TOOLS);
    tools.extend(calc::TOOLS);
    tools.extend(http::TOOLS);
    tools.extend(interact::TOOLS);
    tools
}

pub(crate) fn find(id: &str) -> Option<&'static dyn Tool> {
    let base = id.split(':').next().unwrap_or(id);
    all().into_iter().find(|tool| tool.id() == base)
}

/// 「⌕ kb.search（只读）：检索本机知识库……」——技能条的悬浮说明里列出技能能用的工具。
pub(crate) fn describe(id: &str) -> Option<String> {
    let tool = find(id)?;
    let permission = tool.permission();
    Some(format!(
        "{} {id}（{}）：{}",
        permission.icon(),
        permission.label(),
        tool.description()
    ))
}

pub(crate) fn ids() -> Vec<&'static str> {
    all().into_iter().map(|tool| tool.id()).collect()
}

/// 调一个工具：查白名单、补参数、跑，出错以文字返回，不让流程崩掉。
pub(crate) fn call(
    id: &str,
    ctx: &mut ToolCtx<'_, '_>,
    args: &Value,
) -> Result<ToolOutput, String> {
    let tool = find(id).ok_or_else(|| format!("没有工具「{id}」"))?;
    let mut args = match args {
        Value::Object(map) => map.clone(),
        Value::Null => Map::new(),
        _ => return Err(format!("工具「{id}」的参数必须是键值对象")),
    };
    // `http.call:stat` 这类限定写法：限定名就是 `api` 参数。
    if let Some((_, qualifier)) = id.split_once(':') {
        match args.get("api").and_then(Value::as_str) {
            Some(api) if api != qualifier => {
                return Err(format!(
                    "工具「{id}」的 api 参数写成了「{api}」，与限定名不一致"
                ));
            }
            _ => {
                args.insert("api".into(), Value::String(qualifier.to_string()));
            }
        }
    }
    // 白名单按「工具:接口」查：只声明了 `http.call:stat` 的技能调不到别的接口。
    let checked = match (tool.id(), args.get("api").and_then(Value::as_str)) {
        ("http.call", Some(api)) => format!("http.call:{api}"),
        _ => id.to_string(),
    };
    if !ctx.env.skill.allows_tool(&checked) {
        return Err(format!(
            "技能「{}」没有声明工具「{checked}」",
            ctx.env.skill.name
        ));
    }
    let args = &args;
    for input in tool.inputs().iter().filter(|input| input.required) {
        let missing = match args.get(input.name) {
            None | Some(Value::Null) => true,
            Some(Value::String(text)) => text.trim().is_empty(),
            _ => false,
        };
        if missing {
            return Err(format!(
                "工具「{id}」缺少参数 {}（{}）",
                input.name, input.doc
            ));
        }
    }
    tool.run(ctx, args)
}

// —— 参数取值 ——

pub(crate) fn arg_str(args: &Map<String, Value>, name: &str) -> Option<String> {
    match args.get(name)? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

pub(crate) fn arg_f64(args: &Map<String, Value>, name: &str) -> Option<f64> {
    match args.get(name)? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => parse_number(text),
        _ => None,
    }
}

pub(crate) fn arg_i64(args: &Map<String, Value>, name: &str) -> Option<i64> {
    match args.get(name)? {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

pub(crate) fn arg_usize(args: &Map<String, Value>, name: &str) -> Option<usize> {
    arg_i64(args, name).and_then(|n| usize::try_from(n).ok())
}

pub(crate) fn arg_bool(args: &Map<String, Value>, name: &str) -> Option<bool> {
    match args.get(name)? {
        Value::Bool(flag) => Some(*flag),
        Value::String(text) => match text.trim() {
            "true" | "是" | "1" => Some(true),
            "false" | "否" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// 数字列表：JSON 数组，或一段用逗号、顿号、空白分隔的文字。
pub(crate) fn arg_numbers(args: &Map<String, Value>, name: &str) -> Option<Vec<f64>> {
    match args.get(name)? {
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::Number(n) => n.as_f64(),
                Value::String(s) => parse_number(s),
                _ => None,
            })
            .collect(),
        Value::String(text) => text
            .split(|c: char| c == ',' || c == '，' || c == '、' || c.is_whitespace())
            .filter(|part| !part.is_empty())
            .map(parse_number)
            .collect(),
        _ => None,
    }
}

pub(crate) fn arg_strings(args: &Map<String, Value>, name: &str) -> Vec<String> {
    match args.get(name) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .collect(),
        Some(Value::String(text)) => text
            .split(['\n', ',', '，', '、'])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// 「1,234.5」「12.5%」→ 数字（百分号去掉，不除以 100）。
pub(crate) fn parse_number(text: &str) -> Option<f64> {
    let cleaned: String = text
        .trim()
        .trim_end_matches(['%', '％'])
        .chars()
        .filter(|c| *c != ',' && *c != '，')
        .collect();
    cleaned.parse().ok()
}

/// 文种名或内部名 → 文种。
pub(crate) fn kind_arg(args: &Map<String, Value>, name: &str) -> Option<TemplateKind> {
    let text = arg_str(args, name)?;
    TemplateKind::ALL.into_iter().find(|kind| {
        kind.label() == text.trim() || crate::manuscript::kind_to_str(*kind) == text.trim()
    })
}

/// 截断成给人看的短文字。
pub(crate) fn short(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect::<String>() + "…"
}

#[cfg(test)]
pub(crate) mod testing;

#[cfg(test)]
mod tests {
    use super::testing::Fixture;
    use super::*;
    use serde_json::json;

    #[test]
    fn every_tool_has_a_unique_id_description_and_documented_inputs() {
        let ids = ids();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "工具 id 重复");
        for tool in all() {
            // 设计里只有留言工具叫 `note`，其余都按「组.动作」点分命名。
            assert!(
                tool.id().contains('.') || tool.id() == "note",
                "{} 应当用点分命名",
                tool.id()
            );
            assert!(!tool.description().is_empty(), "{} 缺少说明", tool.id());
            for input in tool.inputs() {
                assert!(
                    !input.doc.is_empty(),
                    "{} 的 {} 缺少说明",
                    tool.id(),
                    input.name
                );
            }
        }
    }

    #[test]
    fn tools_outside_the_whitelist_and_missing_arguments_are_refused() {
        let mut fixture = Fixture::new("# 标题\n\n正文。\n");
        fixture.skill.tools = vec!["doc.read".into()];
        let error = fixture
            .call("ws.write", json!({"text": "改写"}))
            .unwrap_err();
        assert!(error.contains("没有声明工具"), "{error}");
        fixture.skill.tools.push("ws.write".into());
        let error = fixture.call("ws.write", json!({})).unwrap_err();
        assert!(error.contains("缺少参数 text"), "{error}");
        assert!(
            fixture
                .call("没有.这个", json!({}))
                .unwrap_err()
                .contains("没有工具")
        );
    }

    #[test]
    fn number_arguments_accept_text() {
        let args = json!({"a": "1,234.5", "b": "12.5%", "list": "1、2，3 4"});
        let map = args.as_object().unwrap();
        assert_eq!(arg_f64(map, "a"), Some(1234.5));
        assert_eq!(arg_f64(map, "b"), Some(12.5));
        assert_eq!(arg_numbers(map, "list"), Some(vec![1.0, 2.0, 3.0, 4.0]));
    }
}
