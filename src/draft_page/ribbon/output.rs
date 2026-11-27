//! 输出分区：点哪种格式就导出哪一种，外加打印、PDF 预览与最近成品。
//!
//! 这里不设统一导出，也不预选格式：每个格式按钮本身就是那一种成品的动作。
//! 右键才是另存、复制、定位这些后续操作——先把文件出来，再谈放哪儿。

use crate::draft_page::{DraftAction, DraftPage, ExportKind, FileAction, toolbar_separator};
use crate::export;
use crate::models::ExportSelection;
use crate::theme;
use eframe::egui;
use std::path::Path;

impl DraftPage<'_> {
    pub(crate) fn ribbon_output(&mut self, ui: &mut egui::Ui) {
        self.ribbon_export_formats(ui);
        toolbar_separator(ui);
        self.ribbon_print(ui);
        // 看与找贴右缘；窄窗放不下时退回行内顺序，由功能区的横向滚动接住。
        if ui.available_width() >= 300.0 {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.ribbon_recent_output(ui);
                self.ribbon_pdf_preview(ui);
            });
            return;
        }
        toolbar_separator(ui);
        self.ribbon_pdf_preview(ui);
        self.ribbon_recent_output(ui);
    }

    /// 四个格式按钮：点谁就出谁。PDF / Word 导完打开，Typst 包定位，
    /// Markdown 只出源码包；右键另存、复制、定位。
    fn ribbon_export_formats(&mut self, ui: &mut egui::Ui) {
        let ready = !self.doc.busy && !self.doc.generated_markdown.trim().is_empty();
        // 右键菜单里的「复制 / 定位」要指向导出目录里属于本篇的最近成品。
        let stem = export::document_stem_prefix(
            &self.doc.draft,
            &export::document_title(&self.doc.draft, &self.doc.generated_markdown),
        );
        self.export_links
            .refresh(&self.config.output_dir, Some(&stem));

        let mut export = None;
        let mut markdown = false;
        let mut action = None;
        for format in OutputFormat::ALL {
            let response = ui
                .add_enabled(
                    ready,
                    theme::icon_text_button(format.icon(), format.label()),
                )
                .on_hover_text(format.hint());
            if response.clicked() {
                match format {
                    OutputFormat::Markdown => markdown = true,
                    OutputFormat::Artifact(kind) => export = Some(kind),
                }
            }
            let Some(kind) = format.kind() else {
                continue;
            };
            let path = self.export_links.path(kind).map(Path::to_path_buf);
            response.context_menu(|ui| {
                if let Some(picked) = file_menu(ui, kind, path.as_deref(), ready) {
                    action = Some(picked);
                }
            });
        }
        if markdown {
            self.start_markdown_export();
        }
        if let Some(kind) = export {
            self.start_export_and_open(kind);
        }
        if let Some(action) = action {
            self.run_file_action(action);
        }
    }

    /// 打印：在内存里排一份 PDF 再交给打印对话框，不落成品文件。
    fn ribbon_print(&mut self, ui: &mut egui::Ui) {
        let has_draft = !self.doc.generated_markdown.trim().is_empty();
        // 打印能力探测一次就够（Unix 要扫 PATH），不能每帧都问。
        let can_print = crate::print::printing_available_cached();
        let tip = format!(
            "在内存中排版当前稿件，再打开打印对话框（{}）",
            theme::primary_shortcut("P")
        );
        let print = ui.add_enabled(
            !self.doc.busy && has_draft && can_print,
            theme::icon_text_button(theme::Icon::Print, "打印…"),
        );
        let print = if can_print {
            print.on_hover_text(tip)
        } else {
            print.on_disabled_hover_text(
                "未检测到打印服务（CUPS），请导出 PDF 后用系统程序打开再打印",
            )
        };
        if print.clicked() {
            self.start_export_for_print();
        }
    }

    /// 看最终分页：不出文件，进应用内的 PDF 查看器。
    fn ribbon_pdf_preview(&mut self, ui: &mut egui::Ui) {
        let ready = !self.doc.busy && !self.doc.generated_markdown.trim().is_empty();
        if ui
            .add_enabled(
                ready,
                theme::icon_text_button(theme::Icon::FileTypePdf, "PDF 预览"),
            )
            .on_hover_text("在应用内查看当前稿件的实际分页，无需导出")
            .clicked()
        {
            self.actions.push(DraftAction::PreviewPdf(self.doc.key));
        }
    }

    /// 本篇最近一次导出的成品：点一下在文件管理器里定位，右键才打开输出目录本身。
    fn ribbon_recent_output(&mut self, ui: &mut egui::Ui) {
        let latest = self.latest_output_file();
        let tip = match &latest {
            Some(path) => format!(
                "在文件管理器里定位本篇最近一次导出的成品：\n{}",
                path.display()
            ),
            None => format!(
                "还没有导出过，点一下打开输出目录：\n{}",
                self.config.output_dir
            ),
        };
        let response = ui
            .add(theme::icon_text_button(theme::Icon::Folder, "最近成品"))
            .on_hover_text(format!("{tip}\n（右键：打开输出目录）"));
        if response.clicked() {
            self.reveal_latest_output();
        }
        let mut open_dir = false;
        response.context_menu(|ui| {
            if ui
                .add(theme::menu_item(theme::Icon::Folder, "打开输出目录"))
                .clicked()
            {
                open_dir = true;
                ui.close();
            }
        });
        if open_dir {
            self.open_output_dir();
        }
    }

    /// Markdown 只有源码包，没有能直接打开的单个成品，也不进右键菜单。
    fn start_markdown_export(&mut self) {
        self.start_export_with(ExportSelection {
            print_trace: Default::default(),
            markdown: true,
            docx: false,
            pdf: false,
            typst: false,
            // 源码包只有正文，不带签收记录单。
            receipt: false,
            referenced_vocabulary: false,
            overwrite: self.config.export.overwrite,
        });
        if self.doc.busy {
            *self.status = "正在导出 Markdown 源码包…".into();
        }
    }
}

/// 输出分区里的一格：三种能打开或定位的成品，加上只能出源码包的 Markdown。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Markdown,
    Artifact(ExportKind),
}

impl OutputFormat {
    /// 顺序按公文里的常见程度排：PDF、Word 是成品，Markdown、Typst 包是源码包。
    const ALL: [Self; 4] = [
        Self::Artifact(ExportKind::Pdf),
        Self::Artifact(ExportKind::Word),
        Self::Markdown,
        Self::Artifact(ExportKind::Typst),
    ];

    fn icon(self) -> theme::Icon {
        match self {
            Self::Markdown => theme::Icon::PencilLine,
            Self::Artifact(kind) => kind.icon(),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Markdown => "Markdown",
            Self::Artifact(kind) => kind.label(),
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::Markdown => {
                "导出 Markdown 源码包（不打开）：md 正文与稿中引用的图片，研究报告另含 references.bib"
            }
            Self::Artifact(ExportKind::Pdf) => "导出 PDF 并在应用内打开",
            Self::Artifact(ExportKind::Word) => "导出 Word 并用系统程序打开",
            Self::Artifact(ExportKind::Typst) => "导出 Typst 包并在文件管理器中定位",
        }
    }

    fn kind(self) -> Option<ExportKind> {
        match self {
            Self::Markdown => None,
            Self::Artifact(kind) => Some(kind),
        }
    }
}

/// 格式按钮的右键菜单：对已经出过的成品做另存、复制与定位。
fn file_menu(
    ui: &mut egui::Ui,
    kind: ExportKind,
    path: Option<&Path>,
    ready: bool,
) -> Option<FileAction> {
    let mut chosen = None;
    if ui
        .add_enabled(ready, theme::menu_item(theme::Icon::Save, "另存为…"))
        .on_hover_text("先选保存位置，再按当前正文生成这一种成品")
        .clicked()
    {
        chosen = Some(FileAction::SaveAs(kind));
    }
    if ui
        .add_enabled(
            path.is_some(),
            theme::menu_item(theme::Icon::Copy, "复制文件"),
        )
        .on_hover_text("把最近导出的成品放进系统剪贴板")
        .clicked()
        && let Some(path) = path
    {
        chosen = Some(FileAction::CopyToClipboard(path.to_owned()));
    }
    if ui
        .add_enabled(
            path.is_some(),
            theme::menu_item(theme::Icon::Reveal, "在文件管理器中定位"),
        )
        .clicked()
        && let Some(path) = path
    {
        chosen = Some(FileAction::Reveal(path.to_owned()));
    }
    if chosen.is_some() {
        ui.close();
    }
    chosen
}
