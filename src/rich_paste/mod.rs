//! 粘贴时把剪贴板里的富文本（HTML）与多行纯文本转成公文助手的 Markdown。
//!
//! 从 Word、WPS、网页、Excel 复制一段内容，剪贴板里同时有纯文本和一份 HTML 排版
//! 副本：前者丢了标题、表格、列表与加粗，后者带着 `<h1>`、`<strong>`、表格、Word
//! 自动编号（`mso-list:Ignore` 里的「一、」）等格式线索。这里优先读 HTML，转换链路是
//!
//! ```text
//! HTML →（html.rs 容错解析 + 预清洗）→ anydoc 元素树
//!      →（anydoc 的 HTML 前端）→ 文档模型
//!      →（syntax.rs 按文种裁剪语法）→ Markdown
//! ```
//!
//! 再交给导入管线已有的 `clean` + `structure` 还原编号与标题层级（见 pipeline）。
//! 只有纯文本时按「一行一段」走同一条结构还原，微信、记事本里复制的「一、……（一）」
//! 因此也能变成标题。
//!
//! 全程只用确定性规则，不调用模型，也不经过任何 AI 代码路径：粘贴是用户的键盘输入。
//! 任何一步失败（没有 HTML、解析超限、转换结果为空）都退回原样粘贴，绝不丢用户的
//! 剪贴板内容。

pub(crate) mod clipboard;
mod gaps;
mod headings;
mod html;
mod lists;
mod pipeline;
#[cfg(test)]
mod samples;
mod syntax;

pub(crate) use clipboard::read_html;

/// 这次粘贴的内容从哪来。
pub(crate) enum Source {
    /// 剪贴板里有 HTML：`plain` 是同一份内容的纯文本形态（`Event::Paste` 带的那份），
    /// 转换失败或用户点「改为原样粘贴」时用它。
    Html { html: String, plain: String },
    /// 只有纯文本。
    Plain(String),
}

impl Source {
    /// 同一份内容的纯文本形态，「改为原样粘贴」用。
    pub(crate) fn plain_text(&self) -> &str {
        match self {
            Source::Html { plain, .. } => plain,
            Source::Plain(text) => text,
        }
    }
}

/// 粘贴发生时的稿子状态，决定结构还原、标题定级与列表整形。
pub(crate) struct Context {
    /// 文种：研究报告保留引用块与公式，白头件与呈批件不收列表。
    pub(crate) kind: crate::models::TemplateKind,
    /// 稿子是空的：相当于整篇复制一份公文，可以剥掉版头版记。
    pub(crate) draft_empty: bool,
    /// 稿子里已经有 `# ` 标题：粘贴的段落不再认成文档标题，也不再出现 `# ` 级标题。
    pub(crate) has_title: bool,
    /// 插入点上方最近一个标题的级数（`# ` = 1）；附件标题也算。没有标题时为 `None`。
    pub(crate) section_level: Option<u8>,
}

impl Context {
    /// 研究报告：保留引用块与公式。由文种推出来，不另存一份状态。
    pub(crate) fn research(&self) -> bool {
        self.kind.is_research()
    }
}

/// 转换成功的结果。
pub(crate) struct Outcome {
    pub(crate) markdown: String,
    pub(crate) stats: Stats,
}

/// 这次转换都得到了什么，用来写状态提示。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Stats {
    pub(crate) headings: usize,
    pub(crate) tables: usize,
    pub(crate) list_items: usize,
    /// 略过的图片张数（第一、二期不随粘贴带入图片）。
    pub(crate) skipped_images: usize,
    /// 原文同级编号跳号的位置（原文编号对，例如「一、」→「三、」）。
    pub(crate) number_jumps: Vec<(String, String)>,
    /// 认出来的文框个数（研究报告）。
    pub(crate) boxes: usize,
    /// 其中名称用了默认「专栏」的个数。
    pub(crate) default_boxes: usize,
}

impl Stats {
    /// 状态栏文案，数量为 0 的项不写。
    ///
    /// `已按格式粘贴：3 个标题、1 张表、5 个列表项（略过 2 张图片）。`
    pub(crate) fn notice(&self) -> String {
        let mut parts = Vec::new();
        if self.headings > 0 {
            parts.push(format!("{} 个标题", self.headings));
        }
        if self.tables > 0 {
            parts.push(format!("{} 张表", self.tables));
        }
        if self.list_items > 0 {
            parts.push(format!("{} 个列表项", self.list_items));
        }
        let mut notice = String::from("已按格式粘贴");
        if !parts.is_empty() {
            notice.push('：');
            notice.push_str(&parts.join("、"));
        }
        if self.skipped_images > 0 {
            notice.push_str(&format!("（略过 {} 张图片）", self.skipped_images));
        }
        notice.push('。');
        if self.default_boxes > 0 {
            notice.push_str(&format!(
                "识别出 {} 个文框，名称暂写「专栏」，请按需改成案例、做法等。",
                self.default_boxes
            ));
        }
        if !self.number_jumps.is_empty() {
            notice.push_str(&jump_notice(&self.number_jumps));
        }
        notice
    }

    /// 行内粘贴（粘在句子中间）的状态文案：只有图片张数值得说。
    pub(crate) fn inline_notice(&self) -> String {
        if self.skipped_images > 0 {
            format!("已按行内格式粘贴（略过 {} 张图片）。", self.skipped_images)
        } else {
            "已按行内格式粘贴。".to_string()
        }
    }
}

/// 跳号提示，最多列三处；再多写「等 N 处」。
fn jump_notice(jumps: &[(String, String)]) -> String {
    const MAX_LISTED: usize = 3;
    let listed: Vec<String> = jumps
        .iter()
        .take(MAX_LISTED)
        .map(|(from, to)| format!("{from}→{to}"))
        .collect();
    let mut text = format!("原文编号有跳号（{}", listed.join("；"));
    if jumps.len() > MAX_LISTED {
        text.push_str(&format!("等 {} 处", jumps.len()));
    }
    text.push_str("），已按顺序重新编号。");
    text
}

/// 把剪贴板内容转成可直接插进编辑框的 Markdown。
///
/// 返回 `None` 表示不必转换：纯文本本来就是 Markdown、转换结果与原文只差空白，
/// 或者富文本这条路失败而纯文本也没什么可还原的。调用方拿到 `None` 就把事件
/// 交还给编辑框，原样粘贴。「内容只有一行」「光标前有字」这两个条件在调用方
/// （起草页的粘贴拦截）判断，这里只管内容本身。
pub(crate) fn convert(source: &Source, ctx: &Context) -> Option<Outcome> {
    pipeline::convert(source, ctx)
}

/// 粘在句子中间（插入点前面已经有字）时用的转换：多行内容压成**一行行内文字**。
///
/// 去掉标题与列表记号，但保留原文编号文字（这时不是在建结构，编号是正文的一部分）、
/// 保留加粗，段落之间直接相连（中文不加空格、西文一个空格）。含表格时返回 `None`，
/// 表格塞不进一行，调用方交还编辑框原样粘贴。
pub(crate) fn convert_inline(source: &Source, ctx: &Context) -> Option<Outcome> {
    pipeline::convert_inline(source, ctx)
}

/// 大段粘贴作为侧栏材料时的正文：有 HTML 就用它转出来的 Markdown，否则原文。
pub(crate) fn material_text(plain: &str) -> String {
    pipeline::material_text(plain)
}

#[cfg(test)]
mod tests;
