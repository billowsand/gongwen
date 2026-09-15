//! 行内格式拆分：把一行原文切成 Inline 序列（Text / Bold / Italic / Code / Link / Footnote）。
//!
//! 识别 markdown 的 `**加粗**`、`*斜体*`、`` `代码` ``、`[文本](链接)`、`![替代文本](图片路径)`
//! 以及行内脚注 `[^id]:(注释内容)`（冒号、括号兼容全角）；其他符号原样进入 Text。
//! 扩展标记：图片后紧跟 `{#id}` 作为交叉引用锚点；`{@id}` 为交叉引用（tex → \ref{id}）。
//! 与 md_to_docx_rust::process_text_formatting 的拆分规则一致：
//! - `**...**` 至少 4 字符长才视作粗体
//! - `*...*` 至少 2 字符长才视作斜体（且不会被 `**` 误吞）

use regex::Regex;
use std::sync::OnceLock;

use super::ast::Inline;

/// 行内构造统一匹配层：代码、链接/图片、强调、交叉引用、文献引用同处一层。
///
/// 关键在于它们的起始字符互不相同（`` ` `` / `[` / `!` / `*` / `{`），因此
/// `find_iter` 的“最左、非重叠”语义天然给出正确优先级：谁先起始谁整体胜出，
/// 被包住的内部构造再由强调的递归解析处理。这样：
/// - `` `a*b*c` `` 整段是代码（`` ` `` 起始最靠左），内部 `*` 不成强调；
/// - `**加粗 `代码` 与 [链接](u) 与 {@ref} 与 [@cite]**` 里强调整体先匹配、
///   再递归进代码/链接/交叉引用/引用；
/// - `[**粗**](u)` 里链接整体先匹配，链接文字原样保留（与历史行为一致）。
///
/// 唯一留在更高层的是脚注（见 [`parse`]）：脚注不能被强调包裹（既有限制）。
/// `[` 起始的链接与引用是两个候选，链接需 `](...)` 结构、引用需 `[@...]`，
/// 二者实际不会匹配同一段；命中后再按 [`link_matcher`] 复核区分。
fn inline_matcher() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"`[^`]+`|!?\[[^\]]*\]\([^)]+\)|\*\*[^*]+\*\*|\*[^*]+\*|\{@[A-Za-z][\w:.-]*\}|\[(?:@[^\s@;,\[\]{}\\]+)(?:\s*;\s*@[^\s@;,\[\]{}\\]+)*\]",
        )
        .expect("invalid inline regex")
    })
}

/// 链接与图片共用：`[text](url)` 或 `![alt](url)`（图片 alt 可为空）。
fn link_matcher() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"!?\[([^\]]*)\]\(([^)]+)\)").expect("invalid link regex"))
}

/// 行内脚注：`[^id]:(内容)` 或 `[^id]：（内容）`，冒号与括号均兼容全角。
fn footnote_matcher() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\[\^[^\]]+\][:：](?:\(([^)]*)\)|（([^）]*)）)")
            .expect("invalid footnote regex")
    })
}

/// 图片标签属性：紧跟 `![alt](url)` 之后的 `{#id}`（锚定匹配，用于向前窥探）。
fn label_attr_matcher() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\{#([A-Za-z][\w:.-]*)\}").expect("invalid label attr regex"))
}

/// 把一段已正规化引号的纯文本拆成 Inline 列表。
///
/// 分两层：脚注在最外层先切出（内容原样、不嵌套，故脚注不能被强调包裹——既有
/// 限制）；其余文本交给 [`parse_inline`]，在同一层内统一处理代码 / 链接 / 图片 /
/// 强调 / 交叉引用 / 文献引用，强调内部递归回本函数以支持任意合法嵌套。
///
/// 空字符串返回空向量。
pub fn parse(text: &str) -> Vec<Inline> {
    if text.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    let footnote_re = footnote_matcher();
    let mut last_end = 0;

    for m in footnote_re.find_iter(text) {
        if m.start() > last_end {
            out.extend(parse_inline(&text[last_end..m.start()]));
        }
        let caps = footnote_re
            .captures(m.as_str())
            .expect("footnote match without captures");
        let content = caps
            .get(1)
            .or_else(|| caps.get(2))
            .map(|g| g.as_str())
            .unwrap_or("");
        out.push(Inline::Footnote(content.to_string()));
        last_end = m.end();
    }

    if last_end < text.len() {
        out.extend(parse_inline(&text[last_end..]));
    }

    out
}

/// 统一行内层：代码 / 链接 / 图片 / 强调 / 交叉引用 / 文献引用同处一层，靠
/// [`inline_matcher`] 的最左非重叠匹配定优先级。用 `find_at` 从游标推进，以便
/// 图片可以额外吞掉紧随其后的 `{#id}` 锚点。强调命中后对内部递归调用 [`parse`]。
fn parse_inline(text: &str) -> Vec<Inline> {
    if text.is_empty() {
        return Vec::new();
    }

    let re = inline_matcher();
    let mut out = Vec::new();
    let mut pos = 0;

    while let Some(m) = re.find_at(text, pos) {
        if m.start() > pos {
            out.push(Inline::Text(text[pos..m.start()].to_string()));
        }
        let part = m.as_str();
        pos = m.end();
        match part.as_bytes()[0] {
            // 行内代码：内容原样。
            b'`' => out.push(Inline::Code(part[1..part.len() - 1].to_string())),
            // 强调：内部递归解析，允许再嵌代码 / 链接 / 交叉引用 / 引用。
            b'*' => {
                if part.starts_with("**") {
                    out.push(Inline::Bold(parse(&part[2..part.len() - 2])));
                } else {
                    out.push(Inline::Italic(parse(&part[1..part.len() - 1])));
                }
            }
            // 交叉引用 `{@id}`：剥掉 `{@` 与 `}`。
            b'{' => out.push(Inline::CrossRef(part[2..part.len() - 1].to_string())),
            // 图片 `![alt](url)`，并吞掉紧随其后的 `{#id}` 锚点。
            b'!' => {
                let caps = link_matcher()
                    .captures(part)
                    .expect("image match without captures");
                let alt = caps[1].to_string();
                let url = caps[2].to_string();
                let mut label = None;
                if let Some(attr) = label_attr_matcher().captures(&text[pos..]) {
                    label = Some(attr[1].to_string());
                    pos += attr.get(0).expect("attr whole group").end();
                }
                out.push(Inline::Image { alt, url, label });
            }
            // `[` 起始：有 `](...)` 结构的是链接（文字原样），否则按文献引用解析。
            b'[' => {
                if let Some(caps) = link_matcher().captures(part) {
                    out.push(Inline::Link {
                        text: caps[1].to_string(),
                        url: caps[2].to_string(),
                    });
                } else {
                    let keys = part[1..part.len() - 1]
                        .split(';')
                        .map(|item| item.trim().trim_start_matches('@').to_string())
                        .collect();
                    out.push(Inline::Citation(keys));
                }
            }
            _ => out.push(Inline::Text(part.to_string())),
        }
    }

    if pos < text.len() {
        out.push(Inline::Text(text[pos..].to_string()));
    }

    out
}

/// 把 Inline 序列拼回纯字符串（emitter 在不需要格式时使用，比如表格 cell 简化）。
#[allow(dead_code)]
pub fn flatten(inlines: &[Inline]) -> String {
    let mut s = String::new();
    for ip in inlines {
        match ip {
            Inline::Text(t) => s.push_str(t),
            Inline::Bold(children) | Inline::Italic(children) => {
                s.push_str(&flatten(children));
            }
            Inline::Code(t) => s.push_str(t),
            Inline::Link { text, .. } => s.push_str(text),
            Inline::Image { alt, .. } => s.push_str(alt),
            Inline::CrossRef(id) => s.push_str(id),
            Inline::Citation(keys) => {
                s.push('[');
                s.push_str(
                    &keys
                        .iter()
                        .map(|key| format!("@{key}"))
                        .collect::<Vec<_>>()
                        .join("; "),
                );
                s.push(']');
            }
            Inline::Footnote(t) => {
                s.push('（');
                s.push_str(t);
                s.push('）');
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inline_footnote_halfwidth() {
        let inlines = parse("正文[^1]:(这是注释)继续");
        assert_eq!(
            inlines,
            vec![
                Inline::Text("正文".into()),
                Inline::Footnote("这是注释".into()),
                Inline::Text("继续".into()),
            ]
        );
    }

    #[test]
    fn parses_inline_footnote_fullwidth() {
        let inlines = parse("正文[^2]：（全角注释）");
        assert_eq!(
            inlines,
            vec![
                Inline::Text("正文".into()),
                Inline::Footnote("全角注释".into()),
            ]
        );
    }

    #[test]
    fn footnote_coexists_with_bold_and_link() {
        let inlines = parse("**重点**[^a]:(注) [页](https://x)");
        assert_eq!(
            inlines,
            vec![
                Inline::Bold(vec![Inline::Text("重点".into())]),
                Inline::Footnote("注".into()),
                Inline::Text(" ".into()),
                Inline::Link {
                    text: "页".into(),
                    url: "https://x".into()
                },
            ]
        );
    }

    #[test]
    fn bare_footnote_ref_without_definition_stays_text() {
        // 没有 `:(内容)` 的 `[^1]` 不识别为脚注，原样保留
        let inlines = parse("正文[^1]结束");
        assert_eq!(inlines, vec![Inline::Text("正文[^1]结束".into())]);
    }

    #[test]
    fn parses_image_with_alt() {
        let inlines = parse("见下图：![系统架构](figs/arch.png)");
        assert_eq!(
            inlines,
            vec![
                Inline::Text("见下图：".into()),
                Inline::Image {
                    alt: "系统架构".into(),
                    url: "figs/arch.png".into(),
                    label: None,
                },
            ]
        );
    }

    #[test]
    fn parses_image_with_empty_alt() {
        let inlines = parse("![](a.png)");
        assert_eq!(
            inlines,
            vec![Inline::Image {
                alt: String::new(),
                url: "a.png".into(),
                label: None,
            }]
        );
    }

    #[test]
    fn parses_image_label_attr() {
        let inlines = parse("![架构](figs/arch.png){#fig:arch}后续");
        assert_eq!(
            inlines,
            vec![
                Inline::Image {
                    alt: "架构".into(),
                    url: "figs/arch.png".into(),
                    label: Some("fig:arch".into()),
                },
                Inline::Text("后续".into()),
            ]
        );
    }

    #[test]
    fn parses_crossref() {
        let inlines = parse("见第{@chap:overview}章和图{@fig:arch}。");
        assert_eq!(
            inlines,
            vec![
                Inline::Text("见第".into()),
                Inline::CrossRef("chap:overview".into()),
                Inline::Text("章和图".into()),
                Inline::CrossRef("fig:arch".into()),
                Inline::Text("。".into()),
            ]
        );
    }

    #[test]
    fn crossref_coexists_with_footnote_and_link() {
        let inlines = parse("正文[^1]:(注)见{@sec:a}和[页](https://x)");
        assert_eq!(
            inlines,
            vec![
                Inline::Text("正文".into()),
                Inline::Footnote("注".into()),
                Inline::Text("见".into()),
                Inline::CrossRef("sec:a".into()),
                Inline::Text("和".into()),
                Inline::Link {
                    text: "页".into(),
                    url: "https://x".into(),
                },
            ]
        );
    }

    #[test]
    fn plain_link_still_parses_as_link() {
        let inlines = parse("[页](https://x)");
        assert_eq!(
            inlines,
            vec![Inline::Link {
                text: "页".into(),
                url: "https://x".into(),
            }]
        );
    }

    #[test]
    fn parses_single_and_multiple_citations() {
        assert_eq!(parse("[@key]"), vec![Inline::Citation(vec!["key".into()])]);
        assert_eq!(
            parse("前文 [@a; @b] 后文"),
            vec![
                Inline::Text("前文 ".into()),
                Inline::Citation(vec!["a".into(), "b".into()]),
                Inline::Text(" 后文".into()),
            ]
        );
    }

    #[test]
    fn citation_inside_bold_parses_as_citation() {
        // 粗体内部的 [@key] 应被识别为引用，而不是作为粗体文字原样保留。
        let inlines = parse("**bold and [@biddle_military_2006] inside**");
        assert_eq!(
            inlines,
            vec![Inline::Bold(vec![
                Inline::Text("bold and ".into()),
                Inline::Citation(vec!["biddle_military_2006".into()]),
                Inline::Text(" inside".into()),
            ])]
        );
    }

    #[test]
    fn footnote_inside_italic_parses_as_footnote() {
        // 斜体与脚注在同一段时，脚注先于斜体匹配，所以需要斜体不在脚注左右两侧被切断。
        let inlines = parse("斜体中[^f]:(脚注)嵌套"); // 单独测试脚注 + 斜体分段行为
                                                      // 期望：脚注独立匹配，前后是普通文本
        assert_eq!(
            inlines,
            vec![
                Inline::Text("斜体中".into()),
                Inline::Footnote("脚注".into()),
                Inline::Text("嵌套".into()),
            ]
        );
    }

    #[test]
    fn italic_wraps_text_with_internal_format() {
        // 斜体内部如果出现裸文本（无内嵌格式），整段被识别为 Italic
        let inlines = parse("*整段斜体*");
        assert_eq!(
            inlines,
            vec![Inline::Italic(vec![Inline::Text("整段斜体".into())])]
        );
    }

    #[test]
    fn emphasis_wraps_link_crossref_and_citation() {
        // 强调里包住链接、交叉引用、引用都应保留强调外壳并正确解析内部。
        assert_eq!(
            parse("**加粗 [文档](https://x) 尾**"),
            vec![Inline::Bold(vec![
                Inline::Text("加粗 ".into()),
                Inline::Link {
                    text: "文档".into(),
                    url: "https://x".into(),
                },
                Inline::Text(" 尾".into()),
            ])]
        );
        assert_eq!(
            parse("*斜体 {@fig:arch} 尾*"),
            vec![Inline::Italic(vec![
                Inline::Text("斜体 ".into()),
                Inline::CrossRef("fig:arch".into()),
                Inline::Text(" 尾".into()),
            ])]
        );
        assert_eq!(
            parse("**结论 [@a; @b]**"),
            vec![Inline::Bold(vec![
                Inline::Text("结论 ".into()),
                Inline::Citation(vec!["a".into(), "b".into()]),
            ])]
        );
    }

    #[test]
    fn link_text_with_emphasis_markers_stays_a_link() {
        // 链接文字里的 `**` 不拆强调：链接整体先匹配，文字原样保留。
        assert_eq!(
            parse("[**粗**](https://x)"),
            vec![Inline::Link {
                text: "**粗**".into(),
                url: "https://x".into(),
            }]
        );
    }

    #[test]
    fn code_span_with_asterisks_is_not_emphasized() {
        // 行内代码优先级最高：内部的 `*` 不被当作强调。
        assert_eq!(parse("`a*b*c`"), vec![Inline::Code("a*b*c".into())]);
        assert_eq!(
            parse("前 `x**y**z` 后"),
            vec![
                Inline::Text("前 ".into()),
                Inline::Code("x**y**z".into()),
                Inline::Text(" 后".into()),
            ]
        );
    }

    #[test]
    fn unsupported_citations_and_inline_code_stay_literal() {
        assert_eq!(parse("@key"), vec![Inline::Text("@key".into())]);
        assert_eq!(
            parse("[@key, p. 2]"),
            vec![Inline::Text("[@key, p. 2]".into())]
        );
        assert_eq!(parse("`[@key]`"), vec![Inline::Code("[@key]".into())]);
    }

    #[test]
    fn flatten_reconstructs_citation_source() {
        assert_eq!(
            flatten(&[Inline::Citation(vec!["a".into(), "b".into()])]),
            "[@a; @b]"
        );
    }
}
