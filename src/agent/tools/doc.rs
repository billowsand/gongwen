//! A 组：当前稿件（只读）。读的是发起任务时的正文快照，不会改它。

use super::workspace::{headings, section_range};
use super::{Input, Permission, Tool, ToolCtx, ToolOutput, arg_str, optional};
use serde_json::{Map, Value, json};

pub(super) const TOOLS: [&dyn Tool; 5] = [&Read, &Outline, &Selection, &Elements, &Stats];

/// 估算页数用：公文版心每页约 22 字 × 22 行。
const CHARS_PER_PAGE: usize = 22 * 22;

struct Read;

impl Tool for Read {
    fn id(&self) -> &'static str {
        "doc.read"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "读当前正文全文，或只读某一节"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[optional("section", "只读这一节：写标题文字（不含编号）")];
        INPUTS
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let document = &ctx.board.document;
        let (text, label) = match arg_str(args, "section") {
            Some(section) => {
                let range = section_range(document, &section)
                    .ok_or_else(|| format!("正文里没有标题为「{section}」的一节"))?;
                (
                    document[range].trim().to_string(),
                    format!("「{section}」一节"),
                )
            }
            None => (document.clone(), "全文".to_string()),
        };
        let chars = text.chars().filter(|c| !c.is_whitespace()).count();
        Ok(ToolOutput::new(
            Value::String(text),
            format!("读取正文{label}（{chars} 字）"),
        ))
    }
}

struct Outline;

impl Tool for Outline {
    fn id(&self) -> &'static str {
        "doc.outline"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "当前正文的标题结构（层级与标题文字）"
    }
    fn inputs(&self) -> &'static [Input] {
        &[]
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        _args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let list: Vec<Value> = headings(&ctx.board.document)
            .into_iter()
            .map(|h| json!({"level": h.level, "title": h.title}))
            .collect();
        let count = list.len();
        Ok(ToolOutput::new(
            Value::Array(list),
            format!("读取标题结构（{count} 个标题）"),
        ))
    }
}

struct Selection;

impl Tool for Selection {
    fn id(&self) -> &'static str {
        "doc.selection"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "用户在编辑器里锁定的选区；没有选区时为空"
    }
    fn inputs(&self) -> &'static [Input] {
        &[]
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        _args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let text = ctx.board.selection.clone().unwrap_or_default();
        let summary = if text.is_empty() {
            "没有选区".to_string()
        } else {
            format!("读取选区（{} 字）", text.chars().count())
        };
        Ok(ToolOutput::new(Value::String(text), summary))
    }
}

struct Elements;

impl Tool for Elements {
    fn id(&self) -> &'static str {
        "doc.elements"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "读要素：文种、标题提示、主送、发文机关、成文日期等。只读，任何工具都不能改要素"
    }
    fn inputs(&self) -> &'static [Input] {
        &[]
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        _args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let draft = &ctx.board.draft;
        let mut value = serde_json::to_value(draft).map_err(|e| e.to_string())?;
        if let Value::Object(map) = &mut value {
            map.insert(
                "kind_label".into(),
                Value::String(draft.kind.label().into()),
            );
        }
        Ok(ToolOutput::new(
            value,
            format!("读取要素（{}）", draft.kind.label()),
        ))
    }
}

struct Stats;

impl Tool for Stats {
    fn id(&self) -> &'static str {
        "doc.stats"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "正文的字数、段落数与估算页数"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[optional("text", "统计这段文字；不给就统计正文")];
        INPUTS
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let text = arg_str(args, "text").unwrap_or_else(|| ctx.board.document.clone());
        let chars = text.chars().filter(|c| !c.is_whitespace()).count();
        let paragraphs = text
            .split('\n')
            .filter(|line| {
                let line = line.trim();
                !line.is_empty() && !line.starts_with('#')
            })
            .count();
        let pages = chars.div_ceil(CHARS_PER_PAGE).max(usize::from(chars > 0));
        Ok(ToolOutput::new(
            json!({"chars": chars, "paragraphs": paragraphs, "pages_estimated": pages}),
            format!("统计：{chars} 字，{paragraphs} 段，约 {pages} 页"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::Fixture;
    use serde_json::json;

    const DOC: &str = "# 关于做好冬季森林防火工作的通知\n\n各区县：\n\n## 总体要求\n\n加强巡查。\n\n## 工作安排\n\n### 隐患排查\n\n12月1日前完成。\n";

    #[test]
    fn reading_the_whole_document_or_one_section() {
        let mut fixture = Fixture::new(DOC);
        let all = fixture.call("doc.read", json!({})).unwrap();
        assert_eq!(all.value, DOC);
        let section = fixture
            .call("doc.read", json!({"section": "工作安排"}))
            .unwrap();
        assert_eq!(section.value, "### 隐患排查\n\n12月1日前完成。");
        assert!(section.summary.contains("「工作安排」一节"));
        assert!(
            fixture
                .call("doc.read", json!({"section": "不存在"}))
                .unwrap_err()
                .contains("没有标题")
        );
    }

    #[test]
    fn outline_selection_elements_and_stats() {
        let mut fixture = Fixture::new(DOC);
        let outline = fixture.call("doc.outline", json!({})).unwrap();
        assert_eq!(outline.value[1], json!({"level": 2, "title": "总体要求"}));
        assert_eq!(outline.value.as_array().unwrap().len(), 4);

        assert_eq!(fixture.call("doc.selection", json!({})).unwrap().value, "");
        fixture.board.selection = Some("加强巡查。".into());
        assert_eq!(
            fixture.call("doc.selection", json!({})).unwrap().value,
            "加强巡查。"
        );

        fixture.board.draft.title_hint = "冬季防火".into();
        let elements = fixture.call("doc.elements", json!({})).unwrap();
        assert_eq!(elements.value["title_hint"], "冬季防火");
        assert!(elements.value["kind_label"].is_string());

        let stats = fixture.call("doc.stats", json!({})).unwrap();
        assert_eq!(stats.value["paragraphs"], 3);
        assert_eq!(stats.value["pages_estimated"], 1);
    }
}
