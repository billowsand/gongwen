//! 闸门外壳：锁屏 / 首次引导在前，解锁成功后才构造真正的工作区
//! （`GongwenApp`）。外壳只持有锁屏状态与启动参数；`GongwenApp` 里的全部
//! 数据都在解锁之后才会被读进内存，锁定（手动或闲置超时）时整个丢弃。
//!
//! 设计不变量（docs/security-lock-design.md）：
//! - 错误文案只有统一的「密码不正确」，不区分「没有这个密码」；
//! - 锁文件形状恒定，界面不显示「是否设置了第二密码」；
//! - 诱饵密码进入的空白系统与真实系统共用同一套界面，只是数据槽位不同。

use super::GongwenApp;
use crate::models::{DiagramTheme, FontConfig, PaperMode};
use crate::storage::{self, DataSlot};
use crate::theme;
use crate::vault::{Probe, Vault};
use eframe::egui;
use std::path::PathBuf;

/// 外壳持有的启动参数：解锁成功后才交给工作区。锁屏期间双击 .gwx 之类
/// 转交进来的请求排队等解锁，不绕过闸门。
struct StartupBundle {
    startup_imports: Vec<PathBuf>,
    instance: Option<crate::single_instance::Instance>,
    macos_titlebar_metrics: Option<crate::macos_window::NativeTitlebarMetrics>,
}

enum ShellPhase {
    Gate(GateState),
    Ready(Box<GongwenApp>),
}

/// 启动时由 `Vault::probe()` 决定的闸门形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateMode {
    /// 没有锁文件：第一次启动，引导设置密码（也可明确跳过）。
    FirstRun,
    /// 正常待解锁。
    Unlock,
    /// 锁文件损坏：不许猜着解锁，只能清除后重启。
    Broken,
}

struct GateState {
    mode: GateMode,
    password: String,
    confirm: String,
    decoy: String,
    decoy_confirm: String,
    with_decoy: bool,
    /// 统一的失败文案：密码错误、两次输入不一致、锁文件异常都走这里，
    /// 不区分类别。
    error: Option<String>,
    /// 首帧把焦点放进第一个密码框。
    focused: bool,
    /// 损坏锁设置已清除：等用户重启，不再给别的出路。
    cleared: bool,
    /// 正在后台跑的数据处理（明文→密文迁移 / 停用解密）：
    /// 期间锁屏只显示进度，完成后才进工作区。
    working: Option<GateWorking>,
}

/// 闸门上的后台任务：数据密钥已在句柄里，线程只做搬运。
struct GateWorking {
    title: String,
    kind: WorkKind,
    slot: DataSlot,
    rx: std::sync::mpsc::Receiver<Result<(), String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkKind {
    /// 解锁 / 启用后的明文→密文迁移。完成后按槽位进加密工作区。
    Encrypt,
    /// 停用的最后一步：密文→明文。完成后删锁文件、进明文工作区。
    Disable,
}

impl GateState {
    fn first_run() -> Self {
        Self {
            mode: GateMode::FirstRun,
            password: String::new(),
            confirm: String::new(),
            decoy: String::new(),
            decoy_confirm: String::new(),
            with_decoy: false,
            error: None,
            focused: false,
            cleared: false,
            working: None,
        }
    }

    fn unlock() -> Self {
        Self {
            mode: GateMode::Unlock,
            ..Self::first_run()
        }
    }

    fn broken() -> Self {
        Self {
            mode: GateMode::Broken,
            ..Self::first_run()
        }
    }
}

/// 锁屏这一次交互要外壳做的事。
enum GateOutcome {
    Unlock(String),
    Setup { real: String, decoy: Option<String> },
    SkipSetup,
    ClearBroken,
}

/// 应用外壳：先过闸门，再进工作区。
pub(crate) struct AppShell {
    phase: ShellPhase,
    startup: Option<StartupBundle>,
}

impl AppShell {
    pub(crate) fn new(
        cc: &eframe::CreationContext<'_>,
        startup_imports: Vec<PathBuf>,
        instance: Option<crate::single_instance::Instance>,
    ) -> Self {
        // 主题先装起来：锁屏也要跟随主题。此时只能只读探真实系统的配置
        // （诱饵槽位的主题等解锁后按槽位配置重套），配置读不到就用默认主题。
        theme::set_current(storage::peek_theme());
        theme::set_current_paper(PaperMode::Follow);
        crate::mermaid::set_theme(DiagramTheme::Auto);
        theme::configure_fonts(&cc.egui_ctx, &FontConfig::default());
        theme::configure_icons(&cc.egui_ctx);
        theme::configure_style(&cc.egui_ctx);
        theme::omarchy::start_watcher(&cc.egui_ctx);
        let macos_titlebar_metrics = crate::macos_window::configure_native_titlebar(cc);

        let startup = StartupBundle {
            startup_imports,
            instance,
            macos_titlebar_metrics,
        };
        let shell = Self {
            phase: ShellPhase::Gate(GateState::first_run()),
            startup: Some(startup),
        };
        match Vault::probe() {
            Probe::Absent => shell,
            Probe::Skipped => shell.boot_direct(&cc.egui_ctx),
            Probe::Locked => Self::gate(GateState::unlock(), shell.startup),
            Probe::Broken(_) => Self::gate(GateState::broken(), shell.startup),
        }
    }

    /// 未启用（或用户选过跳过）：直接进工作区，等价于旧行为。
    fn boot_direct(mut self, ctx: &egui::Context) -> Self {
        self.enter_workspace(ctx, DataSlot::Real, None);
        self
    }

    fn gate(gate: GateState, startup: Option<StartupBundle>) -> Self {
        Self {
            phase: ShellPhase::Gate(gate),
            startup,
        }
    }

    /// 构造工作区并切过去。`notice` 是解锁后要放进状态栏的一句话。
    fn enter_workspace(&mut self, ctx: &egui::Context, slot: DataSlot, notice: Option<String>) {
        storage::set_current_slot(slot);
        let Some(startup) = self.startup.take() else {
            return;
        };
        let mut app = GongwenApp::new_unlocked(
            ctx,
            startup.startup_imports,
            startup.instance,
            startup.macos_titlebar_metrics,
        );
        if let Some(notice) = notice {
            app.status = notice;
        }
        self.phase = ShellPhase::Ready(Box::new(app));
    }

    /// 起一个闸门后台任务：数据密钥进句柄，搬运在线程里跑，锁屏显示进度。
    /// Disable 的句柄立刻按密文开库（磁盘已是密文）；Encrypt 此刻 db_active=false，
    /// 迁移完成才切 true。
    fn begin_work(&mut self, kind: WorkKind, slot: DataSlot, dek: [u8; 32], title: &str) {
        crate::vault::set_key_handle(dek, kind == WorkKind::Disable);
        crate::storage::set_current_slot(slot);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = match kind {
                WorkKind::Encrypt => crate::vault::ensure_slot_encrypted(),
                WorkKind::Disable => crate::vault::decrypt_slot_files(),
            }
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send(result);
        });
        let mut gate = GateState::unlock();
        gate.working = Some(GateWorking {
            title: title.to_string(),
            kind,
            slot,
            rx,
        });
        self.phase = ShellPhase::Gate(gate);
    }

    /// 后台任务收尾：成功才切工作区；失败回到锁屏并给提示
    ///（两类迁移都幂等，重输密码会接着收拾残局）。
    fn finish_work(
        &mut self,
        ctx: &egui::Context,
        kind: WorkKind,
        slot: DataSlot,
        result: Result<(), String>,
    ) {
        match result {
            Ok(()) => match kind {
                WorkKind::Encrypt => {
                    crate::vault::activate_db_encryption();
                    self.enter_workspace(ctx, slot, None);
                }
                WorkKind::Disable => {
                    crate::vault::clear_key_handle();
                    crate::vault::clear_disable_pending();
                    let notice = match Vault::reset_file()
                        .and_then(|()| Vault::create("", None, true).map(|_| ()))
                    {
                        Ok(()) => "密码保护已停用，数据已解密。".to_string(),
                        Err(error) => format!("数据已解密，但清理锁设置失败：{error:#}"),
                    };
                    self.enter_workspace(ctx, DataSlot::Real, Some(notice));
                }
            },
            Err(message) => {
                crate::vault::clear_key_handle();
                let mut gate = GateState::unlock();
                gate.error = Some(format!("数据处理失败：{message}"));
                self.phase = ShellPhase::Gate(gate);
            }
        }
    }

    /// 处理锁屏提交。所有失败都翻译成统一文案，不区分原因类别。
    fn apply_outcome(&mut self, ctx: &egui::Context, outcome: GateOutcome) {
        let mut gate = GateState::unlock();
        match outcome {
            GateOutcome::Unlock(password) => {
                match Vault::load() {
                    Ok(mut vault) => match vault.unlock(&password) {
                        Some(unlocked) => {
                            let disable = crate::vault::disable_pending();
                            // 没有要搬的明文数据（通常：上次解锁已迁完）就直接
                            // 进工作区，不再显示「正在加密」卡片——那张卡只在
                            // 真的有活干时出现。两套密码各自只看自己槽位的文件，
                            // 交替解锁互不触发对方的数据搬运。
                            if !disable && !crate::vault::slot_needs_encryption(unlocked.slot) {
                                crate::vault::set_key_handle(*unlocked.dek, true);
                                self.enter_workspace(ctx, unlocked.slot, None);
                                return;
                            }
                            let (kind, title) = if disable {
                                (WorkKind::Disable, "正在解密数据并停用密码保护…")
                            } else {
                                (WorkKind::Encrypt, "正在加密现有数据…")
                            };
                            self.begin_work(kind, unlocked.slot, *unlocked.dek, title);
                            return;
                        }
                        None => gate.error = Some("密码不正确".into()),
                    },
                    Err(_) => {
                        // 探测之后文件变损坏（极少见）：不许猜着解锁。
                        gate = GateState::broken();
                    }
                }
            }
            GateOutcome::Setup { real, decoy } => {
                match Vault::create(&real, decoy.as_deref(), false) {
                    Ok((_, Some(unlocked))) => {
                        self.begin_work(
                            WorkKind::Encrypt,
                            DataSlot::Real,
                            *unlocked.dek,
                            "正在加密现有数据…",
                        );
                        return;
                    }
                    Ok((_, None)) | Err(_) => {
                        gate = GateState::first_run();
                        gate.error = Some("锁设置保存失败，请重试".into());
                    }
                }
            }
            GateOutcome::SkipSetup => {
                if let Err(error) = Vault::create("", None, true) {
                    // 写跳过标记失败不阻塞进入，下次启动再引导一次。
                    let mut skipped = GateState::first_run();
                    skipped.error = Some(format!(
                        "记住「暂不设置」的选择失败，下次启动会再询问：{error:#}"
                    ));
                    self.phase = ShellPhase::Gate(skipped);
                    return;
                }
                self.enter_workspace(ctx, DataSlot::Real, None);
                return;
            }
            GateOutcome::ClearBroken => {
                match Vault::reset_file() {
                    Ok(()) => {
                        let mut broken = GateState::broken();
                        broken.cleared = true;
                        self.phase = ShellPhase::Gate(broken);
                    }
                    Err(error) => {
                        let mut broken = GateState::broken();
                        broken.error = Some(format!("清除锁设置失败：{error:#}"));
                        self.phase = ShellPhase::Gate(broken);
                    }
                }
                return;
            }
        }
        self.phase = ShellPhase::Gate(gate);
    }

    /// 退回锁屏：丢掉整个工作区（数据库连接、内存数据随 Drop 释放），
    /// 数据密钥句柄一并清掉——锁定状态内存里不留密钥。
    fn lock_now(&mut self) {
        crate::vault::clear_key_handle();
        storage::set_current_slot(DataSlot::Real);
        self.phase = ShellPhase::Gate(GateState::unlock());
    }
}

impl AppShell {
    pub(crate) fn run(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // 无边框窗口（Windows / 传统 Linux）：锁屏也要有一条能拖动、
        // 能关窗的标题栏。只在锁屏期画——工作区自己的标题栏由
        // update_workspace 画，这里再画一条就成了双标题栏。
        if matches!(self.phase, ShellPhase::Gate(_)) {
            egui::Panel::top("gate_titlebar")
                .show_separator_line(!theme::smartisan::active())
                .frame(
                    egui::Frame::new()
                        .fill(theme::surface())
                        .inner_margin(egui::Margin::ZERO),
                )
                .show(ui, bare_titlebar);
        }

        enum After {
            Outcome(GateOutcome),
            WorkDone {
                kind: WorkKind,
                slot: DataSlot,
                result: Result<(), String>,
            },
            Lock,
        }
        let after = match &mut self.phase {
            ShellPhase::Gate(gate) => match gate_ui(ui, gate) {
                Some(GateEvent::Submit(outcome)) => Some(After::Outcome(outcome)),
                Some(GateEvent::WorkDone { kind, slot, result }) => {
                    Some(After::WorkDone { kind, slot, result })
                }
                None => None,
            },
            ShellPhase::Ready(app) => {
                app.update_workspace(ui, frame);
                app.take_lock_requested().then_some(After::Lock)
            }
        };
        match after {
            Some(After::Lock) => self.lock_now(),
            Some(After::Outcome(outcome)) => self.apply_outcome(&ctx, outcome),
            Some(After::WorkDone { kind, slot, result }) => {
                self.finish_work(&ctx, kind, slot, result)
            }
            None => {}
        }
        // 锁屏期间保持后台任务（Omarchy 跟随）能推动重绘。
        if matches!(self.phase, ShellPhase::Gate(_)) {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
    }
}

impl eframe::App for AppShell {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.run(ui, frame);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::canvas().to_normalized_gamma_f32()
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let ShellPhase::Ready(app) = &mut self.phase {
            app.shutdown();
        }
    }
}

/// 锁屏的标题栏：品牌标 + 居中应用名 + 右侧窗口控制。比工作区的标题栏
/// 少掉快速访问与标签标题，其余视觉语言一致。
fn bare_titlebar(ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let height = if theme::smartisan::active() {
        30.0
    } else {
        34.0
    };
    const BTN_W: f32 = 46.0;
    let maximized = ctx
        .input(|input| input.viewport().maximized)
        .unwrap_or(false);
    let focused = ctx.input(|input| input.viewport().focused).unwrap_or(true);
    let title_color = if theme::smartisan::active() {
        theme::smartisan::chrome_ink().gamma_multiply(if focused { 1.0 } else { 0.75 })
    } else if focused {
        theme::text_soft()
    } else {
        theme::text_muted()
    };

    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    theme::smartisan::chrome(ui.painter(), rect);

    let hyprland = crate::linux_desktop::hyprland_session();
    let buttons: &[TitlebarBtn] = if hyprland {
        &[TitlebarBtn::Close]
    } else {
        &[
            TitlebarBtn::Minimize,
            TitlebarBtn::Maximize,
            TitlebarBtn::Close,
        ]
    };
    let controls_width = buttons.len() as f32 * BTN_W;

    let title_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 14.0, rect.top()),
        egui::pos2(rect.right() - controls_width, rect.bottom()),
    );
    let mut text_right = title_rect.left();
    if title_rect.width() > 60.0 {
        let brand_rect = egui::Rect::from_center_size(
            title_rect.left_center() + egui::vec2(9.0, 0.0),
            egui::vec2(18.0, 18.0),
        );
        let brand_color = if theme::smartisan::active() {
            title_color
        } else if focused {
            theme::accent()
        } else {
            title_color
        };
        ui.put(brand_rect, theme::brand_image(18.0, brand_color))
            .on_hover_text(crate::version::APP_TITLE);
        text_right = brand_rect.right();
    }
    // 应用名按整窗几何中心摆放，左右让开品牌标与窗口按钮。
    {
        let center_x = rect.center().x;
        let half_width = (center_x - (text_right + 16.0))
            .min(rect.right() - controls_width - 16.0 - center_x)
            .max(0.0);
        if half_width > 40.0 {
            let clip = egui::Rect::from_min_max(
                egui::pos2(center_x - half_width, rect.top()),
                egui::pos2(center_x + half_width, rect.bottom()),
            );
            ui.painter().with_clip_rect(clip).text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                crate::version::APP_TITLE,
                egui::TextStyle::Body.resolve(ui.style()),
                title_color,
            );
        }
    }
    let drag = ui.interact(
        title_rect,
        ui.id().with("gate_titlebar_drag"),
        egui::Sense::click_and_drag(),
    );
    if drag.double_clicked() && !hyprland {
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    } else if drag.drag_started() {
        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }

    let mut action: Option<TitlebarAction> = None;
    for (index, kind) in buttons.iter().copied().enumerate() {
        let btn_rect = egui::Rect::from_min_size(
            egui::pos2(
                rect.right() - (buttons.len() - index) as f32 * BTN_W,
                rect.top(),
            ),
            egui::vec2(BTN_W, height),
        );
        let response = ui
            .interact(
                btn_rect,
                ui.id().with(("gate_titlebar_btn", index)),
                egui::Sense::click(),
            )
            .on_hover_text(match kind {
                TitlebarBtn::Minimize => "最小化",
                TitlebarBtn::Maximize => {
                    if maximized {
                        "还原"
                    } else {
                        "最大化"
                    }
                }
                TitlebarBtn::Close => "关闭",
            });
        let is_close = kind == TitlebarBtn::Close;
        let hovered = response.hovered();
        let pressed = response.is_pointer_button_down_on();
        let fill = if hovered {
            if is_close {
                if pressed {
                    theme::danger().gamma_multiply(0.85)
                } else {
                    theme::danger()
                }
            } else if pressed {
                theme::surface_active()
            } else {
                theme::surface_hover()
            }
        } else {
            egui::Color32::TRANSPARENT
        };
        if fill != egui::Color32::TRANSPARENT {
            ui.painter().rect_filled(btn_rect, 0.0, fill);
        }
        let icon_color = if is_close && hovered {
            theme::accent_text()
        } else if hovered && theme::smartisan::active() {
            theme::text()
        } else {
            title_color
        };
        let glyph = match kind {
            TitlebarBtn::Minimize => WindowGlyph::Minimize,
            TitlebarBtn::Maximize if maximized => WindowGlyph::Restore,
            TitlebarBtn::Maximize => WindowGlyph::Maximize,
            TitlebarBtn::Close => WindowGlyph::Close,
        };
        draw_window_glyph(ui.painter(), btn_rect.center(), glyph, icon_color);
        if response.clicked() {
            action = Some(match kind {
                TitlebarBtn::Minimize => TitlebarAction::Minimize,
                TitlebarBtn::Maximize => TitlebarAction::Maximized(!maximized),
                TitlebarBtn::Close => TitlebarAction::Close,
            });
        }
    }
    match action {
        Some(TitlebarAction::Minimize) => {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
        Some(TitlebarAction::Maximized(next)) => {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(next));
        }
        Some(TitlebarAction::Close) => {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        None => {}
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TitlebarAction {
    Minimize,
    Maximized(bool),
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TitlebarBtn {
    Minimize,
    Maximize,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowGlyph {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// 与工作区标题栏同一套纯几何窗口符号（横线、方框、叠框、叉）。
fn draw_window_glyph(
    painter: &egui::Painter,
    center: egui::Pos2,
    glyph: WindowGlyph,
    color: egui::Color32,
) {
    let stroke = egui::Stroke::new(1.4, color);
    match glyph {
        WindowGlyph::Minimize => {
            painter.rect_filled(
                egui::Rect::from_center_size(center, egui::vec2(10.0, 1.6)),
                0.0,
                color,
            );
        }
        WindowGlyph::Maximize => {
            painter.rect_stroke(
                egui::Rect::from_center_size(center, egui::vec2(11.0, 11.0)),
                egui::CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        WindowGlyph::Restore => {
            let main =
                egui::Rect::from_center_size(center + egui::vec2(1.5, 1.5), egui::vec2(10.0, 10.0));
            painter.rect_stroke(
                main,
                egui::CornerRadius::same(1),
                stroke,
                egui::StrokeKind::Inside,
            );
            let sub =
                egui::Rect::from_center_size(center - egui::vec2(1.5, 1.5), egui::vec2(10.0, 10.0));
            painter.line_segment([sub.left_top(), sub.right_top()], stroke);
            painter.line_segment([sub.left_top(), sub.left_bottom()], stroke);
        }
        WindowGlyph::Close => {
            let d = 4.0;
            painter.line_segment(
                [center - egui::vec2(d, d), center + egui::vec2(d, d)],
                stroke,
            );
            painter.line_segment(
                [center - egui::vec2(d, -d), center + egui::vec2(d, -d)],
                stroke,
            );
        }
    }
}

/// 卡片宽度。够放 20 个汉字左右的说明与密码框，超宽屏上不至于散架。
const CARD_WIDTH: f32 = 340.0;
/// 卡片的控件统一高度：比常规行略高半档，锁屏是全场唯一焦点，按得舒服。
const GATE_CONTROL_HEIGHT: f32 = theme::CONTROL_HEIGHT + 4.0;

/// 画一屏闸门，返回这次交互要外壳做的事。
/// 锁屏这一轮交互产生的事件：用户提交，或后台任务跑完了。
enum GateEvent {
    Submit(GateOutcome),
    WorkDone {
        kind: WorkKind,
        slot: DataSlot,
        result: Result<(), String>,
    },
}

fn gate_ui(ui: &mut egui::Ui, gate: &mut GateState) -> Option<GateEvent> {
    let available = ui.available_rect_before_wrap();
    let mut event: Option<GateEvent> = None;
    // 用垂直布局把卡片推到视口正中（与状态栏居中模型的手法一致）。
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(available)
            .layout(egui::Layout::top_down(egui::Align::Center)),
        |ui| {
            ui.add_space((available.height() * 0.5 - 210.0).max(24.0));
            let card = egui::Frame::new()
                .fill(theme::surface())
                .stroke(egui::Stroke::new(1.0, theme::border()))
                .corner_radius(theme::chrome_radius(10))
                .inner_margin(egui::Margin::symmetric(30, 26));
            card.show(ui, |ui| {
                ui.set_width(CARD_WIDTH);
                if gate.working.is_some() {
                    working_card(ui, gate, &mut event);
                } else {
                    event = gate_card(ui, gate).map(GateEvent::Submit);
                }
            });
            ui.add_space(16.0);
            ui.label(
                egui::RichText::new(format!(
                    "v{} · 离线运行 · 数据仅保存在本机",
                    env!("CARGO_PKG_VERSION")
                ))
                .size(theme::font_sizes::SMALL)
                .color(theme::text_muted()),
            );
        },
    );
    event
}

/// 后台数据处理期间的卡片：只显示进度文案，轮询完成事件。
fn working_card(ui: &mut egui::Ui, gate: &mut GateState, event: &mut Option<GateEvent>) {
    let Some(working) = gate.working.as_ref() else {
        return;
    };
    ui.vertical_centered(|ui| {
        ui.add(theme::Icon::Shield.image_sized(30.0).tint(theme::accent()));
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(&working.title)
                .size(theme::font_sizes::HEADING)
                .strong()
                .color(theme::text()),
        );
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new("请稍候，不要关闭应用。")
                .size(theme::font_sizes::SMALL)
                .color(theme::text_muted()),
        );
    });
    if let Ok(result) = working.rx.try_recv() {
        let working = gate.working.take().expect("working 刚确认存在");
        *event = Some(GateEvent::WorkDone {
            kind: working.kind,
            slot: working.slot,
            result,
        });
    }
}

/// 卡片内容：首帧聚焦第一个密码框，Enter 提交，失败文案统一挂在卡片底部。
fn gate_card(ui: &mut egui::Ui, gate: &mut GateState) -> Option<GateOutcome> {
    if !gate.focused {
        gate.focused = true;
        ui.memory_mut(|memory| memory.request_focus(egui::Id::new("gate_password_primary")));
    }
    let mut outcome = None;
    let heading = match gate.mode {
        GateMode::FirstRun => "设置访问密码",
        GateMode::Unlock => "欢迎回来",
        GateMode::Broken => "锁设置已损坏",
    };
    ui.vertical_centered(|ui| {
        ui.add(theme::Icon::Shield.image_sized(30.0).tint(theme::accent()));
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(heading)
                .size(theme::font_sizes::HEADING)
                .strong()
                .color(theme::text()),
        );
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new(crate::version::APP_TITLE)
                .size(theme::font_sizes::SMALL)
                .color(theme::text_muted()),
        );
    });
    ui.add_space(18.0);

    match gate.mode {
        GateMode::Unlock => {
            if crate::vault::disable_pending() {
                ui.label(
                    egui::RichText::new(
                        "已申请停用密码保护：输入一次密码，完成解密后回到无密码状态。",
                    )
                    .size(theme::font_sizes::SMALL)
                    .color(theme::text_soft()),
                );
                ui.add_space(8.0);
            }
            let id = egui::Id::new("gate_password_primary");
            ui.add_sized(
                [CARD_WIDTH, GATE_CONTROL_HEIGHT],
                egui::TextEdit::singleline(&mut gate.password)
                    .id(id)
                    .password(true)
                    .hint_text("输入密码")
                    .margin(egui::Margin::symmetric(10, 0)),
            );
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) && !gate.password.is_empty() {
                outcome = Some(GateOutcome::Unlock(gate.password.clone()));
            }
            ui.add_space(10.0);
            let enter = theme::accent_scope(ui, |ui| {
                ui.add_sized(
                    [CARD_WIDTH, GATE_CONTROL_HEIGHT],
                    theme::primary_button_widget(theme::Icon::Shield, "进入"),
                )
            })
            .clicked();
            if enter && !gate.password.is_empty() {
                outcome = Some(GateOutcome::Unlock(gate.password.clone()));
            }
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("按 Enter 进入")
                    .size(theme::font_sizes::SMALL)
                    .color(theme::text_muted()),
            );
        }
        GateMode::FirstRun => {
            ui.label(
                egui::RichText::new(
                    "设置后，启动与闲置超时都要输入密码才能进入；忘记密码无法恢复数据。",
                )
                .size(theme::font_sizes::SMALL)
                .color(theme::text_soft()),
            );
            ui.add_space(12.0);
            password_field(
                ui,
                "gate_password_primary",
                "设置密码（至少 4 个字符）",
                &mut gate.password,
            );
            ui.add_space(8.0);
            password_field(
                ui,
                "gate_password_confirm",
                "再输一遍确认",
                &mut gate.confirm,
            );
            ui.add_space(10.0);
            ui.checkbox(&mut gate.with_decoy, "设置第二密码：输入它进入的是空白系统")
                .on_hover_text(
                    "第二密码打开一套完全独立的空白数据；两套密码都能正常进入，界面没有区别。",
                );
            if gate.with_decoy {
                ui.add_space(8.0);
                password_field(ui, "gate_password_decoy", "第二密码", &mut gate.decoy);
                ui.add_space(8.0);
                password_field(
                    ui,
                    "gate_password_decoy_confirm",
                    "再输一遍第二密码",
                    &mut gate.decoy_confirm,
                );
            }
            ui.add_space(14.0);
            let enable = theme::accent_scope(ui, |ui| {
                ui.add_sized(
                    [CARD_WIDTH, GATE_CONTROL_HEIGHT],
                    theme::primary_button_widget(theme::Icon::Shield, "启用密码保护"),
                )
            })
            .clicked();
            if enable {
                outcome = Some(GateOutcome::Setup {
                    real: gate.password.clone(),
                    decoy: gate.with_decoy.then(|| gate.decoy.clone()),
                });
            }
            ui.add_space(8.0);
            if ui
                .add(egui::Button::new("暂不设置，直接进入").frame(false))
                .on_hover_text("本次跳过；之后随时可以在「设置 → 应用锁」里启用。")
                .clicked()
            {
                outcome = Some(GateOutcome::SkipSetup);
            }
        }
        GateMode::Broken => {
            if gate.cleared {
                ui.label(
                    egui::RichText::new("锁设置已清除。请重启应用重新设置密码；数据文件没有改动。")
                        .color(theme::text_soft()),
                );
            } else {
                ui.label(
                    egui::RichText::new(
                        "锁文件读不了，无法验证密码。为防误判，本机数据在清除前保持锁定。",
                    )
                    .color(theme::text_soft()),
                );
                ui.add_space(14.0);
                if ui
                    .button("清除锁设置（下次启动重新设置密码）")
                    .on_hover_text(
                        "只删除锁文件，不动任何数据；清除后任何人都能进入，直到重新设置密码。",
                    )
                    .clicked()
                {
                    outcome = Some(GateOutcome::ClearBroken);
                }
            }
        }
    }

    if let Some(error) = gate.error.clone().filter(|_| outcome.is_none()) {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(error).color(theme::danger()));
    }
    if outcome.is_some() {
        gate.error = None;
    }
    outcome
}

fn password_field(ui: &mut egui::Ui, id: &str, hint: &str, text: &mut String) {
    ui.add_sized(
        [CARD_WIDTH, GATE_CONTROL_HEIGHT],
        egui::TextEdit::singleline(text)
            .id(egui::Id::new(id))
            .password(true)
            .hint_text(hint)
            .margin(egui::Margin::symmetric(10, 0)),
    );
}

#[cfg(test)]
mod samples {
    use super::*;
    use crate::models::ThemeName;

    /// 锁屏样张：浅色、深色与稿纸三套主题 × 解锁 / 首设（含第二密码）两种形态。
    /// 手动跑：cargo test --locked gate_samples -- --ignored
    #[test]
    #[ignore = "出样张，手动跑"]
    fn gate_samples() {
        let previous = crate::models::ThemeName::ALL
            .iter()
            .copied()
            .find(|name| theme::by_name(*name).label == theme::current().label)
            .unwrap_or_default();
        let size = egui::vec2(1280.0, 820.0);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        for (theme_name, tag) in [
            (ThemeName::default(), "claude"),
            (ThemeName::GruvboxDark, "dark"),
            (ThemeName::Smartisan, "paper"),
        ] {
            theme::set_current(theme_name);
            for (mode, mode_tag) in [(GateMode::Unlock, "unlock"), (GateMode::FirstRun, "setup")] {
                let mut gate = match mode {
                    GateMode::Unlock => GateState::unlock(),
                    GateMode::FirstRun => GateState::first_run(),
                    GateMode::Broken => GateState::broken(),
                };
                if mode == GateMode::FirstRun {
                    gate.with_decoy = true;
                }
                let ctx = egui::Context::default();
                theme::configure_icons(&ctx);
                theme::configure_fonts(&ctx, &FontConfig::default());
                theme::configure_style(&ctx);
                ctx.set_pixels_per_point(2.0);
                let mut canvas = crate::ui_snapshot::Canvas::default();
                let mut output = None;
                for _ in 0..12 {
                    let raw = egui::RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    };
                    let mut full = ctx.run_ui(raw, |ui| {
                        egui::Panel::top("gate_titlebar")
                            .show_separator_line(!theme::smartisan::active())
                            .frame(
                                egui::Frame::new()
                                    .fill(theme::surface())
                                    .inner_margin(egui::Margin::ZERO),
                            )
                            .show(ui, bare_titlebar);
                        egui::CentralPanel::default()
                            .frame(egui::Frame::new().fill(theme::canvas()))
                            .show(ui, |central_ui| {
                                gate_ui(central_ui, &mut gate);
                            });
                    });
                    // egui 0.36 要求析构前处理纹理增量；最后一帧的清空后留形状。
                    canvas.absorb(&mut full.textures_delta);
                    full.textures_delta.clear();
                    output = Some(full);
                }
                let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tmp")
                    .join(format!("gate-{tag}-{mode_tag}.png"));
                canvas.render(&ctx, output.unwrap(), size, theme::canvas(), &path);
                println!("{}", path.display());
            }
        }
        theme::set_current(previous);
    }
}
