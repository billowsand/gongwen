//! 中间表示：parser 输出，emitter 消费。
//!
//! 设计原则：尽量贴近 markdown 语义，不附加风格信息。具体编号、字体、章节
//! 形态由 emitter 自行决定，common 不预设公文 / 研报。

pub use super::table::TableSpan;

#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
pub enum Block {
    /// 标题。`level` 是原 markdown 中的 `#` 数量（1..=6）。
    /// `text` 为已去除旧编号、引号已正规化后的纯标题文本，未做行内格式拆分。
    Heading { level: u8, text: String },

    /// 普通段落。Inline 序列表示文本/粗体/斜体片段。
    Paragraph(Vec<Inline>),

    /// 列表项。每个 Markdown 列表行对应一个 Block::List；
    /// `level` 是缩进推断出的层级（1..=6），具体编号循环交给 emitter。
    List {
        ordered: bool,
        level: u8,
        content: Vec<Inline>,
    },

    /// 表格。`rows[0]` 视为表头；分隔行 `|---|` 已被剥离。
    /// `caption` 对应 pandoc table caption（如 `Table: 标题` 或表后 `: 标题`）。
    /// `rows` 是矩形网格；`spans` 是其中的合并单元格（见 `table::parse_cells`），
    /// 被合并掉的格子在 `rows` 里是空串。
    /// `numbered` 是序号表（表前一行 `<!-- [序号表] -->`）：整行合并的分组行
    /// 靠左排，其余与普通表格相同。
    Table {
        rows: Vec<Vec<String>>,
        caption: Option<String>,
        spans: Vec<TableSpan>,
        numbered: bool,
    },

    /// 区段切换标记，由 `<!-- [...] -->` 注释触发。emitter 据此切模式。
    Marker(MarkerKind),

    /// 目录，由独占一行的 `<!-- [目录] -->` 触发：目录排在标记所在的位置。
    /// 不是区段切换，不改变 emitter 的模式；仅研究报告排目录，公文忽略。
    Toc,

    /// 交叉引用锚点，作用于紧随其后的标题或表格块。由标题/表题尾部的
    /// `{#id}` 属性产生（图片标签直接挂在 Inline::Image 上，不经此块）。
    /// 仅 research tex 完整支持；official / docx 忽略。
    Label(String),

    /// 代码块。`lang` 为语言标识（如 "rust", "python"），`content` 为代码内容。
    CodeBlock {
        lang: Option<String>,
        content: String,
    },

    /// 独立成段的 LaTeX 数学公式块，由独占一行的 `$$` 定界。
    /// 保存 `$$` 之间的公式源码原文，不做任何转义或改写。
    /// 仅 research tex 输出 `\[...\]`；official / docx 降级为转义后的源码原文。
    Math(String),

    /// 空行；多数 emitter 直接忽略。
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    /// 加粗。如 `**...**`。内容可以再次包含引用、脚注、嵌套格式。
    Bold(Vec<Inline>),
    /// 斜体。如 `*...*`。内容可以再次包含引用、脚注、嵌套格式。
    Italic(Vec<Inline>),
    /// 行内代码。如 `code`。保留原样，不解析内部标记。
    Code(String),
    /// 链接。如 [text](url)。
    Link {
        text: String,
        url: String,
    },
    /// 图片。如 ![alt](path)。tex emitter 对独占一段的图片输出 figure 环境。
    /// `label` 来自图片后紧跟的 `{#id}` 属性，有 caption 时输出 \label{id}。
    Image {
        alt: String,
        url: String,
        label: Option<String>,
    },
    /// 交叉引用。如 {@chap:overview}，tex 输出 \ref{id}；其他端降级为 id 文本。
    CrossRef(String),
    /// Pandoc 风格方括号文献引用。如 `[@key]` 或 `[@a; @b]`。
    Citation(Vec<String>),
    /// 行内脚注。如 `[^1]:(注释内容)`，仅保存注释内容；编号由输出格式自行生成。
    Footnote(String),
    /// 行内 LaTeX 数学公式。如 `$E=mc^2$`，保存 `$` 之间的公式源码原文。
    /// 仅 research tex 输出 `\(...\)`；official / docx 降级为转义后的源码原文。
    Math(String),
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum MarkerKind {
    /// 摘要段：`<!-- [摘要] -->`
    Abstract,
    /// 附录段：`<!-- [附录] -->`
    Appendix,
    /// 版本变更记录段：`<!-- [版本变更记录] -->`
    Changelog,
    /// 正文段（恢复正常章节计数）：`<!-- [正文] -->`
    Body,
    /// 参考文献段：`<!-- [参考文献] -->`
    Reference,
}

/// 目录是否插在摘要与正文之间：第一个目录标记之前有摘要，且摘要之外只有
/// 报告题名（`#`，不排进正文）、`<!-- [正文] -->`、锚点与空行——摘要到目录
/// 之间没有排出任何正文。这时摘要单用一套小写罗马页码。
///
/// research 的 TeX 与 docx 两条路共用这一个判定，两边页码才对得上。
pub fn toc_follows_abstract(blocks: &[Block]) -> bool {
    let mut seen_abstract = false;
    let mut in_abstract = false;
    for b in blocks {
        match b {
            Block::Toc => return seen_abstract,
            Block::Marker(MarkerKind::Abstract) => {
                seen_abstract = true;
                in_abstract = true;
            }
            Block::Marker(MarkerKind::Body) => in_abstract = false,
            Block::Marker(_) => return false,
            _ if in_abstract => {}
            Block::Empty | Block::Label(_) | Block::Heading { level: 1, .. } => {}
            _ => return false,
        }
    }
    false
}
