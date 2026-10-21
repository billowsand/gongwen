//! 主题色板（明暗/强调色/纸面家族）与全局颜色存取入口。
//!
//! 由 src/theme.rs 拆分而来：本文件是模块 `theme::palette`，与其它子模块共享
//! `theme` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::models::{PaperMode, ThemeName};
use crate::theme::{CURRENT, CURRENT_PAPER, REVISION, mdex, smartisan};
use eframe::egui::Color32;
use std::sync::atomic::Ordering;

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
}

/// 屏幕上的公文纸面家族。
///
/// 纸面只承接界面主题的明暗与冷暖，不直接复制主题的强调色，避免公文看起来
/// 像普通换肤界面。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PaperFamily {
    /// 本色：纯白纸、纯黑字。
    Original,
    /// 宣纸白：低饱和暖白。
    Parchment,
    /// 夜墨蓝：冷深色纸面。
    NightBlue,
    /// 檀黑棕：暖深色纸面。
    Sandalwood,
    /// 灰绿墨纸：MDEX 夜墨专用。
    GreenInk,
}

impl PaperFamily {
    pub(crate) const fn is_dark(self) -> bool {
        matches!(self, Self::NightBlue | Self::Sandalwood | Self::GreenInk)
    }
}

/// 一套完整的界面配色。
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Theme {
    /// 设置页展示的主题名。
    pub label: &'static str,
    /// 是否深色主题。决定 egui 走哪套内建 visuals。
    pub dark: bool,
    /// 「跟随主题」时使用的纸面家族。
    pub(crate) paper_family: PaperFamily,
    /// 用户显式选择「深色纸面」时使用的冷或暖深色家族。
    pub(crate) dark_paper_family: PaperFamily,
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

impl Theme {
    /// 默认：Claude 奶油底 + 黏土橙强调。
    pub(crate) const fn claude() -> Self {
        Self {
            label: "Claude 奶油",
            dark: false,
            paper_family: PaperFamily::Original,
            dark_paper_family: PaperFamily::Sandalwood,
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
            },
        }
    }

    // ── 以下借鉴终端配色方案：先取一整组互相协调的色相（红绿黄蓝品红青），再把
    // 它们分派到强调、语义与 Markdown 各个角色上，而不是只换掉一个强调色。

    /// 北欧：Nord。冷灰蓝打底，饱和度最低，适合整天开着。
    const fn nord() -> Self {
        Self {
            label: "北欧",
            dark: true,
            paper_family: PaperFamily::NightBlue,
            dark_paper_family: PaperFamily::NightBlue,
            canvas: Color32::from_rgb(0x2E, 0x34, 0x40),
            surface: Color32::from_rgb(0x3B, 0x42, 0x52),
            surface_sunk: Color32::from_rgb(0x29, 0x2E, 0x39),
            surface_hover: Color32::from_rgb(0x43, 0x4C, 0x5E),
            surface_active: Color32::from_rgb(0x4C, 0x56, 0x6A),
            border: Color32::from_rgb(0x3F, 0x48, 0x59),
            border_strong: Color32::from_rgb(0x4C, 0x56, 0x6A),
            text: Color32::from_rgb(0xEC, 0xEF, 0xF4),
            text_soft: Color32::from_rgb(0xD8, 0xDE, 0xE9),
            text_muted: Color32::from_rgb(0x7B, 0x87, 0x9C),
            accent: Color32::from_rgb(0x88, 0xC0, 0xD0),
            accent_hover: Color32::from_rgb(0x9B, 0xD0, 0xDE),
            accent_active: Color32::from_rgb(0x5E, 0x81, 0xAC),
            accent_soft: Color32::from_rgb(0x33, 0x41, 0x4F),
            warn: Color32::from_rgb(0xEB, 0xCB, 0x8B),
            warn_soft: Color32::from_rgb(0x3D, 0x3A, 0x2C),
            danger: Color32::from_rgb(0xBF, 0x61, 0x6A),
            danger_soft: Color32::from_rgb(0x3C, 0x2E, 0x33),
            success: Color32::from_rgb(0xA3, 0xBE, 0x8C),
            success_soft: Color32::from_rgb(0x31, 0x3D, 0x30),
            info: Color32::from_rgb(0x81, 0xA1, 0xC1),
            md: MdPalette {
                body: Color32::from_rgb(0xE5, 0xE9, 0xF0),
                marker: Color32::from_rgb(0x6D, 0x7A, 0x90),
                title: Color32::from_rgb(0x88, 0xC0, 0xD0),
                heading: Color32::from_rgb(0xEC, 0xEF, 0xF4),
                strong: Color32::from_rgb(0xEB, 0xCB, 0x8B),
                strong_bg: Color32::from_rgb(0x3D, 0x3A, 0x2C),
                bullet: Color32::from_rgb(0x81, 0xA1, 0xC1),
                table_pipe: Color32::from_rgb(0x4C, 0x56, 0x6A),
                table_rule: Color32::from_rgb(0x66, 0x72, 0x86),
                table_cell: Color32::from_rgb(0x8F, 0xBC, 0xBB),
                comment: Color32::from_rgb(0x7B, 0x87, 0x9C),
                comment_bg: Color32::from_rgb(0x33, 0x3B, 0x49),
                todo: Color32::from_rgb(0xBF, 0x61, 0x6A),
                todo_bg: Color32::from_rgb(0x3C, 0x2E, 0x33),
                code: Color32::from_rgb(0xA3, 0xBE, 0x8C),
                quoted: Color32::from_rgb(0xB4, 0x8E, 0xAD),
                anchor_bg: Color32::from_rgb(0x3A, 0x4A, 0x5C),
                search_bg: Color32::from_rgb(0x4A, 0x45, 0x2F),
            },
        }
    }

    /// 复古暗：Gruvbox Dark。暖褐底 + 橙强调，默认主题的深色亲戚。
    const fn gruvbox_dark() -> Self {
        Self {
            label: "复古暗",
            dark: true,
            paper_family: PaperFamily::Sandalwood,
            dark_paper_family: PaperFamily::Sandalwood,
            canvas: Color32::from_rgb(0x1D, 0x20, 0x21),
            surface: Color32::from_rgb(0x28, 0x28, 0x28),
            surface_sunk: Color32::from_rgb(0x32, 0x30, 0x2F),
            surface_hover: Color32::from_rgb(0x3C, 0x38, 0x36),
            surface_active: Color32::from_rgb(0x50, 0x49, 0x45),
            border: Color32::from_rgb(0x3C, 0x38, 0x36),
            border_strong: Color32::from_rgb(0x50, 0x49, 0x45),
            text: Color32::from_rgb(0xEB, 0xDB, 0xB2),
            text_soft: Color32::from_rgb(0xD5, 0xC4, 0xA1),
            text_muted: Color32::from_rgb(0x92, 0x83, 0x74),
            accent: Color32::from_rgb(0xFE, 0x80, 0x19),
            accent_hover: Color32::from_rgb(0xFF, 0x96, 0x45),
            accent_active: Color32::from_rgb(0xD6, 0x5D, 0x0E),
            accent_soft: Color32::from_rgb(0x3B, 0x2D, 0x1F),
            warn: Color32::from_rgb(0xFA, 0xBD, 0x2F),
            warn_soft: Color32::from_rgb(0x3C, 0x33, 0x20),
            danger: Color32::from_rgb(0xFB, 0x49, 0x34),
            danger_soft: Color32::from_rgb(0x3C, 0x26, 0x22),
            success: Color32::from_rgb(0xB8, 0xBB, 0x26),
            success_soft: Color32::from_rgb(0x32, 0x34, 0x1C),
            info: Color32::from_rgb(0x83, 0xA5, 0x98),
            md: MdPalette {
                body: Color32::from_rgb(0xEB, 0xDB, 0xB2),
                marker: Color32::from_rgb(0x92, 0x83, 0x74),
                title: Color32::from_rgb(0xFE, 0x80, 0x19),
                heading: Color32::from_rgb(0xFB, 0xF1, 0xC7),
                strong: Color32::from_rgb(0xFA, 0xBD, 0x2F),
                strong_bg: Color32::from_rgb(0x3C, 0x33, 0x20),
                bullet: Color32::from_rgb(0xB8, 0xBB, 0x26),
                table_pipe: Color32::from_rgb(0x50, 0x49, 0x45),
                table_rule: Color32::from_rgb(0x7C, 0x6F, 0x64),
                table_cell: Color32::from_rgb(0x83, 0xA5, 0x98),
                comment: Color32::from_rgb(0x92, 0x83, 0x74),
                comment_bg: Color32::from_rgb(0x32, 0x30, 0x2F),
                todo: Color32::from_rgb(0xFB, 0x49, 0x34),
                todo_bg: Color32::from_rgb(0x3C, 0x26, 0x22),
                code: Color32::from_rgb(0x8E, 0xC0, 0x7C),
                quoted: Color32::from_rgb(0xD3, 0x86, 0x9B),
                anchor_bg: Color32::from_rgb(0x45, 0x3A, 0x25),
                search_bg: Color32::from_rgb(0x4A, 0x42, 0x20),
            },
        }
    }
}

/// 按配置里的主题名取色板；旧主题名在配置反序列化时回退默认主题。
pub(crate) fn by_name(name: ThemeName) -> Theme {
    match name {
        ThemeName::Claude => Theme::claude(),
        ThemeName::Mdex => mdex::palette(),
        ThemeName::Smartisan => smartisan::palette(),
        ThemeName::MdexDark => mdex::dark_palette(),
        ThemeName::Nord => Theme::nord(),
        ThemeName::GruvboxDark => Theme::gruvbox_dark(),
    }
}

/// 切换并立即生效的当前主题。
pub(crate) fn set_current(name: ThemeName) {
    *CURRENT
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = by_name(name);
    REVISION.fetch_add(1, Ordering::Relaxed);
}

/// 当前主题（字段均为 `Copy`，整体拷出开销可忽略）。
pub(crate) fn current() -> Theme {
    *CURRENT
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 切换并立即生效的纸面显示模式。
pub(crate) fn set_current_paper(mode: PaperMode) {
    *CURRENT_PAPER
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = mode;
    REVISION.fetch_add(1, Ordering::Relaxed);
}

/// 当前配色版本号，供缓存了颜色的地方做缓存键。
pub(crate) fn revision() -> u64 {
    REVISION.load(Ordering::Relaxed)
}

/// 窗口与面板的底色。
pub(crate) fn canvas() -> Color32 {
    current().canvas
}

/// 卡片、编辑区等前景纸面。
pub(crate) fn surface() -> Color32 {
    current().surface
}

/// 次级底色：输入框、表头、分组底。
pub(crate) fn surface_sunk() -> Color32 {
    current().surface_sunk
}

/// 悬停时的底色。
pub(crate) fn surface_hover() -> Color32 {
    current().surface_hover
}

/// 按下时的底色。
pub(crate) fn surface_active() -> Color32 {
    current().surface_active
}

pub(crate) fn border() -> Color32 {
    current().border
}

pub(crate) fn border_strong() -> Color32 {
    current().border_strong
}

/// 主文字。
pub(crate) fn text() -> Color32 {
    current().text
}

/// 次级文字（说明、标签）。
pub(crate) fn text_soft() -> Color32 {
    current().text_soft
}

/// 弱化文字（提示、占位）。
pub(crate) fn text_muted() -> Color32 {
    current().text_muted
}

/// 强调色：主按钮、选中态、链接。
pub(crate) fn accent() -> Color32 {
    current().accent
}

/// 主按钮悬停态：比强调色略亮，提示「可点」。
pub(crate) fn accent_hover() -> Color32 {
    current().accent_hover
}

pub(crate) fn accent_active() -> Color32 {
    current().accent_active
}

/// 强调色的淡底，用于选中项背景、行内高亮。
pub(crate) fn accent_soft() -> Color32 {
    current().accent_soft
}

pub(crate) fn warn() -> Color32 {
    current().warn
}

pub(crate) fn warn_soft() -> Color32 {
    current().warn_soft
}

pub(crate) fn danger() -> Color32 {
    current().danger
}

pub(crate) fn danger_soft() -> Color32 {
    current().danger_soft
}

pub(crate) fn success() -> Color32 {
    current().success
}

/// 成功色的淡底，用于版本对照里新增文字的衬底。
pub(crate) fn success_soft() -> Color32 {
    current().success_soft
}

pub(crate) fn info() -> Color32 {
    current().info
}
