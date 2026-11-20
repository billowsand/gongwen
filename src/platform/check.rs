//! 检查公共类型：一条提示、一个来源。
//!
//! validator、proofread、孤行探针、打印检查以后都收成 `Check`；制度失效、
//! 名单倒序、交付检查单也只加规则，不再往 `warnings: Vec<String>` 里塞自由文本。

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckLevel {
    MustFix,
    Suspected,
    Hint,
}

/// 一键改：能自动改的检查才带，不能改的留 `None`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fix {
    pub label: &'static str,
    pub replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Check {
    pub rule: &'static str,
    pub level: CheckLevel,
    pub message: String,
    /// 正文位置；没有对应位置（如文号重号）时为 `None`。
    pub span: Option<Range<usize>>,
    pub fix: Option<Fix>,
}
