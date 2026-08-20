//! 纯 Rust PDF 查看器：连续滚动，视口内的页才光栅化。
//!
//! 每个打开的 PDF 配一条常驻渲染线程。线程持有 `Pdf` 与 `RenderCache`，这两样
//! 都搬不动：`RenderCache` 内部是 `Rc`，`Pages<'a>` 又借用 `Pdf`。所以文件读一
//! 次、解析一次、字体与图像缓存一次，之后只在这条线程上光栅化；主线程只负责上
//! 传纹理和处理交互。
//!
//! 打开时先取全部页面的点尺寸——`hayro` 是惰性解析，20MB / 138 页的文件也只要
//! 0.14ms——滚动条因此一开始就是准的，不会边渲染边跳。
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
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

pub(crate) type PdfKey = u64;

const MIN_ZOOM: f32 = 0.5;
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

/// 主线程发给渲染线程的指令：「现在要这些页」。
/// 只有最新一条有意义，线程会把积压的旧指令直接丢掉。
struct PdfWant {
    generation: u64,
    /// 逐页的目标像素宽度。横页竖页混排时每页并不相同，不能共用一个宽度。
    pages: Vec<(usize, u16)>,
}

/// PDF 标签要交给应用外壳执行的动作。渲染请求不走这里——会话自己有指令通道。
pub(crate) enum PdfAction {
    OpenExternal(PathBuf),
    Reveal(PathBuf),
    /// 送默认打印机打一份。多份带不同份号的成品由导出阶段生成，不走打印机份数。
    Print(PathBuf),
}

/// 一页的纹理槽位。
#[derive(Default)]
struct PageSlot {
    texture: Option<egui::TextureHandle>,
    /// `texture` 的光栅化宽度，用来判断要不要按新宽度重渲染。
    width: u16,
    /// 已提交给渲染线程、还没回来。
    requested: bool,
    /// 最近一次出现在视口里的帧号，淘汰时按它排序。
    last_used: u64,
    bytes: usize,
}

/// 一份打开的 PDF 的视图状态。
pub(crate) struct PdfSession {
    pub(crate) key: PdfKey,
    path: PathBuf,
    title: String,
    /// 结果通道，交给渲染线程用。
    results: Sender<WorkerResult>,
    /// 渲染线程的指令口。首帧才启动线程，那时才拿得到 `egui::Context`。
    commands: Option<Sender<PdfWant>>,
    /// 每页的点尺寸。空表示还没打开完。
    page_sizes: Vec<(f32, f32)>,
    slots: Vec<PageSlot>,
    zoom: f32,
    fit_width: bool,
    /// 换宽度时 +1，迟到的旧宽度结果靠它作废。
    generation: u64,
    /// 已经提交给渲染线程的基准宽度。
    render_width: u16,
    /// 防抖：正在等待稳定的新宽度，以及它出现的时刻。
    pending_width: u16,
    width_changed_at: Option<f64>,
    /// 工具栏显示用，由滚动位置推出。
    current_page: usize,
    scroll_to: Option<usize>,
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
            commands: None,
            page_sizes: Vec::new(),
            slots: Vec::new(),
            zoom: 1.0,
            // 默认按 100% 原尺寸显示，和纸面一比一；适合宽度留给用户按需切换。
            fit_width: false,
            generation: 0,
            render_width: 0,
            pending_width: 0,
            width_changed_at: None,
            current_page: 0,
            scroll_to: None,
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
            return self.commands.is_some();
        }
        self.slots.iter().any(|slot| slot.requested)
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
                slot.requested = false;
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
                self.commands = None;
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
        if self.commands.is_some() || self.error.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.commands = Some(tx);
        let key = self.key;
        let path = self.path.clone();
        let results = self.results.clone();
        let ctx = ctx.clone();
        thread::spawn(move || worker(key, path, rx, results, ctx));
    }

    fn page_count(&self) -> usize {
        self.page_sizes.len()
    }

    /// 某一页在当前缩放下的显示尺寸（点）。按页取真实宽高，横页和竖页混排也对。
    fn page_display_size(&self, index: usize, available: f32) -> egui::Vec2 {
        let (width, height) = self.page_sizes[index];
        let display = if self.fit_width {
            (available - SIDE_MARGIN * 2.0).max(120.0)
        } else {
            width * self.zoom
        };
        egui::vec2(display, display * height / width.max(1.0))
    }

    /// 全部页面的显示尺寸、顶部偏移与内容总高。
    fn layout(&self, available: f32) -> (Vec<egui::Vec2>, Vec<f32>, f32) {
        let mut sizes = Vec::with_capacity(self.page_count());
        let mut tops = Vec::with_capacity(self.page_count());
        let mut y = PAGE_GAP;
        for index in 0..self.page_count() {
            let size = self.page_display_size(index, available);
            tops.push(y);
            y += size.y + PAGE_GAP;
            sizes.push(size);
        }
        (sizes, tops, y)
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<PdfAction>) {
        ui.horizontal(|ui| {
            let count = self.page_count();
            let has_previous = count > 0 && self.current_page > 0;
            if theme::icon_button_enabled(ui, has_previous, theme::Icon::ChevronUp, "上一页")
                .clicked()
            {
                self.scroll_to = Some(self.current_page.saturating_sub(1));
            }

            let has_next = count > 0 && self.current_page + 1 < count;
            if theme::icon_button_enabled(ui, has_next, theme::Icon::ChevronDown, "下一页")
                .clicked()
            {
                self.scroll_to = Some(self.current_page + 1);
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
            if theme::icon_button_enabled(ui, self.zoom > MIN_ZOOM, theme::Icon::ZoomOut, "缩小")
                .on_hover_text("缩小（也可按住 Ctrl 滚动滚轮）")
                .clicked()
            {
                self.set_zoom((self.zoom - ZOOM_STEP).max(MIN_ZOOM));
            }
            if theme::icon_button_enabled(ui, self.zoom < MAX_ZOOM, theme::Icon::ZoomIn, "放大")
                .on_hover_text("放大（也可按住 Ctrl 滚动滚轮）")
                .clicked()
            {
                self.set_zoom((self.zoom + ZOOM_STEP).min(MAX_ZOOM));
            }
            ui.label(if self.fit_width {
                "适合宽度".to_string()
            } else {
                format!("{:.0}%", self.zoom * 100.0)
            });
            if ui.selectable_label(self.fit_width, "适合宽度").clicked() && !self.fit_width {
                self.fit_width = true;
                // 缩放变了版式跟着变，把当前页重新拉回视野，不然会滚到别处。
                self.scroll_to = Some(self.current_page);
            }

            ui.separator();
            // 打印能力每帧探测一次代价太高（要扫 PATH），进程内只算一次。
            let can_print = *PRINTING_AVAILABLE.get_or_init(crate::app::printing_available);
            let print_button = ui.add_enabled(
                can_print,
                theme::icon_text_button(theme::Icon::Print, "打印"),
            );
            let print_button = if can_print {
                print_button.on_hover_text("送默认打印机打印一份")
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
        self.fit_width = false;
        self.zoom = zoom;
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
            self.set_zoom((self.zoom * zoom_delta).clamp(MIN_ZOOM, MAX_ZOOM));
        }

        let available = ui.available_width();
        let pixels_per_point = ui.ctx().pixels_per_point().max(1.0);
        self.settle_width(ui.ctx(), available, pixels_per_point);

        let (sizes, tops, total) = self.layout(available);
        // 100% 或放大时页面可能比窗口宽，内容区得按最宽的那页撑开，横向才滚得动。
        let content_width = sizes
            .iter()
            .map(|size| size.x + SIDE_MARGIN * 2.0)
            .fold(available, f32::max);
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
        let generation = self.generation;
        let slots = &mut self.slots;
        let (wanted, visible_top) = scroll
            .show(ui, |ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(content_width, total), egui::Sense::hover());
                let clip = ui.clip_rect();
                // 视口上下各多备一屏，滚起来才不总看见占位白页。
                let prefetch = clip.expand2(egui::vec2(0.0, clip.height()));
                let painter = ui.painter();
                let mut wanted = Vec::new();
                let mut visible_top = None;

                for (index, (size, top)) in sizes.iter().zip(&tops).enumerate() {
                    let left = rect.left() + ((content_width - size.x) / 2.0).max(0.0);
                    let page_rect =
                        egui::Rect::from_min_size(egui::pos2(left, rect.top() + top), *size);
                    if !page_rect.intersects(prefetch) {
                        continue;
                    }
                    let slot = &mut slots[index];
                    slot.last_used = frame;

                    // 这一页该有多少像素宽。分辨率不够就排队重渲染。
                    let target = render_width(size.x, pixels_per_point);
                    let stale = slot.texture.is_none()
                        || slot.width.abs_diff(target) >= RERENDER_WIDTH_DELTA;
                    if stale && !slot.requested {
                        slot.requested = true;
                        wanted.push((index, target));
                    }

                    if !page_rect.intersects(clip) {
                        continue;
                    }
                    if visible_top.is_none() {
                        visible_top = Some(index);
                    }
                    paint_page(painter, page_rect, index, slot);
                }
                (wanted, visible_top)
            })
            .inner;

        if let Some(index) = visible_top {
            self.current_page = index;
        }
        self.evict();
        self.request(generation, wanted);
    }

    /// 宽度防抖。拖窗口的过程中先用已有纹理缩放顶着，稳定下来才换一代重渲染。
    fn settle_width(&mut self, ctx: &egui::Context, available: f32, pixels_per_point: f32) {
        let base = self.page_display_size(0, available).x;
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
        for slot in &mut self.slots {
            slot.requested = false;
        }
    }

    /// 把这一帧要的页发给渲染线程。线程只认最新一条，不必在这里排队。
    fn request(&mut self, generation: u64, wanted: Vec<(usize, u16)>) {
        if wanted.is_empty() {
            return;
        }
        let Some(commands) = &self.commands else {
            return;
        };
        if commands
            .send(PdfWant {
                generation,
                pages: wanted,
            })
            .is_err()
        {
            // 线程没了（多半是打开阶段就失败），别再标记等待，否则会一直转圈。
            self.commands = None;
            for slot in &mut self.slots {
                slot.requested = false;
            }
        }
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

/// 渲染线程主体：解析一次、缓存一次，之后按主线程给的页列表光栅化。
fn worker(
    key: PdfKey,
    path: PathBuf,
    commands: Receiver<PdfWant>,
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
        Ok(bytes) => bytes,
        Err(error) => {
            send(PdfMessage::Failed(format!(
                "无法读取 {}：{error}",
                path.display()
            )));
            return;
        }
    };
    let pdf = match Pdf::new(bytes) {
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

    // 字体与图像的解码结果都落在这里。hayro 的建议就是每份 PDF 建一个、全程复用；
    // 每页新建一个等于每翻一页重新解码一遍嵌入的中文字体。
    let cache = RenderCache::new();
    let mut next: Option<PdfWant> = None;
    loop {
        let mut want = match next.take() {
            Some(want) => want,
            None => match commands.recv() {
                Ok(want) => want,
                // 发送端随会话一起析构，标签关了就退出。
                Err(_) => return,
            },
        };
        // 积压的指令里只有最后一条还算数。
        while let Ok(newer) = commands.try_recv() {
            want = newer;
        }

        for &(index, width) in &want.pages {
            // 每页之间回头看一眼：用户已经滚走了就立刻改道，不把整批渲染完。
            if let Ok(newer) = commands.try_recv() {
                next = Some(newer);
                break;
            }
            let Some(page) = pages.get(index) else {
                continue;
            };
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
            let image = egui::ColorImage::from_rgba_premultiplied(
                [usize::from(pixmap.width()), usize::from(pixmap.height())],
                pixmap.data_as_u8_slice(),
            );
            if !send(PdfMessage::Page {
                generation: want.generation,
                index,
                width,
                image,
            }) {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let (tx, rx) = mpsc::channel();
        let (_commands, command_rx) = mpsc::channel();
        thread::spawn(move || {
            worker(1, path, command_rx, tx, egui::Context::default());
        });

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

        let (tx, rx) = mpsc::channel();
        let (commands, command_rx) = mpsc::channel();
        thread::spawn(move || {
            worker(1, path, command_rx, tx, egui::Context::default());
        });
        commands
            .send(PdfWant {
                generation: 7,
                pages: vec![(0, 400)],
            })
            .unwrap();

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

    /// 读不到文件要给出中文提示，而不是让标签一直转圈。
    #[test]
    fn missing_pdf_reports_readable_error() {
        let (tx, rx) = mpsc::channel();
        let (_commands, command_rx) = mpsc::channel();
        thread::spawn(move || {
            worker(
                1,
                PathBuf::from("definitely-missing.pdf"),
                command_rx,
                tx,
                egui::Context::default(),
            );
        });

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
        let (tx, rx) = mpsc::channel();
        let (commands, command_rx) = mpsc::channel();
        thread::spawn(move || {
            worker(1, path, command_rx, tx, egui::Context::default());
        });

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
            .send(PdfWant {
                generation: 1,
                pages: (0..count).map(|index| (index, 900)).collect(),
            })
            .unwrap();
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
