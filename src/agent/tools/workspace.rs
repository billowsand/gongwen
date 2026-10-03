//! B 组：工作稿。流程唯一能写的稿子；变成提案、用户接受后才进正文（红线 1）。
//!
//! 任务开始时工作稿是正文的副本：改写类技能直接在上面替换、插入；起草类技能整篇写入。

use super::{
    Input, Permission, Tool, ToolCtx, ToolOutput, arg_bool, arg_str, arg_usize, optional, required,
    short,
};
use regex::{Regex, RegexBuilder};
use serde_json::{Map, Value, json};
use std::ops::Range;
use std::sync::LazyLock;

pub(super) const TOOLS: [&dyn Tool; 6] = [&Read, &Write, &Replace, &Insert, &Section, &Diff];

/// 正则长度上限。再长的规则多半是模型在乱写。
const MAX_PATTERN_CHARS: usize = 500;
/// 默认最多替换几处。
const DEFAULT_MAX_REPLACEMENTS: usize = 50;

static BOOK_TITLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"《[^》\n]{1,80}》").expect("书名号正则"));

/// 一个标题及其所辖范围。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Heading {
    pub(crate) level: usize,
    pub(crate) title: String,
    /// 标题行起点。
    pub(crate) line_start: usize,
    /// 本节正文起点（标题行之后）。
    pub(crate) body_start: usize,
    /// 本节终点：下一个同级或更高级标题之前。
    pub(crate) end: usize,
}

/// Markdown 里的 ATX 标题（`#` 到 `######`），跳过代码块。
pub(crate) fn headings(markdown: &str) -> Vec<Heading> {
    let mut found: Vec<Heading> = Vec::new();
    let mut offset = 0usize;
    let mut fenced = false;
    for line in markdown.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let trimmed = line.trim_end();
        if trimmed.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let level = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&level) && trimmed[level..].starts_with(' ') {
            found.push(Heading {
                level,
                title: trimmed[level..]
                    .trim()
                    .trim_end_matches('#')
                    .trim()
                    .to_string(),
                line_start: start,
                body_start: offset,
                end: markdown.len(),
            });
        }
    }
    for index in 0..found.len() {
        let level = found[index].level;
        if let Some(next) = found[index + 1..].iter().find(|h| h.level <= level) {
            found[index].end = next.line_start;
        }
    }
    found
}

/// 标题为 `title` 的那一节正文的范围：先找完全相同的，没有再找包含它的。
pub(crate) fn section_range(markdown: &str, title: &str) -> Option<Range<usize>> {
    let title = title.trim();
    let all = headings(markdown);
    all.iter()
        .find(|h| h.title == title)
        .or_else(|| all.iter().find(|h| h.title.contains(title)))
        .map(|h| h.body_start..h.end)
}

struct Read;

impl Tool for Read {
    fn id(&self) -> &'static str {
        "ws.read"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "读 AI 工作稿"
    }
    fn inputs(&self) -> &'static [Input] {
        &[]
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        _args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let text = ctx.board.workspace.clone();
        let chars = text.chars().filter(|c| !c.is_whitespace()).count();
        Ok(ToolOutput::new(
            Value::String(text),
            format!("读取工作稿（{chars} 字）"),
        ))
    }
}

struct Write;

impl Tool for Write {
    fn id(&self) -> &'static str {
        "ws.write"
    }
    fn permission(&self) -> Permission {
        Permission::WriteWorkspace
    }
    fn description(&self) -> &'static str {
        "整篇写入工作稿（覆盖原有内容）"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[required("text", "要写入的 Markdown 全文")];
        INPUTS
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let text = arg_str(args, "text").unwrap_or_default();
        let chars = text.chars().filter(|c| !c.is_whitespace()).count();
        ctx.board.workspace = text;
        (ctx.emit)(super::super::engine::Event::Workspace(
            ctx.board.workspace.clone(),
        ));
        Ok(ToolOutput::new(
            json!({"chars": chars}),
            format!("写入工作稿（{chars} 字）"),
        ))
    }
}

struct Replace;

impl Tool for Replace {
    fn id(&self) -> &'static str {
        "ws.replace"
    }
    fn permission(&self) -> Permission {
        Permission::WriteWorkspace
    }
    fn description(&self) -> &'static str {
        "在工作稿里做字面或正则替换，可限定范围、处数；预期处数对不上就不改"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[
            required("pattern", "要找的文字；regex 为真时是正则表达式"),
            optional(
                "replacement",
                "替换成什么；正则时可用 $1 引用分组；不给表示删除",
            ),
            optional("regex", "是否按正则匹配，默认否"),
            optional("scope", "范围：all（默认）/ section / selection"),
            optional("heading", "scope 为 section 时，那一节的标题"),
            optional("max", "最多替换几处，默认 50"),
            optional("expect", "预期有几处；对不上就一处都不改"),
        ];
        INPUTS
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let pattern = arg_str(args, "pattern").unwrap_or_default();
        let replacement = arg_str(args, "replacement").unwrap_or_default();
        let use_regex = arg_bool(args, "regex").unwrap_or(false);
        if pattern.chars().count() > MAX_PATTERN_CHARS {
            return Err(format!("替换规则超过 {MAX_PATTERN_CHARS} 字"));
        }
        let regex = if use_regex {
            RegexBuilder::new(&pattern)
                .size_limit(1 << 20)
                .build()
                .map_err(|e| format!("正则表达式有误：{e}"))?
        } else {
            Regex::new(&regex::escape(&pattern)).expect("转义后的字面一定合法")
        };
        let text = ctx.board.workspace.clone();
        let region = match arg_str(args, "scope").as_deref().unwrap_or("all") {
            "all" => 0..text.len(),
            "section" => {
                let heading = arg_str(args, "heading").ok_or("scope 为 section 时要给 heading")?;
                section_range(&text, &heading)
                    .ok_or_else(|| format!("工作稿里没有标题为「{heading}」的一节"))?
            }
            "selection" => {
                let selection = ctx
                    .board
                    .selection
                    .clone()
                    .filter(|s| !s.is_empty())
                    .ok_or("没有选区")?;
                let start = text.find(&selection).ok_or("选区在工作稿里找不到了")?;
                start..start + selection.len()
            }
            other => return Err(format!("不认识的范围「{other}」")),
        };
        let protected = protected_ranges(&text, &pattern);
        let matches: Vec<regex::Captures<'_>> = regex
            .captures_iter(&text)
            .filter(|caps| {
                let m = caps.get(0).expect("整段匹配");
                m.start() >= region.start
                    && m.end() <= region.end
                    && !m.is_empty()
                    && !protected
                        .iter()
                        .any(|p| m.start() < p.end && p.start < m.end())
            })
            .collect();
        let found = matches.len();
        if let Some(expect) = arg_usize(args, "expect")
            && expect != found
        {
            return Err(format!("预期 {expect} 处，实际找到 {found} 处，一处都没改"));
        }
        let max = arg_usize(args, "max").unwrap_or(DEFAULT_MAX_REPLACEMENTS);
        let mut out = String::with_capacity(text.len());
        let mut last = 0usize;
        let mut samples = Vec::new();
        for caps in matches.iter().take(max) {
            let m = caps.get(0).expect("整段匹配");
            let mut piece = String::new();
            if use_regex {
                caps.expand(&replacement, &mut piece);
            } else {
                piece.clone_from(&replacement);
            }
            out.push_str(&text[last..m.start()]);
            out.push_str(&piece);
            last = m.end();
            if samples.len() < 3 {
                samples.push(json!({"before": m.as_str(), "after": piece}));
            }
        }
        out.push_str(&text[last..]);
        let replaced = found.min(max);
        if replaced > 0 {
            ctx.board.workspace = out;
            (ctx.emit)(super::super::engine::Event::Workspace(
                ctx.board.workspace.clone(),
            ));
        }
        let skipped = found - replaced;
        let mut summary = format!("替换「{}」{replaced} 处", short(&pattern, 16));
        if skipped > 0 {
            summary.push_str(&format!("（超出上限，另有 {skipped} 处未改）"));
        }
        Ok(ToolOutput::new(
            json!({"replaced": replaced, "found": found, "samples": samples}),
            summary,
        ))
    }
}

/// 默认受保护的范围：「【待核实】」占位与书名号。规则本身写到了它们才放开。
fn protected_ranges(text: &str, pattern: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    if !pattern.contains("【待核实") {
        ranges.extend(
            crate::agent::gaps::find_placeholders(text)
                .into_iter()
                .map(|p| p.span),
        );
    }
    if !pattern.contains('《') && !pattern.contains('》') {
        ranges.extend(BOOK_TITLE.find_iter(text).map(|m| m.range()));
    }
    ranges
}

struct Insert;

impl Tool for Insert {
    fn id(&self) -> &'static str {
        "ws.insert"
    }
    fn permission(&self) -> Permission {
        Permission::WriteWorkspace
    }
    fn description(&self) -> &'static str {
        "在工作稿的锚点处插入一段：文首、文末、某一节末尾、某段文字之前或之后"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[
            required("at", "位置：start / end / section_end / before / after"),
            optional(
                "anchor",
                "section_end 时是标题；before、after 时是要找的文字",
            ),
            required("text", "要插入的内容"),
        ];
        INPUTS
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let at = arg_str(args, "at").unwrap_or_default();
        let piece = arg_str(args, "text").unwrap_or_default();
        let text = ctx.board.workspace.clone();
        let anchor = || arg_str(args, "anchor").ok_or_else(|| format!("位置 {at} 需要 anchor"));
        let (position, block) = match at.as_str() {
            "start" => (0, true),
            "end" => (text.len(), true),
            "section_end" => {
                let heading = anchor()?;
                let range = section_range(&text, &heading)
                    .ok_or_else(|| format!("工作稿里没有标题为「{heading}」的一节"))?;
                (range.end, true)
            }
            "before" | "after" => {
                let needle = anchor()?;
                let start = text
                    .find(&needle)
                    .ok_or_else(|| format!("工作稿里找不到「{}」", short(&needle, 16)))?;
                (
                    if at == "before" {
                        start
                    } else {
                        start + needle.len()
                    },
                    false,
                )
            }
            other => return Err(format!("不认识的位置「{other}」")),
        };
        let inserted = if block {
            // 整段插入：前后各留一个空行，不和相邻段落粘在一起。
            let before = text[..position].trim_end_matches('\n');
            let after = text[position..].trim_start_matches('\n');
            let mut out = String::new();
            out.push_str(before);
            if !before.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(piece.trim_matches('\n'));
            out.push_str("\n\n");
            out.push_str(after);
            if after.is_empty() {
                out.truncate(out.trim_end_matches('\n').len());
                out.push('\n');
            }
            out
        } else {
            format!("{}{}{}", &text[..position], piece, &text[position..])
        };
        ctx.board.workspace = inserted;
        (ctx.emit)(super::super::engine::Event::Workspace(
            ctx.board.workspace.clone(),
        ));
        Ok(ToolOutput::new(
            json!({"inserted_chars": piece.chars().count()}),
            format!(
                "在工作稿{}插入 {} 字",
                position_label(&at),
                piece.chars().count()
            ),
        ))
    }
}

fn position_label(at: &str) -> &'static str {
    match at {
        "start" => "文首",
        "end" => "文末",
        "section_end" => "节末",
        "before" => "锚点前",
        _ => "锚点后",
    }
}

struct Section;

impl Tool for Section {
    fn id(&self) -> &'static str {
        "ws.section"
    }
    fn permission(&self) -> Permission {
        Permission::WriteWorkspace
    }
    fn description(&self) -> &'static str {
        "按标题替换工作稿里整节的内容（标题保留）"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[
            required("heading", "那一节的标题（不含编号）"),
            required("text", "这一节的新内容"),
        ];
        INPUTS
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let heading = arg_str(args, "heading").unwrap_or_default();
        let piece = arg_str(args, "text").unwrap_or_default();
        let text = ctx.board.workspace.clone();
        let range = section_range(&text, &heading)
            .ok_or_else(|| format!("工作稿里没有标题为「{heading}」的一节"))?;
        let tail = &text[range.end..];
        let mut out = String::new();
        out.push_str(&text[..range.start]);
        out.push('\n');
        out.push_str(piece.trim_matches('\n'));
        out.push_str(if tail.is_empty() { "\n" } else { "\n\n" });
        out.push_str(tail);
        ctx.board.workspace = out;
        (ctx.emit)(super::super::engine::Event::Workspace(
            ctx.board.workspace.clone(),
        ));
        Ok(ToolOutput::new(
            json!({"chars": piece.chars().count()}),
            format!("改写「{heading}」一节（{} 字）", piece.chars().count()),
        ))
    }
}

struct Diff;

impl Tool for Diff {
    fn id(&self) -> &'static str {
        "ws.diff"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "工作稿与正文逐句比较，列出删掉的与新增的句子"
    }
    fn inputs(&self) -> &'static [Input] {
        &[]
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        _args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let (removed, added) = sentence_changes(&ctx.board.document, &ctx.board.workspace);
        let summary = format!(
            "工作稿比正文删去 {} 句、新增 {} 句",
            removed.len(),
            added.len()
        );
        Ok(ToolOutput::new(
            json!({"removed": removed, "added": added}),
            summary,
        ))
    }
}

/// 逐句比较：(删掉的句子, 新增的句子)，按最长公共子序列对齐，顺序保持原文先后。
pub(crate) fn sentence_changes(before: &str, after: &str) -> (Vec<String>, Vec<String>) {
    let split = |text: &str| -> Vec<String> {
        crate::agent::gaps::sentence_spans(text)
            .into_iter()
            .map(|span| text[span].to_string())
            .collect()
    };
    let (a, b) = (split(before), split(after));
    // 句子太多时退化成集合差，避免平方级开销。
    if a.len() * b.len() > 4_000_000 {
        let removed = a.iter().filter(|s| !b.contains(s)).cloned().collect();
        let added = b.iter().filter(|s| !a.contains(s)).cloned().collect();
        return (removed, added);
    }
    let mut lcs = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let (mut removed, mut added) = (Vec::new(), Vec::new());
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            removed.push(a[i].clone());
            i += 1;
        } else {
            added.push(b[j].clone());
            j += 1;
        }
    }
    removed.extend(a[i..].iter().cloned());
    added.extend(b[j..].iter().cloned());
    (removed, added)
}

#[cfg(test)]
mod tests {
    use super::super::testing::Fixture;
    use super::*;

    const DOC: &str = "# 通知\n\n## 总体要求\n\n各单位要加强巡查。各单位要落实责任。\n\n## 工作安排\n\n依据《森林防火条例》，于【待核实：排查完成时限】前完成。\n";

    #[test]
    fn headings_know_where_each_section_ends() {
        let all = headings(DOC);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].end, DOC.len(), "一级标题管到文末");
        assert_eq!(
            &DOC[all[1].body_start..all[1].end],
            "\n各单位要加强巡查。各单位要落实责任。\n\n"
        );
        let fenced = "```\n# 不是标题\n```\n## 是标题\n";
        assert_eq!(headings(fenced).len(), 1);
    }

    #[test]
    fn literal_replacement_respects_scope_max_and_expect() {
        let mut fixture = Fixture::new(DOC);
        let out = fixture
            .call(
                "ws.replace",
                json!({"pattern": "各单位", "replacement": "各区县", "max": 1}),
            )
            .unwrap();
        assert_eq!(out.value["replaced"], 1);
        assert!(out.summary.contains("另有 1 处未改"));
        assert!(
            fixture
                .board
                .workspace
                .contains("各区县要加强巡查。各单位要落实责任。")
        );
        // 预期对不上：一处都不改。
        let before = fixture.board.workspace.clone();
        let error = fixture
            .call(
                "ws.replace",
                json!({"pattern": "各单位", "replacement": "甲", "expect": 3}),
            )
            .unwrap_err();
        assert!(error.contains("预期 3 处，实际找到 1 处"));
        assert_eq!(fixture.board.workspace, before);
        // 限定在一节里。
        let out = fixture
            .call("ws.replace", json!({"pattern": "完成", "replacement": "办结", "scope": "section", "heading": "总体要求"}))
            .unwrap();
        assert_eq!(out.value["replaced"], 0);
    }

    #[test]
    fn regex_replacement_supports_groups_and_protects_placeholders_and_titles() {
        let mut fixture = Fixture::new(DOC);
        let out = fixture
            .call(
                "ws.replace",
                json!({"pattern": "要(加强|落实)", "replacement": "务必$1", "regex": true}),
            )
            .unwrap();
        assert_eq!(out.value["replaced"], 2);
        assert!(
            fixture
                .board
                .workspace
                .contains("各单位务必加强巡查。各单位务必落实责任。")
        );
        // 「条例」「完成时限」分别在书名号与占位里，默认不动。
        let out = fixture
            .call(
                "ws.replace",
                json!({"pattern": "条例|时限", "replacement": "X", "regex": true}),
            )
            .unwrap();
        assert_eq!(out.value["replaced"], 0);
        // 规则明写到占位，才放开。
        let out = fixture
            .call(
                "ws.replace",
                json!({"pattern": "【待核实：排查完成时限】", "replacement": "12月1日"}),
            )
            .unwrap();
        assert_eq!(out.value["replaced"], 1);
        assert!(
            fixture
                .call("ws.replace", json!({"pattern": "(", "regex": true}))
                .unwrap_err()
                .contains("正则表达式有误")
        );
    }

    #[test]
    fn insert_section_write_and_diff() {
        let mut fixture = Fixture::new(DOC);
        fixture
            .call(
                "ws.insert",
                json!({"at": "section_end", "anchor": "总体要求", "text": "做好值班值守。"}),
            )
            .unwrap();
        assert!(
            fixture
                .board
                .workspace
                .contains("落实责任。\n\n做好值班值守。\n\n## 工作安排")
        );
        fixture
            .call("ws.insert", json!({"at": "end", "text": "特此通知。"}))
            .unwrap();
        assert!(
            fixture
                .board
                .workspace
                .ends_with("前完成。\n\n特此通知。\n")
        );
        fixture
            .call(
                "ws.insert",
                json!({"at": "after", "anchor": "加强巡查", "text": "和宣传"}),
            )
            .unwrap();
        assert!(fixture.board.workspace.contains("加强巡查和宣传。"));

        fixture
            .call(
                "ws.section",
                json!({"heading": "工作安排", "text": "全面排查隐患。"}),
            )
            .unwrap();
        assert!(
            fixture
                .board
                .workspace
                .contains("## 工作安排\n\n全面排查隐患。\n")
        );

        let diff = fixture.call("ws.diff", json!({})).unwrap();
        assert!(
            diff.value["added"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s == "全面排查隐患。")
        );
        assert!(!diff.value["removed"].as_array().unwrap().is_empty());

        fixture
            .call("ws.write", json!({"text": "# 新稿\n"}))
            .unwrap();
        assert_eq!(fixture.board.workspace, "# 新稿\n");
        assert_eq!(
            fixture.call("ws.read", json!({})).unwrap().value,
            "# 新稿\n"
        );
    }

    #[test]
    fn sentence_changes_align_by_common_subsequence() {
        let (removed, added) = sentence_changes("甲。乙。丙。", "甲。丁。丙。");
        assert_eq!(removed, ["乙。"]);
        assert_eq!(added, ["丁。"]);
    }
}
