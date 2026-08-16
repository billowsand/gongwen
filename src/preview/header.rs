//! 红头/版头：发文机关标志、密级、文号与红线。
//!
//! 由 src/preview.rs 拆分而来：本文件是模块 `preview::header`，与其它子模块共享
//! `preview` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::models::{DraftInput, JointIssuanceMode, LetterVersion, TemplateKind, split_units};
use crate::preview::{
    BODY_PT, HEADER_MAX_GAP_EM, HEADER_PT, HEADER_RULE_GAP_MM, HEADER_RULE_MM, Metrics,
    PREVIEW_PLACEHOLDER, WHITE_PAPER_BLANK_LINES, draw, job, layout, line_galley, place,
    single_line, text_format,
};
use crate::theme;
use crate::units::UnitDisplay;
use eframe::egui;
use eframe::egui::Align;

/// 联合发文模式 1：多家单位并列成文，落款与版记都改成多行。
pub(crate) fn is_joint_mode_one(input: &DraftInput) -> bool {
    input.kind == TemplateKind::OfficialLetter
        && input.profile.joint_issuance_mode == JointIssuanceMode::Mode1
}

/// 红头上印的发文机关：联合发文取主办单位，其余取发文单位，一律展开为全称。
pub(crate) fn header_unit(input: &DraftInput, display: &UnitDisplay) -> String {
    let external = input.uses_external_unit_names();
    if !is_joint_mode_one(input) {
        return display.full_name_for(&input.profile.issuing_unit, external);
    }
    let units = split_units(&input.profile.joint_issuing_units);
    let main = input.profile.main_issuing_unit.trim();
    let chosen = if units.iter().any(|unit| unit == main) {
        main.to_string()
    } else {
        units.first().cloned().unwrap_or_default()
    };
    display.full_name_for(&chosen, external)
}

/// 密级行的文字：“密级★保密期限”，勾选指人专办时空一个全角空格再标注。
/// 未标注密级时返回 None，整行不排。仅用于测试断言：实际渲染在 `security_line`
/// 里按“黑体 + 数字等宽”分段画，不经过这里。
#[cfg(test)]
pub(crate) fn security_text(input: &DraftInput) -> Option<String> {
    let level = input.profile.security_level.trim();
    if level.is_empty() {
        return None;
    }
    let period = input.profile.security_period.trim();
    let mut text = format!("{level}★{period}");
    // 普通公文不带“指人专办”，与 export::latex::security_commands 一致。
    if input.kind != TemplateKind::PlainDocument && input.profile.special_handling {
        text.push('\u{2003}');
        text.push_str("指人专办");
    }
    Some(text)
}

/// 密级行：黑体三号顶格。返回是否真的画了东西，供调用方决定要不要留后续空行。
/// 保密期限的数字随整行用黑体，不另设等宽西文字体。
pub(crate) fn security_line(ui: &mut egui::Ui, metrics: &Metrics, input: &DraftInput) -> bool {
    let level = input.profile.security_level.trim();
    if level.is_empty() {
        return false;
    }
    let period = input.profile.security_period.trim();
    let special = if input.kind != TemplateKind::PlainDocument && input.profile.special_handling {
        "\u{2003}指人专办"
    } else {
        ""
    };
    let mut job = job(metrics.content);
    job.halign = Align::LEFT;
    let heiti = metrics.font(theme::FONT_HEITI, BODY_PT);
    job.append(
        &format!("{level}★{period}"),
        0.0,
        text_format(heiti.clone(), metrics.line),
    );
    if !special.is_empty() {
        job.append(special, 0.0, text_format(heiti.clone(), metrics.line));
    }
    draw(ui, job);
    true
}

/// 发文字号：代字〔年〕序号 号。预览版把流水号留成 1em 空位，与导出一致。
pub(crate) fn document_number(input: &DraftInput) -> String {
    let year = input.document_year();
    let serial = if input.profile.letter_version == LetterVersion::Preview {
        PREVIEW_PLACEHOLDER
    } else {
        input.profile.document_number.trim()
    };
    format!(
        "{}〔{year}〕{serial} 号",
        input.profile.department_code.trim()
    )
}

/// 红头（发文机关标志）：小标宋 29 磅红色，排得下就只拉开字距（上限 1em）、
/// 字形不变，排不下才缩小字号；下面是与版心等宽的红色反线。
pub(crate) fn red_header_inner(ui: &mut egui::Ui, metrics: &Metrics, unit: &str, draw_rule: bool) {
    if unit.trim().is_empty() {
        return;
    }
    let count = unit.chars().count() as f32;
    let em = metrics.pt(HEADER_PT);
    let mut format = text_format(metrics.font(theme::FONT_BIAOSONG, HEADER_PT), em * 1.2);
    format.color = theme::paper::red();

    let natural = layout(ui, single_line(unit, format.clone())).size().x;
    let mut block = natural;
    if natural < metrics.content && count > 1.0 {
        let gap = ((metrics.content - natural) / (count - 1.0)).min(em * HEADER_MAX_GAP_EM);
        format.extra_letter_spacing = gap;
        block = natural + gap * (count - 1.0);
    } else if natural > metrics.content {
        // 类里是横向压扁字形，egui 只能整体缩；两者都保证一行放得下。
        let shrunk = HEADER_PT * metrics.content / natural;
        format.font_id = metrics.font(theme::FONT_BIAOSONG, shrunk);
        block = metrics.content;
    }
    let galley = layout(ui, single_line(unit, format));
    place(ui, metrics, galley.size().y, |painter, rect| {
        let left = rect.left() + (metrics.content - block) / 2.0;
        painter.galley(
            egui::pos2(left, rect.top()),
            galley.clone(),
            theme::paper::red(),
        );
    });

    if draw_rule {
        let gap = metrics.mm(HEADER_RULE_GAP_MM);
        let thickness = metrics.mm(HEADER_RULE_MM).max(1.0);
        place(ui, metrics, gap + thickness, |painter, rect| {
            let top = rect.top() + gap;
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(rect.left(), top),
                    egui::vec2(metrics.content, thickness),
                ),
                0.0,
                theme::paper::red(),
            );
        });
    }
}

pub(crate) fn red_header(ui: &mut egui::Ui, metrics: &Metrics, unit: &str) {
    red_header_inner(ui, metrics, unit, true);
}

/// 份号与发文字号同一行：左端份号（黑体三号），右端“代字〔年〕序号 号”（仿宋三号）。
/// 份号类里固定为 01，导出器不覆盖，这里照排。
pub(crate) fn serial_and_number(ui: &mut egui::Ui, metrics: &Metrics, input: &DraftInput) {
    let number = document_number(input);
    let left = line_galley(
        ui,
        metrics,
        "01",
        metrics.font(theme::FONT_HEITI, BODY_PT),
        metrics.content,
        Align::LEFT,
    );
    let right = line_galley(
        ui,
        metrics,
        &number,
        metrics.font(theme::FONT_FANGSONG, BODY_PT),
        metrics.content,
        Align::Max,
    );
    let height = left.size().y.max(right.size().y);
    place(ui, metrics, height, |painter, rect| {
        painter.galley(rect.left_top(), left.clone(), theme::paper::ink());
        painter.galley(
            egui::pos2(rect.right(), rect.top()),
            right.clone(),
            theme::paper::ink(),
        );
    });
}

/// 抬头：按文种排出密级、红头、份号文号，并留出与标题之间的空行。
pub(crate) fn header_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    input: &DraftInput,
    display: &UnitDisplay,
) {
    match input.kind {
        TemplateKind::OfficialLetter | TemplateKind::PhoneNotice => {
            red_header(ui, metrics, &header_unit(input, display));
            // 电话通知不编发文字号，只有函稿排份号那一行。
            if input.kind == TemplateKind::OfficialLetter {
                serial_and_number(ui, metrics, input);
            }
            security_line(ui, metrics, input);
            ui.add_space(metrics.line);
        }
        TemplateKind::WhitePaper => {
            security_line(ui, metrics, input);
            ui.add_space(metrics.line * WHITE_PAPER_BLANK_LINES);
        }
        TemplateKind::RedHeadApproval => {
            // official_preview 会在进入普通 sheet 前转到专用打印分页器；这里仅作
            // 防御性回落，避免未来单独调用 header_block 时完全没有页首留白。
            security_line(ui, metrics, input);
            ui.add_space(metrics.line * WHITE_PAPER_BLANK_LINES);
        }
        TemplateKind::MeetingAgenda => {
            security_line(ui, metrics, input);
            ui.add_space(metrics.line);
        }
        TemplateKind::PlainDocument => {
            if security_line(ui, metrics, input) {
                ui.add_space(metrics.line);
            }
        }
    }
}
