//! 应用内打印对话框。第一期：打印机（默认标记与状态）、份数、逐份打印、页码范围。
//!
//! 对话框只收集选择，不碰任何打印接口：打印机清单与状态由外壳在后台线程查好后
//! 经 [`PrintDialog::printers_loaded`] / [`PrintDialog::printer_state`] 送进来，
//! 点「打印」交回一个 [`PrintJob`]，同样由外壳放后台线程执行。

use super::pages::{self, PageSpan};
use super::{
    ColorMode, Duplex, MAX_COPIES, PdfFacts, PrintJob, PrintRequest, PrinterCaps, PrinterInfo,
    PrinterState, Scaling,
};
use crate::modal::{self, Dismiss};
use crate::theme;
use eframe::egui;
use std::collections::{HashMap, HashSet};
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
    /// 按打印机名缓存的能力。换回来不重查。
    caps: HashMap<String, PrinterCaps>,
    /// 要外壳去查能力的打印机。对话框自己不起线程。
    caps_request: Option<String>,
    /// 已经交给外壳、还没查回来的打印机。免得能力回来之前每帧都起一个线程。
    caps_in_flight: HashSet<String>,
    duplex: Option<Duplex>,
    color: Option<ColorMode>,
    paper: Option<String>,
    tray: Option<String>,
    scaling: Scaling,
    /// 从 PDF 自身测得的事实（整份横排、有红色）。外壳在后台测好送进来。
    facts: Option<PdfFacts>,
    /// 用户动过哪几项。没动过的，在能力 / 文档默认到达时才可以被预填覆盖。
    duplex_touched: bool,
    color_touched: bool,
    paper_touched: bool,
    tray_touched: bool,
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
            caps: HashMap::new(),
            caps_request: None,
            caps_in_flight: HashSet::new(),
            facts: None,
            duplex: None,
            color: None,
            paper: None,
            tray: None,
            scaling: Scaling::Actual,
            duplex_touched: false,
            color_touched: false,
            paper_touched: false,
            tray_touched: false,
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

    /// 某台打印机的能力查回来了。换回这台时不再重查。
    pub(crate) fn caps_loaded(&mut self, name: &str, caps: PrinterCaps) {
        self.caps_in_flight.remove(name);
        self.caps.insert(name.to_string(), caps);
        self.recompute_defaults();
    }

    /// PDF 自身的版式事实测回来了。读不出来（`None`）就当不知道。
    pub(crate) fn facts_loaded(&mut self, facts: Option<PdfFacts>) {
        self.facts = facts;
        self.recompute_defaults();
    }

    /// 文档默认值只填用户没动过的控件；能力与事实各到达一次，各重算一次。
    fn recompute_defaults(&mut self) {
        if self.duplex_touched {
            return;
        }
        if let Some(caps) = self.gated_caps().flatten() {
            self.duplex = super::default_duplex(self.request.hints, self.facts, &caps);
        }
    }

    /// 外壳该去查哪台打印机的能力了。交出去之后在能力回来之前（或换回来之前）
    /// 不会再要，免得每帧起一个线程。
    pub(crate) fn take_caps_request(&mut self) -> Option<String> {
        let name = self.caps_request.take()?;
        self.caps_in_flight.insert(name.clone());
        Some(name)
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
            500.0,
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
                self.capability_rows(ui);
            });
        ui.add_space(8.0);
        self.notes(ui, &pages);
        ui.add_space(12.0);
        let outcome = self.buttons(ui, &pages);
        self.request_caps();
        outcome
    }

    /// 选中的打印机没查过能力就记下来，外壳每帧取走一次、在后台查。
    /// 对话框自己不起线程，也不等结果。
    fn request_caps(&mut self) {
        if let Some(name) = &self.selected
            && !self.caps.contains_key(name)
            && !self.caps_in_flight.contains(name)
            && self.caps_request.as_deref() != Some(name)
        {
            self.caps_request = Some(name.clone());
        }
    }

    fn printer_row(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
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
                let before = self.selected.clone();
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
                changed = self.selected != before;
            }
        });
        if changed {
            self.printer_changed();
        }
    }

    /// 换了打印机：纸张与纸盒的编号是每台打印机自己的，不能沿用。
    /// 双面、颜色是通用取值，由 [`Self::job`] 按新打印机的能力过滤。
    fn printer_changed(&mut self) {
        self.paper = None;
        self.tray = None;
        self.paper_touched = false;
        self.tray_touched = false;
        // 新打印机的能力可能已经在缓存里（换回来过），默认值当场重算；没缓存就等
        // `caps_loaded`。
        self.recompute_defaults();
    }

    /// 在「页码」之后画双面、颜色、纸张、纸盒、缩放。能力查回来之前只画一个
    /// 提示，不画五个空控件；没有选中打印机（清单列不出来）时整栏不画。
    fn capability_rows(&mut self, ui: &mut egui::Ui) {
        let caps = self.gated_caps();
        match caps {
            None => {}
            Some(None) => {
                ui.label("选项");
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(egui::RichText::new("正在读取打印机能力…").color(theme::text_muted()));
                });
                ui.end_row();
            }
            Some(Some(caps)) => {
                ui.label("双面");
                self.duplex_row(ui, &caps);
                ui.end_row();
                ui.label("颜色");
                self.color_row(ui, &caps);
                ui.end_row();
                ui.label("纸张");
                self.paper_row(ui, &caps);
                ui.end_row();
                ui.label("纸盒");
                self.tray_row(ui, &caps);
                ui.end_row();
                ui.label("缩放");
                self.scaling_row(ui);
                ui.end_row();
            }
        }
    }

    /// 选中打印机的能力缓存。`None` 是没选中打印机；内层 `None` 是还没查回来。
    fn gated_caps(&self) -> Option<Option<PrinterCaps>> {
        let name = self.selected.as_deref()?;
        Some(self.caps.get(name).cloned())
    }

    fn duplex_row(&mut self, ui: &mut egui::Ui, caps: &PrinterCaps) {
        let text = duplex_label(self.duplex);
        let row = ui.horizontal(|ui| {
            ui.add_enabled_ui(caps.duplex, |ui| {
                egui::ComboBox::from_id_salt("print_dialog_duplex")
                    .width(180.0)
                    .selected_text(text)
                    .show_ui(ui, |ui| {
                        let mut chosen = self.duplex;
                        ui.selectable_value(&mut chosen, None, "打印机默认");
                        ui.selectable_value(&mut chosen, Some(Duplex::Simplex), "单面");
                        ui.selectable_value(&mut chosen, Some(Duplex::LongEdge), "长边翻转");
                        ui.selectable_value(&mut chosen, Some(Duplex::ShortEdge), "短边翻转");
                        if chosen != self.duplex {
                            self.duplex = chosen;
                            self.duplex_touched = true;
                        }
                    });
            });
        });
        if !caps.duplex {
            row.response.on_hover_text("这台打印机没有双面单元");
        }
    }

    fn color_row(&mut self, ui: &mut egui::Ui, caps: &PrinterCaps) {
        // 单色机固定显示「黑白」：这一下拉只是告知，选了也没用。
        let text = if caps.color {
            color_label(self.color)
        } else {
            "黑白".to_string()
        };
        ui.horizontal(|ui| {
            ui.add_enabled_ui(caps.color, |ui| {
                egui::ComboBox::from_id_salt("print_dialog_color")
                    .width(180.0)
                    .selected_text(text)
                    .show_ui(ui, |ui| {
                        let mut chosen = self.color;
                        ui.selectable_value(&mut chosen, None, "打印机默认");
                        ui.selectable_value(&mut chosen, Some(ColorMode::Color), "彩色");
                        ui.selectable_value(&mut chosen, Some(ColorMode::Mono), "黑白");
                        if chosen != self.color {
                            self.color = chosen;
                            self.color_touched = true;
                        }
                    });
            });
        });
    }

    fn paper_row(&mut self, ui: &mut egui::Ui, caps: &PrinterCaps) {
        let text = paper_label(self.paper.as_deref(), caps);
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("print_dialog_paper")
                .width(180.0)
                .selected_text(text)
                .show_ui(ui, |ui| {
                    let mut chosen = self.paper.clone();
                    ui.selectable_value(&mut chosen, None, default_paper_text(caps));
                    for paper in &caps.papers {
                        ui.selectable_value(&mut chosen, Some(paper.id.clone()), &paper.label);
                    }
                    if chosen != self.paper {
                        self.paper = chosen;
                        self.paper_touched = true;
                    }
                });
        });
    }

    fn tray_row(&mut self, ui: &mut egui::Ui, caps: &PrinterCaps) {
        let text = tray_label(self.tray.as_deref(), caps);
        let enabled = !caps.trays.is_empty();
        let row = ui.horizontal(|ui| {
            ui.add_enabled_ui(enabled, |ui| {
                egui::ComboBox::from_id_salt("print_dialog_tray")
                    .width(180.0)
                    .selected_text(text)
                    .show_ui(ui, |ui| {
                        let mut chosen = self.tray.clone();
                        ui.selectable_value(&mut chosen, None, "自动选择");
                        for tray in &caps.trays {
                            ui.selectable_value(&mut chosen, Some(tray.id.clone()), &tray.label);
                        }
                        if chosen != self.tray {
                            self.tray = chosen;
                            self.tray_touched = true;
                        }
                    });
            });
        });
        // 「自动选择」对应 PrintJob.tray = None；默认纸盒只在悬停里说明。
        if let Some(default) = caps
            .default_tray
            .as_deref()
            .and_then(|id| caps.trays.iter().find(|tray| tray.id == id))
        {
            row.response
                .on_hover_text(format!("打印机默认纸盒：{}", default.label));
        }
    }

    fn scaling_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.scaling, Scaling::Actual, "实际大小（公文推荐）")
                .on_hover_text("按纸张实际大小出图；页面放不下时自动缩小");
            ui.radio_value(&mut self.scaling, Scaling::Fit, "适合纸张")
                .on_hover_text("按可打印区等比缩放，适合纸型与版面不一致的 PDF");
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
            let double_sided = matches!(
                self.effective_duplex(),
                Some(Duplex::LongEdge | Duplex::ShortEdge)
            );
            let summary = match selection.page_total {
                Some(total) => {
                    let per_copy = if double_sided {
                        total.div_ceil(2)
                    } else {
                        total
                    };
                    let sheets = per_copy * u64::from(self.copies);
                    let mode = if double_sided { "双面" } else { "单面" };
                    if self.copies > 1 {
                        format!("共 {total} 页 × {} 份，{mode}约 {sheets} 张纸", self.copies)
                    } else {
                        format!("共 {total} 页，{mode}约 {sheets} 张纸")
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
        // 公文场景的提醒：只提醒，不拦打印。
        if let Some(caps) = self.gated_caps().flatten()
            && let Some(job) = self.job(pages)
        {
            for warning in super::print_warnings(self.request.hints, self.facts, &caps, &job) {
                ui.colored_label(theme::warn(), warning);
            }
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
            duplex: self.effective_duplex(),
            color: self.effective_color(),
            paper: self.paper.clone(),
            tray: self.tray.clone(),
            scaling: self.scaling,
        })
    }

    /// 这台打印机不支持双面时，即使状态里留着长边也不提交：新选的打印机可能
    /// 没有双面单元（换机前留下的选择、或从偏好里预填的）。
    fn effective_duplex(&self) -> Option<Duplex> {
        let supported = self
            .selected
            .as_deref()
            .and_then(|name| self.caps.get(name))
            .is_none_or(|caps| caps.duplex);
        self.duplex.filter(|_| supported)
    }

    /// 同上，单色机不提交彩色或黑白选择，交给打印机默认。
    fn effective_color(&self) -> Option<ColorMode> {
        let supported = self
            .selected
            .as_deref()
            .and_then(|name| self.caps.get(name))
            .is_none_or(|caps| caps.color);
        self.color.filter(|_| supported)
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

fn duplex_label(duplex: Option<Duplex>) -> String {
    match duplex {
        None => "打印机默认".to_string(),
        Some(Duplex::Simplex) => "单面".to_string(),
        Some(Duplex::LongEdge) => "长边翻转".to_string(),
        Some(Duplex::ShortEdge) => "短边翻转".to_string(),
    }
}

fn color_label(color: Option<ColorMode>) -> String {
    match color {
        None => "打印机默认".to_string(),
        Some(ColorMode::Color) => "彩色".to_string(),
        Some(ColorMode::Mono) => "黑白".to_string(),
    }
}

/// 纸张下拉第一项的文字：「打印机默认（A4）」，没有默认纸张就不写括号。
fn default_paper_text(caps: &PrinterCaps) -> String {
    caps.default_paper
        .as_deref()
        .and_then(|id| caps.papers.iter().find(|paper| paper.id == id))
        .map(|paper| format!("打印机默认（{}）", paper.label))
        .unwrap_or_else(|| "打印机默认".to_string())
}

fn paper_label(paper: Option<&str>, caps: &PrinterCaps) -> String {
    match paper {
        Some(id) => caps
            .papers
            .iter()
            .find(|paper| paper.id == id)
            .map(|paper| paper.label.clone())
            .unwrap_or_else(|| id.to_string()),
        None => default_paper_text(caps),
    }
}

fn tray_label(tray: Option<&str>, caps: &PrinterCaps) -> String {
    match tray {
        Some(id) => caps
            .trays
            .iter()
            .find(|tray| tray.id == id)
            .map(|tray| tray.label.clone())
            .unwrap_or_else(|| id.to_string()),
        None => "自动选择".to_string(),
    }
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
                hints: super::super::PrintHints::default(),
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

    fn caps(duplex: bool, color: bool) -> PrinterCaps {
        use super::super::MediaOption;
        PrinterCaps {
            duplex,
            color,
            papers: vec![
                MediaOption {
                    id: "9".to_string(),
                    label: "A4".to_string(),
                },
                MediaOption {
                    id: "8".to_string(),
                    label: "A3".to_string(),
                },
            ],
            default_paper: Some("9".to_string()),
            trays: vec![MediaOption {
                id: "7".to_string(),
                label: "纸盒 1".to_string(),
            }],
            default_tray: Some("7".to_string()),
        }
    }

    fn ready_dialog() -> PrintDialog {
        let mut dialog = dialog(12);
        dialog.printers_loaded(Ok(vec![printer("Office", true)]));
        dialog.caps_loaded("Office", caps(true, true));
        dialog
    }

    #[test]
    fn capability_choices_become_job_fields() {
        let mut dialog = ready_dialog();
        dialog.duplex = Some(Duplex::ShortEdge);
        dialog.color = Some(ColorMode::Mono);
        dialog.paper = Some("8".to_string());
        dialog.tray = Some("7".to_string());
        dialog.scaling = Scaling::Fit;
        let job = dialog.job(&dialog.pages()).expect("全部");
        assert_eq!(job.duplex, Some(Duplex::ShortEdge));
        assert_eq!(job.color, Some(ColorMode::Mono));
        assert_eq!(job.paper.as_deref(), Some("8"));
        assert_eq!(job.tray.as_deref(), Some("7"));
        assert_eq!(job.scaling, Scaling::Fit);
    }

    #[test]
    fn untouched_options_are_left_to_the_printer() {
        let dialog = ready_dialog();
        let job = dialog.job(&dialog.pages()).expect("全部");
        assert_eq!(job.duplex, None);
        assert_eq!(job.color, None);
        assert_eq!(job.paper, None);
        assert_eq!(job.tray, None);
        assert_eq!(job.scaling, Scaling::Actual);
    }

    #[test]
    fn a_printer_without_duplex_never_gets_a_duplex_choice() {
        let mut dialog = dialog(12);
        dialog.printers_loaded(Ok(vec![printer("Office", true)]));
        dialog.caps_loaded("Office", caps(false, false));
        // 换打印机前留下的长边选择、以及单色机上的彩色选择，都不提交。
        dialog.duplex = Some(Duplex::LongEdge);
        dialog.color = Some(ColorMode::Color);
        let job = dialog.job(&dialog.pages()).expect("全部");
        assert_eq!(job.duplex, None);
        assert_eq!(job.color, None);
    }

    #[test]
    fn document_hints_fill_in_the_duplex_default_until_the_user_touches_it() {
        let mut dialog = dialog(12);
        dialog.request.hints = super::super::PrintHints {
            duplex_typeset: true,
            send_package: false,
        };
        dialog.printers_loaded(Ok(vec![printer("Office", true)]));
        // 能力回来之前不瞎猜。
        assert_eq!(dialog.duplex, None);
        dialog.caps_loaded("Office", caps(true, true));
        assert_eq!(dialog.duplex, Some(Duplex::LongEdge));
        // 事实回来（整份横排）后改成短边。
        dialog.facts_loaded(Some(PdfFacts {
            all_rotated: true,
            has_red: false,
        }));
        assert_eq!(dialog.duplex, Some(Duplex::ShortEdge));
        assert_eq!(
            dialog.job(&dialog.pages()).expect("全部").duplex,
            Some(Duplex::ShortEdge)
        );
    }

    #[test]
    fn a_duplex_choice_the_user_made_wins_over_the_document_default() {
        let mut dialog = dialog(12);
        dialog.request.hints = super::super::PrintHints {
            duplex_typeset: true,
            send_package: false,
        };
        dialog.printers_loaded(Ok(vec![printer("Office", true)]));
        dialog.duplex = Some(Duplex::Simplex);
        dialog.duplex_touched = true;
        dialog.caps_loaded("Office", caps(true, true));
        assert_eq!(dialog.duplex, Some(Duplex::Simplex));
    }

    #[test]
    fn no_duplex_default_on_a_printer_without_the_unit() {
        let mut dialog = dialog(12);
        dialog.request.hints = super::super::PrintHints {
            duplex_typeset: false,
            send_package: true,
        };
        dialog.printers_loaded(Ok(vec![printer("Office", true)]));
        dialog.caps_loaded("Office", caps(false, true));
        assert_eq!(dialog.duplex, None);
    }

    #[test]
    fn caps_are_requested_once_per_printer() {
        let mut dialog = dialog(12);
        assert_eq!(dialog.take_caps_request(), None, "还没选中打印机");
        dialog.printers_loaded(Ok(vec![printer("HP", true), printer("Office", false)]));
        // 记住的是 Office（构造对话框时传入的「上次用的」）。
        dialog.request_caps();
        assert_eq!(dialog.take_caps_request().as_deref(), Some("Office"));
        // 交出去以后、能力回来之前不再要，否则每帧都会起一个线程。
        dialog.request_caps();
        assert_eq!(dialog.take_caps_request(), None);
        dialog.caps_loaded("Office", caps(false, true));
        dialog.request_caps();
        assert_eq!(dialog.take_caps_request(), None, "有了缓存就不再查");
    }

    #[test]
    fn switching_printers_drops_printer_specific_choices_and_queries_the_new_one() {
        let mut dialog = ready_dialog();
        dialog.paper = Some("8".to_string());
        dialog.tray = Some("7".to_string());
        dialog.selected = Some("HP".to_string());
        dialog.printer_changed();
        assert_eq!(dialog.paper, None);
        assert_eq!(dialog.tray, None);
        dialog.request_caps();
        assert_eq!(dialog.take_caps_request().as_deref(), Some("HP"));
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
        // 对话框 500 宽，加上边距与五行选项后高度也要留够。
        let size = egui::vec2(820.0, 780.0);
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
        ready.caps_loaded("Office", caps(true, true));
        snapshot(&mut ready, "ready");

        // 整份横排 + 选了长边 + 黑白 + 有红：三条公文提醒同时出现。
        let mut warned = dialog(12);
        warned.request.hints = super::super::PrintHints {
            duplex_typeset: true,
            send_package: false,
        };
        warned.printers_loaded(Ok(vec![printer("Office", true)]));
        warned.caps_loaded("Office", caps(true, true));
        warned.facts_loaded(Some(PdfFacts {
            all_rotated: true,
            has_red: true,
        }));
        warned.duplex = Some(Duplex::LongEdge);
        warned.color = Some(ColorMode::Mono);
        snapshot(&mut warned, "warnings-rotated-red");

        // 能力还没回来：five 行只剩一个 spinner。
        let mut loading_caps = dialog(12);
        loading_caps.printers_loaded(Ok(vec![printer("Office", true)]));
        snapshot(&mut loading_caps, "caps-loading");

        // 没有双面单元的打印机：双面置灰，纸盒列表为空也置灰。
        let mut single = dialog(12);
        single.printers_loaded(Ok(vec![printer("HP_DeskJet_4900", true)]));
        single.caps_loaded(
            "HP_DeskJet_4900",
            PrinterCaps {
                color: true,
                papers: vec![super::super::MediaOption {
                    id: "9".to_string(),
                    label: "A4".to_string(),
                }],
                default_paper: Some("9".to_string()),
                ..Default::default()
            },
        );
        snapshot(&mut single, "caps-single-sided");

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
