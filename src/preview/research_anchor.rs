//! 研究报告「锚点」按钮：按光标所在的块起一个不重复、看得懂的 id。
//!
//! 挂在哪、起什么前缀，都交给预览的 [`walk`]——它按 mdx 的规则给每一块定身份
//! 与编号，与交叉引用菜单、纸面编号同一口径。只有引得出号的块才给锚点：
//!
//! | 块 | 前缀 | 挂在哪一行 |
//! |---|---|---|
//! | 部分 | `part:` | `#` 标题行 |
//! | 章 | `chap:` | `##` 标题行（附录的章同样） |
//! | 节、小节 | `sec:` | `###` 起的标题行 |
//! | 有图注的图 | `fig:` | 图片行 |
//! | 有表题的表 | `tbl:` | 表题行（光标在表格里也挂到表题上） |
//! | 文框 | 按名称：专栏 `box:`、案例 `case:`、例子 `ex:`，其余取名称拼音 | 首行（光标在框里也挂到首行） |
//!
//! 冒号后面是标题的拼音：jieba 分词后取前几个实词，每词一段，用 `-` 连起来
//! （"总体架构" → `fig:zongti-jiagou`）。全文已有同名 id 就依次加 `-2`、`-3`。

use super::research::{Kind, walk};
use crate::export::{self, LocatedBlock, MarkdownBlock, crossref};
use pinyin::ToPinyin;
use regex::Regex;
use std::ops::Range;
use std::sync::OnceLock;

/// 锚点该插在哪、叫什么。
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AnchorSuggestion {
    /// 目标行的行尾（字节位置），` {#id}` 就插在这里。
    pub(crate) line_end: usize,
    pub(crate) id: String,
}

/// 光标所在处为什么挂不了锚点，或者已经挂着一个。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AnchorRefusal {
    /// 目标行已经有锚点：给出它的 id 和位置（`{#id}` 在全文里的字节范围）。
    Existing { id: String, span: Range<usize> },
    /// 这一块没有编号可引，附一句给用户看的原因。
    NotNumbered(&'static str),
}

/// 标题里取几个词、拼音最长多少字节就收手：够认得出，又不至于长到难打。
const SLUG_WORDS: usize = 3;
const SLUG_BYTES: usize = 18;

/// 按光标位置 `cursor`（字节）给锚点定位置、起 id。
pub(crate) fn suggest(markdown: &str, cursor: usize) -> Result<AnchorSuggestion, AnchorRefusal> {
    let cursor = cursor.min(markdown.len());
    let blocks = export::parse_markdown_located(markdown);
    let mut found: Option<(Range<usize>, Target)> = None;
    walk(&blocks, markdown, |located, kind, _| {
        if found.is_some() {
            return;
        }
        if let Some(target) = target_of(markdown, located, &kind, cursor) {
            found = Some(target);
        }
    });
    let Some((line, target)) = found else {
        return Err(AnchorRefusal::NotNumbered(
            "光标所在处没有编号可引。锚点要挂在章节标题、有图注的图片、表题或文框首行上",
        ));
    };
    let target = target?;
    let text = &markdown[line.clone()];
    if let (_, Some(id)) = crossref::split_label(text) {
        let start = line.start + text.rfind("{#").unwrap_or(0);
        return Err(AnchorRefusal::Existing {
            id: id.to_string(),
            span: start..line.start + text.trim_end().len(),
        });
    }
    let taken = crossref::label_ids(markdown);
    Ok(AnchorSuggestion {
        line_end: line.start + text.trim_end().len(),
        id: unique_id(&target.prefix, &target.slug, &taken),
    })
}

/// 一块能挂锚点时的前缀与拼音；挂不了时是原因。
type Target = Result<Named, AnchorRefusal>;

struct Named {
    prefix: String,
    slug: String,
}

/// 光标落在这一块上时，锚点挂在哪一行、叫什么。光标不在这一块上返回 None。
fn target_of(
    markdown: &str,
    located: &LocatedBlock,
    kind: &Kind<'_>,
    cursor: usize,
) -> Option<(Range<usize>, Target)> {
    let covers = |range: &Range<usize>| range.start <= cursor && cursor <= range.end;
    // 表格：光标在表题行上或表格里都算，锚点挂在表题行。表题那一块本身在
    // `walk` 里是 Skip（并进了表格），轮到表格时再认。
    if let Kind::Table { caption } = kind {
        let on_caption = caption.as_ref().is_some_and(|c| covers(&c.source));
        if !on_caption && !covers(&located.range) {
            return None;
        }
        return Some(match caption {
            Some(caption) => (caption.source.clone(), Ok(named("tbl", &caption.text))),
            None => (
                located.range.clone(),
                Err(AnchorRefusal::NotNumbered(
                    "这张表还没有表题，不编号。先用「表题」在表格上方加一行“表：题名”，再挂锚点",
                )),
            ),
        });
    }
    if !covers(&located.range) || matches!(kind, Kind::Skip | Kind::AbstractOpen) {
        return None;
    }
    let line = first_line(markdown, &located.range);
    let target = match kind {
        Kind::Part { text, .. } => Ok(named("part", text)),
        Kind::Chapter { text, .. } => Ok(named("chap", text)),
        Kind::Section { text, .. } => Ok(named("sec", text)),
        Kind::Figure {
            number: Some(_),
            alt,
            ..
        } => Ok(named("fig", alt)),
        Kind::Figure { number: None, .. } => Err(AnchorRefusal::NotNumbered(
            "没有图注的图片不编号。先在 ![图注](…) 的方括号里写上图注，再挂锚点",
        )),
        Kind::Diagram {
            number: Some(_),
            caption,
            ..
        } => Ok(named("fig", caption)),
        Kind::Diagram { number: None, .. } => Err(AnchorRefusal::NotNumbered(
            "没有图题的流程图不编号。先在结束围栏下一行写“图：题名”，再挂锚点",
        )),
        Kind::Box { name, title, .. } => {
            let slug_source = if title.trim().is_empty() { name } else { title };
            Ok(Named {
                prefix: box_prefix(name),
                slug: slug(slug_source),
            })
        }
        Kind::Citation => Err(AnchorRefusal::NotNumbered(
            "引文不编号，锚点挂上去引不出号；要编号的材料请写成文框（> [!专栏] 标题）",
        )),
        Kind::ReportTitle(_) => Err(AnchorRefusal::NotNumbered(
            "报告题名印在封面上、不编号，锚点挂上去引不出号",
        )),
        Kind::ChapterStar(_) | Kind::SectionStar(_) | Kind::PartStar(_) => Err(
            AnchorRefusal::NotNumbered("不编号的标题引不出号，锚点挂上去只会印成 ??"),
        ),
        Kind::AbstractHeading(_) => Err(AnchorRefusal::NotNumbered(
            "摘要里的标题不编号，锚点挂上去引不出号",
        )),
        _ => Err(AnchorRefusal::NotNumbered(
            "正文段落没有编号可引。锚点要挂在章节标题、有图注的图片、表题或文框首行上",
        )),
    };
    // 引用块（文框）的锚点写在首行，光标在框里哪一行都挂到首行上。
    let line = match located.block {
        MarkdownBlock::Quote { .. } => line,
        MarkdownBlock::Diagram { .. } => {
            let raw = &markdown[located.range.clone()];
            let offset = raw.rfind('\n').map_or(0, |pos| pos + 1);
            located.range.start + offset..located.range.end
        }
        _ => located.range.clone(),
    };
    Some((line, target))
}

/// 块的第一行（不含换行符）。
fn first_line(markdown: &str, range: &Range<usize>) -> Range<usize> {
    let text = &markdown[range.clone()];
    let len = text.find('\n').unwrap_or(text.len());
    range.start..range.start + len
}

fn named(prefix: &str, title: &str) -> Named {
    Named {
        prefix: prefix.to_string(),
        slug: slug(title),
    }
}

/// 文框名称对应的前缀：常用的几种给英文缩写，其余取名称的拼音。
fn box_prefix(name: &str) -> String {
    match name.trim() {
        "专栏" => "box".into(),
        "案例" | "典型案例" => "case".into(),
        "例子" | "示例" | "举例" | "例" => "ex".into(),
        other => {
            let slug = slug(other).replace('-', "");
            if slug.is_empty() { "box".into() } else { slug }
        }
    }
}

/// 标题的拼音：去掉行内标记，jieba 分词后取前几个实词，每词一段。
fn slug(title: &str) -> String {
    let clean = marks_re().replace_all(title, "");
    let clean = crossref::split_label(&clean).0.to_string();
    let clean = export::plain_text(&clean);
    let mut parts: Vec<String> = Vec::new();
    let mut bytes = 0usize;
    for (word, tag) in crate::lexicon::segmenter::tagged(&clean) {
        // 助词、介词、连词、标点不进 id：“关于……的……”取出来全是虚词没法认。
        // 助词（u 打头的 uj、ul……）按前缀认；其余单字母词性按整串认，免得把
        // 英文词的 `eng` 当成叹词 `e` 丢掉。
        if tag.starts_with('u') || matches!(tag.as_str(), "p" | "c" | "x" | "w" | "e" | "y" | "o") {
            continue;
        }
        let piece = word_slug(&word);
        if piece.is_empty() {
            continue;
        }
        bytes += piece.len();
        parts.push(piece);
        if parts.len() >= SLUG_WORDS || bytes >= SLUG_BYTES {
            break;
        }
    }
    parts.join("-")
}

/// 一个词的拼音：汉字取默认读音连写，ASCII 字母数字转小写保留，其余字符丢掉。
fn word_slug(word: &str) -> String {
    let mut out = String::new();
    for ch in word.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if let Some(pinyin) = ch.to_pinyin() {
            out.push_str(&pinyin.plain().replace('ü', "v"));
        }
    }
    out
}

/// 标题里的交叉引用、文献引用、脚注：不进 id。
fn marks_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\{@[^}]*\}|\[@[^\]]*\]|\[\^[^\]]*\][:：]?(?:\([^)]*\)|（[^）]*）)?")
            .expect("行内标记正则")
    })
}

/// `prefix:slug`，全文已有就依次加 `-2`、`-3`。拼音为空（标题全是符号）时直接
/// 用序号：`fig:1`、`fig:2`。
fn unique_id(prefix: &str, slug: &str, taken: &[&str]) -> String {
    let base = if slug.is_empty() {
        None
    } else {
        Some(format!("{prefix}:{slug}"))
    };
    if let Some(base) = &base
        && !taken.contains(&base.as_str())
    {
        return base.clone();
    }
    (if base.is_some() { 2 } else { 1 }..)
        .map(|n| match &base {
            Some(base) => format!("{base}-{n}"),
            None => format!("{prefix}:{n}"),
        })
        .find(|id| !taken.contains(&id.as_str()))
        .expect("总能找到没用过的序号")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(markdown: &str, needle: &str) -> Result<AnchorSuggestion, AnchorRefusal> {
        let _guard = crate::lexicon::segmenter::test_lock();
        suggest(markdown, markdown.find(needle).expect("光标位置"))
    }

    fn id(markdown: &str, needle: &str) -> String {
        match at(markdown, needle) {
            Ok(suggestion) => suggestion.id,
            Err(refusal) => panic!("应能挂锚点：{refusal:?}"),
        }
    }

    const REPORT: &str = concat!(
        "# 某某问题研究报告\n\n",
        "<!-- [不编号] -->\n## 前言\n\n",
        "## 研究背景\n\n### 数据来源与方法\n\n正文段落。\n\n",
        "![总体架构](images/a.png)\n\n![](images/b.png)\n\n",
        "```mermaid\nflowchart LR\n A[收文] --> B[办理]\n```\n图：办理流程\n\n",
        "表：三省样本分布\n\n| 省份 | 样本数 |\n| --- | --- |\n| 甲省 | 320 |\n\n",
        "| 无题 | 表 |\n| --- | --- |\n| 1 | 2 |\n\n",
        "> [!案例] 某市场景牵引做法\n>\n> 一是先定场景。\n\n",
        "> [!做法]\n>\n> 内容。\n\n",
        "> 引文。\n",
    );

    #[test]
    fn prefixes_follow_the_block_and_the_rest_is_title_pinyin() {
        assert_eq!(id(REPORT, "研究背景"), "chap:yanjiu-beijing");
        assert_eq!(id(REPORT, "数据来源"), "sec:shuju-laiyuan-fangfa");
        assert_eq!(id(REPORT, "![总体架构]"), "fig:zongti-jiagou");
        assert_eq!(id(REPORT, "flowchart LR"), "fig:banli-liucheng");
        // jieba 把“样本分布”切成一个词，拼音就连写成一段
        assert_eq!(id(REPORT, "表：三省"), "tbl:sansheng-yangbenfenbu");
        // 光标在表格里、文框正文里，都挂到表题行、首行上
        assert_eq!(id(REPORT, "| 甲省"), "tbl:sansheng-yangbenfenbu");
        assert_eq!(id(REPORT, "一是先定"), "case:moushi-changjing-qianyin");
        // 名称没有现成缩写时取拼音，没有标题时拼音取自名称
        assert_eq!(id(REPORT, "> [!做法]"), "zuofa:zuofa");
    }

    #[test]
    fn the_anchor_goes_at_the_end_of_the_target_line() {
        let suggestion = at(REPORT, "| 甲省").unwrap();
        assert_eq!(
            &REPORT[..suggestion.line_end],
            REPORT[..REPORT.find("\n\n| 省份").unwrap()].to_string()
        );
        let suggestion = at(REPORT, "一是先定").unwrap();
        assert!(REPORT[..suggestion.line_end].ends_with("> [!案例] 某市场景牵引做法"));
        let suggestion = at(REPORT, "flowchart LR").unwrap();
        assert!(REPORT[..suggestion.line_end].ends_with("图：办理流程"));
    }

    #[test]
    fn taken_ids_get_a_numeric_suffix() {
        let markdown = "## 研究背景 {#chap:yanjiu-beijing}\n\n## 研究背景\n";
        assert_eq!(id(markdown, "## 研究背景\n"), "chap:yanjiu-beijing-2");
        let markdown = "## 研究背景 {#chap:yanjiu-beijing}\n\n\
                        ## 研究背景 {#chap:yanjiu-beijing-2}\n\n## 研究背景\n";
        assert_eq!(id(markdown, "## 研究背景\n"), "chap:yanjiu-beijing-3");
        assert_eq!(unique_id("fig", "", &["fig:1"]), "fig:2");
    }

    #[test]
    fn blocks_without_a_number_are_refused_with_a_reason() {
        for needle in [
            "# 某某",
            "## 前言",
            "正文段落",
            "![](images/b.png)",
            "| 无题",
            "> 引文",
        ] {
            assert!(
                matches!(at(REPORT, needle), Err(AnchorRefusal::NotNumbered(_))),
                "{needle}"
            );
        }
    }

    #[test]
    fn an_existing_anchor_is_reported_instead_of_a_second_one() {
        let markdown = "## 研究背景 {#chap:bg}\n";
        assert_eq!(
            at(markdown, "研究"),
            Err(AnchorRefusal::Existing {
                id: "chap:bg".into(),
                span: markdown.find("{#").unwrap()..markdown.len() - 1,
            })
        );
    }

    #[test]
    fn function_words_and_marks_stay_out_of_the_slug() {
        let _guard = crate::lexicon::segmenter::test_lock();
        assert_eq!(slug("关于**数据**的治理{@chap:a}"), "shuju-zhili");
        assert_eq!(slug("AI 应用现状"), "ai-yingyong-xianzhuang");
    }
}
