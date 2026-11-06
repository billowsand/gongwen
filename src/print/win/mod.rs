//! Windows 后端：Win32 枚举打印机，按所选打印机建 DEVMODE 与设备上下文，
//! hayro 光栅化后经 GDI 出纸。老式 `PrintDlg` 系统对话框留作「使用系统对话框…」。
//!
//! **打印任务去标识**（与 CUPS 后端同一口径）：加密工作区放行打印后，送进
//! spooler 的文档名固定为「公文助手打印任务」，送打前还把 PDF 复制成缓存目录里
//! 随机命名的副本，公文标题与真实路径都不进打印队列；见 [`StagedDocument`]。

mod gdi;
mod printers;

pub(super) use printers::{edit_preferences, list_printers, printer_caps, printer_state};

use super::pages::{self, PageSpan};
use super::{PrintJob, PrintOutcome, Scaling};
use crate::file_limits::MAX_PDF_BYTES;
use gdi::{DcGuard, Document, GlobalMemory};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use windows_sys::Win32::UI::Controls::Dialogs::{
    CommDlgExtendedError, PD_PAGENUMS, PD_RETURNDC, PD_USEDEVMODECOPIESANDCOLLATE, PRINTDLGW,
    PrintDlgW,
};

/// 送打用副本所在的缓存子目录（相对 [`crate::storage::cache_dir`]），与 CUPS
/// 后端放在同一处：两套后端的临时正文都不落在配置目录或共享系统临时目录里。
const STAGED_PRINT_DIR: &str = "print";

/// 送打用的中性 PDF 副本：spooler 与临时目录只看得到随机文件名，公文标题和
/// 真实路径都不出现。析构即删除，覆盖成功、失败与提前返回三种收尾。
struct StagedDocument {
    path: PathBuf,
}

impl StagedDocument {
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StagedDocument {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// 把 PDF 字节复制到 `缓存目录/print/<uuid>.pdf`（Unix 0600，由
/// [`crate::storage::atomic_write`] 保证），返回负责删除的副本。
fn stage_document(source: &Path) -> Result<StagedDocument, String> {
    let dir = crate::storage::cache_dir()
        .map_err(|error| format!("无法确定打印缓存目录：{error}"))?
        .join(STAGED_PRINT_DIR);
    stage_document_in(&dir, source)
}

/// [`stage_document`] 的可测试入口：显式指定副本目录。
fn stage_document_in(dir: &Path, source: &Path) -> Result<StagedDocument, String> {
    let bytes = crate::file_limits::read_bytes(source, MAX_PDF_BYTES, "要打印的 PDF")
        .map_err(|error| format!("无法读取 {}：{error}", source.display()))?;
    let path = dir.join(format!("{}.pdf", uuid::Uuid::new_v4()));
    crate::storage::atomic_write(&path, |file| {
        file.write_all(&bytes)?;
        Ok(())
    })
    .map_err(|error| format!("无法准备打印副本：{error}"))?;
    Ok(StagedDocument { path })
}

pub(super) fn print(
    job: &PrintJob,
    progress: &dyn Fn(u32, u32),
    cancel: &AtomicBool,
) -> Result<PrintOutcome, String> {
    // 先复制出中性副本，后面只读副本：公文标题与真实路径不进打印链路。
    // 副本在本函数返回（成功、失败、取消、早退）时由析构删除。
    let staged = stage_document(&job.path)?;
    let document = Document::open(staged.path())?;
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
pub(super) fn print_with_system_dialog(path: &Path) -> Result<PrintOutcome, String> {
    let staged = stage_document(path)?;
    let document = Document::open(staged.path())?;
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

    /// 送打副本落在给定目录、随机命名、内容一致，打印返回后删除。
    #[test]
    fn staged_copy_hides_the_document_title() {
        let source_dir = tempfile::tempdir().expect("临时目录");
        let source = source_dir.path().join("关于××的请示.pdf");
        std::fs::write(&source, b"%PDF-1.7\n").expect("写入假 PDF");

        let stage = tempfile::tempdir().expect("临时目录");
        let staged = stage_document_in(stage.path(), &source).expect("应能准备副本");
        let path = staged.path().to_path_buf();
        assert_eq!(path.parent(), Some(stage.path()));
        assert_eq!(path.extension().and_then(|ext| ext.to_str()), Some("pdf"));
        let name = path
            .file_name()
            .expect("有文件名")
            .to_string_lossy()
            .into_owned();
        assert!(!name.contains("请示"), "副本名不能带公文标题：{name}");
        assert_eq!(std::fs::read(&path).expect("读副本"), b"%PDF-1.7\n");
        drop(staged);
        assert!(!path.exists(), "打印返回（含失败）后必须删掉副本");
    }

    /// 原 PDF 读不到时连副本都不建，也不会把路径交给打印机。
    #[test]
    fn staging_a_missing_pdf_leaves_no_copy() {
        let stage = tempfile::tempdir().expect("临时目录");
        let missing = stage.path().join("missing.pdf");
        assert!(stage_document_in(stage.path(), &missing).is_err());
        assert_eq!(
            std::fs::read_dir(stage.path()).expect("读目录").count(),
            0,
            "失败时不该留下任何文件"
        );
    }
}
