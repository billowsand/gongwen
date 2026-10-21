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
    // 先卡体积：两条路都从这里进，超大的 PDF 不该被整体读进内存。
    let size = std::fs::metadata(path)
        .map_err(|error| format!("无法读取 {}：{error}", path.display()))?
        .len();
    if size > crate::file_limits::MAX_PDF_BYTES {
        return Err(format!(
            "要打印的 PDF 超过 {} MB 上限",
            crate::file_limits::MAX_PDF_BYTES / 1024 / 1024
        ));
    }
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
    // Rust 的浮点转整数本身是饱和的（NaN 得 0），不会出未定义行为；这里显式
    // 夹到 [1, i32::MAX] 并把 NaN 当 1，是为了极端比例下也至少画出一个像素。
    let to_i32 = |value: f64| {
        if value.is_nan() {
            1
        } else {
            value.round().clamp(1.0, f64::from(i32::MAX)) as i32
        }
    };
    let w = to_i32(f64::from(src_w) * scale);
    let h = to_i32(f64::from(src_h) * scale);
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
    use windows_sys::Win32::Foundation::{GlobalFree, HGLOBAL};
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
            // SAFETY: `self.0` 是打印对话框交回的设备上下文，只由本守卫持有，
            // `DeleteDC` 恰好释放一次；守卫不会复制句柄。
            unsafe { DeleteDC(self.0) };
        }
    }

    /// 打印对话框可能交回两块需要归还的全局内存（设备模式与设备名）。
    /// 中途早退的分支也得归还，所以用守卫托管。
    struct GlobalMemory {
        dev_mode: HGLOBAL,
        dev_names: HGLOBAL,
    }
    impl Drop for GlobalMemory {
        fn drop(&mut self) {
            // SAFETY: 两块内存都由打印对话框交回、所有权转给我们；空句柄跳过，
            // 非空句柄各 `GlobalFree` 一次，与分配方成对。
            unsafe {
                if !self.dev_mode.is_null() {
                    GlobalFree(self.dev_mode);
                }
                if !self.dev_names.is_null() {
                    GlobalFree(self.dev_names);
                }
            }
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
                // SAFETY: 打印任务还未 `EndDoc`（`finished` 为 false），此时必须
                // `AbortDoc` 作废它；句柄由外层的 `DcGuard` 管，不在这里释放。
                unsafe { AbortDoc(self.dc) };
            }
        }
    }

    pub(super) fn print_pdf(path: &Path) -> Result<PrintOutcome, String> {
        let bytes =
            crate::file_limits::read_bytes(path, crate::file_limits::MAX_PDF_BYTES, "要打印的 PDF")
                .map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
        let pdf = Pdf::new(bytes).map_err(|error| format!("PDF 文件结构无法解析：{error:?}"))?;
        let pages = pdf.pages();
        // 先取全部页面的点尺寸：既是页数，也是逐页光栅化的比例基准。
        let page_sizes: Vec<(f32, f32)> =
            pages.iter().map(|page| page.render_dimensions()).collect();
        if page_sizes.is_empty() {
            return Err("PDF 中没有可打印的页面".to_string());
        }
        // PrintDlg 的页码字段是 u16，超出部分按最大页截断。
        let max_page =
            u16::try_from(page_sizes.len().min(usize::from(u16::MAX))).unwrap_or(u16::MAX);

        let mut pd = PRINTDLGW {
            lStructSize: u32::try_from(size_of::<PRINTDLGW>()).unwrap_or(u32::MAX),
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
        // SAFETY: `pd` 是本函数独占的栈上结构，`PrintDlgW` 只会往里写句柄与
        // 页码字段；句柄取出后由守卫接管。
        if unsafe { PrintDlgW(&mut pd) } == 0 {
            // SAFETY: 取错误码不需要任何前置条件，句柄此时已由对话框自己清理。
            let code = unsafe { CommDlgExtendedError() };
            if code == 0 {
                return Ok(PrintOutcome::Cancelled);
            }
            return Err(format!(
                "无法打开打印对话框（错误码 {code}），请确认已安装打印机"
            ));
        }
        // PD_RETURNDC 模式下这两个句柄通常为空，非空就要 GlobalFree 归还。
        let _memory = GlobalMemory {
            dev_mode: pd.hDevMode,
            dev_names: pd.hDevNames,
        };
        if pd.hDC.is_null() {
            return Err("未能从打印对话框取得打印机上下文".to_string());
        }
        let dc = DcGuard(pd.hDC);

        // SAFETY: `dc.0` 是打印对话框交回的设备上下文，在本函数内有效。
        let (printer_dpi, area_w, area_h) = unsafe {
            (
                GetDeviceCaps(dc.0, LOGPIXELSX as i32),
                GetDeviceCaps(dc.0, HORZRES as i32),
                GetDeviceCaps(dc.0, VERTRES as i32),
            )
        };
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
            cbSize: i32::try_from(size_of::<DOCINFOW>()).unwrap_or(i32::MAX),
            lpszDocName: doc_name.as_ptr(),
            ..Default::default()
        };
        // SAFETY: `info` 与 `doc_name` 都在本次调用期间存活，`lpszDocName` 指向
        // 以 NUL 结尾的宽字符数组；`dc.0` 是有效的设备上下文。
        if unsafe { StartDocW(dc.0, &info) } <= 0 {
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
                // 已夹在 u16 范围内，`as` 不会截断（NaN 饱和成 0 再被 max 抬成 1）。
                let width = (target.clamp(1.0, f32::from(u16::MAX)) as u16).max(1);
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
                        biSize: u32::try_from(size_of::<BITMAPINFOHEADER>()).unwrap_or(u32::MAX),
                        biWidth: src_w,
                        biHeight: -src_h,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                // SAFETY: `dc.0` 是有效的设备上下文，页面任务已由 `StartDocW` 开启。
                if unsafe { StartPage(dc.0) } <= 0 {
                    return Err(doc.abort("打印机在换页时报错，任务已取消"));
                }
                // SAFETY: `bgra` 与 `bmi` 在调用期间存活；`bgra` 长度是
                // `src_w * src_h * 4`，与 DIB 的 32 位色一致；`dc.0` 有效。
                let lines = unsafe {
                    StretchDIBits(
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
                    )
                };
                if lines == GDI_ERROR || lines == 0 {
                    return Err(doc.abort("写入打印数据失败，任务已取消"));
                }
                // SAFETY: `dc.0` 有效，页面已由 `StartPage` 开启。
                if unsafe { EndPage(dc.0) } <= 0 {
                    return Err(doc.abort("打印机在结束页面时报错，任务已取消"));
                }
            }
        }
        // SAFETY: `dc.0` 有效，文档任务已由 `StartDocW` 开启。
        if unsafe { EndDoc(dc.0) } <= 0 {
            return Err(doc.abort("收尾打印任务失败"));
        }
        doc.finished = true;
        Ok(PrintOutcome::Printed)
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
