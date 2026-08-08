//! Claude 风格的配色与 egui 全局样式。
//!
//! 取色思路与 Claude 一致：奶油色纸面打底、黏土橙（clay）作唯一强调色、
//! 暖灰而非纯黑的文字。界面上所有颜色都从这里取，避免各处硬编码 RGB。

use eframe::egui::{self, Color32, CornerRadius, Margin, Stroke};
use std::path::{Path, PathBuf};

// ── 纸面与分隔线 ────────────────────────────────────────────────────────────
/// 窗口与面板的底色（奶油）。
pub const CANVAS: Color32 = Color32::from_rgb(0xF5, 0xF4, 0xEE);
/// 卡片、编辑区等前景纸面。
pub const SURFACE: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
/// 次级底色：输入框、表头、分组底。
pub const SURFACE_SUNK: Color32 = Color32::from_rgb(0xF0, 0xEE, 0xE6);
/// 悬停时的底色。
pub const SURFACE_HOVER: Color32 = Color32::from_rgb(0xE9, 0xE6, 0xDA);
/// 按下时的底色。
pub const SURFACE_ACTIVE: Color32 = Color32::from_rgb(0xE0, 0xDC, 0xCC);
pub const BORDER: Color32 = Color32::from_rgb(0xE3, 0xE0, 0xD5);
pub const BORDER_STRONG: Color32 = Color32::from_rgb(0xD1, 0xCC, 0xBC);

// ── 文字 ────────────────────────────────────────────────────────────────────
/// 主文字。
pub const TEXT: Color32 = Color32::from_rgb(0x1F, 0x1E, 0x1D);
/// 次级文字（说明、标签）。
pub const TEXT_SOFT: Color32 = Color32::from_rgb(0x45, 0x44, 0x41);
/// 弱化文字（提示、占位）。
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x86, 0x84, 0x7B);

// ── 强调与语义色 ────────────────────────────────────────────────────────────
/// 黏土橙：主按钮、选中态、链接。
pub const ACCENT: Color32 = Color32::from_rgb(0xC9, 0x64, 0x42);
/// 主按钮悬停态：比 ACCENT 略亮，提示「可点」。
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0xD4, 0x74, 0x50);
pub const ACCENT_ACTIVE: Color32 = Color32::from_rgb(0x9E, 0x4B, 0x30);
/// 强调色的淡底，用于选中项背景、行内高亮。
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(0xF4, 0xE5, 0xDC);
pub const WARN: Color32 = Color32::from_rgb(0xA1, 0x63, 0x1C);
pub const WARN_SOFT: Color32 = Color32::from_rgb(0xF9, 0xF0, 0xDE);
pub const DANGER: Color32 = Color32::from_rgb(0xB4, 0x46, 0x3C);
pub const DANGER_SOFT: Color32 = Color32::from_rgb(0xFA, 0xE9, 0xE6);
pub const SUCCESS: Color32 = Color32::from_rgb(0x3C, 0x74, 0x53);
/// 成功色的淡底，用于版本对照里新增文字的衬底。
pub const SUCCESS_SOFT: Color32 = Color32::from_rgb(0xE7, 0xEF, 0xE9);
pub const INFO: Color32 = Color32::from_rgb(0x44, 0x66, 0x8A);

/// Markdown 审校区的语法高亮取色。整体保持低饱和，只让结构性符号跳出来。
pub mod md {
    use super::*;

    /// 普通正文。
    pub const BODY: Color32 = Color32::from_rgb(0x2A, 0x29, 0x27);
    /// `#` 等标记符号。
    pub const MARKER: Color32 = Color32::from_rgb(0xC4, 0x9A, 0x86);
    /// 文档标题 `# `。
    pub const TITLE: Color32 = ACCENT;
    /// 各级小标题文字。
    pub const HEADING: Color32 = Color32::from_rgb(0x1A, 0x19, 0x18);
    /// 加粗内容。
    pub const STRONG: Color32 = Color32::from_rgb(0x8A, 0x3F, 0x24);
    pub const STRONG_BG: Color32 = ACCENT_SOFT;
    /// 列表符号。
    pub const BULLET: Color32 = ACCENT;
    /// 表格竖线。
    pub const TABLE_PIPE: Color32 = Color32::from_rgb(0xBC, 0xB7, 0xA6);
    /// 表格分隔行 `|---|`。
    pub const TABLE_RULE: Color32 = Color32::from_rgb(0xA8, 0xA3, 0x93);
    /// 表格单元内容。
    pub const TABLE_CELL: Color32 = Color32::from_rgb(0x33, 0x4A, 0x52);
    /// HTML 注释、区段标记、`<div>`。
    pub const COMMENT: Color32 = Color32::from_rgb(0x5F, 0x7A, 0x6B);
    pub const COMMENT_BG: Color32 = Color32::from_rgb(0xEC, 0xF1, 0xEB);
    /// 待核实占位【…】。
    pub const TODO: Color32 = DANGER;
    pub const TODO_BG: Color32 = DANGER_SOFT;
    /// 行内代码。
    pub const CODE: Color32 = Color32::from_rgb(0x6B, 0x57, 0x8A);
    /// 中文引号内的内容。
    pub const QUOTED: Color32 = Color32::from_rgb(0x2C, 0x53, 0x6B);
    /// 在公文预览里点中的那一块，在源码里对应的底色。
    pub const ANCHOR_BG: Color32 = ACCENT_SOFT;
    /// 查找条的普通命中；当前命中仍用更醒目的 `ANCHOR_BG`。
    pub const SEARCH_BG: Color32 = Color32::from_rgb(0xF6, 0xE7, 0xA9);
}

/// 卡片外框：白纸面 + 细描边 + 圆角。
pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(10))
}

/// 面板外框：只填底色，不描边；`margin` 为内边距。
pub fn panel(fill: Color32, margin: i8) -> egui::Frame {
    egui::Frame::new()
        .fill(fill)
        .inner_margin(Margin::symmetric(margin, (margin / 2).max(4)))
}

/// 界面中反复出现的紧凑操作图标。资源来自 Lucide 1.28.0（ISC）与
/// Tabler Icons 3.46.0（MIT，只取 Lucide 没有的三个文件类型图标），
/// 两套的画法一致：24×24 画布、2px 圆头描边、`currentColor`。
/// 以 SVG 原始字节随应用编译，不依赖外部文件或运行时网络。
#[derive(Debug, Clone, Copy)]
pub enum Icon {
    Archive,
    ArrowDown,
    ArrowUp,
    ArrowUpDown,
    BookmarkCheck,
    Book,
    Building,
    ChevronDown,
    ChevronRight,
    Collapse,
    Compare,
    Copy,
    Edit,
    Expand,
    Eye,
    FileDown,
    FilePlus,
    FileTypeDoc,
    FileTypePdf,
    FileUp,
    FitWidth,
    Folder,
    FolderPlus,
    GitCommit,
    History,
    Library,
    Menu,
    Open,
    Package,
    PackageOpen,
    PanelClose,
    PanelOpen,
    Paperclip,
    PencilLine,
    PlugZap,
    Publish,
    Refresh,
    Reveal,
    RotateCcw,
    SearchClear,
    Save,
    Settings,
    Sparkles,
    Square,
    SquareCheck,
    Tex,
    Trash,
    Undo,
    UserPlus,
    WandSparkles,
    X,
    ZoomIn,
    ZoomOut,
}

impl Icon {
    fn source(self) -> (&'static str, &'static [u8]) {
        match self {
            Self::Archive => ("archive", include_bytes!("../assets/icons/archive.svg")),
            Self::ArrowDown => (
                "arrow-down",
                include_bytes!("../assets/icons/arrow-down.svg"),
            ),
            Self::ArrowUp => ("arrow-up", include_bytes!("../assets/icons/arrow-up.svg")),
            Self::ArrowUpDown => (
                "arrow-up-down",
                include_bytes!("../assets/icons/arrow-up-down.svg"),
            ),
            Self::BookmarkCheck => (
                "bookmark-check",
                include_bytes!("../assets/icons/bookmark-check.svg"),
            ),
            Self::Book => ("book-open", include_bytes!("../assets/icons/book-open.svg")),
            Self::Building => (
                "building-2",
                include_bytes!("../assets/icons/building-2.svg"),
            ),
            Self::ChevronDown => (
                "chevron-down",
                include_bytes!("../assets/icons/chevron-down.svg"),
            ),
            Self::ChevronRight => (
                "chevron-right",
                include_bytes!("../assets/icons/chevron-right.svg"),
            ),
            Self::Collapse => (
                "minimize-2",
                include_bytes!("../assets/icons/minimize-2.svg"),
            ),
            Self::Compare => (
                "arrow-left-right",
                include_bytes!("../assets/icons/arrow-left-right.svg"),
            ),
            Self::Copy => ("copy", include_bytes!("../assets/icons/copy.svg")),
            Self::Edit => ("pencil", include_bytes!("../assets/icons/pencil.svg")),
            Self::Expand => (
                "maximize-2",
                include_bytes!("../assets/icons/maximize-2.svg"),
            ),
            Self::Eye => ("eye", include_bytes!("../assets/icons/eye.svg")),
            Self::FileDown => ("file-down", include_bytes!("../assets/icons/file-down.svg")),
            Self::FilePlus => (
                "file-plus-2",
                include_bytes!("../assets/icons/file-plus-2.svg"),
            ),
            Self::FileTypeDoc => (
                "file-type-doc",
                include_bytes!("../assets/icons/file-type-doc.svg"),
            ),
            Self::FileTypePdf => (
                "file-type-pdf",
                include_bytes!("../assets/icons/file-type-pdf.svg"),
            ),
            Self::FileUp => ("file-up", include_bytes!("../assets/icons/file-up.svg")),
            Self::FitWidth => (
                "move-horizontal",
                include_bytes!("../assets/icons/move-horizontal.svg"),
            ),
            Self::Folder => ("folder", include_bytes!("../assets/icons/folder.svg")),
            Self::FolderPlus => (
                "folder-plus",
                include_bytes!("../assets/icons/folder-plus.svg"),
            ),
            Self::GitCommit => (
                "git-commit-horizontal",
                include_bytes!("../assets/icons/git-commit-horizontal.svg"),
            ),
            Self::History => ("history", include_bytes!("../assets/icons/history.svg")),
            Self::Library => ("library", include_bytes!("../assets/icons/library.svg")),
            Self::Menu => ("menu", include_bytes!("../assets/icons/menu.svg")),
            Self::Open => (
                "external-link",
                include_bytes!("../assets/icons/external-link.svg"),
            ),
            Self::Package => ("package", include_bytes!("../assets/icons/package.svg")),
            Self::PackageOpen => (
                "package-open",
                include_bytes!("../assets/icons/package-open.svg"),
            ),
            Self::PanelClose => (
                "panel-left-close",
                include_bytes!("../assets/icons/panel-left-close.svg"),
            ),
            Self::PanelOpen => (
                "panel-left-open",
                include_bytes!("../assets/icons/panel-left-open.svg"),
            ),
            Self::Paperclip => ("paperclip", include_bytes!("../assets/icons/paperclip.svg")),
            Self::PencilLine => (
                "pencil-line",
                include_bytes!("../assets/icons/pencil-line.svg"),
            ),
            Self::PlugZap => ("plug-zap", include_bytes!("../assets/icons/plug-zap.svg")),
            Self::Publish => ("send", include_bytes!("../assets/icons/send.svg")),
            Self::Refresh => (
                "refresh-cw",
                include_bytes!("../assets/icons/refresh-cw.svg"),
            ),
            Self::Reveal => (
                "folder-search",
                include_bytes!("../assets/icons/folder-search.svg"),
            ),
            Self::RotateCcw => (
                "rotate-ccw",
                include_bytes!("../assets/icons/rotate-ccw.svg"),
            ),
            Self::SearchClear => ("search-x", include_bytes!("../assets/icons/search-x.svg")),
            Self::Save => ("save", include_bytes!("../assets/icons/save.svg")),
            Self::Settings => (
                "settings-2",
                include_bytes!("../assets/icons/settings-2.svg"),
            ),
            Self::Sparkles => ("sparkles", include_bytes!("../assets/icons/sparkles.svg")),
            Self::Square => ("square", include_bytes!("../assets/icons/square.svg")),
            Self::SquareCheck => (
                "square-check-big",
                include_bytes!("../assets/icons/square-check-big.svg"),
            ),
            Self::Tex => ("tex", include_bytes!("../assets/icons/tex.svg")),
            Self::Trash => ("trash-2", include_bytes!("../assets/icons/trash-2.svg")),
            Self::Undo => ("undo-2", include_bytes!("../assets/icons/undo-2.svg")),
            Self::UserPlus => ("user-plus", include_bytes!("../assets/icons/user-plus.svg")),
            Self::WandSparkles => (
                "wand-sparkles",
                include_bytes!("../assets/icons/wand-sparkles.svg"),
            ),
            Self::X => ("x", include_bytes!("../assets/icons/x.svg")),
            Self::ZoomIn => ("zoom-in", include_bytes!("../assets/icons/zoom-in.svg")),
            Self::ZoomOut => ("zoom-out", include_bytes!("../assets/icons/zoom-out.svg")),
        }
    }

    pub fn image(self) -> egui::Image<'static> {
        self.image_sized(16.0)
    }

    /// 指定边长的图标。文件类型图标里嵌着 PDF/DOC 这样的字母，16 px 下笔画糊成一团，
    /// 这类图标单独放大几个像素。
    pub fn image_sized(self, size: f32) -> egui::Image<'static> {
        let (name, bytes) = self.source();
        egui::Image::from_bytes(format!("bytes://icon/{name}.svg"), bytes)
            .fit_to_exact_size(egui::vec2(size, size))
    }
}

/// 安装 egui 的 SVG 解码器。重复调用安全，但应用启动时只需调用一次。
pub fn configure_icons(ctx: &egui::Context) {
    egui_extras::install_image_loaders(ctx);
}

/// 图标与文字组合的普通按钮。
pub fn icon_text_button(icon: Icon, label: &str) -> egui::Button<'static> {
    egui::Button::image_and_text(icon.image(), label.to_owned())
        .image_tint_follows_text_color(true)
        .corner_radius(CornerRadius::same(7))
}

/// 在子作用域内把按钮三态底色覆盖成橙色系（clone-on-write，退出自动还原），
/// 让 `egui::Button` 自己按状态切换底色并保留按压动效。供主按钮与需要自定义
/// 尺寸的橙色按钮共用。
pub fn accent_scope<R>(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope(|ui| {
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.inactive.weak_bg_fill = ACCENT;
        widgets.inactive.bg_fill = ACCENT;
        widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        widgets.hovered.weak_bg_fill = ACCENT_HOVER;
        widgets.hovered.bg_fill = ACCENT_HOVER;
        widgets.hovered.fg_stroke = Stroke::new(1.5, Color32::WHITE);
        widgets.active.weak_bg_fill = ACCENT_ACTIVE;
        widgets.active.bg_fill = ACCENT_ACTIVE;
        widgets.active.fg_stroke = Stroke::new(1.5, Color32::WHITE);
        add_contents(ui)
    })
    .inner
}

/// 主按钮内部的图文 Button（白字、深橙描边），不含底色——底色由 `accent_scope` 提供。
/// 需要自定义尺寸（如 `add_sized`）时，在 `accent_scope` 里 add 这个 widget。
pub fn primary_button_widget(icon: Icon, label: &str) -> egui::Button<'static> {
    egui::Button::image_and_text(
        icon.image().tint(Color32::WHITE),
        egui::RichText::new(label.to_owned())
            .color(Color32::WHITE)
            .strong(),
    )
    .stroke(Stroke::new(1.0, ACCENT_ACTIVE))
    .corner_radius(CornerRadius::same(7))
}

/// 图标与文字组合的主按钮。
///
/// 不能用 `.fill(ACCENT)` 写死背景：egui 的 `Button::fill` 会覆盖所有交互状态
/// （normal/hovered/active）的底色，按压时背景不变，看起来「像没点中」。这里改为
/// 用 `accent_scope` 按状态切换底色，保留按压动效。
pub fn primary_icon_button(ui: &mut egui::Ui, icon: Icon, label: &str) -> egui::Response {
    primary_icon_button_enabled(ui, true, icon, label)
}

/// 可禁用的主按钮。禁用时退化为灰底，不参与橙色状态切换。
pub fn primary_icon_button_enabled(
    ui: &mut egui::Ui,
    enabled: bool,
    icon: Icon,
    label: &str,
) -> egui::Response {
    if !enabled {
        return ui.add_enabled(false, primary_button_widget(icon, label).fill(TEXT_MUTED));
    }
    accent_scope(ui, |ui| ui.add(primary_button_widget(icon, label)))
}

/// 图标与文字组合的次按钮。
pub fn secondary_icon_button(icon: Icon, label: &str) -> egui::Button<'static> {
    egui::Button::image_and_text(icon.image(), label.to_owned())
        .image_tint_follows_text_color(true)
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER_STRONG))
        .corner_radius(CornerRadius::same(7))
}

/// 图标与文字组合的警示按钮，用于需要保留明确文字的删除/清空操作。
pub fn warning_icon_button(icon: Icon, label: &str) -> egui::Button<'static> {
    egui::Button::image_and_text(
        icon.image().tint(WARN),
        egui::RichText::new(label.to_owned()).color(WARN),
    )
    .corner_radius(CornerRadius::same(7))
}

/// 已选项标签：文字在左，移除图标固定在右；未知词条沿用警示色。
pub fn removable_tag_button(label: &str, warning: bool) -> egui::Button<'static> {
    let color = if warning { WARN } else { TEXT_SOFT };
    egui::Button::new(egui::RichText::new(label.to_owned()).color(color))
        .right_text(Icon::X.image())
        .image_tint_follows_text_color(true)
        .small()
}

/// 只显示图形的紧凑按钮。`label` 同时用作悬停说明与无障碍名称。
pub fn icon_button(ui: &mut egui::Ui, icon: Icon, label: &str) -> egui::Response {
    icon_button_impl(ui, true, icon, label, false)
}

/// 可禁用的图标按钮。
pub fn icon_button_enabled(
    ui: &mut egui::Ui,
    enabled: bool,
    icon: Icon,
    label: &str,
) -> egui::Response {
    icon_button_impl(ui, enabled, icon, label, false)
}

/// 危险操作图标按钮。仅改变图标颜色，不用大面积红底抢夺视觉注意力。
pub fn danger_icon_button(ui: &mut egui::Ui, icon: Icon, label: &str) -> egui::Response {
    icon_button_impl(ui, true, icon, label, true)
}

/// 可禁用的危险操作图标按钮。
pub fn danger_icon_button_enabled(
    ui: &mut egui::Ui,
    enabled: bool,
    icon: Icon,
    label: &str,
) -> egui::Response {
    icon_button_impl(ui, enabled, icon, label, true)
}

/// 顶部导航按钮：保留文字识别效率，同时用图标建立稳定的视觉锚点。
pub fn nav_button(ui: &mut egui::Ui, selected: bool, icon: Icon, label: &str) -> egui::Response {
    ui.add(
        egui::Button::image_and_text(icon.image(), label)
            .image_tint_follows_text_color(true)
            .selected(selected)
            .frame_when_inactive(selected)
            .min_size(egui::vec2(74.0, 28.0)),
    )
}

/// 编辑区右上角的视图切换：仿代码编辑器只保留图标，名称放在悬停说明中。
pub fn view_icon_button(
    ui: &mut egui::Ui,
    selected: bool,
    icon: Icon,
    label: &str,
) -> egui::Response {
    ui.add(
        egui::Button::image(icon.image())
            .image_tint_follows_text_color(true)
            .selected(selected)
            .frame_when_inactive(selected)
            .min_size(egui::vec2(30.0, 28.0))
            .corner_radius(CornerRadius::same(6)),
    )
    .on_hover_text(label)
}

fn icon_button_impl(
    ui: &mut egui::Ui,
    enabled: bool,
    icon: Icon,
    label: &str,
    danger: bool,
) -> egui::Response {
    let image = if danger {
        icon.image().tint(if enabled { DANGER } else { TEXT_MUTED })
    } else {
        icon.image()
    };
    let response = ui.add_enabled(
        enabled,
        egui::Button::image(image)
            .image_tint_follows_text_color(!danger)
            .min_size(egui::vec2(28.0, 26.0))
            .corner_radius(CornerRadius::same(6)),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    response.on_hover_text(label)
}

/// 状态小圆点，用于状态栏与列表行。
pub fn dot(ui: &mut egui::Ui, color: Color32) {
    let size = egui::vec2(8.0, 8.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

/// 一枚淡底圆角标签，用来显示模型名、状态、版本号等元信息。
pub fn chip(ui: &mut egui::Ui, text: &str, fg: Color32, bg: Color32) {
    egui::Frame::new()
        .fill(bg)
        .corner_radius(CornerRadius::same(255))
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).color(fg));
        });
}

// ── 字体 ────────────────────────────────────────────────────────────────────
/// 公文预览专用字体族的名字。与导出器一一对应：正文仿宋、一级标题黑体、
/// 二级标题楷体、文档标题小标宋。英文、数字不单独设西文字体，一律随对应的
/// 中文字体排（预览不走 Times New Roman）。
pub const FONT_FANGSONG: &str = "gw-fangsong";
pub const FONT_HEITI: &str = "gw-heiti";
pub const FONT_KAITI: &str = "gw-kaiti";
pub const FONT_BIAOSONG: &str = "gw-biaosong";

/// 应用界面使用的本地字体。通过 `include_bytes!` 编译进可执行文件，运行时不依赖
/// 字体文件或系统是否安装了该字体。
const UI_FONT_BYTES: &[u8] = include_bytes!("../font/sfss.ttf");

/// 取公文字体族。字体缺失时 `configure_fonts` 会使用独立的预览后备字体，不会报错。
pub fn official_family(name: &str) -> egui::FontFamily {
    egui::FontFamily::Name(name.into())
}

/// 依次尝试候选路径，把第一个读到的字体以 `key` 存入 `font_data`。
fn load_font(
    fonts: &mut egui::FontDefinitions,
    key: &str,
    candidates: &[PathBuf],
) -> Option<String> {
    for path in candidates {
        if let Ok(data) = std::fs::read(path) {
            fonts
                .font_data
                .insert(key.to_owned(), egui::FontData::from_owned(data).into());
            return Some(key.to_owned());
        }
    }
    None
}

fn font_candidates(bundled: Option<&Path>, file: &str, system: &[&str]) -> Vec<PathBuf> {
    bundled
        .map(|dir| dir.join(file))
        .into_iter()
        .chain(system.iter().map(PathBuf::from))
        .collect()
}

pub fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let bundled_fonts = crate::portable_runtime::find_font_dir();

    // 界面通用字体固定为随应用打包的 sfss.ttf。比例字体和等宽字体族都把它放在
    // 首位，因此普通界面、电话等旧有 monospace 文本以及 Markdown 源码保持一致；
    // 公文预览仍使用下方四个独立的专用字体族。
    let ui_font = "gw-ui".to_owned();
    fonts.font_data.insert(
        ui_font.clone(),
        egui::FontData::from_static(UI_FONT_BYTES).into(),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, ui_font.clone());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .insert(0, ui_font);

    // 公文预览专用字体缺失时使用独立的系统后备字体，不影响界面字体选择。
    let preview_fallback = load_font(
        &mut fonts,
        "gw-preview-fallback",
        &font_candidates(
            bundled_fonts.as_deref(),
            "SimSun.ttf",
            &[
                r"C:\Windows\Fonts\msyh.ttc",
                r"C:\Windows\Fonts\SourceHanSansCN-Normal.ttf",
                r"C:\Windows\Fonts\NotoSansSC-VF.ttf",
                r"C:\Windows\Fonts\simsun.ttc",
            ],
        ),
    );
    // 公文专用字体缺失时也不回退到 sfss，保持预览与界面字体相互隔离。
    let mut fallback = Vec::new();
    if let Some(font) = &preview_fallback {
        fallback.push(font.clone());
    }
    fallback.extend(
        egui::FontDefinitions::default()
            .families
            .get(&egui::FontFamily::Proportional)
            .cloned()
            .unwrap_or_default(),
    );

    for (family, bundled_file, system_candidates) in [
        (
            FONT_FANGSONG,
            "FangSong.ttf",
            &[
                r"C:\Windows\Fonts\simfang.ttf",
                r"C:\Windows\Fonts\simsun.ttc",
            ][..],
        ),
        (
            FONT_HEITI,
            "SimHei.ttf",
            &[
                r"C:\Windows\Fonts\simhei.ttf",
                r"C:\Windows\Fonts\msyhbd.ttc",
            ][..],
        ),
        (
            FONT_KAITI,
            "KaiTi.ttf",
            &[
                r"C:\Windows\Fonts\simkai.ttf",
                r"C:\Windows\Fonts\simfang.ttf",
            ][..],
        ),
        (
            FONT_BIAOSONG,
            "XiaoBiaoSong.ttf",
            &[
                r"C:\Windows\Fonts\方正小标宋简.TTF",
                r"C:\Windows\Fonts\FZXBSJW.TTF",
                r"C:\Windows\Fonts\STZHONGS.TTF",
                r"C:\Windows\Fonts\simhei.ttf",
            ][..],
        ),
    ] {
        // 每个字体族只放对应的中文字体：英文、数字也用它自带的全角字形，不把
        // Times New Roman 放在最前作西文优先（与国标一致，预览不单独设英文字体）。
        let candidates = font_candidates(bundled_fonts.as_deref(), bundled_file, system_candidates);
        let list = match load_font(&mut fonts, family, &candidates) {
            Some(key) => vec![key],
            // 一个都没装上时退回界面字体：预览的字体不对，但排版仍然成立。
            None => fallback.clone(),
        };
        fonts
            .families
            .insert(egui::FontFamily::Name(family.into()), list);
    }

    ctx.set_fonts(fonts);
}

pub fn configure_style(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Light);
    let mut style = (*ctx.style_of(egui::Theme::Light)).clone();

    style.spacing.item_spacing = egui::vec2(8.0, 7.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.menu_margin = Margin::same(6);
    style.spacing.indent = 18.0;
    style.spacing.scroll.bar_width = 9.0;
    style.spacing.scroll.floating = false;

    style.text_styles = [
        (
            egui::TextStyle::Heading,
            egui::FontId::new(18.0, egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Body,
            egui::FontId::new(14.5, egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Button,
            egui::FontId::new(14.5, egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Small,
            egui::FontId::new(14.5, egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Monospace,
            egui::FontId::new(14.5, egui::FontFamily::Monospace),
        ),
    ]
    .into();

    let visuals = &mut style.visuals;
    visuals.panel_fill = CANVAS;
    visuals.window_fill = SURFACE;
    visuals.window_stroke = Stroke::new(1.0, BORDER);
    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.menu_corner_radius = CornerRadius::same(8);
    visuals.faint_bg_color = SURFACE_SUNK;
    visuals.extreme_bg_color = SURFACE;
    visuals.text_edit_bg_color = Some(SURFACE);
    visuals.code_bg_color = SURFACE_SUNK;
    visuals.hyperlink_color = ACCENT;
    visuals.warn_fg_color = WARN;
    visuals.error_fg_color = DANGER;
    visuals.selection = egui::style::Selection {
        bg_fill: ACCENT_SOFT,
        stroke: Stroke::new(1.0, ACCENT_ACTIVE),
    };
    visuals.text_cursor.stroke = Stroke::new(2.0, ACCENT);
    visuals.striped = true;
    visuals.indent_has_left_vline = false;
    visuals.window_shadow = egui::epaint::Shadow {
        offset: [0, 2],
        blur: 12,
        spread: 0,
        color: Color32::from_black_alpha(24),
    };
    visuals.popup_shadow = visuals.window_shadow;

    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = CANVAS;
    widgets.noninteractive.weak_bg_fill = CANVAS;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    widgets.noninteractive.corner_radius = CornerRadius::same(7);

    widgets.inactive.bg_fill = SURFACE_SUNK;
    widgets.inactive.weak_bg_fill = SURFACE_SUNK;
    widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_SOFT);
    widgets.inactive.corner_radius = CornerRadius::same(7);

    widgets.hovered.bg_fill = SURFACE_HOVER;
    widgets.hovered.weak_bg_fill = SURFACE_HOVER;
    widgets.hovered.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
    widgets.hovered.fg_stroke = Stroke::new(1.5, TEXT);
    widgets.hovered.corner_radius = CornerRadius::same(7);
    widgets.hovered.expansion = 0.0;

    widgets.active.bg_fill = SURFACE_ACTIVE;
    widgets.active.weak_bg_fill = SURFACE_ACTIVE;
    widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    widgets.active.fg_stroke = Stroke::new(1.5, TEXT);
    widgets.active.corner_radius = CornerRadius::same(7);
    widgets.active.expansion = 0.0;

    widgets.open.bg_fill = SURFACE_SUNK;
    widgets.open.weak_bg_fill = SURFACE_SUNK;
    widgets.open.bg_stroke = Stroke::new(1.0, ACCENT);
    widgets.open.fg_stroke = Stroke::new(1.0, TEXT);
    widgets.open.corner_radius = CornerRadius::same(7);

    ctx.set_style_of(egui::Theme::Light, style);
}

#[cfg(test)]
mod tests {
    use super::configure_icons;

    #[test]
    fn configure_icons_installs_png_loader() {
        let ctx = egui::Context::default();
        configure_icons(&ctx);

        assert!(ctx.is_loader_installed(egui_extras::loaders::image_loader::ImageCrateLoader::ID));
    }
}
