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
mod html;
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

/// 粘贴发生时的稿子状态，决定结构还原的几个开关。
pub(crate) struct Context {
    /// 研究报告：保留引用块与公式，其余文种拆成普通段落 / 丢掉。
    pub(crate) research: bool,
    /// 稿子是空的：相当于整篇复制一份公文，可以剥掉版头版记。
    pub(crate) draft_empty: bool,
    /// 稿子里已经有 `# ` 标题：粘贴的段落不再认成文档标题。
    pub(crate) has_title: bool,
}

/// 转换成功的结果。
pub(crate) struct Outcome {
    pub(crate) markdown: String,
    pub(crate) stats: Stats,
}

/// 这次转换都得到了什么，用来写状态提示。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stats {
    pub(crate) headings: usize,
    pub(crate) tables: usize,
    pub(crate) list_items: usize,
    /// 略过的图片张数（第一、二期不随粘贴带入图片）。
    pub(crate) skipped_images: usize,
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
        notice
    }
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

#[cfg(test)]
mod tests;
