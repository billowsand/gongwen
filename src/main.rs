#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
// 安全门禁（见 docs/security-hardening.md）：
// - 全 crate 默认禁止 `unsafe`（`unsafe_code`）；只有确实做 FFI 的模块在声明处
//   单独 `#[allow(unsafe_code)]`，新加的模块自动受门禁约束；
// - `unsafe fn` 内部必须显式写 `unsafe` 块（`unsafe_op_in_unsafe_fn`），且必须写
//   「# Safety」契约（`clippy::missing_safety_doc`）；
// - 每个 `unsafe` 块上面都要有 SAFETY 注释说明为什么这么写是安全的
//   （`clippy::undocumented_unsafe_blocks`）。
#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(clippy::missing_safety_doc)]
#![warn(clippy::undocumented_unsafe_blocks)]

#[cfg(feature = "ai-workbench")]
mod agent;
#[cfg(feature = "ai-workbench")]
mod ai_guard;
#[cfg(feature = "ai-workbench")]
mod ai_panel;
#[cfg(feature = "ai-workbench")]
mod ai_services;
mod app;
mod content_hash;
mod crash_log;
mod data_backup;
#[cfg(test)]
mod dependency_bench;
mod diff;
mod diff_view;
mod doc_import;
mod document_reference;
mod draft_page;
mod element_fields;
mod export;
// FFI 模块：Win32 / AppKit 剪贴板调用，每个 unsafe 块都带 SAFETY 注释。
#[allow(unsafe_code)]
mod file_clipboard;
mod file_limits;
mod font_cmap;
// 图形环境兜底（Linux）：探测 GL 能力、必要时降级软件渲染、迟到的 GLX 错误不再崩。
// 模块里有裸 FFI，按仓库约定在声明处单独放行 unsafe。
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod gl_boot;
mod highlight;
mod images;
mod ime;
#[cfg(feature = "ai-workbench")]
mod knowledge;
#[cfg(feature = "ai-workbench")]
mod knowledge_ui;
mod last_char_orphan;
mod lexicon;
mod lexicon_ui;
mod linux_desktop;
#[cfg(feature = "ai-workbench")]
mod lmstudio;
// FFI 模块：AppKit 窗口调用，每个 unsafe 块都带 SAFETY 注释。
#[allow(unsafe_code)]
mod macos_window;
mod manuscript;
mod manuscript_io;
mod mermaid;
mod metafile;
mod metrics;
mod modal;
mod models;
mod net;
mod orphan_probe;
mod package_file;
mod password_policy;
mod pdf_text;
mod pdf_viewer;
#[cfg(feature = "ai-workbench")]
mod pi_agent;
mod portable_runtime;
mod preview;
// 打印：Win32 那一半是 FFI，在 `print` 内部的 `win` 模块声明处单独放行 unsafe。
mod pdf_image;
mod print;
#[cfg(feature = "ai-workbench")]
mod prompt;
mod proofread;
mod proofread_rules;
mod proofread_xlsx;
#[cfg(feature = "ai-workbench")]
mod qa;
#[cfg(feature = "ai-workbench")]
mod rag;
#[cfg(feature = "ai-workbench")]
mod rag_client;
mod redline;
mod resources;
mod retired_help;
/// 文字复核回归集。纯测试资产：样例文件与解析器只在测试构建里存在，
/// 不进发行版二进制。
#[cfg(test)]
#[cfg(feature = "ai-workbench")]
mod revise_cases;
#[cfg(feature = "ai-workbench")]
mod revise_model;
mod revision;
mod rich_paste;
mod samples;
mod sealed_json;
mod single_instance;
#[cfg(feature = "ai-workbench")]
mod skill_pack;
mod storage;
mod system_fonts;
#[cfg(feature = "task-assistant")]
mod task_assistant;
#[cfg(feature = "task-assistant")]
mod task_ui;
#[cfg(test)]
mod test_support;
mod text_file;
mod theme;
mod typst_engine;
#[cfg(test)]
mod ui_snapshot;
mod units;
mod update_check;
mod validator;
mod vault;
mod version;
mod version_link;
mod version_pair_view;
mod visual_diff;
// 维基知识库存储层（一期切片 1 先行落地；界面/工具/智能体切片接入后移除 allow）。
mod vocabulary_xlsx;
#[allow(dead_code)]
#[cfg(feature = "ai-workbench")]
mod wiki;
#[cfg(feature = "ai-workbench")]
mod wiki_ui;

use std::cell::Cell;
use std::rc::Rc;

use app::gate::AppShell;

fn main() -> eframe::Result {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--build-edition")
    {
        println!(
            "{}",
            if cfg!(feature = "ai-workbench") {
                "ai"
            } else {
                "no-ai"
            }
        );
        return Ok(());
    }
    // 图形能力探测子进程：只做探测、打印一行结果就退出，主进程照常建窗口。
    // 放在最前面，免得子进程也去抢单实例锁、建窗口。
    #[cfg(target_os = "linux")]
    if let Some(code) = gl_boot::probe_child_entry() {
        std::process::exit(code);
    }
    // 最先装：之后读配置、装字体、建窗口任何一步 panic 都要留下日志。
    crash_log::install();
    // 双击 .gwx 时系统把文件路径作为参数传进来。
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let new_window = args
        .iter()
        .any(|arg| arg == single_instance::NEW_WINDOW_FLAG);
    let startup_imports =
        package_file::startup_paths(args, &std::env::current_dir().unwrap_or_default());
    // 已有窗口开着就把稿件包转交给它，本进程不建窗口。
    let instance = if new_window {
        None
    } else {
        match single_instance::start(&startup_imports) {
            Ok(single_instance::Startup::Forwarded) => return Ok(()),
            Ok(single_instance::Startup::Primary(instance)) => Some(instance),
            // 锁或收件目录用不了（只读盘、网络盘不支持文件锁）：照旧开一个独立窗口。
            Err(_) => None,
        }
    };
    let app_icon = theme::app_icon();
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 820.0])
        // 760 宽能在常见 1920 屏幕的二等分平铺与高 DPI 逻辑尺寸下正常容纳；
        // 功能区本身支持横向滚动，左侧表单最窄 332，中央区仍保留约 400。
        .with_min_inner_size([760.0, 600.0])
        .with_title(version::APP_TITLE)
        .with_app_id(version::APP_ID)
        .with_icon(app_icon);

    // macOS 保留真正的 AppKit 标题栏和标准红黄绿按钮，仅把标题栏设为透明，
    // 让 egui 顶栏内容延伸到其下方；Windows/Linux 继续使用无边框自绘方案。
    #[cfg(target_os = "macos")]
    let viewport = viewport
        .with_decorations(true)
        .with_fullsize_content_view(true)
        .with_title_shown(false)
        .with_titlebar_shown(false)
        .with_titlebar_buttons_shown(true);
    #[cfg(not(target_os = "macos"))]
    let viewport = viewport.with_decorations(false);

    let deferred_window_state = Rc::new(Cell::new(DeferredWindowState::default()));
    let options = eframe::NativeOptions {
        viewport,
        window_builder: defer_window_state_hook(Rc::clone(&deferred_window_state)),
        // 外壳自己记工作区尺寸；不能把启动密码小窗写成下次工作区尺寸。
        persist_window: false,
        ..Default::default()
    };

    // 建窗口之前先探一次图形环境：麒麟 V10 这类机器上 OpenGL 上下文可能建不起来，
    // 探测到确实不行就改用软件渲染；同时装上 X11 错误钩子，迟到的 GLX 错误
    // 不再把程序打成核心转储。
    #[cfg(target_os = "linux")]
    gl_boot::prepare();

    let result = eframe::run_native(
        version::APP_TITLE,
        options,
        Box::new(move |cc| {
            #[cfg(target_os = "macos")]
            theme::apply_macos_dock_icon();
            // 这时窗口还隐藏着；命令随首帧输出，在首帧画完、窗口显示之后才执行。
            let state = deferred_window_state.get();
            let locked = matches!(vault::Vault::probe(), vault::Probe::Locked);
            let app = AppShell::new(cc, startup_imports, instance, state);
            if !locked && state.fullscreen {
                cc.egui_ctx
                    .send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
            } else if !locked && state.maximized {
                cc.egui_ctx
                    .send_viewport_cmd(egui::ViewportCommand::Maximized(true));
            }
            Ok(Box::new(app))
        }),
    );
    if let Err(error) = &result {
        crash_log::record_startup_error(error);
        // 图形环境起不来时，把能救命的办法说清楚，别只丢一个日志路径。
        #[cfg(target_os = "linux")]
        {
            let hint = gl_boot::startup_failure_hint();
            gl_boot::log_line(&format!("启动失败：{error}\n{hint}"));
            eprintln!("启动失败：{error}\n{hint}");
        }
    }
    result
}

/// eframe 从上次会话恢复、但要推迟到首帧之后才应用的窗口状态。
#[derive(Clone, Copy, Default)]
pub(crate) struct DeferredWindowState {
    pub(crate) maximized: bool,
    pub(crate) fullscreen: bool,
    pub(crate) inner_size: Option<egui::Vec2>,
    pub(crate) position: Option<egui::Pos2>,
}

/// eframe 先把窗口建成隐藏的，画完首帧再显示，避免启动时闪出空白窗口；
/// 但 Windows 上 winit 给隐藏窗口「最大化 / 全屏」会直接 `ShowWindow`，把窗口
/// 提前亮出来。上次退出时是最大化的话，启动时就会先闪几次还没画内容的空窗口，
/// 隔一两秒界面才出来。这里把恢复出来的这两个状态摘掉，记下来交给 App 创建时补发。
/// 恢复的尺寸本来就是最大化时的尺寸，补发时窗口几乎不跳。
fn defer_window_state_hook(
    state: Rc<Cell<DeferredWindowState>>,
) -> Option<eframe::WindowBuilderHook> {
    Some(Box::new(move |builder: egui::ViewportBuilder| {
        state.set(DeferredWindowState {
            maximized: builder.maximized == Some(true),
            fullscreen: builder.fullscreen == Some(true),
            inner_size: builder.inner_size,
            position: builder.position,
        });
        if matches!(vault::Vault::probe(), vault::Probe::Locked) {
            // 创建原生窗口之前就决定小窗尺寸，避免先亮出完整窗口再缩小。
            builder
                .with_maximized(false)
                .with_fullscreen(false)
                .with_min_inner_size([340.0, 220.0])
                .with_inner_size([380.0, 250.0])
        } else if cfg!(target_os = "windows") {
            builder.with_maximized(false).with_fullscreen(false)
        } else {
            builder
        }
    }))
}
