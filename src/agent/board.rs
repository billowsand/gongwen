//! 黑板：一次技能运行里，所有步骤共享的数据。
//!
//! 算子与工具都只通过黑板交换数据：用户原话、已确认的回答、要素（只读）、正文快照（只读）、
//! 选区、工作稿、证据包、缺口台账、变量、待问的题。黑板可以整块交回界面线程保存，挂起后
//! 原样带回来接着跑。

use super::clarify::Question;
use super::evidence::EvidencePack;
use super::gaps::Ledger;
use crate::models::DraftInput;
use regex::Regex;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::LazyLock;

/// 模板变量：`{request}`、`{baseline.title}`。
static VARIABLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\{([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)?)\}").expect("变量正则")
});

#[derive(Debug, Clone, Default)]
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
    /// 已经问过动笔前澄清。
    pub(crate) clarified: bool,
    /// 起草的系统提示（含日期规则）。
    pub(crate) system_prompt: String,
    /// 今天、明天这些日期，算作合法出处。
    pub(crate) time_sources: String,
    /// 模型输出撞上过上限。
    pub(crate) truncated: bool,
    /// 缺口循环跑了几轮。
    pub(crate) rounds: usize,
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
                    .unwrap_or_else(|| caps[0].to_string())
            })
            .into_owned()
    }

    /// 把工具参数里的字符串逐个渲染。整串只有一个变量时保留变量原来的类型
    /// （例如 `"{baseline_id}"` 渲染成数字），方便直接当工具输入。
    pub(crate) fn render_value(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => {
                if let Some(caps) = VARIABLE.captures(text)
                    && caps[0].len() == text.len()
                {
                    let name = &caps[1];
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
                Value::String(self.render(text))
            }
            Value::Array(items) => {
                Value::Array(items.iter().map(|v| self.render_value(v)).collect())
            }
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, v)| (key.clone(), self.render_value(v)))
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

fn item_line(item: &Value) -> String {
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
