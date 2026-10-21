//! 从 PDF 自身读两个打印相关的版式事实：是不是整份横排、第 1 页有没有明显的红色。
//!
//! 调用方知道的版式（稿件按双面排、送批材料）由 [`super::PrintHints`] 带进来；
//! 这两项只能从文件里测。检查与平台无关，Windows 与 CUPS 共用；只用来给默认值与
//! 提醒，读不出来就当不知道，绝不拦住打印。

use hayro::{
    RenderCache, RenderSettings,
    hayro_interpret::{InterpreterSettings, hayro_syntax::Pdf},
    hayro_syntax::page::{Page, Rotation},
    vello_cpu::color::palette::css::WHITE,
};
use std::path::Path;

/// 第 1 页按这个分辨率光栅化来数红色像素。36 DPI 的 A4 只有 298 × 421，一枚
/// 直径 4 cm 的印章仍有约 2500 个像素，判断足够；又不必为一个提示把整页按
/// 300 DPI 渲一遍。
const INSPECT_DPI: f32 = 36.0;
/// 「明显的红色」：红头文件的正红、印章的朱红都在这个范围里，灰黑文字与
/// 彩色照片的相近色不会同时满足三个条件。
const RED_MIN_RED: u8 = 180;
const RED_MAX_GREEN: u8 = 90;
const RED_MAX_BLUE: u8 = 90;
/// 红像素达到这个数才算有红色版头 / 印章。36 DPI 下 4 cm 的印章约 2500 像素，
/// 被文字盖掉一半也远大于 50；这个下限只挡彩色插图边沿与扫描噪点。
const RED_PIXEL_THRESHOLD: usize = 50;

/// 从 PDF 里测得的两项版式事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PdfFacts {
    /// 每一页的 `/Rotate` 都是 90 或 270：整份横排。
    pub(crate) all_rotated: bool,
    /// 第 1 页有明显的红色（红色版头或印章）。
    pub(crate) has_red: bool,
}

/// 读这两项事实。打不开、超过体积上限都返回 `None`：不知道就当没有。
pub(crate) fn inspect_pdf(path: &Path) -> Option<PdfFacts> {
    // 与打印走同一条体积上限，别为了一个提示把 200 MB 的扫描件读进内存。
    super::check_size(path).ok()?;
    let bytes =
        crate::file_limits::read_bytes(path, crate::file_limits::MAX_PDF_BYTES, "要打印的 PDF")
            .ok()?;
    let pdf = Pdf::new(bytes).ok()?;
    let pages = pdf.pages();
    let all_rotated = !pages.is_empty()
        && pages.iter().all(|page| {
            matches!(
                page.rotation(),
                Rotation::Horizontal | Rotation::FlippedHorizontal
            )
        });
    let has_red = pages.first().is_some_and(has_red_header);
    Some(PdfFacts {
        all_rotated,
        has_red,
    })
}

/// 第 1 页按 [`INSPECT_DPI`] 光栅化，数「明显的红色」像素。底色是白纸，
/// hayro 给的是不透明 RGBA，红绿蓝三个字节可直接比。
fn has_red_header(page: &Page<'_>) -> bool {
    let (display_w, _) = page.render_dimensions();
    let scale = INSPECT_DPI / 72.0;
    // 夹在 u16 里：hayro 的渲染宽度是 u16，极端尺寸的页面也不该 panic。
    let width = (display_w * scale).round().clamp(1.0, f32::from(u16::MAX)) as u16;
    let cache = RenderCache::new();
    let pixmap = hayro::render(
        page,
        &cache,
        &InterpreterSettings::default(),
        &RenderSettings {
            x_scale: scale,
            y_scale: scale,
            width: Some(width),
            bg_color: WHITE,
            ..Default::default()
        },
    );
    pixmap
        .data_as_u8_slice()
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| px[0] >= RED_MIN_RED && px[1] <= RED_MAX_GREEN && px[2] <= RED_MAX_BLUE)
        .take(RED_PIXEL_THRESHOLD)
        .count()
        >= RED_PIXEL_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// 手写一份最小 PDF：两页 A4，可以都带 `/Rotate 90`，第 1 页可以整页填红。
    ///
    /// 不用 Typst 排版来造样本：那要本机字体（CI 与干净环境没有），也不能提交
    /// 二进制 PDF。手写 PDF 是纯文本，xref 偏移在测试里现算。
    fn write_pdf(dir: &Path, rotated: bool, red: bool) -> PathBuf {
        let rotate = if rotated { " /Rotate 90" } else { "" };
        let red_stream = "1 0 0 rg 0 0 595.28 841.89 re f\n";
        let white_stream = "1 1 1 rg 0 0 595.28 841.89 re f\n";
        let streams = [if red { red_stream } else { white_stream }, white_stream];
        let mut objects: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_vec(),
        ];
        for index in 0..streams.len() {
            objects.push(
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595.28 841.89]{rotate} \
                     /Contents {} 0 R /Resources << >> >>",
                    5 + index
                )
                .into_bytes(),
            );
        }
        for stream in streams {
            objects.push(
                format!("<< /Length {} >>\nstream\n{stream}endstream", stream.len()).into_bytes(),
            );
        }

        let mut out: Vec<u8> = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::with_capacity(objects.len());
        for (index, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }
        let size = objects.len() + 1;
        let xref_offset = out.len();
        out.extend_from_slice(format!("xref\n0 {size}\n").as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for offset in offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n")
                .as_bytes(),
        );
        let path = dir.join(if rotated { "rotated.pdf" } else { "plain.pdf" });
        std::fs::write(&path, out).expect("写测试 PDF");
        path
    }

    #[test]
    fn a_rotated_red_document_reports_both_facts() {
        let dir = tempfile::tempdir().expect("临时目录");
        let facts = inspect_pdf(&write_pdf(dir.path(), true, true)).expect("应能解析");
        assert!(facts.all_rotated);
        assert!(facts.has_red);
    }

    #[test]
    fn a_plain_white_document_reports_neither_fact() {
        let dir = tempfile::tempdir().expect("临时目录");
        let facts = inspect_pdf(&write_pdf(dir.path(), false, false)).expect("应能解析");
        assert!(!facts.all_rotated);
        assert!(!facts.has_red);
    }

    #[test]
    fn broken_or_missing_files_return_none_instead_of_panicking() {
        let dir = tempfile::tempdir().expect("临时目录");
        let broken = dir.path().join("broken.pdf");
        std::fs::write(&broken, b"not a pdf").expect("写坏样本");
        assert_eq!(inspect_pdf(&broken), None);
        assert_eq!(inspect_pdf(&dir.path().join("missing.pdf")), None);
    }
}
