//! 主题化的界面配色与 egui 全局样式。
//!
//! 默认取色思路与 Claude 一致：奶油色纸面打底、黏土橙（clay）作唯一强调色、
//! 暖灰而非纯黑的文字。除默认外还内置多套明色主题（天青、淡紫等），设置页可
//! 随时切换。界面上所有颜色都从这里取，避免各处硬编码 RGB；公文「纸面」渲染
//! （预览、编辑区）仍按红头文件规范固定为白纸黑字，不受主题影响。

use crate::models::{FontConfig, FontRole, ThemeName};
use eframe::egui::{self, Color32, CornerRadius, Margin, Stroke};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

// ── 主题结构 ────────────────────────────────────────────────────────────────
/// Markdown 审校区的语法高亮取色。整体保持低饱和，只让结构性符号跳出来。
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MdPalette {
    /// 普通正文。
    pub body: Color32,
    /// `#` 等标记符号。
    pub marker: Color32,
    /// 文档标题 `# `。
    pub title: Color32,
    /// 各级小标题文字。
    pub heading: Color32,
    /// 加粗内容。
    pub strong: Color32,
    pub strong_bg: Color32,
    /// 列表符号。
    pub bullet: Color32,
    /// 表格竖线。
    pub table_pipe: Color32,
    /// 表格分隔行 `|---|`。
    pub table_rule: Color32,
    /// 表格单元内容。
    pub table_cell: Color32,
    /// HTML 注释、区段标记、`<div>`。
    pub comment: Color32,
    pub comment_bg: Color32,
    /// 待核实占位【…】。
    pub todo: Color32,
    pub todo_bg: Color32,
    /// 行内代码。
    pub code: Color32,
    /// 中文引号内的内容。
    pub quoted: Color32,
    /// 在公文预览里点中的那一块，在源码里对应的底色。
    pub anchor_bg: Color32,
    /// 查找条的普通命中；当前命中仍用更醒目的 `anchor_bg`。
    pub search_bg: Color32,
    /// 实时排版里独占一行的图片引用：TextEdit 无法内嵌图片，用淡底高亮为占位提示。
    pub image_bg: Color32,
}

/// 一套完整的明色界面配色。
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Theme {
    /// 设置页展示的主题名。
    pub label: &'static str,
    /// 窗口与面板的底色。
    pub canvas: Color32,
    /// 卡片、编辑区等前景纸面。
    pub surface: Color32,
    /// 次级底色：输入框、表头、分组底。
    pub surface_sunk: Color32,
    /// 悬停时的底色。
    pub surface_hover: Color32,
    /// 按下时的底色。
    pub surface_active: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    /// 主文字。
    pub text: Color32,
    /// 次级文字（说明、标签）。
    pub text_soft: Color32,
    /// 弱化文字（提示、占位）。
    pub text_muted: Color32,
    /// 强调色：主按钮、选中态、链接。
    pub accent: Color32,
    /// 主按钮悬停态：比强调色略亮，提示「可点」。
    pub accent_hover: Color32,
    pub accent_active: Color32,
    /// 强调色的淡底，用于选中项背景、行内高亮。
    pub accent_soft: Color32,
    pub warn: Color32,
    pub warn_soft: Color32,
    pub danger: Color32,
    pub danger_soft: Color32,
    pub success: Color32,
    /// 成功色的淡底，用于版本对照里新增文字的衬底。
    pub success_soft: Color32,
    pub info: Color32,
    /// Markdown 审校区语法高亮色。
    pub md: MdPalette,
}

/// 当前生效的主题。egui 的 UI 循环单线程读取，切换主题时短暂写锁一次。
static CURRENT: RwLock<Theme> = RwLock::new(Theme::claude());

impl Theme {
    /// 默认：Claude 奶油底 + 黏土橙强调。
    const fn claude() -> Self {
        Self {
            label: "Claude 奶油",
            canvas: Color32::from_rgb(0xF5, 0xF4, 0xEE),
            surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            surface_sunk: Color32::from_rgb(0xF0, 0xEE, 0xE6),
            surface_hover: Color32::from_rgb(0xE9, 0xE6, 0xDA),
            surface_active: Color32::from_rgb(0xE0, 0xDC, 0xCC),
            border: Color32::from_rgb(0xE3, 0xE0, 0xD5),
            border_strong: Color32::from_rgb(0xD1, 0xCC, 0xBC),
            text: Color32::from_rgb(0x1F, 0x1E, 0x1D),
            text_soft: Color32::from_rgb(0x45, 0x44, 0x41),
            text_muted: Color32::from_rgb(0x86, 0x84, 0x7B),
            accent: Color32::from_rgb(0xC9, 0x64, 0x42),
            accent_hover: Color32::from_rgb(0xD4, 0x74, 0x50),
            accent_active: Color32::from_rgb(0x9E, 0x4B, 0x30),
            accent_soft: Color32::from_rgb(0xF4, 0xE5, 0xDC),
            warn: Color32::from_rgb(0xA1, 0x63, 0x1C),
            warn_soft: Color32::from_rgb(0xF9, 0xF0, 0xDE),
            danger: Color32::from_rgb(0xB4, 0x46, 0x3C),
            danger_soft: Color32::from_rgb(0xFA, 0xE9, 0xE6),
            success: Color32::from_rgb(0x3C, 0x74, 0x53),
            success_soft: Color32::from_rgb(0xE7, 0xEF, 0xE9),
            info: Color32::from_rgb(0x44, 0x66, 0x8A),
            md: MdPalette {
                body: Color32::from_rgb(0x2A, 0x29, 0x27),
                marker: Color32::from_rgb(0xC4, 0x9A, 0x86),
                title: Color32::from_rgb(0xC9, 0x64, 0x42),
                heading: Color32::from_rgb(0x1A, 0x19, 0x18),
                strong: Color32::from_rgb(0x8A, 0x3F, 0x24),
                strong_bg: Color32::from_rgb(0xF4, 0xE5, 0xDC),
                bullet: Color32::from_rgb(0xC9, 0x64, 0x42),
                table_pipe: Color32::from_rgb(0xBC, 0xB7, 0xA6),
                table_rule: Color32::from_rgb(0xA8, 0xA3, 0x93),
                table_cell: Color32::from_rgb(0x33, 0x4A, 0x52),
                comment: Color32::from_rgb(0x5F, 0x7A, 0x6B),
                comment_bg: Color32::from_rgb(0xEC, 0xF1, 0xEB),
                todo: Color32::from_rgb(0xB4, 0x46, 0x3C),
                todo_bg: Color32::from_rgb(0xFA, 0xE9, 0xE6),
                code: Color32::from_rgb(0x6B, 0x57, 0x8A),
                quoted: Color32::from_rgb(0x2C, 0x53, 0x6B),
                anchor_bg: Color32::from_rgb(0xF4, 0xE5, 0xDC),
                search_bg: Color32::from_rgb(0xF6, 0xE7, 0xA9),
                image_bg: Color32::from_rgb(0xD8, 0xEA, 0xF5),
            },
        }
    }

    /// 天青：青白底 + 天蓝强调。
    const fn sky() -> Self {
        Self {
            label: "天青",
            canvas: Color32::from_rgb(0xEF, 0xF5, 0xFB),
            surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            surface_sunk: Color32::from_rgb(0xE4, 0xEE, 0xF6),
            surface_hover: Color32::from_rgb(0xD8, 0xE7, 0xF2),
            surface_active: Color32::from_rgb(0xC9, 0xDE, 0xEC),
            border: Color32::from_rgb(0xD6, 0xE2, 0xEC),
            border_strong: Color32::from_rgb(0xB7, 0xCC, 0xDC),
            text: Color32::from_rgb(0x17, 0x24, 0x2E),
            text_soft: Color32::from_rgb(0x3A, 0x4A, 0x57),
            text_muted: Color32::from_rgb(0x74, 0x86, 0x9A),
            accent: Color32::from_rgb(0x1F, 0x8A, 0xC0),
            accent_hover: Color32::from_rgb(0x37, 0xA0, 0xD6),
            accent_active: Color32::from_rgb(0x16, 0x6C, 0x9E),
            accent_soft: Color32::from_rgb(0xD8, 0xEB, 0xF7),
            warn: Color32::from_rgb(0xA1, 0x62, 0x2E),
            warn_soft: Color32::from_rgb(0xF6, 0xEB, 0xDD),
            danger: Color32::from_rgb(0xC0, 0x49, 0x3E),
            danger_soft: Color32::from_rgb(0xF9, 0xE4, 0xE1),
            success: Color32::from_rgb(0x2E, 0x7D, 0x5B),
            success_soft: Color32::from_rgb(0xDE, 0xEE, 0xE7),
            info: Color32::from_rgb(0x3D, 0x6F, 0x9E),
            md: MdPalette {
                body: Color32::from_rgb(0x20, 0x30, 0x3C),
                marker: Color32::from_rgb(0x7F, 0xA8, 0xC9),
                title: Color32::from_rgb(0x1F, 0x8A, 0xC0),
                heading: Color32::from_rgb(0x14, 0x22, 0x2C),
                strong: Color32::from_rgb(0x1B, 0x5E, 0x8E),
                strong_bg: Color32::from_rgb(0xD8, 0xEB, 0xF7),
                bullet: Color32::from_rgb(0x1F, 0x8A, 0xC0),
                table_pipe: Color32::from_rgb(0xA8, 0xC0, 0xD2),
                table_rule: Color32::from_rgb(0x93, 0xAF, 0xC3),
                table_cell: Color32::from_rgb(0x2B, 0x4A, 0x63),
                comment: Color32::from_rgb(0x5F, 0x7A, 0x6B),
                comment_bg: Color32::from_rgb(0xE7, 0xEF, 0xEB),
                todo: Color32::from_rgb(0xC0, 0x49, 0x3E),
                todo_bg: Color32::from_rgb(0xF9, 0xE4, 0xE1),
                code: Color32::from_rgb(0x4C, 0x6A, 0x9E),
                quoted: Color32::from_rgb(0x1F, 0x5B, 0x8A),
                anchor_bg: Color32::from_rgb(0xD8, 0xEB, 0xF7),
                search_bg: Color32::from_rgb(0xFF, 0xF1, 0xB8),
                image_bg: Color32::from_rgb(0xD8, 0xEB, 0xF7),
            },
        }
    }

    /// 淡紫：藕紫底 + 淡紫强调。
    const fn lilac() -> Self {
        Self {
            label: "淡紫",
            canvas: Color32::from_rgb(0xF7, 0xF4, 0xFB),
            surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            surface_sunk: Color32::from_rgb(0xF0, 0xEA, 0xF8),
            surface_hover: Color32::from_rgb(0xE7, 0xDD, 0xF4),
            surface_active: Color32::from_rgb(0xDC, 0xCC, 0xF0),
            border: Color32::from_rgb(0xE4, 0xDC, 0xEF),
            border_strong: Color32::from_rgb(0xCD, 0xBF, 0xE2),
            text: Color32::from_rgb(0x26, 0x21, 0x3A),
            text_soft: Color32::from_rgb(0x4A, 0x44, 0x62),
            text_muted: Color32::from_rgb(0x84, 0x7C, 0x9E),
            accent: Color32::from_rgb(0x8A, 0x63, 0xC9),
            accent_hover: Color32::from_rgb(0x9C, 0x78, 0xD8),
            accent_active: Color32::from_rgb(0x6E, 0x47, 0xA8),
            accent_soft: Color32::from_rgb(0xEE, 0xE3, 0xFA),
            warn: Color32::from_rgb(0xA1, 0x62, 0x2E),
            warn_soft: Color32::from_rgb(0xF6, 0xEB, 0xDD),
            danger: Color32::from_rgb(0xC0, 0x49, 0x3E),
            danger_soft: Color32::from_rgb(0xF9, 0xE4, 0xE1),
            success: Color32::from_rgb(0x3E, 0x7D, 0x5B),
            success_soft: Color32::from_rgb(0xDE, 0xEE, 0xE7),
            info: Color32::from_rgb(0x5C, 0x6F, 0x9E),
            md: MdPalette {
                body: Color32::from_rgb(0x2B, 0x25, 0x40),
                marker: Color32::from_rgb(0xA9, 0x8F, 0xCB),
                title: Color32::from_rgb(0x8A, 0x63, 0xC9),
                heading: Color32::from_rgb(0x1D, 0x18, 0x30),
                strong: Color32::from_rgb(0x7A, 0x4F, 0xB0),
                strong_bg: Color32::from_rgb(0xEE, 0xE3, 0xFA),
                bullet: Color32::from_rgb(0x8A, 0x63, 0xC9),
                table_pipe: Color32::from_rgb(0xC3, 0xB8, 0xDA),
                table_rule: Color32::from_rgb(0xB0, 0xA3, 0xCC),
                table_cell: Color32::from_rgb(0x4A, 0x3E, 0x6B),
                comment: Color32::from_rgb(0x5F, 0x7A, 0x6B),
                comment_bg: Color32::from_rgb(0xE7, 0xEF, 0xEB),
                todo: Color32::from_rgb(0xC0, 0x49, 0x3E),
                todo_bg: Color32::from_rgb(0xF9, 0xE4, 0xE1),
                code: Color32::from_rgb(0x6B, 0x57, 0xA8),
                quoted: Color32::from_rgb(0x5B, 0x4A, 0x9E),
                anchor_bg: Color32::from_rgb(0xEE, 0xE3, 0xFA),
                search_bg: Color32::from_rgb(0xFF, 0xF1, 0xB8),
                image_bg: Color32::from_rgb(0xD8, 0xEB, 0xF7),
            },
        }
    }

    /// 浅绿：草绿白底 + 森林绿强调。
    const fn green() -> Self {
        Self {
            label: "浅绿",
            canvas: Color32::from_rgb(0xEF, 0xF5, 0xEE),
            surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            surface_sunk: Color32::from_rgb(0xE3, 0xEE, 0xE1),
            surface_hover: Color32::from_rgb(0xD7, 0xE7, 0xD4),
            surface_active: Color32::from_rgb(0xC8, 0xDD, 0xC4),
            border: Color32::from_rgb(0xD8, 0xE4, 0xD5),
            border_strong: Color32::from_rgb(0xB8, 0xCD, 0xB4),
            text: Color32::from_rgb(0x1C, 0x2A, 0x1E),
            text_soft: Color32::from_rgb(0x3E, 0x4F, 0x41),
            text_muted: Color32::from_rgb(0x7A, 0x8C, 0x7D),
            accent: Color32::from_rgb(0x3E, 0x8E, 0x4E),
            accent_hover: Color32::from_rgb(0x56, 0xA2, 0x62),
            accent_active: Color32::from_rgb(0x2F, 0x70, 0x40),
            accent_soft: Color32::from_rgb(0xDC, 0xED, 0xDB),
            warn: Color32::from_rgb(0xA1, 0x62, 0x2E),
            warn_soft: Color32::from_rgb(0xF6, 0xEB, 0xDD),
            danger: Color32::from_rgb(0xC0, 0x49, 0x3E),
            danger_soft: Color32::from_rgb(0xF9, 0xE4, 0xE1),
            success: Color32::from_rgb(0x2E, 0x7D, 0x5B),
            success_soft: Color32::from_rgb(0xDE, 0xEE, 0xE7),
            info: Color32::from_rgb(0x3D, 0x6F, 0x6E),
            md: MdPalette {
                body: Color32::from_rgb(0x21, 0x30, 0x24),
                marker: Color32::from_rgb(0x7F, 0xA8, 0x7F),
                title: Color32::from_rgb(0x3E, 0x8E, 0x4E),
                heading: Color32::from_rgb(0x15, 0x23, 0x18),
                strong: Color32::from_rgb(0x2F, 0x7A, 0x3F),
                strong_bg: Color32::from_rgb(0xDC, 0xED, 0xDB),
                bullet: Color32::from_rgb(0x3E, 0x8E, 0x4E),
                table_pipe: Color32::from_rgb(0xA9, 0xC4, 0xA7),
                table_rule: Color32::from_rgb(0x95, 0xB3, 0x92),
                table_cell: Color32::from_rgb(0x2E, 0x4A, 0x38),
                comment: Color32::from_rgb(0x5F, 0x7A, 0x6B),
                comment_bg: Color32::from_rgb(0xE7, 0xEF, 0xEB),
                todo: Color32::from_rgb(0xC0, 0x49, 0x3E),
                todo_bg: Color32::from_rgb(0xF9, 0xE4, 0xE1),
                code: Color32::from_rgb(0x4E, 0x7A, 0x52),
                quoted: Color32::from_rgb(0x2F, 0x6B, 0x4A),
                anchor_bg: Color32::from_rgb(0xDC, 0xED, 0xDB),
                search_bg: Color32::from_rgb(0xFF, 0xF1, 0xB8),
                image_bg: Color32::from_rgb(0xD8, 0xEB, 0xF7),
            },
        }
    }
}

/// 按配置里的主题名取色板，未知名字回退默认主题。
pub fn by_name(name: ThemeName) -> Theme {
    match name {
        ThemeName::Claude => Theme::claude(),
        ThemeName::Sky => Theme::sky(),
        ThemeName::Lilac => Theme::lilac(),
        ThemeName::Green => Theme::green(),
    }
}

/// 切换并立即生效的当前主题。
pub fn set_current(name: ThemeName) {
    *CURRENT.write().unwrap() = by_name(name);
}

/// 当前主题（字段均为 `Copy`，整体拷出开销可忽略）。
pub fn current() -> Theme {
    *CURRENT.read().unwrap()
}

// ── 纸面与分隔线 ────────────────────────────────────────────────────────────
/// 窗口与面板的底色。
pub fn canvas() -> Color32 {
    current().canvas
}
/// 卡片、编辑区等前景纸面。
pub fn surface() -> Color32 {
    current().surface
}
/// 次级底色：输入框、表头、分组底。
pub fn surface_sunk() -> Color32 {
    current().surface_sunk
}
/// 悬停时的底色。
pub fn surface_hover() -> Color32 {
    current().surface_hover
}
/// 按下时的底色。
pub fn surface_active() -> Color32 {
    current().surface_active
}
pub fn border() -> Color32 {
    current().border
}
pub fn border_strong() -> Color32 {
    current().border_strong
}

// ── 文字 ────────────────────────────────────────────────────────────────────
/// 主文字。
pub fn text() -> Color32 {
    current().text
}
/// 次级文字（说明、标签）。
pub fn text_soft() -> Color32 {
    current().text_soft
}
/// 弱化文字（提示、占位）。
pub fn text_muted() -> Color32 {
    current().text_muted
}

// ── 强调与语义色 ────────────────────────────────────────────────────────────
/// 强调色：主按钮、选中态、链接。
pub fn accent() -> Color32 {
    current().accent
}
/// 主按钮悬停态：比强调色略亮，提示「可点」。
pub fn accent_hover() -> Color32 {
    current().accent_hover
}
pub fn accent_active() -> Color32 {
    current().accent_active
}
/// 强调色的淡底，用于选中项背景、行内高亮。
pub fn accent_soft() -> Color32 {
    current().accent_soft
}
pub fn warn() -> Color32 {
    current().warn
}
pub fn warn_soft() -> Color32 {
    current().warn_soft
}
pub fn danger() -> Color32 {
    current().danger
}
pub fn danger_soft() -> Color32 {
    current().danger_soft
}
pub fn success() -> Color32 {
    current().success
}
/// 成功色的淡底，用于版本对照里新增文字的衬底。
pub fn success_soft() -> Color32 {
    current().success_soft
}
pub fn info() -> Color32 {
    current().info
}

/// Markdown 审校区的语法高亮取色。整体保持低饱和，只让结构性符号跳出来。
pub mod md {
    use super::{Color32, current};

    /// 普通正文。
    pub fn body() -> Color32 {
        current().md.body
    }
    /// `#` 等标记符号。
    pub fn marker() -> Color32 {
        current().md.marker
    }
    /// 文档标题 `# `。
    pub fn title() -> Color32 {
        current().md.title
    }
    /// 各级小标题文字。
    pub fn heading() -> Color32 {
        current().md.heading
    }
    /// 加粗内容。
    pub fn strong() -> Color32 {
        current().md.strong
    }
    pub fn strong_bg() -> Color32 {
        current().md.strong_bg
    }
    /// 列表符号。
    pub fn bullet() -> Color32 {
        current().md.bullet
    }
    /// 表格竖线。
    pub fn table_pipe() -> Color32 {
        current().md.table_pipe
    }
    /// 表格分隔行 `|---|`。
    pub fn table_rule() -> Color32 {
        current().md.table_rule
    }
    /// 表格单元内容。
    pub fn table_cell() -> Color32 {
        current().md.table_cell
    }
    /// HTML 注释、区段标记、`<div>`。
    pub fn comment() -> Color32 {
        current().md.comment
    }
    pub fn comment_bg() -> Color32 {
        current().md.comment_bg
    }
    /// 待核实占位【…】。
    pub fn todo() -> Color32 {
        current().md.todo
    }
    pub fn todo_bg() -> Color32 {
        current().md.todo_bg
    }
    /// 行内代码。
    pub fn code() -> Color32 {
        current().md.code
    }
    /// 中文引号内的内容。
    pub fn quoted() -> Color32 {
        current().md.quoted
    }
    /// 在公文预览里点中的那一块，在源码里对应的底色。
    pub fn anchor_bg() -> Color32 {
        current().md.anchor_bg
    }
    /// 查找条的普通命中；当前命中仍用更醒目的 `anchor_bg`。
    pub fn search_bg() -> Color32 {
        current().md.search_bg
    }
    /// 实时排版里图片引用行的淡底（TextEdit 无法内嵌图片，作为占位提示）。
    pub fn image_bg() -> Color32 {
        current().md.image_bg
    }
}

/// 卡片外框：白纸面 + 细描边 + 圆角。
pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(surface())
        .stroke(Stroke::new(1.0, border()))
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
        widgets.inactive.weak_bg_fill = accent();
        widgets.inactive.bg_fill = accent();
        widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        widgets.hovered.weak_bg_fill = accent_hover();
        widgets.hovered.bg_fill = accent_hover();
        widgets.hovered.fg_stroke = Stroke::new(1.5, Color32::WHITE);
        widgets.active.weak_bg_fill = accent_active();
        widgets.active.bg_fill = accent_active();
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
    .stroke(Stroke::new(1.0, accent_active()))
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
        return ui.add_enabled(false, primary_button_widget(icon, label).fill(text_muted()));
    }
    accent_scope(ui, |ui| ui.add(primary_button_widget(icon, label)))
}

/// 图标与文字组合的次按钮。
pub fn secondary_icon_button(icon: Icon, label: &str) -> egui::Button<'static> {
    egui::Button::image_and_text(icon.image(), label.to_owned())
        .image_tint_follows_text_color(true)
        .fill(surface())
        .stroke(Stroke::new(1.0, border_strong()))
        .corner_radius(CornerRadius::same(7))
}

/// 图标与文字组合的警示按钮，用于需要保留明确文字的删除/清空操作。
pub fn warning_icon_button(icon: Icon, label: &str) -> egui::Button<'static> {
    egui::Button::image_and_text(
        icon.image().tint(warn()),
        egui::RichText::new(label.to_owned()).color(warn()),
    )
    .corner_radius(CornerRadius::same(7))
}

/// 已选项标签：文字在左，移除图标固定在右；未知词条沿用警示色。
pub fn removable_tag_button(label: &str, warning: bool) -> egui::Button<'static> {
    let color = if warning { warn() } else { text_soft() };
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
    dangerous: bool,
) -> egui::Response {
    let image = if dangerous {
        icon.image()
            .tint(if enabled { danger() } else { text_muted() })
    } else {
        icon.image()
    };
    let response = ui.add_enabled(
        enabled,
        egui::Button::image(image)
            .image_tint_follows_text_color(!dangerous)
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
const BRIGHT_MEDIUM_FONT_BYTES: &[u8] = include_bytes!("../font/LXGWBright-Medium.ttf");
const BRIGHT_CODE_FONT_BYTES: &[u8] = include_bytes!("../font/LXGWBrightCode-Regular.ttf");

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

/// 配置界面与公文预览的字体。`config` 里选了本机字体的位置，预览也跟着换，
/// 否则屏幕上看到的版式和编译出来的 PDF 对不上。
pub fn configure_fonts(ctx: &egui::Context, config: &FontConfig) {
    let mut fonts = egui::FontDefinitions::default();
    let bundled_fonts = crate::portable_runtime::find_font_dir();

    // 界面正文默认使用随应用内置的 LXGW Bright Medium；设置里指定了本机字体就
    // 优先用它，文件被删或读不出来时照旧回退内置。Markdown/等宽文本仍用
    // LXGW Bright Code。两个内置字体在编译前由 build.rs 从 GitHub 下载并校验。
    let bright_font = "gw-bright".to_owned();
    let bright_code_font = "gw-bright-code".to_owned();
    let ui_font_loaded = config.active_ui_font().is_some_and(|choice| {
        load_font(
            &mut fonts,
            &bright_font,
            &[PathBuf::from(choice.path.trim())],
        )
        .is_some()
    });
    if !ui_font_loaded {
        fonts.font_data.insert(
            bright_font.clone(),
            egui::FontData::from_static(BRIGHT_MEDIUM_FONT_BYTES).into(),
        );
    }
    fonts.font_data.insert(
        bright_code_font.clone(),
        egui::FontData::from_static(BRIGHT_CODE_FONT_BYTES).into(),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, bright_font.clone());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .insert(0, bright_code_font.clone());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .insert(1, bright_font);

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
    // 公文专用字体缺失时也不回退到界面字体，保持预览与界面字体相互隔离。
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

    for (family, role, bundled_file, system_candidates) in [
        (
            FONT_FANGSONG,
            FontRole::Body,
            "FangSong.ttf",
            &[
                r"C:\Windows\Fonts\simfang.ttf",
                r"C:\Windows\Fonts\simsun.ttc",
            ][..],
        ),
        (
            FONT_HEITI,
            FontRole::Heading1,
            "SimHei.ttf",
            &[
                r"C:\Windows\Fonts\simhei.ttf",
                r"C:\Windows\Fonts\msyhbd.ttc",
            ][..],
        ),
        (
            FONT_KAITI,
            FontRole::Heading2,
            "KaiTi.ttf",
            &[
                r"C:\Windows\Fonts\simkai.ttf",
                r"C:\Windows\Fonts\simfang.ttf",
            ][..],
        ),
        (
            FONT_BIAOSONG,
            FontRole::Title,
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
        let mut candidates =
            font_candidates(bundled_fonts.as_deref(), bundled_file, system_candidates);
        // 设置里指定了本机字体就排在最前；读不出来（文件被删）时照旧回退。
        if let Some(choice) = config.active(role) {
            candidates.insert(0, PathBuf::from(choice.path.trim()));
        }
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
    visuals.panel_fill = canvas();
    visuals.window_fill = surface();
    visuals.window_stroke = Stroke::new(1.0, border());
    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.menu_corner_radius = CornerRadius::same(8);
    visuals.faint_bg_color = surface_sunk();
    visuals.extreme_bg_color = surface();
    visuals.text_edit_bg_color = Some(surface());
    visuals.code_bg_color = surface_sunk();
    visuals.hyperlink_color = accent();
    visuals.warn_fg_color = warn();
    visuals.error_fg_color = danger();
    visuals.selection = egui::style::Selection {
        bg_fill: accent_soft(),
        stroke: Stroke::new(1.0, accent_active()),
    };
    visuals.text_cursor.stroke = Stroke::new(2.0, accent());
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
    widgets.noninteractive.bg_fill = canvas();
    widgets.noninteractive.weak_bg_fill = canvas();
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, border());
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, text());
    widgets.noninteractive.corner_radius = CornerRadius::same(7);

    widgets.inactive.bg_fill = surface_sunk();
    widgets.inactive.weak_bg_fill = surface_sunk();
    widgets.inactive.bg_stroke = Stroke::new(1.0, border());
    widgets.inactive.fg_stroke = Stroke::new(1.0, text_soft());
    widgets.inactive.corner_radius = CornerRadius::same(7);

    widgets.hovered.bg_fill = surface_hover();
    widgets.hovered.weak_bg_fill = surface_hover();
    widgets.hovered.bg_stroke = Stroke::new(1.0, border_strong());
    widgets.hovered.fg_stroke = Stroke::new(1.5, text());
    widgets.hovered.corner_radius = CornerRadius::same(7);
    widgets.hovered.expansion = 0.0;

    widgets.active.bg_fill = surface_active();
    widgets.active.weak_bg_fill = surface_active();
    widgets.active.bg_stroke = Stroke::new(1.0, accent());
    widgets.active.fg_stroke = Stroke::new(1.5, text());
    widgets.active.corner_radius = CornerRadius::same(7);
    widgets.active.expansion = 0.0;

    widgets.open.bg_fill = surface_sunk();
    widgets.open.weak_bg_fill = surface_sunk();
    widgets.open.bg_stroke = Stroke::new(1.0, accent());
    widgets.open.fg_stroke = Stroke::new(1.0, text());
    widgets.open.corner_radius = CornerRadius::same(7);

    ctx.set_style_of(egui::Theme::Light, style);
}

#[cfg(test)]
mod tests {
    use super::configure_icons;
    use crate::models::ThemeName;
    use crate::theme::{Theme, by_name};

    #[test]
    fn configure_icons_installs_png_loader() {
        let ctx = egui::Context::default();
        configure_icons(&ctx);

        assert!(ctx.is_loader_installed(egui_extras::loaders::image_loader::ImageCrateLoader::ID));
    }

    /// 三套内置主题都必须是明色：纸面底很亮、文字为深色，保证可读性。
    #[test]
    fn all_presets_are_light_themes() {
        for name in ThemeName::ALL {
            let theme: Theme = by_name(name);
            let (r, g, b) = (theme.canvas.r(), theme.canvas.g(), theme.canvas.b());
            assert!(
                r >= 220 && g >= 220 && b >= 220,
                "{:?} 的画布底不是明色：{:02X}{:02X}{:02X}",
                name,
                r,
                g,
                b
            );
            let (tr, tg, tb) = (theme.text.r(), theme.text.g(), theme.text.b());
            assert!(
                tr < 90 && tg < 90 && tb < 100,
                "{:?} 的文字色不够深：{:02X}{:02X}{:02X}",
                name,
                tr,
                tg,
                tb
            );
            // 强调色不能是纯黑或纯白，且与深色文字保持明显区别。
            let (ar, ag, ab) = (theme.accent.r(), theme.accent.g(), theme.accent.b());
            let accent_sum = ar as u32 + ag as u32 + ab as u32;
            assert!(
                accent_sum > 60 && accent_sum < 720,
                "{:?} 的强调色不适用：{:02X}{:02X}{:02X}",
                name,
                ar,
                ag,
                ab
            );
        }
    }
}
