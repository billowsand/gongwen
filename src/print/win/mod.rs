//! Windows 后端：Win32 枚举打印机，按所选打印机建 DEVMODE 与设备上下文，
//! hayro 光栅化后经 GDI 出纸。老式 `PrintDlg` 系统对话框留作「使用系统对话框…」。
//!
//! **打印任务去标识**（与 CUPS 后端同一口径）：加密工作区放行打印后，送进
//! spooler 的文档名固定为「公文助手打印任务」，PDF 直接从内存解析，
//! 不创建送打缓存文件，公文标题与真实路径不进打印队列。

mod gdi;
mod printers;

pub(super) use printers::{edit_preferences, list_printers, printer_caps, printer_state};

use super::pages::{self, PageSpan};
use super::{PrintJob, PrintOutcome, Scaling};
use crate::file_limits::MAX_PDF_BYTES;
use gdi::{DcGuard, Document, GlobalMemory};
use std::sync::atomic::AtomicBool;
use windows_sys::Win32::UI::Controls::Dialogs::{
    CommDlgExtendedError, PD_PAGENUMS, PD_RETURNDC, PD_USEDEVMODECOPIESANDCOLLATE, PRINTDLGW,
    PrintDlgW,
};

/// 直接在内存中解析 PDF；交给 GDI 的文档名始终为中性名称。
pub(super) fn print(
    job: &PrintJob,
    progress: &dyn Fn(u32, u32),
    cancel: &AtomicBool,
) -> Result<PrintOutcome, String> {
    let bytes = match &job.bytes {
        Some(bytes) => bytes.as_ref().clone(),
        None => crate::file_limits::read_bytes(&job.path, MAX_PDF_BYTES, "打印文件")
            .map_err(|e| e.to_string())?,
    };
    let document = Document::from_bytes(bytes)?;
    let page_count = u32::try_from(document.page_count()).unwrap_or(u32::MAX);
    let spans = clip_spans(job.pages.as_deref(), page_count);
    if spans.is_empty() {
        return Err("所选页码超出了文档页数".to_string());
    }
    let name = job
        .printer
        .clone()
        .or_else(printers::default_printer)
        .ok_or_else(|| "本机没有设置默认打印机，请在对话框里选一台".to_string())?;
    let device = printers::open_device(&name, job)?;
    let (copies, collate) = if device.driver_copies {
        (1, false)
    } else {
        (job.copies, job.collate)
    };
    let total = u32::try_from(pages::span_page_count(&spans).saturating_mul(u64::from(copies)))
        .unwrap_or(u32::MAX);
    // 份数乘页数可能很大，出纸顺序按迭代器走，不展开成数组。只有手动双面第二轮
    // 倒序，那时固定一份，展开也只有页数那么长。
    let forward = pages::print_sequence(&spans, copies, collate);
    let sequence: Box<dyn Iterator<Item = u32> + '_> = if job.reverse {
        let mut all: Vec<u32> = forward.collect();
        all.reverse();
        Box::new(all.into_iter())
    } else {
        Box::new(forward)
    };
    // 文档名由 `gdi::Document::print` 固定成中性文字，这里不传标题。
    let status = document.print(
        &device.dc,
        sequence,
        total,
        job.scaling == Scaling::Fit,
        &gdi::Feedback { progress, cancel },
    )?;
    Ok(match status {
        gdi::PrintStatus::Done => PrintOutcome::Printed { job: None },
        gdi::PrintStatus::Cancelled => PrintOutcome::Cancelled,
    })
}

/// 页码区间截到文档实际页数；`None` 是全部页。
fn clip_spans(spans: Option<&[PageSpan]>, page_count: u32) -> Vec<PageSpan> {
    match spans {
        None => vec![(1, page_count)],
        Some(spans) => spans
            .iter()
            .filter(|(start, _)| *start <= page_count)
            .map(|&(start, end)| (start, end.min(page_count)))
            .collect(),
    }
}

/// 系统自带的打印对话框（可选打印机、页码范围、份数，以及驱动的首选项）。
///
/// 与 [`print`] 一样先复制中性副本：对话框路径原来拿文件名当文档名，文件名往往
/// 就是公文标题；现在两处都只给中性文档名与随机副本。
pub(super) fn print_with_system_dialog(bytes: Vec<u8>) -> Result<PrintOutcome, String> {
    let document = Document::from_bytes(bytes)?;
    // PrintDlg 的页码字段是 u16，超出部分按最大页截断。
    let max_page =
        u16::try_from(document.page_count().min(usize::from(u16::MAX))).unwrap_or(u16::MAX);

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
    let copies = u32::from(pd.nCopies.max(1));
    let spans = [(u32::from(from), u32::from(to))];
    let total = u32::try_from(pages::span_page_count(&spans).saturating_mul(u64::from(copies)))
        .unwrap_or(u32::MAX);
    let sequence = pages::print_sequence(&spans, copies, true);
    // 系统对话框自己就是入口，这里不报进度、也不取消。
    let cancel = AtomicBool::new(false);
    // 系统对话框里驱动自己的设置说了算，程序只按实际大小出图。
    let feedback = gdi::Feedback {
        progress: &|_, _| {},
        cancel: &cancel,
    };
    // 文档名固定中性文字，不再用文件名（文件名通常就是公文标题）。
    let status = document.print(&dc, sequence, total, false, &feedback)?;
    Ok(match status {
        gdi::PrintStatus::Done => PrintOutcome::Printed { job: None },
        gdi::PrintStatus::Cancelled => PrintOutcome::Cancelled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// spooler 里的文档名是固定中性文字，公文标题进不去。
    #[test]
    fn spooler_document_name_is_neutral() {
        assert_eq!(gdi::NEUTRAL_DOCUMENT_NAME, "公文助手打印任务");
        assert!(!gdi::NEUTRAL_DOCUMENT_NAME.contains("请示"));
    }
}
