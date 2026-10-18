//! 稿件库页：列表/详情、导入导出、PDF 附件与状态流转。
//!
//! 页面实现已拆分到 `app/manuscript_ui/` 子模块（对话框、导入预览、列表、详情、
//! 同步收发），这里保留页面状态类型与测试。

use crate::manuscript::ManuscriptFilter;
use crate::manuscript::send_package::{PinChoice, PinPreview};
use crate::manuscript_io;
use std::path::PathBuf;

mod detail;
mod dialogs;
mod import;
mod list;
mod sync_io;

pub(crate) use dialogs::{
    archive_form, batch_delete_dialog, dialog_notice, large_dialog_size, row_delete_confirm,
};
pub(crate) use import::{import_preview_form, pending_merge_form, zip_password_form};

/// 稿件列表上的操作在遍历表格时不能直接改 `self`，先记下来循环结束后再执行。
pub(crate) enum ManuscriptAction {
    /// 打开只读详情（归档行或“查看”）。
    Detail(i64),
    /// 载入起草页继续编辑。
    Edit(i64),
    /// 复制该稿件的行文要素和正文，作为一篇尚未入库的新稿载入起草页。
    CreateFromExisting(i64),
    Publish(i64),
    RevertToDraft(i64),
    /// 归档。PDF 附件来自 `manuscript_archive_pending` 中已选的文件。
    Archive(i64),
    Delete(i64),
    DeleteSelected(Vec<i64>),
    /// 进入删除二次确认。
    DeletePending(i64),
    /// 进入归档确认（先选扫描盖章 PDF）。
    ArchivePending(i64),
    /// 打开某篇呈批件的送批材料面板。
    OpenSendPackage(i64),
    /// 打开版本对照窗，对照该版本与其上一版（旧在左、新在右）。
    DiffVersion {
        manuscript_id: i64,
        version_number: i64,
    },
    /// 打开版本对照窗，默认对照最新版与上一版。
    OpenVersionDiff {
        manuscript_id: i64,
    },
    /// 把某版本载入起草页继续编辑（不写活稿行）。
    LoadVersion {
        manuscript_id: i64,
        version_number: i64,
    },
    /// 进入"回退到该版本"的二次确认。
    RevertPending {
        manuscript_id: i64,
        version_number: i64,
    },
}

/// 详情窗内 PDF 附件的操作，同样延迟到帧末执行。
pub(crate) enum PdfAction {
    Open(i64),
    SaveAs(i64),
    Delete(i64),
}

/// 等待二次确认的归档操作：记录要归档的稿件与已选择的扫描盖章 PDF。
pub(crate) struct ArchivePending {
    manuscript_id: i64,
    pdf_paths: Vec<PathBuf>,
    /// 挂着送批材料时的钉版预览；进入确认时查一次。
    send_package: Option<PinPreview>,
    pin_choice: PinChoice,
}

/// 「导出 PDF」选项弹窗的勾选状态：盖章件取附件、非盖章件编译生成。
/// `ids` 是打开弹窗那一刻的勾选，之后一路带到导出，不再回头读当前勾选。
#[derive(Debug, Clone)]
pub(crate) struct PdfExportDialog {
    stamped: bool,
    compiled: bool,
    ids: Vec<i64>,
}

/// 批量删除确认框的内容，点「批量删除」那一刻算好、冻结。
#[derive(Debug, Clone)]
pub(crate) struct BatchDeleteConfirm {
    /// 真正要删的稿件。
    deletable: Vec<i64>,
    /// 归档稿件，受保护不删。
    archived: usize,
    /// 已归档呈批件的送批材料，受保护不删。
    packaged: usize,
}

/// ZIP 密码框之后要做的事。导出范围在打开密码框时就定下来（所选稿件的 id、
/// 或当时的过滤条件），密码框开着时改勾选、改筛选都不影响这次导出。
#[derive(Debug, Clone)]
pub(crate) enum ZipPasswordPurpose {
    FilteredExport(ManuscriptFilter),
    SelectedExport(Vec<i64>),
    PdfExport(manuscript_io::PdfExportOptions, Vec<i64>),
    Import(PathBuf),
}

impl ZipPasswordPurpose {
    fn is_import(&self) -> bool {
        matches!(self, Self::Import(_))
    }
}

/// 每次 ZIP 导入/导出都显示的密码弹窗。导出要二次确认并检查强度；导入只验证非空，
/// 以便兼容由其他工具生成的历史压缩包。
pub(crate) struct ZipPasswordDialog {
    purpose: ZipPasswordPurpose,
    password: String,
    confirmation: String,
    remember: bool,
    show_password: bool,
    /// 导出同步包时连同所选呈批件的送批材料一起打包（默认勾选）。
    include_send_package: bool,
    error: Option<String>,
}

impl ZipPasswordDialog {
    fn new(purpose: ZipPasswordPurpose, remembered: Option<&str>) -> Self {
        let password = remembered.unwrap_or_default().to_string();
        Self {
            purpose,
            confirmation: password.clone(),
            password,
            remember: remembered.is_some(),
            show_password: false,
            include_send_package: true,
            error: None,
        }
    }
}

/// 导入 ZIP 的预览状态：清单、逐篇身份判定、动作与过滤条件。
pub(crate) struct ImportPreview {
    manifest: manuscript_io::Manifest,
    zip_path: PathBuf,
    selected: Vec<bool>,
    keyword: String,
    relations: Vec<manuscript_io::sync::RecordPreview>,
    actions: Vec<manuscript_io::sync::ImportAction>,
    reviews: Vec<Option<MergeReview>>,
    manifest_hash: String,
    /// 每篇主件的送批材料清单差异（两边一致或包里没带清单时为 None）。
    package_diffs: Vec<Option<manuscript_io::sync::PackageDifference>>,
    focused: Option<usize>,
    /// 包内随附的标准词库；旧包无此条目时为 None。
    vocabulary: Option<manuscript_io::VocabularyFile>,
    /// 是否把包内词库增量合并到本机词库，默认勾选。
    merge_vocabulary: bool,
    /// 仅在这次预览/确认导入期间驻留内存，不写入稿件或应用配置。
    password: String,
    /// 上次确认没能执行的原因，显示在预览框里（框是模态的，状态栏被遮罩压着容易看漏）。
    notice: Option<String>,
}

pub(crate) struct PendingMergeDialog {
    manuscript_id: i64,
    preview: manuscript_io::sync::PendingPreview,
    choices: Vec<bool>,
    markdown_override: Option<String>,
    take_status: bool,
    review: Option<MergeReview>,
    /// 上次确认没能执行的原因，显示在合并框里。
    notice: Option<String>,
}

pub(crate) struct MergeReview {
    hash: String,
    parsed_blocks: usize,
    warnings: Vec<String>,
    notes: Vec<String>,
}

#[cfg(test)]
mod dialog_snapshot_tests {
    //! 稿件管理页几个模态框的样张：
    //! `cargo test --locked dialog_snapshot -- --ignored`，输出到 `tmp/*.png`，改版时目视检查。
    use super::*;
    use crate::modal::{self, Dismiss};
    use crate::models::{ManuscriptStatus, TemplateKind};
    use crate::theme;

    /// 背后摆一点稿件列表的样子，再画 `dialog`，渲染成 `tmp/{name}.png`。
    fn snapshot(name: &str, size: egui::Vec2, mut dialog: impl FnMut(&mut egui::Ui)) {
        let ctx = egui::Context::default();
        theme::configure_icons(&ctx);
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        theme::configure_style(&ctx);
        ctx.set_pixels_per_point(2.0);
        let mut canvas = crate::ui_snapshot::Canvas::default();
        let mut frame = || {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                ..Default::default()
            };
            ctx.run_ui(raw, |ui| {
                ui.heading("稿件管理");
                for title in [
                    "关于开展安全检查的通知",
                    "关于调整会议时间的函",
                    "第三季度工作总结",
                ] {
                    ui.label(title);
                }
                dialog(ui);
            })
        };
        // 多跑几帧，等模态的淡入动画走完（默认每帧按 1/60 秒推进）。
        for _ in 0..30 {
            let output = frame();
            canvas.absorb(&output.textures_delta);
        }
        let output = frame();
        canvas.absorb(&output.textures_delta);
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tmp")
            .join(format!("{name}.png"));
        canvas.render(&ctx, output, size, theme::canvas(), &path);
        println!("{}", path.display());
    }

    #[test]
    #[ignore]
    fn batch_delete_dialog_snapshot() {
        let confirm = BatchDeleteConfirm {
            deletable: vec![1, 2, 3],
            archived: 1,
            packaged: 1,
        };
        snapshot("batch-delete-dialog", egui::vec2(720.0, 420.0), |ui| {
            batch_delete_dialog(ui.ctx(), &confirm);
        });
    }

    #[test]
    #[ignore]
    fn archive_dialog_snapshot() {
        let mut pending = ArchivePending {
            manuscript_id: 1,
            pdf_paths: vec![
                PathBuf::from("扫描件-盖章页.pdf"),
                PathBuf::from("扫描件-附件一.pdf"),
            ],
            send_package: None,
            pin_choice: PinChoice::default(),
        };
        snapshot("archive-dialog", egui::vec2(760.0, 480.0), |ui| {
            modal::dialog(
                ui.ctx(),
                egui::Id::new("manuscript_archive"),
                "归档稿件",
                460.0,
                Dismiss::EscOnly,
                |ui| archive_form(ui, &mut pending, "关于开展安全检查的通知"),
            );
        });
    }

    #[test]
    #[ignore]
    fn import_preview_dialog_snapshot() {
        use manuscript_io::sync::{ImportAction, RecordPreview, Relationship};
        let record = |title: &str, date: &str| manuscript_io::ManifestRecord {
            id: 0,
            title: title.into(),
            kind: TemplateKind::default(),
            status: ManuscriptStatus::Draft,
            doc_number: String::new(),
            doc_date: date.into(),
            notes: String::new(),
            content_markdown: format!("# {title}\n\n正文。\n"),
            snapshot: crate::models::DraftInput::default(),
            created_at: String::new(),
            updated_at: String::new(),
            published_at: None,
            archived_at: None,
            pdfs: Vec::new(),
            document_uuid: None,
            aliases: Vec::new(),
            head_revision_uuid: None,
            revisions: Vec::new(),
            send_package: None,
        };
        let records = vec![
            record("关于开展安全检查的通知", "2026-09-28"),
            record("关于调整会议时间的函", "2026-09-30"),
            record("第三季度工作总结", "2026-10-02"),
        ];
        let relation = || RecordPreview {
            relationship: Relationship::New,
            local_id: None,
            local_head: None,
            local_status: None,
            attachments_changed: false,
            candidates: Vec::new(),
            base: None,
            proposal: None,
            local_hash: None,
            local_markdown: None,
            incoming_hash: "0".repeat(64),
        };
        let count = records.len();
        let mut preview = ImportPreview {
            manifest: manuscript_io::Manifest {
                schema: 2,
                exported_at: String::new(),
                records,
                assets: Vec::new(),
            },
            zip_path: PathBuf::new(),
            selected: vec![true; count],
            keyword: String::new(),
            relations: (0..count).map(|_| relation()).collect(),
            actions: vec![ImportAction::New; count],
            reviews: (0..count).map(|_| None).collect(),
            manifest_hash: String::new(),
            package_diffs: vec![None; count],
            focused: Some(0),
            vocabulary: None,
            merge_vocabulary: true,
            password: String::new(),
            notice: Some("请先逐篇查看合并候选和规则复核结果，再确认导入。".into()),
        };
        let config = crate::models::AppConfig::default();
        let size = egui::vec2(1100.0, 760.0);
        snapshot("import-preview-dialog", size, |ui| {
            let (width, body) = large_dialog_size(ui.ctx());
            modal::dialog(
                ui.ctx(),
                egui::Id::new("manuscript_import_preview"),
                "导入预览",
                width,
                Dismiss::ButtonsOnly,
                |ui| import_preview_form(ui, &mut preview, &config, body),
            );
        });
    }
}
