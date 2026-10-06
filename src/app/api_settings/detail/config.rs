//! 「技术配置」页：按「请求 → 返回」分组的配置表单，删除放在最下面。

use super::*;

pub(super) fn config_ui(ui: &mut egui::Ui, page: &mut ApisPage, index: usize) {
    let problems = page.store.endpoints[index].problems();
    if problems.is_empty() {
        theme::caption(
            ui,
            "从文档识别时程序已经填好，一般不用动。改了影响请求或返回的项，要重新测一次。",
        );
    } else {
        let mut text = format!("配置有 {} 个问题：", problems.len());
        for problem in &problems {
            text.push_str(&format!("\n· {problem}"));
        }
        theme::notice(
            ui,
            theme::Icon::TriangleAlert,
            theme::warn(),
            theme::warn_soft(),
            text,
        );
    }
    ui.add_space(10.0);
    panel(
        ui,
        theme::Icon::ArrowUp,
        "请求",
        None,
        |_| {},
        |ui| {
            page.dirty |= forms::request_form(ui, &mut page.store.endpoints[index], "detail");
        },
    );
    ui.add_space(12.0);
    panel(
        ui,
        theme::Icon::ArrowDown,
        "返回",
        Some("写英文字段名、/JSON 指针、{模板} 或固定文字；留空按常见字段名猜"),
        |_| {},
        |ui| {
            page.dirty |= forms::response_form(ui, &mut page.store.endpoints[index]);
        },
    );
    ui.add_space(12.0);
    let shared: Vec<String> = page.store.endpoints[index]
        .secret_names()
        .into_iter()
        .filter(|name| page.users_of(name).len() > 1)
        .collect();
    panel(
        ui,
        theme::Icon::Trash,
        "删除接口",
        None,
        |_| {},
        |ui| {
            ui.horizontal_wrapped(|ui| {
                let mut text = "删除后，引用它的技能运行时会报「找不到接口」。".to_string();
                if !shared.is_empty() {
                    text.push_str(&format!(
                        "密钥 {} 还有其他接口在用，不会删。",
                        shared.join("、")
                    ));
                }
                theme::caption(ui, &text);
                if ui
                    .add(theme::warning_icon_button(
                        theme::Icon::Trash,
                        "删除这个接口",
                    ))
                    .clicked()
                {
                    page.detail.confirm_remove = true;
                }
            });
        },
    );
}
