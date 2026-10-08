//! MDEX 纸墨：米白画布、墨黑文字与终端绿，按第二版设计稿落地。

use super::{Color32, MdPalette, PaperFamily, Theme};

pub(super) const LABEL: &str = "MDEX 纸墨";
pub(super) const DARK_LABEL: &str = "MDEX 夜墨";

pub(super) const fn palette() -> Theme {
    Theme {
        label: LABEL,
        dark: false,
        paper_family: PaperFamily::Parchment,
        dark_paper_family: PaperFamily::Sandalwood,
        canvas: Color32::from_rgb(0xF3, 0xF1, 0xEA),
        surface: Color32::from_rgb(0xFA, 0xF8, 0xF2),
        surface_sunk: Color32::from_rgb(0xEC, 0xEA, 0xE2),
        surface_hover: Color32::from_rgb(0xE7, 0xEC, 0xE2),
        surface_active: Color32::from_rgb(0xD4, 0xE1, 0xD1),
        border: Color32::from_rgb(0xCF, 0xD0, 0xC7),
        border_strong: Color32::from_rgb(0xA5, 0xAB, 0x9F),
        text: Color32::from_rgb(0x20, 0x22, 0x1F),
        text_soft: Color32::from_rgb(0x55, 0x59, 0x50),
        text_muted: Color32::from_rgb(0x72, 0x76, 0x6C),
        accent: Color32::from_rgb(0x21, 0x6B, 0x45),
        accent_hover: Color32::from_rgb(0x28, 0x7B, 0x50),
        accent_active: Color32::from_rgb(0x18, 0x53, 0x35),
        accent_soft: Color32::from_rgb(0xE1, 0xEB, 0xDF),
        warn: Color32::from_rgb(0x97, 0x62, 0x18),
        warn_soft: Color32::from_rgb(0xFA, 0xEF, 0xD8),
        danger: Color32::from_rgb(0xAD, 0x3E, 0x36),
        danger_soft: Color32::from_rgb(0xF8, 0xE5, 0xE1),
        success: Color32::from_rgb(0x21, 0x6B, 0x45),
        success_soft: Color32::from_rgb(0xE1, 0xEB, 0xDF),
        info: Color32::from_rgb(0x3E, 0x63, 0x7D),
        md: MdPalette {
            body: Color32::from_rgb(0x20, 0x22, 0x1F),
            marker: Color32::from_rgb(0x72, 0x76, 0x6C),
            title: Color32::from_rgb(0x21, 0x6B, 0x45),
            heading: Color32::from_rgb(0x20, 0x22, 0x1F),
            strong: Color32::from_rgb(0x18, 0x53, 0x35),
            strong_bg: Color32::from_rgb(0xE1, 0xEB, 0xDF),
            bullet: Color32::from_rgb(0x21, 0x6B, 0x45),
            table_pipe: Color32::from_rgb(0x72, 0x76, 0x6C),
            table_rule: Color32::from_rgb(0xA5, 0xAB, 0x9F),
            table_cell: Color32::from_rgb(0x55, 0x59, 0x50),
            comment: Color32::from_rgb(0x72, 0x76, 0x6C),
            comment_bg: Color32::from_rgb(0xEC, 0xEA, 0xE2),
            todo: Color32::from_rgb(0xAD, 0x3E, 0x36),
            todo_bg: Color32::from_rgb(0xF8, 0xE5, 0xE1),
            code: Color32::from_rgb(0x21, 0x6B, 0x45),
            quoted: Color32::from_rgb(0x55, 0x59, 0x50),
            anchor_bg: Color32::from_rgb(0xD4, 0xE1, 0xD1),
            search_bg: Color32::from_rgb(0xFA, 0xEF, 0xD8),
        },
    }
}

/// 夜墨与纸墨共用控件形状，使用灰绿墨纸与浅绿强调色。
pub(super) const fn dark_palette() -> Theme {
    Theme {
        label: DARK_LABEL,
        dark: true,
        paper_family: PaperFamily::GreenInk,
        dark_paper_family: PaperFamily::GreenInk,
        canvas: Color32::from_rgb(0x19, 0x1C, 0x19),
        surface: Color32::from_rgb(0x22, 0x27, 0x22),
        surface_sunk: Color32::from_rgb(0x2A, 0x30, 0x2A),
        surface_hover: Color32::from_rgb(0x30, 0x3B, 0x2D),
        surface_active: Color32::from_rgb(0x40, 0x54, 0x38),
        border: Color32::from_rgb(0x41, 0x4B, 0x40),
        border_strong: Color32::from_rgb(0x68, 0x77, 0x62),
        text: Color32::from_rgb(0xE9, 0xE7, 0xDE),
        text_soft: Color32::from_rgb(0xB5, 0xBC, 0xAE),
        text_muted: Color32::from_rgb(0x92, 0x9D, 0x8E),
        accent: Color32::from_rgb(0xA9, 0xC9, 0x8F),
        accent_hover: Color32::from_rgb(0xBB, 0xDD, 0xA0),
        accent_active: Color32::from_rgb(0x8B, 0xAC, 0x73),
        accent_soft: Color32::from_rgb(0x34, 0x45, 0x2F),
        warn: Color32::from_rgb(0xE0, 0xB7, 0x6C),
        warn_soft: Color32::from_rgb(0x40, 0x37, 0x25),
        danger: Color32::from_rgb(0xE9, 0x97, 0x8C),
        danger_soft: Color32::from_rgb(0x42, 0x2C, 0x29),
        success: Color32::from_rgb(0xA9, 0xC9, 0x8F),
        success_soft: Color32::from_rgb(0x34, 0x45, 0x2F),
        info: Color32::from_rgb(0x9D, 0xBA, 0xCB),
        md: MdPalette {
            body: Color32::from_rgb(0xE9, 0xE7, 0xDE),
            marker: Color32::from_rgb(0x92, 0x9D, 0x8E),
            title: Color32::from_rgb(0xA9, 0xC9, 0x8F),
            heading: Color32::from_rgb(0xE9, 0xE7, 0xDE),
            strong: Color32::from_rgb(0x8B, 0xAC, 0x73),
            strong_bg: Color32::from_rgb(0x34, 0x45, 0x2F),
            bullet: Color32::from_rgb(0xA9, 0xC9, 0x8F),
            table_pipe: Color32::from_rgb(0x92, 0x9D, 0x8E),
            table_rule: Color32::from_rgb(0x68, 0x77, 0x62),
            table_cell: Color32::from_rgb(0xB5, 0xBC, 0xAE),
            comment: Color32::from_rgb(0x92, 0x9D, 0x8E),
            comment_bg: Color32::from_rgb(0x2A, 0x30, 0x2A),
            todo: Color32::from_rgb(0xE9, 0x97, 0x8C),
            todo_bg: Color32::from_rgb(0x42, 0x2C, 0x29),
            code: Color32::from_rgb(0xA9, 0xC9, 0x8F),
            quoted: Color32::from_rgb(0xB5, 0xBC, 0xAE),
            anchor_bg: Color32::from_rgb(0x40, 0x54, 0x38),
            search_bg: Color32::from_rgb(0x40, 0x37, 0x25),
        },
    }
}

/// 用真实主题控件出样张，单独运行以免全局主题与其他界面测试互相影响。
#[test]
#[ignore = "出主题样张，手动跑"]
fn mdex_theme_sample() {
    use crate::{models, theme, ui_snapshot};
    use eframe::egui;

    for (name, path) in [
        (models::ThemeName::Mdex, "tmp/mdex-theme.png"),
        (models::ThemeName::MdexDark, "tmp/mdex-dark-theme.png"),
    ] {
        let ctx = egui::Context::default();
        theme::set_current(name);
        theme::configure_icons(&ctx);
        theme::configure_fonts(&ctx, &models::FontConfig::default());
        theme::configure_style(&ctx);
        let size = egui::vec2(880.0, 520.0);
        let mut canvas = ui_snapshot::Canvas::default();
        let mut title = "关于开展专项工作的通知".to_owned();
        for _ in 0..5 {
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ui| {
                    ui.horizontal(|ui| {
                        ui.add(theme::brand_image(48.0, theme::accent()));
                        ui.heading(theme::by_name(name).label);
                    });
                    ui.separator();
                    ui.horizontal(|ui| {
                        let _ = theme::primary_icon_button(ui, theme::Icon::Save, "保存");
                        let _ = ui.add(theme::secondary_icon_button(theme::Icon::FileDown, "导出"));
                        let _ = ui.selectable_label(true, "审校稿");
                        let _ = ui.selectable_label(false, "公文预览");
                    });
                    ui.add_space(12.0);
                    theme::card().show(ui, |ui| {
                        ui.strong("文档要素");
                        ui.horizontal(|ui| {
                            ui.label("标题");
                            ui.text_edit_singleline(&mut title);
                        });
                        ui.label("通知 · 各有关单位");
                    });
                    ui.add_space(12.0);
                    theme::card().show(ui, |ui| {
                        ui.heading("一、工作目标");
                        ui.label("为做好专项工作，现将有关事项通知如下。");
                        ui.label("明确责任分工，细化工作措施，确保工作取得实效。");
                    });
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        theme::chip(ui, "已保存", theme::success(), theme::success_soft());
                        theme::chip(ui, "待核实", theme::warn(), theme::warn_soft());
                        theme::chip(ui, "校对问题", theme::danger(), theme::danger_soft());
                    });
                    ui.label(egui::RichText::new("次级文字与说明").color(theme::text_soft()));
                    ui.label(egui::RichText::new("提示文字与占位").color(theme::text_muted()));
                    ui.monospace("mdex:~ $  UTF-8 / Markdown / Typst");
                },
            );
            canvas.render(
                &ctx,
                output,
                size,
                theme::canvas(),
                std::path::Path::new(path),
            );
        }
    }
    theme::set_current(models::ThemeName::default());
}
