use anyhow::Result;
use std::path::{Path, PathBuf};

mod common;
mod docx_official;
mod docx_research;
mod input;
mod parser;
mod tex_compile;
mod tex_official;
mod tex_research;
mod tex_research_emitter;

/// 转换后的目标格式。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    Docx,
    Tex,
}

impl OutputFormat {
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Docx => "docx",
            Self::Tex => "tex",
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
    pub template: Option<PathBuf>,
    pub compile_pdf: bool,
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
    pub pdf: Option<PathBuf>,
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

    if request.style == DocumentStyle::Official && request.template.is_some() {
        report(ProgressEvent {
            level: ProgressLevel::Warning,
            message: "公文 TeX 不支持自定义模板，已忽略该设置".to_owned(),
        });
    }

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
        (OutputFormat::Tex, DocumentStyle::Official) => {
            tex_official::run(&request.input, Some(&output), request.compile_pdf)?
        }
        (OutputFormat::Tex, DocumentStyle::Research) => tex_research::run(
            &request.input,
            Some(&output),
            request.template.as_deref(),
            request.compile_pdf,
        )?,
    }

    let pdf = (request.format == OutputFormat::Tex)
        .then(|| output.with_extension("pdf"))
        .filter(|path| path.exists());

    report(ProgressEvent {
        level: ProgressLevel::Info,
        message: "转换完成".to_owned(),
    });

    Ok(ConvertOutcome { output, pdf })
}

/// 按 CLI 既有规则计算默认输出文件名。
pub fn default_output(input: &Path, format: OutputFormat) -> PathBuf {
    input::default_output(input, format.extension())
}
