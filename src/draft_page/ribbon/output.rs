//! 输出分区：一行紧凑命令，动作仍走原有导出路径。

use crate::draft_page::{DraftPage, toolbar_separator};
use crate::models::ExportSelection;
use crate::theme;
use eframe::egui;

impl DraftPage<'_> {
    pub(crate) fn ribbon_output(&mut self, ui: &mut egui::Ui) {
        self.ribbon_export_formats(ui);
        if theme::smartisan::active() && ui.available_width() >= 320.0 {
            // 宽窗左侧负责生成，右侧负责找到成品；窄窗仍连续排布并沿用横向滚动。
            // 预留目录按钮、三个成品按钮与分隔线的完整宽度，避免向左溢出覆盖格式组。
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.export_open_buttons(ui);
                toolbar_separator(ui);
                self.ribbon_output_directory(ui);
            });
            return;
        }
        toolbar_separator(ui);
        self.ribbon_output_directory(ui);
        if theme::smartisan::active() {
            toolbar_separator(ui);
        }
        self.export_open_buttons(ui);
    }

    fn ribbon_export_formats(&mut self, ui: &mut egui::Ui) {
        let has_draft = !self.doc.generated_markdown.trim().is_empty();
        let ready = !self.doc.busy && has_draft;
        let export_button = if theme::smartisan::active() {
            theme::primary_icon_button_enabled(ui, ready, theme::Icon::FileDown, "导出")
        } else {
            ui.add_enabled(
                ready,
                theme::secondary_icon_button(theme::Icon::FileDown, "导出"),
            )
        };
        if export_button
            .on_hover_text("按设置里勾选的格式导出当前审校稿，可反复导出")
            .clicked()
        {
            self.start_export_current();
        }
        // 起草页直接打印：先按覆盖策略导出一份只含 PDF 的成品，导完由外壳打开
        // 并弹打印对话框。打印能力探测一次就够（Unix 要扫 PATH），不能每帧都问。
        let can_print = crate::print::printing_available_cached();
        let print_button = ui.add_enabled(
            ready && can_print,
            theme::icon_text_button(theme::Icon::Print, "打印…"),
        );
        let print_button = if can_print {
            print_button.on_hover_text("先按覆盖策略导出一份 PDF，再打开打印对话框")
        } else {
            print_button.on_disabled_hover_text(
                "未检测到打印服务（CUPS），请导出 PDF 后用系统程序打开再打印",
            )
        };
        if print_button.clicked() {
            self.start_export_for_print();
        }
        // PDF / Word / Typst 包由右侧的成品图标一键导出并打开；Markdown 源码包
        // 没有可打开的单个成品，仍留一个临时导出入口。
        let mut button = theme::icon_text_button(theme::Icon::PencilLine, "仅 Markdown");
        if theme::smartisan::active() {
            button = button.frame_when_inactive(false);
        }
        if ui
            .add_enabled(ready, button)
            .on_hover_text(
                "这一次只出 Markdown 源码包：md 正文、稿中引用的图片，研究报告另含 references.bib",
            )
            .clicked()
        {
            self.start_export_with(ExportSelection {
                markdown: true,
                docx: false,
                pdf: false,
                typst: false,
                // 源码包只有正文，不带签收记录单。
                receipt: false,
                overwrite: self.config.export.overwrite,
            });
            if self.doc.busy {
                *self.status = "正在按「仅 Markdown」导出…".into();
            }
        }
    }

    fn ribbon_output_directory(&mut self, ui: &mut egui::Ui) {
        let tip = match self.latest_output_file() {
            Some(path) => format!("在文件管理器里定位最近导出的成品：{}", path.display()),
            None => format!("还没有导出过，打开输出目录：{}", self.config.output_dir),
        };
        if ui
            .add(theme::icon_text_button(theme::Icon::Folder, "输出目录"))
            .on_hover_text(tip)
            .clicked()
        {
            self.reveal_latest_output();
        }
    }
}
