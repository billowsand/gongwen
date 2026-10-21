//! 打印：应用内打印对话框（[`dialog`]）加两个平台的出纸后端。
//!
//! 对话框三平台一致：选打印机（带默认标记与状态）、份数、逐份打印、页码范围。
//! 后端分两条路：
//!
//! - Windows 不能指望第三方 PDF 软件：Edge、WPS、Chrome 这些常见默认阅读器都不
//!   注册 shell 的 print 谓词。这里用 Win32 枚举打印机、按所选打印机建 DEVMODE 与
//!   设备上下文，hayro 把页面光栅化后经 GDI 送打印（`win`），跟机器上装没装 PDF
//!   阅读器完全无关。老式 `PrintDlg` 系统对话框留作「使用系统对话框…」的出口。
//! - Unix（含 macOS 与麒麟等国产系统）交给 CUPS 命令行（`cups`）：`lpstat` 列打印机，
//!   `lp` 原生吃 PDF，由 CUPS 自己渲染，质量比位图好；`lp` 不在（Linux 没装
//!   cups-client）时按钮置灰。
//!
//! 设计与分期见 `docs/print-dialog-design.md`。

#[cfg(unix)]
mod cups;
mod dialog;
mod pages;
mod raster;
#[cfg(windows)]
#[allow(unsafe_code)]
mod win;

pub(crate) use dialog::{DialogOutcome, PrintDialog};
pub(crate) use pages::PageSpan;

use std::path::{Path, PathBuf};

/// 份数上限。再多就该去印刷厂了，也免得手滑输入一个天文数字把打印队列塞满。
pub(crate) const MAX_COPIES: u32 = 999;

/// 打开打印对话框所需的信息，由 PDF 查看器给出。
#[derive(Debug, Clone)]
pub(crate) struct PrintRequest {
    pub(crate) path: PathBuf,
    /// 对话框标题与打印任务名用。
    pub(crate) title: String,
    /// 总页数；0 表示还没打开完或打不开，这时只能打全部。
    pub(crate) page_count: usize,
    /// 正在看的页（从 0 起）。
    pub(crate) current_page: usize,
}

/// 一台打印机。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrinterInfo {
    pub(crate) name: String,
    pub(crate) is_default: bool,
    pub(crate) state: PrinterState,
}

/// 打印机状态。只用于提示，不拦打印：网络打印机的状态常常报不准。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrinterState {
    /// 还没查到，或系统没有报。
    Unknown,
    Ready,
    Printing,
    /// 暂停或被停用（CUPS 的 disabled）。
    Paused,
    Offline,
    /// 缺纸、卡纸之类需要人去处理的状况。
    Attention(String),
}

impl PrinterState {
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Unknown => "状态未知".to_string(),
            Self::Ready => "就绪".to_string(),
            Self::Printing => "正在打印".to_string(),
            Self::Paused => "已暂停".to_string(),
            Self::Offline => "脱机".to_string(),
            Self::Attention(reason) => reason.clone(),
        }
    }

    /// 能不能指望它马上出纸。界面据此决定状态点的颜色。
    pub(crate) fn healthy(&self) -> Option<bool> {
        match self {
            Self::Unknown => None,
            Self::Ready | Self::Printing => Some(true),
            Self::Paused | Self::Offline | Self::Attention(_) => Some(false),
        }
    }
}

/// 一次打印任务：对话框里的选择定下来之后交给后端。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrintJob {
    pub(crate) path: PathBuf,
    pub(crate) title: String,
    /// `None` 表示交给系统默认打印机（列不出打印机时的退路）。
    pub(crate) printer: Option<String>,
    pub(crate) copies: u32,
    /// 逐份打印：一份打完再打下一份。
    pub(crate) collate: bool,
    /// `None` 表示全部页。
    pub(crate) pages: Option<Vec<PageSpan>>,
}

/// 打印任务的结果。「打完了」和「用户取消」是两回事，状态栏文案要分开。
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrintOutcome {
    /// 打印数据已完整交给系统。
    Printed,
    /// 用户在系统打印对话框里点了取消。不是错误，不该报红。
    Cancelled,
}

/// 本机是否具备打印能力。不具备时界面把「打印」按钮置灰，并引导改用
/// 「系统打开」后自行打印，而不是让人点了没反应。
///
/// Windows 走原生 GDI 打印，有没有打印机由对话框自己呈现，恒可按。
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
        cups::lp_available()
    }
}

/// 列出本机的打印机（名字与默认标记）。状态随后用 [`printer_state`] 逐台补：
/// 网络打印机查一次可能要几秒，不能让整张清单等它。阻塞调用，调用方放后台线程。
pub(crate) fn list_printers() -> Result<Vec<PrinterInfo>, String> {
    #[cfg(windows)]
    {
        win::list_printers()
    }
    #[cfg(unix)]
    {
        cups::list_printers()
    }
}

/// 查一台打印机的状态。阻塞调用。
pub(crate) fn printer_state(name: &str) -> PrinterState {
    #[cfg(windows)]
    {
        win::printer_state(name)
    }
    #[cfg(unix)]
    {
        cups::printer_state(name)
    }
}

/// 执行打印任务。阻塞调用（逐页光栅化或等 `lp` 返回），调用方必须放后台线程。
pub(crate) fn print(job: &PrintJob) -> Result<PrintOutcome, String> {
    check_size(&job.path)?;
    #[cfg(windows)]
    {
        win::print(job)
    }
    #[cfg(unix)]
    {
        cups::print(job)
    }
}

/// 改用系统自带的打印对话框打印（驱动独有的装订、打孔等选项走这里）。
/// 只有 Windows 有；阻塞调用。
#[cfg(windows)]
pub(crate) fn print_with_system_dialog(path: &Path) -> Result<PrintOutcome, String> {
    check_size(path)?;
    win::print_with_system_dialog(path)
}

/// 先卡体积：超大的 PDF 不该被整体读进内存。
fn check_size(path: &Path) -> Result<(), String> {
    let size = std::fs::metadata(path)
        .map_err(|error| format!("无法读取 {}：{error}", path.display()))?
        .len();
    if size > crate::file_limits::MAX_PDF_BYTES {
        return Err(format!(
            "要打印的 PDF 超过 {} MB 上限",
            crate::file_limits::MAX_PDF_BYTES / 1024 / 1024
        ));
    }
    Ok(())
}

/// Windows 打印机状态位（`PRINTER_INFO_6::dwStatus`）转成界面上的状态。
/// 放在平台无关处，好在任何平台上单测；常量取自 winspool 头文件。
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_status(status: u32, work_offline: bool) -> PrinterState {
    const PAUSED: u32 = 0x1;
    const ERROR: u32 = 0x2;
    const PAPER_JAM: u32 = 0x8;
    const PAPER_OUT: u32 = 0x10;
    const PAPER_PROBLEM: u32 = 0x40;
    const OFFLINE: u32 = 0x80;
    const PRINTING: u32 = 0x400;
    const BUSY: u32 = 0x200;
    const OUTPUT_BIN_FULL: u32 = 0x800;
    const NOT_AVAILABLE: u32 = 0x1000;
    const NO_TONER: u32 = 0x4_0000;
    const USER_INTERVENTION: u32 = 0x10_0000;
    const DOOR_OPEN: u32 = 0x40_0000;
    const SERVER_OFFLINE: u32 = 0x200_0000;

    if work_offline || status & (OFFLINE | SERVER_OFFLINE | NOT_AVAILABLE) != 0 {
        return PrinterState::Offline;
    }
    if status & PAUSED != 0 {
        return PrinterState::Paused;
    }
    let attention = [
        (PAPER_OUT, "缺纸"),
        (PAPER_JAM, "卡纸"),
        (DOOR_OPEN, "机盖未关"),
        (NO_TONER, "缺墨粉"),
        (OUTPUT_BIN_FULL, "出纸槽已满"),
        (PAPER_PROBLEM | USER_INTERVENTION | ERROR, "需要处理"),
    ];
    if let Some((_, reason)) = attention.iter().find(|(bits, _)| status & bits != 0) {
        return PrinterState::Attention((*reason).to_string());
    }
    if status & (PRINTING | BUSY) != 0 {
        return PrinterState::Printing;
    }
    PrinterState::Ready
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_status_bits_map_to_states() {
        assert_eq!(windows_status(0, false), PrinterState::Ready);
        assert_eq!(windows_status(0, true), PrinterState::Offline);
        assert_eq!(windows_status(0x80, false), PrinterState::Offline);
        assert_eq!(windows_status(0x1, false), PrinterState::Paused);
        assert_eq!(
            windows_status(0x10 | 0x400, false),
            PrinterState::Attention("缺纸".to_string())
        );
        assert_eq!(windows_status(0x400, false), PrinterState::Printing);
    }
}
