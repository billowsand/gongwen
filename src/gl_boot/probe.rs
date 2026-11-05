//! 图形能力探测。
//!
//! 探测本身跑在子进程里（见 `run_child`）：里面全是裸 FFI，万一段错误，
//! 也只是子进程退出，父进程照常往下走。
//!
//! 探测的请求跟 eframe / glutin 实际发的请求一一对应（见 `eframe` 的
//! `glow_integration.rs` 与 `glutin` 的 `api/glx/context.rs`）：
//!
//! 1. 默认请求 OpenGL 3.3 **核心配置**（一定带配置文件位）；
//! 2. 失败后退回 OpenGL ES 2.0（eframe 的兜底属性）；
//! 3. 客户端没有 `GLX_ARB_create_context` 时，glutin 走旧路
//!    （`glXCreateNewContext`），照样能起，只是版本老。
//!
//! 只有这三条都走不通，才算「这台机器的图形环境真的坏了」。

use std::ffi::{c_int, c_uchar};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use super::{PROBE_ENV, Report, Verdict, decode, encode, log_line, verdict_of};

/// GLX 常量（见 glxproto.h / GLX 1.4 规范）。
const GLX_RENDER_TYPE: c_int = 0x8011;
const GLX_RGBA_BIT: c_int = 0x0000_0001;
const GLX_DRAWABLE_TYPE: c_int = 0x8010;
const GLX_WINDOW_BIT: c_int = 0x0000_0001;
const GLX_RGBA_TYPE: c_int = 0x8014;
const GLX_PBUFFER_WIDTH: c_int = 0x8041;
const GLX_PBUFFER_HEIGHT: c_int = 0x8040;
const GLX_CONTEXT_MAJOR_VERSION_ARB: c_int = 0x2091;
const GLX_CONTEXT_MINOR_VERSION_ARB: c_int = 0x2092;
const GLX_CONTEXT_PROFILE_MASK_ARB: c_int = 0x9126;
const GLX_CONTEXT_CORE_PROFILE_BIT_ARB: c_int = 0x0000_0001;
const GLX_CONTEXT_ES2_PROFILE_BIT_ARB: c_int = 0x0000_0004;
/// `glXGetClientString` 的名字常量。
const GLX_CLIENT_EXTENSIONS: c_int = 1;
/// `glGetString` 的名字常量。
const GL_VERSION: c_int = 0x1F02;
const GL_RENDERER: c_int = 0x1F01;

/// 探测子进程最多跑多久：X 服务器卡住时不能把启动也拖死。
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// 探测子进程的入口：环境变量置上时只做探测、打印一行结果就退出。
///
/// 返回 `Some(退出码)` 表示「我是探测子进程」，父进程该等着拿这个退出码。
pub fn child_entry() -> Option<i32> {
    std::env::var_os(PROBE_ENV)?;
    let report = probe_here();
    println!("{}", encode(&report));
    Some(0)
}

/// 拉起子进程做一次探测。
pub fn run_child() -> Report {
    let Ok(exe) = std::env::current_exe() else {
        return Report::new(Verdict::Unknown, "找不到程序路径");
    };
    let mut child = match Command::new(exe)
        .env(PROBE_ENV, "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return Report::new(Verdict::Unknown, format!("起不来探测子进程：{error}"));
        }
    };
    // 单独一个线程读 stdout：管道容量有限，父进程不读的话子进程写日志会被卡住。
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        use std::io::Read as _;
        let mut text = String::new();
        if let Some(mut stdout) = stdout {
            let _ = stdout.read_to_string(&mut text);
        }
        text
    });

    let deadline = std::time::Instant::now() + PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(error) => {
                log_line(&format!("探测子进程状态读取失败：{error}"));
                break None;
            }
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };

    let text = reader.join().unwrap_or_default();
    match status {
        Some(status) if status.success() => {
            decode(&text).unwrap_or_else(|| Report::new(Verdict::Unknown, "探测结果读不出来"))
        }
        Some(status) => Report::new(Verdict::Unknown, format!("探测子进程异常退出：{status}")),
        None => Report::new(Verdict::Unknown, "探测超时"),
    }
}

/// 探测过程中收到的第一条 X 错误码（0 = 没收到）。
static FIRST_ERROR: AtomicU32 = AtomicU32::new(0);

/// 探测期间的 X 错误处理器：只记下来，不打印也不退出。
///
/// Xlib 默认的错误处理器会往 stderr 打印并直接 `exit(1)`，探测里不能那样。
unsafe extern "C" fn record_x_error(
    _display: *mut x11_dl::xlib::Display,
    event: *mut x11_dl::xlib::XErrorEvent,
) -> c_int {
    // SAFETY: Xlib 保证回调期间 `event` 指向一个有效的错误事件。
    let code = unsafe { (*event).error_code } as u32;
    let _ = FIRST_ERROR.compare_exchange(0, code, Ordering::Relaxed, Ordering::Relaxed);
    0
}

/// 真正做探测的代码，只在子进程里跑。
fn probe_here() -> Report {
    let Ok(xlib) = x11_dl::xlib::Xlib::open() else {
        return Report::new(Verdict::Unknown, "加载不了 libX11");
    };
    let Ok(glx) = x11_dl::glx::Glx::open() else {
        return Report::new(Verdict::Unknown, "加载不了 libGL");
    };
    // SAFETY: 传空指针表示用 DISPLAY 环境变量里的显示。
    let display = unsafe { (xlib.XOpenDisplay)(std::ptr::null()) };
    if display.is_null() {
        return Report::new(Verdict::Unknown, "连不上 X 服务器");
    }
    // SAFETY: 处理器的签名与 Xlib 一致，只做一次原子写，不碰别的状态。
    unsafe { (xlib.XSetErrorHandler)(Some(record_x_error)) };

    let report = probe_glx(&xlib, &glx, display);
    // SAFETY: display 是刚打开的连接。
    unsafe { (xlib.XCloseDisplay)(display) };
    report
}

/// 建上下文、绑到离屏目标上，读出版本。
fn probe_glx(
    xlib: &x11_dl::xlib::Xlib,
    glx: &x11_dl::glx::Glx,
    display: *mut x11_dl::xlib::Display,
) -> Report {
    let mut major = 0;
    let mut first_event = 0;
    let mut first_error = 0;
    // SAFETY: display 有效；名字是以 NUL 结尾的字符串常量。
    let has_glx = unsafe {
        (xlib.XQueryExtension)(
            display,
            c"GLX".as_ptr(),
            &mut major,
            &mut first_event,
            &mut first_error,
        )
    } != 0;
    if !has_glx {
        return Report::new(Verdict::Unknown, "X 服务器没有 GLX 扩展");
    }

    let mut version_major = 0;
    let mut version_minor = 0;
    // SAFETY: display 有效，两个输出指针有效。
    if unsafe { (glx.glXQueryVersion)(display, &mut version_major, &mut version_minor) } == 0 {
        return Report::new(Verdict::Broken, "GLX 版本查询失败").with_glx(first_error);
    }

    // SAFETY: display 有效。
    let screen = unsafe { (xlib.XDefaultScreen)(display) };
    // 选一个能渲染的 fbconfig：只要 RGBA + 窗口型，具体位深交给驱动。
    let attributes = [
        GLX_RENDER_TYPE,
        GLX_RGBA_BIT,
        GLX_DRAWABLE_TYPE,
        GLX_WINDOW_BIT,
        0,
    ];
    let mut count = 0;
    // SAFETY: attributes 以 0 结尾；count 是有效的输出指针。
    let configs =
        unsafe { (glx.glXChooseFBConfig)(display, screen, attributes.as_ptr(), &mut count) };
    if configs.is_null() || count <= 0 {
        return Report::new(Verdict::Broken, "选不到可用的 GLX 配置").with_glx(first_error);
    }
    // SAFETY: configs 指向 count 个 GLXFBConfig，上面确认过 count > 0。
    let config = unsafe { *configs };

    // SAFETY: configs 是 Xlib 分配的，用完要还回去。
    unsafe { (xlib.XFree)(configs.cast()) };

    let extensions = client_extensions(glx, display);
    let mut report = Report::new(Verdict::Unknown, "还没试").with_glx(first_error);
    let mut attempts: Vec<(&str, Vec<c_int>)> = Vec::new();
    let legacy = !extensions.contains("GLX_ARB_create_context");
    if !legacy {
        // 与 glutin 一致：默认请求 3.3 核心配置，配置文件位一定要带上。
        attempts.push((
            "OpenGL 3.3 核心配置",
            vec![
                GLX_CONTEXT_PROFILE_MASK_ARB,
                GLX_CONTEXT_CORE_PROFILE_BIT_ARB,
                GLX_CONTEXT_MAJOR_VERSION_ARB,
                3,
                GLX_CONTEXT_MINOR_VERSION_ARB,
                3,
                0,
            ],
        ));
        // eframe 的兜底：OpenGL ES 2.0。
        if extensions.contains("GLX_ARB_create_context_es2_profile") {
            attempts.push((
                "OpenGL ES 2.0",
                vec![
                    GLX_CONTEXT_PROFILE_MASK_ARB,
                    GLX_CONTEXT_ES2_PROFILE_BIT_ARB,
                    GLX_CONTEXT_MAJOR_VERSION_ARB,
                    2,
                    GLX_CONTEXT_MINOR_VERSION_ARB,
                    0,
                    0,
                ],
            ));
        }
    }

    for (name, attributes) in attempts {
        FIRST_ERROR.store(0, Ordering::Relaxed);
        let Some(context) = create_context(glx, display, config, &attributes) else {
            continue;
        };
        match bind_and_read(glx, xlib, display, config, context) {
            Ok((version, renderer)) => {
                report.verdict = verdict_of(&version);
                report.version = version;
                report.renderer = renderer;
                report.detail = format!("{name} 可用");
                return report;
            }
            Err(BindError::Unsupported(reason)) => {
                // 这套 GLX 连离屏目标都没有，探不出结论，不当成「图形环境坏了」。
                return Report::new(Verdict::Unknown, reason).with_glx(first_error);
            }
            Err(BindError::Failed(reason)) => {
                report.detail = format!("{name}：{reason}");
            }
        }
    }

    if legacy {
        // 客户端没有 ARB 扩展时 glutin 走旧路：glXCreateNewContext。
        FIRST_ERROR.store(0, Ordering::Relaxed);
        let Some(context) = create_legacy_context(glx, display, config) else {
            return Report::new(Verdict::Broken, "旧式上下文也建不出来").with_glx(first_error);
        };
        match bind_and_read(glx, xlib, display, config, context) {
            Ok((version, renderer)) => {
                report.verdict = verdict_of(&version);
                report.version = version;
                report.renderer = renderer;
                report.detail = "旧式上下文可用".to_owned();
                return report;
            }
            Err(BindError::Unsupported(reason)) => {
                return Report::new(Verdict::Unknown, reason).with_glx(first_error);
            }
            Err(BindError::Failed(reason)) => {
                report.detail = format!("旧式上下文：{reason}");
            }
        }
    }

    // 一条上下文都没建起来：eframe 也建不起来。
    let error = FIRST_ERROR.load(Ordering::Relaxed);
    let detail = if error > 0 {
        format!("建不出 OpenGL 上下文（X 错误码 {error}）")
    } else {
        "建不出 OpenGL 上下文".to_owned()
    };
    Report::new(Verdict::Broken, detail).with_glx(first_error)
}

/// 绑上下文时的两种失败：探不出结论的，和确实建不成的。
enum BindError {
    /// 这套 GLX 不支持离屏目标之类，探不出结论。
    Unsupported(String),
    /// 上下文确实建不成。
    Failed(String),
}

/// 离屏目标的两种做法。
enum Drawable {
    /// GLX 1.3 的 pbuffer。
    Pbuffer(x11_dl::glx::GLXPbuffer),
    /// 一个 1×1 的 X 窗口，跟 eframe 一样走窗口上下文。
    Window(x11_dl::xlib::XID),
}

/// 把上下文绑到离屏目标上，读出版本与渲染器名。
fn bind_and_read(
    glx: &x11_dl::glx::Glx,
    xlib: &x11_dl::xlib::Xlib,
    display: *mut x11_dl::xlib::Display,
    config: x11_dl::glx::GLXFBConfig,
    context: x11_dl::glx::GLXContext,
) -> Result<(String, String), BindError> {
    let mut created = false;
    // 先试 pbuffer（GLX 1.3），再试 1×1 的窗口。
    let pbuffer = create_pbuffer(glx, display, config);
    if pbuffer != 0 {
        created = true;
        if let Some(result) = bind_drawable(glx, xlib, display, context, Drawable::Pbuffer(pbuffer))
        {
            return Ok(result);
        }
    }
    if let Some(window) = create_window(xlib, glx, display, config) {
        created = true;
        if let Some(result) = bind_drawable(glx, xlib, display, context, Drawable::Window(window)) {
            return Ok(result);
        }
    }
    if !created {
        // SAFETY: display 有效。
        unsafe { (glx.glXDestroyContext)(display, context) };
        return Err(BindError::Unsupported(
            "这套 GLX 建不出离屏目标，探不出结论".to_owned(),
        ));
    }
    Err(BindError::Failed(
        "上下文绑不上（GLXBadContextTag 一类）".to_owned(),
    ))
}

/// 把上下文绑到指定的离屏目标上，读出版本。
fn bind_drawable(
    glx: &x11_dl::glx::Glx,
    xlib: &x11_dl::xlib::Xlib,
    display: *mut x11_dl::xlib::Display,
    context: x11_dl::glx::GLXContext,
    drawable: Drawable,
) -> Option<(String, String)> {
    let (draw, read) = match drawable {
        Drawable::Pbuffer(pbuffer) => (pbuffer, pbuffer),
        Drawable::Window(window) => (window, window),
    };
    // SAFETY: display 与离屏目标有效，context 是刚建出来的。
    if unsafe { (glx.glXMakeContextCurrent)(display, draw, read, context) } == 0 {
        destroy_drawable(glx, xlib, display, drawable);
        // SAFETY: display 与 context 有效。
        unsafe { (glx.glXDestroyContext)(display, context) };
        return None;
    }

    let version = read_gl_string(glx, GL_VERSION).unwrap_or_default();
    let renderer = read_gl_string(glx, GL_RENDERER).unwrap_or_default();

    // SAFETY: display、离屏目标、context 都有效。
    unsafe {
        (glx.glXMakeContextCurrent)(display, 0, 0, std::ptr::null_mut());
        (xlib.XSync)(display, 0);
    }
    destroy_drawable(glx, xlib, display, drawable);
    // SAFETY: display 与 context 有效。
    unsafe { (glx.glXDestroyContext)(display, context) };

    if version.is_empty() {
        return None;
    }
    Some((version, renderer))
}

/// 收掉离屏目标。
fn destroy_drawable(
    glx: &x11_dl::glx::Glx,
    xlib: &x11_dl::xlib::Xlib,
    display: *mut x11_dl::xlib::Display,
    drawable: Drawable,
) {
    match drawable {
        // SAFETY: display 与 pbuffer 有效。
        Drawable::Pbuffer(pbuffer) => unsafe { (glx.glXDestroyPbuffer)(display, pbuffer) },
        // SAFETY: display 与 window 有效。
        Drawable::Window(window) => unsafe {
            (xlib.XDestroyWindow)(display, window);
        },
    }
}

/// 建一个 1×1 的 pbuffer。
fn create_pbuffer(
    glx: &x11_dl::glx::Glx,
    display: *mut x11_dl::xlib::Display,
    config: x11_dl::glx::GLXFBConfig,
) -> x11_dl::glx::GLXPbuffer {
    let attributes = [GLX_PBUFFER_WIDTH, 1, GLX_PBUFFER_HEIGHT, 1, 0];
    // SAFETY: display 与 config 有效，attributes 以 0 结尾。
    unsafe { (glx.glXCreatePbuffer)(display, config, attributes.as_ptr()) }
}

/// 建一个 1×1 的窗口（不映射，窗口管理器看不到），视觉跟 fbconfig 一致。
fn create_window(
    xlib: &x11_dl::xlib::Xlib,
    glx: &x11_dl::glx::Glx,
    display: *mut x11_dl::xlib::Display,
    config: x11_dl::glx::GLXFBConfig,
) -> Option<x11_dl::xlib::XID> {
    // SAFETY: display 与 config 有效。
    let info = unsafe { (glx.glXGetVisualFromFBConfig)(display, config) };
    if info.is_null() {
        return None;
    }
    // SAFETY: info 是 Xlib 分配的 XVisualInfo。
    let (depth, visual) = unsafe { ((*info).depth, (*info).visual) };
    // SAFETY: info 是 Xlib 分配的，要用 XFree 还回去。
    unsafe { (xlib.XFree)(info.cast()) };

    // SAFETY: XSetWindowAttributes 是 C 结构体，全零是合法初值。
    let mut attributes: x11_dl::xlib::XSetWindowAttributes = unsafe { std::mem::zeroed() };
    attributes.background_pixmap = 0;
    attributes.border_pixel = 0;
    // SAFETY: display 有效。
    let root = unsafe { (xlib.XDefaultRootWindow)(display) };
    // SAFETY: display、root、visual 有效，属性只填了两项。
    let window = unsafe {
        (xlib.XCreateWindow)(
            display,
            root,
            0,
            0,
            1,
            1,
            0,
            depth,
            x11_dl::xlib::CopyFromParent as u32,
            visual,
            x11_dl::xlib::CWBackPixel | x11_dl::xlib::CWBorderPixel,
            &mut attributes,
        )
    };
    (window != 0).then_some(window)
}

/// 按 ARB 扩展建上下文。
fn create_context(
    glx: &x11_dl::glx::Glx,
    display: *mut x11_dl::xlib::Display,
    config: x11_dl::glx::GLXFBConfig,
    attributes: &[c_int],
) -> Option<x11_dl::glx::GLXContext> {
    type CreateContextAttribs = unsafe extern "C" fn(
        *mut x11_dl::xlib::Display,
        x11_dl::glx::GLXFBConfig,
        x11_dl::glx::GLXContext,
        c_int,
        *const c_int,
    ) -> x11_dl::glx::GLXContext;
    // SAFETY: 传入的是以 NUL 结尾的函数名。
    let raw = unsafe {
        (glx.glXGetProcAddressARB)(c"glXCreateContextAttribsARB".as_ptr().cast::<c_uchar>())
    }?;
    // SAFETY: `glXCreateContextAttribsARB` 的真实签名与上面一致。
    let create: CreateContextAttribs = unsafe { std::mem::transmute(raw) };
    // SAFETY: display、config 有效，attributes 以 0 结尾，共享上下文传空。
    let context = unsafe {
        create(
            display,
            config,
            std::ptr::null_mut(),
            1,
            attributes.as_ptr(),
        )
    };
    (!context.is_null()).then_some(context)
}

/// 旧式上下文：客户端没有 `GLX_ARB_create_context` 时 glutin 走这条路。
fn create_legacy_context(
    glx: &x11_dl::glx::Glx,
    display: *mut x11_dl::xlib::Display,
    config: x11_dl::glx::GLXFBConfig,
) -> Option<x11_dl::glx::GLXContext> {
    // SAFETY: display 与 config 有效，共享上下文传空。
    let context = unsafe {
        (glx.glXCreateNewContext)(display, config, GLX_RGBA_TYPE, std::ptr::null_mut(), 1)
    };
    (!context.is_null()).then_some(context)
}

/// 读客户端扩展串。
fn client_extensions(glx: &x11_dl::glx::Glx, display: *mut x11_dl::xlib::Display) -> String {
    // SAFETY: display 有效。
    let raw = unsafe { (glx.glXGetClientString)(display, GLX_CLIENT_EXTENSIONS) };
    if raw.is_null() {
        return String::new();
    }
    // SAFETY: Xlib 返回的是以 NUL 结尾的常量字符串。
    unsafe { std::ffi::CStr::from_ptr(raw) }
        .to_string_lossy()
        .into_owned()
}

/// 读一个 GL 字符串（需要当前上下文）。
fn read_gl_string(glx: &x11_dl::glx::Glx, name: c_int) -> Option<String> {
    type GetString = unsafe extern "C" fn(c_int) -> *const u8;
    // SAFETY: 传入的是以 NUL 结尾的函数名。
    let raw = unsafe { (glx.glXGetProcAddressARB)(c"glGetString".as_ptr().cast::<c_uchar>()) }?;
    // SAFETY: `glGetString` 的真实签名与上面一致。
    let get_string: GetString = unsafe { std::mem::transmute(raw) };
    // SAFETY: 调用时上下文是当前上下文。
    let raw = unsafe { get_string(name) };
    if raw.is_null() {
        return None;
    }
    // SAFETY: GL 返回的是以 NUL 结尾的常量字符串。
    Some(
        unsafe { std::ffi::CStr::from_ptr(raw.cast()) }
            .to_string_lossy()
            .into_owned(),
    )
}
