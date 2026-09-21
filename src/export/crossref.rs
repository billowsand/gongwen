//! 研究报告的行内扩展标记：锚点 `{#id}`、交叉引用 `{@id}`、文献引用 `[@key]`
//! 与行内脚注 `[^id]:(内容)`。
//!
//! 前三样都来自 mdx research。编译成 PDF 之后它们分别是 `\label`、`\ref` 和
//! `\cite`：锚点自己不占版面，`{@id}` 印的是被引对象的编号，`[@key]` 印的是
//! 方括号文献序号。脚注印成 `\footnote{}`，纸面上是页下注。纸上从来看不到大括
//! 号和 at 号，预览要照纸面显示，就得在排版之前把它们换掉。
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

/// 行内脚注 `[^id]:(内容)` 或 `[^id]：（内容）`，与 mdx 的 `common::inline`
/// 逐字一致：id 是 `]` 以外的任意非空串，冒号兼容全角，括号要么一对半角
/// 要么一对全角，内容里不能再出现同种闭括号（不支持嵌套，第一个闭括号收尾）。
/// 裸 `[^id]` 不带 `:(...)` 不匹配，原样保留。
fn footnote_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\[\^[^\]]+\][:：](?:\(([^)]*)\)|（([^）]*)）)").expect("脚注正则")
    })
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

fn split_keys(inner: &str) -> impl Iterator<Item = &str> {
    inner
        .split(';')
        .map(|key| key.trim().trim_start_matches('@'))
}

/// 全文所有行尾锚点的 id，按出现顺序；同一个 id 重复定义只留第一次。
/// 起草页「交叉引用」菜单拿它列出可引的目标。
pub(crate) fn label_ids(text: &str) -> Vec<&str> {
    let mut ids = Vec::new();
    for line in text.lines() {
        if let (_, Some(id)) = split_label(line)
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    }
    ids
}

/// 全文出现的交叉引用 `{@id}` 的 id，按出现顺序去重。
/// 校验器拿它对照锚点清单，提前揪出 PDF 里会印成 `??` 的悬空引用。
pub(crate) fn crossref_ids(text: &str) -> Vec<&str> {
    let mut ids = Vec::new();
    for caps in crossref_re().captures_iter(text) {
        let id = caps.get(1).expect("交叉引用 id").as_str();
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

/// BibTeX 条目开头 `@article{key,`。
fn bibtex_entry_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"@\w+\s*\{\s*([^,\s]+)").expect("BibTeX 条目正则"))
}

/// BibTeX 文献库里的条目键，按出现顺序去重。
/// 起草页「文献引用」菜单拿它列出可引的键。
pub(crate) fn bibtex_keys(bib: &str) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for caps in bibtex_entry_re().captures_iter(bib) {
        let key = caps[1].to_string();
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
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
        if label.is_none()
            && !stripped.contains("{@")
            && !stripped.contains("[@")
            && !stripped.contains("[^")
        {
            return Cow::Borrowed(line);
        }
        let text = crossref_re().replace_all(stripped, |caps: &Captures| self.reference(&caps[1]));
        let text = citation_re().replace_all(&text, |caps: &Captures| self.citation(&caps[1]));
        // 脚注不参与编号，纸面是页下注；预览就地展开成可读形式。
        let text = footnote_re().replace_all(&text, |caps: &Captures| {
            let content = caps
                .get(1)
                .or_else(|| caps.get(2))
                .map_or("", |g| g.as_str());
            format!("〔注：{content}〕")
        });
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
    fn inline_footnotes_expand_into_readable_notes() {
        let marks = marks();
        assert_eq!(
            marks.apply("正文[^1]:(注释内容)。"),
            "正文〔注：注释内容〕。"
        );
        assert_eq!(
            marks.apply("正文[^2]：（注释内容）。"),
            "正文〔注：注释内容〕。"
        );
        // 全角括号里可以装半角括号，只有同种闭括号收尾（内容不支持嵌套）。
        assert_eq!(
            marks.apply("注[^a]：（含(半角)括号）。"),
            "注〔注：含(半角)括号〕。"
        );
    }

    #[test]
    fn a_bare_footnote_mark_without_content_stays_as_is() {
        let marks = marks();
        // 裸 `[^1]` 不是脚注（mdx 也不认），原样保留。
        assert_eq!(marks.apply("引用[^1]待补。"), "引用[^1]待补。");
        assert_eq!(
            marks.apply("引用[^1]: 冒号后没括号。"),
            "引用[^1]: 冒号后没括号。"
        );
    }

    #[test]
    fn footnotes_mix_with_text_and_other_marks_on_one_line() {
        let marks = marks();
        assert_eq!(
            marks.apply("见{@chap:a}章[^1]:(详见附录)[@wang2020]，脚注[^n]：（再注）收尾。"),
            "见1章〔注：详见附录〕[1]，脚注〔注：再注〕收尾。"
        );
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
    fn label_ids_dedupe_in_reading_order() {
        let text = "## 研究背景 {#chap:a}\n正文。\n![架构](a.png) {#fig:x}\n## 后文 {#chap:a}";
        assert_eq!(label_ids(text), ["chap:a", "fig:x"]);
        // 行内的 `{#` 不是锚点：锚点必须独占行尾。
        assert!(label_ids("正文里 {#inline} 不算。").is_empty());
    }

    #[test]
    fn bibtex_keys_dedupe_in_reading_order() {
        let bib = "@article{wang2020, title={A}}\n@book{ li2021 , title={B}}\n@article{wang2020, title={C}}";
        assert_eq!(bibtex_keys(bib), ["wang2020", "li2021"]);
        assert!(bibtex_keys("").is_empty());
    }

    #[test]
    fn crossref_ids_dedupe_in_reading_order() {
        let text = "见{@chap:a}章与图{@fig:x}，后文再引{@chap:a}。";
        assert_eq!(crossref_ids(text), ["chap:a", "fig:x"]);
        // 锚点定义 `{#id}` 不是交叉引用，不该被收进来。
        assert!(crossref_ids("## 研究背景 {#chap:a}").is_empty());
        assert!(crossref_ids("普通正文。").is_empty());
    }
}
