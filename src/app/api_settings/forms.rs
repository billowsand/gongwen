//! 接口的编辑表单，详情页与「从文档添加」的候选共用：
//! - [`info_form`]：名称、用途、查询条件——给人和模型看的那部分；
//! - [`request_form`] / [`response_form`]：技术配置，按「请求 → 返回」分开。
//!
//! 表格用定宽单元格逐行排（[`cell`]），不用 `egui::Grid`：Grid 里的标签不换行，
//! 说明一长就被截成几个字。

use super::super::settings::setting_row;
use crate::agent::api::{
    ApiDestination, ApiEndpoint, ApiHeader, ApiInput, ApiMethod, BodyKind, InputKind,
};
use crate::theme;
use eframe::egui;

/// 定宽单元格：内容在里面换行。
pub(super) fn cell<R>(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_width(width);
            add(ui)
        },
    )
    .inner
}

/// 表头一行：淡底小字。
pub(super) fn table_head(ui: &mut egui::Ui, columns: &[(&str, f32)]) {
    egui::Frame::new()
        .fill(theme::surface_sunk())
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for (title, width) in columns {
                    cell(ui, *width, |ui| {
                        theme::caption(ui, title);
                    });
                }
            });
        });
}

/// 表格一行：左右留出与表头相同的内边距，下面一道细线。
pub(super) fn table_row(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.horizontal_top(add);
        });
    theme::hairline(ui);
}

/// 「说明」列能分到的宽度：总宽减去其余定宽列和间距。
pub(super) fn flex_width(ui: &egui::Ui, fixed: &[f32]) -> f32 {
    let gaps = ui.spacing().item_spacing.x * fixed.len() as f32 + 16.0;
    (ui.available_width() - fixed.iter().sum::<f32>() - gaps).max(120.0)
}

/// 名称、用途与查询条件。返回是否改了东西。
pub(super) fn info_form(ui: &mut egui::Ui, endpoint: &mut ApiEndpoint, salt: &str) -> bool {
    let before = endpoint.clone();
    ui.label(egui::RichText::new("名称").strong());
    ui.add(
        egui::TextEdit::singleline(&mut endpoint.name)
            .desired_width(360.0f32.min(ui.available_width())),
    );
    ui.add_space(10.0);
    ui.label(egui::RichText::new("用途").strong());
    theme::caption(ui, "这个接口能查什么、按什么查。AI 靠它判断什么时候该调用");
    ui.add(
        egui::TextEdit::multiline(&mut endpoint.description)
            .desired_rows(3)
            .hint_text("例如：按地区、年份查森林火灾起数，返回每个地区的起数与过火面积")
            .desired_width(f32::INFINITY),
    );
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("查询条件").strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(theme::secondary_icon_button(theme::Icon::Plus, "添加条件"))
                .clicked()
            {
                endpoint.inputs.push(ApiInput::default());
            }
        });
    });
    params_editor(ui, endpoint, salt);
    *endpoint != before
}

/// 查询条件的编辑表：参数名、类型、必填、说明、样例。
fn params_editor(ui: &mut egui::Ui, endpoint: &mut ApiEndpoint, salt: &str) {
    if endpoint.inputs.is_empty() {
        theme::caption(ui, "没有查询条件：接口不需要参数就能查。");
        return;
    }
    const NAME: f32 = 120.0;
    const KIND: f32 = 72.0;
    const REQUIRED: f32 = 36.0;
    const EXAMPLE: f32 = 140.0;
    const REMOVE: f32 = 28.0;
    let describe = flex_width(ui, &[NAME, KIND, REQUIRED, EXAMPLE, REMOVE]);
    table_head(
        ui,
        &[
            ("参数名", NAME),
            ("类型", KIND),
            ("必填", REQUIRED),
            ("说明", describe),
            ("样例", EXAMPLE),
            ("", REMOVE),
        ],
    );
    let mut remove = None;
    for (index, input) in endpoint.inputs.iter_mut().enumerate() {
        table_row(ui, |ui| {
            cell(ui, NAME, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut input.name)
                        .font(egui::TextStyle::Monospace)
                        .hint_text("region")
                        .desired_width(NAME),
                );
            });
            cell(ui, KIND, |ui| {
                egui::ComboBox::from_id_salt(("api_input_kind", salt, index))
                    .selected_text(input.kind.label())
                    .width(KIND - 6.0)
                    .show_ui(ui, |ui| {
                        for kind in InputKind::ALL {
                            ui.selectable_value(&mut input.kind, kind, kind.label());
                        }
                    });
            });
            cell(ui, REQUIRED, |ui| {
                ui.checkbox(&mut input.required, "");
            });
            cell(ui, describe, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut input.description)
                        .hint_text("如：地区名称，填省、市或县")
                        .desired_rows(1)
                        .desired_width(describe),
                );
            });
            cell(ui, EXAMPLE, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut input.example)
                        .hint_text("测试用")
                        .desired_width(EXAMPLE),
                );
            });
            cell(ui, REMOVE, |ui| {
                if theme::icon_button(ui, theme::Icon::Trash, "删除这个条件").clicked() {
                    remove = Some(index);
                }
            });
        });
    }
    if let Some(index) = remove {
        endpoint.inputs.remove(index);
    }
}

/// 请求：方法与地址、技能引用 id、超时、请求头、请求体。返回是否改了东西。
pub(super) fn request_form(ui: &mut egui::Ui, endpoint: &mut ApiEndpoint, salt: &str) -> bool {
    let before = endpoint.clone();
    setting_row(
        ui,
        "地址",
        Some("可以带变量：http://10.0.0.8/api/stat?region={region}"),
        |ui| {
            egui::ComboBox::from_id_salt(("api_method", salt))
                .selected_text(endpoint.method.label())
                .width(84.0)
                .show_ui(ui, |ui| {
                    for method in ApiMethod::ALL {
                        ui.selectable_value(&mut endpoint.method, method, method.label());
                    }
                });
            ui.add(
                egui::TextEdit::singleline(&mut endpoint.url)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY),
            );
        },
    );
    setting_row(
        ui,
        "性质",
        Some("只有只查询的接口给 AI 调、自动试调；会改数据的接口只能人在调试台发"),
        |ui| {
            ui.checkbox(&mut endpoint.readonly, "只查询，不改数据");
            ui.add_enabled(
                endpoint.readonly,
                egui::Checkbox::new(&mut endpoint.ai, "给 AI 用"),
            )
            .on_disabled_hover_text("会改数据的接口不给 AI 调");
        },
    );
    setting_row(
        ui,
        "技能引用 id",
        Some("只能用英文字母、数字、下划线、短横线和点"),
        |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut endpoint.id)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(200.0),
            );
            theme::caption(ui, &format!("技能里写 http.call:{}", endpoint.id));
        },
    );
    setting_row(ui, "超时", None, |ui| {
        ui.add(egui::DragValue::new(&mut endpoint.timeout_seconds).range(1..=300));
        ui.label("秒");
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label("请求头");
        theme::caption(ui, "密钥写成 {secret:名字}，值不进配置");
    });
    let mut remove = None;
    for (index, header) in endpoint.headers.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut header.name)
                    .font(egui::TextStyle::Monospace)
                    .hint_text("Authorization")
                    .desired_width(180.0),
            );
            let value_width = (ui.available_width() - 40.0).max(160.0);
            ui.add(
                egui::TextEdit::singleline(&mut header.value)
                    .font(egui::TextStyle::Monospace)
                    .hint_text("Bearer {secret:token}")
                    .desired_width(value_width),
            );
            if theme::icon_button(ui, theme::Icon::Trash, "删除这个请求头").clicked() {
                remove = Some(index);
            }
        });
    }
    if let Some(index) = remove {
        endpoint.headers.remove(index);
    }
    if ui
        .add(theme::secondary_icon_button(
            theme::Icon::Plus,
            "添加请求头",
        ))
        .clicked()
    {
        endpoint.headers.push(ApiHeader::default());
    }
    if endpoint.method.has_body() {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("请求体模板");
            theme::segmented(ui, |ui| {
                ui.selectable_value(&mut endpoint.body_kind, BodyKind::Json, "JSON");
                ui.selectable_value(&mut endpoint.body_kind, BodyKind::Form, "表单")
                    .on_hover_text(
                        "application/x-www-form-urlencoded：模板写成对象，逐项编成 键=值",
                    );
            });
            theme::caption(
                ui,
                "单独一个 \"{变量}\" 会按类型换成数字或是否；没给的可选变量整项不发",
            );
        });
        ui.push_id(("api_body", salt), |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut endpoint.body)
                    .code_editor()
                    .desired_rows(5)
                    .desired_width(f32::INFINITY),
            );
        });
    }
    *endpoint != before
}

/// 返回：条目位置与各字段怎么取、成功判据、结果放到哪。返回是否改了东西。
pub(super) fn response_form(ui: &mut egui::Ui, endpoint: &mut ApiEndpoint) -> bool {
    let before = endpoint.clone();
    let mapping = &mut endpoint.mapping;
    setting_row(
        ui,
        "条目位置",
        Some("JSON 指针，如 /data/items；留空表示整个返回"),
        |ui| {
            ui.add(mono_field(&mut mapping.list, "/data/items", 260.0));
        },
    );
    setting_row(ui, "标题", None, |ui| {
        ui.add(mono_field(&mut mapping.title, "title", 260.0));
    });
    setting_row(
        ui,
        "正文",
        Some("如 {region}{year}年共发生森林火灾{count}起"),
        |ui| {
            ui.add(mono_field(&mut mapping.text, "text", f32::INFINITY));
        },
    );
    setting_row(ui, "出处", None, |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut mapping.source)
                .hint_text("省统计系统")
                .desired_width(260.0),
        );
    });
    setting_row(
        ui,
        "编号",
        Some("同一条资料只进一次证据包，靠它认"),
        |ui| {
            ui.add(mono_field(&mut mapping.id, "id", 160.0));
        },
    );
    setting_row(
        ui,
        "成功判据",
        Some("返回里表示查询成功的字段与取值；多个取值用 | 分开"),
        |ui| {
            ui.add(mono_field(&mut endpoint.success.pointer, "/code", 120.0));
            ui.label("等于");
            ui.add(mono_field(&mut endpoint.success.equals, "0", 80.0));
            theme::caption(ui, "不填则 HTTP 200 就算成功");
        },
    );
    setting_row(ui, "结果放到", None, |ui| {
        theme::segmented(ui, |ui| {
            for destination in [ApiDestination::Evidence, ApiDestination::Variable] {
                ui.selectable_value(&mut endpoint.destination, destination, destination.label());
            }
        });
    });
    *endpoint != before
}

fn mono_field<'t>(text: &'t mut String, hint: &str, width: f32) -> egui::TextEdit<'t> {
    egui::TextEdit::singleline(text)
        .font(egui::TextStyle::Monospace)
        .hint_text(hint.to_owned())
        .desired_width(width)
}
