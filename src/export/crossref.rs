//! 研究报告的行内扩展标记：锚点 `{#id}`、交叉引用 `{@id}` 与文献引用 `[@key]`。
//!
//! 三样都来自 mdx research。编译成 PDF 之后它们分别是 `\label`、`\ref` 和
//! `\cite`：锚点自己不占版面，`{@id}` 印的是被引对象的编号，`[@key]` 印的是
//! 方括号文献序号。纸上从来看不到大括号和 at 号，预览要照纸面显示，就得在排版
//! 之前把它们换掉。
//!
//! 认的写法必须与 mdx 的 `common::inline` / `common::parser` 完全一致——两边认
//! 的不是同一套，用户就会撞上"预览换了、编译不认"（或者反过来）这种最难查的
//! 毛病。`preview::research` 里有一条契约测试真跑一遍 mdx 转换，拿生成的 TeX
//! 核对这里的假设。

use regex::{Captures, Regex};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;

/// 交叉引用 `{@id}`。
fn crossref_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{@([A-Za-z][\w:.-]*)\}").expect("交叉引用正则"))
}

/// 文献引用 `[@key]`、`[@a; @b]`。
fn citation_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\[(@[^\s@;,\[\]{}\\]+(?:\s*;\s*@[^\s@;,\[\]{}\\]+)*)\]").expect("文献引用正则")
    })
}

/// 行尾锚点 `{#id}`。
fn label_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\s*\{#([A-Za-z][\w:.-]*)\}\s*$").expect("锚点正则"))
}

/// 剥掉行尾的锚点，返回（剩下的文字，锚点 id）。
///
/// 整行只有一个锚点时原样返回：剥空了这一行就从块序列里消失，而预览要靠"两遍
/// 解析切出同一串块"才能把第一遍算好的编号对到第二遍的块上。mdx 那边这种写法
/// 本来也不生效（锚点必须挂在标题、表题或图片上），留着不影响。
pub(crate) fn split_label(text: &str) -> (&str, Option<&str>) {
    let Some(caps) = label_re().captures(text) else {
        return (text, None);
    };
    let whole = caps.get(0).expect("锚点整体匹配");
    let rest = text[..whole.start()].trim_end();
    if rest.is_empty() {
        return (text, None);
    }
    (rest, caps.get(1).map(|id| id.as_str()))
}

/// 一行里出现的文献引用键，按出现顺序；`[@a; @b]` 拆成两条。
pub(crate) fn citation_keys(text: &str) -> Vec<&str> {
    citation_re()
        .captures_iter(text)
        .filter_map(|caps| caps.get(1))
        .flat_map(|group| split_keys(group.as_str()))
        .collect()
}

/// 大纲标题只保留可读文字，不显示研究报告 Markdown 中供转换器使用的标识符。
///
/// 行尾锚点本来就不占纸面；标题里的交叉引用和文献引用需要全文编号表才能转换，
/// 大纲不承担正文引用展示，因此直接省略，避免把内部 id / BibTeX key 暴露给用户。
pub(crate) fn strip_heading_identifiers(text: &str) -> Cow<'_, str> {
    let (stripped, label) = split_label(text);
    if label.is_none() && !stripped.contains("{@") && !stripped.contains("[@") {
        return Cow::Borrowed(text);
    }
    let text = crossref_re().replace_all(stripped, "");
    let text = citation_re().replace_all(&text, "");
    Cow::Owned(text.trim().to_string())
}

fn split_keys(inner: &str) -> impl Iterator<Item = &str> {
    inner
        .split(';')
        .map(|key| key.trim().trim_start_matches('@'))
}

/// 一篇研究报告里锚点与文献的编号表。
///
/// 编号只有走完全文才定得下来（`{@id}` 可以前引后面的章节），所以预览分两遍：
/// 第一遍把编号填进这张表，第二遍再解析一次，行内标记这时才换得成纸面字面。
#[derive(Debug, Default)]
pub(crate) struct ResearchMarks {
    labels: HashMap<String, String>,
    citations: HashMap<String, usize>,
}

impl ResearchMarks {
    /// 登记一个锚点的编号，例如 `fig:arch` → `1.2`。重复定义以先到的为准，
    /// 与 LaTeX 一致（后一个 `\label` 覆盖不了已经写进 aux 的引用目标）。
    pub(crate) fn define(&mut self, id: &str, number: &str) {
        self.labels
            .entry(id.to_string())
            .or_insert_with(|| number.to_string());
    }
    /// 按正文引用先后给文献编号——国标 GB/T 7714 的顺序编码制就是这么排的。
    pub(crate) fn cite(&mut self, key: &str) {
        let next = self.citations.len() + 1;
        self.citations.entry(key.to_string()).or_insert(next);
    }

    /// 把一行源码里的行内标记换成纸面上的样子。
    pub(crate) fn apply<'a>(&self, line: &'a str) -> Cow<'a, str> {
        let (stripped, label) = split_label(line);
        if label.is_none() && !stripped.contains("{@") && !stripped.contains("[@") {
            return Cow::Borrowed(line);
        }
        let text = crossref_re().replace_all(stripped, |caps: &Captures| self.reference(&caps[1]));
        let text = citation_re().replace_all(&text, |caps: &Captures| self.citation(&caps[1]));
        Cow::Owned(text.into_owned())
    }

    /// 未定义的锚点按 LaTeX 的办法印成 `??`：纸面和预览都看得见这处引用坏了。
    fn reference(&self, id: &str) -> String {
        self.labels.get(id).cloned().unwrap_or_else(|| "??".into())
    }

    /// `\citestyle{numbers}` 下的方括号序号。多键按写的顺序排，不做区间压缩。
    fn citation(&self, inner: &str) -> String {
        let numbers = split_keys(inner)
            .map(|key| match self.citations.get(key) {
                Some(number) => number.to_string(),
                None => "?".into(),
            })
            .collect::<Vec<_>>();
        format!("[{}]", numbers.join(","))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks() -> ResearchMarks {
        let mut marks = ResearchMarks::default();
        marks.define("chap:a", "1");
        marks.define("fig:x", "1.2");
        marks.cite("wang2020");
        marks.cite("li2021");
        marks
    }

    #[test]
    fn crossrefs_and_citations_become_the_numbers_the_pdf_prints() {
        let marks = marks();
        assert_eq!(
            marks.apply("见第{@chap:a}章的图{@fig:x}[@wang2020]。"),
            "见第1章的图1.2[1]。"
        );
        assert_eq!(marks.apply("综述[@wang2020; @li2021]。"), "综述[1,2]。");
    }

    #[test]
    fn unknown_targets_print_the_same_placeholders_latex_would() {
        let marks = marks();
        assert_eq!(marks.apply("见{@chap:nope}。"), "见??。");
        assert_eq!(marks.apply("见[@nope]。"), "见[?]。");
    }

    #[test]
    fn a_trailing_anchor_is_stripped_but_never_empties_the_line() {
        let marks = marks();
        assert_eq!(marks.apply("## 研究背景 {#chap:a}"), "## 研究背景");
        assert_eq!(marks.apply("![架构](a.png){#fig:x}"), "![架构](a.png)");
        // 整行只有锚点：留着，否则这一行会从块序列里消失。
        assert_eq!(marks.apply("{#fig:x}"), "{#fig:x}");
    }

    #[test]
    fn plain_lines_are_borrowed_not_rebuilt() {
        let marks = marks();
        assert!(matches!(
            marks.apply("普通正文，没有任何标记。"),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn citation_keys_come_out_in_reading_order() {
        assert_eq!(
            citation_keys("先 [@b] 后 [@a; @c]"),
            ["b", "a", "c"],
            "文献序号按正文引用先后排"
        );
    }

    #[test]
    fn outline_headings_drop_source_identifiers() {
        assert_eq!(strip_heading_identifiers("研究背景 {#chap:bg}"), "研究背景");
        assert_eq!(
            strip_heading_identifiers("相关工作{@chap:prior} [@wang2020; @li2021]"),
            "相关工作"
        );
        assert!(matches!(
            strip_heading_identifiers("普通标题"),
            Cow::Borrowed(_)
        ));
    }
}
