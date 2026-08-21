//! 把 PDF 送上打印机。
//!
//! 三个平台分两条路：
//!
//! - Windows 不能指望第三方 PDF 软件：Edge、WPS、Chrome 这些常见默认阅读器都不
//!   注册 shell 的 print 谓词，`Start-Process -Verb Print` 会直接报错；就算谓词
//!   存在，也是绕过打印对话框直冲默认打印机。这里改为应用内用 hayro 把页面光栅
//!   化，经 PrintDlg 原生对话框（可选打印机、页码范围、份数）加 GDI 送打印，
//!   跟机器上装没装 PDF 阅读器完全无关。
//! - Unix（含 macOS 与麒麟等国产系统）直接交给 CUPS 的 `lp`：它原生吃 PDF，由
//!   CUPS 自己渲染，质量比位图好；`lp` 不在（Linux 没装 cups-client）时按钮置灰。

use std::path::Path;

/// 打印任务的结果。「打完了」和「用户取消」是两回事，状态栏文案要分开。
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrintOutcome {
    /// 打印数据已完整交给系统。
    Printed,
    /// 用户在打印对话框里点了取消。不是错误，不该报红。
    Cancelled,
}

/// 本机是否具备打印能力。不具备时界面把「打印」按钮置灰，并引导改用
/// 「系统打开」后自行打印，而不是让人点了没反应。
///
/// Windows 走原生 GDI 打印，有没有打印机由打印对话框自己呈现，恒可按。
/// Unix 统一看 CUPS 的 `lp` 在不在 PATH 里：macOS 自带，Linux 由
/// `cups-client` 提供，deb 包已把它列入依赖，但用户自行编译或用便携包时
/// 仍可能缺。
pub(crate) fn printing_available() -> bool {
    #[cfg(windows)]
    {
        true
    }
    #[cfg(unix)]
    {
        lp_program().is_some()
    }
}

/// 打印一份 PDF。阻塞调用（对话框 + 逐页光栅化），调用方必须放后台线程。
pub(crate) fn print_pdf(path: &Path) -> Result<PrintOutcome, String> {
    #[cfg(windows)]
    {
        windows::print_pdf(path)
    }
    #[cfg(unix)]
    {
        unix::print_pdf(path)
    }
}

/// 把页面按纵横比放进可打印区域并居中，返回目标矩形 (x, y, w, h)，单位是
/// 设备像素。PDF 自身的页边距已经排在版面上，这里不再加边距。
#[cfg_attr(not(windows), allow(dead_code))]
fn fit_rect(src_w: i32, src_h: i32, area_w: i32, area_h: i32) -> (i32, i32, i32, i32) {
    let scale = f64::from(area_w) / f64::from(src_w).max(1.0);
    let scale = scale.min(f64::from(area_h) / f64::from(src_h).max(1.0));
    let w = (f64::from(src_w) * scale).round().max(1.0) as i32;
    let h = (f64::from(src_h) * scale).round().max(1.0) as i32;
    ((area_w - w) / 2, (area_h - h) / 2, w, h)
}

/// hayro 给的是预乘 RGBA，GDI 的 32 位 DIB 要 BGRA。底色是白纸，alpha 恒 255。
#[cfg_attr(not(windows), allow(dead_code))]
fn rgba_to_bgra(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.as_chunks::<4>().0 {
        out.extend_from_slice(&[px[2], px[1], px[0], 255]);
    }
    out
}

#[cfg(unix)]
fn lp_program() -> Option<std::path::PathBuf> {
    lp_in_path(&std::env::var_os("PATH")?)
}

/// 在给定的 PATH 串里找 `lp`。与环境变量解耦，便于单测。
#[cfg(unix)]
fn lp_in_path(path: &std::ffi::OsStr) -> Option<std::path::PathBuf> {
    std::env::split_paths(path)
        .map(|dir| dir.join("lp"))
        .find(|candidate| candidate.is_file())
}

#[cfg(unix)]
mod unix {
    use super::{PrintOutcome, lp_program};
    use std::path::Path;

    /// 交给 CUPS 的 `lp`——它原生就吃 PDF，成败看退出码。`lp` 自己的报错（没有
    /// 默认打印机之类）比我们编的话准确，所以原样转出去。
    pub(super) fn print_pdf(path: &Path) -> Result<PrintOutcome, String> {
        let program = lp_program()
            .ok_or_else(|| "未找到 lp 命令，本机可能没有安装 CUPS 打印服务".to_string())?;
        // `--` 之后即使文件名以短横线开头也不会被当成选项。
        let output = std::process::Command::new(program)
            .arg("--")
            .arg(path)
            .output()
            .map_err(|error| format!("无法启动打印：{error}"))?;
        if output.status.success() {
            return Ok(PrintOutcome::Printed);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        Err(if detail.is_empty() {
            "打印命令执行失败，请检查是否已设置默认打印机".to_string()
        } else {
            detail.to_string()
        })
    }
}

#[cfg(windows)]
mod windows {
    use super::{PrintOutcome, fit_rect, rgba_to_bgra};
    use hayro::{
        RenderCache, RenderSettings,
        hayro_interpret::{InterpreterSettings, hayro_syntax::Pdf},
        vello_cpu::color::palette::css::WHITE,
    };
    use std::path::Path;
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteDC, GDI_ERROR, GetDeviceCaps,
        HDC, HORZRES, LOGPIXELSX, SRCCOPY, StretchDIBits, VERTRES,
    };
    use windows_sys::Win32::Storage::Xps::{
        AbortDoc, DOCINFOW, EndDoc, EndPage, StartDocW, StartPage,
    };
    use windows_sys::Win32::UI::Controls::Dialogs::{
        CommDlgExtendedError, PD_PAGENUMS, PD_RETURNDC, PD_USEDEVMODECOPIESANDCOLLATE, PRINTDLGW,
        PrintDlgW,
    };

    /// 光栅化的分辨率上限（DPI）。300 DPI 打印公文文字已经足够清晰，再高只是把
    /// 位图撑大：600 DPI 的 A4 一页 34MiB，上百页的汇编能吃掉几个 GiB 内存。
    /// 超出部分交给 GDI 拉伸，文字边沿由打印机插值。
    const MAX_PRINT_DPI: i32 = 300;

    /// 打印对话框返回的 DC，用完必须 DeleteDC 归还。
    struct DcGuard(HDC);
    impl Drop for DcGuard {
        fn drop(&mut self) {
            unsafe { DeleteDC(self.0) };
        }
    }

    /// StartDoc 之后任何一步失败都要 AbortDoc，否则打印队列里会卡着一个半成品
    /// 任务，把后面所有文档堵住。
    struct DocGuard {
        dc: HDC,
        finished: bool,
    }
    impl DocGuard {
        /// 中途失败：作废任务并报告原因。
        fn abort(mut self, reason: &str) -> String {
            // Drop 会补 AbortDoc。
            self.finished = false;
            reason.to_string()
        }
    }
    impl Drop for DocGuard {
        fn drop(&mut self) {
            if !self.finished {
                unsafe { AbortDoc(self.dc) };
            }
        }
    }

    pub(super) fn print_pdf(path: &Path) -> Result<PrintOutcome, String> {
        let bytes =
            std::fs::read(path).map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
        let pdf = Pdf::new(bytes).map_err(|error| format!("PDF 文件结构无法解析：{error:?}"))?;
        let pages = pdf.pages();
        // 先取全部页面的点尺寸：既是页数，也是逐页光栅化的比例基准。
        let page_sizes: Vec<(f32, f32)> =
            pages.iter().map(|page| page.render_dimensions()).collect();
        if page_sizes.is_empty() {
            return Err("PDF 中没有可打印的页面".to_string());
        }
        // PrintDlg 的页码字段是 u16，超出部分按最大页截断。
        let max_page = page_sizes.len().min(usize::from(u16::MAX)) as u16;

        // SAFETY: 本模块整体是不安全代码块包裹的 Win32 调用；逐个调用处的约定在
        // 注释里说明。PrintDlgW 我们持有 pd 的全部所有权，输出句柄按文档归还。
        unsafe {
            let mut pd = PRINTDLGW {
                lStructSize: size_of::<PRINTDLGW>() as u32,
                Flags: PD_RETURNDC | PD_USEDEVMODECOPIESANDCOLLATE,
                nMinPage: 1,
                nMaxPage: max_page,
                nFromPage: 1,
                nToPage: max_page,
                nCopies: 1,
                ..Default::default()
            };
            // hwndOwner 留空：对话框独立出现在任务栏，不归档到主窗口。打印跑在
            // 后台线程，拿不到也不该阻塞主窗口。
            if PrintDlgW(&mut pd) == 0 {
                let code = CommDlgExtendedError();
                if code == 0 {
                    return Ok(PrintOutcome::Cancelled);
                }
                return Err(format!(
                    "无法打开打印对话框（错误码 {code}），请确认已安装打印机"
                ));
            }
            // PD_RETURNDC 模式下这两个句柄通常为空，非空就要 GlobalFree 归还。
            if !pd.hDevMode.is_null() {
                GlobalFree(pd.hDevMode);
            }
            if !pd.hDevNames.is_null() {
                GlobalFree(pd.hDevNames);
            }
            if pd.hDC.is_null() {
                return Err("未能从打印对话框取得打印机上下文".to_string());
            }
            let dc = DcGuard(pd.hDC);

            let printer_dpi = GetDeviceCaps(dc.0, LOGPIXELSX as i32);
            let area_w = GetDeviceCaps(dc.0, HORZRES as i32);
            let area_h = GetDeviceCaps(dc.0, VERTRES as i32);
            if printer_dpi <= 0 || area_w <= 0 || area_h <= 0 {
                return Err("无法读取打印机的分辨率与可打印区域".to_string());
            }
            let raster_dpi = printer_dpi.clamp(96, MAX_PRINT_DPI);

            // PD_PAGENUMS 出现在返回值里，说明用户选了「页码范围」而非「全部」。
            let (from, to) = if pd.Flags & PD_PAGENUMS != 0 {
                (
                    pd.nFromPage.clamp(1, max_page),
                    pd.nToPage.clamp(1, max_page),
                )
            } else {
                (1, max_page)
            };
            if from > to {
                return Err("页码范围无效".to_string());
            }
            // 带 PD_USEDEVMODECOPIESANDCOLLATE 时，驱动能自己印多份就会把它留成 1
            // 并把份数写进 DEVMODE；大于 1 才需要我们逐份重复（按页序=自动分页）。
            let copies = pd.nCopies.max(1);

            let doc_name: Vec<u16> = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "PDF".to_string())
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let info = DOCINFOW {
                cbSize: size_of::<DOCINFOW>() as i32,
                lpszDocName: doc_name.as_ptr(),
                ..Default::default()
            };
            if StartDocW(dc.0, &info) <= 0 {
                return Err("打印机拒绝了打印任务，请检查打印机状态".to_string());
            }
            let mut doc = DocGuard {
                dc: dc.0,
                finished: false,
            };

            // 字体与图像的解码结果落在 cache 里全程复用；每页新建等于每页重新
            // 解码一遍嵌入的中文字体。
            let cache = RenderCache::new();
            for _copy in 0..copies {
                for page_num in from..=to {
                    let index = usize::from(page_num - 1);
                    let Some(page) = pages.get(index) else {
                        continue;
                    };
                    let (page_width, _) = page_sizes[index];
                    let target = ((page_width / 72.0) * raster_dpi as f32).round();
                    let width = target.clamp(1.0, f32::from(u16::MAX)) as u16;
                    let scale = f32::from(width) / page_width.max(1.0);
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
                    let src_w = i32::from(pixmap.width());
                    let src_h = i32::from(pixmap.height());
                    let bgra = rgba_to_bgra(pixmap.data_as_u8_slice());
                    let (dx, dy, dw, dh) = fit_rect(src_w, src_h, area_w, area_h);
                    // 负高度 = 自顶向下的 DIB，行序与 hayro 的输出一致。
                    let bmi = BITMAPINFO {
                        bmiHeader: BITMAPINFOHEADER {
                            biSize: size_of::<BITMAPINFOHEADER>() as u32,
                            biWidth: src_w,
                            biHeight: -src_h,
                            biPlanes: 1,
                            biBitCount: 32,
                            biCompression: BI_RGB,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    if StartPage(dc.0) <= 0 {
                        return Err(doc.abort("打印机在换页时报错，任务已取消"));
                    }
                    let lines = StretchDIBits(
                        dc.0,
                        dx,
                        dy,
                        dw,
                        dh,
                        0,
                        0,
                        src_w,
                        src_h,
                        bgra.as_ptr().cast(),
                        &bmi,
                        DIB_RGB_COLORS,
                        SRCCOPY,
                    );
                    if lines == GDI_ERROR || lines == 0 {
                        return Err(doc.abort("写入打印数据失败，任务已取消"));
                    }
                    if EndPage(dc.0) <= 0 {
                        return Err(doc.abort("打印机在结束页面时报错，任务已取消"));
                    }
                }
            }
            if EndDoc(dc.0) <= 0 {
                return Err(doc.abort("收尾打印任务失败"));
            }
            doc.finished = true;
            Ok(PrintOutcome::Printed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_rect_fills_width_when_page_is_wide() {
        // 横页放竖纸：宽度顶满，高度按比例，垂直居中。
        let (x, y, w, h) = fit_rect(2000, 1000, 1000, 1400);
        assert_eq!((x, w), (0, 1000));
        assert_eq!(h, 500);
        assert_eq!(y, (1400 - 500) / 2);
    }

    #[test]
    fn fit_rect_fills_height_when_page_is_tall() {
        // 瘦长页：高度顶满，横向居中。
        let (x, y, w, h) = fit_rect(500, 2000, 1000, 1400);
        assert_eq!((y, h), (0, 1400));
        assert_eq!(w, 350);
        assert_eq!(x, (1000 - 350) / 2);
    }

    #[test]
    fn fit_rect_never_collapses_to_zero() {
        let (_, _, w, h) = fit_rect(1, 1, 1, 1);
        assert!(w >= 1 && h >= 1);
    }

    #[test]
    fn rgba_to_bgra_swaps_red_and_blue_and_forces_opaque() {
        let bgra = rgba_to_bgra(&[10, 20, 30, 40, 200, 100, 50, 0]);
        assert_eq!(bgra, vec![30, 20, 10, 255, 50, 100, 200, 255]);
    }

    #[cfg(unix)]
    #[test]
    fn lp_is_found_only_when_it_exists_on_the_path() {
        use std::os::unix::fs::PermissionsExt;

        let empty = tempfile::tempdir().expect("临时目录");
        let with_lp = tempfile::tempdir().expect("临时目录");
        let lp = with_lp.path().join("lp");
        std::fs::write(&lp, "#!/bin/sh\n").expect("写入假 lp");
        std::fs::set_permissions(&lp, std::fs::Permissions::from_mode(0o755)).expect("置可执行");

        // 只有空目录：找不到。
        let path = std::env::join_paths([empty.path()]).expect("拼 PATH");
        assert!(lp_in_path(&path).is_none());

        // 后一个目录里有 lp：找得到，且返回的就是它。
        let path = std::env::join_paths([empty.path(), with_lp.path()]).expect("拼 PATH");
        assert_eq!(lp_in_path(&path), Some(lp));
    }
}
