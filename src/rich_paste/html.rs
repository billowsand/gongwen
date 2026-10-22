//! 剪贴板 HTML → `anydoc` 的元素树，带预清洗。
//!
//! 剪贴板里的 HTML 与网页上的 HTML 一样脏：不闭合的标签、乱嵌套的行内元素、
//! Office 的条件注释与带命名空间前缀的元素（`<o:p>`）、整页带上的 `<script>` /
//! `<style>`。这里用 `scraper`（内部是 html5ever，也就是浏览器的 HTML5 解析算法）
//! 先把字符串解析成容错的 DOM，再按粘贴的需要走一遍、转成 `anydoc` 的元素树：
//!
//! - 丢掉脚本、样式、页头、注释、`<o:p>` 这类噪音元素（`<style>` 的文本收集起来
//!   当样式表交给 `anydoc`，macOS / Linux 的整页复制才有类名样式可用）；
//! - 丢掉行内样式写着 `mso-hide:all` / `display:none` 的元素（Word 用它藏修订标记）；
//! - **保留** `mso-list:Ignore` 里的文字：那是 Word 自动编号（「一、」「（一）」「1.」），
//!   后面 `structure` 靠它定级；
//! - `class` 里有 `MsoTitle` 的段落当 `<h1>`，`MsoQuote` 当 `<blockquote>`（剪贴板片段
//!   里没有样式表，只能认类名）；
//! - 去掉零宽字符，`&nbsp;` 与全角空格按空格处理，段首缩进在块级元素开头去掉。
//!
//! 节点数与嵌套深度都有上限：超了返回错误，上层退回纯文本粘贴（见 `pipeline`）。
//! 解析本身不 panic——畸形 HTML 由 html5ever 按规范修复。

use anydoc::html::{Attr, Element as DocElement, Node as DocNode};
use scraper::{ElementRef, Html};

/// 最多转换多少个节点（元素 + 文本）。正常公文几十页也就一两万个，8 MiB 的
/// HTML 上限下最坏能到几十万个，这里挡在转换之前。
const MAX_NODES: usize = 200_000;
/// 最大嵌套深度。浏览器整页复制的嵌套可能很深，64 层足够，也保证递归不爆栈。
const MAX_DEPTH: usize = 64;
/// 收集的样式表文本上限：`Stylesheet::add` 要逐条解析，超长的样式表没有价值。
const MAX_CSS_BYTES: usize = 256 * 1024;

/// 转换失败的原因。上层不区分，一律退回纯文本。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HtmlError {
    /// 节点数超限。
    TooManyNodes,
    /// 嵌套层级超限。
    TooDeep,
}

/// 解析结果。
pub(crate) struct Parsed {
    /// 已经清洗过的 `<body>` 元素，直接交给 `anydoc::html::to_document`。
    pub(crate) body: DocElement,
    /// 页面里收集到的样式表文本，可能为空。
    pub(crate) css: String,
    /// 丢掉的图片张数（`<img>` 一个算一张）。
    pub(crate) images: usize,
}

/// 解析剪贴板 HTML。整页与片段都认：没有 `<body>` 时把整棵树当 body。
pub(crate) fn parse(source: &str) -> Result<Parsed, HtmlError> {
    parse_with_limits(source, MAX_NODES, MAX_DEPTH)
}

/// 带上限的解析。测试用小额度验证「超限退回纯文本」，业务代码走 [`parse`]。
pub(super) fn parse_with_limits(
    source: &str,
    nodes: usize,
    depth: usize,
) -> Result<Parsed, HtmlError> {
    let document = Html::parse_document(source);
    let root = document.root_element();
    let body = root
        .descendent_elements()
        .find(|element| element.value().name() == "body")
        .unwrap_or(root);
    let css = collect_css(&root);
    let mut walker = Walker {
        budget: nodes,
        max_depth: depth,
        images: 0,
    };
    let body = walker.element(body, 0)?;
    Ok(Parsed {
        body,
        css,
        images: walker.images,
    })
}

/// 收集 `<style>` 里的文本。Windows 的片段里通常没有，macOS / Linux 的整页有。
fn collect_css(root: &ElementRef<'_>) -> String {
    let mut css = String::new();
    for element in root.descendent_elements() {
        if element.value().name() != "style" || css.len() >= MAX_CSS_BYTES {
            continue;
        }
        for text in element.text() {
            css.push_str(text);
        }
        css.push('\n');
    }
    css
}

struct Walker {
    /// 还能转换多少个节点。
    budget: usize,
    /// 允许的最大嵌套深度。
    max_depth: usize,
    /// 丢掉的图片张数。
    images: usize,
}

impl Walker {
    fn spend(&mut self) -> Result<(), HtmlError> {
        if self.budget == 0 {
            return Err(HtmlError::TooManyNodes);
        }
        self.budget -= 1;
        Ok(())
    }

    /// 把一个 DOM 元素（连同子树）转成 `anydoc` 元素。
    fn element(&mut self, element: ElementRef<'_>, depth: usize) -> Result<DocElement, HtmlError> {
        if depth > self.max_depth {
            return Err(HtmlError::TooDeep);
        }
        self.spend()?;
        let value = element.value();
        let mut local = value.name().to_ascii_lowercase();
        if let Some(renamed) = word_class_tag(&local, value.attr("class")) {
            local = renamed.to_string();
        }
        let attrs = value
            .attrs()
            .map(|(name, value)| Attr {
                ns: None,
                local: name.to_string(),
                value: value.to_string(),
            })
            .collect();
        let mut out = DocElement {
            ns: None,
            local,
            attrs,
            children: Vec::new(),
        };
        // 段首缩进只在该块的第一个文本节点上去掉：`<b> bar</b>` 这种行内元素
        // 前面的空格是有意义的，不能一概删。
        let trim_leading = is_block(&out.local);
        let mut has_content = false;
        for child in element.children() {
            match child.value() {
                scraper::Node::Text(text) => {
                    self.spend()?;
                    let text = normalize_text(text, trim_leading && !has_content);
                    if text.is_empty() {
                        continue;
                    }
                    has_content = true;
                    out.children.push(DocNode::Text(text));
                }
                scraper::Node::Element(_) => {
                    let Some(child) = ElementRef::wrap(child) else {
                        continue;
                    };
                    // 图片一律不取（第一、二期），只数张数写进状态提示。
                    if matches!(child.value().name(), "img" | "image") {
                        self.images += 1;
                        continue;
                    }
                    if should_skip(&child) {
                        continue;
                    }
                    let converted = self.element(child, depth + 1)?;
                    has_content = true;
                    out.children.push(DocNode::Elem(converted));
                }
                // 注释（含 Office 的 `<!--[if !supportLists]-->` 条件注释）、
                // doctype、处理指令都不是正文。
                _ => {}
            }
        }
        Ok(out)
    }
}

/// 这个元素整个丢掉吗。
fn should_skip(element: &ElementRef<'_>) -> bool {
    let value = element.value();
    let name = value.name();
    if noise_tag(name) {
        return true;
    }
    // 带命名空间前缀的 Office 元素（`o:p`、`w:wrap`、`v:shape`）：html5ever 认不出
    // 前缀，元素名里会留下冒号。
    if name.contains(':') {
        return true;
    }
    if value.attr("hidden").is_some() {
        return true;
    }
    value.attr("style").is_some_and(style_is_hidden)
}

/// 不是正文的元素。
fn noise_tag(name: &str) -> bool {
    matches!(
        name,
        "head"
            | "meta"
            | "link"
            | "base"
            | "title"
            | "script"
            | "style"
            | "noscript"
            | "template"
            | "iframe"
            | "object"
            | "embed"
            | "applet"
            | "param"
            | "svg"
            | "canvas"
            | "map"
            | "audio"
            | "video"
            | "source"
            | "track"
            | "input"
            | "button"
            | "select"
            | "option"
            | "textarea"
    )
}

/// Word 用类名表达标题与引用样式（剪贴板片段里没有样式表，`<style>` 拿不到）。
fn word_class_tag(name: &str, class: Option<&str>) -> Option<&'static str> {
    if !matches!(name, "p" | "div") {
        return None;
    }
    let class = class?;
    let classes = class.split_ascii_whitespace();
    for class in classes {
        match class {
            "MsoTitle" => return Some("h1"),
            "MsoQuote" | "MsoIntenseQuote" => return Some("blockquote"),
            _ => {}
        }
    }
    None
}

/// 行内样式是否要求整块藏起来。
fn style_is_hidden(style: &str) -> bool {
    let compact: String = style
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    compact.contains("mso-hide:all") || compact.contains("display:none")
}

/// 块级元素：它的第一个文本节点若是段首缩进，可以安全去掉。
fn is_block(name: &str) -> bool {
    matches!(
        name,
        "body"
            | "p"
            | "div"
            | "section"
            | "article"
            | "aside"
            | "header"
            | "footer"
            | "main"
            | "nav"
            | "figure"
            | "figcaption"
            | "center"
            | "blockquote"
            | "pre"
            | "li"
            | "ul"
            | "ol"
            | "dl"
            | "dt"
            | "dd"
            | "table"
            | "thead"
            | "tbody"
            | "tfoot"
            | "tr"
            | "td"
            | "th"
            | "caption"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "details"
            | "summary"
    )
}

/// 文本节点归一：去掉零宽字符与不可见控制符，`&nbsp;` 与全角空格按空格算。
///
/// 换行与制表符保留：`<pre>` 靠它们分行，普通段落里的空白由 `anydoc` 自己折叠。
fn normalize_text(raw: &str, trim_leading: bool) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '\u{a0}' | '\u{3000}' => out.push(' '),
            // 软连字符、零宽空格、左右书写标记、词连接符、字节序标记。
            '\u{ad}' | '\u{200b}' | '\u{200e}' | '\u{200f}' | '\u{2060}' | '\u{feff}' => {}
            ch if ch.is_control() && ch != '\n' && ch != '\t' => {}
            ch => out.push(ch),
        }
    }
    if trim_leading {
        out.trim_start_matches([' ', '\n', '\t']).to_string()
    } else {
        out
    }
}
