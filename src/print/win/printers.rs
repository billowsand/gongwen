//! 打印机清单、状态，以及按所选打印机建设备上下文。

use super::super::{PrinterInfo, PrinterState, windows_status};
use super::gdi::DcGuard;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, GetLastError};
use windows_sys::Win32::Graphics::Gdi::{
    CreateDCW, DEVMODEW, DM_COLLATE, DM_COPIES, DM_IN_BUFFER, DM_OUT_BUFFER, DMCOLLATE_FALSE,
    DMCOLLATE_TRUE,
};
use windows_sys::Win32::Graphics::Printing::{
    ClosePrinter, DocumentPropertiesW, EnumPrintersW, GetDefaultPrinterW, GetPrinterW,
    OpenPrinterW, PRINTER_ATTRIBUTE_WORK_OFFLINE, PRINTER_ENUM_CONNECTIONS, PRINTER_ENUM_LOCAL,
    PRINTER_HANDLE, PRINTER_INFO_4W, PRINTER_INFO_6,
};
use windows_sys::Win32::Storage::Xps::{DC_COLLATE, DC_COPIES, DeviceCapabilitiesW};

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

/// 建好的打印机设备上下文。
pub(super) struct Device {
    pub(super) dc: DcGuard,
    /// 份数与逐份已交给驱动，程序只需出一份。
    pub(super) driver_copies: bool,
}

/// 按打印机的默认设置建设备上下文；驱动能自己印多份就把份数与逐份写进 DEVMODE。
/// 其余设置（纸张、双面、颜色）一期不动，沿用打印机首选项。
pub(super) fn open_device(name: &str, copies: u32, collate: bool) -> Result<Device, String> {
    let wide_name = wide(name);
    let printer = PrinterHandle::open(&wide_name)
        .ok_or_else(|| format!("无法打开打印机「{name}」，它可能已被删除或无权访问"))?;
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
    let size = u32::try_from(size)
        .ok()
        .filter(|&size| size > 0)
        .ok_or_else(|| format!("无法读取打印机「{name}」的默认设置"))?;
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
    if filled < 0 {
        return Err(format!("无法读取打印机「{name}」的默认设置"));
    }

    let driver_copies = copies > 1 && driver_handles_copies(&wide_name, copies, collate);
    if driver_copies {
        // SAFETY: `devmode` 指向上面已由驱动填好的完整 `DEVMODEW`，缓冲区归本函数独占。
        let mut head = unsafe { devmode.read() };
        head.dmFields |= DM_COPIES | DM_COLLATE;
        head.Anonymous1.Anonymous1.dmCopies = i16::try_from(copies).unwrap_or(i16::MAX);
        head.dmCollate = if collate {
            DMCOLLATE_TRUE
        } else {
            DMCOLLATE_FALSE
        };
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
