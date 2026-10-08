//! 黑板：一次技能运行里，所有步骤共享的数据。
//!
//! 算子与工具都只通过黑板交换数据：用户原话、已确认的回答、要素（只读）、正文快照（只读）、
//! 选区、工作稿、证据包、缺口台账、变量、待问的题。黑板可以整块交回界面线程保存，挂起后
//! 原样带回来接着跑。

use super::clarify::Question;
use super::evidence::EvidencePack;
use super::gaps::Ledger;
use crate::models::{DraftInput, TemplateKind};
use regex::Regex;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::LazyLock;

/// 模板变量：`{request}`、`{baseline.title}`，可带缺省文字 `{baseline.text|（没有基准稿）}`。
static VARIABLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\{([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)?)(?:\|([^{}]*))?\}")
        .expect("变量正则")
});

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct Board {
    /// 要素。工具只读，流程不得回写（红线 2）。
    pub(crate) draft: DraftInput,
    /// 用户原话：材料与要求。
    pub(crate) request: String,
    /// 已确认的回答（动笔前澄清等）。
    pub(crate) notes: Vec<String>,
    /// 发起时的正文快照，只读。
    pub(crate) document: String,
    /// 用户锁定的选区原文。
    pub(crate) selection: Option<String>,
    /// 润色预设的指令文字。
    pub(crate) preset: String,
    /// AI 工作稿：流程唯一能写的稿子。
    pub(crate) workspace: String,
    pub(crate) evidence: EvidencePack,
    pub(crate) ledger: Ledger,
    /// `save_as` 存下的结果。
    pub(crate) vars: BTreeMap<String, Value>,
    /// 交付时要问用户的题。
    pub(crate) questions: Vec<Question>,
    /// 已经问过动笔前澄清（方向题与六要素题）。只对 `premise` 那个文种成立。
    pub(crate) clarified: bool,
    /// 定文种这一关的结论：按这个文种写（起草人在定文种题里确认的，或程序核对要求与当前
    /// 文种无出入）。动笔前的其余题、技能的选择都以它为前提；当前文种与它不符（要素区里
    /// 改过文种）时，以旧文种为前提的澄清作废重问。`None` 表示这一关还没过。
    pub(crate) premise: Option<TemplateKind>,
    /// 起草的系统提示（含日期规则）。
    pub(crate) system_prompt: String,
    /// 今天、明天这些日期，算作合法出处。
    pub(crate) time_sources: String,
    /// 模型输出撞上过上限。
    pub(crate) truncated: bool,
    /// 缺口循环跑了几轮。
    pub(crate) rounds: usize,
    /// 审核类技能的问题清单。
    pub(crate) findings: Vec<Finding>,
    /// 用户在输入框里 `@` 引用的文章。引擎在第一步之前按技能的 `references` 处理（16.13）。
    pub(crate) refs: Vec<Reference>,
    /// 当证据用的引用在证据包里的编号：逐节检索时也一直带着，不被检索结果冲掉。
    pub(crate) pinned: Vec<usize>,
    /// 本会话之前的往来（会话摘要 + 最近几轮），追问时理解「再短一点」指的是什么（16.15 B.7）。
    /// 起草模型的系统提示自动带上；技能提示词里也可以写 `{history}`。
    pub(crate) history: String,
    /// 这一轮用的写法风格（已按预算排好的文字，16.15 C.3）。起草模型的系统提示自动带上；
    /// 技能提示词里也可以写 `{style}`。接着跑时沿用，不再重挑。
    pub(crate) style: String,
}

/// `@` 引用的一篇文章：稿件库或知识库里的文档。正文由引擎在后台线程读。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Reference {
    pub(crate) source: RefSource,
    pub(crate) id: i64,
    pub(crate) title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum RefSource {
    Manuscript,
    Knowledge,
}

impl RefSource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            RefSource::Manuscript => "稿件库",
            RefSource::Knowledge => "知识库",
        }
    }
}

/// 审核类技能查出的一条问题。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Finding {
    /// 分组：要素与格式、表述、结构、有出处 / 无出处 / 与资料矛盾、要点……
    pub(crate) group: String,
    /// 问题说明。
    pub(crate) text: String,
    /// 原文片段或所在句。
    pub(crate) excerpt: String,
    /// 依据或出处（规则名、[K3]《题名》……）。
    pub(crate) source: String,
    /// 有明确改法时：对发起时正文的一处替换，转成审校抽屉里的修订建议逐条采纳。
    pub(crate) fix: Option<Fix>,
}

/// 对正文的一处替换。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Fix {
    pub(crate) span: std::ops::Range<usize>,
    pub(crate) before: String,
    pub(crate) after: String,
}

impl Board {
    /// 内置变量与 `save_as` 变量的文字形式。
    pub(crate) fn var_text(&self, name: &str) -> Option<String> {
        let (head, field) = match name.split_once('.') {
            Some((head, field)) => (head, Some(field)),
            None => (name, None),
        };
        let builtin = match head {
            "request" => Some(self.request.clone()),
            "notes" => Some(self.notes.join("\n")),
            "kind" => Some(self.draft.kind.label().to_string()),
            "title" => Some(self.draft.title_hint.clone()),
            "document" => Some(self.document.clone()),
            "selection" => Some(self.selection.clone().unwrap_or_default()),
            "workspace" => Some(self.workspace.clone()),
            "preset" => Some(self.preset.clone()),
            "history" => Some(self.history.clone()),
            "style" => Some(self.style.clone()),
            _ => None,
        };
        if field.is_none()
            && let Some(text) = builtin
        {
            return Some(text);
        }
        let value = self.vars.get(head)?;
        match field {
            Some(field) => value.get(field).map(value_to_text),
            None => Some(value_to_text(value)),
        }
    }

    /// 替换模板里的 `{变量}`。只扫一遍：替换进去的文字里即使带花括号也不会再被替换；
    /// 不认识的变量原样保留。
    pub(crate) fn render(&self, template: &str) -> String {
        self.render_with(template, &[])
    }

    /// 同 [`Board::render`]，另带几个只在这一步有效的变量（证据、缺口所在句……），
    /// 同名时它们优先。
    pub(crate) fn render_with(&self, template: &str, locals: &[(&str, String)]) -> String {
        VARIABLE
            .replace_all(template, |caps: &regex::Captures<'_>| {
                let name = &caps[1];
                locals
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.clone())
                    .or_else(|| self.var_text(name))
                    // 写了缺省文字时，变量为空也用缺省（如还没填标题提示）。
                    .filter(|text| caps.get(2).is_none() || !text.trim().is_empty())
                    .or_else(|| caps.get(2).map(|default| default.as_str().to_string()))
                    .unwrap_or_else(|| caps[0].to_string())
            })
            .into_owned()
    }

    /// 把工具参数里的字符串逐个渲染。整串只有一个变量时保留变量原来的类型
    /// （例如 `"{baseline_id}"` 渲染成数字），方便直接当工具输入。
    pub(crate) fn render_value(&self, value: &Value) -> Value {
        self.render_value_with(value, &[])
    }

    /// 同 [`Board::render_value`]，另带本步变量（如检索词 `{query}`），同名时它们优先。
    pub(crate) fn render_value_with(&self, value: &Value, locals: &[(&str, String)]) -> Value {
        match value {
            Value::String(text) => {
                if let Some(caps) = VARIABLE.captures(text)
                    && caps[0].len() == text.len()
                {
                    let name = &caps[1];
                    if let Some((_, local)) = locals.iter().find(|(key, _)| *key == name) {
                        return Value::String(local.clone());
                    }
                    let (head, field) = match name.split_once('.') {
                        Some((head, field)) => (head, Some(field)),
                        None => (name, None),
                    };
                    if let Some(raw) = self.vars.get(head) {
                        let picked = match field {
                            Some(field) => raw.get(field).cloned(),
                            None => Some(raw.clone()),
                        };
                        if let Some(picked) = picked {
                            return picked;
                        }
                    }
                }
                Value::String(self.render_with(text, locals))
            }
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|v| self.render_value_with(v, locals))
                    .collect(),
            ),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, v)| (key.clone(), self.render_value_with(v, locals)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// 算作出处的全部文字：材料、已确认的回答、日期、要素、证据包。
    pub(crate) fn sources_text(&self) -> String {
        let mut text = String::new();
        text.push_str(&self.request);
        text.push('\n');
        text.push_str(&self.notes.join("\n"));
        text.push('\n');
        text.push_str(&self.time_sources);
        text.push('\n');
        text.push_str(&serde_json::to_string(&self.draft).unwrap_or_default());
        text.push('\n');
        text.push_str(&self.evidence.text_of(&self.evidence.all_ids()));
        text
    }

    /// 恢复前按界面当前值重灌只读区（红线 2：要素与正文快照以用户手上的为准，
    /// 系统提示与合法日期按现在重建——隔天恢复，「今天」得是今天）。
    ///
    /// `request` / `selection` / `preset` / `refs` / `history` / `style` **不**重灌：它们是
    /// 这一轮任务本身的输入（发起时说的话、锁的选区、当时挑的风格），换成界面当前值就成了
    /// 另一个任务（`docs/agent-kernel-hardening.md` 第 4 期与方案的出入 1）。
    ///
    /// 两条恢复路径（答完题接着跑、崩溃 / 停止 / 出错后接着跑）都调它，检查点里存的
    /// 这几个字段一律被覆盖。
    pub(crate) fn refresh_from_ui(
        &mut self,
        draft: &DraftInput,
        document: &str,
        time: &crate::prompt::TimeContext,
    ) {
        self.draft = draft.clone();
        self.document = document.to_string();
        self.system_prompt = crate::prompt::build_system_prompt(time);
        self.time_sources = [
            &time.now,
            &time.yesterday,
            &time.today,
            &time.tomorrow,
            &time.day_after_tomorrow,
            &time.three_days_later,
            &time.this_week,
            &time.next_week,
        ]
        .map(String::as_str)
        .join("\n");
    }

    /// 用户原话加上已确认的回答，给提示词用。
    pub(crate) fn request_with_notes(&self) -> String {
        let mut text = self.request.trim().to_string();
        if !self.notes.is_empty() {
            text.push_str("\n\n已确认：");
            for note in &self.notes {
                text.push_str("\n- ");
                text.push_str(note);
            }
        }
        text
    }
}

/// 结构化结果 → 给模型看的文字：字符串原样；列表逐条编号（对象取标题与正文）；对象按「键：值」。
pub(crate) fn value_to_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Bool(flag) => if *flag { "是" } else { "否" }.into(),
        Value::Number(number) => number.to_string(),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(index, item)| format!("{}. {}", index + 1, item_line(item)))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(map) => map
            .iter()
            .map(|(key, v)| format!("{key}：{}", value_to_text(v)))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// 列表里的一项写成一行：对象取标题与说明，其余照 [`value_to_text`]。
pub(crate) fn item_line(item: &Value) -> String {
    let Value::Object(map) = item else {
        return value_to_text(item);
    };
    let pick = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| map.get(*key).and_then(Value::as_str))
            .map(str::to_string)
    };
    match (
        pick(&["title", "name", "canonical"]),
        pick(&["text", "summary", "note"]),
    ) {
        (Some(title), Some(text)) => format!("{title}：{text}"),
        (Some(title), None) => title,
        (None, Some(text)) => text,
        (None, None) => value_to_text(item).replace('\n', "；"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn board() -> Board {
        let mut board = Board {
            request: "写个{kind}通知".into(),
            ..Board::default()
        };
        board.draft.title_hint = "冬季防火".into();
        board.vars.insert("baseline_id".into(), json!(42));
        board
            .vars
            .insert("baseline".into(), json!({"title": "去年的通知", "id": 42}));
        board.vars.insert(
            "hits".into(),
            json!([{"title": "条例", "text": "第十条"}, {"name": "办法"}, "第三条"]),
        );
        board
    }

    #[test]
    fn templates_replace_builtin_and_saved_variables_once() {
        let board = board();
        assert_eq!(
            board.render("{title}：{request}，参考{baseline.title}，{unknown}"),
            "冬季防火：写个{kind}通知，参考去年的通知，{unknown}",
            "插进去的 {{kind}} 不再二次替换，不认识的原样保留"
        );
        assert_eq!(
            board.render_with(
                "{title}·{evidence}",
                &[("title", "本步".into()), ("evidence", "{title}".into())]
            ),
            "本步·{title}",
            "本步变量优先，替换进去的花括号不再替换"
        );
        assert_eq!(
            board.render("{missing|（无）}·{title|没有标题}·{notes|无回答}"),
            "（无）·冬季防火·无回答",
            "缺失或为空时用缺省文字"
        );
        assert_eq!(
            board.render("{hits}"),
            "1. 条例：第十条\n2. 办法\n3. 第三条"
        );
    }

    #[test]
    fn a_lone_variable_keeps_its_type_in_tool_arguments() {
        let board = board();
        let args =
            json!({"id": "{baseline_id}", "keyword": "{title} 通知", "list": ["{baseline.id}"]});
        assert_eq!(
            board.render_value(&args),
            json!({"id": 42, "keyword": "冬季防火 通知", "list": [42]})
        );
    }

    #[test]
    fn notes_are_appended_for_prompts() {
        let mut board = board();
        board.notes = vec!["受文对象：各区县".into()];
        assert!(
            board
                .request_with_notes()
                .ends_with("已确认：\n- 受文对象：各区县")
        );
        assert_eq!(board.var_text("notes").unwrap(), "受文对象：各区县");
    }
}
