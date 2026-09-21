//! 研究报告 → tex pipeline：纯 Rust 实现，不依赖 pandoc。
//!
//! 整体行为：
//! - 输入可以是单 .md 文件或目录（按文件名升序合并）
//! - 标题层级：# → 文档标题，## → chapter，### → section，#### → subsection，##### → subsubsection
//! - 自动去除标题里的旧编号（parser 层面处理）
//! - 引号正规化为中文双引号（parser 层面处理）
//! - 内嵌 md2tex.cls
//! - 使用 TexResearchEmitter 生成 LaTeX
//! - 分章输出：正文各章写入 data/，附录写入 appendix/，图片复制到 figures/

mod merger;
mod resources;

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use merger::Merger;

/// 入口：调研究报告 tex 转换。input 可以是单 .md 或目录。
pub fn run(
    input: &Path,
    output: Option<&Path>,
    user_template: Option<&Path>,
    compile_pdf: bool,
) -> Result<()> {
    let kind = crate::input::classify(input)?;
    let is_dir = kind == crate::input::InputKind::Directory;

    if let Some(t) = user_template {
        if !t.exists() {
            anyhow::bail!("找不到模板文件 '{}'", t.display());
        }
    }

    let output_path: PathBuf = match output {
        Some(p) => p.to_path_buf(),
        None => crate::input::default_output(input, "tex"),
    };

    println!("开始合并和转换过程...");
    if is_dir {
        println!("输入目录: {}", input.display());
    } else {
        println!("输入文件: {}", input.display());
    }
    println!("输出文件: {}", output_path.display());
    if user_template.is_some() {
        println!("使用外部模板");
    }
    println!("标题映射: # -> title, ## -> chapter, ### -> section, #### -> subsection, ##### -> subsubsection");

    let mut merger = Merger::new();
    merger
        .process(input, is_dir, user_template, &output_path, compile_pdf)
        .context("处理失败")?;

    println!("[完成] 处理完成! 输出文件: {}", output_path.display());
    Ok(())
}
