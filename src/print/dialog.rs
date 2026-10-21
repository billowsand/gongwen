//! 应用内打印对话框。第一期：打印机（默认标记与状态）、份数、逐份打印、页码范围。
//!
//! 对话框只收集选择，不碰任何打印接口：打印机清单与状态由外壳在后台线程查好后
//! 经 [`PrintDialog::printers_loaded`] / [`PrintDialog::printer_state`] 送进来，
//! 点「打印」交回一个 [`PrintJob`]，同样由外壳放后台线程执行。

use super::pages::{self, PageSpan};
use super::{MAX_COPIES, PrintJob, PrintRequest, PrinterInfo, PrinterState};
use crate::modal::{self, Dismiss};
use crate::theme;
use eframe::egui;
use std::path::Path;

/// 对话框的结果。
pub(crate) enum DialogOutcome {
    Print(PrintJob),
    /// 改用系统自带的打印对话框（只有 Windows 有这个按钮）。
    #[cfg_attr(not(windows), allow(dead_code))]
    SystemDialog,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Range {
    All,
    Current,
    Custom,
}

enum Printers {
    Loading,
    Loaded(Vec<PrinterInfo>),
    /// 列不出来（Unix 上没有 `lpstat` 之类）。仍可交给系统默认打印机。
    Failed(String),
}

pub(crate) struct PrintDialog {
    /// 每次打开递增。后台线程的结果带着它回来，对不上的是上一个对话框的，丢掉。
    id: u64,
    request: PrintRequest,
    printers: Printers,
    /// `None` 表示系统默认打印机。
    selected: Option<String>,
    /// 上次用的打印机，清单到了以后优先选它。
    remembered: String,
    copies: u32,
    collate: bool,
    range: Range,
    custom: String,
}

impl PrintDialog {
    pub(crate) fn new(id: u64, request: PrintRequest, remembered: &str) -> Self {
        Self {
            id,
            request,
            printers: Printers::Loading,
            selected: None,
            remembered: remembered.to_string(),
            copies: 1,
            collate: true,
            range: Range::All,
            custom: String::new(),
        }
    }

    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    pub(crate) fn path(&self) -> &Path {
        &self.request.path
    }

    /// 打印机清单到了。默认选中顺序：上次用的 → 系统默认 → 第一台。
    pub(crate) fn printers_loaded(&mut self, result: Result<Vec<PrinterInfo>, String>) {
        match result {
            Ok(printers) => {
                let pick = printers
                    .iter()
                    .find(|printer| printer.name == self.remembered)
                    .or_else(|| printers.iter().find(|printer| printer.is_default))
                    .or_else(|| printers.first());
                self.selected = pick.map(|printer| printer.name.clone());
                self.printers = Printers::Loaded(printers);
            }
            Err(error) => {
                self.selected = None;
                self.printers = Printers::Failed(error);
            }
        }
    }

    pub(crate) fn printer_state(&mut self, name: &str, state: PrinterState) {
        if let Printers::Loaded(printers) = &mut self.printers
            && let Some(printer) = printers.iter_mut().find(|printer| printer.name == name)
        {
            printer.state = state;
        }
    }

    /// 画对话框。返回 `Some` 时调用方关掉它并照办。
    pub(crate) fn ui(&mut self, ctx: &egui::Context) -> Option<DialogOutcome> {
        let dialog = modal::dialog(
            ctx,
            egui::Id::new("print_dialog"),
            "打印",
            460.0,
            Dismiss::EscOnly,
            |ui| self.contents(ui),
        );
        if dialog.inner.is_some() {
            return dialog.inner;
        }
        dialog.dismissed.then_some(DialogOutcome::Cancel)
    }

    fn contents(&mut self, ui: &mut egui::Ui) -> Option<DialogOutcome> {
        ui.add(
            egui::Label::new(egui::RichText::new(&self.request.title).color(theme::text_soft()))
                .truncate(),
        );
        ui.add_space(10.0);
        let pages = self.pages();
        egui::Grid::new("print_dialog_grid")
            .num_columns(2)
            .striped(false)
            .spacing([14.0, 10.0])
            .show(ui, |ui| {
                ui.label("打印机");
                self.printer_row(ui);
                ui.end_row();
                ui.label("份数");
                self.copies_row(ui);
                ui.end_row();
                // 页码一栏有好几行，标签贴顶，与第一个单选框齐平。
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.label("页码");
                });
                self.range_rows(ui, &pages);
                ui.end_row();
            });
        ui.add_space(8.0);
        self.notes(ui, &pages);
        ui.add_space(12.0);
        self.buttons(ui, &pages)
    }

    fn printer_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| match &self.printers {
            Printers::Loading => {
                ui.spinner();
                ui.label(egui::RichText::new("正在查找打印机…").color(theme::text_muted()));
            }
            Printers::Failed(_) => {
                ui.label("系统默认打印机");
            }
            Printers::Loaded(printers) if printers.is_empty() => {
                ui.colored_label(theme::warn(), "未找到打印机");
            }
            Printers::Loaded(printers) => {
                let selected_text = self
                    .selected
                    .as_deref()
                    .and_then(|name| printers.iter().find(|printer| printer.name == name))
                    .map(display_name)
                    .unwrap_or_else(|| "请选择打印机".to_string());
                egui::ComboBox::from_id_salt("print_dialog_printer")
                    .width(250.0)
                    .truncate()
                    .selected_text(selected_text)
                    .show_ui(ui, |ui| {
                        for printer in printers {
                            ui.selectable_value(
                                &mut self.selected,
                                Some(printer.name.clone()),
                                display_name(printer),
                            );
                        }
                    });
                if let Some(printer) = self
                    .selected
                    .as_deref()
                    .and_then(|name| printers.iter().find(|printer| printer.name == name))
                {
                    state_badge(ui, &printer.state);
                }
            }
        });
    }

    fn copies_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add(
                egui::DragValue::new(&mut self.copies)
                    .range(1..=MAX_COPIES)
                    .speed(0.1),
            );
            ui.add_space(8.0);
            ui.add_enabled(
                self.copies > 1,
                egui::Checkbox::new(&mut self.collate, "逐份打印"),
            )
            .on_hover_text("一份打完再打下一份；不勾则每页连打几张");
        });
    }

    fn range_rows(&mut self, ui: &mut egui::Ui, pages: &Result<Selection, String>) {
        let count = self.request.page_count;
        ui.vertical(|ui| {
            let all = if count > 0 {
                format!("全部（共 {count} 页）")
            } else {
                "全部".to_string()
            };
            ui.radio_value(&mut self.range, Range::All, all);
            ui.add_enabled_ui(count > 0, |ui| {
                let current = format!("当前页（第 {} 页）", self.request.current_page + 1);
                ui.radio_value(&mut self.range, Range::Current, current);
            });
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.range, Range::Custom, "自定义");
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.custom)
                        .hint_text("例如 1-3,5")
                        .desired_width(150.0),
                );
                // 往框里点、往框里打字，都意味着要按自定义页码打。
                if edit.gained_focus() || edit.changed() {
                    self.range = Range::Custom;
                }
            });
            if self.range == Range::Custom {
                match pages {
                    Err(error) if !self.custom.trim().is_empty() => {
                        ui.colored_label(theme::danger(), error);
                    }
                    Ok(selection) if selection.truncated => {
                        ui.label(
                            egui::RichText::new(format!("超出部分已截到末页（共 {count} 页）"))
                                .color(theme::text_muted()),
                        );
                    }
                    _ => {}
                }
            }
        });
    }

    fn notes(&self, ui: &mut egui::Ui, pages: &Result<Selection, String>) {
        if let Ok(selection) = pages {
            let summary = match selection.page_total {
                Some(total) => {
                    let sheets = total * u64::from(self.copies);
                    if self.copies > 1 {
                        format!("共 {total} 页 × {} 份，单面约 {sheets} 张纸", self.copies)
                    } else {
                        format!("共 {total} 页，单面约 {sheets} 张纸")
                    }
                }
                None => format!("全部页面 × {} 份", self.copies),
            };
            ui.label(egui::RichText::new(summary).color(theme::text_soft()));
        }
        if self.copies > 1 {
            muted(
                ui,
                "打印机份数只是把同一份重复打出；份号不同的多份请在导出时生成。",
            );
        }
        match &self.printers {
            Printers::Failed(error) => {
                ui.colored_label(
                    theme::warn(),
                    format!("无法列出打印机（{error}），将送往系统默认打印机。"),
                );
            }
            Printers::Loaded(printers) if printers.is_empty() => {
                ui.colored_label(theme::warn(), "本机没有安装打印机，请先在系统设置里添加。");
            }
            Printers::Loaded(printers) => {
                if let Some(printer) = self
                    .selected
                    .as_deref()
                    .and_then(|name| printers.iter().find(|printer| printer.name == name))
                    && printer.state.healthy() == Some(false)
                {
                    ui.colored_label(
                        theme::warn(),
                        format!(
                            "这台打印机当前{}，任务会先排进打印队列，处理好后再出纸。",
                            printer.state.label()
                        ),
                    );
                }
            }
            Printers::Loading => {}
        }
    }

    fn buttons(
        &mut self,
        ui: &mut egui::Ui,
        pages: &Result<Selection, String>,
    ) -> Option<DialogOutcome> {
        let no_printer =
            matches!(&self.printers, Printers::Loaded(printers) if printers.is_empty());
        let can_print = pages.is_ok() && !no_printer;
        let mut outcome = None;
        ui.horizontal(|ui| {
            #[cfg(windows)]
            if ui
                .button("使用系统对话框…")
                .on_hover_text(
                    "改用 Windows 自带的打印对话框，可设置驱动独有的选项（装订、打孔等）",
                )
                .clicked()
            {
                outcome = Some(DialogOutcome::SystemDialog);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary_icon_button_enabled(ui, can_print, theme::Icon::Print, "打印")
                    .clicked()
                {
                    outcome = self.job(pages).map(DialogOutcome::Print);
                }
                if ui.button("取消").clicked() {
                    outcome = Some(DialogOutcome::Cancel);
                }
            });
        });
        // 回车即打印：下拉开着时回车归下拉。
        let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
        if outcome.is_none() && can_print && enter && !egui::Popup::is_any_open(ui.ctx()) {
            outcome = self.job(pages).map(DialogOutcome::Print);
        }
        outcome
    }

    fn job(&self, pages: &Result<Selection, String>) -> Option<PrintJob> {
        let selection = pages.as_ref().ok()?;
        Some(PrintJob {
            path: self.request.path.clone(),
            title: self.request.title.clone(),
            printer: self.selected.clone(),
            copies: self.copies.clamp(1, MAX_COPIES),
            collate: self.copies > 1 && self.collate,
            pages: selection.spans.clone(),
            // 第二期才在对话框里给这几项；现在先按「沿用打印机默认」提交。
            duplex: None,
            color: None,
            paper: None,
            tray: None,
            scaling: super::Scaling::Actual,
        })
    }

    /// 当前页码选择解析出的结果。
    fn pages(&self) -> Result<Selection, String> {
        let count = u32::try_from(self.request.page_count).unwrap_or(u32::MAX);
        match self.range {
            Range::All => Ok(Selection {
                spans: None,
                page_total: (count > 0).then_some(u64::from(count)),
                truncated: false,
            }),
            Range::Current => {
                let page = u32::try_from(self.request.current_page)
                    .unwrap_or(u32::MAX)
                    .saturating_add(1);
                if count == 0 || page > count {
                    return Err("当前页未知".to_string());
                }
                Ok(Selection {
                    spans: Some(vec![(page, page)]),
                    page_total: Some(1),
                    truncated: false,
                })
            }
            Range::Custom => {
                let parsed = pages::parse_ranges(&self.custom, count)?;
                Ok(Selection {
                    page_total: Some(pages::span_page_count(&parsed.spans)),
                    spans: Some(parsed.spans),
                    truncated: parsed.truncated,
                })
            }
        }
    }
}

/// 解析好的页码选择。
struct Selection {
    /// `None` 是全部页。
    spans: Option<Vec<PageSpan>>,
    /// 要打的页数；全部页而总页数未知时为 `None`。
    page_total: Option<u64>,
    truncated: bool,
}

fn display_name(printer: &PrinterInfo) -> String {
    if printer.is_default {
        format!("{}（默认）", printer.name)
    } else {
        printer.name.clone()
    }
}

/// 打印机名后面的状态点与文字。
fn state_badge(ui: &mut egui::Ui, state: &PrinterState) {
    let color = match state.healthy() {
        Some(true) => theme::success(),
        Some(false) => theme::warn(),
        None => theme::text_muted(),
    };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
    ui.label(egui::RichText::new(state.label()).color(color));
}

fn muted(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(theme::text_muted()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dialog(page_count: usize) -> PrintDialog {
        PrintDialog::new(
            1,
            PrintRequest {
                path: PathBuf::from("/tmp/请示.pdf"),
                title: "关于××的请示".to_string(),
                page_count,
                current_page: 2,
            },
            "Office",
        )
    }

    fn printer(name: &str, is_default: bool) -> PrinterInfo {
        PrinterInfo {
            name: name.to_string(),
            is_default,
            state: PrinterState::Unknown,
        }
    }

    #[test]
    fn remembered_printer_wins_over_the_system_default() {
        let mut dialog = dialog(5);
        dialog.printers_loaded(Ok(vec![printer("HP", true), printer("Office", false)]));
        assert_eq!(dialog.selected.as_deref(), Some("Office"));

        let mut dialog = PrintDialog::new(2, dialog.request.clone(), "已删除的打印机");
        dialog.printers_loaded(Ok(vec![printer("HP", false), printer("Canon", true)]));
        assert_eq!(dialog.selected.as_deref(), Some("Canon"));

        dialog.printers_loaded(Ok(vec![printer("HP", false)]));
        assert_eq!(dialog.selected.as_deref(), Some("HP"));
    }

    #[test]
    fn failed_listing_falls_back_to_the_system_default() {
        let mut dialog = dialog(5);
        dialog.printers_loaded(Err("未找到 lpstat".to_string()));
        assert_eq!(dialog.selected, None);
        let job = dialog.job(&dialog.pages()).expect("仍可打印");
        assert_eq!(job.printer, None);
    }

    #[test]
    fn state_updates_reach_the_matching_printer() {
        let mut dialog = dialog(5);
        dialog.printers_loaded(Ok(vec![printer("HP", true)]));
        dialog.printer_state("HP", PrinterState::Offline);
        dialog.printer_state("不存在", PrinterState::Ready);
        let Printers::Loaded(printers) = &dialog.printers else {
            panic!("清单应已载入");
        };
        assert_eq!(printers[0].state, PrinterState::Offline);
    }

    #[test]
    fn page_choices_become_spans() {
        let mut dialog = dialog(10);
        dialog.printers_loaded(Ok(vec![printer("Office", true)]));
        let job = dialog.job(&dialog.pages()).expect("全部");
        assert_eq!(job.pages, None);

        dialog.range = Range::Current;
        let job = dialog.job(&dialog.pages()).expect("当前页");
        assert_eq!(job.pages, Some(vec![(3, 3)]));

        dialog.range = Range::Custom;
        dialog.custom = "1-3，5".to_string();
        dialog.copies = 2;
        dialog.collate = false;
        let job = dialog.job(&dialog.pages()).expect("自定义");
        assert_eq!(job.pages, Some(vec![(1, 3), (5, 5)]));
        assert_eq!((job.copies, job.collate), (2, false));

        dialog.custom = "12".to_string();
        assert!(dialog.job(&dialog.pages()).is_none());
    }

    #[test]
    fn single_copy_never_asks_for_collation() {
        let dialog = dialog(3);
        let job = dialog.job(&dialog.pages()).expect("全部");
        assert_eq!((job.copies, job.collate), (1, false));
    }

    #[test]
    fn unknown_page_count_only_allows_printing_everything() {
        let mut dialog = dialog(0);
        assert!(dialog.job(&dialog.pages()).is_some());
        dialog.range = Range::Current;
        assert!(dialog.pages().is_err());
    }

    fn snapshot(dialog: &mut PrintDialog, name: &str) {
        use crate::theme;
        theme::set_current(crate::models::ThemeName::Mdex);
        let ctx = egui::Context::default();
        theme::configure_icons(&ctx);
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        theme::configure_style(&ctx);
        let size = egui::vec2(760.0, 560.0);
        let mut canvas = crate::ui_snapshot::Canvas::default();
        let mut frame = || {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ui| {
                    let _ = dialog.ui(ui.ctx());
                },
            )
        };
        for _ in 0..5 {
            canvas.absorb(&mut frame().textures_delta);
        }
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tmp")
            .join(format!("print-dialog-{name}.png"));
        canvas.render(&ctx, frame(), size, theme::canvas(), &path);
    }

    #[test]
    #[ignore = "生成打印对话框样张，供目视检查"]
    fn print_dialog_samples() {
        let mut loading = dialog(12);
        snapshot(&mut loading, "loading");

        let mut ready = dialog(12);
        ready.printers_loaded(Ok(vec![
            printer("HP_LaserJet_M403", true),
            printer("Office", false),
        ]));
        ready.printer_state("Office", PrinterState::Ready);
        snapshot(&mut ready, "ready");

        ready.printer_state("Office", PrinterState::Offline);
        ready.copies = 2;
        ready.range = Range::Custom;
        ready.custom = "1-3,5-99".to_string();
        snapshot(&mut ready, "offline-custom");

        ready.custom = "5-3".to_string();
        snapshot(&mut ready, "bad-range");

        let mut empty = dialog(12);
        empty.printers_loaded(Ok(Vec::new()));
        snapshot(&mut empty, "no-printer");
    }
}
