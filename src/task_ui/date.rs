//! 日期输入：文本框照常可用输入法键入，旁边一枚日历按钮弹出中文月历。
//!
//! 不用 `egui_extras::DatePickerButton`：它的按钮、星期与月份都是英文，且要多开
//! 一个依赖特性。这里只要年月翻页、点日即填、今天与清除四样。
use crate::theme;
use chrono::{Datelike, Duration, NaiveDate};

const WEEKDAYS: [&str; 7] = ["一", "二", "三", "四", "五", "六", "日"];

pub(super) fn field(ui: &mut egui::Ui, id_salt: &str, value: &mut Option<String>, width: f32) {
    let mut text = value.clone().unwrap_or_default();
    let invalid = !crate::task_assistant::valid_date(value);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let edit = ui.add(theme::field(
            &mut text,
            "YYYY-MM-DD",
            (width - 52.0).max(80.0),
        ));
        if edit.changed() {
            *value = (!text.trim().is_empty()).then(|| text.trim().to_owned());
        }
        let button = theme::icon_button(ui, theme::Icon::Calendar, "选日期");
        if button.clicked() {
            // 每次打开都回到所填日期（或今天）所在的月份。
            let month_id = egui::Id::new(("task_date_month", id_salt));
            ui.data_mut(|d| d.remove::<NaiveDate>(month_id));
        }
        egui::Popup::menu(&button)
            .id(egui::Id::new(("task_date_popup", id_salt)))
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| calendar(ui, id_salt, value));
    });
    if invalid {
        ui.label(
            egui::RichText::new("日期应为真实的 YYYY-MM-DD")
                .size(theme::font_sizes::SMALL)
                .color(theme::danger()),
        );
    }
}

fn calendar(ui: &mut egui::Ui, id_salt: &str, value: &mut Option<String>) {
    let today = chrono::Local::now().date_naive();
    let picked = value
        .as_deref()
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
    let month_id = egui::Id::new(("task_date_month", id_salt));
    let mut month = ui
        .data(|d| d.get_temp::<NaiveDate>(month_id))
        .unwrap_or_else(|| first_of_month(picked.unwrap_or(today)));
    ui.set_min_width(230.0);
    ui.horizontal(|ui| {
        if ui.small_button("‹").on_hover_text("上个月").clicked() {
            month = first_of_month(month - Duration::days(1));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("›").on_hover_text("下个月").clicked() {
                month = first_of_month(month + Duration::days(32));
            }
            ui.with_layout(
                egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                |ui| ui.strong(format!("{}年{}月", month.year(), month.month())),
            );
        });
    });
    let lead = month.weekday().num_days_from_monday() as i64;
    let start = month - Duration::days(lead);
    egui::Grid::new(("task_date_grid", id_salt))
        .spacing([2.0, 2.0])
        .show(ui, |ui| {
            for name in WEEKDAYS {
                ui.vertical_centered(|ui| theme::caption(ui, name));
            }
            ui.end_row();
            for week in 0..6 {
                for day in 0..7 {
                    let date = start + Duration::days(week * 7 + day);
                    let mut text = egui::RichText::new(date.day().to_string());
                    if date.month() != month.month() {
                        text = text.color(theme::text_muted());
                    } else if date == today {
                        text = text.color(theme::accent()).strong();
                    }
                    let button = egui::Button::new(text)
                        .selected(picked == Some(date))
                        .frame_when_inactive(picked == Some(date))
                        .min_size(egui::vec2(30.0, 24.0));
                    if ui.add(button).clicked() {
                        *value = Some(date.format("%Y-%m-%d").to_string());
                        ui.close();
                    }
                }
                ui.end_row();
            }
        });
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("今天").clicked() {
            *value = Some(today.format("%Y-%m-%d").to_string());
            ui.close();
        }
        if ui.button("清除").clicked() {
            *value = None;
            ui.close();
        }
    });
    ui.data_mut(|d| d.insert_temp(month_id, month));
}

fn first_of_month(date: NaiveDate) -> NaiveDate {
    date.with_day(1).unwrap_or(date)
}
