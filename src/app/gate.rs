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

mod backdrop;

/// 只存外壳外观，不存数据槽、稿件、密码或密钥。
#[derive(serde::Serialize, serde::Deserialize)]
struct ShellPreferences {
    size: [f32; 2],
    position: Option<[f32; 2]>,
    maximized: bool,
    fullscreen: bool,
    theme: crate::models::ThemeName,
}
const SHELL_KEY: &str = "gongwen_shell_v1";

/// 外壳持有的启动参数：解锁成功后才交给工作区。锁屏期间双击 .gwx 之类
/// 转交进来的请求排队等解锁，不绕过闸门。
struct StartupBundle {
    startup_imports: Option<Vec<PathBuf>>,
    instance: Option<crate::single_instance::Instance>,
    forwarded_requests: Option<std::sync::mpsc::Receiver<Vec<PathBuf>>>,
    macos_titlebar_metrics: Option<crate::macos_window::NativeTitlebarMetrics>,
}

type WorkspaceResources = (
    Vec<PathBuf>,
    Option<crate::single_instance::Instance>,
    Option<std::sync::mpsc::Receiver<Vec<PathBuf>>>,
    Option<crate::macos_window::NativeTitlebarMetrics>,
);

impl StartupBundle {
    fn take_workspace_resources(&mut self) -> WorkspaceResources {
        (
            self.startup_imports.take().unwrap_or_default(),
            self.instance.take(),
            self.forwarded_requests.take(),
            self.macos_titlebar_metrics,
        )
    }

    fn retain_lifecycle(
        &mut self,
        instance: Option<crate::single_instance::Instance>,
        forwarded_requests: Option<std::sync::mpsc::Receiver<Vec<PathBuf>>>,
    ) {
        self.instance = instance;
        self.forwarded_requests = forwarded_requests;
    }
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
    show_password: bool,
    /// Runtime lock veils the existing window with unreadable abstract layout masses.
    veiled: bool,
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
            show_password: false,
            veiled: false,
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
    compact_startup: bool,
    saved_workspace_rect: Option<egui::Rect>,
    saved_maximized: bool,
    saved_fullscreen: bool,
    unlock_fade_started: Option<f64>,
}

impl AppShell {
    pub(crate) fn new(
        cc: &eframe::CreationContext<'_>,
        startup_imports: Vec<PathBuf>,
        instance: Option<crate::single_instance::Instance>,
        window: crate::DeferredWindowState,
    ) -> Self {
        // 主题先装起来：锁屏也要跟随主题。此时只能只读探真实系统的配置
        // （诱饵槽位的主题等解锁后按槽位配置重套），配置读不到就用默认主题。
        let preferences = cc
            .storage
            .and_then(|storage| eframe::get_value::<ShellPreferences>(storage, SHELL_KEY));
        let _ = theme::omarchy::refresh();
        theme::set_current(
            preferences
                .as_ref()
                .map_or_else(storage::peek_theme, |p| p.theme),
        );
        theme::set_current_paper(PaperMode::Follow);
        crate::mermaid::set_theme(DiagramTheme::Auto);
        theme::configure_fonts(&cc.egui_ctx, &FontConfig::default());
        theme::configure_icons(&cc.egui_ctx);
        theme::configure_style(&cc.egui_ctx);
        theme::omarchy::start_watcher(&cc.egui_ctx);
        let macos_titlebar_metrics = crate::macos_window::configure_native_titlebar(cc);

        let startup = StartupBundle {
            startup_imports: Some(startup_imports),
            instance,
            forwarded_requests: None,
            macos_titlebar_metrics,
        };
        let mut shell = Self {
            phase: ShellPhase::Gate(GateState::first_run()),
            startup: Some(startup),
            compact_startup: false,
            saved_workspace_rect: Some(egui::Rect::from_min_size(
                preferences
                    .as_ref()
                    .and_then(|p| p.position)
                    .map(egui::Pos2::from)
                    .or(window.position)
                    .unwrap_or(egui::pos2(80.0, 60.0)),
                preferences
                    .as_ref()
                    .map(|p| egui::vec2(p.size[0], p.size[1]))
                    .or(window.inner_size)
                    .unwrap_or(egui::vec2(1280.0, 820.0))
                    .max(egui::vec2(760.0, 600.0)),
            )),
            saved_maximized: preferences
                .as_ref()
                .map_or(window.maximized, |p| p.maximized),
            saved_fullscreen: preferences
                .as_ref()
                .map_or(window.fullscreen, |p| p.fullscreen),
            unlock_fade_started: None,
        };
        match Vault::probe() {
            Probe::Absent => shell,
            Probe::Skipped => shell.boot_direct(&cc.egui_ctx),
            Probe::Locked => {
                shell.compact_gate(&cc.egui_ctx);
                shell.compact_startup = true;
                shell.phase = ShellPhase::Gate(GateState::unlock());
                shell
            }
            Probe::Broken(_) => Self::gate(GateState::broken(), shell.startup),
        }
    }

    /// 未启用（或用户选过跳过）：直接进工作区，等价于旧行为。
    fn boot_direct(mut self, ctx: &egui::Context) -> Self {
        self.restore_workspace_window(ctx);
        self.enter_workspace(ctx, DataSlot::Real, None);
        self
    }

    fn gate(gate: GateState, startup: Option<StartupBundle>) -> Self {
        Self {
            phase: ShellPhase::Gate(gate),
            startup,
            compact_startup: false,
            saved_workspace_rect: None,
            saved_maximized: false,
            saved_fullscreen: false,
            unlock_fade_started: None,
        }
    }

    /// 构造工作区并切过去。`notice` 是解锁后要放进状态栏的一句话。
    fn enter_workspace(&mut self, ctx: &egui::Context, slot: DataSlot, notice: Option<String>) {
        // 不把上一工作区的弹窗、编辑撤销栈与焦点带入另一数据槽。
        ctx.memory_mut(|memory| *memory = egui::Memory::default());
        ctx.data_mut(|data| data.clear());
        storage::set_current_slot(slot);
        let Some(startup) = self.startup.as_mut() else {
            return;
        };
        let (imports, instance, requests, titlebar_metrics) = startup.take_workspace_resources();
        let mut app = GongwenApp::new_unlocked(ctx, imports, instance, requests, titlebar_metrics);
        if let Some(notice) = notice {
            app.status = notice;
        }
        self.phase = ShellPhase::Ready(Box::new(app));
        ctx.memory_mut(|memory| memory.surrender_focus(egui::Id::new("gate_password_primary")));
        self.unlock_fade_started = Some(ctx.input(|input| input.time));
        if self.compact_startup {
            self.restore_workspace_window(ctx);
            self.compact_startup = false;
        }
    }

    /// 起一个闸门后台任务：数据密钥进句柄，搬运在线程里跑，锁屏显示进度。
    /// Disable 的句柄立刻按密文开库（磁盘已是密文）；Encrypt 此刻 db_active=false，
    /// 迁移完成才切 true。
    fn begin_work(&mut self, kind: WorkKind, slot: DataSlot, dek: [u8; 32], title: &str) {
        let veiled = matches!(&self.phase, ShellPhase::Gate(gate) if gate.veiled);
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
        gate.veiled = veiled;
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
                gate.veiled = !self.compact_startup;
                gate.error = Some(format!("数据处理失败：{message}"));
                self.phase = ShellPhase::Gate(gate);
            }
        }
    }

    /// 处理锁屏提交。所有失败都翻译成统一文案，不区分原因类别。
    fn apply_outcome(&mut self, ctx: &egui::Context, outcome: GateOutcome) {
        let mut gate = GateState::unlock();
        gate.veiled = matches!(&self.phase, ShellPhase::Gate(previous) if previous.veiled);
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
        if gate.mode == GateMode::Broken && self.compact_startup {
            self.restore_workspace_window(ctx);
            self.compact_startup = false;
        }
        self.phase = ShellPhase::Gate(gate);
    }

    /// 退回锁屏：丢掉整个工作区（数据库连接、内存数据随 Drop 释放），
    /// 数据密钥句柄一并清掉——锁定状态内存里不留密钥。
    fn lock_now(&mut self, ctx: &egui::Context) {
        let previous = std::mem::replace(&mut self.phase, ShellPhase::Gate(GateState::unlock()));
        if let ShellPhase::Ready(mut app) = previous
            && let Some(startup) = self.startup.as_mut()
        {
            let (instance, receiver) = app.take_lifecycle_for_lock();
            startup.retain_lifecycle(instance, receiver);
        }
        crate::vault::clear_key_handle();
        ctx.memory_mut(|memory| *memory = egui::Memory::default());
        ctx.data_mut(|data| data.clear());
        storage::set_current_slot(DataSlot::Real);
        if let ShellPhase::Gate(gate) = &mut self.phase {
            gate.veiled = true;
        }
    }

    fn compact_gate(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::vec2(
            340.0, 220.0,
        )));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(380.0, 250.0)));
        if let Some(rect) = self.saved_workspace_rect {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(
                rect.center() - egui::vec2(190.0, 125.0),
            ));
        }
    }

    fn restore_workspace_window(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::vec2(
            760.0, 600.0,
        )));
        if let Some(rect) = self.saved_workspace_rect {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(rect.size()));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(rect.min));
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1280.0, 820.0)));
        }
        if self.saved_fullscreen {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        } else if self.saved_maximized {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
        }
    }
}

impl AppShell {
    pub(crate) fn run(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if matches!(self.phase, ShellPhase::Gate(_))
            && theme::current().label == theme::omarchy::LABEL
            && theme::omarchy::palette().is_some_and(|palette| palette != theme::current())
        {
            theme::set_current(crate::models::ThemeName::Omarchy);
            theme::reconfigure_style(&ctx);
        }
        if matches!(self.phase, ShellPhase::Ready(_)) {
            let info = ctx.input(|input| input.viewport().clone());
            if self.unlock_fade_started.is_none() {
                self.saved_fullscreen = info.fullscreen.unwrap_or(false);
                self.saved_maximized = info.maximized.unwrap_or(false);
                if !self.saved_maximized
                    && !self.saved_fullscreen
                    && let Some(rect) = info.inner_rect
                {
                    self.saved_workspace_rect = Some(egui::Rect::from_min_size(
                        info.outer_rect.map_or(rect.min, |outer| outer.min),
                        rect.size(),
                    ));
                }
            }
        }
        // 无边框窗口（Windows / 传统 Linux）：锁屏也要有一条能拖动、
        // 能关窗的标题栏。只在锁屏期画——工作区自己的标题栏由
        // update_workspace 画，这里再画一条就成了双标题栏。
        if matches!(self.phase, ShellPhase::Gate(_)) {
            let minimal = self.compact_startup;
            egui::Panel::top("gate_titlebar")
                .show_separator_line(!theme::smartisan::active())
                .frame(
                    egui::Frame::new()
                        .fill(theme::surface())
                        .inner_margin(egui::Margin::ZERO),
                )
                .show(ui, |ui| bare_titlebar(ui, minimal));
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
                let fading = self
                    .unlock_fade_started
                    .is_some_and(|started| ctx.input(|input| input.time - started < 0.3));
                if fading {
                    ctx.input_mut(|input| {
                        input.raw.events.clear();
                        input.pointer = egui::PointerState::default();
                    });
                }
                app.update_workspace(ui, frame);
                if app.take_lock_requested() {
                    if let Err(error) = app.save_before_lock() {
                        app.status = format!("锁定前保存失败，工作区仍保持打开：{error:#}");
                        None
                    } else {
                        // Lock was requested during workspace rendering. Cover existing egui
                        // areas in this same frame so no clear-content frame is presented.
                        paint_lock_veil(&ctx, 1.0);
                        Some(After::Lock)
                    }
                } else {
                    None
                }
            }
        };
        match after {
            Some(After::Lock) => self.lock_now(&ctx),
            Some(After::Outcome(outcome)) => self.apply_outcome(&ctx, outcome),
            Some(After::WorkDone { kind, slot, result }) => {
                self.finish_work(&ctx, kind, slot, result)
            }
            None => {}
        }
        if let Some(started) = self.unlock_fade_started {
            let elapsed = ctx.input(|input| input.time - started);
            if elapsed >= 0.3 {
                self.unlock_fade_started = None;
            } else {
                paint_lock_veil(&ctx, (1.0 - elapsed / 0.3).clamp(0.0, 1.0) as f32);
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
        }
        // 锁屏期间保持后台任务（Omarchy 跟随）能推动重绘。
        if matches!(self.phase, ShellPhase::Gate(_)) {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
    }
}

fn paint_lock_veil(ctx: &egui::Context, opacity: f32) {
    // ViewportInfo::inner_rect 是桌面坐标，绘制必须使用本地内容坐标。
    let rect = ctx.input(|input| input.content_rect());
    let id = egui::Id::new("lock_transition_veil");
    egui::Area::new(id)
        .order(egui::Order::Tooltip)
        .fade_in(false)
        .constrain(false)
        .fixed_pos(rect.min)
        .movable(false)
        .interactable(true)
        .show(ctx, |ui| {
            ui.set_min_size(rect.size());
            ui.interact(
                rect,
                ui.id().with("input_shield"),
                egui::Sense::click_and_drag(),
            );
        });
    // Area 首帧是隐藏测量帧。直接画在已经登记的层上，保证锁定当帧也完全遮盖。
    let layer = egui::LayerId::new(egui::Order::Tooltip, id);
    ctx.move_to_top(layer);
    backdrop::paint(&ctx.layer_painter(layer), rect, opacity);
}

impl eframe::App for AppShell {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.run(ui, frame);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::canvas().to_normalized_gamma_f32()
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let rect = self.saved_workspace_rect.unwrap_or_else(|| {
            egui::Rect::from_min_size(egui::pos2(80.0, 60.0), egui::vec2(1280.0, 820.0))
        });
        let theme = crate::models::ThemeName::ALL
            .iter()
            .copied()
            .chain(std::iter::once(crate::models::ThemeName::Omarchy))
            .find(|name| theme::by_name(*name).label == theme::current().label)
            .unwrap_or_default();
        eframe::set_value(
            storage,
            SHELL_KEY,
            &ShellPreferences {
                size: [rect.width(), rect.height()],
                position: Some([rect.min.x, rect.min.y]),
                maximized: self.saved_maximized,
                fullscreen: self.saved_fullscreen,
                theme,
            },
        );
    }

    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let ShellPhase::Ready(app) = &mut self.phase {
            app.shutdown();
        }
    }
}

/// 锁屏的标题栏：品牌标 + 居中应用名 + 右侧窗口控制。比工作区的标题栏
/// 少掉快速访问与标签标题，其余视觉语言一致。
fn bare_titlebar(ui: &mut egui::Ui, minimal: bool) {
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
    let buttons: &[TitlebarBtn] = if minimal || hyprland {
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
    if minimal {
        ui.painter().text(
            egui::pos2(text_right + 8.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            crate::version::APP_TITLE,
            egui::TextStyle::Body.resolve(ui.style()),
            title_color,
        );
    }
    // 应用名按整窗几何中心摆放，左右让开品牌标与窗口按钮。
    {
        let center_x = if minimal { -1000.0 } else { rect.center().x };
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
    if gate.veiled {
        draw_veil_masses(ui, available);
    }
    if gate.mode == GateMode::Unlock {
        if gate.working.is_some() {
            compact_working(ui, gate, &mut event);
        } else {
            unlock_form(ui, gate, &mut event);
        }
        return event;
    }
    // 用垂直布局把卡片推到视口正中（与状态栏居中模型的手法一致）。
    egui::ScrollArea::vertical()
        .id_salt("gate_setup_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(available)
                    .layout(egui::Layout::top_down(egui::Align::Center)),
                |ui| {
                    ui.add_space(((available.height() - 650.0) * 0.5).max(16.0));
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
        });
    event
}

/// Compact startup unlock form. Runtime lock uses it over themed abstract layout masses;
/// no screenshot pixels or readable workspace content are retained.
fn unlock_form(ui: &mut egui::Ui, gate: &mut GateState, event: &mut Option<GateEvent>) {
    if !gate.focused {
        gate.focused = true;
        ui.memory_mut(|memory| memory.request_focus(egui::Id::new("gate_password_primary")));
    }
    let available = ui.available_rect_before_wrap();
    let width = (available.width() - 48.0).clamp(180.0, 304.0);
    let height = if crate::vault::disable_pending() {
        178.0
    } else {
        154.0
    };
    let form = egui::Rect::from_center_size(available.center(), egui::vec2(width, height));
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(form)
            .layout(egui::Layout::top_down(egui::Align::Min)),
        |ui| {
            ui.set_width(width);
            if crate::vault::disable_pending() {
                ui.label(
                    egui::RichText::new("输入一次密码以完成解密并停用密码保护")
                        .size(theme::font_sizes::SMALL)
                        .color(theme::text_muted()),
                );
                ui.add_space(6.0);
            }
            ui.label(egui::RichText::new("访问密码").color(theme::text_soft()));
            let field_size = egui::vec2(width, 40.0);
            let (field_rect, _) = ui.allocate_exact_size(field_size, egui::Sense::hover());
            let field = egui::TextEdit::singleline(&mut gate.password)
                .id(egui::Id::new("gate_password_primary"))
                .password(!gate.show_password)
                .hint_text("输入密码")
                .vertical_align(egui::Align::Center)
                .margin(egui::Margin {
                    left: 10,
                    right: 38,
                    top: 0,
                    bottom: 0,
                });
            ui.put(field_rect, field);
            let eye_rect = egui::Rect::from_center_size(
                egui::pos2(field_rect.right() - 20.0, field_rect.center().y),
                egui::vec2(30.0, 30.0),
            );
            if ui
                .put(
                    eye_rect,
                    theme::Icon::Eye
                        .image_sized(16.0)
                        .tint(theme::text_muted())
                        .sense(egui::Sense::click()),
                )
                .on_hover_text(if gate.show_password {
                    "隐藏密码"
                } else {
                    "显示密码"
                })
                .clicked()
            {
                gate.show_password = !gate.show_password;
            }
            if ui.input(|input| input.key_pressed(egui::Key::Enter)) && !gate.password.is_empty() {
                *event = Some(GateEvent::Submit(GateOutcome::Unlock(
                    gate.password.clone(),
                )));
            }
            ui.add_space(10.0);
            let clicked = theme::accent_scope(ui, |ui| {
                ui.add_sized(
                    [width, 38.0],
                    egui::Button::new(
                        egui::RichText::new("解锁")
                            .color(theme::accent_text())
                            .strong(),
                    )
                    .fill(theme::accent())
                    .corner_radius(theme::chrome_radius(6)),
                )
            })
            .clicked();
            if clicked && !gate.password.is_empty() {
                *event = Some(GateEvent::Submit(GateOutcome::Unlock(
                    gate.password.clone(),
                )));
            }
            ui.add_space(4.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new("Enter")
                        .size(theme::font_sizes::SMALL)
                        .color(theme::text_muted()),
                );
            });
            if let Some(error) = gate.error.as_deref() {
                ui.label(egui::RichText::new(error).color(theme::danger()));
            }
        },
    );
}

fn compact_working(ui: &mut egui::Ui, gate: &mut GateState, event: &mut Option<GateEvent>) {
    let Some(working) = gate.working.as_ref() else {
        return;
    };
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(ui.available_rect_before_wrap())
            .layout(egui::Layout::top_down(egui::Align::Center)),
        |ui| {
            ui.add_space(42.0);
            ui.add(egui::Spinner::new().size(24.0));
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(&working.title)
                    .strong()
                    .color(theme::text()),
            );
            ui.label(
                egui::RichText::new("请稍候，不要关闭应用。")
                    .size(theme::font_sizes::SMALL)
                    .color(theme::text_muted()),
            );
        },
    );
    if let Ok(result) = working.rx.try_recv() {
        let working = gate.working.take().expect("working 刚确认存在");
        *event = Some(GateEvent::WorkDone {
            kind: working.kind,
            slot: working.slot,
            result,
        });
    }
}

/// Opaque abstract layout masses preserve only the notion of a workspace, never its content.
fn draw_veil_masses(ui: &egui::Ui, rect: egui::Rect) {
    backdrop::paint(ui.painter(), rect, 1.0);
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
        GateMode::Unlock => unreachable!("解锁由紧凑表单处理"),
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

    /// 锁屏样张：浅色、深色与稿纸三套主题 × 紧凑解锁、遮罩与首设形态。
    /// 手动跑：cargo test --locked gate_samples -- --ignored
    #[test]
    #[ignore = "出样张，手动跑"]
    fn gate_samples() {
        let previous = crate::models::ThemeName::ALL
            .iter()
            .copied()
            .find(|name| theme::by_name(*name).label == theme::current().label)
            .unwrap_or_default();
        for (theme_name, tag) in [
            (ThemeName::default(), "claude"),
            (ThemeName::GruvboxDark, "dark"),
            (ThemeName::Smartisan, "paper"),
            (ThemeName::MdexDark, "square"),
        ] {
            theme::set_current(theme_name);
            for (mode, mode_tag, size, veiled) in [
                (GateMode::Unlock, "unlock", egui::vec2(380.0, 250.0), false),
                (GateMode::Unlock, "veil", egui::vec2(1280.0, 820.0), true),
                (GateMode::FirstRun, "setup", egui::vec2(820.0, 680.0), false),
            ] {
                let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
                let mut gate = match mode {
                    GateMode::Unlock => GateState::unlock(),
                    GateMode::FirstRun => GateState::first_run(),
                    GateMode::Broken => GateState::broken(),
                };
                if mode == GateMode::FirstRun {
                    gate.with_decoy = true;
                }
                gate.veiled = veiled;
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
                            .show(ui, |ui| {
                                bare_titlebar(ui, mode == GateMode::Unlock && !veiled)
                            });
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

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    fn isolated(test: impl FnOnce()) {
        let dir =
            tempfile::tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tmp"))
                .unwrap();
        storage::set_test_config_dir(Some(dir.path().to_path_buf()));
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                crate::vault::clear_key_handle();
                storage::set_test_config_dir(None);
            }
        }
        let _reset = Reset;
        test();
    }

    #[test]
    fn lock_saves_latest_drafts_even_with_autosave_disabled() {
        isolated(|| {
            let mut app = crate::app::management_samples::fixture();
            app.config.auto_save = false;
            app.docs[0].generated_markdown = "# 原稿\n\n旧内容。".into();
            app.try_save_to_manuscript_library().unwrap();
            let id = app.docs[0].manuscript_id.unwrap();
            app.docs[0].generated_markdown = "# 原稿\n\n最新内容。".into();
            let mut new = crate::draft_page::DraftSession::blank(1, &app.config);
            new.generated_markdown = "# 尚未入库的新稿".into();
            app.docs.push(new);
            app.active_doc = 1;
            app.save_before_lock().unwrap();
            assert_eq!(app.active_doc, 1);
            assert!(app.docs[1].manuscript_id.is_some());
            assert!(app.docs.iter().all(|doc| !doc.is_dirty()));
            assert_eq!(
                app.manuscript_store
                    .as_mut()
                    .unwrap()
                    .get(id)
                    .unwrap()
                    .unwrap()
                    .content_markdown,
                "# 原稿\n\n最新内容。"
            );
        });
    }

    #[test]
    fn lock_save_failure_preserves_unsaved_work_and_active_document() {
        isolated(|| {
            let mut app = crate::app::management_samples::fixture();
            app.docs[0].generated_markdown = "# 仍在内存中的修改".into();
            app.docs[0].manuscript_id = Some(-1);
            app.docs
                .push(crate::draft_page::DraftSession::blank(1, &app.config));
            app.active_doc = 1;
            assert!(app.save_before_lock().is_err());
            assert_eq!(app.active_doc, 1);
            assert!(app.docs[0].is_dirty());
            assert_eq!(app.docs[0].generated_markdown, "# 仍在内存中的修改");
        });
    }

    #[test]
    fn wrong_password_keeps_runtime_veil_and_clears_password() {
        isolated(|| {
            Vault::create("valid-password", None, false).unwrap();
            let mut gate = GateState::unlock();
            gate.veiled = true;
            let mut shell = AppShell::gate(gate, None);
            shell.apply_outcome(
                &egui::Context::default(),
                GateOutcome::Unlock("wrong-password".into()),
            );
            let ShellPhase::Gate(gate) = &shell.phase else {
                panic!("wrong password unlocked workspace")
            };
            assert!(gate.veiled);
            assert!(gate.password.is_empty());
            assert_eq!(gate.error.as_deref(), Some("密码不正确"));
        });
    }

    #[test]
    fn repeated_lock_unlock_keeps_instance_queue_but_consumes_startup_import_once() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut startup = StartupBundle {
            startup_imports: Some(vec![PathBuf::from("first.gwx")]),
            instance: None,
            forwarded_requests: Some(receiver),
            macos_titlebar_metrics: None,
        };

        let (imports, _, receiver, _) = startup.take_workspace_resources();
        assert_eq!(imports, [PathBuf::from("first.gwx")]);
        startup.retain_lifecycle(None, receiver);

        sender
            .send(vec![PathBuf::from("while-locked.gwx")])
            .unwrap();
        let (imports, _, receiver, _) = startup.take_workspace_resources();
        assert!(
            imports.is_empty(),
            "startup imports must not replay after relock"
        );
        assert_eq!(
            receiver.as_ref().unwrap().try_recv().unwrap(),
            [PathBuf::from("while-locked.gwx")],
            "the single-instance inbox must remain drained while the workspace is locked"
        );
        startup.retain_lifecycle(None, receiver);

        let (imports, _, receiver, _) = startup.take_workspace_resources();
        assert!(imports.is_empty());
        assert!(
            receiver.is_some(),
            "another lock/unlock cycle retains the inbox receiver"
        );
    }
}
