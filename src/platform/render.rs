//! 渲染公共类型：一次渲染、一个产物。
//!
//! 文件名、加密、页数、批量合并都走这里；具体 Typst/DOCX/XLSX 适配器在 F3/F5
//! 接入。产物先放内存，打印和加密都不落明文临时文件。

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputFormat {
    Pdf,
    Docx,
    Xlsx,
    TypstPackage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceRef {
    /// 单篇稿件。
    Manuscript(i64),
    /// 多篇合并（文件汇编、会议相关文件）。
    Manuscripts(Vec<i64>),
    /// 外部 PDF，只合并、不重排。
    ExternalPdf(PathBuf),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RenderOptions {
    pub with_toc: bool,
    pub page_numbers: bool,
    pub copies: u32,
    pub header: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Protection {
    Plain,
    /// 独立导出密码。
    Password(String),
    /// 工作区密钥/应用锁保护的导出。
    Workspace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderPlan {
    pub source: SourceRef,
    pub format: OutputFormat,
    pub options: RenderOptions,
    pub protection: Protection,
}

/// 一份成品。`bytes` 在内存里，调用方决定落盘、打印还是加密。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Artifact {
    pub bytes: Vec<u8>,
    pub file_name: String,
    pub pages: Option<u32>,
}

/// 多份成品：文件汇编、会议相关文件、以后会议包都用它。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchPlan {
    pub items: Vec<RenderPlan>,
    pub with_toc: bool,
}
