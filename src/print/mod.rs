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
mod inspect;
mod pages;
mod raster;
#[cfg(windows)]
#[allow(unsafe_code)]
mod win;

pub(crate) use dialog::{DialogOutcome, PrintDialog};
pub(crate) use inspect::{PdfFacts, inspect_pdf};
pub(crate) use pages::PageSpan;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 份数上限。再多就该去印刷厂了，也免得手滑输入一个天文数字把打印队列塞满。
pub(crate) const MAX_COPIES: u32 = 999;

/// 双面方式。`None` 表示沿用打印机自己的设置，所以选项用 `Option<Duplex>`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Duplex {
    Simplex,
    /// 长边翻转：竖纸上的公文，翻面后沿长边翻，像翻书。
    LongEdge,
    /// 短边翻转：整份横排时用，装订边才对得上。
    ShortEdge,
    /// 手动双面：先打正面（要打的页里第 1、3、5… 张），用户把纸翻面放回后再打背面。
    /// 不需要打印机有双面单元。
    Manual,
}

/// 彩色 / 黑白。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ColorMode {
    Color,
    Mono,
}

/// 缩放方式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Scaling {
    /// 实际大小：放不下时自动缩小（第一期的行为，公文按毫米打准靠它）。
    #[default]
    Actual,
    /// 适合纸张：按可打印区等比缩放，给非 A4 来源的 PDF 用。
    Fit,
}

/// 纸张或纸盒的一个可选项。`id` 交给驱动 / CUPS，`label` 给人看。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MediaOption {
    pub(crate) id: String,
    pub(crate) label: String,
}

/// 一台打印机支持哪些选项。查不到就用 [`Default`]：双面、彩色都当不支持，
/// 纸张与纸盒列表为空，界面据此置灰而不是报错。
#[derive(Default, Clone)]
pub(crate) struct PrinterCaps {
    /// 有双面单元。
    pub(crate) duplex: bool,
    /// 能在彩色 / 黑白之间选。
    pub(crate) color: bool,
    pub(crate) papers: Vec<MediaOption>,
    pub(crate) default_paper: Option<String>,
    pub(crate) trays: Vec<MediaOption>,
    pub(crate) default_tray: Option<String>,
}

/// 调用方知道的、与打印有关的版式事实。打开归档扫描件时两项都是 false。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PrintHints {
    /// 稿件按双面排了页码（模板 `profile.duplex_printing`，奇右偶左）。
    pub(crate) duplex_typeset: bool,
    /// 送批材料合并件：每件从奇数页起排。
    pub(crate) send_package: bool,
}

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
    pub(crate) hints: PrintHints,
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
    /// `None` 表示沿用打印机的双面设置。
    pub(crate) duplex: Option<Duplex>,
    /// `None` 表示沿用打印机的颜色设置。
    pub(crate) color: Option<ColorMode>,
    /// 纸张的驱动编号 / CUPS 关键字；`None` 表示打印机默认。
    pub(crate) paper: Option<String>,
    /// 纸盒的驱动编号 / CUPS 关键字；`None` 表示自动选择。
    pub(crate) tray: Option<String>,
    pub(crate) scaling: Scaling,
    /// Windows「打印机首选项」改过的整块 DEVMODE；有它时双面、颜色、纸张、纸盒
    /// 都以首选项为准。Unix 恒为 `None`。
    pub(crate) devmode: Option<Vec<u8>>,
    /// 倒序出纸（页码从大到小）。手动双面第二轮用：出纸面朝上的机器翻面后
    /// 正反才对得上。
    pub(crate) reverse: bool,
}

/// 打印任务的结果。「打完了」和「用户取消」是两回事，状态栏文案要分开。
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrintOutcome {
    /// 打印数据已完整交给系统。`job` 是 CUPS 的任务号（`X-12`），
    /// Windows 从 GDI 拿不到，是 `None`。
    Printed { job: Option<String> },
    /// 用户取消。不是错误，不该报红。
    Cancelled,
}

/// 按稿件与打印机能力决定双面的默认值。只在用户没动过双面时用：
/// 送批材料与按双面排的稿件默认长边，整份横排默认短边（装订边才对得上）。
pub(crate) fn default_duplex(
    hints: PrintHints,
    facts: Option<PdfFacts>,
    caps: &PrinterCaps,
) -> Option<Duplex> {
    if !caps.duplex || !(hints.send_package || hints.duplex_typeset) {
        return None;
    }
    let all_rotated = facts.is_some_and(|facts| facts.all_rotated);
    Some(if all_rotated {
        Duplex::ShortEdge
    } else {
        Duplex::LongEdge
    })
}

/// 公文场景的提醒。都只是提醒，不拦打印。
pub(crate) fn print_warnings(
    hints: PrintHints,
    facts: Option<PdfFacts>,
    caps: &PrinterCaps,
    job: &PrintJob,
) -> Vec<String> {
    let mut warnings = Vec::new();
    let single_sided = job.duplex == Some(Duplex::Simplex);
    if single_sided && hints.duplex_typeset {
        warnings.push("页码按双面排（奇右偶左），单面打印页码会左右交替。".to_string());
    }
    if single_sided && hints.send_package {
        warnings.push("送批材料每件从奇数页起排，单面打印会多出空白页。".to_string());
    }
    let all_rotated = facts.is_some_and(|facts| facts.all_rotated);
    if all_rotated && job.duplex == Some(Duplex::LongEdge) {
        warnings.push("整份横排，双面建议选短边翻转，装订边才对得上。".to_string());
    }
    let has_red = facts.is_some_and(|facts| facts.has_red);
    if has_red && (job.color == Some(ColorMode::Mono) || !caps.color) {
        warnings.push("版面有红色（版头或印章），黑白打印会印成灰色。".to_string());
    }
    if job.scaling == Scaling::Actual
        && let Some(paper) = &job.paper
        && !paper_is_a4(paper, caps)
    {
        warnings.push("所选纸张不是 A4，公文按 A4 排版，建议改为「适合纸张」。".to_string());
    }
    warnings
}

/// 纸张编号或它在能力列表里的名字里带「A4」就算 A4（各驱动的编号体系不同，
/// Windows 的 `DMPAPER_A4` 是 9，CUPS 的 id 就是 A4 字样）。
fn paper_is_a4(paper: &str, caps: &PrinterCaps) -> bool {
    let label = caps
        .papers
        .iter()
        .find(|option| option.id == paper)
        .map(|option| option.label.as_str())
        .unwrap_or(paper);
    paper.to_ascii_uppercase().contains("A4") || label.to_ascii_uppercase().contains("A4")
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

/// 查一台打印机支持的双面、颜色、纸张与纸盒。阻塞调用，调用方放后台线程。
/// 查不到返回默认值（什么都不支持），不报错：能力只用来置灰选项，不该拦住打印。
pub(crate) fn printer_caps(name: &str) -> PrinterCaps {
    #[cfg(windows)]
    {
        win::printer_caps(name)
    }
    #[cfg(unix)]
    {
        cups::printer_caps(name)
    }
}

/// 弹驱动自己的「打印机首选项」窗口（只有 Windows 有）。会弹窗等用户操作，
/// 阻塞调用，必须在后台线程调。返回改过的整块 DEVMODE；取消是 `Ok(None)`。
#[cfg(windows)]
pub(crate) fn edit_preferences(
    name: &str,
    current: Option<&[u8]>,
) -> Result<Option<Vec<u8>>, String> {
    win::edit_preferences(name, current)
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
///
/// `progress` 报「已送页数 / 总页数」；Windows 每画完一页调一次并检查 `cancel`，
/// CUPS 不报进度（`lp` 是一次交出整份），只在调用 `lp` 之前看一次取消。
pub(crate) fn print(
    job: &PrintJob,
    progress: &dyn Fn(u32, u32),
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<PrintOutcome, String> {
    check_size(&job.path)?;
    #[cfg(windows)]
    {
        win::print(job, progress, cancel)
    }
    #[cfg(unix)]
    {
        // CUPS 不报进度：`lp` 是一次交出整份。
        let _ = progress;
        cups::print(job, cancel)
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

/// 从定长宽字符缓冲区里取一个名字：到第一个 NUL 为止，没有 NUL 就取全长。
///
/// Windows 的 `DC_PAPERNAMES` / `DC_BINNAMES` 每项定长（32 / 24 个宽字符），
/// 名字短于定长时有 NUL，正好占满时没有。放在平台无关处好在 macOS 上单测。
#[cfg_attr(not(windows), allow(dead_code))]
fn fixed_wide_name(units: &[u16]) -> String {
    let end = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
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

    fn caps(duplex: bool, color: bool) -> PrinterCaps {
        PrinterCaps {
            duplex,
            color,
            papers: vec![MediaOption {
                id: "9".to_string(),
                label: "A4".to_string(),
            }],
            default_paper: Some("9".to_string()),
            ..Default::default()
        }
    }

    fn a4_job() -> PrintJob {
        PrintJob {
            path: PathBuf::from("/tmp/a.pdf"),
            title: "测试".to_string(),
            printer: None,
            copies: 1,
            collate: false,
            pages: None,
            duplex: Some(Duplex::Simplex),
            color: None,
            paper: None,
            tray: None,
            scaling: Scaling::Actual,
            devmode: None,
            reverse: false,
        }
    }

    #[test]
    fn duplex_defaults_when_the_document_is_typeset_for_double_sided() {
        let hints = PrintHints {
            duplex_typeset: true,
            send_package: false,
        };
        assert_eq!(
            default_duplex(hints, None, &caps(true, true)),
            Some(Duplex::LongEdge)
        );
        let hints = PrintHints {
            duplex_typeset: false,
            send_package: true,
        };
        assert_eq!(
            default_duplex(hints, None, &caps(true, true)),
            Some(Duplex::LongEdge)
        );
        // 整份横排：短边翻转装订边才对得上。
        let facts = PdfFacts {
            all_rotated: true,
            has_red: false,
        };
        assert_eq!(
            default_duplex(hints, Some(facts), &caps(true, true)),
            Some(Duplex::ShortEdge)
        );
        // 没有双面单元、或文档没按双面排：不设默认。
        assert_eq!(default_duplex(hints, None, &caps(false, true)), None);
        assert_eq!(
            default_duplex(PrintHints::default(), None, &caps(true, true)),
            None
        );
    }

    #[test]
    fn typeset_for_double_sided_but_printing_single_sided_warns() {
        let hints = PrintHints {
            duplex_typeset: true,
            send_package: false,
        };
        let warnings = print_warnings(hints, None, &caps(true, true), &a4_job());
        assert_eq!(
            warnings,
            ["页码按双面排（奇右偶左），单面打印页码会左右交替。"]
        );
    }

    #[test]
    fn send_package_printed_single_sided_warns_about_blank_pages() {
        let hints = PrintHints {
            duplex_typeset: false,
            send_package: true,
        };
        let warnings = print_warnings(hints, None, &caps(true, true), &a4_job());
        assert_eq!(
            warnings,
            ["送批材料每件从奇数页起排，单面打印会多出空白页。"]
        );
    }

    #[test]
    fn all_rotated_with_long_edge_warns_about_binding() {
        let facts = PdfFacts {
            all_rotated: true,
            has_red: false,
        };
        let job = PrintJob {
            duplex: Some(Duplex::LongEdge),
            ..a4_job()
        };
        let warnings = print_warnings(PrintHints::default(), Some(facts), &caps(true, true), &job);
        assert_eq!(warnings, ["整份横排，双面建议选短边翻转，装订边才对得上。"]);
    }

    #[test]
    fn red_header_on_a_mono_choice_or_mono_printer_warns_about_grey() {
        let facts = PdfFacts {
            all_rotated: false,
            has_red: true,
        };
        let mono = PrintJob {
            color: Some(ColorMode::Mono),
            ..a4_job()
        };
        assert_eq!(
            print_warnings(PrintHints::default(), Some(facts), &caps(true, true), &mono),
            ["版面有红色（版头或印章），黑白打印会印成灰色。"]
        );
        // 单色机即使没选颜色也要提醒。
        let plain = a4_job();
        assert_eq!(
            print_warnings(
                PrintHints::default(),
                Some(facts),
                &caps(true, false),
                &plain
            ),
            ["版面有红色（版头或印章），黑白打印会印成灰色。"]
        );
        // 彩色打印不提醒。
        let color = PrintJob {
            color: Some(ColorMode::Color),
            ..a4_job()
        };
        assert!(
            print_warnings(
                PrintHints::default(),
                Some(facts),
                &caps(true, true),
                &color
            )
            .is_empty()
        );
    }

    #[test]
    fn non_a4_paper_at_actual_size_warns() {
        let caps = caps(true, true);
        let letter = PrintJob {
            paper: Some("1".to_string()),
            ..a4_job()
        };
        let warnings = print_warnings(PrintHints::default(), None, &caps, &letter);
        // 编号 1 不在能力列表里，名称按编号本身算，也不含 A4。
        assert_eq!(
            warnings,
            ["所选纸张不是 A4，公文按 A4 排版，建议改为「适合纸张」。"]
        );
        // A4、或改成「适合纸张」就不提醒。
        let a4 = PrintJob {
            paper: Some("9".to_string()),
            ..a4_job()
        };
        assert!(print_warnings(PrintHints::default(), None, &caps, &a4).is_empty());
        let fit = PrintJob {
            paper: Some("1".to_string()),
            scaling: Scaling::Fit,
            ..a4_job()
        };
        assert!(print_warnings(PrintHints::default(), None, &caps, &fit).is_empty());
    }

    #[test]
    fn fixed_width_names_stop_at_the_first_nul() {
        let mut padded: Vec<u16> = "A4".encode_utf16().collect();
        padded.extend([0; 8]);
        assert_eq!(fixed_wide_name(&padded), "A4");

        // 名字正好占满定长缓冲区时没有 NUL，取全长。
        let full: Vec<u16> = "Legal".encode_utf16().collect();
        assert_eq!(fixed_wide_name(&full), "Legal");

        assert_eq!(fixed_wide_name(&[0, u16::from(b'X')]), "");
        assert_eq!(fixed_wide_name(&[]), "");
    }

    #[test]
    fn unknown_capabilities_enable_nothing() {
        let caps = PrinterCaps::default();
        assert!(!caps.duplex && !caps.color);
        assert!(caps.papers.is_empty() && caps.default_paper.is_none());
        assert!(caps.trays.is_empty() && caps.default_tray.is_none());
    }

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
