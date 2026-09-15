//! DOCX 图片读取、格式转换和等比缩放。

use anyhow::{Context, Result};
use docx_rs::Pic;
use image::GenericImageView;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Word DrawingML 使用 EMU；docx-rs 按 96 DPI 把像素换算为 9525 EMU。
pub const EMU_PER_PIXEL: u32 = 9_525;
const PDF_DPI: u32 = 200;

/// 读取本地图片，统一转成 DOCX 可稳定嵌入的 PNG，并在超宽时等比缩放。
pub fn load(url: &str, base_dir: &Path, max_width_emu: u32) -> Result<Pic> {
    if url.starts_with("http://") || url.starts_with("https://") {
        anyhow::bail!("DOCX 暂不下载远程图片 '{url}'");
    }

    let (path_url, pdf_page) = split_pdf_page(url)?;
    let path = resolve_path(path_url, base_dir);
    let bytes = if is_pdf(&path) {
        rasterize_pdf_page(&path, pdf_page.unwrap_or(1))?
    } else {
        fs::read(&path).with_context(|| format!("读取图片 {} 失败", path.display()))?
    };
    let image = image::load_from_memory(&bytes)
        .with_context(|| format!("无法识别图片格式 {}", path.display()))?;
    let (width_px, height_px) = image.dimensions();
    if width_px == 0 || height_px == 0 {
        anyhow::bail!("图片尺寸无效 {}", path.display());
    }

    let mut png = Cursor::new(Vec::new());
    image
        .write_to(&mut png, image::ImageFormat::Png)
        .with_context(|| format!("转换图片 {} 为 PNG 失败", path.display()))?;

    let mut pic = Pic::new_with_dimensions(png.into_inner(), width_px, height_px);
    let (width_emu, height_emu) = pic.size;
    if width_emu > max_width_emu {
        let scaled_height =
            ((u64::from(height_emu) * u64::from(max_width_emu)) / u64::from(width_emu)) as u32;
        pic = pic.size(max_width_emu, scaled_height.max(1));
    }
    Ok(pic)
}

/// PDF 图片默认使用第一页；`file.pdf#page=2` 可选择其他页。
fn split_pdf_page(url: &str) -> Result<(&str, Option<u32>)> {
    let Some((path, page_text)) = url.rsplit_once("#page=") else {
        return Ok((url, None));
    };
    let page = page_text
        .parse::<u32>()
        .with_context(|| format!("PDF 页码无效 '{page_text}'（来源：{url}）"))?;
    if page == 0 {
        anyhow::bail!("PDF 页码必须从 1 开始（来源：{url}）");
    }
    Ok((path, Some(page)))
}

fn is_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

/// 使用 Poppler 把指定 PDF 页面栅格化为 PNG。优先 pdftocairo，缺失或失败时
/// 回退到 pdftoppm；命令参数独立传递，路径中的空格不会被 shell 重新解释。
fn rasterize_pdf_page(path: &Path, page: u32) -> Result<Vec<u8>> {
    if !path.is_file() {
        anyhow::bail!("读取 PDF 图片 {} 失败：文件不存在", path.display());
    }

    let temp_dir = tempfile::tempdir().context("创建 PDF 图片转换临时目录失败")?;
    let prefix = temp_dir.path().join("page");
    let page_arg = page.to_string();
    let dpi_arg = PDF_DPI.to_string();
    let renderers: [(&str, Vec<&std::ffi::OsStr>); 2] = [
        (
            "pdftocairo",
            vec![
                std::ffi::OsStr::new("-f"),
                std::ffi::OsStr::new(&page_arg),
                std::ffi::OsStr::new("-l"),
                std::ffi::OsStr::new(&page_arg),
                std::ffi::OsStr::new("-singlefile"),
                std::ffi::OsStr::new("-png"),
                std::ffi::OsStr::new("-r"),
                std::ffi::OsStr::new(&dpi_arg),
                path.as_os_str(),
                prefix.as_os_str(),
            ],
        ),
        (
            "pdftoppm",
            vec![
                std::ffi::OsStr::new("-f"),
                std::ffi::OsStr::new(&page_arg),
                std::ffi::OsStr::new("-l"),
                std::ffi::OsStr::new(&page_arg),
                std::ffi::OsStr::new("-singlefile"),
                std::ffi::OsStr::new("-r"),
                std::ffi::OsStr::new(&dpi_arg),
                std::ffi::OsStr::new("-png"),
                path.as_os_str(),
                prefix.as_os_str(),
            ],
        ),
    ];

    let mut failures = Vec::new();
    for (program, args) in renderers {
        match Command::new(program).args(args).output() {
            Ok(output) if output.status.success() => {
                let png_path = prefix.with_extension("png");
                return fs::read(&png_path).with_context(|| {
                    format!(
                        "{program} 已完成但未生成 PDF 第 {page} 页图片 {}",
                        png_path.display()
                    )
                });
            }
            Ok(output) => failures.push(renderer_failure(program, &output)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                failures.push(format!("{program} 未安装或不在 PATH 中"));
            }
            Err(error) => failures.push(format!("启动 {program} 失败：{error}")),
        }
    }

    anyhow::bail!(
        "PDF 图片 {} 第 {} 页转 PNG 失败：{}",
        path.display(),
        page,
        failures.join("；")
    )
}

fn renderer_failure(program: &str, output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    if detail.is_empty() {
        format!("{program} 退出码 {}", output.status)
    } else {
        format!("{program}：{detail}")
    }
}

fn resolve_path(url: &str, base_dir: &Path) -> PathBuf {
    let clean = url.trim().trim_matches(|ch| ch == '<' || ch == '>');
    let path = Path::new(clean);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_and_scales_png() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wide.png");
        let image = image::DynamicImage::new_rgb8(100, 20);
        image.save(&path).unwrap();

        let pic = load("wide.png", dir.path(), 10 * EMU_PER_PIXEL).unwrap();
        assert_eq!(pic.size, (10 * EMU_PER_PIXEL, 2 * EMU_PER_PIXEL));
        assert!(!pic.image.is_empty());
    }

    #[test]
    fn missing_image_is_an_error() {
        let error = load("missing.png", Path::new("."), 100).unwrap_err();
        assert!(error.to_string().contains("读取图片"));
    }

    #[test]
    fn parses_pdf_page_fragment() {
        assert_eq!(
            split_pdf_page("figures/a.pdf").unwrap(),
            ("figures/a.pdf", None)
        );
        assert_eq!(
            split_pdf_page("figures/a.pdf#page=3").unwrap(),
            ("figures/a.pdf", Some(3))
        );
        assert!(split_pdf_page("a.pdf#page=0").is_err());
        assert!(split_pdf_page("a.pdf#page=x").is_err());
    }

    #[test]
    fn recognizes_pdf_extension_case_insensitively() {
        assert!(is_pdf(Path::new("figure.PDF")));
        assert!(!is_pdf(Path::new("figure.png")));
    }
}
