//! 纯 Rust PDF 查看器：连续滚动或单页翻页，视口内的页才光栅化。
//!
//! 每个打开的 PDF 配一小组常驻渲染线程。每条线程各持一份 `Pdf` 与 `RenderCache`，
//! 这两样都搬不动：`RenderCache` 内部是 `Rc`，`Pages<'a>` 又借用 `Pdf`。文件只读
//! 一次，字节在线程间共享；解析是惰性的，多开几份几乎不花时间。主线程只负责上传
//! 纹理和处理交互。多条线程是为了扫描封面这类一页要一两秒的重页不堵住后面的轻页。
//!
//! 主线程每帧把「现在需要的页」按优先级整批交给渲染队列，队列整批替换，不追加。
//! 这样滚走之后没渲完的页自然作废，滚回来时又会出现在新的一批里，不会漏页。
//!
//! 打开时先取全部页面的点尺寸——`hayro` 是惰性解析，300 页的文件也只要几毫秒——
//! 滚动条因此一开始就是准的，不会边渲染边跳。
//!
//! 系统程序仍作为加密或复杂外来 PDF 的兼容性兜底。

use crate::app::WorkerResult;
use crate::theme;
use eframe::egui;
use hayro::{
    RenderCache, RenderSettings,
    hayro_interpret::{InterpreterSettings, hayro_syntax::Pdf},
    vello_cpu::color::palette::css::WHITE,
};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;

pub(crate) type PdfKey = u64;

const MIN_ZOOM: f32 = 0.25;
const MAX_ZOOM: f32 = 4.0;
const ZOOM_STEP: f32 = 0.25;
/// 本机有没有打印能力。探测要扫一遍 PATH，不能每帧都做，进程内只算一次。
static PRINTING_AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
/// 单页光栅化宽度上限。再往上收益全被显存吃掉：2400px 宽的 A4 单页纹理已经
/// 33MiB，8192px 是 380MiB。超出部分交给 GPU 放大采样更划算。
const MAX_RENDER_WIDTH: u16 = 2400;
const MIN_RENDER_WIDTH: u16 = 160;
/// 宽度变化不到这个像素数就不重渲染，拖窗口时才不会一路抖。
const RERENDER_WIDTH_DELTA: u16 = 32;
/// 宽度稳定这么久（秒）才按新宽度重来；期间先把已有纹理缩放顶上。
const RESIZE_SETTLE: f64 = 0.15;
/// 纹理总预算。超出后按最近使用时间淘汰，当前帧用到的页不动。
/// 138 页的扫描件全缓存要 1.5GiB，必须有上界。
const TEXTURE_BUDGET: usize = 256 * 1024 * 1024;
/// 页与页之间、以及首尾的留白。
const PAGE_GAP: f32 = 18.0;
/// 页面两侧留给投影的空间。
const SIDE_MARGIN: f32 = 21.0;
/// 渲染线程数上限。每条线程各有一份字体与图像缓存，多了只是占内存。
const MAX_RENDER_THREADS: usize = 3;
/// 单页模式下触控板滑过这么多点算「一格」。鼠标滚轮按格上报，不走这个换算。
const TOUCHPAD_POINTS_PER_STEP: f32 = 80.0;
/// 滚轮停这么久（秒），没攒够一格的零头就清掉。
const WHEEL_IDLE: f64 = 0.3;
/// 翻页后这么久（秒）内屏蔽残余的平滑滚动，免得新页一出来就被余量带着往下走。
const FLIP_QUIET: f64 = 0.2;
/// 单页模式一屏最多并排几页。再多每页都小得没法读，渲染也白费。
const MAX_PAGES_PER_VIEW: usize = 6;

/// 渲染线程回传主线程的消息。
pub(crate) enum PdfMessage {
    /// 打开成功。`page_sizes` 是每页的点尺寸，拿到就能把滚动条按真实高度铺好。
    Opened { page_sizes: Vec<(f32, f32)> },
    /// 一页光栅化完成。`generation` 用来丢弃换宽度之前发出的旧结果。
    Page {
        generation: u64,
        index: usize,
        width: u16,
        image: egui::ColorImage,
    },
    /// 打开失败。渲染线程随即退出。
    Failed(String),
}

/// PDF 标签要交给应用外壳执行的动作。渲染请求不走这里——会话自己有渲染队列。
pub(crate) enum PdfAction {
    OpenExternal(PathBuf),
    Reveal(PathBuf),
    /// 打印这份 PDF。份号不同的多份成品必须由导出阶段生成，不能靠打印机份数。
    Print(PathBuf),
}

/// 浏览方式。
#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    /// 所有页上下相连，自由滚动。
    Continuous,
    /// 一次只看一页，滚轮一格翻一页（同 SumatraPDF 的「单页」）。
    SinglePage,
}

/// 缩放方式。适合宽度 / 适合页面随窗口大小变化，固定比例不随。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fit {
    Zoom,
    Width,
    Page,
}

/// 单页模式翻页后滚到页首还是页尾。往回翻停在页尾，读起来才是接着的。
#[derive(Clone, Copy)]
enum Edge {
    Top,
    Bottom,
}

/// 一页的纹理槽位。
#[derive(Default)]
struct PageSlot {
    texture: Option<egui::TextureHandle>,
    /// `texture` 的光栅化宽度，用来判断要不要按新宽度重渲染。
    width: u16,
    /// 最近一次出现在视口或预取区的帧号，淘汰时按它排序。
    last_used: u64,
    bytes: usize,
}

/// 渲染队列。主线程整批替换，渲染线程从队首取。
#[derive(Default)]
struct RenderQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
}

#[derive(Default)]
struct QueueState {
    generation: u64,
    jobs: VecDeque<(usize, u16)>,
    /// 正在某条线程上光栅化的页。新一批里有同代同宽的同一页就跳过，不重复干活。
    in_flight: Vec<(u64, usize, u16)>,
    closed: bool,
}

impl RenderQueue {
    fn lock(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 用新的一批整体替换队列。上一批没轮到的页直接作废。
    fn replace(&self, generation: u64, pages: Vec<(usize, u16)>) {
        let mut state = self.lock();
        state.generation = generation;
        state.jobs = pages.into();
        drop(state);
        self.ready.notify_all();
    }

    fn close(&self) {
        self.lock().closed = true;
        self.ready.notify_all();
    }

    /// 取下一页，没有就睡着等。队列关了返回 `None`，线程据此退出。
    fn next(&self) -> Option<(u64, usize, u16)> {
        let mut state = self.lock();
        loop {
            if state.closed {
                return None;
            }
            while let Some((index, width)) = state.jobs.pop_front() {
                let job = (state.generation, index, width);
                if !state.in_flight.contains(&job) {
                    state.in_flight.push(job);
                    return Some(job);
                }
            }
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn finish(&self, job: (u64, usize, u16)) {
        self.lock().in_flight.retain(|&other| other != job);
    }
}

/// 会话持有的渲染队列句柄。标签关闭时随会话析构，关掉队列，线程跟着退出。
struct RenderHandle(Arc<RenderQueue>);

impl Drop for RenderHandle {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// 一份打开的 PDF 的视图状态。
pub(crate) struct PdfSession {
    pub(crate) key: PdfKey,
    path: PathBuf,
    title: String,
    /// 结果通道，交给渲染线程用。
    results: Sender<WorkerResult>,
    /// 渲染队列。首帧才启动线程，那时才拿得到 `egui::Context`。
    renderer: Option<RenderHandle>,
    /// 每页的点尺寸。空表示还没打开完。
    page_sizes: Vec<(f32, f32)>,
    slots: Vec<PageSlot>,
    mode: ViewMode,
    fit: Fit,
    zoom: f32,
    /// 当前页实际显示的缩放比例。从「适合宽度 / 页面」切回按钮缩放时以它为起点。
    shown_scale: f32,
    /// 换宽度时 +1，迟到的旧宽度结果靠它作废。
    generation: u64,
    /// 已经提交给渲染线程的基准宽度。
    render_width: u16,
    /// 防抖：正在等待稳定的新宽度，以及它出现的时刻。
    pending_width: u16,
    width_changed_at: Option<f64>,
    /// 最近一次交给队列的那一批，按优先级排好。结果回来一页划掉一页；
    /// 这一帧算出的需求和它不同才重发。
    sent: Vec<(usize, u16)>,
    /// 连续模式由滚动位置推出；单页模式就是正在看的那页。
    current_page: usize,
    scroll_to: Option<usize>,
    /// 单页模式：窗口够宽时一屏并排几页。按上一帧的版式算，翻页步长也用它。
    per_view: usize,
    /// 单页模式：下一帧把页内滚动条放到页首或页尾。
    single_jump: Option<Edge>,
    /// 单页模式：上一帧页内滚动是否已到顶 / 到底，到头了滚轮才翻页。
    at_top: bool,
    at_bottom: bool,
    /// 单页模式：攒着的滚轮格数、最后一次滚轮时刻、残余平滑滚动屏蔽到何时。
    wheel_steps: f32,
    wheel_at: f64,
    quiet_until: f64,
    frame: u64,
    error: Option<String>,
}

impl PdfSession {
    pub(crate) fn new(
        key: PdfKey,
        path: PathBuf,
        title: Option<String>,
        results: Sender<WorkerResult>,
    ) -> Self {
        let fallback = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("PDF")
            .to_string();
        Self {
            key,
            path,
            title: title
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(fallback),
            results,
            renderer: None,
            page_sizes: Vec::new(),
            slots: Vec::new(),
            mode: ViewMode::Continuous,
            // 默认按 100% 原尺寸显示，和纸面一比一；适合宽度留给用户按需切换。
            fit: Fit::Zoom,
            zoom: 1.0,
            shown_scale: 1.0,
            generation: 0,
            render_width: 0,
            pending_width: 0,
            width_changed_at: None,
            sent: Vec::new(),
            current_page: 0,
            scroll_to: None,
            per_view: 1,
            single_jump: None,
            at_top: true,
            at_bottom: true,
            wheel_steps: 0.0,
            wheel_at: f64::NEG_INFINITY,
            quiet_until: f64::NEG_INFINITY,
            frame: 0,
            error: None,
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    pub(crate) fn tab_hover(&self) -> String {
        format!("PDF：{}", self.path.display())
    }

    /// 还在打开，或还有页在渲染。
    pub(crate) fn busy(&self) -> bool {
        if self.error.is_some() {
            return false;
        }
        if self.page_sizes.is_empty() {
            return self.renderer.is_some();
        }
        !self.sent.is_empty()
    }

    /// 收到渲染线程的消息。
    pub(crate) fn apply_message(&mut self, ctx: &egui::Context, message: PdfMessage) {
        match message {
            PdfMessage::Opened { page_sizes } => {
                self.slots = (0..page_sizes.len()).map(|_| PageSlot::default()).collect();
                self.page_sizes = page_sizes;
                self.error = None;
            }
            PdfMessage::Page {
                generation,
                index,
                width,
                image,
            } => {
                // 换宽度之前发出的结果直接丢，别盖掉新纹理。
                if generation != self.generation {
                    return;
                }
                let Some(slot) = self.slots.get_mut(index) else {
                    return;
                };
                self.sent.retain(|&job| job != (index, width));
                slot.bytes = image.width() * image.height() * 4;
                slot.width = width;
                // 旧句柄在这里析构，显存随之释放。
                slot.texture = Some(ctx.load_texture(
                    format!("pdf-{}-{index}", self.key),
                    image,
                    egui::TextureOptions::LINEAR,
                ));
            }
            PdfMessage::Failed(error) => {
                self.error = Some(error);
                self.renderer = None;
                self.sent.clear();
                self.slots.clear();
                self.page_sizes.clear();
            }
        }
    }

    /// 绘制查看器并返回需要外壳执行的动作。
    pub(crate) fn ui(&mut self, ui: &mut egui::Ui) -> Vec<PdfAction> {
        let mut actions = Vec::new();
        self.frame += 1;
        self.ensure_worker(ui.ctx());

        egui::Panel::top(egui::Id::new("pdf_toolbar").with(self.key))
            .frame(theme::panel(theme::surface(), 10))
            .show(ui, |ui| self.toolbar(ui, &mut actions));

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::canvas()))
            .show(ui, |ui| self.viewer(ui));
        actions
    }

    /// 首帧启动渲染线程。放在这里是因为要等 `egui::Context` 才能让线程主动唤醒 UI。
    fn ensure_worker(&mut self, ctx: &egui::Context) {
        if self.renderer.is_some() || self.error.is_some() {
            return;
        }
        let queue = Arc::new(RenderQueue::default());
        self.renderer = Some(RenderHandle(Arc::clone(&queue)));
        let key = self.key;
        let path = self.path.clone();
        let results = self.results.clone();
        let ctx = ctx.clone();
        thread::spawn(move || worker(key, path, queue, results, ctx));
    }

    fn page_count(&self) -> usize {
        self.page_sizes.len()
    }

    /// 某一页在当前缩放下的显示尺寸（点）。按页取真实宽高，横页和竖页混排也对。
    fn page_display_size(&self, index: usize, viewport: egui::Vec2) -> egui::Vec2 {
        let (width, height) = self.page_sizes[index];
        let width = width.max(1.0);
        let fit_width = (viewport.x - SIDE_MARGIN * 2.0).max(120.0);
        let display = match self.fit {
            Fit::Zoom => width * self.zoom,
            Fit::Width => fit_width,
            Fit::Page => {
                let fit_height = (viewport.y - PAGE_GAP * 2.0).max(120.0);
                fit_width.min(fit_height * width / height.max(1.0))
            }
        };
        egui::vec2(display, display * height / width)
    }

    /// 全部页面的显示尺寸、顶部偏移与内容总高。
    fn layout(&self, viewport: egui::Vec2) -> (Vec<egui::Vec2>, Vec<f32>, f32) {
        let mut sizes = Vec::with_capacity(self.page_count());
        let mut tops = Vec::with_capacity(self.page_count());
        let mut y = PAGE_GAP;
        for index in 0..self.page_count() {
            let size = self.page_display_size(index, viewport);
            tops.push(y);
            y += size.y + PAGE_GAP;
            sizes.push(size);
        }
        (sizes, tops, y)
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<PdfAction>) {
        ui.horizontal(|ui| {
            let count = self.page_count();
            let previous = self.step_target(false);
            if theme::icon_button_enabled(
                ui,
                previous.is_some(),
                theme::Icon::ChevronUp,
                "上一页",
            )
            .clicked()
            {
                self.scroll_to = previous;
            }

            let next = self.step_target(true);
            if theme::icon_button_enabled(ui, next.is_some(), theme::Icon::ChevronDown, "下一页")
                .clicked()
            {
                self.scroll_to = next;
            }

            let mut page_number = self.current_page + 1;
            let max_page = count.max(1);
            let response = ui.add_enabled(
                count > 0,
                egui::DragValue::new(&mut page_number)
                    .range(1..=max_page)
                    .speed(0.1),
            );
            ui.label(if count > 0 {
                format!("/ {count} 页")
            } else {
                "/ … 页".to_string()
            });
            if response.changed() {
                self.scroll_to = Some(page_number.saturating_sub(1));
            }

            ui.separator();
            if ui
                .selectable_label(self.mode == ViewMode::Continuous, "连续")
                .on_hover_text("所有页上下相连，自由滚动")
                .clicked()
                && self.mode != ViewMode::Continuous
            {
                self.mode = ViewMode::Continuous;
                self.scroll_to = Some(self.current_page);
            }
            if ui
                .selectable_label(self.mode == ViewMode::SinglePage, "单页")
                .on_hover_text("按页翻看，滚轮一格翻一屏；窗口够宽时从左到右并排多页。也可用 PageUp / PageDown、方向键翻页")
                .clicked()
                && self.mode != ViewMode::SinglePage
            {
                self.mode = ViewMode::SinglePage;
                // 单页翻着看，整页落在窗口里最顺手，同 SumatraPDF 的默认。
                self.fit = Fit::Page;
                self.scroll_to = Some(self.current_page);
            }

            ui.separator();
            let scale = self.shown_scale;
            if theme::icon_button_enabled(ui, scale > MIN_ZOOM, theme::Icon::ZoomOut, "缩小")
                .on_hover_text("缩小（也可按住 Ctrl 滚动滚轮）")
                .clicked()
            {
                self.set_zoom(zoom_step_down(scale));
            }
            if theme::icon_button_enabled(ui, scale < MAX_ZOOM, theme::Icon::ZoomIn, "放大")
                .on_hover_text("放大（也可按住 Ctrl 滚动滚轮）")
                .clicked()
            {
                self.set_zoom(zoom_step_up(scale));
            }
            ui.label(format!("{:.0}%", scale * 100.0));
            if ui
                .selectable_label(self.fit == Fit::Zoom && self.zoom == 1.0, "100%")
                .clicked()
            {
                self.set_zoom(1.0);
            }
            if ui
                .selectable_label(self.fit == Fit::Width, "适合宽度")
                .clicked()
                && self.fit != Fit::Width
            {
                self.fit = Fit::Width;
                // 缩放变了版式跟着变，把当前页重新拉回视野，不然会滚到别处。
                self.scroll_to = Some(self.current_page);
            }
            if ui
                .selectable_label(self.fit == Fit::Page, "适合页面")
                .clicked()
                && self.fit != Fit::Page
            {
                self.fit = Fit::Page;
                self.scroll_to = Some(self.current_page);
            }

            ui.separator();
            // 打印能力每帧探测一次代价太高（Unix 要扫 PATH），进程内只算一次。
            let can_print = *PRINTING_AVAILABLE.get_or_init(crate::print_pdf::printing_available);
            let print_button = ui.add_enabled(
                can_print,
                theme::icon_text_button(theme::Icon::Print, "打印"),
            );
            let print_button = if can_print {
                print_button.on_hover_text("打印这份 PDF")
            } else {
                print_button
                    .on_disabled_hover_text("未检测到打印服务，请用「系统打开」后从阅读器打印")
            };
            if print_button.clicked() {
                actions.push(PdfAction::Print(self.path.clone()));
            }

            if ui
                .add(theme::icon_text_button(theme::Icon::Open, "系统打开"))
                .on_hover_text("遇到复杂或加密 PDF 时使用系统程序打开")
                .clicked()
            {
                actions.push(PdfAction::OpenExternal(self.path.clone()));
            }
            if theme::icon_button(ui, theme::Icon::Reveal, "在文件管理器中定位").clicked()
            {
                actions.push(PdfAction::Reveal(self.path.clone()));
            }
        });
    }

    fn set_zoom(&mut self, zoom: f32) {
        self.fit = Fit::Zoom;
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        self.scroll_to = Some(self.current_page);
    }

    fn viewer(&mut self, ui: &mut egui::Ui) {
        if let Some(error) = self.error.clone() {
            ui.vertical_centered(|ui| {
                ui.add_space(64.0);
                ui.colored_label(theme::danger(), "无法在应用内预览这个 PDF");
                ui.add_space(6.0);
                ui.label(error);
                ui.add_space(4.0);
                ui.label("可以使用上方“系统打开”继续查看。");
            });
            return;
        }
        if self.page_sizes.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(64.0);
                ui.spinner();
                ui.label("正在打开 PDF…");
            });
            return;
        }

        // Ctrl+滚轮缩放：按住 Ctrl（mac 为 Cmd）时 egui 把滚动量报成 zoom_delta，
        // 滚动区不会同时滚动，两者天然不冲突。只在指针位于查看区时响应。
        let zoom_delta = ui.ctx().input(|input| input.zoom_delta());
        if zoom_delta != 1.0 && ui.rect_contains_pointer(ui.max_rect()) {
            self.set_zoom(self.shown_scale * zoom_delta);
        }
        self.keyboard(ui.ctx());

        let viewport = ui.available_size();
        let pixels_per_point = ui.ctx().pixels_per_point().max(1.0);
        self.settle_width(ui.ctx(), viewport, pixels_per_point);
        // 拖窗口或连续缩放的过程中，已有纹理先缩放顶着，只补还没有纹理的页。
        let settling = self.width_changed_at.is_some();

        let mut wanted = match self.mode {
            ViewMode::Continuous => self.continuous(ui, viewport, pixels_per_point, settling),
            ViewMode::SinglePage => self.single_page(ui, viewport, pixels_per_point, settling),
        };
        let (width, _) = self.page_sizes[self.current_page];
        self.shown_scale = self.page_display_size(self.current_page, viewport).x / width.max(1.0);

        // 离视口越近越先渲；同样近的按页序。
        wanted.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let wanted = wanted
            .into_iter()
            .map(|(_, index, width)| (index, width))
            .collect();
        self.evict();
        self.request(wanted);
    }

    /// 翻页快捷键。有输入框拿着焦点时不抢键。
    fn keyboard(&mut self, ctx: &egui::Context) {
        if ctx.memory(|memory| memory.focused().is_some()) {
            return;
        }
        let none = egui::Modifiers::NONE;
        let (next, previous, first, last) = ctx.input_mut(|input| {
            // 用 `|` 而不是 `||`：两个键都要消费掉，不能短路。
            (
                input.consume_key(none, egui::Key::PageDown)
                    | input.consume_key(none, egui::Key::ArrowRight),
                input.consume_key(none, egui::Key::PageUp)
                    | input.consume_key(none, egui::Key::ArrowLeft),
                input.consume_key(none, egui::Key::Home),
                input.consume_key(none, egui::Key::End),
            )
        });
        let last_page = self.page_count().saturating_sub(1);
        if next && let Some(target) = self.step_target(true) {
            self.scroll_to = Some(target);
        }
        if previous && let Some(target) = self.step_target(false) {
            self.scroll_to = Some(target);
        }
        if first {
            self.scroll_to = Some(0);
        }
        if last {
            self.scroll_to = Some(last_page);
        }
    }

    /// 上一屏 / 下一屏从哪页开始。连续模式一次一页；单页模式一次一整屏，
    /// 并排几页就跳几页。到头了返回 `None`，按钮随之置灰。
    fn step_target(&self, forward: bool) -> Option<usize> {
        let count = self.page_count();
        if count == 0 {
            return None;
        }
        let (base, step) = match self.mode {
            ViewMode::Continuous => (self.current_page, 1),
            ViewMode::SinglePage => (
                spread_start(self.current_page, self.per_view),
                self.per_view,
            ),
        };
        if forward {
            (base + step < count).then_some(base + step)
        } else {
            (base > 0).then(|| base.saturating_sub(step))
        }
    }

    /// 单页模式一屏能并排几页：按当前页的显示宽度算，放不下两页就是一页。
    /// 「适合宽度」本来就是一页占满，自然只有一页。
    fn pages_per_view(&self, viewport: egui::Vec2) -> usize {
        let width = self.page_display_size(self.current_page, viewport).x;
        let room = viewport.x - SIDE_MARGIN * 2.0 + PAGE_GAP;
        ((room / (width + PAGE_GAP)).floor() as usize).clamp(1, MAX_PAGES_PER_VIEW)
    }

    /// 连续模式：所有页上下相连。返回这一帧需要（重新）渲染的页及其优先级。
    fn continuous(
        &mut self,
        ui: &mut egui::Ui,
        viewport: egui::Vec2,
        pixels_per_point: f32,
        settling: bool,
    ) -> Vec<(f32, usize, u16)> {
        let (sizes, tops, total) = self.layout(viewport);
        // 放大时页面可能比窗口宽，内容区得按最宽的那页撑开，横向才滚得动。
        let content_width = sizes
            .iter()
            .map(|size| size.x + SIDE_MARGIN * 2.0)
            .fold(viewport.x, f32::max);
        let mut scroll = egui::ScrollArea::both()
            .id_salt(("pdf_scroll", self.key))
            .auto_shrink([false, false]);
        if let Some(index) = self.scroll_to.take()
            && let Some(top) = tops.get(index.min(tops.len().saturating_sub(1)))
        {
            scroll = scroll.vertical_scroll_offset((top - PAGE_GAP).max(0.0));
        }

        // 闭包里要可变借用 slots，先把别的字段读出来。
        let frame = self.frame;
        let slots = &mut self.slots;
        let (wanted, current) = scroll
            .show(ui, |ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(content_width, total), egui::Sense::hover());
                let clip = ui.clip_rect();
                // 视口上下各多备一屏，滚起来才不总看见占位白页。
                let prefetch = clip.expand2(egui::vec2(0.0, clip.height()));
                let painter = ui.painter();
                let mut wanted = Vec::new();
                // 当前页取占视口高度最多的那页，而不是露了一条边的上一页。
                let mut current = None;
                let mut best_visible = 0.0;

                // 只看预取区附近的页：先二分找到第一页，300 页也不用逐页判断。
                let first = tops.partition_point(|top| rect.top() + top < prefetch.top());
                for index in first.saturating_sub(1)..sizes.len() {
                    let size = sizes[index];
                    let left = rect.left() + ((content_width - size.x) / 2.0).max(0.0);
                    let page_rect =
                        egui::Rect::from_min_size(egui::pos2(left, rect.top() + tops[index]), size);
                    if page_rect.top() > prefetch.bottom() {
                        break;
                    }
                    if !page_rect.intersects(prefetch) {
                        continue;
                    }
                    let slot = &mut slots[index];
                    slot.last_used = frame;
                    let distance = vertical_distance(page_rect, clip);
                    if let Some(target) = wants_render(slot, size.x, pixels_per_point, settling) {
                        wanted.push((distance, index, target));
                    }
                    if distance > 0.0 {
                        continue;
                    }
                    let visible = page_rect.intersect(clip).height();
                    if visible > best_visible {
                        best_visible = visible;
                        current = Some(index);
                    }
                    paint_page(painter, page_rect, index, slot);
                }
                (wanted, current)
            })
            .inner;

        if let Some(index) = current {
            self.current_page = index;
        }
        wanted
    }

    /// 单页模式：一次一屏。窗口够宽时从左到右并排多页，滚轮一格翻一屏。
    /// 页面比窗口高时先在页内滚，滚到头再翻。
    fn single_page(
        &mut self,
        ui: &mut egui::Ui,
        viewport: egui::Vec2,
        pixels_per_point: f32,
        settling: bool,
    ) -> Vec<(f32, usize, u16)> {
        let count = self.page_count();
        if let Some(index) = self.scroll_to.take() {
            self.current_page = index.min(count - 1);
            self.single_jump = Some(Edge::Top);
        }
        self.current_page = self.current_page.min(count - 1);
        self.per_view = self.pages_per_view(viewport);
        let (_, content) = self.spread_layout(viewport);
        self.wheel_flip(ui, content.y > viewport.y + 0.5);

        let now = ui.ctx().input(|input| input.time);
        if now < self.quiet_until {
            ui.ctx()
                .input_mut(|input| input.smooth_scroll_delta.y = 0.0);
        }

        // 翻过页的话版式要按新的一屏重算。
        let (pages, content) = self.spread_layout(viewport);
        let mut scroll = egui::ScrollArea::both()
            .id_salt(("pdf_single", self.key))
            .auto_shrink([false, false]);
        if let Some(edge) = self.single_jump.take() {
            scroll = scroll.vertical_scroll_offset(match edge {
                Edge::Top => 0.0,
                // 超出部分由滚动区自己夹回，不必精确算。
                Edge::Bottom => content.y,
            });
        }

        let frame = self.frame;
        let slots = &mut self.slots;
        let output = scroll.show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(content, egui::Sense::hover());
            let row_width: f32 = pages.iter().map(|(_, size)| size.x).sum::<f32>()
                + PAGE_GAP * (pages.len() - 1) as f32;
            let mut left = rect.center().x - row_width / 2.0;
            for &(index, size) in &pages {
                let page_rect = egui::Rect::from_min_size(
                    egui::pos2(left, rect.center().y - size.y / 2.0),
                    size,
                );
                let slot = &mut slots[index];
                slot.last_used = frame;
                paint_page(ui.painter(), page_rect, index, slot);
                left += size.x + PAGE_GAP;
            }
        });
        let max_offset = (output.content_size.y - output.inner_rect.height()).max(0.0);
        self.at_top = output.state.offset.y <= 1.0;
        self.at_bottom = output.state.offset.y >= max_offset - 1.0;

        // 当前一屏先渲，从左到右；再备下一屏、上一屏，翻过去就是现成的。
        let start = spread_start(self.current_page, self.per_view);
        let end = (start + self.per_view).min(count);
        let next = end..(end + self.per_view).min(count);
        let previous = start.saturating_sub(self.per_view)..start;
        let mut wanted = Vec::new();
        let ahead = (start..end)
            .map(|index| (0.0, index))
            .chain(next.map(|index| (1.0, index)))
            .chain(previous.map(|index| (2.0, index)));
        for (priority, index) in ahead {
            let width = self.page_display_size(index, viewport).x;
            let slot = &mut self.slots[index];
            slot.last_used = self.frame;
            if let Some(target) = wants_render(slot, width, pixels_per_point, settling) {
                wanted.push((priority, index, target));
            }
        }
        wanted
    }

    /// 当前这一屏的各页及其显示尺寸，和整屏内容区的大小。
    fn spread_layout(&self, viewport: egui::Vec2) -> (Vec<(usize, egui::Vec2)>, egui::Vec2) {
        let start = spread_start(self.current_page, self.per_view);
        let end = (start + self.per_view).min(self.page_count());
        let pages: Vec<(usize, egui::Vec2)> = (start..end)
            .map(|index| (index, self.page_display_size(index, viewport)))
            .collect();
        let width =
            pages.iter().map(|(_, size)| size.x).sum::<f32>() + PAGE_GAP * (pages.len() - 1) as f32;
        let height = pages.iter().map(|(_, size)| size.y).fold(0.0, f32::max);
        let content = egui::vec2(
            (width + SIDE_MARGIN * 2.0).max(viewport.x),
            (height + PAGE_GAP * 2.0).max(viewport.y),
        );
        (pages, content)
    }

    /// 单页模式的滚轮：攒够一格翻一屏。鼠标滚轮按「行」上报，一格正好一行；
    /// 触控板按点上报，按 `TOUCHPAD_POINTS_PER_STEP` 折算。
    fn wheel_flip(&mut self, ui: &egui::Ui, overflows: bool) {
        if !ui.rect_contains_pointer(ui.max_rect()) {
            self.wheel_steps = 0.0;
            return;
        }
        let (now, steps) = ui.ctx().input(|input| {
            let mut steps = 0.0;
            for event in &input.events {
                if let egui::Event::MouseWheel {
                    unit,
                    delta,
                    modifiers,
                    ..
                } = event
                {
                    // Ctrl+滚轮是缩放，不翻页。
                    if modifiers.ctrl || modifiers.command {
                        continue;
                    }
                    steps += match unit {
                        egui::MouseWheelUnit::Point => delta.y / TOUCHPAD_POINTS_PER_STEP,
                        egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => delta.y,
                    };
                }
            }
            (input.time, steps)
        });
        if steps == 0.0 {
            if now - self.wheel_at > WHEEL_IDLE {
                self.wheel_steps = 0.0;
            }
            return;
        }
        self.wheel_at = now;
        // egui 的滚动量是内容移动方向：负值是往下滚，看后面的页。
        let forward = steps < 0.0;
        // 页内还能往这个方向滚，就交给滚动区，不算翻页。
        if overflows && !(if forward { self.at_bottom } else { self.at_top }) {
            self.wheel_steps = 0.0;
            return;
        }
        // 换了方向，之前攒的零头作废。
        if self.wheel_steps != 0.0 && (self.wheel_steps < 0.0) != forward {
            self.wheel_steps = 0.0;
        }
        self.wheel_steps += steps;
        if self.wheel_steps.abs() < 0.999 {
            return;
        }
        self.wheel_steps = 0.0;
        if let Some(target) = self.step_target(forward) {
            self.current_page = target;
            self.single_jump = Some(if forward { Edge::Top } else { Edge::Bottom });
            self.quiet_until = now + FLIP_QUIET;
        }
    }

    /// 宽度防抖。拖窗口的过程中先用已有纹理缩放顶着，稳定下来才换一代重渲染。
    fn settle_width(&mut self, ctx: &egui::Context, viewport: egui::Vec2, pixels_per_point: f32) {
        let base = self.page_display_size(0, viewport).x;
        let target = render_width(base, pixels_per_point);
        if target.abs_diff(self.render_width) < RERENDER_WIDTH_DELTA {
            self.width_changed_at = None;
            return;
        }
        let now = ctx.input(|input| input.time);
        if self.pending_width != target {
            self.pending_width = target;
            self.width_changed_at = Some(now);
        }
        if now - self.width_changed_at.unwrap_or(now) < RESIZE_SETTLE {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(RESIZE_SETTLE));
            return;
        }
        self.render_width = target;
        self.width_changed_at = None;
        // 换代作废在途结果；旧纹理留着继续缩放显示，直到新的回来。
        self.generation += 1;
        self.sent.clear();
        if let Some(renderer) = &self.renderer {
            renderer.0.replace(self.generation, Vec::new());
        }
    }

    /// 把这一帧要的页整批交给渲染队列。和上一批一样就不动，免得每帧抢锁。
    fn request(&mut self, wanted: Vec<(usize, u16)>) {
        if wanted == self.sent {
            return;
        }
        let Some(renderer) = &self.renderer else {
            return;
        };
        renderer.0.replace(self.generation, wanted.clone());
        self.sent = wanted;
    }

    /// 纹理超预算就按最近使用时间淘汰。这一帧用到的页不动。
    fn evict(&mut self) {
        let mut total: usize = self.slots.iter().map(|slot| slot.bytes).sum();
        if total <= TEXTURE_BUDGET {
            return;
        }
        let mut order: Vec<usize> = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.texture.is_some() && slot.last_used != self.frame)
            .map(|(index, _)| index)
            .collect();
        order.sort_by_key(|&index| self.slots[index].last_used);
        for index in order {
            if total <= TEXTURE_BUDGET {
                break;
            }
            let slot = &mut self.slots[index];
            total -= slot.bytes;
            slot.texture = None;
            slot.bytes = 0;
            slot.width = 0;
        }
    }
}

/// 这一页要不要（重新）光栅化；要的话返回目标像素宽度。
/// 已在队列里的页照样返回——每帧交出去的是完整的需求，不是增量。
fn wants_render(
    slot: &PageSlot,
    display_width: f32,
    pixels_per_point: f32,
    settling: bool,
) -> Option<u16> {
    let target = render_width(display_width, pixels_per_point);
    let stale = match slot.texture {
        None => true,
        Some(_) => !settling && slot.width.abs_diff(target) >= RERENDER_WIDTH_DELTA,
    };
    stale.then_some(target)
}

/// 并排 `per_view` 页时，第 `page` 页所在那一屏的首页。按页序对齐，
/// 1–2、3–4 这样分屏，来回翻页每屏的组合不变。
fn spread_start(page: usize, per_view: usize) -> usize {
    page / per_view.max(1) * per_view.max(1)
}

/// 两个矩形在竖直方向上的间距，相交为 0。
fn vertical_distance(page: egui::Rect, clip: egui::Rect) -> f32 {
    (page.top() - clip.bottom())
        .max(clip.top() - page.bottom())
        .max(0.0)
}

/// 按钮缩放：落到 `ZOOM_STEP` 的整数倍上，从「适合宽度」切过来也是整齐的百分比。
fn zoom_step_up(scale: f32) -> f32 {
    ((scale / ZOOM_STEP + 0.01).floor() + 1.0) * ZOOM_STEP
}

fn zoom_step_down(scale: f32) -> f32 {
    ((scale / ZOOM_STEP - 0.01).ceil() - 1.0) * ZOOM_STEP
}

/// 页面显示宽度（点）换算成光栅化宽度（设备像素）。
fn render_width(display_width: f32, pixels_per_point: f32) -> u16 {
    (display_width * pixels_per_point)
        .round()
        .clamp(f32::from(MIN_RENDER_WIDTH), f32::from(MAX_RENDER_WIDTH)) as u16
}

/// 画一页：白纸 + 细边 + 轻投影，纹理没到位时留页码占位。
fn paint_page(painter: &egui::Painter, rect: egui::Rect, index: usize, slot: &PageSlot) {
    painter.add(
        egui::epaint::Shadow {
            offset: [0, 3],
            blur: 12,
            spread: 0,
            color: egui::Color32::from_black_alpha(35),
        }
        .as_shape(rect, 0),
    );
    painter.rect_filled(rect, 0, egui::Color32::WHITE);
    if let Some(texture) = &slot.texture {
        painter.image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("第 {} 页", index + 1),
            egui::FontId::proportional(13.0),
            theme::text_muted(),
        );
    }
    painter.rect_stroke(
        rect,
        0,
        egui::Stroke::new(1.0, theme::border()),
        egui::StrokeKind::Inside,
    );
}

/// 渲染线程数：CPU 的一半，至少 1 条，至多 `MAX_RENDER_THREADS` 条。
fn render_threads() -> usize {
    thread::available_parallelism()
        .map(|cores| (cores.get() / 2).clamp(1, MAX_RENDER_THREADS))
        .unwrap_or(1)
}

/// 首条渲染线程：读文件、报页面尺寸，再拉起其余渲染线程，自己也加入渲染。
fn worker(
    key: PdfKey,
    path: PathBuf,
    queue: Arc<RenderQueue>,
    results: Sender<WorkerResult>,
    ctx: egui::Context,
) {
    let send = |message: PdfMessage| -> bool {
        let delivered = results.send(WorkerResult::Pdf { key, message }).is_ok();
        // 主线程可能正闲着睡着，主动敲醒它。
        ctx.request_repaint();
        delivered
    };

    let bytes = match std::fs::read(&path) {
        Ok(bytes) => Arc::new(bytes),
        Err(error) => {
            send(PdfMessage::Failed(format!(
                "无法读取 {}：{error}",
                path.display()
            )));
            return;
        }
    };
    let pdf = match Pdf::new(Arc::clone(&bytes)) {
        Ok(pdf) => pdf,
        Err(error) => {
            send(PdfMessage::Failed(format!(
                "PDF 文件结构无法解析：{error:?}"
            )));
            return;
        }
    };
    let pages = pdf.pages();
    if pages.is_empty() {
        send(PdfMessage::Failed("PDF 中没有可显示的页面".into()));
        return;
    }
    let page_sizes: Vec<(f32, f32)> = pages.iter().map(|page| page.render_dimensions()).collect();
    if page_sizes
        .iter()
        .any(|(w, h)| !w.is_finite() || !h.is_finite() || *w <= 0.0 || *h <= 0.0)
    {
        send(PdfMessage::Failed("PDF 页面尺寸无效".into()));
        return;
    }
    if !send(PdfMessage::Opened { page_sizes }) {
        return;
    }

    for _ in 1..render_threads() {
        let bytes = Arc::clone(&bytes);
        let queue = Arc::clone(&queue);
        let results = results.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            // 同一份字节首条线程已经解析成功，这里失败只可能是极端情况，少一条线程而已。
            if let Ok(pdf) = Pdf::new(bytes) {
                render_loop(key, &pdf, &queue, &results, &ctx);
            }
        });
    }
    render_loop(key, &pdf, &queue, &results, &ctx);
}

/// 渲染线程主循环：从队列取页、光栅化、回传，队列关闭即退出。
fn render_loop(
    key: PdfKey,
    pdf: &Pdf,
    queue: &RenderQueue,
    results: &Sender<WorkerResult>,
    ctx: &egui::Context,
) {
    let pages = pdf.pages();
    // 字体与图像的解码结果都落在这里。hayro 的建议就是每份 PDF 建一个、全程复用；
    // 每页新建一个等于每翻一页重新解码一遍嵌入的中文字体。
    let cache = RenderCache::new();
    while let Some(job) = queue.next() {
        let (generation, index, width) = job;
        let image = pages.get(index).map(|page| {
            let (page_width, _) = page.render_dimensions();
            let scale = f32::from(width) / page_width;
            let pixmap = hayro::render(
                page,
                &cache,
                &InterpreterSettings::default(),
                &RenderSettings {
                    x_scale: scale,
                    y_scale: scale,
                    width: Some(width),
                    bg_color: WHITE,
                    ..Default::default()
                },
            );
            // hayro 出的是预乘 alpha，直接按预乘读；顺带把逐像素转换留在这条线程上。
            egui::ColorImage::from_rgba_premultiplied(
                [usize::from(pixmap.width()), usize::from(pixmap.height())],
                pixmap.data_as_u8_slice(),
            )
        });
        queue.finish(job);
        let Some(image) = image else {
            continue;
        };
        let delivered = results
            .send(WorkerResult::Pdf {
                key,
                message: PdfMessage::Page {
                    generation,
                    index,
                    width,
                    image,
                },
            })
            .is_ok();
        ctx.request_repaint();
        if !delivered {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{self, Receiver};

    /// 起一组渲染线程；返回的句柄析构时关队列，测试结束线程随之退出。
    fn spawn_worker(path: PathBuf) -> (RenderHandle, Receiver<WorkerResult>) {
        let (tx, rx) = mpsc::channel();
        let queue = Arc::new(RenderQueue::default());
        let handle = RenderHandle(Arc::clone(&queue));
        thread::spawn(move || worker(1, path, queue, tx, egui::Context::default()));
        (handle, rx)
    }

    /// 生成一页最小 PDF，避免单元测试依赖系统 PDF 程序或原生动态库。
    fn minimal_pdf() -> Vec<u8> {
        let mut bytes = b"%PDF-1.4\n".to_vec();
        let mut offsets = vec![0usize];
        let objects: [&[u8]; 5] = [
            b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>",
            b"<< /Length 41 >>\nstream\nBT /F1 24 Tf 20 150 Td (Hello PDF) Tj ET\nendstream",
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        ];
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            bytes.extend_from_slice(object);
            bytes.extend_from_slice(b"\nendobj\n");
        }
        let xref = bytes.len();
        bytes.extend_from_slice(format!("xref\n0 {}\n", offsets.len()).as_bytes());
        bytes.extend_from_slice(b"0000000000 65535 f \n");
        for offset in offsets.iter().skip(1) {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                offsets.len()
            )
            .as_bytes(),
        );
        bytes
    }

    /// 收干渲染线程的消息，直到拿到想要的那条或超时。
    fn drain(rx: &Receiver<WorkerResult>, mut take: impl FnMut(PdfMessage) -> bool) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            match rx.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok(WorkerResult::Pdf { message, .. }) => {
                    if take(message) {
                        return true;
                    }
                }
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return false,
            }
        }
        false
    }

    /// 打开阶段就该报出全部页面尺寸，滚动条不用等光栅化。
    #[test]
    fn reports_page_sizes_before_rendering() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("one-page.pdf");
        std::fs::write(&path, minimal_pdf()).unwrap();

        let (_commands, rx) = spawn_worker(path);

        let mut sizes = Vec::new();
        assert!(drain(&rx, |message| match message {
            PdfMessage::Opened { page_sizes } => {
                sizes = page_sizes;
                true
            }
            _ => false,
        }));
        assert_eq!(sizes, vec![(200.0, 300.0)]);
    }

    /// 请求一页应当回来一张宽度吻合、按预乘读入的不透明纹理。
    #[test]
    fn renders_requested_page() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("one-page.pdf");
        std::fs::write(&path, minimal_pdf()).unwrap();

        let (commands, rx) = spawn_worker(path);
        commands.0.replace(7, vec![(0, 400)]);

        let mut got = None;
        assert!(drain(&rx, |message| match message {
            PdfMessage::Page {
                generation,
                index,
                width,
                image,
            } => {
                got = Some((generation, index, width, image));
                true
            }
            _ => false,
        }));
        let (generation, index, width, image) = got.unwrap();
        assert_eq!((generation, index, width), (7, 0, 400));
        assert_eq!(image.width(), 400);
        assert_eq!(image.height(), 600);
        assert!(image.pixels.iter().all(|pixel| pixel.a() == 255));
    }

    /// 整批替换：上一批没轮到的页作废，新一批按顺序取；正在渲的同一页不重复派发。
    /// 旧实现在这里丢页——被新指令顶掉的页在主线程上永远挂着「已请求」，中间页一直空白。
    #[test]
    fn queue_replaces_batch_and_skips_in_flight() {
        let queue = RenderQueue::default();
        queue.replace(1, vec![(0, 400), (1, 400), (2, 400)]);
        let first = queue.next().unwrap();
        assert_eq!(first, (1, 0, 400));

        // 用户滚走了：新一批里仍有正在渲的 0 页，外加 5、6 页。
        queue.replace(1, vec![(0, 400), (5, 400), (6, 400)]);
        assert_eq!(queue.next(), Some((1, 5, 400)));
        queue.finish(first);
        assert_eq!(queue.next(), Some((1, 6, 400)));

        // 1、2 页没在新一批里，再滚回来时重新交上去照样会渲。
        queue.replace(1, vec![(1, 400), (2, 400)]);
        assert_eq!(queue.next(), Some((1, 1, 400)));
        assert_eq!(queue.next(), Some((1, 2, 400)));

        queue.close();
        assert_eq!(queue.next(), None);
    }

    /// 单页模式：宽屏下 A4 竖页适合页面能并排两页，窄屏一页，适合宽度永远一页；
    /// 分屏按页序对齐，翻页一次跳一整屏。
    #[test]
    fn single_page_mode_spreads_pages_across_wide_windows() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut session = PdfSession::new(1, PathBuf::from("a.pdf"), None, tx);
        session.page_sizes = vec![(595.0, 842.0); 7];
        session.mode = ViewMode::SinglePage;
        session.fit = Fit::Page;
        assert_eq!(session.pages_per_view(egui::vec2(1600.0, 900.0)), 2);
        assert_eq!(session.pages_per_view(egui::vec2(1000.0, 900.0)), 1);
        session.fit = Fit::Width;
        assert_eq!(session.pages_per_view(egui::vec2(1600.0, 900.0)), 1);

        session.per_view = 2;
        session.current_page = 3; // 第 4 页，落在 3–4 页那一屏
        assert_eq!(session.step_target(true), Some(4));
        assert_eq!(session.step_target(false), Some(0));
        session.current_page = 6; // 最后单出的第 7 页
        assert_eq!(session.step_target(true), None);
        assert_eq!(session.step_target(false), Some(4));
    }

    #[test]
    fn zoom_steps_land_on_quarter_marks() {
        assert_eq!(zoom_step_up(1.0), 1.25);
        assert_eq!(zoom_step_down(1.0), 0.75);
        // 「适合宽度」下的 1.37 倍，放大落到 1.5，缩小落到 1.25。
        assert_eq!(zoom_step_up(1.37), 1.5);
        assert_eq!(zoom_step_down(1.37), 1.25);
    }

    /// 读不到文件要给出中文提示，而不是让标签一直转圈。
    #[test]
    fn missing_pdf_reports_readable_error() {
        let (_commands, rx) = spawn_worker(PathBuf::from("definitely-missing.pdf"));

        let mut error = String::new();
        assert!(drain(&rx, |message| match message {
            PdfMessage::Failed(text) => {
                error = text;
                true
            }
            _ => false,
        }));
        assert!(error.contains("无法读取"));
    }

    /// 手工回归入口：给出真实导出件即可验证复杂中文公文，不把本机产物写死进测试。
    #[test]
    #[ignore = "set GONGWEN_PDF_TEST to an exported PDF path"]
    fn renders_real_export_when_requested() {
        let path = std::env::var_os("GONGWEN_PDF_TEST")
            .map(PathBuf::from)
            .expect("请设置 GONGWEN_PDF_TEST");
        let (commands, rx) = spawn_worker(path);

        let mut count = 0;
        assert!(drain(&rx, |message| match message {
            PdfMessage::Opened { page_sizes } => {
                count = page_sizes.len();
                true
            }
            _ => false,
        }));
        assert!(count >= 1);

        commands
            .0
            .replace(1, (0..count).map(|index| (index, 900)).collect());
        let mut rendered = 0;
        drain(&rx, |message| {
            if let PdfMessage::Page { image, .. } = message {
                assert_eq!(image.width(), 900);
                rendered += 1;
            }
            rendered == count
        });
        assert_eq!(rendered, count);
    }
}
