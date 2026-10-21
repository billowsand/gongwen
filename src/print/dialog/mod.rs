//! 应用内打印对话框。第一期：打印机（默认标记与状态）、份数、逐份打印、页码范围。
//!
//! 对话框只收集选择，不碰任何打印接口：打印机清单与状态由外壳在后台线程查好后
//! 经 [`PrintDialog::printers_loaded`] / [`PrintDialog::printer_state`] 送进来，
//! 点「打印」交回一个 [`PrintJob`]，同样由外壳放后台线程执行。

use super::pages::{self, PageSpan};
use super::{
    ColorMode, Duplex, HAS_DRIVER_PREFERENCES, MAX_COPIES, PdfFacts, PrintJob, PrintOutcome,
    PrintRequest, PrinterCaps, PrinterInfo, PrinterState, Scaling,
};
use crate::modal::{self, Dismiss};
use crate::models::PrinterPrefs;
use crate::theme;
use eframe::egui;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// 预览栏宽度。
const PREVIEW_WIDTH: f32 = 180.0;
/// 预览纸张的最大高度（A4 在 180 宽时约 255）。
const PREVIEW_HEIGHT: f32 = 260.0;

/// 对话框的结果。
pub(crate) enum DialogOutcome {
    /// 开始打印；对话框留在屏幕上显示进度，打完（或失败）由外壳转回结果。
    Print(PrintJob),
    /// 改用系统自带的打印对话框（只有 Windows 有这个按钮）。
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

/// 手动双面进行到哪一轮。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManualPass {
    /// 正面：要打的页里第 1、3、5… 张。
    First,
    /// 背面：翻面放回后的第 2、4、6… 张。
    Second,
}

/// 对话框进行到哪一步。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// 可编辑。普通打印从打开到提交都在这一步。
    Editing,
    /// 正在送纸；`manual` 是手动双面的哪一轮，普通打印是 `None`。
    Printing { manual: Option<ManualPass> },
    /// 手动双面第一轮打完，等用户把整叠纸翻面放回纸盒。
    WaitingFlip,
}

/// 手动双面第一轮打完以后要用的东西：背面轮打哪些页，以及页数是不是奇数。
struct ManualPasses {
    back: Vec<PageSpan>,
    odd: bool,
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
    /// 按打印机名记住的偏好，用来预填控件（不算「用户动过」）。
    prefs: BTreeMap<String, PrinterPrefs>,
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
    /// Windows「打印机首选项」改过的整块 DEVMODE。换打印机时清掉。
    /// 非 Windows 上没人会设它（没有首选项窗口），但字段与逻辑照常编译、可测。
    devmode: Option<Vec<u8>>,
    /// 要外壳去弹首选项窗口的打印机。
    preferences_request: Option<String>,
    /// 已经交给外壳、还在等用户操作，别重复弹窗口。
    preferences_pending: bool,
    preferences_error: Option<String>,
    /// 用户动过哪几项。没动过的，在能力 / 文档默认到达时才可以被预填覆盖。
    duplex_touched: bool,
    color_touched: bool,
    paper_touched: bool,
    tray_touched: bool,
    /// 缩放没有文档默认值，同样记「用户动过」才好用记住的偏好预填。
    scaling_touched: bool,
    /// 手动双面第二轮是否倒序出纸（逐台记住）。
    manual_reverse: bool,
    /// 手动双面进行到哪一步；普通打印一直在 [`Stage::Editing`]。
    stage: Stage,
    manual_passes: Option<ManualPasses>,
    /// 手动双面某一轮失败的原因，显示在对话框里。
    manual_error: Option<String>,
    /// 「反面倒序」刚改过，要马上写进这台打印机的偏好。
    manual_reverse_dirty: bool,
    /// 这一轮打印的取消标志，外壳把同一个 `Arc` 交给后台线程。
    cancel: Arc<AtomicBool>,
    /// 进度：已送页数 / 总页数。CUPS 不报，一直是 `None`。
    progress: Option<(u32, u32)>,
    /// 预览翻到「要打的页」里的第几张（从 0 起）。
    preview_index: usize,
}

impl PrintDialog {
    pub(crate) fn new(
        id: u64,
        request: PrintRequest,
        remembered: &str,
        prefs: BTreeMap<String, PrinterPrefs>,
    ) -> Self {
        let preview_index = request.current_page;
        Self {
            id,
            request,
            printers: Printers::Loading,
            selected: None,
            remembered: remembered.to_string(),
            prefs,
            copies: 1,
            collate: true,
            range: Range::All,
            custom: String::new(),
            caps: HashMap::new(),
            caps_request: None,
            caps_in_flight: HashSet::new(),
            facts: None,
            devmode: None,
            preferences_request: None,
            preferences_pending: false,
            preferences_error: None,
            duplex: None,
            color: None,
            paper: None,
            tray: None,
            scaling: Scaling::Actual,
            duplex_touched: false,
            color_touched: false,
            paper_touched: false,
            tray_touched: false,
            scaling_touched: false,
            manual_reverse: false,
            stage: Stage::Editing,
            manual_passes: None,
            manual_error: None,
            manual_reverse_dirty: false,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: None,
            preview_index,
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
        self.apply_defaults();
    }

    /// 某台打印机的能力查回来了。换回这台时不再重查。
    pub(crate) fn caps_loaded(&mut self, name: &str, caps: PrinterCaps) {
        self.caps_in_flight.remove(name);
        self.caps.insert(name.to_string(), caps);
        self.apply_defaults();
    }

    /// 外壳该去弹哪台打印机的首选项窗口了；带上上次改过的 DEVMODE 当输入。
    /// 非 Windows 上不会有请求（界面不画那个按钮），外壳收到时复位挂起标志。
    pub(crate) fn take_preferences_request(&mut self) -> Option<(String, Option<Vec<u8>>)> {
        if self.preferences_pending {
            return None;
        }
        let name = self.preferences_request.take()?;
        self.preferences_pending = true;
        Some((name, self.devmode.clone()))
    }

    /// 首选项窗口的结果。用户取消是 `Ok(None)`；换了打印机的话旧结果丢掉。
    pub(crate) fn preferences_edited(
        &mut self,
        printer: &str,
        result: Result<Option<Vec<u8>>, String>,
    ) {
        self.preferences_pending = false;
        if self.selected.as_deref() != Some(printer) {
            return;
        }
        match result {
            Ok(Some(devmode)) => {
                self.devmode = Some(devmode);
                self.preferences_error = None;
            }
            Ok(None) => {}
            Err(error) => self.preferences_error = Some(error),
        }
    }

    /// 丢掉驱动首选项，回到按对话框选项出纸（「恢复」按钮）。
    fn reset_preferences(&mut self) {
        self.devmode = None;
        self.preferences_error = None;
    }

    /// PDF 自身的版式事实测回来了。读不出来（`None`）就当不知道。
    pub(crate) fn facts_loaded(&mut self, facts: Option<PdfFacts>) {
        self.facts = facts;
        self.apply_defaults();
    }

    /// 重新算一遍预填值。优先级：用户动过 > 文档默认值 > 这台打印机记住的偏好 >
    /// 打印机默认。能力、事实、选中打印机每到达一次就重算一次。
    fn apply_defaults(&mut self) {
        let caps = self.gated_caps().flatten();
        let prefs = self
            .selected
            .as_deref()
            .and_then(|name| self.prefs.get(name))
            .cloned();
        if !self.duplex_touched {
            let from_document = caps
                .as_ref()
                .and_then(|caps| super::default_duplex(self.request.hints, self.facts, caps));
            self.duplex = from_document.or_else(|| {
                let remembered = prefs.as_ref().and_then(|prefs| prefs.duplex);
                // 新选的打印机可能没有双面单元：记着的双面方式要按能力过滤。
                // 手动双面也不从偏好恢复：第一次用哪台机器都要人看着翻面。
                remembered.filter(|duplex| {
                    *duplex != Duplex::Manual && caps.as_ref().is_none_or(|caps| caps.duplex)
                })
            });
        }
        if !self.color_touched {
            self.color = prefs.as_ref().and_then(|prefs| prefs.color);
        }
        if !self.paper_touched {
            self.paper = prefs.as_ref().and_then(|prefs| prefs.paper.clone());
        }
        if !self.tray_touched {
            self.tray = prefs.as_ref().and_then(|prefs| prefs.tray.clone());
        }
        if !self.scaling_touched {
            self.scaling = prefs
                .as_ref()
                .map_or(Scaling::Actual, |prefs| prefs.scaling);
        }
        self.manual_reverse = prefs.as_ref().is_some_and(|prefs| prefs.manual_reverse);
    }

    /// 这台打印机当前的完整选择，点打印时写回配置（和「上次用的打印机」一起）。
    pub(crate) fn preferences(&self) -> Option<(String, PrinterPrefs)> {
        let name = self.selected.clone()?;
        Some((
            name,
            PrinterPrefs {
                duplex: self.duplex,
                color: self.color,
                paper: self.paper.clone(),
                tray: self.tray.clone(),
                scaling: self.scaling,
                manual_reverse: self.manual_reverse,
            },
        ))
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

    /// 画对话框。`textures` 按页号（从 0 起）取 PDF 标签里已经渲染好的纹理，
    /// 没有就返回 `None`；预览不为这一页另起渲染线程。
    pub(crate) fn ui(
        &mut self,
        ctx: &egui::Context,
        textures: &dyn Fn(usize) -> Option<(egui::TextureId, egui::Vec2)>,
    ) -> Option<DialogOutcome> {
        // 打印进行中不认 Esc 和遮罩，免得一不小心把正在送纸的对话框关掉。
        let dismiss = match self.stage {
            Stage::Printing { .. } => Dismiss::ButtonsOnly,
            _ => Dismiss::EscOnly,
        };
        let dialog = modal::dialog(
            ctx,
            egui::Id::new("print_dialog"),
            "打印",
            680.0,
            dismiss,
            |ui| self.contents(ui, textures),
        );
        if dialog.inner.is_some() {
            return dialog.inner;
        }
        dialog.dismissed.then_some(DialogOutcome::Cancel)
    }

    fn contents(
        &mut self,
        ui: &mut egui::Ui,
        textures: &dyn Fn(usize) -> Option<(egui::TextureId, egui::Vec2)>,
    ) -> Option<DialogOutcome> {
        ui.add(
            egui::Label::new(egui::RichText::new(&self.request.title).color(theme::text_soft()))
                .truncate(),
        );
        ui.add_space(10.0);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(PREVIEW_WIDTH, 0.0),
                egui::Layout::top_down(egui::Align::Center),
                |ui| self.preview_column(ui, textures),
            );
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(10.0);
            ui.vertical(|ui| match self.stage {
                Stage::Editing => self.editing_contents(ui),
                Stage::Printing { manual } => self.printing_contents(ui, manual),
                Stage::WaitingFlip => self.waiting_flip_contents(ui),
            })
            .inner
        })
        .inner
    }

    fn editing_contents(&mut self, ui: &mut egui::Ui) -> Option<DialogOutcome> {
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

    /// 左侧预览：按所选纸张的比例画纸张轮廓，里面摆当前预览页。
    fn preview_column(
        &mut self,
        ui: &mut egui::Ui,
        textures: &dyn Fn(usize) -> Option<(egui::TextureId, egui::Vec2)>,
    ) {
        let numbers = print_page_numbers(&self.pages(), self.request.page_count);
        let total = numbers.len();
        // 换了页码范围后原来的位置可能越界，夹到最后一页。
        self.preview_index = if total == 0 {
            0
        } else {
            self.preview_index.min(total - 1)
        };
        let (paper_w, paper_h) = self.preview_paper_size_mm();
        let aspect = paper_h / paper_w.max(1.0);
        let width = PREVIEW_WIDTH.min(ui.available_width().max(60.0));
        let height = (width * aspect).min(PREVIEW_HEIGHT);
        let width = (height / aspect).min(width);
        let (outer, _) = ui.allocate_exact_size(
            egui::vec2(PREVIEW_WIDTH, PREVIEW_HEIGHT),
            egui::Sense::hover(),
        );
        let paper = egui::Rect::from_center_size(
            egui::pos2(outer.center().x, outer.top() + height / 2.0),
            egui::vec2(width, height),
        );
        let painter = ui.painter().clone();
        // 纸张轮廓：白纸 + 一点投影 + 细边，和编辑区里的纸一样。
        painter.rect_filled(
            paper.translate(egui::vec2(0.0, 2.0)),
            2.0,
            egui::Color32::from_black_alpha(22),
        );
        painter.rect_filled(paper, 2.0, egui::Color32::WHITE);
        painter.rect_stroke(
            paper,
            2.0,
            egui::Stroke::new(1.0, theme::border_strong()),
            egui::StrokeKind::Inside,
        );
        // 「适合纸张」按可打印区缩一点，「实际大小」填满纸面。
        let content = match self.scaling {
            Scaling::Actual => paper,
            Scaling::Fit => paper.shrink(paper.width() * 0.05),
        };
        if let Some(page) = numbers.get(self.preview_index) {
            // 页号从 1 起，纹理槽从 0 起。
            match textures(page.saturating_sub(1) as usize) {
                Some((texture, size)) => {
                    let image = fit_inside(content.size(), size.y / size.x.max(1.0));
                    let rect = egui::Rect::from_center_size(content.center(), image);
                    painter.image(
                        texture,
                        rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
                None => {
                    // 占位底色取纸张与文字之间的灰：浅色主题浅灰、深色主题深灰。
                    let placeholder = if theme::current().dark {
                        egui::Color32::from_gray(58)
                    } else {
                        egui::Color32::from_gray(236)
                    };
                    painter.rect_filled(content, 1.0, placeholder);
                    // 用不占位的子 Ui 画在纸面上：`scope_builder` 会把父 Ui 的光标推到
                    // 纸面中间，下面的翻页按钮就叠到纸上去了。
                    let mut text_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(content.shrink(8.0))
                            .layout(egui::Layout::top_down(egui::Align::Center)),
                    );
                    text_ui.add_space(content.height() * 0.28);
                    text_ui.add(
                        egui::Label::new(
                            egui::RichText::new("这一页还没渲染，滚动到该页后可预览")
                                .small()
                                .color(theme::text_muted()),
                        )
                        .wrap(),
                    );
                }
            }
            // 双面时在纸角标出这一页是正面还是反面。
            if self.double_sided() {
                let label = if self.preview_index.is_multiple_of(2) {
                    "正面"
                } else {
                    "反面"
                };
                painter.text(
                    paper.left_top() + egui::vec2(4.0, 4.0),
                    egui::Align2::LEFT_TOP,
                    label,
                    egui::FontId::proportional(12.0),
                    theme::text_soft(),
                );
            }
        }
        ui.add_space(6.0);
        if total > 0 {
            // 在「要打的页」里翻，不是按 PDF 的页码翻。
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(self.preview_index > 0, egui::Button::new("◀"))
                    .clicked()
                {
                    self.preview_index -= 1;
                }
                ui.label(
                    egui::RichText::new(format!("{} / {total}", self.preview_index + 1))
                        .color(theme::text_soft()),
                );
                if ui
                    .add_enabled(self.preview_index + 1 < total, egui::Button::new("▶"))
                    .clicked()
                {
                    self.preview_index += 1;
                }
            });
        } else {
            // 页码框里输错了，或 PDF 还没打开完：没有「要打的页」可翻。
            let message = if self.pages().is_err() {
                "页码无效"
            } else {
                "页数未知"
            };
            ui.label(
                egui::RichText::new(message)
                    .small()
                    .color(theme::text_muted()),
            );
        }
    }

    fn double_sided(&self) -> bool {
        matches!(
            self.effective_duplex(),
            Some(Duplex::LongEdge | Duplex::ShortEdge | Duplex::Manual)
        )
    }

    /// 所选纸张的尺寸（毫米，宽 × 高）。拿不到就用 A4。
    fn preview_paper_size_mm(&self) -> (f32, f32) {
        let caps = self.gated_caps().flatten();
        let (id, label) = match (&self.paper, caps.as_ref()) {
            (Some(id), Some(caps)) => caps
                .papers
                .iter()
                .find(|paper| &paper.id == id)
                .map(|paper| (paper.id.as_str(), paper.label.as_str()))
                .unwrap_or((id.as_str(), id.as_str())),
            (Some(id), None) => (id.as_str(), id.as_str()),
            (None, Some(caps)) => match caps.default_paper.as_deref() {
                Some(default) => caps
                    .papers
                    .iter()
                    .find(|paper| paper.id == default)
                    .map(|paper| (paper.id.as_str(), paper.label.as_str()))
                    .unwrap_or((default, default)),
                None => ("A4", "A4"),
            },
            (None, None) => ("A4", "A4"),
        };
        paper_size_mm(id, label).unwrap_or((210.0, 297.0))
    }

    /// 打印对话框对应哪个 PDF 标签（页面预览从那里取纹理）。
    pub(crate) fn pdf_key(&self) -> Option<crate::pdf_viewer::PdfKey> {
        self.request.pdf_key
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
        // 预览栏占了左边以后右栏变窄；Windows 上还有「打印机首选项…」按钮，
        // 放不下就换行，不挤进预览栏。
        ui.horizontal_wrapped(|ui| match &self.printers {
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
                    .width(200.0)
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
                // 驱动独有的选项（装订、打孔、水印）只能从驱动自己的窗口设。
                if HAS_DRIVER_PREFERENCES {
                    let can_open = self.selected.is_some() && !self.preferences_pending;
                    if ui
                        .add_enabled(can_open, egui::Button::new("打印机首选项…"))
                        .on_hover_text("打开驱动自己的设置窗口，可设装订、打孔等驱动独有选项")
                        .clicked()
                    {
                        self.preferences_request = self.selected.clone();
                    }
                }
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
        // 首选项是每台打印机、每个驱动自己的，换机器就没了。
        self.devmode = None;
        self.preferences_error = None;
        // 新打印机的能力可能已经在缓存里（换回来过），默认值当场重算；没缓存就等
        // `caps_loaded`。
        self.apply_defaults();
    }

    /// 在「页码」之后画双面、颜色、纸张、纸盒、缩放。能力查回来之前只画一个
    /// 提示，不画五个空控件；没有选中打印机（清单列不出来）时整栏不画。
    fn capability_rows(&mut self, ui: &mut egui::Ui) {
        let locked = self.preferences_locked();
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
                self.duplex_row(ui, &caps, locked);
                ui.end_row();
                ui.label("颜色");
                self.color_row(ui, &caps, locked);
                ui.end_row();
                ui.label("纸张");
                self.paper_row(ui, &caps, locked);
                ui.end_row();
                ui.label("纸盒");
                self.tray_row(ui, &caps, locked);
                ui.end_row();
                if locked {
                    ui.label("");
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("双面、颜色、纸张与纸盒已按打印机首选项设置。")
                                .color(theme::text_soft()),
                        );
                        // 首选项窗口只有 Windows 有，非 Windows 上这个状态走不到。
                        if HAS_DRIVER_PREFERENCES && ui.small_button("恢复").clicked() {
                            self.reset_preferences();
                        }
                    });
                    ui.end_row();
                }
                ui.label("缩放");
                self.scaling_row(ui);
                ui.end_row();
            }
        }
    }

    /// 首选项改过时，双面、颜色、纸张、纸盒四行置灰：以驱动的设置为准。
    fn preferences_locked(&self) -> bool {
        self.devmode.is_some()
    }

    /// 选中打印机的能力缓存。`None` 是没选中打印机；内层 `None` 是还没查回来。
    fn gated_caps(&self) -> Option<Option<PrinterCaps>> {
        let name = self.selected.as_deref()?;
        Some(self.caps.get(name).cloned())
    }

    fn duplex_row(&mut self, ui: &mut egui::Ui, caps: &PrinterCaps, locked: bool) {
        let text = duplex_label(self.duplex);
        let row = ui.horizontal(|ui| {
            ui.add_enabled_ui(!locked, |ui| {
                egui::ComboBox::from_id_salt("print_dialog_duplex")
                    .width(180.0)
                    .selected_text(text)
                    .show_ui(ui, |ui| {
                        let mut chosen = self.duplex;
                        ui.selectable_value(&mut chosen, None, "打印机默认");
                        ui.selectable_value(&mut chosen, Some(Duplex::Simplex), "单面");
                        if caps.duplex {
                            ui.selectable_value(&mut chosen, Some(Duplex::LongEdge), "长边翻转");
                            ui.selectable_value(&mut chosen, Some(Duplex::ShortEdge), "短边翻转");
                        }
                        // 手动双面不需要双面单元，什么打印机都能选。
                        ui.selectable_value(
                            &mut chosen,
                            Some(Duplex::Manual),
                            "手动双面（翻面再打）",
                        );
                        if chosen != self.duplex {
                            self.duplex = chosen;
                            self.duplex_touched = true;
                        }
                    });
            });
        });
        if !caps.duplex {
            row.response
                .on_hover_text("这台打印机没有双面单元，可用「手动双面」");
        }
    }

    fn color_row(&mut self, ui: &mut egui::Ui, caps: &PrinterCaps, locked: bool) {
        // 单色机固定显示「黑白」：这一下拉只是告知，选了也没用。
        let text = if caps.color {
            color_label(self.color)
        } else {
            "黑白".to_string()
        };
        ui.horizontal(|ui| {
            ui.add_enabled_ui(caps.color && !locked, |ui| {
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

    fn paper_row(&mut self, ui: &mut egui::Ui, caps: &PrinterCaps, locked: bool) {
        let text = paper_label(self.paper.as_deref(), caps);
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!locked, |ui| {
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
        });
    }

    fn tray_row(&mut self, ui: &mut egui::Ui, caps: &PrinterCaps, locked: bool) {
        let text = tray_label(self.tray.as_deref(), caps);
        let enabled = !caps.trays.is_empty() && !locked;
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
        let before = self.scaling;
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.scaling, Scaling::Actual, "实际大小（公文推荐）")
                .on_hover_text("按纸张实际大小出图；页面放不下时自动缩小");
            ui.radio_value(&mut self.scaling, Scaling::Fit, "适合纸张")
                .on_hover_text("按可打印区等比缩放，适合纸型与版面不一致的 PDF");
        });
        if self.scaling != before {
            self.scaling_touched = true;
        }
    }

    fn copies_row(&mut self, ui: &mut egui::Ui) {
        // 手动双面一次只翻一叠纸，多份没法对齐，所以固定一份、份数框置灰。
        let manual = self.effective_duplex() == Some(Duplex::Manual);
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.add_enabled(
                    !manual,
                    egui::DragValue::new(&mut self.copies)
                        .range(1..=MAX_COPIES)
                        .speed(0.1),
                );
                ui.add_space(8.0);
                ui.add_enabled(
                    !manual && self.copies > 1,
                    egui::Checkbox::new(&mut self.collate, "逐份打印"),
                )
                .on_hover_text("一份打完再打下一份；不勾则每页连打几张");
            });
            if manual {
                muted(ui, "手动双面每次只打一份");
            }
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
        // 上一次打印失败或被取消的原因：对话框退回编辑状态，改完可以直接重试。
        if let Some(error) = &self.manual_error {
            ui.colored_label(theme::danger(), error);
        }
        // 手动双面固定一份（份数框置灰时里面的数字不算数）。
        let copies = if self.effective_duplex() == Some(Duplex::Manual) {
            1
        } else {
            self.copies
        };
        if let Ok(selection) = pages {
            let double_sided = self.double_sided();
            let summary = match selection.page_total {
                Some(total) => {
                    let per_copy = if double_sided {
                        total.div_ceil(2)
                    } else {
                        total
                    };
                    let sheets = per_copy * u64::from(copies);
                    let mode = if double_sided { "双面" } else { "单面" };
                    if copies > 1 {
                        format!("共 {total} 页 × {} 份，{mode}约 {sheets} 张纸", copies)
                    } else {
                        format!("共 {total} 页，{mode}约 {sheets} 张纸")
                    }
                }
                None => format!("全部页面 × {} 份", copies),
            };
            ui.label(egui::RichText::new(summary).color(theme::text_soft()));
        }
        if copies > 1 {
            muted(
                ui,
                "打印机份数只是把同一份重复打出；份号不同的多份请在导出时生成。",
            );
        }
        // 首选项窗口打不开或设置失败：不拦打印，说清楚以哪套设置为准。
        if let Some(error) = &self.preferences_error {
            ui.colored_label(theme::warn(), error);
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
            // 系统对话框只有 Windows 有；非 Windows 上不画，但这段代码照常编译
            // （`HAS_DRIVER_PREFERENCES` 是常量，`DialogOutcome::SystemDialog` 因此不算死代码）。
            if HAS_DRIVER_PREFERENCES
                && ui
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
                    outcome = self.print_outcome(pages);
                }
                if ui.button("取消").clicked() {
                    outcome = Some(DialogOutcome::Cancel);
                }
            });
        });
        // 回车即打印：下拉开着时回车归下拉。
        let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
        if outcome.is_none() && can_print && enter && !egui::Popup::is_any_open(ui.ctx()) {
            outcome = self.print_outcome(pages);
        }
        outcome
    }

    /// 正在送纸：进度、取消，以及把窗口让开的出口。
    ///
    /// 对话框是模态的，一直挡着会把整个程序锁住：上百页的光栅打印要几分钟，驱动
    /// 卡住时取消也要等当前页画完才生效。所以普通打印可以「转到后台」，取消之后
    /// 可以直接「关闭窗口」；任务照常在后台结束，结果仍回状态栏。
    fn printing_contents(
        &mut self,
        ui: &mut egui::Ui,
        manual: Option<ManualPass>,
    ) -> Option<DialogOutcome> {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.spinner();
            let prefix = match manual {
                Some(ManualPass::First) => "正面",
                Some(ManualPass::Second) => "反面",
                None => "",
            };
            let text = match self.progress {
                Some((done, total)) => format!("正在送{prefix}第 {done} / {total} 页…"),
                // CUPS 把整份一次交给打印服务，报不出逐页进度。
                None if cfg!(windows) => format!("正在准备{prefix}打印…"),
                None => "正在交给打印服务…".to_string(),
            };
            ui.label(text);
        });
        if let Some(error) = &self.manual_error {
            ui.add_space(6.0);
            ui.colored_label(theme::danger(), error);
        }
        ui.add_space(12.0);
        let mut outcome = None;
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.cancelling() {
                    if ui
                        .button("关闭窗口")
                        .on_hover_text("不再等待；正在画的那一页画完后任务作废")
                        .clicked()
                    {
                        outcome = Some(DialogOutcome::Cancel);
                    }
                    ui.add_enabled(false, egui::Button::new("正在取消…"));
                } else {
                    if ui.button("取消").clicked() {
                        self.request_cancel();
                    }
                    // 手动双面还要回来等翻面，不能转后台。
                    if manual.is_none()
                        && ui
                            .button("转到后台")
                            .on_hover_text("关掉这个窗口继续干活，打完后在状态栏报告")
                            .clicked()
                    {
                        outcome = Some(DialogOutcome::Cancel);
                    }
                }
            });
        });
        outcome
    }

    /// 手动双面第一轮打完：等用户把整叠纸翻面放回纸盒。
    fn waiting_flip_contents(&mut self, ui: &mut egui::Ui) -> Option<DialogOutcome> {
        ui.add_space(4.0);
        ui.label("正面已送打印机。等纸全部打出后，把整叠纸翻面放回纸盒，再点「继续打印反面」。");
        if self.manual_passes.as_ref().is_some_and(|passes| passes.odd) {
            ui.label("最后一张没有反面，放回前先把它取出。");
        }
        if let Some(error) = &self.manual_error {
            ui.add_space(4.0);
            ui.colored_label(theme::danger(), error);
        }
        ui.add_space(8.0);
        let changed = ui
            .checkbox(&mut self.manual_reverse, "反面倒序出纸")
            .on_hover_text("出纸面朝上的机器倒序打出，翻面后正反才对得上")
            .changed();
        if changed {
            self.manual_reverse_dirty = true;
        }
        muted(ui, "第一次用这台打印机，建议先拿两三页试一下翻面方向。");
        ui.add_space(12.0);
        let mut outcome = None;
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary_icon_button(ui, theme::Icon::Print, "继续打印反面").clicked()
                {
                    outcome = self.begin_second_pass();
                }
                if ui.button("取消").clicked() {
                    outcome = Some(DialogOutcome::Cancel);
                }
            });
        });
        outcome
    }

    /// 点「打印」：手动双面先送正面轮并留在对话框里，其余照旧交给外壳。
    fn print_outcome(&mut self, pages: &Result<Selection, String>) -> Option<DialogOutcome> {
        if self.effective_duplex() == Some(Duplex::Manual) {
            return match self.begin_manual(pages) {
                Ok(job) => Some(DialogOutcome::Print(job)),
                Err(error) => {
                    self.manual_error = Some(error);
                    None
                }
            };
        }
        let job = self.job(pages)?;
        self.start_printing(None);
        Some(DialogOutcome::Print(job))
    }

    /// 要打的页展开成区间；「全部」用总页数补出来。页数未知时不能手动双面
    /// （不知道该把第几张分到正面）。
    fn current_spans(&self, pages: &Result<Selection, String>) -> Option<Vec<PageSpan>> {
        let selection = pages.as_ref().ok()?;
        match &selection.spans {
            Some(spans) => Some(spans.clone()),
            None => {
                let count = u32::try_from(self.request.page_count).ok()?;
                (count > 0).then_some(vec![(1, count)])
            }
        }
    }

    fn begin_manual(&mut self, pages: &Result<Selection, String>) -> Result<PrintJob, String> {
        let spans = self
            .current_spans(pages)
            .ok_or_else(|| "页数未知，无法手动双面".to_string())?;
        let (front, back, odd) = pages::manual_duplex_passes(&spans);
        if front.is_empty() {
            return Err("没有要打印的页面".to_string());
        }
        let job = self.manual_job(&front, false);
        self.manual_passes = Some(ManualPasses { back, odd });
        self.start_printing(Some(ManualPass::First));
        Ok(job)
    }

    fn begin_second_pass(&mut self) -> Option<DialogOutcome> {
        let back = self.manual_passes.as_ref()?.back.clone();
        // 只有一页时没有反面可打，直接收场。
        if back.is_empty() {
            return Some(DialogOutcome::Cancel);
        }
        let job = self.manual_job(&back, self.manual_reverse);
        self.start_printing(Some(ManualPass::Second));
        Some(DialogOutcome::Print(job))
    }

    /// 手动双面的每一轮：只打这一轮的页、固定一份、倒序按偏好。
    fn manual_job(&self, pages: &[PageSpan], reverse: bool) -> PrintJob {
        let mut job = self.build_job(Some(pages.to_vec()), 1, false, Some(Duplex::Manual));
        job.reverse = reverse;
        job
    }

    /// 后台打印结果回来了。返回 `true` 表示对话框可以关了。
    /// 这一轮打印的取消标志。外壳拿它交给后台线程，同一个 `Arc`。
    pub(crate) fn cancel_token(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    /// 后台报来的进度。CUPS 不报，Windows 每送一页报一次。
    pub(crate) fn progress(&mut self, done: u32, total: u32) {
        self.progress = Some((done, total));
    }

    fn request_cancel(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn cancelling(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// 开始一轮打印：清掉上一轮的进度与取消标志，进入「送纸中」。
    fn start_printing(&mut self, manual: Option<ManualPass>) {
        self.cancel.store(false, Ordering::Relaxed);
        self.progress = None;
        self.manual_error = None;
        self.stage = Stage::Printing { manual };
    }

    pub(crate) fn manual_finished(&mut self, result: &Result<PrintOutcome, String>) -> bool {
        match (self.stage, result) {
            // 普通打印：打完就关，失败则留在对话框里显示原因。
            (Stage::Printing { manual: None }, Ok(_)) => true,
            (Stage::Printing { manual: None }, Err(error)) => {
                self.manual_error = Some(error.clone());
                self.stage = Stage::Editing;
                false
            }
            // 手动双面正面轮被取消：没有正面可翻，回到可编辑状态。
            (
                Stage::Printing {
                    manual: Some(ManualPass::First),
                },
                Ok(PrintOutcome::Cancelled),
            ) => {
                self.manual_error = Some("已取消打印。".to_string());
                self.stage = Stage::Editing;
                false
            }
            // 手动双面第一轮成功后等翻面。
            (
                Stage::Printing {
                    manual: Some(ManualPass::First),
                },
                Ok(_),
            ) => {
                self.manual_error = None;
                self.stage = Stage::WaitingFlip;
                false
            }
            (
                Stage::Printing {
                    manual: Some(ManualPass::First),
                },
                Err(error),
            ) => {
                // 第一轮失败：回到可编辑状态，让用户改完重试。
                self.manual_error = Some(error.clone());
                self.stage = Stage::Editing;
                false
            }
            (
                Stage::Printing {
                    manual: Some(ManualPass::Second),
                },
                Ok(_),
            ) => true,
            (
                Stage::Printing {
                    manual: Some(ManualPass::Second),
                },
                Err(error),
            ) => {
                // 第二轮失败：回到等待翻面，纸别动，修好后可以再点继续。
                self.manual_error = Some(error.clone());
                self.stage = Stage::WaitingFlip;
                false
            }
            _ => false,
        }
    }

    /// 「反面倒序」改了要马上记进这台打印机的偏好（外壳每帧取一次）。
    pub(crate) fn take_preferences_update(&mut self) -> Option<(String, PrinterPrefs)> {
        if !self.manual_reverse_dirty {
            return None;
        }
        self.manual_reverse_dirty = false;
        let update = self.preferences()?;
        // 对话框里的偏好副本也要跟着改：之后能力、事实再到达时重算预填值，
        // 不能把刚改的「反面倒序」又按旧偏好改回去。
        self.prefs.insert(update.0.clone(), update.1.clone());
        Some(update)
    }

    fn job(&self, pages: &Result<Selection, String>) -> Option<PrintJob> {
        let selection = pages.as_ref().ok()?;
        let copies = self.copies.clamp(1, MAX_COPIES);
        Some(self.build_job(
            selection.spans.clone(),
            copies,
            copies > 1 && self.collate,
            self.effective_duplex(),
        ))
    }

    fn build_job(
        &self,
        pages: Option<Vec<PageSpan>>,
        copies: u32,
        collate: bool,
        duplex: Option<Duplex>,
    ) -> PrintJob {
        PrintJob {
            path: self.request.path.clone(),
            title: self.request.title.clone(),
            printer: self.selected.clone(),
            copies,
            collate,
            pages,
            duplex,
            color: self.effective_color(),
            paper: self.paper.clone(),
            tray: self.tray.clone(),
            scaling: self.scaling,
            devmode: self.devmode.clone(),
            reverse: false,
        }
    }

    /// 这台打印机不支持双面时，即使状态里留着长边也不提交：新选的打印机可能
    /// 没有双面单元（换机前留下的选择、或从偏好里预填的）。手动双面不需要硬件。
    fn effective_duplex(&self) -> Option<Duplex> {
        if self.duplex == Some(Duplex::Manual) {
            return self.duplex;
        }
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

/// 要打的页号（从 1 起，按出纸顺序，不展开份数重复）。
fn print_page_numbers(pages: &Result<Selection, String>, page_count: usize) -> Vec<u32> {
    let Ok(selection) = pages else {
        return Vec::new();
    };
    match &selection.spans {
        Some(spans) => spans.iter().flat_map(|&(start, end)| start..=end).collect(),
        None => (1..=u32::try_from(page_count).unwrap_or(0)).collect(),
    }
}

/// 常见纸型的毫米尺寸（宽 × 高）。认不出的纸型由调用方退回 A4。
fn paper_size_mm(id: &str, label: &str) -> Option<(f32, f32)> {
    let text = format!("{id} {label}").to_ascii_uppercase();
    let table: [(&str, (f32, f32)); 8] = [
        ("A3", (297.0, 420.0)),
        ("A4", (210.0, 297.0)),
        ("A5", (148.0, 210.0)),
        ("A6", (105.0, 148.0)),
        ("B5", (176.0, 250.0)),
        ("LETTER", (215.9, 279.4)),
        ("LEGAL", (215.9, 355.6)),
        ("EXECUTIVE", (184.15, 266.7)),
    ];
    table
        .iter()
        .find_map(|(needle, size)| text.contains(needle).then_some(*size))
}

/// 按纵横比（高 / 宽）把页面放进容器并等比缩小。
fn fit_inside(size: egui::Vec2, aspect: f32) -> egui::Vec2 {
    let aspect = aspect.max(0.01);
    let width = size.x.min(size.y / aspect);
    egui::vec2(width, width * aspect)
}

fn duplex_label(duplex: Option<Duplex>) -> String {
    match duplex {
        None => "打印机默认".to_string(),
        Some(Duplex::Simplex) => "单面".to_string(),
        Some(Duplex::LongEdge) => "长边翻转".to_string(),
        Some(Duplex::ShortEdge) => "短边翻转".to_string(),
        Some(Duplex::Manual) => "手动双面（翻面再打）".to_string(),
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
mod tests;
