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
    plaintext_acknowledged: bool,
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
    /// 磁盘上有加密数据、但锁文件缺失/为空：没有密钥就绝不进工作区。
    /// 只给恢复指引，不提供「清除锁设置」这种会丢数据的按钮。
    lock_lost: bool,
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
            plaintext_acknowledged: false,
            decoy: String::new(),
            decoy_confirm: String::new(),
            with_decoy: false,
            error: None,
            focused: false,
            show_password: false,
            veiled: false,
            cleared: false,
            lock_lost: false,
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

    /// 锁文件丢失/为空，但磁盘上已有加密数据：拒绝进入，给恢复指引。
    fn lock_lost() -> Self {
        Self {
            mode: GateMode::Broken,
            lock_lost: true,
            ..Self::first_run()
        }
    }
}

/// 磁盘上是否已有加密形态的数据（真实槽或诱饵槽）。锁文件丢失时靠它拒绝
/// 以「没有密钥」的工作区进入，避免把密封配置当损坏文件改名、再写回明文。
fn encrypted_data_on_disk() -> bool {
    crate::vault::slot_has_encrypted_data(DataSlot::Real)
        || crate::vault::slot_has_encrypted_data(DataSlot::Decoy)
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
        // 配置目录与已知子目录收紧到仅本人可访问（Unix，尽力而为）。
        crate::storage::harden_permissions();
        // 解锁前先清一次应用自己的明文缓存（旧预览、打印副本、花脸稿预览）：
        // 崩溃/强杀留下的明文不该等下次解锁才消失。
        crate::app::manuscript_ui::purge_plaintext_caches();

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
            // 没有锁文件：只有在磁盘上也没有加密数据时才走首次设置；否则绝不能
            // 以「没有密钥」的工作区进入（那会把密封配置当损坏改名并写回明文）。
            Probe::Absent => {
                if encrypted_data_on_disk() {
                    shell.phase = ShellPhase::Gate(GateState::lock_lost());
                    shell
                } else {
                    shell
                }
            }
            Probe::Skipped => {
                if encrypted_data_on_disk() {
                    shell.phase = ShellPhase::Gate(GateState::lock_lost());
                    shell
                } else {
                    shell.boot_direct(&cc.egui_ctx)
                }
            }
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
        self.boot_direct_mut(ctx);
        self
    }

    fn boot_direct_mut(&mut self, ctx: &egui::Context) {
        self.restore_workspace_window(ctx);
        self.enter_workspace(ctx, DataSlot::Real, None);
    }

    /// 备份恢复把整个配置目录换掉之后：先按新磁盘重新 probe，再决定闸门形态。
    /// 固定回「输密码」在恢复到无锁备份时会永远提示密码不正确。
    fn gate_after_restore(&mut self, ctx: &egui::Context) {
        match Vault::probe() {
            Probe::Locked => {
                self.phase = ShellPhase::Gate(GateState::unlock());
            }
            Probe::Broken(_) => {
                self.phase = ShellPhase::Gate(GateState::broken());
            }
            Probe::Skipped => {
                if encrypted_data_on_disk() {
                    self.phase = ShellPhase::Gate(GateState::lock_lost());
                } else {
                    self.boot_direct_mut(ctx);
                }
            }
            Probe::Absent => {
                if encrypted_data_on_disk() {
                    self.phase = ShellPhase::Gate(GateState::lock_lost());
                } else {
                    // 恢复的是从未启用锁的备份：直接进工作区。
                    self.boot_direct_mut(ctx);
                }
            }
        }
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
        // 进入工作区：槽位与密钥句柄都明确，恢复配置目录写入。
        storage::set_writes_blocked(false);
        // 上次崩溃/锁屏时没能落终态的 Pi 轮次仍停在 Running：在持有密钥、
        // 尚未开始新任务的窗口里纠正一次（尽力而为，失败不阻断进入工作区）。
        let reconciled = crate::pi_agent::recovery::reconcile_interrupted_runs();
        let Some(startup) = self.startup.as_mut() else {
            return;
        };
        let (imports, instance, requests, titlebar_metrics) = startup.take_workspace_resources();
        let mut app = GongwenApp::new_unlocked(ctx, imports, instance, requests, titlebar_metrics);
        if let Some(notice) = notice {
            app.status = notice;
        }
        match reconciled {
            Ok(count) if count > 0 => {
                app.status =
                    format!("已把 {count} 个中断的智能写作轮次标记为已中断，可重新接着写。");
            }
            Err(error) => {
                // 不阻断进入工作区，但要让人知道纠正失败，避免记录一直停在 Running。
                app.status = format!("纠正中断的智能写作轮次失败：{error}");
            }
            _ => {}
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
        // 先切槽位，再用**显式槽位**创建句柄：句柄绑定的槽位必须与磁盘路径一致，
        // 不能依赖「先设句柄、后切槽位」的调用顺序（诱饵密码会拿到 Real 句柄）。
        crate::resources::close();
        crate::storage::set_current_slot(slot);
        crate::vault::set_key_handle_for_slot(dek, kind == WorkKind::Disable, slot);
        // 迁移/停用任务要在槽位与密钥都明确之后写盘，放行配置目录写入。
        crate::storage::set_writes_blocked(false);
        let (tx, rx) = std::sync::mpsc::channel();
        // 搬运线程要看到同一份配置目录与同一把密钥。生产构建里这两样都是
        // 进程级的，外层那次设置就够；测试构建里它们是线程局部的（并行测试
        // 互不干扰），后台线程看不到——所以显式带过去，两种构建都成立。
        #[cfg(test)]
        let worker_config_dir = crate::storage::config_dir().ok();
        std::thread::spawn(move || {
            #[cfg(test)]
            if let Some(dir) = worker_config_dir {
                crate::storage::set_test_config_dir_in_current_thread(dir);
            }
            crate::vault::set_key_handle_for_slot(dek, kind == WorkKind::Disable, slot);
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
                    self.enter_workspace(
                        ctx,
                        slot,
                        Some("密码保护已启用；现有数据已加密到磁盘。".into()),
                    );
                }
                WorkKind::Disable => {
                    // 先确认当前槽位真的没有密文残留，再删锁封套。少解一类文件
                    // （apis/配置副本等）时，这里必须失败并保留封套，不能让数据
                    // 变成「没有 DEK 的密文」。
                    if crate::vault::slot_has_encrypted_data(slot) {
                        crate::vault::clear_key_handle();
                        crate::storage::set_writes_blocked(true);
                        let mut gate = GateState::unlock();
                        gate.veiled = !self.compact_startup;
                        gate.error = Some(
                            "停用未完成：仍有加密数据没有解开，已保留密码保护设置。请重试或从备份恢复。"
                                .into(),
                        );
                        self.phase = ShellPhase::Gate(gate);
                        return;
                    }
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
                // 迁移/停用失败后仍是锁定态：先停写再清密钥，避免晚到线程在
                // 「无密钥」状态把明文写回配置目录。
                crate::storage::set_writes_blocked(true);
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
                            // 停用标记必须由「刚解锁的 DEK」认证：伪造的明文标记
                            // 一律无效。停用只对真实槽生效——用诱饵密码不能执行
                            // 真实槽的解密与删锁，否则可否认性与数据都保不住。
                            let disable = unlocked.slot == DataSlot::Real
                                && crate::vault::disable_pending(&unlocked.dek);
                            // 没有要搬的明文数据（通常：上次解锁已迁完）就直接
                            // 进工作区，不再显示「正在加密」卡片——那张卡只在
                            // 真的有活干时出现。两套密码各自只看自己槽位的文件，
                            // 交替解锁互不触发对方的数据搬运。
                            if !disable && !crate::vault::slot_needs_encryption(unlocked.slot) {
                                crate::vault::set_key_handle_for_slot(
                                    *unlocked.dek,
                                    true,
                                    unlocked.slot,
                                );
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
                    Ok((_, None)) => {
                        gate = GateState::first_run();
                        gate.error = Some("锁设置保存失败，请重试".into());
                    }
                    // 密码不合要求时把**具体原因**告诉用户（哪一条不满足），
                    // 否则用户只会看到一句“保存失败”，不知道该改哪里。
                    Err(error) => {
                        gate = GateState::first_run();
                        gate.error = Some(format!("{error:#}"));
                    }
                }
            }
            GateOutcome::SkipSetup => {
                // 防御纵深：磁盘上已有加密数据时，「暂不设置」不能成为绕过钥匙的入口。
                if encrypted_data_on_disk() {
                    self.phase = ShellPhase::Gate(GateState::lock_lost());
                    return;
                }
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

    /// 测试用入口：模拟外壳下一帧接到“启用后立刻加密”的请求。
    #[cfg(test)]
    fn process_enable_encrypt_for_test(&mut self, ctx: &egui::Context) {
        let ShellPhase::Ready(app) = &mut self.phase else {
            return;
        };
        assert!(
            app.take_enable_encrypt_requested(),
            "启用密码后应发出立刻加密的请求"
        );
        app.save_before_lock().expect("启用前应能存盘");
        self.begin_enable_encrypt(ctx);
    }

    /// 刚在设置页启用了密码保护：把磁盘上的明文数据**立即**迁成密文。
    ///
    /// 顺序要紧：迁移要**换掉** manuscripts.db 文件（sqlcipher 重新导出），
    /// 工作区里的旧连接必须先关掉，否则它们会继续往已被替换的旧文件里写。
    /// 所以先丢掉工作区（与锁屏同一套丢弃逻辑），再由后台线程迁移，
    /// 最后重建工作区（此时库已是密文，`activate_db_encryption` 生效）。
    ///
    /// 不等“下次解锁”：从启用到下次解锁之间，磁盘上的正文一直是明文，
    /// 与「拷走配置目录就能翻看正文」的威胁模型直接冲突。
    fn begin_enable_encrypt(&mut self, ctx: &egui::Context) {
        let Some(handle) = crate::vault::key_handle() else {
            // 没有密钥句柄（异常态）：退回锁屏，用户重新解锁后再走迁移。
            self.lock_now(ctx);
            return;
        };
        let dek = *handle.dek();
        let slot = handle.slot();
        self.lock_now(ctx);
        self.begin_work(WorkKind::Encrypt, slot, dek, "正在把现有数据加密到磁盘…");
    }

    /// 退回锁屏：丢掉整个工作区（数据库连接、内存数据随 Drop 释放），
    /// 数据密钥句柄一并清掉——锁定状态内存里不留密钥。
    fn lock_now(&mut self, ctx: &egui::Context) {
        // 停写在前：被丢弃的工作区里晚到的线程不能以「无密钥」状态把明文写回
        // 配置目录；槽位复位也在 drop 之前，晚到线程至少拿到真实槽路径。
        crate::storage::set_writes_blocked(true);
        // 资源库连接随工作区一起释放：换槽、重锁与备份换入之后都必须重新打开。
        crate::resources::close();
        storage::set_current_slot(DataSlot::Real);
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
        // 锁定即丢弃工作区：顺手清掉应用自己的明文缓存与花脸稿预览副本。
        crate::app::manuscript_ui::purge_plaintext_caches();
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
            /// 备份恢复后：目录已整体替换，必须先重新读磁盘再决定闸门形态。
            RestoreLock,
            WorkDone {
                kind: WorkKind,
                slot: DataSlot,
                result: Result<(), String>,
            },
            Lock,
            /// 刚启用密码保护：丢弃工作区并把磁盘上的明文数据迁成密文。
            EnableEncrypt,
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
                app.guard_clipboard_input(&ctx);
                app.update_workspace(ui, frame);
                app.guard_clipboard_output(&ctx);
                if app.take_force_relock() {
                    // 备份恢复：目录已整体替换，保存会拿旧密钥覆盖新数据；
                    // 恢复后还需要按新磁盘重新 probe（可能变成无锁/跳过/损坏）。
                    paint_lock_veil(&ctx, 1.0);
                    Some(After::RestoreLock)
                } else if app.take_enable_encrypt_requested() {
                    // 启用密码保护：先把工作区存盘并丢弃，再把明文迁成密文。
                    match app.save_before_lock() {
                        Ok(()) => {
                            paint_lock_veil(&ctx, 1.0);
                            Some(After::EnableEncrypt)
                        }
                        Err(error) => {
                            // 还有任务在跑就下帧再试（存盘要在任务结束后做）；
                            // 真失败（写不动）不重试，免得每帧刷一次状态栏。
                            // 两种情况下磁盘上的数据都还没加密，下次解锁时闸门
                            // 会补上迁移（见 apply_outcome 的 slot_needs_encryption）。
                            if app.any_busy() {
                                app.request_enable_encrypt();
                                app.status = "正在等待任务结束后加密现有数据…".into();
                            } else {
                                app.status = format!(
                                    "启用密码保护前保存失败，现有数据尚未加密（下次解锁时会自动补上）：{error:#}"
                                );
                            }
                            None
                        }
                    }
                } else if app.take_lock_requested() {
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
            Some(After::RestoreLock) => {
                self.lock_now(&ctx);
                self.gate_after_restore(&ctx);
            }
            Some(After::EnableEncrypt) => self.begin_enable_encrypt(&ctx),
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
    let height = if crate::vault::disable_marker_present() {
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
            if crate::vault::disable_marker_present() {
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
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(crate::password_policy::LOCK_HINT)
                    .size(theme::font_sizes::SMALL)
                    .color(theme::text_muted()),
            );
            ui.add_space(12.0);
            password_field(
                ui,
                "gate_password_primary",
                "设置密码（长度大于 10 个字符）",
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
            ui.label(egui::RichText::new("不启用应用锁时，稿件、人员资料、模型密钥及本机配置将以明文保存。获得这些文件的人可以直接读取；以后启用加密也不能保证擦除历史副本。").color(theme::danger()));
            ui.checkbox(
                &mut gate.plaintext_acknowledged,
                "我已了解明文保存风险，仍选择不加密",
            );
            if ui
                .add_enabled(
                    gate.plaintext_acknowledged,
                    egui::Button::new("暂不设置，直接进入").frame(false),
                )
                .on_hover_text("本次跳过；之后随时可以在「设置 → 应用锁」里启用。")
                .clicked()
            {
                outcome = Some(GateOutcome::SkipSetup);
            }
        }
        GateMode::Broken => {
            if gate.lock_lost {
                ui.label(
                    egui::RichText::new(
                        "检测到本机已有加密数据，但锁文件不存在（可能被删除、损坏或随目录迁移丢失）。",
                    )
                    .color(theme::danger()),
                );
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new(
                        "没有原来的应用密码就无法打开这些数据。请不要删除数据文件，也不要重复「暂不设置」。",
                    )
                    .color(theme::text_soft()),
                );
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(
                        "恢复办法：从备份里找到同一代的 vault.dat 放回配置目录，或用全量备份恢复本机数据。",
                    )
                    .color(theme::text_soft()),
                );
            } else if gate.cleared {
                ui.label(
                    egui::RichText::new(
                        "锁设置已清除。若本机原有加密数据，它们将无法再打开；请重启应用，从备份恢复锁文件或数据。",
                    )
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
                    .button("清除锁设置（之后无法打开原有加密数据）")
                    .on_hover_text(
                        "只删除锁文件，不删除任何数据文件；但数据密钥随锁文件一起丢失，原有加密数据将永久无法打开。仅在确认没有加密数据、或已从备份恢复时才这样做。",
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
        // 临时目录放在仓库的 `tmp/` 下（不入库，干净检出的 CI 上不存在），
        // 用前先建出来；`tempdir_in` 不会自建父目录。
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tmp");
        std::fs::create_dir_all(&root).expect("应能建立测试临时目录");
        let dir = tempfile::tempdir_in(&root).unwrap();
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
            Vault::create("Test#Valid2026Lock", None, false).unwrap();
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

    /// 启用密码保护后，磁盘上的稿件库必须**当场**变成密文：
    /// 推迟到下次解锁等于让正文在磁盘上多敞开一段时间。
    #[test]
    fn enabling_password_encrypts_the_database_right_away() {
        isolated(|| {
            // 先造一份明文库与明文配置，模拟“还没启用锁”的状态。
            let db = storage::manuscript_db_path().unwrap();
            {
                let conn = rusqlite::Connection::open(&db).unwrap();
                conn.execute_batch("CREATE TABLE t(body TEXT);").unwrap();
                conn.execute("INSERT INTO t(body) VALUES ('机密正文')", [])
                    .unwrap();
            }
            assert!(
                !crate::vault::slot_has_encrypted_data(DataSlot::Real),
                "前置条件：此时磁盘上还没有密文"
            );

            // 走设置页的“启用密码保护”。
            let mut app = crate::app::management_samples::fixture();
            app.vault_enable_with_for_test("Test#Enable2026Lock", None);

            // 外壳下一帧接到请求 → 丢弃工作区 → 后台迁移 → 重建工作区。
            let ctx = egui::Context::default();
            let mut shell = AppShell::gate(GateState::unlock(), None);
            shell.phase = ShellPhase::Ready(Box::new(app));
            shell.process_enable_encrypt_for_test(&ctx);

            // 迁移在后台线程里跑，等它完成。
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                if let ShellPhase::Gate(gate) = &shell.phase
                    && let Some(working) = &gate.working
                    && let Ok(result) = working.rx.try_recv()
                {
                    result.expect("加密迁移应当成功");
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "加密迁移应在超时前完成"
                );
                std::thread::sleep(std::time::Duration::from_millis(20));
            }

            // 磁盘上的库已经是密文：不再能当明文 SQLite 打开。
            let head = std::fs::read(&db).unwrap();
            assert!(
                !head.starts_with(b"SQLite format 3"),
                "启用密码后稿件库不该再是明文 SQLite"
            );
            assert!(
                crate::vault::slot_has_encrypted_data(DataSlot::Real),
                "启用密码后磁盘上应能检出密文数据"
            );
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
