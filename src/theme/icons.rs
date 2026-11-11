//! 图标枚举、品牌图标与图标加载。
//!
//! 由 src/theme.rs 拆分而来：本文件是模块 `theme::icons`，与其它子模块共享
//! `theme` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::theme::{current, mdex, omarchy, smartisan};
use eframe::egui;
use eframe::egui::{Color32, CornerRadius};

/// 界面中反复出现的紧凑操作图标。资源来自 Lucide 1.28.0（ISC）与
/// Tabler Icons 3.46.0（MIT，只取 Lucide 没有的三个文件类型图标），
/// 两套的画法一致：24×24 画布、2px 圆头描边、`currentColor`。
/// 以 SVG 原始字节随应用编译，不依赖外部文件或运行时网络。
#[derive(Debug, Clone, Copy)]
pub enum Icon {
    BrandMark,
    AlignCenter,
    AlignLeft,
    AlignRight,
    Archive,
    ArrowDown,
    ArrowUp,
    ArrowUpDown,
    Bold,
    BookmarkCheck,
    Book,
    Braces,
    Building,
    Calendar,
    Check,
    ChevronDown,
    ChevronRight,
    ChevronUp,
    Collapse,
    Columns,
    Compare,
    Copy,
    Edit,
    Eraser,
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
    Globe,
    Grip,
    Hash,
    Heading,
    HelpCircle,
    History,
    Library,
    List,
    ListOrdered,
    Lock,
    Menu,
    Open,
    Package,
    PackageOpen,
    PanelClose,
    PanelOpen,
    Paperclip,
    Palette,
    PencilLine,
    Phone,
    Plus,
    #[cfg(feature = "ai-workbench")]
    PlugZap,
    Print,
    Publish,
    Quote,
    Refresh,
    RemoveFormatting,
    Reveal,
    RotateCcw,
    Rows,
    Search,
    SearchClear,
    Save,
    Settings,
    Shield,
    Shirt,
    Sparkles,
    Square,
    SquareCheck,
    Table,
    Trash,
    TriangleAlert,
    Type,
    Undo,
    User,
    UserPlus,
    WandSparkles,
    X,
    ZoomIn,
    ZoomOut,
}

impl Icon {
    pub(crate) fn source(self) -> (&'static str, &'static [u8]) {
        match self {
            Self::BrandMark => (
                "brand-mark",
                include_bytes!("../../assets/icons/brand-mark.svg"),
            ),
            Self::AlignCenter => (
                "align-center",
                include_bytes!("../../assets/icons/align-center.svg"),
            ),
            Self::AlignLeft => (
                "align-left",
                include_bytes!("../../assets/icons/align-left.svg"),
            ),
            Self::AlignRight => (
                "align-right",
                include_bytes!("../../assets/icons/align-right.svg"),
            ),
            Self::Archive => ("archive", include_bytes!("../../assets/icons/archive.svg")),
            Self::ArrowDown => (
                "arrow-down",
                include_bytes!("../../assets/icons/arrow-down.svg"),
            ),
            Self::ArrowUp => (
                "arrow-up",
                include_bytes!("../../assets/icons/arrow-up.svg"),
            ),
            Self::ArrowUpDown => (
                "arrow-up-down",
                include_bytes!("../../assets/icons/arrow-up-down.svg"),
            ),
            Self::Bold => ("bold", include_bytes!("../../assets/icons/bold.svg")),
            Self::BookmarkCheck => (
                "bookmark-check",
                include_bytes!("../../assets/icons/bookmark-check.svg"),
            ),
            Self::Book => (
                "book-open",
                include_bytes!("../../assets/icons/book-open.svg"),
            ),
            Self::Braces => ("braces", include_bytes!("../../assets/icons/braces.svg")),
            Self::Building => (
                "building-2",
                include_bytes!("../../assets/icons/building-2.svg"),
            ),
            Self::Calendar => (
                "calendar",
                include_bytes!("../../assets/icons/calendar.svg"),
            ),
            Self::Check => ("check", include_bytes!("../../assets/icons/check.svg")),
            Self::ChevronDown => (
                "chevron-down",
                include_bytes!("../../assets/icons/chevron-down.svg"),
            ),
            Self::ChevronRight => (
                "chevron-right",
                include_bytes!("../../assets/icons/chevron-right.svg"),
            ),
            Self::ChevronUp => (
                "chevron-up",
                include_bytes!("../../assets/icons/chevron-up.svg"),
            ),
            Self::Collapse => (
                "minimize-2",
                include_bytes!("../../assets/icons/minimize-2.svg"),
            ),
            Self::Columns => (
                "columns-3",
                include_bytes!("../../assets/icons/columns-3.svg"),
            ),
            Self::Compare => (
                "arrow-left-right",
                include_bytes!("../../assets/icons/arrow-left-right.svg"),
            ),
            Self::Copy => ("copy", include_bytes!("../../assets/icons/copy.svg")),
            Self::Edit => ("pencil", include_bytes!("../../assets/icons/pencil.svg")),
            Self::Eraser => ("eraser", include_bytes!("../../assets/icons/eraser.svg")),
            Self::Expand => (
                "maximize-2",
                include_bytes!("../../assets/icons/maximize-2.svg"),
            ),
            Self::Eye => ("eye", include_bytes!("../../assets/icons/eye.svg")),
            Self::FileDown => (
                "file-down",
                include_bytes!("../../assets/icons/file-down.svg"),
            ),
            Self::FilePlus => (
                "file-plus-2",
                include_bytes!("../../assets/icons/file-plus-2.svg"),
            ),
            Self::FileTypeDoc => (
                "file-type-doc",
                include_bytes!("../../assets/icons/file-type-doc.svg"),
            ),
            Self::FileTypePdf => (
                "file-type-pdf",
                include_bytes!("../../assets/icons/file-type-pdf.svg"),
            ),
            Self::FileUp => ("file-up", include_bytes!("../../assets/icons/file-up.svg")),
            Self::FitWidth => (
                "move-horizontal",
                include_bytes!("../../assets/icons/move-horizontal.svg"),
            ),
            Self::Folder => ("folder", include_bytes!("../../assets/icons/folder.svg")),
            Self::FolderPlus => (
                "folder-plus",
                include_bytes!("../../assets/icons/folder-plus.svg"),
            ),
            Self::GitCommit => (
                "git-commit-horizontal",
                include_bytes!("../../assets/icons/git-commit-horizontal.svg"),
            ),
            Self::Globe => ("globe", include_bytes!("../../assets/icons/globe.svg")),
            Self::Grip => (
                "grip-vertical",
                include_bytes!("../../assets/icons/grip-vertical.svg"),
            ),
            Self::Hash => ("hash", include_bytes!("../../assets/icons/hash.svg")),
            Self::Heading => ("heading", include_bytes!("../../assets/icons/heading.svg")),
            Self::HelpCircle => (
                "circle-help",
                include_bytes!("../../assets/icons/circle-help.svg"),
            ),
            Self::History => ("history", include_bytes!("../../assets/icons/history.svg")),
            Self::Library => ("library", include_bytes!("../../assets/icons/library.svg")),
            Self::List => ("list", include_bytes!("../../assets/icons/list.svg")),
            Self::ListOrdered => (
                "list-ordered",
                include_bytes!("../../assets/icons/list-ordered.svg"),
            ),
            Self::Lock => ("lock", include_bytes!("../../assets/icons/lock.svg")),
            Self::Menu => ("menu", include_bytes!("../../assets/icons/menu.svg")),
            Self::Open => (
                "external-link",
                include_bytes!("../../assets/icons/external-link.svg"),
            ),
            Self::Package => ("package", include_bytes!("../../assets/icons/package.svg")),
            Self::PackageOpen => (
                "package-open",
                include_bytes!("../../assets/icons/package-open.svg"),
            ),
            Self::PanelClose => (
                "panel-left-close",
                include_bytes!("../../assets/icons/panel-left-close.svg"),
            ),
            Self::PanelOpen => (
                "panel-left-open",
                include_bytes!("../../assets/icons/panel-left-open.svg"),
            ),
            Self::Paperclip => (
                "paperclip",
                include_bytes!("../../assets/icons/paperclip.svg"),
            ),
            Self::Palette => ("palette", include_bytes!("../../assets/icons/palette.svg")),
            Self::PencilLine => (
                "pencil-line",
                include_bytes!("../../assets/icons/pencil-line.svg"),
            ),
            Self::Phone => ("phone", include_bytes!("../../assets/icons/phone.svg")),
            Self::Plus => ("plus", include_bytes!("../../assets/icons/plus.svg")),
            #[cfg(feature = "ai-workbench")]
            Self::PlugZap => (
                "plug-zap",
                include_bytes!("../../assets/icons/plug-zap.svg"),
            ),
            Self::Print => ("printer", include_bytes!("../../assets/icons/printer.svg")),
            Self::Publish => ("send", include_bytes!("../../assets/icons/send.svg")),
            Self::Quote => ("quote", include_bytes!("../../assets/icons/quote.svg")),
            Self::Refresh => (
                "refresh-cw",
                include_bytes!("../../assets/icons/refresh-cw.svg"),
            ),
            Self::RemoveFormatting => (
                "remove-formatting",
                include_bytes!("../../assets/icons/remove-formatting.svg"),
            ),
            Self::Reveal => (
                "folder-search",
                include_bytes!("../../assets/icons/folder-search.svg"),
            ),
            Self::RotateCcw => (
                "rotate-ccw",
                include_bytes!("../../assets/icons/rotate-ccw.svg"),
            ),
            Self::Rows => ("rows-3", include_bytes!("../../assets/icons/rows-3.svg")),
            Self::Search => ("search", include_bytes!("../../assets/icons/search.svg")),
            Self::SearchClear => (
                "search-x",
                include_bytes!("../../assets/icons/search-x.svg"),
            ),
            Self::Save => ("save", include_bytes!("../../assets/icons/save.svg")),
            Self::Settings => (
                "settings-2",
                include_bytes!("../../assets/icons/settings-2.svg"),
            ),
            Self::Shield => ("shield", include_bytes!("../../assets/icons/shield.svg")),
            Self::Shirt => ("shirt", include_bytes!("../../assets/icons/shirt.svg")),
            Self::Sparkles => (
                "sparkles",
                include_bytes!("../../assets/icons/sparkles.svg"),
            ),
            Self::Square => ("square", include_bytes!("../../assets/icons/square.svg")),
            Self::SquareCheck => (
                "square-check-big",
                include_bytes!("../../assets/icons/square-check-big.svg"),
            ),
            Self::Table => ("table", include_bytes!("../../assets/icons/table.svg")),
            Self::Trash => ("trash-2", include_bytes!("../../assets/icons/trash-2.svg")),
            Self::TriangleAlert => (
                "triangle-alert",
                include_bytes!("../../assets/icons/triangle-alert.svg"),
            ),
            Self::Type => ("type", include_bytes!("../../assets/icons/type.svg")),
            Self::Undo => ("undo-2", include_bytes!("../../assets/icons/undo-2.svg")),
            Self::User => ("user", include_bytes!("../../assets/icons/user.svg")),
            Self::UserPlus => (
                "user-plus",
                include_bytes!("../../assets/icons/user-plus.svg"),
            ),
            Self::WandSparkles => (
                "wand-sparkles",
                include_bytes!("../../assets/icons/wand-sparkles.svg"),
            ),
            Self::X => ("x", include_bytes!("../../assets/icons/x.svg")),
            Self::ZoomIn => ("zoom-in", include_bytes!("../../assets/icons/zoom-in.svg")),
            Self::ZoomOut => (
                "zoom-out",
                include_bytes!("../../assets/icons/zoom-out.svg"),
            ),
        }
    }

    pub(crate) fn image(self) -> egui::Image<'static> {
        self.image_sized(16.0)
    }

    /// 指定边长的图标。文件类型图标里嵌着 PDF/DOC 这样的字母，16 px 下笔画糊成一团，
    /// 这类图标单独放大几个像素。
    pub(crate) fn image_sized(self, size: f32) -> egui::Image<'static> {
        let (name, bytes) = self.source();
        egui::Image::from_bytes(format!("bytes://icon/{name}.svg"), bytes)
            .fit_to_exact_size(egui::vec2(size, size))
    }
}

/// 直角、细描边、不投影的平面外壳，配色仍由统一主题接口提供。MDEX 两套与
/// 「跟随 Omarchy」（Hyprland 默认是直角细线边框）用它。
pub(crate) fn square_chrome() -> bool {
    matches!(
        current().label,
        mdex::LABEL | mdex::DARK_LABEL | omarchy::LABEL
    )
}

pub(crate) fn chrome_radius(default: u8) -> CornerRadius {
    if smartisan::active() {
        return CornerRadius::same(default.min(4));
    }
    CornerRadius::same(if square_chrome() { 0 } else { default })
}

/// 标题栏使用单色品牌标识，颜色随界面主题变化。
pub(crate) fn brand_image(size: f32, tint: Color32) -> egui::Image<'static> {
    Icon::BrandMark.image_sized(size).tint(tint)
}

const APP_ICON_PNG: &[u8] = include_bytes!("../../assets/app-icon/app-icon-256.png");

pub(crate) fn app_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(APP_ICON_PNG).expect("embedded app icon must be a valid PNG")
}

/// 平台的主快捷键文案：macOS 用 Command，其他平台用 Ctrl。
pub(crate) fn primary_shortcut(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘{key}")
    } else {
        format!("Ctrl+{key}")
    }
}

/// 为开发运行的裸二进制设置 Dock 图标。发布包也通过 ICNS 提供系统图标。
#[cfg(target_os = "macos")]
pub(crate) fn apply_macos_dock_icon() {
    use objc2::ClassType as _;
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::{MainThreadMarker, NSData};

    // egui 的 update 在主线程执行；若嵌入到非标准宿主则安全地放弃 Dock 更新。
    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };
    let data = NSData::with_bytes(APP_ICON_PNG);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return;
    };
    let application = NSApplication::sharedApplication(main_thread);
    // SAFETY: 当前线程持有 MainThreadMarker，image 在调用期间有效。
    unsafe { application.setApplicationIconImage(Some(&image)) };
}

/// 安装 egui 的 SVG 解码器。重复调用安全，但应用启动时只需调用一次。
pub(crate) fn configure_icons(ctx: &egui::Context) {
    egui_extras::install_image_loaders(ctx);
}
