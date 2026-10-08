//! Typst 引擎的导出：公文排成 PDF（研究报告见子模块 `research`）。
//!
//! 解析、编号、标题与表格排布的判定都在 Rust 侧，产出给 `assets/typst/gongwen.typ`
//! 的数据（见 `data`），再交给进程内的 `typst_engine` 排版。不生成任何中间文件；
//! 插图直接按用户目录解析、原样嵌入。

pub(crate) mod body;
pub(crate) mod data;
mod frame;
mod math;
pub(crate) mod research;
mod research_body;
mod research_redline;
#[cfg(test)]
mod research_sample_tests;
pub(crate) mod runs;
#[cfg(test)]
mod sample_tests;

use std::path::Path;

use anyhow::{Context, Result};

use crate::models::{DraftInput, FontConfig, NumberingConfig};
use crate::typst_engine::{self, TypstJob, TypstOutcome};
use crate::units::UnitDisplay;
use crate::visual_diff::ElementMarks;
use data::{Block, Doc};

/// 生成模板数据（JSON）。`markdown` 里的 Mermaid 围栏须已物化成图片引用。
/// 只在对照测试里落盘查看用。
#[cfg(test)]
pub(crate) fn document_json(
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    numbering: &NumberingConfig,
    elements: &ElementMarks,
    fonts: typst_engine::FontFamilies,
) -> Result<String> {
    let doc = frame::document(input, markdown, display, numbering, elements, fonts);
    serde_json::to_string(&doc).context("无法序列化 Typst 文档数据")
}

/// 去掉找不到文件的插图，返回被去掉的引用。TeX 路径下缺图同样不进 PDF；这里
/// 不让一张丢失的图拖垮整份排版，改由调用方提示。
fn drop_missing_images(doc: &mut Doc, base_dir: &Path) -> Vec<String> {
    let mut missing = Vec::new();
    let mut keep = |block: &Block| match block {
        Block::Image { src } => {
            let ok = crate::images::resolve_from_base(base_dir, src).is_ok_and(|p| p.is_file());
            if !ok {
                missing.push(src.clone());
            }
            ok
        }
        _ => true,
    };
    doc.body.retain(&mut keep);
    for attachment in &mut doc.attachments {
        attachment.blocks.retain(&mut keep);
    }
    missing
}

/// 排版并写出 PDF。插图按 `base_dir` 解析（正常是用户配置目录）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_pdf_with_base(
    path: &Path,
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    fonts: &FontConfig,
    numbering: &NumberingConfig,
    elements: &ElementMarks,
    base_dir: &Path,
) -> Result<TypstOutcome> {
    let (doc, set, missing) = prepare(
        input, markdown, display, fonts, numbering, elements, base_dir,
    )?;
    let data = serde_json::to_string(&doc).context("无法序列化 Typst 文档数据")?;
    let mut outcome = typst_engine::compile(&TypstJob::official(data, base_dir), &set)?;
    for src in missing {
        outcome
            .warnings
            .insert(0, format!("插图文件不存在，已略过：{src}"));
    }
    std::fs::write(path, &outcome.pdf)
        .with_context(|| format!("无法写入 PDF：{}", path.display()))?;
    Ok(outcome)
}

/// 状态栏按单份稿件数页，份号和版记印数保留，仅省去重复排出的其它份。
pub(crate) fn page_count(
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    fonts: &FontConfig,
    numbering: &NumberingConfig,
) -> Result<usize> {
    let base = crate::storage::config_dir()?;
    page_count_with_base(input, markdown, display, fonts, numbering, &base)
}

fn page_count_with_base(
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    fonts: &FontConfig,
    numbering: &NumberingConfig,
    base: &Path,
) -> Result<usize> {
    let (mut doc, set, _) = prepare(
        input,
        markdown,
        display,
        fonts,
        numbering,
        &Default::default(),
        base,
    )?;
    doc.copies.truncate(1);
    let data = serde_json::to_string(&doc).context("无法序列化 Typst 文档数据")?;
    typst_engine::page_count(&TypstJob::official(data, base), &set)
}

#[cfg(test)]
mod page_count_tests;

#[allow(clippy::too_many_arguments)]
fn prepare(
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    fonts: &FontConfig,
    numbering: &NumberingConfig,
    elements: &ElementMarks,
    base_dir: &Path,
) -> Result<(Doc, typst_engine::FontSet, Vec<String>)> {
    let rendered = crate::mermaid::materialize(
        markdown,
        crate::mermaid::Style::Official,
        crate::mermaid::Format::Pdf,
    )?;
    let set = typst_engine::font_set(fonts)?;
    let mut doc = frame::document(
        input,
        &rendered,
        display,
        numbering,
        elements,
        set.families.clone(),
    );
    let missing = drop_missing_images(&mut doc, base_dir);
    Ok((doc, set, missing))
}

/// 同上，插图按用户配置目录解析。
pub(crate) fn write_pdf(
    path: &Path,
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    fonts: &FontConfig,
    numbering: &NumberingConfig,
    elements: &ElementMarks,
) -> Result<TypstOutcome> {
    let base = crate::storage::config_dir()?;
    write_pdf_with_base(
        path, input, markdown, display, fonts, numbering, elements, &base,
    )
}
