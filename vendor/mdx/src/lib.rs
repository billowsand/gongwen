use anyhow::Result;
use std::path::{Path, PathBuf};

mod common;
pub mod cover;
mod docx_official;
mod docx_research;
mod input;
mod parser;

/// 研究报告的 Typst 排版数据（公文助手的 Typst 引擎用）。
pub mod typst_research;

/// 表格解析（含 `||` / `^^` 合并单元格）。公开出来供调用方核对：交给 mdx 的
/// 表格源码与自己预览用的网格是否一致。
pub use common::table;

/// 引用块 `>` 的行级识别。公开出来供调用方的预览照同一规则认引文与文框。
pub use common::quote;

/// 研究报告插图的默认宽度。公开出来供调用方的预览照同一规则排图。
pub use common::figure_size;

/// 转换后的目标格式。这份副本只出 Word；PDF 由公文助手的 Typst 引擎排
/// （研究报告的排版数据见 [`typst_research`]）。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    Docx,
}

impl OutputFormat {
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Docx => "docx",
        }
    }
}

/// mdx 内置的文档样式。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DocumentStyle {
    Official,
    Research,
}

/// 一次转换所需的全部参数。
#[derive(Clone, Debug)]
pub struct ConvertRequest {
    pub input: PathBuf,
    pub output: Option<PathBuf>,
    pub format: OutputFormat,
    pub style: DocumentStyle,
}

/// 转换过程中可供前端展示的消息级别。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ProgressLevel {
    Info,
    Warning,
}

/// 转换过程中可供前端展示的消息。
#[derive(Clone, Debug)]
pub struct ProgressEvent {
    pub level: ProgressLevel,
    pub message: String,
}

/// 一次成功转换产生的主要文件。
#[derive(Clone, Debug)]
pub struct ConvertOutcome {
    pub output: PathBuf,
}

/// 使用默认的空进度接收器执行转换。
pub fn convert(request: ConvertRequest) -> Result<ConvertOutcome> {
    convert_with_progress(request, |_| {})
}

/// 执行转换，并将适合 UI 展示的阶段消息发送给调用方。
pub fn convert_with_progress(
    request: ConvertRequest,
    mut report: impl FnMut(ProgressEvent),
) -> Result<ConvertOutcome> {
    input::classify(&request.input)?;

    let output = request
        .output
        .clone()
        .unwrap_or_else(|| input::default_output(&request.input, request.format.extension()));

    report(ProgressEvent {
        level: ProgressLevel::Info,
        message: format!("输入：{}", request.input.display()),
    });
    report(ProgressEvent {
        level: ProgressLevel::Info,
        message: format!("输出：{}", output.display()),
    });
    report(ProgressEvent {
        level: ProgressLevel::Info,
        message: "正在解析 Markdown 并生成文档…".to_owned(),
    });

    match (request.format, request.style) {
        (OutputFormat::Docx, DocumentStyle::Official) => {
            docx_official::run(&request.input, Some(&output))?
        }
        (OutputFormat::Docx, DocumentStyle::Research) => {
            docx_research::run(&request.input, Some(&output))?
        }
    }

    report(ProgressEvent {
        level: ProgressLevel::Info,
        message: "转换完成".to_owned(),
    });

    Ok(ConvertOutcome { output })
}

/// 按 CLI 既有规则计算默认输出文件名。
pub fn default_output(input: &Path, format: OutputFormat) -> PathBuf {
    input::default_output(input, format.extension())
}
