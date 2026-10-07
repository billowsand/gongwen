//! SKILL.md 技能文件：一个技能的说明、适用条件、可用工具、流程与各步提示词。
//!
//! 格式沿用 Agent Skills 的写法——开头是标准 YAML，正文是提示词（`docs/ai-agent-workbench.md` 16.5）：
//!
//! ```text
//! ---
//! name: 研究式起草
//! description: ……
//! triggers: [起草, 写一份]
//! when: { text: any }
//! tools: [kb.search, llm.generate]
//! params: { max_rounds: 3 }
//! flow:
//!   - step: clarify          # 执行一个算子
//!     prompt: 动笔前澄清
//!   - tool: kb.search         # 直接调一个工具
//!     args: { query: "{request}" }
//!     save_as: hits
//! ---
//! ## 动笔前澄清
//! 这一步的提示词，{变量} 由引擎替换
//! ```
//!
//! 内置技能编进二进制；用户技能放在 `配置目录/skills/<id>/SKILL.md`。同 id 的用户技能覆盖内置，
//! 缺的步骤提示词、参数、流程沿用内置——改一处提示词不必把整份抄全。

use crate::models::TemplateKind;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// 内置技能：(id, 文件内容)。
const BUILTIN: [(&str, &str); 15] = [
    (
        RESEARCH_DRAFT,
        include_str!("../../assets/agent-skills/research-draft/SKILL.md"),
    ),
    (
        POLICY_REPORT,
        include_str!("../../assets/agent-skills/policy-report/SKILL.md"),
    ),
    (
        IMITATE,
        include_str!("../../assets/agent-skills/imitate/SKILL.md"),
    ),
    (
        MATERIAL,
        include_str!("../../assets/agent-skills/material/SKILL.md"),
    ),
    (
        REPLY_LETTER,
        include_str!("../../assets/agent-skills/reply-letter/SKILL.md"),
    ),
    (
        POLISH,
        include_str!("../../assets/agent-skills/polish/SKILL.md"),
    ),
    (
        CONDENSE,
        include_str!("../../assets/agent-skills/condense/SKILL.md"),
    ),
    (
        TONE,
        include_str!("../../assets/agent-skills/tone/SKILL.md"),
    ),
    (
        NORMALIZE,
        include_str!("../../assets/agent-skills/normalize/SKILL.md"),
    ),
    (
        REVIEW,
        include_str!("../../assets/agent-skills/review/SKILL.md"),
    ),
    (
        FACT_CHECK,
        include_str!("../../assets/agent-skills/fact-check/SKILL.md"),
    ),
    (
        EXTRACT,
        include_str!("../../assets/agent-skills/extract/SKILL.md"),
    ),
    (
        POLICY_BASIS,
        include_str!("../../assets/agent-skills/policy-basis/SKILL.md"),
    ),
    (
        FREE_TASK,
        include_str!("../../assets/agent-skills/free-task/SKILL.md"),
    ),
    (
        STYLE_LEARN,
        include_str!("../../assets/agent-skills/style-learn/SKILL.md"),
    ),
];

pub(crate) const RESEARCH_DRAFT: &str = "research-draft";
pub(crate) const POLISH: &str = "polish";
pub(crate) const POLICY_REPORT: &str = "policy-report";
pub(crate) const IMITATE: &str = "imitate";
pub(crate) const MATERIAL: &str = "material";
pub(crate) const REPLY_LETTER: &str = "reply-letter";
pub(crate) const CONDENSE: &str = "condense";
pub(crate) const TONE: &str = "tone";
pub(crate) const NORMALIZE: &str = "normalize";
pub(crate) const REVIEW: &str = "review";
pub(crate) const FACT_CHECK: &str = "fact-check";
pub(crate) const EXTRACT: &str = "extract";
pub(crate) const POLICY_BASIS: &str = "policy-basis";
pub(crate) const FREE_TASK: &str = "free-task";
pub(crate) const STYLE_LEARN: &str = "style-learn";

/// 技能产出什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OutputKind {
    /// 提案：新稿或改写，用户接受才落入正文。
    #[default]
    Proposal,
    /// 只出问题清单，不改稿。
    Report,
    /// 看结果定（自主步骤用）：工作稿改过就是提案，没改就是清单（自主步骤的答复作一条）。
    Auto,
}

impl OutputKind {
    /// 这次交清单还是提案：`auto` 看工作稿和发起时的正文是否不同。
    pub(crate) fn is_report(self, board: &super::board::Board) -> bool {
        match self {
            OutputKind::Proposal => false,
            OutputKind::Report => true,
            OutputKind::Auto => {
                let workspace = board.workspace.trim();
                workspace.is_empty() || workspace == board.document.trim()
            }
        }
    }
}

/// `@` 引用的文章怎么用（16.13、决定 F14）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RefUse {
    /// 并入证据包：可引用、要核验。
    #[default]
    Evidence,
    /// 第一篇当基准稿（变量 `baseline` / `baseline_id`，不进证据包），其余当证据。
    Baseline,
    /// 全文并进材料（`{request}` 前面）。
    Material,
    /// 当来函：全文以「来函」并进材料。
    Letter,
    /// 当样稿：引擎不处理，交给算子自己读（风格学习）。
    Sample,
}

/// 对正文的要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TextNeed {
    #[default]
    Any,
    Empty,
    Present,
}

/// 对选区的要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SelectionNeed {
    #[default]
    Any,
    Required,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
pub(crate) struct WhenSpec {
    #[serde(default)]
    pub(crate) text: TextNeed,
    #[serde(default)]
    pub(crate) selection: SelectionNeed,
}

/// 流程里的一步：`step`（算子）与 `tool`（工具）二选一。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct StepSpec {
    #[serde(default)]
    pub(crate) step: Option<String>,
    #[serde(default)]
    pub(crate) tool: Option<String>,
    /// 工具的输入；字符串里的 `{变量}` 由引擎替换。
    #[serde(default)]
    pub(crate) args: Option<Value>,
    /// 结果存进黑板的变量名。
    #[serde(default)]
    pub(crate) save_as: Option<String>,
    /// 工具结果是否并入证据包（只对资料类工具有意义）。
    #[serde(default)]
    pub(crate) evidence: Option<bool>,
    /// 执行条件：`has_sources`、`has_text`……或 `{ kind: [研究报告] }`、`{ var: 名字 }`。
    #[serde(default)]
    pub(crate) when: Option<Value>,
    /// `for_each` 的子流程。
    #[serde(default, rename = "do")]
    pub(crate) body: Vec<StepSpec>,
    /// 算子的其余参数：`prompt`、`max`、`rounds`……
    #[serde(flatten)]
    pub(crate) params: BTreeMap<String, Value>,
}

impl StepSpec {
    /// 「算子 clarify」「工具 kb.search」——过程记录与校验信息里用。
    pub(crate) fn label(&self) -> String {
        match (&self.step, &self.tool) {
            (Some(step), _) => format!("算子 {step}"),
            (None, Some(tool)) => format!("工具 {tool}"),
            (None, None) => "空步骤".into(),
        }
    }

    pub(crate) fn param_str(&self, key: &str) -> Option<&str> {
        self.params.get(key).and_then(Value::as_str)
    }

    pub(crate) fn param_usize(&self, key: &str) -> Option<usize> {
        self.params.get(key).and_then(as_usize)
    }
}

#[derive(Debug, Deserialize)]
struct Frontmatter {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    hint: String,
    #[serde(default)]
    triggers: Vec<String>,
    #[serde(default)]
    applies_to: Vec<String>,
    #[serde(default)]
    when: WhenSpec,
    #[serde(default)]
    output: OutputKind,
    #[serde(default)]
    references: RefUse,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    params: BTreeMap<String, Value>,
    #[serde(default)]
    flow: Vec<StepSpec>,
    #[serde(default = "enabled_default")]
    enabled: bool,
    /// 第二期的旧写法把参数直接写在顶层（`max_rounds: 3`），照样认作参数。
    #[serde(flatten)]
    legacy: BTreeMap<String, Value>,
}

fn enabled_default() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Skill {
    /// 目录名，技能的稳定标识。
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: String,
    /// 输入框的占位提示：告诉用户这个技能要写些什么。
    pub(crate) hint: String,
    pub(crate) triggers: Vec<String>,
    /// 适用文种；空表示全部。
    pub(crate) applies_to: Vec<TemplateKind>,
    pub(crate) when: WhenSpec,
    pub(crate) output: OutputKind,
    /// `@` 引用的文章怎么用。
    pub(crate) references: RefUse,
    /// 允许使用的工具（白名单）。
    pub(crate) tools: Vec<String>,
    pub(crate) params: BTreeMap<String, Value>,
    pub(crate) flow: Vec<StepSpec>,
    pub(crate) enabled: bool,
    /// 二级标题 → 提示词。
    pub(crate) sections: BTreeMap<String, String>,
    /// 内置，或用户文件的路径。
    pub(crate) origin: String,
    /// 解析时发现的问题（未知文种名之类），校验时一并报出。
    pub(crate) warnings: Vec<String>,
}

impl Skill {
    pub(crate) fn section(&self, name: &str) -> Option<&str> {
        self.sections.get(name).map(String::as_str)
    }

    /// 整数参数；缺了或写错就用默认值，并夹在 `range` 里，防止手误写出 0 轮或 1000 轮。
    pub(crate) fn param_usize(
        &self,
        key: &str,
        default: usize,
        range: std::ops::RangeInclusive<usize>,
    ) -> usize {
        self.params
            .get(key)
            .and_then(as_usize)
            .unwrap_or(default)
            .clamp(*range.start(), *range.end())
    }

    /// 流程里用不用知识库：有检索算子、列检索问题的 `plan`，或直接调知识库工具。侧栏据此
    /// 显示「检索知识库」开关；材料成文这类只用材料的技能不显示，也不检索。
    pub(crate) fn uses_knowledge(&self) -> bool {
        // 自主步骤没限定工具时用整个白名单。
        let agent_uses_kb = |step: &StepSpec| match step.params.get("tools") {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(Value::as_str)
                .any(|id| id.starts_with("kb.")),
            _ => self.tools.iter().any(|id| id.starts_with("kb.")),
        };
        fn walk(steps: &[StepSpec], agent_uses_kb: &dyn Fn(&StepSpec) -> bool) -> bool {
            steps.iter().any(|step| {
                let op = step.step.as_deref();
                matches!(op, Some("retrieve" | "fact_check" | "cite_check"))
                    || (op == Some("plan")
                        && matches!(step.param_str("mode"), None | Some("queries")))
                    || step
                        .tool
                        .as_deref()
                        .is_some_and(|tool| tool.starts_with("kb."))
                    || (op == Some("agent") && agent_uses_kb(step))
                    || walk(&step.body, agent_uses_kb)
            })
        }
        walk(&self.flow, &agent_uses_kb)
    }

    /// 提示词里用不用润色预设（`{preset}`）。侧栏据此显示预设下拉框。
    pub(crate) fn uses_preset(&self) -> bool {
        self.sections.values().any(|text| text.contains("{preset}"))
    }

    /// 白名单里有没有这个工具。`http.call:接口` 这类带限定的写法，写了 `http.call` 也算全放行。
    pub(crate) fn allows_tool(&self, id: &str) -> bool {
        self.tools.iter().any(|allowed| {
            allowed == id || id.split_once(':').is_some_and(|(base, _)| allowed == base)
        })
    }

    /// 用 `fallback` 补齐缺失的提示词、参数、流程与说明。
    pub(crate) fn merged_with(mut self, fallback: &Skill) -> Skill {
        for (key, value) in &fallback.sections {
            self.sections
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
        for (key, value) in &fallback.params {
            self.params
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
        let fill = |own: &mut String, other: &String| {
            if own.is_empty() {
                own.clone_from(other);
            }
        };
        fill(&mut self.name, &fallback.name);
        fill(&mut self.description, &fallback.description);
        fill(&mut self.hint, &fallback.hint);
        if self.triggers.is_empty() {
            self.triggers.clone_from(&fallback.triggers);
        }
        if self.tools.is_empty() {
            self.tools.clone_from(&fallback.tools);
        }
        if self.flow.is_empty() {
            self.flow.clone_from(&fallback.flow);
            // 流程沿用内置时，适用条件与产出也沿用，免得两边对不上。
            self.when = fallback.when;
            self.output = fallback.output;
            if self.applies_to.is_empty() {
                self.applies_to.clone_from(&fallback.applies_to);
            }
        }
        self
    }
}

fn as_usize(value: &Value) -> Option<usize> {
    value
        .as_u64()
        .map(|n| n as usize)
        .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
}

/// 文种名（「研究报告」）或内部名（`ResearchReport`）→ 文种。
fn kind_from_name(name: &str) -> Option<TemplateKind> {
    let name = name.trim();
    TemplateKind::ALL
        .into_iter()
        .find(|kind| kind.label() == name || crate::manuscript::kind_to_str(*kind) == name)
}

/// 解析 SKILL.md。
pub(crate) fn parse(id: &str, text: &str, origin: &str) -> Result<Skill, String> {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let rest = text
        .strip_prefix("---\n")
        .ok_or("SKILL.md 缺少开头的 --- frontmatter")?;
    let (front, body) = rest
        .split_once("\n---")
        .ok_or("SKILL.md 的 frontmatter 没有用 --- 收尾")?;
    let front: Frontmatter = if front.trim().is_empty() {
        serde_yaml::from_str("{}").expect("空对象")
    } else {
        serde_yaml::from_str(front).map_err(|error| format!("YAML 开头解析失败：{error}"))?
    };
    let mut warnings = Vec::new();
    let mut applies_to = Vec::new();
    for name in &front.applies_to {
        match kind_from_name(name) {
            Some(kind) => applies_to.push(kind),
            None => warnings.push(format!("applies_to 里的「{name}」不是认识的文种")),
        }
    }
    let mut params = front.params;
    for (key, value) in front.legacy {
        params.entry(key).or_insert(value);
    }
    Ok(Skill {
        id: id.to_string(),
        name: front.name,
        description: front.description,
        hint: front.hint,
        triggers: front.triggers,
        applies_to,
        when: front.when,
        output: front.output,
        references: front.references,
        tools: front.tools,
        params,
        flow: front.flow,
        enabled: front.enabled,
        sections: parse_sections(body),
        origin: origin.to_string(),
        warnings,
    })
}

/// 正文按二级标题切段；一级标题与第一个二级标题之前的说明不算提示词。
fn parse_sections(body: &str) -> BTreeMap<String, String> {
    let mut sections = BTreeMap::new();
    let mut current: Option<(String, String)> = None;
    for line in body.lines() {
        if let Some(title) = line.strip_prefix("## ") {
            if let Some((name, text)) = current.take() {
                sections.insert(name, text.trim().to_string());
            }
            current = Some((title.trim().to_string(), String::new()));
        } else if line.starts_with("# ") {
            if let Some((name, text)) = current.take() {
                sections.insert(name, text.trim().to_string());
            }
        } else if let Some((_, text)) = current.as_mut() {
            text.push_str(line);
            text.push('\n');
        }
    }
    if let Some((name, text)) = current {
        sections.insert(name, text.trim().to_string());
    }
    sections
}

/// 静态校验：步骤写法、算子与工具名、白名单、引用的提示词。返回问题清单，空表示通过。
///
/// `operators` 与 `tools` 由调用方给（引擎认识哪些），本模块不依赖引擎。
pub(crate) fn validate(skill: &Skill, operators: &[&str], tools: &[&str]) -> Vec<String> {
    let mut problems = skill.warnings.clone();
    if skill.name.trim().is_empty() {
        problems.push("缺少 name".into());
    }
    if skill.flow.is_empty() {
        problems.push("flow 是空的，技能什么都不会做".into());
    }
    for tool in &skill.tools {
        let base = tool.split(':').next().unwrap_or(tool);
        let base = base.strip_suffix(".phone").unwrap_or(base);
        if !tools.contains(&base) {
            problems.push(format!("tools 里的「{tool}」不是认识的工具"));
        }
    }
    validate_steps(skill, &skill.flow, operators, tools, &mut problems, "");
    problems
}

fn validate_steps(
    skill: &Skill,
    steps: &[StepSpec],
    operators: &[&str],
    tools: &[&str],
    problems: &mut Vec<String>,
    prefix: &str,
) {
    for (index, step) in steps.iter().enumerate() {
        let at = format!("{prefix}第 {} 步", index + 1);
        match (&step.step, &step.tool) {
            (Some(_), Some(_)) => problems.push(format!("{at}同时写了 step 和 tool，只能二选一")),
            (None, None) => problems.push(format!("{at}既没有 step 也没有 tool")),
            (Some(op), None) => {
                if !operators.contains(&op.as_str()) {
                    problems.push(format!("{at}的算子「{op}」不认识"));
                }
                if op == "for_each" {
                    if step.body.is_empty() {
                        problems.push(format!("{at} for_each 缺少 do 子流程"));
                    }
                    if step.param_str("over").is_none() {
                        problems.push(format!("{at} for_each 缺少 over（要遍历的变量）"));
                    }
                    validate_steps(
                        skill,
                        &step.body,
                        operators,
                        tools,
                        problems,
                        &format!("{at}的子流程"),
                    );
                }
                if op == "agent" {
                    agent_problems(skill, step, &at, problems);
                }
            }
            (None, Some(tool)) => {
                if !tools.contains(&tool.split(':').next().unwrap_or(tool)) {
                    problems.push(format!("{at}的工具「{tool}」不认识"));
                } else if !skill.allows_tool(tool) {
                    problems.push(format!("{at}用了工具「{tool}」，但它不在 tools 白名单里"));
                }
                // 运行时流程里的工具调用不查多余参数（`tools::Caller::Flow`），拼错的参数名在这里报。
                if let (Some(found), Some(Value::Object(args))) =
                    (super::tools::find(tool), step.args.as_ref())
                    && !found.open_args()
                {
                    for name in args.keys() {
                        if !found.inputs().iter().any(|input| input.name == name) {
                            problems.push(format!("{at}给工具「{tool}」的参数「{name}」它不认识"));
                        }
                    }
                }
            }
        }
        if let Some(condition) = &step.when {
            condition_problems(condition, &at, problems);
        }
        if let Some(Value::Array(items)) = step.params.get("apis") {
            for item in items {
                let id = match item {
                    Value::String(id) => Some(id.as_str()),
                    Value::Object(map) => map.get("api").and_then(Value::as_str),
                    _ => None,
                };
                match id {
                    Some(id) if skill.allows_tool(&format!("http.call:{id}")) => {}
                    Some(id) => problems.push(format!(
                        "{at}的 apis 用了接口「{id}」，但 tools 里没有声明 http.call:{id}"
                    )),
                    None => problems.push(format!(
                        "{at}的 apis 写法不对：写接口 id，或 {{ api: id, args: {{…}} }}"
                    )),
                }
            }
        }
        for key in [
            "prompt",
            "evidence_prompt",
            "fill_prompt",
            "source_prompt",
            "elements_prompt",
            "generalize_prompt",
        ] {
            if let Some(name) = step.param_str(key)
                && skill.section(name).is_none()
            {
                problems.push(format!("{at}引用的提示词「{name}」在正文里找不到"));
            }
        }
    }
}

/// 自主步骤的写法：工具都在白名单里、不问用户、`require` 认得、目标提示词在。
fn agent_problems(skill: &Skill, step: &StepSpec, at: &str, problems: &mut Vec<String>) {
    if let Some(value) = step.params.get("tools") {
        match value {
            Value::Array(items) => {
                for id in items.iter().filter_map(Value::as_str) {
                    if !skill.allows_tool(id) {
                        problems.push(format!("{at}自主步骤的工具「{id}」不在 tools 白名单里"));
                    } else if super::tools::find(id).is_some_and(|tool| {
                        tool.id() == "llm.generate"
                            || tool.permission() == super::tools::Permission::AskUser
                    }) {
                        problems.push(format!(
                            "{at}自主步骤不能用「{id}」（模型本身就在跑；问用户请放到后面的 ask 步骤）"
                        ));
                    }
                }
            }
            _ => problems.push(format!("{at}自主步骤的 tools 要写成列表")),
        }
    }
    match step.params.get("direct") {
        None => {}
        Some(Value::Array(items)) => {
            for id in items.iter().filter_map(Value::as_str) {
                if !skill.allows_tool(id) {
                    problems.push(format!(
                        "{at}自主步骤 direct 里的「{id}」不在 tools 白名单里"
                    ));
                }
            }
        }
        Some(_) => problems.push(format!("{at}自主步骤的 direct 要写成列表")),
    }
    if let Some(require) = step.param_str("require")
        && !["workspace", "findings"].contains(&require)
    {
        problems.push(format!(
            "{at}自主步骤的 require「{require}」不认识（可用 workspace、findings）"
        ));
    }
    if step.param_str("prompt").is_none() && skill.section("任务").is_none() {
        problems.push(format!(
            "{at}自主步骤没写 prompt，正文里也没有默认的「任务」提示词"
        ));
    }
}

/// `when` 认的条件名（引擎按这张表求值）。
pub(crate) const CONDITIONS: [&str; 4] =
    ["has_sources", "has_text", "has_selection", "has_evidence"];

fn condition_problems(condition: &Value, at: &str, problems: &mut Vec<String>) {
    match condition {
        Value::String(name) if CONDITIONS.contains(&name.as_str()) => {}
        Value::String(name) => problems.push(format!(
            "{at}的条件「{name}」不认识（可用 {}，或 kind / var / not）",
            CONDITIONS.join("、")
        )),
        Value::Array(all) => {
            for item in all {
                condition_problems(item, at, problems);
            }
        }
        Value::Object(map) => {
            for (key, value) in map {
                match key.as_str() {
                    "kind" => {
                        let names: Vec<&str> = match value {
                            Value::String(name) => vec![name.as_str()],
                            Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
                            _ => Vec::new(),
                        };
                        if names.is_empty() {
                            problems.push(format!("{at}的 kind 条件要写文种名"));
                        }
                        for name in names {
                            if kind_from_name(name).is_none() {
                                problems
                                    .push(format!("{at}的 kind 条件里「{name}」不是认识的文种"));
                            }
                        }
                    }
                    "var" if value.is_string() => {}
                    "var" => problems.push(format!("{at}的 var 条件要写变量名")),
                    "not" => condition_problems(value, at, problems),
                    other => problems.push(format!("{at}的条件「{other}」不认识")),
                }
            }
        }
        _ => problems.push(format!("{at}的 when 写法不对")),
    }
}

/// 内置技能（解析失败是编译期就该发现的错误，测试锁住）。
pub(crate) fn builtin_skills() -> Vec<Skill> {
    BUILTIN
        .iter()
        .map(|(id, text)| parse(id, text, "内置").expect("内置 SKILL.md 必须能解析"))
        .collect()
}

#[cfg(test)]
pub(crate) fn builtin(id: &str) -> Option<Skill> {
    builtin_skills().into_iter().find(|skill| skill.id == id)
}

#[cfg(test)]
pub(crate) fn builtin_research_draft() -> Skill {
    builtin(RESEARCH_DRAFT).expect("内置研究式起草")
}

/// 内置技能的原文（技能页「复制为我的技能」、编辑器显示用）。
pub(crate) fn builtin_text(id: &str) -> Option<&'static str> {
    BUILTIN
        .iter()
        .find(|(builtin, _)| *builtin == id)
        .map(|(_, text)| *text)
}

/// 技能声明了、但「数据接口」里还没配的接口 id；配了但不能给 AI 调的注明原因。
pub(crate) fn missing_apis(skill: &Skill, apis: &crate::agent::api::ApiStore) -> Vec<String> {
    let mut missing: Vec<String> = skill
        .tools
        .iter()
        .filter_map(|tool| tool.strip_prefix("http.call:"))
        .filter_map(|id| match apis.get(id) {
            None => Some(id.to_string()),
            Some(endpoint) if !endpoint.readonly => Some(format!("{id}（会改数据，不给 AI 调）")),
            Some(endpoint) if !endpoint.ai => Some(format!("{id}（没开放给 AI）")),
            Some(_) => None,
        })
        .collect();
    missing.dedup();
    missing
}

/// 按引擎认识的算子与工具校验。
fn check(skill: &Skill) -> Vec<String> {
    validate(
        skill,
        &crate::agent::ops::names(),
        &crate::agent::tools::ids(),
    )
}

/// 全部技能：内置 + 用户目录。同 id 的用户技能覆盖内置（缺的部分用内置补齐）；用户文件
/// 写坏了不中断：覆盖内置的退回内置，新增的停用，各给一条说明。返回 (技能, 说明)。
pub(crate) fn load_all() -> (Vec<Skill>, Vec<String>) {
    let (mut skills, notes) = load_files();
    let disabled = crate::agent::skill_files::disabled_ids();
    for skill in &mut skills {
        if disabled.contains(&skill.id) {
            skill.enabled = false;
        }
    }
    (skills, notes)
}

fn load_files() -> (Vec<Skill>, Vec<String>) {
    let mut skills = builtin_skills();
    let mut notes = Vec::new();
    let Ok(dir) = crate::storage::config_dir() else {
        return (skills, notes);
    };
    let Ok(entries) = std::fs::read_dir(dir.join("skills")) else {
        return (skills, notes);
    };
    let mut user_dirs: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    user_dirs.sort();
    for path in user_dirs {
        let file = path.join("SKILL.md");
        let Some(id) = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let origin = file.display().to_string();
        let user = match parse(&id, &text, &origin) {
            Ok(user) => user,
            Err(error) => {
                notes.push(format!("技能文件 {origin} 解析失败，已跳过：{error}"));
                continue;
            }
        };
        match skills.iter().position(|skill| skill.id == id) {
            Some(index) => {
                let merged = user.merged_with(&skills[index]);
                let problems = check(&merged);
                if problems.is_empty() {
                    skills[index] = merged;
                } else {
                    notes.push(format!(
                        "技能文件 {origin} 有问题，沿用内置版本：{}",
                        problems.join("；")
                    ));
                }
            }
            None => {
                let mut user = user;
                let problems = check(&user);
                if !problems.is_empty() {
                    notes.push(format!(
                        "技能「{}」有问题，已停用：{}",
                        if user.name.is_empty() {
                            &id
                        } else {
                            &user.name
                        },
                        problems.join("；")
                    ));
                    user.enabled = false;
                }
                skills.push(user);
            }
        }
    }
    (skills, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPS: [&str; 3] = ["clarify", "generate", "for_each"];
    const TOOLS: [&str; 4] = ["kb.search", "llm.generate", "vocab.persons", "http.call"];

    #[test]
    fn declared_but_unconfigured_apis_are_reported() {
        use crate::agent::api::{ApiEndpoint, ApiStore};
        let skill = parse(
            "t",
            "---\nname: x\ntools: [http.call:stat, http.call:policy]\n---\n",
            "测试",
        )
        .unwrap();
        let apis = ApiStore {
            endpoints: vec![ApiEndpoint {
                id: "stat".into(),
                ..ApiEndpoint::default()
            }],
            ..Default::default()
        };
        assert_eq!(missing_apis(&skill, &apis), ["policy"]);
        assert_eq!(
            builtin_text(POLISH).map(|t| t.contains("name: 润色")),
            Some(true)
        );
        assert!(builtin_text("nope").is_none());
    }

    #[test]
    fn builtin_skills_parse_with_flows() {
        let skills = builtin_skills();
        assert_eq!(skills.len(), BUILTIN.len());
        let research = builtin_research_draft();
        assert_eq!(research.name, "研究式起草");
        assert!(!research.flow.is_empty());
        assert_eq!(research.flow[0].step.as_deref(), Some("clarify"));
        for step in [
            "动笔前澄清",
            "预研",
            "起草附加要求",
            "缺口修订",
            "来源核对",
            "核验",
        ] {
            assert!(research.section(step).is_some(), "缺少步骤：{step}");
        }
        assert_eq!(research.param_usize("max_rounds", 0, 1..=10), 3);
        let polish = builtin(POLISH).unwrap();
        assert_eq!(polish.when.text, TextNeed::Present);
        assert!(polish.section("润色").is_some());
    }

    #[test]
    fn builtin_skills_pass_validation_against_the_real_engine() {
        for skill in builtin_skills() {
            assert_eq!(
                check(&skill),
                Vec::<String>::new(),
                "内置技能「{}」校验不过",
                skill.name
            );
        }
    }

    #[test]
    fn steps_read_tools_args_conditions_and_nested_flows() {
        let text = "---\nname: 测试\ntools: [kb.search]\nflow:\n  - tool: kb.search\n    args: { query: \"{request}\", top: 5 }\n    save_as: hits\n    evidence: true\n    when: has_sources\n  - step: for_each\n    over: outline\n    do:\n      - step: generate\n        prompt: 写一节\n---\n## 写一节\n内容\n";
        let skill = parse("t", text, "测试").unwrap();
        let first = &skill.flow[0];
        assert_eq!(first.tool.as_deref(), Some("kb.search"));
        assert_eq!(first.args.as_ref().unwrap()["top"], 5);
        assert_eq!(first.save_as.as_deref(), Some("hits"));
        assert_eq!(first.evidence, Some(true));
        assert_eq!(first.when.as_ref().unwrap(), "has_sources");
        let second = &skill.flow[1];
        assert_eq!(second.param_str("over"), Some("outline"));
        assert_eq!(second.body[0].param_str("prompt"), Some("写一节"));
        assert!(
            validate(&skill, &OPS, &TOOLS).is_empty(),
            "{:?}",
            validate(&skill, &OPS, &TOOLS)
        );
    }

    #[test]
    fn validation_reports_every_kind_of_mistake() {
        let text = "---\nname: 坏\napplies_to: [公函, 不存在的文种]\ntools: [kb.search, 乱写.工具]\nflow:\n  - step: 乱写\n  - tool: llm.generate\n  - step: generate\n    tool: kb.search\n  - {}\n  - step: generate\n    prompt: 没有这段\n  - step: for_each\n  - step: generate\n    when: [has_text, 乱写, { kind: 不存在, not: { var: x } }]\n  - step: retrieve\n    apis: [stat]\n  - tool: kb.search\n    args: { qeury: 防火 }\n---\n";
        let skill = parse("bad", text, "测试").unwrap();
        let problems = validate(&skill, &OPS, &TOOLS).join("\n");
        for expected in [
            "不存在的文种",
            "「乱写.工具」不是认识的工具",
            "算子「乱写」不认识",
            "不在 tools 白名单里",
            "只能二选一",
            "既没有 step 也没有 tool",
            "提示词「没有这段」",
            "缺少 do",
            "缺少 over",
            "条件「乱写」不认识",
            "「不存在」不是认识的文种",
            "没有声明 http.call:stat",
            "给工具「kb.search」的参数「qeury」它不认识",
        ] {
            assert!(
                problems.contains(expected),
                "缺少「{expected}」：\n{problems}"
            );
        }
        assert_eq!(skill.applies_to, [TemplateKind::OfficialLetter]);
    }

    #[test]
    fn whitelist_handles_qualified_tools() {
        let skill = parse(
            "t",
            "---\nname: x\ntools: [http.call, vocab.persons.phone]\n---\n",
            "测试",
        )
        .unwrap();
        assert!(skill.allows_tool("http.call:stat"));
        assert!(skill.allows_tool("vocab.persons.phone"));
        assert!(
            !skill.allows_tool("vocab.persons"),
            "要电话得明写 .phone，反过来不放行"
        );
        assert!(
            validate(&skill, &OPS, &TOOLS)
                .iter()
                .all(|p| !p.contains("不是认识的工具"))
        );
    }

    #[test]
    fn broken_files_are_rejected_with_a_reason() {
        assert!(
            parse("x", "没有 frontmatter", "x")
                .unwrap_err()
                .contains("frontmatter")
        );
        assert!(
            parse("x", "---\nname: 甲\n", "x")
                .unwrap_err()
                .contains("收尾")
        );
        assert!(
            parse("x", "---\nflow: [不闭合\n---\n", "x")
                .unwrap_err()
                .contains("YAML")
        );
    }

    #[test]
    fn a_partial_or_legacy_user_skill_falls_back_to_builtin() {
        // 第二期的旧写法：参数写在顶层，没有 flow。
        let user = parse(
            RESEARCH_DRAFT,
            "---\nmax_rounds: 2\n---\n## 预研\n只查政策依据。\n",
            "用户",
        )
        .unwrap();
        let merged = user.merged_with(&builtin_research_draft());
        assert_eq!(merged.section("预研"), Some("只查政策依据。"));
        assert!(merged.section("缺口修订").is_some());
        assert_eq!(merged.param_usize("max_rounds", 3, 1..=10), 2);
        assert_eq!(merged.name, "研究式起草");
        assert_eq!(
            merged.flow,
            builtin_research_draft().flow,
            "没写 flow 就沿用内置流程"
        );
    }

    #[test]
    fn user_skills_are_loaded_merged_and_added() {
        let dir = std::env::temp_dir().join(format!("gongwen-skills-{}", std::process::id()));
        let skills_dir = dir.join("skills");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(skills_dir.join(RESEARCH_DRAFT)).unwrap();
        std::fs::create_dir_all(skills_dir.join("my-skill")).unwrap();
        std::fs::create_dir_all(skills_dir.join("broken")).unwrap();
        std::fs::write(
            skills_dir.join(RESEARCH_DRAFT).join("SKILL.md"),
            "---\nparams: { max_rounds: 1 }\n---\n",
        )
        .unwrap();
        std::fs::write(
            skills_dir.join("my-skill").join("SKILL.md"),
            "---\nname: 我的技能\nflow:\n  - step: generate\n---\n",
        )
        .unwrap();
        std::fs::write(skills_dir.join("broken").join("SKILL.md"), "坏掉的文件").unwrap();
        crate::storage::set_test_config_dir(Some(dir.clone()));
        let (skills, notes) = load_all();
        crate::storage::set_test_config_dir(None);
        let research = skills.iter().find(|s| s.id == RESEARCH_DRAFT).unwrap();
        assert_eq!(research.param_usize("max_rounds", 3, 1..=10), 1);
        assert!(!research.flow.is_empty());
        assert!(
            skills
                .iter()
                .any(|s| s.id == "my-skill" && s.name == "我的技能")
        );
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("broken"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
