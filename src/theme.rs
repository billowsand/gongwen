//! 主题化的界面配色与 egui 全局样式。
//!
//! 默认取色思路与 Claude 一致：奶油色纸面打底、黏土橙（clay）作唯一强调色、
//! 暖灰而非纯黑的文字。各功能域已拆分到 `theme/` 子模块（色板、外壳、图标、
//! 控件、字体、全局样式），这里保留字号/动效常量、`paper` 与 `md` 取色模块及测试。

mod mdex;
pub mod smartisan;

use crate::models::PaperMode;
use eframe::egui::Color32;
use std::sync::RwLock;
use std::sync::atomic::AtomicU64;

mod chrome;
mod fonts;
mod icons;
mod palette;
mod style;
mod widgets;

pub(crate) use chrome::{
    PANE_PADDING, PANE_RADIUS, card, clickable_card, comparison_pane, configure_comparison_scroll,
    configure_ribbon_controls, connected_ribbon_shape, float_shadow, pane, panel, paper_shadow,
    reset_window_anim, ribbon_tray_layout, window_enter_anim,
};
#[cfg(test)]
pub(crate) use fonts::font_definitions;
pub(crate) use fonts::{
    EDITOR_FONT_FAMILY, FONT_BIAOSONG, FONT_BOLD, FONT_FANGSONG, FONT_HEITI, FONT_KAITI,
    FONT_RESEARCH_HEI, FONT_RESEARCH_HEI_PLAIN, FONT_RESEARCH_TERMINAL, FONT_SONGTI,
    configure_fonts, default_ui_font_label, editor_face_family, official_family,
    preview_uses_dedicated_bold, research_family,
};
#[cfg(test)]
pub(crate) use fonts::{
    FONT_RESEARCH_BIAOSONG, FONT_RESEARCH_KAI, FONT_RESEARCH_SONG, line_gap_offset_factor,
};
pub(crate) use icons::{
    Icon, app_icon, apply_app_icon, brand_image, chrome_radius, configure_icons, is_mdex,
    primary_shortcut,
};
pub(crate) use palette::{
    MdPalette, PaperFamily, Theme, accent, accent_active, accent_hover, accent_soft, border,
    border_strong, by_name, canvas, current, danger, danger_soft, info, revision, set_current,
    set_current_paper, success, success_soft, surface, surface_active, surface_hover, surface_sunk,
    text, text_muted, text_soft, warn, warn_soft,
};
pub(crate) use style::{configure_style, reconfigure_style};
#[cfg(test)]
pub(crate) use widgets::accent_text_for;
pub(crate) use widgets::{
    accent_scope, accent_text, caption, chip, danger_icon_button, divider_v, dot, dropdown_button,
    field, hairline, icon_button, icon_button_enabled, icon_text_button, menu_item,
    menu_selectable_item, menu_text_item, metric, nav_button, notice, popup_scroll,
    primary_button_widget, primary_icon_button, primary_icon_button_enabled, removable_tag_button,
    ribbon_tab_button, secondary_icon_button, segmented, spinner, titlebar_icon_button,
    view_icon_button, warning_icon_button,
};

// ── 字号体系 ────────────────────────────────────────────────────────────────
/// 界面字号按 Windows 惯例固化：96 DPI 下 1pt ≈ 1.333px，
/// 正文 ≈ 10.5pt、菜单/状态栏/次级文字 ≈ 9pt、区块标题 ≈ 13.5pt。
/// 所有界面文字一律从这里取值，避免各处散落的魔法数字。
pub mod font_sizes {
    /// 正文、普通按钮默认字号（≈10.5pt）。
    pub const BODY: f32 = 14.0;
    /// 菜单、状态栏、标签栏、次要说明等次级文字（≈9pt）。
    pub const SMALL: f32 = 12.0;
    /// 区块标题。
    pub const HEADING: f32 = 18.0;
    /// 等宽文字（Markdown 源码、代码等），与正文同档保证可读性。
    pub const MONO: f32 = 14.0;
}

// ── 尺寸 ────────────────────────────────────────────────────────────────────
/// 一行控件的统一高度：按钮、下拉框、输入框与它们同行的标签共用这一档。
/// 起草页表单（`FORM_CONTROL_HEIGHT`）用的就是这个数，全应用保持一致。
pub const CONTROL_HEIGHT: f32 = 30.0;

// ── 动效时长 ────────────────────────────────────────────────────────────────
/// 全应用统一的动效节奏（秒），三档递进：
/// 悬停/按下等瞬时反馈用短档，选中/抽屉/弹窗等过渡用中档，
/// 大范围入场与位移用长档。各处动画一律从这里取值。
pub mod anim {
    /// 瞬时反馈：悬停、按下、行高亮（≈0.1s）。
    pub const FAST: f32 = 0.10;
    /// 常规过渡：选中、淡入淡出、抽屉、弹窗（≈0.18s）。
    pub const MEDIUM: f32 = 0.18;
    /// 长程动画：大幅位移、整块入场（≈0.25s）。
    pub const SLOW: f32 = 0.25;
}

// ── 主题结构 ────────────────────────────────────────────────────────────────

/// 当前生效的主题。egui 的 UI 循环单线程读取，切换主题时短暂写锁一次。
static CURRENT: RwLock<Theme> = RwLock::new(Theme::claude());

/// 当前的纸面显示模式，与 [`CURRENT`] 同样只在切换时写一次。
static CURRENT_PAPER: RwLock<PaperMode> = RwLock::new(PaperMode::Follow);

/// 配色版本号：主题或纸面每变一次就加一。
///
/// 审校区的语法高亮把排好版的 galley 按（文本, 宽度, 锚点…）缓存起来，而
/// galley 里的颜色是排版当时从 [`CURRENT`]/[`CURRENT_PAPER`] 取的。只看文本的
/// 缓存键认不出「字没变但颜色该变了」，切主题后编辑区会继续画上一套配色，直到
/// 用户碰一下光标或改一个字才刷新。把这个版本号也混进缓存键，任何配色变化都会
/// 让缓存自然失效。
static REVISION: AtomicU64 = AtomicU64::new(0);

/// 屏幕上的公文纸面取色。
///
/// **只作用于屏幕预览**：导出的 DOCX/PDF 由 `export` 模块另行生成，完全不读
/// 这里的颜色，因此不论用户把纸面调成什么，落到纸上的永远是白纸黑字红头。
///
/// 「跟随主题」使用本色、宣纸白、夜墨蓝与檀黑棕，另为 MDEX 夜墨
/// 提供灰绿墨纸。纸面只跟随冷暖和明暗，不复制主题强调色；正文只用黑或白，
/// 红头、红色反线与份号始终使用同一个规范红。
pub mod paper {
    use super::{CURRENT_PAPER, Color32, PaperFamily, Theme, current};
    use crate::models::PaperMode;

    /// 红头与红色反线的颜色：与 LaTeX 类里一致的纯红，所有纸面一律不变。
    const OFFICIAL_RED: Color32 = Color32::from_rgb(0xFF, 0x00, 0x00);

    /// 一套纸面取色。
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    struct Sheet {
        bg: Color32,
        ink: Color32,
        ink_muted: Color32,
        ink_faint: Color32,
        hover: Color32,
        /// 专属纸边；未指定时沿用主题的原有边框。
        border: Option<Color32>,
        shadow_alpha: u8,
    }

    /// 本色：标准白纸黑字，既是默认纸面，也是「白纸黑字」的强制结果。
    const ORIGINAL_SHEET: Sheet = Sheet {
        bg: Color32::WHITE,
        ink: Color32::BLACK,
        ink_muted: Color32::from_gray(90),
        ink_faint: Color32::from_gray(150),
        hover: Color32::from_rgb(0xF3, 0xF3, 0xF3),
        border: None,
        shadow_alpha: 18,
    };

    /// 宣纸白：暖而不黄，仍使用纯黑正文。
    const PARCHMENT_SHEET: Sheet = Sheet {
        bg: Color32::from_rgb(0xFB, 0xF8, 0xF0),
        ink: Color32::BLACK,
        ink_muted: Color32::from_rgb(0x5A, 0x58, 0x53),
        ink_faint: Color32::from_rgb(0x96, 0x92, 0x8A),
        hover: Color32::from_rgb(0xF3, 0xED, 0xE1),
        border: None,
        shadow_alpha: 18,
    };

    /// 夜墨蓝：冷深色纸，正文严格用白色。
    const NIGHT_BLUE_SHEET: Sheet = Sheet {
        bg: Color32::from_rgb(0x22, 0x2A, 0x33),
        ink: Color32::WHITE,
        ink_muted: Color32::from_rgb(0xC6, 0xCD, 0xD4),
        ink_faint: Color32::from_rgb(0x7F, 0x89, 0x94),
        hover: Color32::from_rgb(0x2D, 0x37, 0x42),
        border: None,
        shadow_alpha: 90,
    };

    /// 檀黑棕：暖深色纸，正文严格用白色。
    const SANDALWOOD_SHEET: Sheet = Sheet {
        bg: Color32::from_rgb(0x2B, 0x28, 0x25),
        ink: Color32::WHITE,
        ink_muted: Color32::from_rgb(0xD0, 0xC8, 0xBF),
        ink_faint: Color32::from_rgb(0x8A, 0x81, 0x78),
        hover: Color32::from_rgb(0x39, 0x33, 0x2E),
        border: None,
        shadow_alpha: 90,
    };

    /// 灰绿墨纸：去掉檀黑棕的棕调，以细纸边和轻投影融入 MDEX 夜墨。
    const GREEN_INK_SHEET: Sheet = Sheet {
        bg: Color32::from_rgb(0x26, 0x2C, 0x27),
        ink: Color32::WHITE,
        ink_muted: Color32::from_rgb(0xB5, 0xBC, 0xAE),
        ink_faint: Color32::from_rgb(0x7F, 0x8B, 0x7A),
        hover: Color32::from_rgb(0x30, 0x3A, 0x31),
        border: Some(Color32::from_rgb(0x46, 0x50, 0x44)),
        shadow_alpha: 64,
    };

    /// 给定主题与用户选择，决定纸面家族。不读全局，便于直接测试。
    fn family_for(theme: Theme, mode: PaperMode) -> PaperFamily {
        match mode {
            PaperMode::Follow => theme.paper_family,
            PaperMode::Light => PaperFamily::Original,
            PaperMode::Dark => theme.dark_paper_family,
        }
    }

    /// 纸面家族对应的固定取色。这里不读取界面强调色，避免主题色渗进公文正文。
    const fn sheet_for_family(family: PaperFamily) -> Sheet {
        match family {
            PaperFamily::Original => ORIGINAL_SHEET,
            PaperFamily::Parchment => PARCHMENT_SHEET,
            PaperFamily::NightBlue => NIGHT_BLUE_SHEET,
            PaperFamily::Sandalwood => SANDALWOOD_SHEET,
            PaperFamily::GreenInk => GREEN_INK_SHEET,
        }
    }

    /// 给定主题与用户选择，算出完整纸面取色。
    fn sheet_for(theme: Theme, mode: PaperMode) -> Sheet {
        sheet_for_family(family_for(theme, mode))
    }

    std::thread_local! {
        // 只在一次报告预览绘制内覆盖纸面，不改应用主题和用户纸色设置。
        static REPORT_SHEET: std::cell::Cell<Option<Sheet>> = const { std::cell::Cell::new(None) };
    }

    pub(crate) struct ReportPaper(Option<Sheet>);

    impl Drop for ReportPaper {
        fn drop(&mut self) {
            REPORT_SHEET.with(|sheet| sheet.set(self.0));
        }
    }

    pub(crate) fn report_paper(dark: bool) -> ReportPaper {
        let sheet = if dark {
            Sheet {
                bg: Color32::from_rgb(20, 27, 33),
                ink: Color32::from_rgb(231, 237, 240),
                ink_muted: Color32::from_rgb(141, 218, 233),
                ink_faint: Color32::from_rgb(80, 122, 136),
                hover: Color32::from_rgb(37, 56, 65),
                border: None,
                shadow_alpha: 90,
            }
        } else {
            Sheet {
                ink_muted: Color32::from_rgb(54, 85, 99),
                ink_faint: Color32::from_rgb(160, 173, 179),
                ..ORIGINAL_SHEET
            }
        };
        ReportPaper(REPORT_SHEET.with(|current| current.replace(Some(sheet))))
    }

    /// 同一模板内的标题强调色；仍在报告绘制的线程局部作用域里恢复。
    pub(crate) fn report_ink(ink: Color32) -> ReportPaper {
        let sheet = Sheet { ink, ..sheet() };
        ReportPaper(REPORT_SHEET.with(|current| current.replace(Some(sheet))))
    }

    /// 当前生效的纸面取色。
    fn sheet() -> Sheet {
        REPORT_SHEET
            .with(|sheet| sheet.get())
            .unwrap_or_else(|| sheet_for(current(), *CURRENT_PAPER.read().unwrap()))
    }

    /// 当前纸面是否为深色。
    pub fn is_dark() -> bool {
        REPORT_SHEET.with(|sheet| sheet.get()).map_or_else(
            || family_for(current(), *CURRENT_PAPER.read().unwrap()).is_dark(),
            |sheet| sheet.bg.r() < 128,
        )
    }

    /// 纸底。
    pub fn bg() -> Color32 {
        sheet().bg
    }

    /// 正文墨色，也用于表格线等纸面上的黑色笔画。
    pub fn ink() -> Color32 {
        sheet().ink
    }

    /// 红头、红色反线与份号等规范要求用红的地方。
    pub fn red() -> Color32 {
        OFFICIAL_RED
    }

    /// 鼠标悬停在可点击块上时的淡底。
    pub fn hover_tint() -> Color32 {
        sheet().hover
    }

    /// 纸面上的次级文字（图片占位卡片的说明等）。明色下与原来的 `gray(90)` 一致。
    pub fn ink_muted() -> Color32 {
        sheet().ink_muted
    }

    /// 纸面上最弱的笔画（占位卡片的细边框）。明色下与原来的 `gray(150)` 一致。
    pub fn ink_faint() -> Color32 {
        sheet().ink_faint
    }

    /// 纸边与纸底一起取色；没有专属纸边的纸面保留原有主题边框。
    pub fn border() -> Color32 {
        sheet().border.unwrap_or_else(|| {
            if super::is_mdex() {
                super::border().gamma_multiply(0.55)
            } else {
                super::border()
            }
        })
    }

    /// 纸张投影。各纸面按底色和纸边搭配控制离底感。
    pub fn shadow_alpha() -> u8 {
        sheet().shadow_alpha
    }

    #[cfg(test)]
    mod tests {
        use super::{
            GREEN_INK_SHEET, NIGHT_BLUE_SHEET, ORIGINAL_SHEET, PARCHMENT_SHEET, SANDALWOOD_SHEET,
            Sheet, sheet_for,
        };
        use crate::models::{PaperMode, ThemeName};
        use crate::theme::by_name;
        use eframe::egui::Color32;

        fn assert_follow(names: &[ThemeName], expected: Sheet) {
            for &name in names {
                assert_eq!(
                    sheet_for(by_name(name), PaperMode::Follow),
                    expected,
                    "{name:?} 的跟随主题纸面映射错误"
                );
            }
        }

        /// 跟随主题时，通用纸面与专属纸面使用各自确认过的配色。
        #[test]
        fn follow_mode_maps_themes_to_paper_families() {
            assert_follow(&[ThemeName::Claude], ORIGINAL_SHEET);
            assert_follow(&[ThemeName::Mdex, ThemeName::Smartisan], PARCHMENT_SHEET);
            assert_follow(&[ThemeName::MdexDark], GREEN_INK_SHEET);
            assert_follow(&[ThemeName::Nord], NIGHT_BLUE_SHEET);
            assert_follow(&[ThemeName::GruvboxDark], SANDALWOOD_SHEET);
        }

        /// 「白纸黑字」永远回到本色，与打印稿逐字节一致。
        #[test]
        fn light_paper_is_pure_black_on_white_under_every_theme() {
            for name in ThemeName::ALL {
                let sheet = sheet_for(by_name(name), PaperMode::Light);
                assert_eq!(sheet.bg, Color32::WHITE, "{name:?} 的白纸底不是纯白");
                assert_eq!(sheet.ink, Color32::BLACK, "{name:?} 的白纸墨色不是纯黑");
            }
        }

        /// 显式深色纸面仍按主题冷暖分组，不退回一套无差别的中性黑纸。
        #[test]
        fn forced_dark_paper_keeps_themes_cool_or_warm() {
            let cool = [ThemeName::Nord];
            let warm = [ThemeName::Claude, ThemeName::Mdex, ThemeName::GruvboxDark];
            for name in cool {
                assert_eq!(
                    sheet_for(by_name(name), PaperMode::Dark),
                    NIGHT_BLUE_SHEET,
                    "{name:?} 没有使用夜墨蓝"
                );
            }
            for name in warm {
                assert_eq!(
                    sheet_for(by_name(name), PaperMode::Dark),
                    SANDALWOOD_SHEET,
                    "{name:?} 没有使用檀黑棕"
                );
            }
            assert_eq!(
                sheet_for(by_name(ThemeName::MdexDark), PaperMode::Dark),
                GREEN_INK_SHEET,
                "MDEX 夜墨的显式深色纸面应与跟随主题一致"
            );
        }

        /// 正文只允许纯黑或纯白，不能被外观主题的文字色带偏。
        #[test]
        fn body_ink_is_only_black_or_white() {
            for name in ThemeName::ALL {
                for mode in PaperMode::ALL {
                    let ink = sheet_for(by_name(name), mode).ink;
                    assert!(
                        ink == Color32::BLACK || ink == Color32::WHITE,
                        "{name:?} / {mode:?} 的正文不是纯黑或纯白"
                    );
                }
            }
        }
    }
}

// ── 纸面与分隔线 ────────────────────────────────────────────────────────────

// ── 文字 ────────────────────────────────────────────────────────────────────

// ── 强调与语义色 ────────────────────────────────────────────────────────────

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
    /// Markdown 分栏模式的文字选区。按同步锚点的色相选另一组颜色，
    /// 避免两种含义叠在一起时分不清实际选中了哪些字。
    pub fn selection_bg() -> Color32 {
        let anchor = current().md.anchor_bg;
        let dark = current().dark;
        if anchor.g() > anchor.r() && anchor.g() > anchor.b() {
            if dark {
                Color32::from_rgb(0x53, 0x3D, 0x68)
            } else {
                Color32::from_rgb(0xDF, 0xCB, 0xF3)
            }
        } else if anchor.b() > anchor.r() {
            if dark {
                Color32::from_rgb(0x64, 0x49, 0x28)
            } else {
                Color32::from_rgb(0xF5, 0xD3, 0xA2)
            }
        } else if dark {
            Color32::from_rgb(0x28, 0x49, 0x6A)
        } else {
            Color32::from_rgb(0xB9, 0xD8, 0xF5)
        }
    }
    /// 查找条的普通命中；当前命中仍用更醒目的 `anchor_bg`。
    pub fn search_bg() -> Color32 {
        current().md.search_bg
    }
}

// ── 字体 ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{Color32, app_icon, configure_icons};
    use crate::models::ThemeName;
    use crate::theme::{Theme, by_name};
    use eframe::egui;

    /// 在一个浮层里连画几帧，列表依次是 `rows` 条，返回最后一帧浮层的高度。
    fn popup_height(rows: &[usize], scroll: impl Fn() -> egui::ScrollArea) -> f32 {
        let ctx = egui::Context::default();
        let mut height = 0.0;
        for &count in rows {
            // 每种条数画几帧，等浮层尺寸稳定下来。
            for _ in 0..3 {
                let raw = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 800.0),
                    )),
                    ..Default::default()
                };
                let _ = crate::test_support::run_ui(&ctx, raw, |ui| {
                    height = egui::Area::new(egui::Id::new("popup"))
                        .fixed_pos(egui::pos2(10.0, 10.0))
                        .show(ui.ctx(), |ui| {
                            scroll().show(ui, |ui| {
                                for row in 0..count {
                                    ui.label(format!("第 {row} 条"));
                                }
                            });
                        })
                        .response
                        .rect
                        .height();
                });
            }
        }
        height
    }

    /// 浮层里的列表筛少了再放开，要长回原来的高度；egui 原样的滚动区会卡在矮的那截。
    #[test]
    fn popup_lists_grow_back_after_shrinking() {
        let full = popup_height(&[40], || super::popup_scroll(200.0));
        assert!(full > 150.0, "{full}");
        let regrown = popup_height(&[40, 2, 40], || super::popup_scroll(200.0));
        assert!((regrown - full).abs() < 1.0, "{regrown} vs {full}");
        let short = popup_height(&[40, 2], || super::popup_scroll(200.0));
        assert!(short < 80.0, "内容少时照常收缩：{short}");

        let stuck = popup_height(&[40, 2, 40], || {
            egui::ScrollArea::vertical().max_height(200.0)
        });
        assert!(
            stuck < full - 50.0,
            "对照：原样的滚动区确实长不回去（{stuck}）"
        );
    }

    /// 预览里的每支公文字体后面都要挂着一支排得出生僻字的字体。
    ///
    /// 仿宋与楷体只有 GB2312 字库，「喆」「赟」在它们上面没有字形；字体族末尾
    /// 没挂兜底字体时，预览里看到的是一个个方框，而 PDF 里那几个字干脆消失。
    /// 三端要看到同一份稿子，这条和 TeX 侧的缺字断言是一对。
    ///
    /// 断的是字体族末尾那支字体的字库，而不是 egui 的 `has_glyph`：后者返回的
    /// 是「这个字是不是落在拥有替代字形的那支字体上」，主字体自带 `◻` 时会把
    /// 正常的字也判成排不出来，问不出我们要的答案。
    /// 冬青黑体的 line gap 是半个字高（500/1000），字形要下移四分之一字号才居中；
    /// 这是新版 macOS 找不到苹方时的系统默认界面字体，界面文字曾因此整体偏上。
    #[cfg(target_os = "macos")]
    #[test]
    fn hiragino_glyphs_shift_down_by_half_the_line_gap() {
        use crate::theme::line_gap_offset_factor;

        let Ok(data) = std::fs::read("/System/Library/Fonts/Hiragino Sans GB.ttc") else {
            return;
        };
        assert_eq!(line_gap_offset_factor(&data, 0), 0.25);
    }

    #[test]
    fn every_preview_font_family_ends_with_a_font_that_covers_rare_characters() {
        use crate::models::FontConfig;
        use crate::theme::{
            FONT_BIAOSONG, FONT_BOLD, FONT_FANGSONG, FONT_HEITI, FONT_KAITI, FONT_SONGTI,
            font_definitions, official_family,
        };

        let definitions = font_definitions(&FontConfig::default());
        if definitions.font_data.is_empty() {
            return; // 精简检出没有字体资产，跳过。
        }
        for family in [
            FONT_FANGSONG,
            FONT_KAITI,
            FONT_HEITI,
            FONT_BIAOSONG,
            FONT_SONGTI,
            FONT_BOLD,
        ] {
            let chain = definitions
                .families
                .get(&official_family(family))
                .unwrap_or_else(|| panic!("{family} 没有绑定字体"));
            // 本机既没有随应用分发的字体目录，系统里也没装这支公文字体时，整个
            // 字体族退回界面字体链（CI 的 macOS / Linux 机器就是这样），链尾是
            // egui 自带的 emoji 字体。此时没有「公文字体」可谈，兜底挂没挂也无从
            // 检验，跳过而不是误报。装上了才断。
            if !chain.iter().any(|key| key == family) {
                continue;
            }
            let last = chain
                .last()
                .unwrap_or_else(|| panic!("{family} 的字体族是空的"));
            let data = definitions
                .font_data
                .get(last)
                .unwrap_or_else(|| panic!("{family} 末尾的 {last} 没有字体数据"));
            let face = ttf_parser::Face::parse(&data.font, data.index)
                .unwrap_or_else(|_| panic!("{last} 解析不出字体"));
            for ch in "喆赟堃昇".chars() {
                assert!(
                    face.glyph_index(ch).is_some(),
                    "{family} 末尾的 {last} 排不出「{ch}」，兜底字体没挂上"
                );
            }
        }
    }

    #[test]
    fn configure_icons_installs_png_loader() {
        let ctx = egui::Context::default();
        configure_icons(&ctx);

        assert!(ctx.is_loader_installed(egui_extras::loaders::image_loader::ImageCrateLoader::ID));
    }

    /// 主题是在一次 UI 帧处理菜单点击时切换的，而这一帧已经创建的 `Ui` 仍持有
    /// 切换前的样式。必须作废当前 pass 并重跑，避免把新旧主题混绘到窗口中。
    #[test]
    fn reconfiguring_style_restarts_the_current_frame() {
        let ctx = egui::Context::default();
        let mut passes = 0;
        let output = crate::test_support::run_ui(&ctx, egui::RawInput::default(), |ui| {
            if passes == 0 {
                super::reconfigure_style(ui.ctx());
            }
            passes += 1;
        });

        assert_eq!(passes, 2, "主题切换必须作废旧样式所在的第一遍 UI");
        assert_eq!(output.platform_output.num_completed_passes, 2);
    }

    #[test]
    fn every_theme_has_a_valid_runtime_app_icon() {
        for name in ThemeName::ALL {
            let icon = app_icon(name);
            assert_eq!((icon.width, icon.height), (256, 256), "{name:?}");
            assert_eq!(icon.rgba.len(), 256 * 256 * 4, "{name:?}");
        }
    }

    /// sRGB 相对亮度（WCAG 2.1 定义），用于对比度计算。
    fn luminance(color: Color32) -> f32 {
        let channel = |value: u8| {
            let v = value as f32 / 255.0;
            if v <= 0.039_28 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
    }

    /// WCAG 对比度，取值 1.0（同色）到 21.0（纯黑对纯白）。
    fn contrast(a: Color32, b: Color32) -> f32 {
        let (high, low) = {
            let (x, y) = (luminance(a), luminance(b));
            if x > y { (x, y) } else { (y, x) }
        };
        (high + 0.05) / (low + 0.05)
    }

    /// 主题可以是明色也可以是深色，但读得清这条不能松：正文与强调色都必须
    /// 与各自的底色拉开足够对比度。原来那条「画布底必须发白」的断言随深色
    /// 主题一起去掉了——它约束的是色相而不是可读性。
    #[test]
    fn every_preset_stays_readable() {
        for name in ThemeName::ALL {
            let theme: Theme = by_name(name);

            // 正文对纸面：按 WCAG AA 的正文标准取 4.5。
            let body = contrast(theme.text, theme.surface);
            assert!(body >= 4.5, "{name:?} 的正文对比度只有 {body:.2}，低于 4.5");

            // 次级文字放宽到 3.0：它本来就该比正文弱一档，但仍要能读。
            let soft = contrast(theme.text_soft, theme.surface);
            assert!(
                soft >= 3.0,
                "{name:?} 的次级文字对比度只有 {soft:.2}，低于 3.0"
            );

            // 强调色对画布：按 WCAG AA 的大字/图形标准取 3.0。
            let accent = contrast(theme.accent, theme.canvas);
            assert!(
                accent >= 3.0,
                "{name:?} 的强调色对比度只有 {accent:.2}，低于 3.0"
            );

            // 强调淡底是用来衬托强调色文字的，两者不能糊在一起。
            let accent_on_soft = contrast(theme.accent, theme.accent_soft);
            assert!(
                accent_on_soft >= 3.0,
                "{name:?} 的强调色在淡底上对比度只有 {accent_on_soft:.2}，低于 3.0"
            );
        }
    }

    /// 明暗 MDEX 的实底按钮在普通、悬停和按下时均须保持正文级对比度。
    #[test]
    fn mdex_button_states_stay_readable() {
        for name in [ThemeName::Mdex, ThemeName::MdexDark] {
            let palette = by_name(name);
            let foreground = super::accent_text_for(palette);
            for background in [palette.accent, palette.accent_hover, palette.accent_active] {
                let ratio = contrast(foreground, background);
                assert!(ratio >= 4.5, "{name:?} 按钮字色对比度只有 {ratio:.2}");
            }
        }
    }

    /// `dark` 标志必须和实际配色一致，否则 egui 会拿错那套内建 visuals。
    #[test]
    fn dark_flag_matches_palette() {
        for name in ThemeName::ALL {
            let theme: Theme = by_name(name);
            let canvas_is_dark = luminance(theme.canvas) < 0.18;
            assert_eq!(
                theme.dark, canvas_is_dark,
                "{name:?} 的 dark 标志与画布底明暗不符"
            );
        }
    }

    /// 纸面明暗只能是屏幕上的事：导出器一旦引用界面颜色，用户把纸面调成深色
    /// 就可能导出一份灰底白字的公文。这里从源码层面钉死这条边界——`export`
    /// 下不允许出现任何颜色或主题取色，导出结果因此不可能随主题漂移。
    #[test]
    fn export_never_references_ui_colors() {
        fn scan(dir: &std::path::Path, offenders: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).expect("export 目录应可读") {
                let path = entry.expect("目录项应可读").path();
                if path.is_dir() {
                    scan(&path, offenders);
                    continue;
                }
                if path.extension().is_none_or(|ext| ext != "rs") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).expect("源文件应可读");
                for (index, line) in source.lines().enumerate() {
                    if line.contains("Color32") || line.contains("theme::paper") {
                        offenders.push(format!(
                            "{}:{}: {}",
                            path.file_name().unwrap_or_default().to_string_lossy(),
                            index + 1,
                            line.trim()
                        ));
                    }
                }
            }
        }

        let export_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("export");
        let mut offenders = Vec::new();
        scan(&export_dir, &mut offenders);
        assert!(
            offenders.is_empty(),
            "导出器不得引用界面颜色，否则纸面主题会漏进导出结果：\n{}",
            offenders.join("\n")
        );
    }

    /// 设置页与应用菜单都按 `dark` 标志把 ALL 分成明色、深色两组，
    /// 两组都不能为空，否则分组标题下会露出一块空白。
    #[test]
    fn both_theme_groups_are_non_empty() {
        let dark = ThemeName::ALL
            .into_iter()
            .filter(|name| by_name(*name).dark)
            .count();
        assert!(dark > 0, "深色组为空");
        assert!(dark < ThemeName::ALL.len(), "明色组为空");
    }
}
