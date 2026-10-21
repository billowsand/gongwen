//! GDI 出纸：把 PDF 逐页光栅化，按纸张方向与实际大小画到打印机设备上下文上。

use super::super::raster::{DeviceGeometry, place_page, rgba_to_bgra, rotate_ccw, turns_for_paper};
use hayro::{
    RenderCache, RenderSettings,
    hayro_interpret::{InterpreterSettings, hayro_syntax::Pdf},
    hayro_syntax::page::Rotation,
    vello_cpu::color::palette::css::WHITE,
};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::Foundation::{GlobalFree, HGLOBAL};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteDC, GDI_ERROR, GetDeviceCaps, HDC,
    HORZRES, LOGPIXELSX, LOGPIXELSY, PHYSICALHEIGHT, PHYSICALOFFSETX, PHYSICALOFFSETY,
    PHYSICALWIDTH, SRCCOPY, StretchDIBits, VERTRES,
};
use windows_sys::Win32::Storage::Xps::{AbortDoc, DOCINFOW, EndDoc, EndPage, StartDocW, StartPage};

/// 光栅化的分辨率上限（DPI）。300 DPI 打印公文文字已经足够清晰，再高只是把
/// 位图撑大：600 DPI 的 A4 一页 34MiB，上百页的汇编能吃掉几个 GiB 内存。
/// 超出部分交给 GDI 拉伸，文字边沿由打印机插值。
const MAX_PRINT_DPI: i32 = 300;

/// 打印机设备上下文，用完必须 DeleteDC 归还。
pub(super) struct DcGuard(pub(super) HDC);
impl Drop for DcGuard {
    fn drop(&mut self) {
        // SAFETY: `self.0` 是 `CreateDCW` 或打印对话框交回的设备上下文，只由本守卫
        // 持有，`DeleteDC` 恰好释放一次；守卫不会复制句柄。
        unsafe { DeleteDC(self.0) };
    }
}

/// 打印对话框可能交回两块需要归还的全局内存（设备模式与设备名）。
/// 中途早退的分支也得归还，所以用守卫托管。
pub(super) struct GlobalMemory {
    pub(super) dev_mode: HGLOBAL,
    pub(super) dev_names: HGLOBAL,
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

/// 出纸过程中往外报进度、往里看取消。
pub(super) struct Feedback<'a> {
    /// 已送页数 / 总页数，每画完一页调一次。
    pub(super) progress: &'a dyn Fn(u32, u32),
    /// 页与页之间检查；置位后任务作废。
    pub(super) cancel: &'a AtomicBool,
}

/// 一轮打印是打完了还是被用户取消。
pub(super) enum PrintStatus {
    Done,
    Cancelled,
}

/// 读进内存、解析好的 PDF。
pub(super) struct Document {
    pdf: Pdf,
}

impl Document {
    pub(super) fn open(path: &Path) -> Result<Self, String> {
        let bytes =
            crate::file_limits::read_bytes(path, crate::file_limits::MAX_PDF_BYTES, "要打印的 PDF")
                .map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
        let pdf = Pdf::new(bytes).map_err(|error| format!("PDF 文件结构无法解析：{error:?}"))?;
        if pdf.pages().is_empty() {
            return Err("PDF 中没有可打印的页面".to_string());
        }
        Ok(Self { pdf })
    }

    pub(super) fn page_count(&self) -> usize {
        self.pdf.pages().len()
    }

    /// 按 `sequence`（页码从 1 起，可重复）逐页出纸。整个过程是一个打印任务，
    /// 中途失败就作废。
    pub(super) fn print(
        &self,
        dc: &DcGuard,
        title: &str,
        sequence: impl Iterator<Item = u32>,
        total: u32,
        force_fit: bool,
        feedback: &Feedback<'_>,
    ) -> Result<PrintStatus, String> {
        let device = geometry(dc)?;
        let raster_dpi = device.dpi_x.clamp(96, MAX_PRINT_DPI);
        let paper_landscape = device.paper_w > device.paper_h;

        let doc_name: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
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

        let pages = self.pdf.pages();
        // 字体与图像的解码结果落在 cache 里全程复用；每页新建等于每页重新
        // 解码一遍嵌入的中文字体。
        let cache = RenderCache::new();
        // 不逐份打印时同一页连着出好几张，光栅化一次就够。
        let mut last: Option<(u32, PageBitmap)> = None;
        let mut sent = 0u32;
        for page_number in sequence {
            // 取消只在页与页之间检查：画到一半的页交给 GDI 画完，任务整体作废。
            if feedback.cancel.load(Ordering::Relaxed) {
                return Ok(PrintStatus::Cancelled);
            }
            let Some(page) = page_number
                .checked_sub(1)
                .and_then(|index| pages.get(usize::try_from(index).ok()?))
            else {
                continue;
            };
            if last
                .as_ref()
                .is_none_or(|(number, _)| *number != page_number)
            {
                let bitmap = render_page(page, &cache, raster_dpi, paper_landscape);
                last = Some((page_number, bitmap));
            }
            let Some((_, bitmap)) = &last else {
                continue;
            };
            if let Err(reason) = bitmap.draw(dc, &device, force_fit) {
                return Err(doc.abort(reason));
            }
            sent = sent.saturating_add(1);
            (feedback.progress)(sent.min(total), total);
        }
        // 取消检查在每个循环开头；这里再确认一次，免得用户在最后一页之后才点。
        if feedback.cancel.load(Ordering::Relaxed) {
            return Ok(PrintStatus::Cancelled);
        }
        // SAFETY: `dc.0` 有效，文档任务已由 `StartDocW` 开启。
        if unsafe { EndDoc(dc.0) } <= 0 {
            return Err(doc.abort("收尾打印任务失败"));
        }
        doc.finished = true;
        Ok(PrintStatus::Done)
    }
}

/// 已转成纸张方向的一页位图（BGRA）及其点尺寸。
struct PageBitmap {
    bgra: Vec<u8>,
    width: i32,
    height: i32,
    width_pt: f32,
    height_pt: f32,
}

impl PageBitmap {
    /// 开一页、画上去、收页。
    fn draw(
        &self,
        dc: &DcGuard,
        device: &DeviceGeometry,
        force_fit: bool,
    ) -> Result<(), &'static str> {
        let (dx, dy, dw, dh) = place_page(self.width_pt, self.height_pt, device, force_fit);
        // 负高度 = 自顶向下的 DIB，行序与 hayro 的输出一致。
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: u32::try_from(size_of::<BITMAPINFOHEADER>()).unwrap_or(u32::MAX),
                biWidth: self.width,
                biHeight: -self.height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: `dc.0` 是有效的设备上下文，页面任务已由 `StartDocW` 开启。
        if unsafe { StartPage(dc.0) } <= 0 {
            return Err("打印机在换页时报错，任务已取消");
        }
        // SAFETY: `bgra` 与 `bmi` 在调用期间存活；`bgra` 长度是
        // `width * height * 4`，与 DIB 的 32 位色一致；`dc.0` 有效。
        let lines = unsafe {
            StretchDIBits(
                dc.0,
                dx,
                dy,
                dw,
                dh,
                0,
                0,
                self.width,
                self.height,
                self.bgra.as_ptr().cast(),
                &bmi,
                DIB_RGB_COLORS,
                SRCCOPY,
            )
        };
        if lines == GDI_ERROR || lines == 0 {
            return Err("写入打印数据失败，任务已取消");
        }
        // SAFETY: `dc.0` 有效，页面已由 `StartPage` 开启。
        if unsafe { EndPage(dc.0) } <= 0 {
            return Err("打印机在结束页面时报错，任务已取消");
        }
        Ok(())
    }
}

/// 光栅化一页并转到纸张方向（见 [`turns_for_paper`]）。
fn render_page<'a>(
    page: &'a hayro::hayro_syntax::page::Page<'a>,
    cache: &RenderCache<'a>,
    raster_dpi: i32,
    paper_landscape: bool,
) -> PageBitmap {
    let (display_w, display_h) = page.render_dimensions();
    let (media_w, media_h) = page.base_dimensions();
    let target = ((display_w / 72.0) * raster_dpi as f32).round();
    // 已夹在 u16 范围内，`as` 不会截断（NaN 饱和成 0 再被 max 抬成 1）。
    let width = (target.clamp(1.0, f32::from(u16::MAX)) as u16).max(1);
    let scale = f32::from(width) / display_w.max(1.0);
    let pixmap = hayro::render(
        page,
        cache,
        &InterpreterSettings::default(),
        &RenderSettings {
            x_scale: scale,
            y_scale: scale,
            width: Some(width),
            bg_color: WHITE,
            ..Default::default()
        },
    );
    let rotate_degrees = match page.rotation() {
        Rotation::None => 0,
        Rotation::Horizontal => 90,
        Rotation::Flipped => 180,
        Rotation::FlippedHorizontal => 270,
    };
    let turns = turns_for_paper(rotate_degrees, media_w > media_h, paper_landscape);
    let bgra = rgba_to_bgra(pixmap.data_as_u8_slice());
    let (bgra, w, h) = rotate_ccw(
        &bgra,
        usize::from(pixmap.width()),
        usize::from(pixmap.height()),
        turns,
    );
    // 位图转了几个 90°，点尺寸就跟着换几次。
    let (width_pt, height_pt) = if turns % 2 == 1 {
        (display_h, display_w)
    } else {
        (display_w, display_h)
    };
    PageBitmap {
        bgra,
        width: i32::try_from(w).unwrap_or(i32::MAX),
        height: i32::try_from(h).unwrap_or(i32::MAX),
        width_pt,
        height_pt,
    }
}

/// 读打印机的分辨率、纸张与可打印区。
fn geometry(dc: &DcGuard) -> Result<DeviceGeometry, String> {
    // SAFETY: `dc.0` 是有效的打印机设备上下文；`GetDeviceCaps` 只读不写。
    let caps = |index| unsafe { GetDeviceCaps(dc.0, index as i32) };
    let device = DeviceGeometry {
        dpi_x: caps(LOGPIXELSX),
        dpi_y: caps(LOGPIXELSY),
        paper_w: caps(PHYSICALWIDTH),
        paper_h: caps(PHYSICALHEIGHT),
        offset_x: caps(PHYSICALOFFSETX),
        offset_y: caps(PHYSICALOFFSETY),
        area_w: caps(HORZRES),
        area_h: caps(VERTRES),
    };
    if device.dpi_x <= 0 || device.dpi_y <= 0 || device.area_w <= 0 || device.area_h <= 0 {
        return Err("无法读取打印机的分辨率与可打印区域".to_string());
    }
    Ok(device)
}
