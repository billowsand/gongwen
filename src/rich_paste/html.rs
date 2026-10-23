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
//! - 去掉零宽字符，`&nbsp;` 与全角空格按空格处理，段首缩进在块级元素开头去掉；
//! - 研究报告里把「文框候选」转成引用块：`<aside>`、只有一格的表格（Word 文本框表格）、
//!   带边框或底纹的 `div` / `section`，只要第一段是整段加粗的短标题，就写成
//!   `> [!名称] 标题` 的形式（见 [`rewrite_box`]；非研究报告不做这一步）。
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
    /// 认出来的文框个数（只可能是研究报告那条路）。
    pub(crate) boxes: usize,
    /// 其中名称用了默认「专栏」的个数。
    pub(crate) default_boxes: usize,
}

/// 解析剪贴板 HTML。整页与片段都认：没有 `<body>` 时把整棵树当 body。
///
/// `boxes` 为真时（研究报告）把文框候选转成 `> [!名称] 标题`；别的文种不做，单格
/// 表格照旧由 `syntax` 拆成段落。
pub(crate) fn parse(source: &str, boxes: bool) -> Result<Parsed, HtmlError> {
    parse_with_limits(source, MAX_NODES, MAX_DEPTH, boxes)
}

/// 带上限的解析。测试用小额度验证「超限退回纯文本」，业务代码走 [`parse`]。
pub(super) fn parse_with_limits(
    source: &str,
    nodes: usize,
    depth: usize,
    boxes: bool,
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
        boxes,
        found_boxes: 0,
        default_boxes: 0,
    };
    let body = walker.element(body, 0)?;
    Ok(Parsed {
        body,
        css,
        images: walker.images,
        boxes: walker.found_boxes,
        default_boxes: walker.default_boxes,
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
    /// 要不要把文框候选转成引用块（只有研究报告要）。
    boxes: bool,
    /// 认出来的文框个数与其中默认名称的个数。
    found_boxes: usize,
    default_boxes: usize,
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
        // 研究报告的文框：第一段是整段加粗短标题的 aside / 单格表格 / 有框线的容器。
        // 先试这个，再考虑把只装行内内容的容器改成段落。
        if self.boxes
            && is_box_candidate(&out)
            && let Some(found) = rewrite_box(&mut out)
        {
            self.found_boxes += 1;
            if found.default_named {
                self.default_boxes += 1;
            }
        }
        // 只装着行内内容的 `div` / `section`：浏览器里每个都自成一段，`anydoc` 却把它们
        // 当透明容器，几个挨着的就接成一段（网页、编辑器复制的内容常这样分段）。
        // 改成 `p` 交给它，就是一段一段的。
        if is_leaf_container(&out) {
            out.local = "p".to_string();
        }
        Ok(out)
    }
}

/// 文框首行在模型里的占位：`\u{FDD1}名称\u{FDD2}标题`。
///
/// anydoc 渲染时会转义 `[`（`\[!专栏]`），`mdx::quote` 就认不出文框首行了；所以模型里
/// 先放非字符占位，渲染完再由 [`restore_boxes`] 换成 `[!名称] 标题`。
const BOX_NAME_MARK: char = '\u{FDD1}';
const BOX_TITLE_MARK: char = '\u{FDD2}';

/// 文框名称：标题以这些词开头时用它，标题去掉前缀。其余情况统一写「专栏」。
const BOX_NAMES: &[&str] = &["专栏", "案例", "例子", "做法", "延伸阅读", "知识链接"];

/// 文框候选：网页的 `<aside>`、Word 的「文本框表格」（只有一格的表格）、带边框或
/// 底纹的 `div` / `section`。
fn is_box_candidate(element: &DocElement) -> bool {
    match element.local.as_str() {
        "aside" => true,
        "table" => single_cell(element),
        "div" | "section" => element.attr_any("style").is_some_and(styled_box),
        _ => false,
    }
}

/// 只有一格的表格：Word 的文本框常常是一个 `td` 的表格。
fn single_cell(element: &DocElement) -> bool {
    fn count_cells(element: &DocElement) -> usize {
        let mut count = usize::from(matches!(element.local.as_str(), "td" | "th"));
        for child in &element.children {
            if let DocNode::Elem(child) = child {
                count = count.saturating_add(count_cells(child));
            }
        }
        count
    }
    count_cells(element) == 1
}

/// 行内样式里有**看得见的**边框或底纹：网页与 WPS 用它画「框」。
///
/// 只写了 `border` / `background` 不算：微信公众号几乎每个 `section` 都带
/// `background-color: rgb(255, 255, 255)`，`border: none`、`background: transparent`
/// 更是常见的重置写法，按字面认会把整篇文章的小节都当成文框。
fn styled_box(style: &str) -> bool {
    style.split(';').any(|decl| {
        let Some((name, value)) = decl.split_once(':') else {
            return false;
        };
        let name = name.trim().to_ascii_lowercase();
        let value: String = value
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        if name.starts_with("border") {
            visible_border(&value)
        } else if name == "background" || name == "background-color" {
            visible_background(&value)
        } else {
            false
        }
    })
}

/// 边框要有线型（`solid` 之类），且不是 `none` / `hidden` / 零宽。
fn visible_border(value: &str) -> bool {
    let styled = [
        "solid", "dashed", "dotted", "double", "groove", "ridge", "inset", "outset",
    ]
    .iter()
    .any(|style| value.contains(style));
    styled && !value.contains("none") && !value.contains("hidden") && !value.starts_with('0')
}

/// 底纹要是有颜色的：白色、透明、`none` 与继承都不算。
fn visible_background(value: &str) -> bool {
    const PLAIN: &[&str] = &[
        "none",
        "transparent",
        "initial",
        "inherit",
        "unset",
        "white",
        "#fff",
        "#ffffff",
        "windowbackground",
        "rgb(255,255,255)",
        "rgba(0,0,0,0)",
    ];
    !value.is_empty()
        && !PLAIN.contains(&value)
        && !value.starts_with("rgba(255,255,255,")
        && !value.ends_with(",0)")
}

/// 改写结果：名称用了默认值时要提示一句。
struct BoxFound {
    default_named: bool,
}

/// 满足条件就把候选改写成引用块，首段换成文框标记。
///
/// 条件：第一个子元素是块级元素、整段加粗、文字不长，而且后面还有正文。单格表格先
/// 把单元格里的块取出来（表格本身不留结构）；其余容器直接用它的孩子。
fn rewrite_box(element: &mut DocElement) -> Option<BoxFound> {
    let mut children = if element.local == "table" {
        take_cell_children(element)?
    } else {
        std::mem::take(&mut element.children)
    };
    let head_index = children
        .iter()
        .position(|node| matches!(node, DocNode::Elem(_)))?;
    let DocNode::Elem(head) = &children[head_index] else {
        return None;
    };
    let (name, title, default_named) = box_heading(head)?;
    // 后面还得有正文，否则就是一个加粗段落，不算文框。
    let has_body = children[head_index + 1..]
        .iter()
        .any(|node| node_text(node).trim().chars().count() > 0);
    if !has_body {
        return None;
    }
    children.remove(head_index);
    let marker = if title.is_empty() {
        format!("{BOX_NAME_MARK}{name}")
    } else {
        format!("{BOX_NAME_MARK}{name}{BOX_TITLE_MARK}{title}")
    };
    let mut body = Vec::with_capacity(children.len() + 1);
    body.push(DocNode::Elem(DocElement {
        ns: None,
        local: "p".to_string(),
        attrs: Vec::new(),
        children: vec![DocNode::Text(marker)],
    }));
    body.append(&mut children);
    element.local = "blockquote".to_string();
    element.attrs.clear();
    element.children = body;
    Some(BoxFound { default_named })
}

/// 单格表格里那个单元格的块内容。
fn take_cell_children(element: &mut DocElement) -> Option<Vec<DocNode>> {
    if matches!(element.local.as_str(), "td" | "th") {
        return Some(std::mem::take(&mut element.children));
    }
    for child in &mut element.children {
        if let DocNode::Elem(child) = child
            && let Some(children) = take_cell_children(child)
        {
            return Some(children);
        }
    }
    None
}

/// 首段是不是「整段加粗的短标题」；是就给出文框名称、标题与是否用了默认名称。
fn box_heading(head: &DocElement) -> Option<(String, String, bool)> {
    if !is_block(&head.local) || !entirely_bold(head, false) {
        return None;
    }
    let title = collapse(&text_of(head));
    let length = title.chars().count();
    if length == 0 || length > crate::doc_import::structure::TITLE_MAX {
        return None;
    }
    // 「一、总体要求」「1.1 背景」这类带编号的加粗行是章节标题，不是文框标题：
    // 网页常把一节正文包在一个带底纹的容器里。
    let scheme = crate::doc_import::structure::marker_scheme(&title);
    if crate::doc_import::structure::marker_level(scheme, &title).is_some() {
        return None;
    }
    Some(box_name(&title))
}

/// 标题以「案例」「专栏」这类词开头、后面跟着编号或分隔符时当名称，其余用默认的「专栏」。
/// 「案例分析的三点启示」「做法创新」里的「案例」「做法」是标题的一部分，不能拆。
fn box_name(title: &str) -> (String, String, bool) {
    for name in BOX_NAMES {
        if let Some(rest) = title.strip_prefix(name)
            && let Some(title) = box_label_rest(rest)
        {
            return (name.to_string(), title, false);
        }
    }
    ("专栏".to_string(), title.to_string(), true)
}

/// 名称后面的部分像不像「编号 + 分隔符 + 标题」：「1：××」「一 ××」「：××」「 ××」，或者
/// 什么都没有。像就返回去掉编号与分隔符后的标题，不像返回 `None`。
fn box_label_rest(rest: &str) -> Option<String> {
    const SEPARATORS: [char; 10] = [':', '：', '、', '.', '．', ',', '，', ' ', '\u{3000}', '\t'];
    if rest.trim().is_empty() {
        return Some(String::new());
    }
    let after_space = rest.trim_start_matches([' ', '\u{3000}', '\t']);
    let spaced = after_space.len() != rest.len();
    let after_number = after_space.trim_start_matches(|ch: char| {
        ch.is_ascii_digit()
            || ('０'..='９').contains(&ch)
            || "一二三四五六七八九十百零〇".contains(ch)
    });
    let numbered = after_number.len() != after_space.len();
    let arabic = after_space
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_digit() || ('０'..='９').contains(&ch));
    let after_separator = after_number.trim_start_matches(SEPARATORS);
    let separated = after_separator.len() != after_number.len();
    // 汉字序号（「专栏一」）后面必须有分隔符，否则「专栏一些做法」会被拆开。
    let accepted = spaced || separated || (numbered && arabic) || after_separator.is_empty();
    accepted.then(|| after_separator.trim().to_string())
}

/// 这一段是不是整段加粗：所有非空白文字都落在 `<b>` / `<strong>` / 标题里，或外层
/// 样式写着粗体。
fn entirely_bold(element: &DocElement, inherited: bool) -> bool {
    let mut bold = inherited
        || matches!(
            element.local.as_str(),
            "b" | "strong" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
        );
    if let Some(style) = element.attr_any("style")
        && let Some(value) = styled_bold(style)
    {
        bold = value;
    }
    let mut seen = false;
    for child in &element.children {
        match child {
            DocNode::Text(text) => {
                if text.trim().is_empty() {
                    continue;
                }
                if !bold {
                    return false;
                }
                seen = true;
            }
            DocNode::Elem(child) => {
                // 空元素（`<br>`、只有图片的 span）不算文字，不影响判断。
                if text_of(child).trim().is_empty() {
                    continue;
                }
                if !entirely_bold(child, bold) {
                    return false;
                }
                seen = true;
            }
        }
    }
    seen
}

/// 行内样式里的粗细：`font-weight:600` / `bold` 算粗，`normal` 与小于 600 的数值算不粗。
fn styled_bold(style: &str) -> Option<bool> {
    let compact: String = style
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    let value = compact.split(';').find_map(|decl| {
        decl.strip_prefix("font-weight:")
            .map(|value| value.to_string())
    })?;
    if value.contains("bold") {
        return Some(true);
    }
    if value.contains("normal") || value.contains("lighter") {
        return Some(false);
    }
    value
        .trim_end_matches(['!', 'i', 'm', 'p', 'o', 'r', 't', 'a', 'n', 't'])
        .parse::<u32>()
        .ok()
        .map(|weight| weight >= 600)
}

/// 元素里的全部文字。
fn text_of(element: &DocElement) -> String {
    let mut out = String::new();
    for child in &element.children {
        match child {
            DocNode::Text(text) => out.push_str(text),
            DocNode::Elem(child) => out.push_str(&text_of(child)),
        }
    }
    out
}

/// 节点的文字。
fn node_text(node: &DocNode) -> String {
    match node {
        DocNode::Text(text) => text.clone(),
        DocNode::Elem(element) => text_of(element),
    }
}

/// 空白折成一个空格。
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 把渲染结果里的文框占位换回 `[!名称] 标题`。
pub(crate) fn restore_boxes(markdown: &str) -> String {
    if !markdown.contains(BOX_NAME_MARK) {
        return markdown.to_string();
    }
    let mut out = String::with_capacity(markdown.len());
    for (index, line) in markdown.lines().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&restore_box_line(line));
    }
    if markdown.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn restore_box_line(line: &str) -> String {
    let Some(start) = line.find(BOX_NAME_MARK) else {
        return line.to_string();
    };
    let after = &line[start + BOX_NAME_MARK.len_utf8()..];
    let (name, title) = match after.find(BOX_TITLE_MARK) {
        Some(end) => (
            &after[..end],
            after[end + BOX_TITLE_MARK.len_utf8()..].trim_end(),
        ),
        None => (after.trim_end(), ""),
    };
    let prefix = &line[..start];
    if title.is_empty() {
        format!("{prefix}[!{name}]")
    } else {
        format!("{prefix}[!{name}] {title}")
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

/// 只装着行内内容、要改成段落的容器。`li`、`dt`、`dd` 有自己的含义，`body` 是根，
/// 都不动；判据与 `anydoc` 的 `is_container_tag` / `has_block_children` 对应。
fn is_leaf_container(element: &DocElement) -> bool {
    matches!(
        element.local.as_str(),
        "div"
            | "section"
            | "article"
            | "aside"
            | "main"
            | "nav"
            | "header"
            | "footer"
            | "figure"
            | "figcaption"
            | "center"
            | "details"
            | "summary"
    ) && !element.children.iter().any(|child| {
        matches!(child, DocNode::Elem(child) if is_block(&child.local) || child.local == "hr")
    })
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
    // 「张某　联系电话：」里的分隔空格先护住，下面再把别的全角空格当普通空格。
    let raw = super::gaps::protect(raw);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn element(local: &str, attrs: Vec<(&str, &str)>, children: Vec<DocNode>) -> DocElement {
        DocElement {
            ns: None,
            local: local.to_string(),
            attrs: attrs
                .into_iter()
                .map(|(name, value)| Attr {
                    ns: None,
                    local: name.to_string(),
                    value: value.to_string(),
                })
                .collect(),
            children,
        }
    }

    #[test]
    fn box_names_come_from_the_title_prefix() {
        assert_eq!(
            box_name("案例1：某市做法"),
            ("案例".to_string(), "某市做法".to_string(), false)
        );
        assert_eq!(
            box_name("专栏 新加坡经验"),
            ("专栏".to_string(), "新加坡经验".to_string(), false)
        );
        assert_eq!(
            box_name("延伸阅读：三条路径"),
            ("延伸阅读".to_string(), "三条路径".to_string(), false)
        );
        // 没有前缀：名称用默认「专栏」，标题原样。
        assert_eq!(
            box_name("某市做法"),
            ("专栏".to_string(), "某市做法".to_string(), true)
        );
        // 只有名称没有标题。
        assert_eq!(box_name("做法"), ("做法".to_string(), String::new(), false));
    }

    #[test]
    fn font_weight_parsing_handles_numbers_and_keywords() {
        assert_eq!(styled_bold("font-weight:700"), Some(true));
        assert_eq!(styled_bold("font-weight: 600; color: red"), Some(true));
        assert_eq!(styled_bold("font-weight:normal"), Some(false));
        assert_eq!(styled_bold("font-weight:400"), Some(false));
        assert_eq!(styled_bold("color:red"), None);
        // `!important` 的尾巴不参与数字解析。
        assert_eq!(styled_bold("font-weight:700 !important"), Some(true));
    }

    #[test]
    fn containers_with_border_or_background_are_box_candidates() {
        assert!(styled_box("border:1px solid #999"));
        assert!(styled_box("background-color:#f2f2f2"));
        assert!(!styled_box("color:red;padding:8px"));
        assert!(!is_box_candidate(&element(
            "div",
            vec![("class", "note")],
            vec![]
        )));
        assert!(is_box_candidate(&element(
            "div",
            vec![("style", "border:1px solid")],
            vec![]
        )));
        assert!(is_box_candidate(&element("aside", vec![], vec![])));
        // 两个单元格的表格不是候选。
        let cell = || element("td", vec![], vec![]);
        let row = || craft_row(cell(), cell());
        assert!(!is_box_candidate(&element("table", vec![], vec![row()])));
    }

    fn craft_row(first: DocElement, second: DocElement) -> DocNode {
        DocNode::Elem(element(
            "tr",
            vec![],
            vec![DocNode::Elem(first), DocNode::Elem(second)],
        ))
    }

    #[test]
    fn box_placeholder_becomes_the_mdx_heading() {
        let markdown = "> \u{FDD1}案例\u{FDD2}某市做法\n>\n> 正文\n";
        assert_eq!(restore_boxes(markdown), "> [!案例] 某市做法\n>\n> 正文\n");
        // 没有标题的文框。
        assert_eq!(restore_boxes("> \u{FDD1}专栏\n"), "> [!专栏]\n");
        // 普通正文不动。
        assert_eq!(restore_boxes("正文\n"), "正文\n");
    }

    /// 名称只在后面跟着编号或分隔符时才拆；标题里的字一个都不能丢。
    #[test]
    fn box_names_never_eat_title_characters() {
        // 「中」曾被当成汉字数字剥掉。
        assert_eq!(
            box_name("专栏 中国经验"),
            ("专栏".to_string(), "中国经验".to_string(), false)
        );
        assert_eq!(
            box_name("专栏二：国际比较"),
            ("专栏".to_string(), "国际比较".to_string(), false)
        );
        // 「案例」「做法」是标题的一部分：用默认名称，标题原样。
        for title in ["案例分析的三点启示", "做法创新值得推广", "专栏一些做法"]
        {
            assert_eq!(
                box_name(title),
                ("专栏".to_string(), title.to_string(), true),
                "{title}"
            );
        }
    }

    /// 只有看得见的边框、有颜色的底纹才算「框」。
    #[test]
    fn only_visible_borders_and_backgrounds_make_a_box() {
        assert!(styled_box("border: solid windowtext 1.0pt"));
        assert!(styled_box("border-left: 3px solid #c00"));
        assert!(styled_box("background: #f2f2f2"));
        assert!(styled_box("background-color: rgb(242, 242, 242)"));
        for style in [
            "background-color: rgb(255, 255, 255); margin: 0px",
            "background: transparent",
            "background-color: rgba(0, 0, 0, 0)",
            "border: none",
            "border: 0px solid black",
            "border-image: none",
        ] {
            assert!(!styled_box(style), "{style}");
        }
    }
}
