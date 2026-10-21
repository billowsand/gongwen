//! 打印机清单、状态，以及按所选打印机建设备上下文。

use super::super::{
    ColorMode, Duplex, MediaOption, PrintJob, PrinterCaps, PrinterInfo, PrinterState,
    fixed_wide_name, windows_status,
};
use super::gdi::DcGuard;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, GetLastError};
use windows_sys::Win32::Graphics::Gdi::{
    CreateDCW, DEVMODEW, DM_COLLATE, DM_COLOR, DM_COPIES, DM_DEFAULTSOURCE, DM_DUPLEX,
    DM_IN_BUFFER, DM_IN_PROMPT, DM_OUT_BUFFER, DM_PAPERSIZE, DMCOLLATE_FALSE, DMCOLLATE_TRUE,
    DMCOLOR_COLOR, DMCOLOR_MONOCHROME, DMDUP_HORIZONTAL, DMDUP_SIMPLEX, DMDUP_VERTICAL,
};
use windows_sys::Win32::Graphics::Printing::{
    ClosePrinter, DocumentPropertiesW, EnumPrintersW, GetDefaultPrinterW, GetPrinterW,
    OpenPrinterW, PRINTER_ATTRIBUTE_WORK_OFFLINE, PRINTER_ENUM_CONNECTIONS, PRINTER_ENUM_LOCAL,
    PRINTER_HANDLE, PRINTER_INFO_4W, PRINTER_INFO_6,
};
use windows_sys::Win32::Storage::Xps::{
    DC_BINNAMES, DC_BINS, DC_COLLATE, DC_COLORDEVICE, DC_COPIES, DC_DUPLEX, DC_PAPERNAMES,
    DC_PAPERS, DeviceCapabilitiesW,
};

/// `DC_PAPERNAMES` 每项定长 64 个宽字符，`DC_BINNAMES` 24 个（见 winspool 头文件）。
const PAPER_NAME_UNITS: usize = 64;
const BIN_NAME_UNITS: usize = 24;

/// 以 NUL 结尾的宽字符串。
pub(super) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// winspool 填的缓冲区。按 8 字节对齐分配：里面放的是含指针的结构体。
struct Buffer(Vec<u64>);

impl Buffer {
    fn new(bytes: u32) -> Self {
        let bytes = usize::try_from(bytes).unwrap_or(usize::MAX);
        Self(vec![0; bytes.div_ceil(8).max(1)])
    }

    fn len_bytes(&self) -> usize {
        self.0.len() * 8
    }

    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.0.as_mut_ptr().cast()
    }

    fn as_ptr(&self) -> *const u8 {
        self.0.as_ptr().cast()
    }

    /// 从外部字节（打印机首选项交回的 DEVMODE）造 8 字节对齐的缓冲区。Windows 是
    /// 小端，按小端拼成 u64：既满足驱动要求的对齐，也不用非对齐读写。
    fn from_bytes(bytes: &[u8]) -> Self {
        let mut words = vec![0u64; bytes.len().div_ceil(8).max(1)];
        for (index, chunk) in bytes.chunks(8).enumerate() {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            words[index] = u64::from_le_bytes(word);
        }
        Self(words)
    }

    /// 首选项交回的字节先过一遍校验：长度要够一个 DEVMODE，`dmSize + dmDriverExtra`
    /// 不能超出数据本身。坏数据不能整块交给驱动（读私有区会越界）。
    fn from_devmode(bytes: &[u8]) -> Result<Self, String> {
        const INVALID: &str = "打印机首选项数据无效，请重新设置";
        if bytes.len() < size_of::<DEVMODEW>() {
            return Err(INVALID.to_string());
        }
        let buffer = Self::from_bytes(bytes);
        // SAFETY: 已核对长度不小于 `DEVMODEW`，缓冲区 8 字节对齐，内容来自首选项
        // 对话框；只按值读公共部分。
        let head = unsafe { buffer.as_ptr().cast::<DEVMODEW>().read() };
        let declared = usize::from(head.dmSize) + usize::from(head.dmDriverExtra);
        if declared > bytes.len() {
            return Err(INVALID.to_string());
        }
        Ok(buffer)
    }

    /// 原样写回字节流（末尾不足 8 字节的补零一并带上）。
    fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.0.len() * 8);
        for word in &self.0 {
            out.extend_from_slice(&word.to_le_bytes());
        }
        out
    }

    /// 读缓冲区里一个以 NUL 结尾的宽字符串。winspool 把结构体里的字符串放在同一块
    /// 缓冲区的尾部；指针不在缓冲区内就当没有。只按下标读，不解引用外来指针。
    fn wide_string_at(&self, ptr: *const u16) -> Option<String> {
        let offset = (ptr as usize).checked_sub(self.as_ptr() as usize)?;
        if offset % 2 != 0 {
            return None;
        }
        let mut units = Vec::new();
        let mut at = offset;
        while at + 1 < self.len_bytes() {
            // Windows 是小端：第 at 个字节起的两字节就是 u64 里对应位置的 16 位。
            let unit = (self.0[at / 8] >> ((at % 8) * 8)) as u16;
            if unit == 0 {
                return Some(String::from_utf16_lossy(&units));
            }
            units.push(unit);
            at += 2;
        }
        None
    }
}

/// `OpenPrinterW` 打开的打印机句柄，用完必须 `ClosePrinter`。
struct PrinterHandle(PRINTER_HANDLE);

impl PrinterHandle {
    fn open(name: &[u16]) -> Option<Self> {
        let mut handle = PRINTER_HANDLE::default();
        // SAFETY: `name` 以 NUL 结尾；出参指向本地变量；默认访问权限传空。
        if unsafe { OpenPrinterW(name.as_ptr(), &mut handle, null()) } == 0 {
            return None;
        }
        Some(Self(handle))
    }

    /// 两次调用的 `GetPrinterW`：先取所需字节数，再取数据。
    fn info(&self, level: u32) -> Option<Buffer> {
        let mut needed = 0u32;
        // SAFETY: 缓冲区为空、长度 0，只为取所需字节数；句柄有效。
        unsafe { GetPrinterW(self.0, level, null_mut(), 0, &mut needed) };
        if needed == 0 {
            return None;
        }
        let mut buffer = Buffer::new(needed);
        // SAFETY: 缓冲区不小于 `needed` 字节，长度如实传入；句柄有效。
        let ok = unsafe { GetPrinterW(self.0, level, buffer.as_mut_ptr(), needed, &mut needed) };
        (ok != 0).then_some(buffer)
    }
}

impl Drop for PrinterHandle {
    fn drop(&mut self) {
        // SAFETY: 句柄由 `OpenPrinterW` 打开、只由本守卫持有，恰好关闭一次。
        unsafe { ClosePrinter(self.0) };
    }
}

/// 系统默认打印机的名字。没有默认打印机时为 `None`。
pub(super) fn default_printer() -> Option<String> {
    let mut len = 0u32;
    // SAFETY: 缓冲区为空，只为取所需长度（宽字符数，含 NUL）。
    unsafe { GetDefaultPrinterW(null_mut(), &mut len) };
    if len == 0 {
        return None;
    }
    let mut buffer = vec![0u16; usize::try_from(len).ok()?];
    // SAFETY: 缓冲区恰好 `len` 个宽字符，长度如实传入。
    if unsafe { GetDefaultPrinterW(buffer.as_mut_ptr(), &mut len) } == 0 {
        return None;
    }
    let end = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..end])).filter(|name| !name.is_empty())
}

/// 列出本机打印机与网络连接的打印机。只取名字（level 4），这一级不去连网络
/// 打印机，几台共享打印机脱机也不会让清单卡住。
pub(in crate::print) fn list_printers() -> Result<Vec<PrinterInfo>, String> {
    let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
    let mut needed = 0u32;
    let mut returned = 0u32;
    // SAFETY: 缓冲区为空、长度 0，只为取所需字节数；两个出参指向本地变量。
    unsafe { EnumPrintersW(flags, null(), 4, null_mut(), 0, &mut needed, &mut returned) };
    // 两次调用之间有人加了打印机，所需字节数会变大：按新数字重试几次。
    let mut attempts = 0;
    let buffer = loop {
        if needed == 0 {
            return Ok(Vec::new());
        }
        let mut buffer = Buffer::new(needed);
        // SAFETY: 缓冲区不小于 `needed` 字节、8 字节对齐，长度如实传入。
        let ok = unsafe {
            EnumPrintersW(
                flags,
                null(),
                4,
                buffer.as_mut_ptr(),
                needed,
                &mut needed,
                &mut returned,
            )
        };
        if ok != 0 {
            break buffer;
        }
        // SAFETY: 读取本线程的最后错误码，无前置条件。
        let error = unsafe { GetLastError() };
        attempts += 1;
        if error != ERROR_INSUFFICIENT_BUFFER || attempts >= 3 {
            return Err(format!("无法列出打印机（错误码 {error}）"));
        }
    };
    let count = usize::try_from(returned).unwrap_or(0);
    let fits = count
        .checked_mul(size_of::<PRINTER_INFO_4W>())
        .is_some_and(|bytes| bytes <= buffer.len_bytes());
    if !fits {
        return Err("打印机清单数据不完整".to_string());
    }
    let default = default_printer();
    let mut printers: Vec<PrinterInfo> = Vec::with_capacity(count);
    for index in 0..count {
        // SAFETY: 上面已核对 `count` 个结构体都在缓冲区内；缓冲区 8 字节对齐，满足
        // 结构体的对齐要求；结构体是纯数据，按值读出不涉及所有权。
        let info = unsafe { buffer.as_ptr().cast::<PRINTER_INFO_4W>().add(index).read() };
        let Some(name) = buffer.wide_string_at(info.pPrinterName) else {
            continue;
        };
        if name.is_empty() || printers.iter().any(|printer| printer.name == name) {
            continue;
        }
        printers.push(PrinterInfo {
            is_default: default.as_deref() == Some(name.as_str()),
            name,
            state: PrinterState::Unknown,
        });
    }
    Ok(printers)
}

/// 查一台打印机的状态。打不开、查不到都算「状态未知」，不报错。
pub(in crate::print) fn printer_state(name: &str) -> PrinterState {
    let Some(printer) = PrinterHandle::open(&wide(name)) else {
        return PrinterState::Unknown;
    };
    let mut info = PRINTER_INFO_6::default();
    let mut needed = 0u32;
    // SAFETY: 出参是本地的 `PRINTER_INFO_6`（只含一个 u32，没有字符串），长度如实传入。
    let ok = unsafe {
        GetPrinterW(
            printer.0,
            6,
            (&raw mut info).cast(),
            u32::try_from(size_of::<PRINTER_INFO_6>()).unwrap_or(0),
            &mut needed,
        )
    };
    if ok == 0 {
        return PrinterState::Unknown;
    }
    // 「脱机使用打印机」是属性不是状态位，要另取一次 level 4。
    let offline = printer.info(4).is_some_and(|buffer| {
        if buffer.len_bytes() < size_of::<PRINTER_INFO_4W>() {
            return false;
        }
        // SAFETY: 缓冲区至少容得下一个结构体，8 字节对齐；按值读出纯数据。
        let info = unsafe { buffer.as_ptr().cast::<PRINTER_INFO_4W>().read() };
        info.Attributes & PRINTER_ATTRIBUTE_WORK_OFFLINE != 0
    });
    windows_status(info.dwStatus, offline)
}

/// 查一台打印机支持的双面、颜色、纸张与纸盒。查不到给空能力，不报错。
pub(in crate::print) fn printer_caps(name: &str) -> PrinterCaps {
    let wide_name = wide(name);
    let mut caps = PrinterCaps {
        duplex: capability_flag(&wide_name, DC_DUPLEX),
        color: capability_flag(&wide_name, DC_COLORDEVICE),
        papers: media_list(&wide_name, DC_PAPERS, DC_PAPERNAMES, PAPER_NAME_UNITS),
        trays: media_list(&wide_name, DC_BINS, DC_BINNAMES, BIN_NAME_UNITS),
        ..Default::default()
    };
    // 默认纸张与纸盒在驱动的默认 DEVMODE 里，不在能力查询的返回值里。
    if let Some(printer) = PrinterHandle::open(&wide_name)
        && let Some(buffer) = default_devmode(&printer, &wide_name)
    {
        // SAFETY: 缓冲区不小于 `DEVMODEW`、8 字节对齐，内容由驱动填好；按值读出纯数据。
        let devmode = unsafe { buffer.as_ptr().cast::<DEVMODEW>().read() };
        // SAFETY: 读默认 DEVMODE 的联合体字段（纸张尺寸、纸盒）。`devmode` 是刚按值
        // 读出的本地副本，字段是 i16、不涉及指针，读出后与缓冲区无关。
        let (paper, source) = unsafe {
            (
                devmode.Anonymous1.Anonymous1.dmPaperSize,
                devmode.Anonymous1.Anonymous1.dmDefaultSource,
            )
        };
        caps.default_paper = (paper > 0).then(|| paper.to_string());
        caps.default_tray = (source > 0).then(|| source.to_string());
    }
    // 默认值不在列表里就无从显示，当没有默认。
    if let Some(default) = &caps.default_paper
        && !caps.papers.iter().any(|paper| &paper.id == default)
    {
        caps.default_paper = None;
    }
    if let Some(default) = &caps.default_tray
        && !caps.trays.iter().any(|tray| &tray.id == default)
    {
        caps.default_tray = None;
    }
    caps
}

/// `DC_DUPLEX` / `DC_COLORDEVICE` 这类只回一个布尔的能力。
fn capability_flag(wide_name: &[u16], capability: u16) -> bool {
    // SAFETY: 打印机名以 NUL 结尾；这一级能力查询不写输出缓冲区（传空）。
    let value =
        unsafe { DeviceCapabilitiesW(wide_name.as_ptr(), null(), capability, null_mut(), null()) };
    value == 1
}

/// 取纸张 / 纸盒列表：先查个数，再分别取编号与名字。两次调用的个数对不上、
/// 或返回 -1（驱动不支持），都当查不到，给空列表。
fn media_list(
    wide_name: &[u16],
    codes_capability: u16,
    names_capability: u16,
    name_units: usize,
) -> Vec<MediaOption> {
    // SAFETY: 打印机名以 NUL 结尾；这一级查询只取个数，不写输出缓冲区。
    let count = unsafe {
        DeviceCapabilitiesW(
            wide_name.as_ptr(),
            null(),
            codes_capability,
            null_mut(),
            null(),
        )
    };
    let Ok(count) = usize::try_from(count) else {
        return Vec::new();
    };
    if count == 0 {
        return Vec::new();
    }
    let mut codes = vec![0u16; count];
    // SAFETY: 输出缓冲区有 `count` 个 u16，`count` 正是上一次调用给出的个数。
    let written = unsafe {
        DeviceCapabilitiesW(
            wide_name.as_ptr(),
            null(),
            codes_capability,
            codes.as_mut_ptr(),
            null(),
        )
    };
    if usize::try_from(written).ok() != Some(count) {
        return Vec::new();
    }
    let units = count.saturating_mul(name_units);
    let mut names = vec![0u16; units];
    // SAFETY: 输出缓冲区为 `count × name_units` 个宽字符，正是这一级能力要求的布局。
    let written = unsafe {
        DeviceCapabilitiesW(
            wide_name.as_ptr(),
            null(),
            names_capability,
            names.as_mut_ptr(),
            null(),
        )
    };
    if usize::try_from(written).ok() != Some(count) {
        return Vec::new();
    }
    codes
        .iter()
        .zip(names.chunks_exact(name_units))
        .map(|(&code, name)| MediaOption {
            id: code.to_string(),
            label: fixed_wide_name(name),
        })
        .collect()
}

/// 取一台打印机的默认 DEVMODE，放在 8 字节对齐的缓冲区里（驱动会填公共部分与
/// 私有区）。`open_device` 按本次选择改它，能力查询只读默认纸张与纸盒。
fn default_devmode(printer: &PrinterHandle, wide_name: &[u16]) -> Option<Buffer> {
    // SAFETY: 输出输入都传空、模式 0，只取 DEVMODE 所需字节数。
    let size = unsafe {
        DocumentPropertiesW(
            null_mut(),
            printer.0,
            wide_name.as_ptr(),
            null_mut(),
            null(),
            0,
        )
    };
    let size = u32::try_from(size).ok().filter(|&size| size > 0)?;
    let size = size.max(u32::try_from(size_of::<DEVMODEW>()).unwrap_or(u32::MAX));
    let mut buffer = Buffer::new(size);
    let devmode = buffer.as_mut_ptr().cast::<DEVMODEW>();
    // SAFETY: 缓冲区不小于驱动要求的字节数，也不小于 `DEVMODEW`，8 字节对齐；
    // 驱动只往里写默认设置。
    let filled = unsafe {
        DocumentPropertiesW(
            null_mut(),
            printer.0,
            wide_name.as_ptr(),
            devmode,
            null(),
            DM_OUT_BUFFER,
        )
    };
    (filled >= 0).then_some(buffer)
}

/// 建好的打印机设备上下文。
pub(super) struct Device {
    pub(super) dc: DcGuard,
    /// 份数与逐份已交给驱动，程序只需出一份。
    pub(super) driver_copies: bool,
}

/// 按打印机的默认设置建带本次选择的设备上下文。份数与逐份交给驱动（驱动不支持
/// 就由程序重复出纸），双面、颜色、纸张、纸盒写进 DEVMODE，任何一项改了就走一次
/// `DM_IN_BUFFER | DM_OUT_BUFFER` 合并，让驱动把它们并回自己的私有区。
pub(super) fn open_device(name: &str, job: &PrintJob) -> Result<Device, String> {
    let wide_name = wide(name);
    let printer = PrinterHandle::open(&wide_name)
        .ok_or_else(|| format!("无法打开打印机「{name}」，它可能已被删除或无权访问"))?;
    // 用户改过「打印机首选项」：整块 DEVMODE（含驱动私有区）优先，双面 / 颜色 /
    // 纸张 / 纸盒都以首选项为准，本次只再写份数与逐份。否则从驱动默认值出发，
    // 把对话框里的选择写进去。
    let from_preferences = job.devmode.is_some();
    let mut buffer = match &job.devmode {
        Some(bytes) => Buffer::from_devmode(bytes)?,
        None => default_devmode(&printer, &wide_name)
            .ok_or_else(|| format!("无法读取打印机「{name}」的默认设置"))?,
    };
    let devmode = buffer.as_mut_ptr().cast::<DEVMODEW>();

    let driver_copies =
        job.copies > 1 && driver_handles_copies(&wide_name, job.copies, job.collate);
    let mut changed = false;
    // SAFETY: `devmode` 指向上面由驱动填好（或首选项交回并校验过）的完整
    // `DEVMODEW`，缓冲区归本函数独占。
    let mut head = unsafe { devmode.read() };
    if driver_copies {
        head.dmFields |= DM_COPIES | DM_COLLATE;
        head.Anonymous1.Anonymous1.dmCopies = i16::try_from(job.copies).unwrap_or(i16::MAX);
        head.dmCollate = if job.collate {
            DMCOLLATE_TRUE
        } else {
            DMCOLLATE_FALSE
        };
        changed = true;
    }
    if !from_preferences {
        if let Some(duplex) = job.duplex {
            head.dmFields |= DM_DUPLEX;
            // 我们总是按竖向纸张出图，所以长边翻转 = 竖向翻转、短边 = 横向翻转。
            head.dmDuplex = match duplex {
                Duplex::Simplex => DMDUP_SIMPLEX,
                Duplex::LongEdge => DMDUP_VERTICAL,
                Duplex::ShortEdge => DMDUP_HORIZONTAL,
            };
            changed = true;
        }
        if let Some(color) = job.color {
            head.dmFields |= DM_COLOR;
            head.dmColor = match color {
                ColorMode::Color => DMCOLOR_COLOR,
                ColorMode::Mono => DMCOLOR_MONOCHROME,
            };
            changed = true;
        }
        // 纸张与纸盒的 id 是 `DeviceCapabilitiesW` 给的十进制编号；解析不了就不设，
        // 由驱动按自己的默认值处理，不让一个坏编号毁掉整次打印。
        if let Some(size) = job.paper.as_deref().and_then(|id| id.parse::<i16>().ok()) {
            head.dmFields |= DM_PAPERSIZE;
            head.Anonymous1.Anonymous1.dmPaperSize = size;
            changed = true;
        }
        if let Some(source) = job.tray.as_deref().and_then(|id| id.parse::<i16>().ok()) {
            head.dmFields |= DM_DEFAULTSOURCE;
            head.Anonymous1.Anonymous1.dmDefaultSource = source;
            changed = true;
        }
    }
    if changed {
        // SAFETY: 同上；只写回公共部分，紧随其后的驱动私有区不动。
        unsafe { devmode.write(head) };
        // SAFETY: 输入输出是同一块缓冲区，驱动按 `dmSize + dmDriverExtra` 读写，
        // 不超出它最初报的字节数。
        let merged = unsafe {
            DocumentPropertiesW(
                null_mut(),
                printer.0,
                wide_name.as_ptr(),
                devmode,
                devmode,
                DM_IN_BUFFER | DM_OUT_BUFFER,
            )
        };
        if merged < 0 {
            return Err(format!("打印机「{name}」不接受这组设置"));
        }
    }

    let driver = wide("WINSPOOL");
    // SAFETY: 驱动名与打印机名都是 NUL 结尾宽字符串；`devmode` 已由驱动校验，
    // 调用期间缓冲区存活。
    let dc = unsafe { CreateDCW(driver.as_ptr(), wide_name.as_ptr(), null(), devmode) };
    if dc.is_null() {
        return Err(format!("无法连接打印机「{name}」"));
    }
    Ok(Device {
        dc: DcGuard(dc),
        driver_copies,
    })
}

/// 弹驱动自己的「打印机首选项」窗口。用户点确定就返回改过的整块 DEVMODE
/// （含驱动私有区），取消返回 `None`。会弹窗并等用户操作，必须在后台线程调。
pub(in crate::print) fn edit_preferences(
    name: &str,
    current: Option<&[u8]>,
) -> Result<Option<Vec<u8>>, String> {
    // IDOK / IDCANCEL，见 winuser.h；`DocumentPropertiesW` 只回这两个或负数。
    const IDOK: i32 = 1;
    const IDCANCEL: i32 = 2;
    let wide_name = wide(name);
    let printer = PrinterHandle::open(&wide_name)
        .ok_or_else(|| format!("无法打开打印机「{name}」，它可能已被删除或无权访问"))?;
    // 输入：优先用上次改过的，其次驱动默认值。长度不够的旧数据当没有。
    let base = current
        .filter(|bytes| bytes.len() >= size_of::<DEVMODEW>())
        .map(Buffer::from_bytes)
        .or_else(|| default_devmode(&printer, &wide_name))
        .ok_or_else(|| format!("无法读取打印机「{name}」的默认设置"))?;
    // 输出缓冲区不小于输入：驱动会把输入连同自己改的私有区写到输出。
    let size = u32::try_from(base.len_bytes()).map_err(|_| "打印机首选项数据过大".to_string())?;
    let mut output = Buffer::new(size);
    // SAFETY: hwnd 传空（对话框独立出现，不归档到主窗口）；打印机句柄有效；名字以
    // NUL 结尾；输入输出都是 8 字节对齐、长度不小于 DEVMODE 的缓冲区。
    let code = unsafe {
        DocumentPropertiesW(
            null_mut(),
            printer.0,
            wide_name.as_ptr(),
            output.as_mut_ptr().cast(),
            base.as_ptr().cast(),
            DM_IN_PROMPT | DM_IN_BUFFER | DM_OUT_BUFFER,
        )
    };
    if code == IDOK {
        return Ok(Some(output.to_bytes()));
    }
    if code == IDCANCEL {
        return Ok(None);
    }
    Err(format!("无法打开打印机「{name}」的首选项（错误码 {code}）"))
}

/// 驱动能不能自己印这么多份（以及逐份）。不能就由程序逐份重复出纸。
fn driver_handles_copies(wide_name: &[u16], copies: u32, collate: bool) -> bool {
    // SAFETY: 打印机名以 NUL 结尾；份数上限查询不写输出缓冲区（传空）。
    let max =
        unsafe { DeviceCapabilitiesW(wide_name.as_ptr(), null(), DC_COPIES, null_mut(), null()) };
    if u32::try_from(max).map_or(true, |max| max < copies) {
        return false;
    }
    if !collate {
        return true;
    }
    // SAFETY: 同上，逐份能力查询也不写输出缓冲区。
    let collate =
        unsafe { DeviceCapabilitiesW(wide_name.as_ptr(), null(), DC_COLLATE, null_mut(), null()) };
    collate == 1
}
