//! 文尾：主送、落款、联合发文与版记。
//!
//! 由 src/preview.rs 拆分而来：本文件是模块 `preview::tail`，与其它子模块共享
//! `preview` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::export;
use crate::models::{DraftInput, LetterVersion, TemplateKind, split_units};
use crate::preview::{
    BODY_PT, CLOSING_GAP_LINES, JOINT_COLUMN_MM, JOINT_DATE_GAP_MM, JOINT_ROW_GAP_MM, Metrics,
    PREVIEW_PLACEHOLDER, RECORD_GAP_MM, RECORD_PHONE_COLUMN_EM, RECORD_PT, SIGNATURE_WIDTH_MM,
    TABLE_LINE_PT, is_joint_mode_one, job, layout, line_block, line_galley, place, stacked,
    text_format,
};
use crate::theme;
use crate::units::UnitDisplay;
use eframe::egui;
use eframe::egui::{Align, FontId};
use std::sync::Arc;

/// 落款单位：留空时回落发文单位；电话通知用简称并逐字加空格；白头件按
/// “使用简称”选项取简称/全称（多单位时的逐行排布走 `white_paper_signature_units`）。
pub(crate) fn signature_unit(input: &DraftInput, display: &UnitDisplay) -> String {
    let raw = if input.profile.signing_unit.trim().is_empty() {
        input.profile.issuing_unit.trim()
    } else {
        input.profile.signing_unit.trim()
    };
    match input.kind {
        TemplateKind::PhoneNotice => display.abbr_spaced(raw),
        TemplateKind::WhitePaper | TemplateKind::RedHeadApproval => {
            let first = split_units(raw).into_iter().next().unwrap_or_default();
            display.signature_name(&first, input.profile.use_short_name_for_signature)
        }
        _ => display.full_name_for(raw, input.uses_external_unit_names()),
    }
}

/// 成文日期。预览版把“日”留成 1em 空位，与导出一致。
pub(crate) fn signature_date(input: &DraftInput) -> String {
    let preview = input.profile.letter_version == LetterVersion::Preview;
    match export::chinese_date_parts(&input.date) {
        Some((year, month, day)) => {
            let day = if preview { PREVIEW_PLACEHOLDER } else { day };
            format!("{year}年{month}月{day}日")
        }
        None => input.date.trim().to_string(),
    }
}

/// 代章标注：当前落款单位在标准词库中启用代章时返回“（代章）”。
pub(crate) fn signature_seal_mark(
    input: &DraftInput,
    display: &UnitDisplay,
) -> Option<&'static str> {
    if export::seals_on_behalf(input, display) {
        Some("（代章）")
    } else {
        None
    }
}

/// 主送机关（函稿、电话通知）或呈报领导（白头件）：楷体三号顶格，末尾加冒号。
pub(crate) fn addressee_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    input: &DraftInput,
    display: &UnitDisplay,
) {
    let text = match input.kind {
        TemplateKind::OfficialLetter | TemplateKind::PhoneNotice => display.join_hierarchical_for(
            &split_units(&input.profile.recipient),
            input.uses_external_unit_names(),
        ),
        TemplateKind::WhitePaper | TemplateKind::RedHeadApproval => {
            display.reporting_leaders(&input.profile.reporting_leaders)
        }
        TemplateKind::PlainDocument | TemplateKind::MeetingAgenda => String::new(),
    };
    let text = text.trim().trim_end_matches('：');
    if text.is_empty() {
        return;
    }
    line_block(
        ui,
        metrics,
        &format!("{text}："),
        theme::FONT_KAITI,
        BODY_PT,
        Align::LEFT,
    );
}

/// 落款：单位与成文日期。函稿/电话通知排在版心右侧 11cm 块内居中；白头件右侧
/// 留 4cm 签字空间、单位与日期之间空一行、多单位行间各空一行；联合发文模式 1 排成两列。
pub(crate) fn signature_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    input: &DraftInput,
    display: &UnitDisplay,
) {
    if input.kind == TemplateKind::MeetingAgenda || input.kind == TemplateKind::PlainDocument {
        return;
    }
    ui.add_space(metrics.line * CLOSING_GAP_LINES as f32);
    if crate::models::is_joint_signature(input) {
        joint_signature_block(ui, metrics, input, display);
        return;
    }
    let date = signature_date(input);
    let font = metrics.font(theme::FONT_FANGSONG, BODY_PT);
    let width = metrics.mm(SIGNATURE_WIDTH_MM).min(metrics.content);
    match input.kind {
        TemplateKind::WhitePaper | TemplateKind::RedHeadApproval => {
            // 块内右对齐；单位与日期之间空一行，多个单位自上而下分行、行间各空
            // 一行（便于分别签字）。红头呈批件的主预览已由打印分页器接管；这里
            // 保留相同的辅助布局，供独立块测试与防御性回落使用。
            let room = metrics.mm(crate::export::SIGNATURE_ROOM_MM);
            let left = (metrics.content - room - width).max(0.0);
            let units = display
                .white_paper_signature_units(input)
                .into_iter()
                .filter(|unit| !unit.trim().is_empty())
                .collect::<Vec<_>>();
            if units.is_empty() {
                return;
            }
            let mut galleys = Vec::new();
            for (index, unit) in units.iter().enumerate() {
                if index > 0 {
                    galleys.push(line_galley(
                        ui,
                        metrics,
                        "",
                        font.clone(),
                        width,
                        Align::Min,
                    ));
                }
                galleys.push(signature_unit_galley(
                    ui,
                    metrics,
                    unit,
                    font.clone(),
                    width,
                ));
            }
            galleys.push(line_galley(
                ui,
                metrics,
                "",
                font.clone(),
                width,
                Align::Min,
            ));
            galleys.push(line_galley(ui, metrics, &date, font, width, Align::Min));
            stacked(ui, metrics, &galleys, left, width, Align::Max);
        }
        _ => {
            let unit = if is_joint_mode_one(input) {
                // 联合发文模式 1 只剩 1 个发文单位：回落右侧落款，单位取该唯一发文单位。
                let raw = split_units(&input.profile.joint_issuing_units)
                    .into_iter()
                    .next()
                    .unwrap_or_default();
                display.full_name_for(&raw, input.uses_external_unit_names())
            } else {
                signature_unit(input, display)
            };
            if unit.trim().is_empty() {
                return;
            }
            let left = metrics.content - width;
            // 代章直接跟在落款单位后面同一行，不另起一行。
            let unit_line = signature_seal_mark(input, display)
                .map(|mark| format!("{unit}{mark}"))
                .unwrap_or_else(|| unit.clone());
            let galleys = [
                line_galley(ui, metrics, &unit_line, font.clone(), width, Align::Center),
                line_galley(ui, metrics, &date, font, width, Align::Center),
            ];
            stacked(ui, metrics, &galleys, left, width, Align::Center);
        }
    }
}

/// 落款单位单行：少于 5 字时分散对齐到 5 字宽（各端一致的“便于签字”宽度），
/// 5 字及以上按自然宽度排。行以自然宽度产出，右对齐交给 `stacked` 按宽度摆位。
pub(crate) fn signature_unit_galley(
    ui: &egui::Ui,
    metrics: &Metrics,
    text: &str,
    font: FontId,
    width: f32,
) -> Arc<egui::Galley> {
    match crate::units::spread_gap(text) {
        Some(gap) => {
            let mut job = job(width);
            let gap_px = gap * metrics.pt(BODY_PT);
            for (index, ch) in text.chars().enumerate() {
                job.append(
                    &ch.to_string(),
                    if index == 0 { 0.0 } else { gap_px },
                    text_format(font.clone(), metrics.line),
                );
            }
            layout(ui, job)
        }
        None => line_galley(ui, metrics, text, font, width, Align::Min),
    }
}

/// 联合发文模式 1 的落款：每行两个单位、各占 72mm 居中；单位多于两个且为奇数时
/// 最后一个跨两列居中；行间留出公章空档；最后把成文日期（含代章）压在主发文单位
/// 所在列下方居中，主单位跨两列时整体居中。
pub(crate) fn joint_signature_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    input: &DraftInput,
    display: &UnitDisplay,
) {
    let external = input.uses_external_unit_names();
    let mut units = split_units(&input.profile.joint_issuing_units)
        .iter()
        .map(|unit| display.full_name_for(unit, external))
        .collect::<Vec<_>>();
    if units.is_empty() {
        return;
    }
    // 代章直接跟在主发文单位后面同一行，不另起一行。
    if let Some(index) = export::joint_seal_index(input, display)
        && let Some(name) = units.get_mut(index)
    {
        name.push_str("（代章）");
    }
    let column = metrics.mm(JOINT_COLUMN_MM).min(metrics.content / 2.0);
    let table = column * 2.0;
    let left = (metrics.content - table) / 2.0;
    let font = metrics.font(theme::FONT_FANGSONG, BODY_PT);
    let rows = units.len().div_ceil(2);
    let odd_last = units.len() > 2 && units.len() % 2 == 1;

    for (index, pair) in units.chunks(2).enumerate() {
        let last = index + 1 == rows;
        if odd_last && last {
            let galley = line_galley(ui, metrics, &pair[0], font.clone(), table, Align::Center);
            stacked(ui, metrics, &[galley], left, table, Align::Center);
        } else {
            let cells = pair
                .iter()
                .map(|unit| line_galley(ui, metrics, unit, font.clone(), column, Align::Center))
                .collect::<Vec<_>>();
            let height = cells
                .iter()
                .map(|galley| galley.size().y)
                .fold(0.0, f32::max);
            place(ui, metrics, height, |painter, rect| {
                for (column_index, galley) in cells.iter().enumerate() {
                    let center = rect.left() + left + column * (column_index as f32 + 0.5);
                    painter.galley(
                        egui::pos2(center, rect.top()),
                        galley.clone(),
                        theme::paper::ink(),
                    );
                }
            });
        }
        // 公章需要空档：单位多于两个时，除最后一行外行间留 45mm。
        if units.len() > 2 && !last {
            ui.add_space(metrics.mm(JOINT_ROW_GAP_MM));
        }
    }
    ui.add_space(metrics.mm(JOINT_DATE_GAP_MM));
    // 日期压在主发文单位所在列下方，而不是整块居中；主单位跨列时整体居中。
    let (closing_left, closing_width) = match export::joint_main_column(input) {
        Some(col) => (left + column * col as f32, column),
        None => (left, table),
    };
    let date = line_galley(
        ui,
        metrics,
        &signature_date(input),
        font,
        closing_width,
        Align::Center,
    );
    stacked(
        ui,
        metrics,
        &[date],
        closing_left,
        closing_width,
        Align::Center,
    );
}

/// 版记（仅函稿）：四号三线表。第一行是抄送与共印份数，第二行起是承办单位、
/// 联系人、联系电话三列。成文时版记贴在版心底部，预览里紧跟落款。
pub(crate) fn footer_record(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    input: &DraftInput,
    display: &UnitDisplay,
) {
    if input.kind != TemplateKind::OfficialLetter {
        return;
    }
    ui.add_space(metrics.mm(RECORD_GAP_MM));
    let font = metrics.font(theme::FONT_FANGSONG, RECORD_PT);
    let line = metrics.pt(TABLE_LINE_PT);
    let joint = is_joint_mode_one(input);
    let external = input.uses_external_unit_names();

    // 共印份数 = 主送 + 抄送 + 承办单位数（类中的 autocalc）。
    let responsible_field = if joint {
        &input.profile.joint_responsible_units
    } else {
        &input.profile.responsible_unit
    };
    let copies = split_units(&input.profile.recipient).len()
        + split_units(&input.profile.copies_to).len()
        + split_units(responsible_field).len();
    let copies_to = display.join_hierarchical_for(&split_units(&input.profile.copies_to), external);
    let head = if copies_to.trim().is_empty() {
        String::new()
    } else {
        format!("抄送：{copies_to}")
    };

    // 承办单位/联系人/电话：联合发文有多行，其余只有一行。联合发文的承办单位
    // 与联系人成对录入（一一对应），旧稿件按索引回落配对。
    let rows: Vec<[String; 3]> = if joint {
        let entries = crate::models::joint_responsible_entries(&input.profile);
        let count = entries.len().max(1);
        (0..count)
            .map(|index| {
                let entry = entries.get(index);
                [
                    display.abbr(entry.map_or("", |value| value.unit.as_str())),
                    entry.map_or(String::new(), |value| value.name.clone()),
                    entry.map_or(String::new(), |value| value.phone.clone()),
                ]
            })
            .collect()
    } else {
        vec![[
            display.abbr(&input.profile.responsible_unit),
            input.profile.contact_person.trim().to_string(),
            input.profile.contact_phone.trim().to_string(),
        ]]
    };

    let phone_column = metrics.pt(RECORD_PT) * RECORD_PHONE_COLUMN_EM;
    let column = (metrics.content - phone_column) / 2.0;
    let columns = [column, column, phone_column];
    let thick = metrics.mm(0.6).max(1.0);
    let thin = metrics.mm(0.3).max(1.0);

    // 抄送行：三字符悬挂缩进，共印份数固定在行末。
    let head_galley = line_galley(
        ui,
        metrics,
        &head,
        font.clone(),
        metrics.content,
        Align::LEFT,
    );
    let copies_galley = line_galley(
        ui,
        metrics,
        &format!("（共印{copies}份）"),
        font.clone(),
        metrics.content,
        Align::Max,
    );
    let head_height = head_galley.size().y.max(line);

    // 三列：承办单位左、联系人中、联系电话右；第 2 行起用 5em/4em 占位与首行对齐。
    let cells: Vec<[Arc<egui::Galley>; 3]> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            // 第 2 行起用全角空格占位（一个全角空格正好 1em），让名称与首行标签后对齐。
            let pad = |ems: usize| {
                if index == 0 {
                    String::new()
                } else {
                    "\u{2003}".repeat(ems)
                }
            };
            let labels = ["承办单位：", "联系人：", "联系电话："];
            let aligns = [Align::LEFT, Align::Center, Align::Max];
            let pads = [5usize, 4, 0];
            std::array::from_fn(|index_column| {
                let label = if index == 0 { labels[index_column] } else { "" };
                let text = format!("{label}{}{}", pad(pads[index_column]), row[index_column]);
                line_galley(
                    ui,
                    metrics,
                    &text,
                    font.clone(),
                    columns[index_column],
                    aligns[index_column],
                )
            })
        })
        .collect();
    let row_heights = cells
        .iter()
        .map(|row| {
            row.iter()
                .map(|galley| galley.size().y)
                .fold(line, f32::max)
        })
        .collect::<Vec<_>>();

    let total = thick + head_height + thin + row_heights.iter().sum::<f32>() + thick;
    place(ui, metrics, total, |painter, rect| {
        let rule = |y: f32, width: f32| {
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(rect.left(), y),
                    egui::vec2(metrics.content, width),
                ),
                0.0,
                theme::paper::ink(),
            );
        };
        let mut y = rect.top();
        rule(y, thick);
        y += thick;
        painter.galley(
            egui::pos2(rect.left(), y),
            head_galley.clone(),
            theme::paper::ink(),
        );
        painter.galley(
            egui::pos2(rect.right(), y),
            copies_galley.clone(),
            theme::paper::ink(),
        );
        y += head_height;
        rule(y, thin);
        y += thin;
        for (row, height) in cells.iter().zip(&row_heights) {
            for (index_column, galley) in row.iter().enumerate() {
                let x = match index_column {
                    0 => rect.left(),
                    1 => rect.left() + columns[0] + columns[1] / 2.0,
                    _ => rect.right(),
                };
                painter.galley(egui::pos2(x, y), galley.clone(), theme::paper::ink());
            }
            y += height;
        }
        rule(y, thick);
    });
}
