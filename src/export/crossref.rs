//! 研究报告的行内扩展标记：锚点 `{#id}`、交叉引用 `{@id}`、文献引用 `[@key]`
//! 与 `@key`、行内脚注 `[^id]:(内容)`。
//!
//! 前几样都来自 mdx research。排成 PDF 之后：锚点自己不占版面，`{@id}` 印的是被引
//! 对象的编号；文献引用按 GB/T 7714 顺序编码制印方括号序号，`[@key]` 整组上标，
//! 叙述式的 `@key`（“见文献@key”，序号作句子成分）与正文平排。脚注是页下注。纸上
//! 从来看不到大括号和 at 号，预览要照纸面显示，就得在排版之前把它们换掉。
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

/// 文献引用的行内扫描，照 mdx `common::inline` 的最左匹配次序：脚注、行内代码、
/// 链接与图片、交叉引用、行内公式只为占住位置（它们里面的 `@` 不是引用），真正要
/// 收的是方括号引用 `[@a; @b]`（第 1 组）与叙述式引用 `@key`（第 2 组）。
fn citation_scan_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(concat!(
            r"\[\^[^\]]+\][:：](?:\([^)]*\)|（[^）]*）)",
            r"|`[^`]+`",
            r"|!?\[[^\]]*\]\([^)]+\)",
            r"|\{@[A-Za-z][\w:.-]*\}",
            r"|\[(@[^\s@;,\[\]{}\\]+(?:\s*;\s*@[^\s@;,\[\]{}\\]+)*)\]",
            r"|\$(?:\\\$|[^$\n])+\$",
            r"|@([A-Za-z_][A-Za-z0-9_]*(?:[:./-][A-Za-z0-9_]+)*)",
        ))
        .expect("文献引用正则")
    })
}

/// 上标的起止哨兵。预览里方括号引用印成上标：[`ResearchMarks::apply`] 把整组序号
/// 夹在这两个字符中间，排版时（`preview::layout::append_run`）去掉哨兵、缩小上移。
/// 私用区字符，与花脸稿哨兵（U+E000–E003）错开。
pub(crate) const SUPER_OPEN: char = '\u{E004}';
pub(crate) const SUPER_CLOSE: char = '\u{E005}';

/// 去掉上标哨兵：导航、交叉引用菜单这类只要纯文字的地方用。
pub(crate) fn strip_superscript(text: &str) -> Cow<'_, str> {
    if text.contains([SUPER_OPEN, SUPER_CLOSE]) {
        Cow::Owned(text.replace([SUPER_OPEN, SUPER_CLOSE], ""))
    } else {
        Cow::Borrowed(text)
    }
}

/// 正文里的一处文献引用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CitationMark<'a> {
    /// 整处引用（`[@a; @b]` 或 `@key`）在文字里的字节范围。
    pub(crate) range: std::ops::Range<usize>,
    pub(crate) keys: Vec<&'a str>,
    /// 叙述式 `@key`：序号与正文平排。否则是方括号引用，印成上标。
    pub(crate) narrative: bool,
}

/// 文字里的全部文献引用，按出现顺序。
///
/// 叙述式 `@key` 只在 `known(key)` 时算数——键不在文献库里的 `@` 是正文碰巧写的
/// 账号、记号，mdx 排 PDF 时原样印（`common::citation::validate`）。`@` 前一个字符的
/// 要求直接用 mdx 的 [`mdx::text_citation_is_live`]，两边是同一条规则。
pub(crate) fn citation_marks<'a>(
    text: &'a str,
    known: &dyn Fn(&str) -> bool,
) -> Vec<CitationMark<'a>> {
    let re = citation_scan_re();
    let mut marks = Vec::new();
    let mut pos = 0;
    while let Some(caps) = re.captures_at(text, pos) {
        let whole = caps.get(0).expect("整体匹配");
        let narrative = caps.get(2);
        if escaped_at(text, whole.start())
            || (narrative.is_some() && !mdx::text_citation_is_live(text, whole.start()))
        {
            // 起始字符是字面的：从下一个字符起重找，与 mdx 一致。
            pos = whole.start()
                + text[whole.start()..]
                    .chars()
                    .next()
                    .map_or(1, char::len_utf8);
            continue;
        }
        pos = whole.end();
        if let Some(group) = caps.get(1) {
            marks.push(CitationMark {
                range: whole.range(),
                keys: split_keys(group.as_str()).collect(),
                narrative: false,
            });
        } else if let Some(key) = narrative
            && known(key.as_str())
        {
            marks.push(CitationMark {
                range: whole.range(),
                keys: vec![key.as_str()],
                narrative: true,
            });
        }
    }
    marks
}

/// `index` 处的字符前面是否紧跟奇数个反斜杠（被转义成了字面字符）。
fn escaped_at(text: &str, index: usize) -> bool {
    text.as_bytes()[..index]
        .iter()
        .rev()
        .take_while(|&&byte| byte == b'\\')
        .count()
        % 2
        == 1
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

/// 文字里出现的文献引用键，按出现顺序；`[@a; @b]` 拆成两条。叙述式 `@key` 只收
/// `known` 认的（见 [`citation_marks`]）。
pub(crate) fn citation_keys<'a>(text: &'a str, known: &dyn Fn(&str) -> bool) -> Vec<&'a str> {
    citation_marks(text, known)
        .into_iter()
        .flat_map(|mark| mark.keys)
        .collect()
}

/// 光标所在的那组文献引用：光标落在 `[@…]` 里面或紧挨在 `]` 之后。返回整组在全文里的
/// 字节范围与组里的键。起草页插文献时据此并进这一组，免得插出 `[@a][@b]`。
pub(crate) fn citation_at(
    text: &str,
    cursor: usize,
) -> Option<(std::ops::Range<usize>, Vec<&str>)> {
    let cursor = cursor.min(text.len());
    let line_start = text[..cursor].rfind('\n').map_or(0, |at| at + 1);
    let line_end = text[cursor..]
        .find('\n')
        .map_or(text.len(), |at| cursor + at);
    let line = &text[line_start..line_end];
    // 只并进方括号组：叙述式 `@key` 是句子成分，插进去的文献另起一组。
    citation_marks(line, &|_| false)
        .into_iter()
        .find_map(|mark| {
            let range = line_start + mark.range.start..line_start + mark.range.end;
            (range.start < cursor && cursor <= range.end).then_some((range, mark.keys))
        })
}

fn split_keys(inner: &str) -> impl Iterator<Item = &str> {
    inner
        .split(';')
        .map(|key| key.trim().trim_start_matches('@'))
}

/// 全文每一处锚点定义：id 与 `{#id}` 在全文里的字节范围，按出现顺序，不去重。
/// 查重复定义用：同一个 id 出现几次就有几条。
pub(crate) fn label_definitions(text: &str) -> Vec<(&str, std::ops::Range<usize>)> {
    let mut definitions = Vec::new();
    let mut offset = 0usize;
    for piece in text.split_inclusive('\n') {
        let line = piece.trim_end_matches(['\n', '\r']);
        if split_label(line).1.is_some()
            && let Some(caps) = label_re().captures(line)
        {
            let id = caps.get(1).expect("锚点 id");
            // `{#` 在 id 前面两个字节，`}` 紧跟在 id 后面。
            definitions.push((
                &text[offset + id.start()..offset + id.end()],
                offset + id.start() - 2..offset + id.end() + 1,
            ));
        }
        offset += piece.len();
    }
    definitions
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
    /// 每个键在正文里被引了几处（`[@a; @a]` 算两处）。
    uses: HashMap<String, usize>,
}

/// 正文引过的一条文献：PDF 里的序号与引用处数。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CitedKey {
    pub(crate) key: String,
    pub(crate) number: usize,
    pub(crate) uses: usize,
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
        *self.uses.entry(key.to_string()).or_default() += 1;
    }

    /// 正文引过的文献，按序号排。
    pub(crate) fn cited(&self) -> Vec<CitedKey> {
        let mut cited: Vec<_> = self
            .citations
            .iter()
            .map(|(key, number)| CitedKey {
                key: key.clone(),
                number: *number,
                uses: self.uses.get(key).copied().unwrap_or_default(),
            })
            .collect();
        cited.sort_by_key(|cited| cited.number);
        cited
    }

    /// 把一行源码里的行内标记换成纸面上的样子。方括号引用的序号夹在上标哨兵
    /// （[`SUPER_OPEN`]、[`SUPER_CLOSE`]）中间；叙述式 `@key` 只换第一遍登记过的键
    /// （第一遍只登记文献库里有的，见 [`citation_marks`]），其余原样。
    pub(crate) fn apply<'a>(&self, line: &'a str) -> Cow<'a, str> {
        let (stripped, label) = split_label(line);
        if label.is_none() && !stripped.contains('@') && !stripped.contains("[^") {
            return Cow::Borrowed(line);
        }
        let marks = citation_marks(stripped, &|key| self.citations.contains_key(key));
        let mut cited = String::with_capacity(stripped.len());
        let mut last = 0;
        for mark in &marks {
            cited.push_str(&stripped[last..mark.range.start]);
            let numbers = self.citation(&mark.keys);
            if mark.narrative {
                cited.push_str(&numbers);
            } else {
                cited.push(SUPER_OPEN);
                cited.push_str(&numbers);
                cited.push(SUPER_CLOSE);
            }
            last = mark.range.end;
        }
        cited.push_str(&stripped[last..]);
        let text = crossref_re().replace_all(&cited, |caps: &Captures| self.reference(&caps[1]));
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

    /// 一组引用的方括号序号，照 hayagriva 的 `gb-7714-2015-numeric`：序号从小到大，
    /// 三个及以上连号压成区间（`[1–3]`，连接号是 en dash），其余用半角逗号隔开。
    /// 文献库里没有的键印 `?`，排在最后。
    fn citation(&self, keys: &[&str]) -> String {
        let mut numbers: Vec<usize> = keys
            .iter()
            .filter_map(|key| self.citations.get(*key).copied())
            .collect();
        numbers.sort_unstable();
        numbers.dedup();
        let mut parts: Vec<String> = Vec::new();
        let mut index = 0;
        while index < numbers.len() {
            let mut end = index;
            while end + 1 < numbers.len() && numbers[end + 1] == numbers[end] + 1 {
                end += 1;
            }
            if end - index >= 2 {
                parts.push(format!("{}–{}", numbers[index], numbers[end]));
            } else {
                parts.extend(numbers[index..=end].iter().map(usize::to_string));
            }
            index = end + 1;
        }
        if keys.iter().any(|key| !self.citations.contains_key(*key)) {
            parts.push("?".into());
        }
        format!("[{}]", parts.join(","))
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

    /// 上标哨兵换成 `^{…}`，断言好读。
    fn shown(text: &str) -> String {
        text.replace(SUPER_OPEN, "^{").replace(SUPER_CLOSE, "}")
    }

    #[test]
    fn crossrefs_and_citations_become_the_numbers_the_pdf_prints() {
        let marks = marks();
        assert_eq!(
            shown(&marks.apply("见第{@chap:a}章的图{@fig:x}[@wang2020]。")),
            "见第1章的图1.2^{[1]}。"
        );
        assert_eq!(
            shown(&marks.apply("综述[@wang2020; @li2021]。")),
            "综述^{[1,2]}。"
        );
    }

    /// 叙述式 `@key` 与正文平排，只换第一遍登记过的键；其余 `@` 原样。
    #[test]
    fn text_citations_print_inline_and_only_for_cited_keys() {
        let marks = marks();
        assert_eq!(
            shown(&marks.apply("见文献@li2021，王某等@wang2020指出。")),
            "见文献[2]，王某等[1]指出。"
        );
        assert_eq!(
            marks.apply("联系 @admin 或 user@wang2020.cn。"),
            "联系 @admin 或 user@wang2020.cn。"
        );
    }

    /// 与 hayagriva 的 GB/T 7714 顺序编码样式一致：排序、三连号压成区间。
    #[test]
    fn citation_groups_sort_and_collapse_like_the_pdf() {
        let mut marks = ResearchMarks::default();
        for key in ["a", "b", "c", "d", "e"] {
            marks.cite(key);
        }
        let group = |text: &str| shown(&marks.apply(text));
        assert_eq!(group("[@c; @a; @b]"), "^{[1–3]}");
        assert_eq!(group("[@d; @b]"), "^{[2,4]}");
        assert_eq!(group("[@a; @b; @c; @e]"), "^{[1–3,5]}");
        assert_eq!(group("[@a; @a]"), "^{[1]}");
        assert_eq!(group("[@b; @nope]"), "^{[2,?]}");
    }

    #[test]
    fn citation_marks_follow_the_mdx_rules() {
        let known = |key: &str| key != "admin";
        let marks = |text| {
            citation_marks(text, &known)
                .into_iter()
                .map(|mark| (mark.keys, mark.narrative))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            marks("甲[@a; @b]乙@c。(@d) @admin"),
            [
                (vec!["a", "b"], false),
                (vec!["c"], true),
                (vec!["d"], true)
            ]
        );
        // 代码、链接、脚注、公式、交叉引用里的 `@` 不是引用；转义的也不是。
        for literal in [
            "`[@a] @b`",
            "[见 @a](https://x/@b)",
            "注[^1]:(@a)",
            "$@a$",
            "{@sec:a}",
            r"\[@a] \@b",
            "x@a",
            "[@a, p. 2]",
        ] {
            assert!(marks(literal).is_empty(), "{literal}");
        }
    }

    #[test]
    fn strip_superscript_removes_only_the_sentinels() {
        let marks = marks();
        assert_eq!(strip_superscript(&marks.apply("题[@li2021]")), "题[2]");
        assert!(matches!(strip_superscript("普通"), Cow::Borrowed(_)));
    }

    #[test]
    fn citation_at_finds_the_group_around_or_just_before_the_cursor() {
        let text = "甲[@a; @b]乙\n丙[@c]@d";
        let at =
            |cursor: usize| citation_at(text, cursor).map(|(range, keys)| (&text[range], keys));
        let first = text.find('[').unwrap();
        assert_eq!(at(first), None, "光标在 [ 之前不算");
        assert_eq!(at(first + 3), Some(("[@a; @b]", vec!["a", "b"])));
        let end = first + "[@a; @b]".len();
        assert_eq!(at(end), Some(("[@a; @b]", vec!["a", "b"])), "紧挨在 ] 之后");
        assert_eq!(at(end + "乙".len()), None);
        // 叙述式 `@d` 不是组：紧挨着它也不并。
        assert_eq!(at(text.len()), None);
        assert_eq!(at(text.len() - 2), Some(("[@c]", vec!["c"])));
    }

    /// 序号按首次引用定，处数每处都算，`[@a; @a]` 也是两处。
    #[test]
    fn cited_lists_numbers_and_uses_in_number_order() {
        let mut marks = ResearchMarks::default();
        for key in citation_keys("先 [@b] 后 [@a; @b]，再 @b", &|_| true) {
            marks.cite(key);
        }
        let cited: Vec<_> = marks
            .cited()
            .into_iter()
            .map(|cited| (cited.key, cited.number, cited.uses))
            .collect();
        assert_eq!(cited, [("b".into(), 1, 3), ("a".into(), 2, 1)]);
    }

    #[test]
    fn unknown_targets_print_the_same_placeholders_latex_would() {
        let marks = marks();
        assert_eq!(marks.apply("见{@chap:nope}。"), "见??。");
        assert_eq!(shown(&marks.apply("见[@nope]。")), "见^{[?]}。");
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
            shown(
                &marks.apply("见{@chap:a}章[^1]:(详见附录)[@wang2020]，脚注[^n]：（再注）收尾。")
            ),
            "见1章〔注：详见附录〕^{[1]}，脚注〔注：再注〕收尾。"
        );
    }

    #[test]
    fn citation_keys_come_out_in_reading_order() {
        assert_eq!(
            citation_keys("先 [@b] 后 [@a; @c]，@d 与 @e", &|key| key != "e"),
            ["b", "a", "c", "d"],
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
    fn label_definitions_keep_every_occurrence_with_its_span() {
        let text = "## 甲 {#chap:a}\r\n正文。\n![图](a.png){#fig:x}\n## 乙 {#chap:a}";
        let definitions = label_definitions(text);
        let ids: Vec<&str> = definitions.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, ["chap:a", "fig:x", "chap:a"]);
        for (id, span) in &definitions {
            assert_eq!(&text[span.clone()], format!("{{#{id}}}"));
        }
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
