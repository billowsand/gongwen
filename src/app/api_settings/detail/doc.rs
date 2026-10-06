//! 「接口说明」页：像一份接口文档——用途、查询条件、返回什么、技能里怎么引用；就地编辑说明。

use super::*;

/// 文档里的一节标题，右边可以挂按钮。
pub(super) fn doc_heading(ui: &mut egui::Ui, title: &str, right: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).size(15.0).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
    });
    ui.add_space(4.0);
}

pub(super) fn doc_ui(ui: &mut egui::Ui, page: &mut ApisPage, index: usize) {
    let mut edit = false;
    let mut go = None;
    {
        let endpoint = &page.store.endpoints[index];
        doc_heading(ui, "用途", |ui| {
            if ui
                .add(theme::secondary_icon_button(theme::Icon::Edit, "编辑说明"))
                .clicked()
            {
                edit = true;
            }
        });
        let description = endpoint.description.trim();
        if description.is_empty() {
            theme::notice(
                ui,
                theme::Icon::TriangleAlert,
                theme::warn(),
                theme::warn_soft(),
                "还没写用途。AI 靠这段文字判断什么时候调用这个接口：写清能查什么、按什么查。点「编辑说明」补上。",
            );
        } else {
            ui.add(
                egui::Label::new(egui::RichText::new(description).color(theme::text_soft())).wrap(),
            );
        }

        ui.add_space(18.0);
        doc_heading(ui, "查询条件", |_| {});
        params_table(ui, endpoint);

        if !endpoint.examples.is_empty() {
            ui.add_space(18.0);
            // 最近一次整组用例的结论（配置改过就不算数）。
            let suite = page
                .log
                .suites
                .get(&endpoint.id)
                .filter(|record| record.fingerprint == endpoint.fingerprint());
            doc_heading(ui, "用例", |ui| {
                if let Some(record) = suite {
                    let (passed, total) = record.case_counts();
                    let color = if passed == total {
                        theme::success()
                    } else {
                        theme::danger()
                    };
                    ui.colored_label(color, format!("{passed}/{total} 通过 · {}", record.at));
                }
            });
            egui::CollapsingHeader::new(format!("展开 {} 条用例", endpoint.examples.len()))
                .id_salt(("api_doc_examples", &endpoint.id))
                .show(ui, |ui| {
                    for (at, example) in endpoint.examples.iter().enumerate() {
                        let case = suite.and_then(|record| {
                            record.cases.iter().find(|case| case.name == example.name)
                        });
                        ui.horizontal_wrapped(|ui| {
                            match case {
                                Some(case) if case.ok => {
                                    ui.colored_label(theme::success(), "✓");
                                }
                                Some(case) => {
                                    ui.colored_label(theme::danger(), "✗")
                                        .on_hover_text(&case.reason);
                                }
                                None => {}
                            }
                            ui.label(egui::RichText::new(&example.name).strong());
                            if !example.note.is_empty() {
                                theme::caption(ui, &example.note);
                            }
                            if ui.small_button("去试一下").clicked() {
                                go = Some(at);
                            }
                        });
                        let expects: Vec<String> =
                            example.expect.iter().map(|e| e.label()).collect();
                        theme::caption(
                            ui,
                            &if expects.is_empty() {
                                "期望：调通且业务成功".to_string()
                            } else {
                                format!("期望：调通且业务成功；{}", expects.join("；"))
                            },
                        );
                        if let Some(case) = case.filter(|case| !case.ok) {
                            ui.colored_label(theme::danger(), &case.reason);
                        }
                    }
                });
            theme::caption(
                ui,
                "每条是一种典型用法，写着期望；在「试一下」里「全部跑一遍」，结果就是这个接口可用的证明。",
            );
        }

        if !endpoint.ai_cases.is_empty() {
            ui.add_space(18.0);
            let suite = page.log.ai_suites.get(&endpoint.id);
            doc_heading(ui, "AI 用例", |ui| {
                if let Some(record) = suite {
                    let (passed, total) = record.case_counts();
                    let color = if passed == total {
                        theme::success()
                    } else {
                        theme::danger()
                    };
                    ui.colored_label(color, format!("{passed}/{total} 调对 · {}", record.at))
                        .on_hover_text(format!("模型 {}", record.model));
                }
            });
            egui::CollapsingHeader::new(format!("展开 {} 条 AI 用例", endpoint.ai_cases.len()))
                .id_salt(("api_doc_ai_examples", &endpoint.id))
                .show(ui, |ui| {
                    for case in &endpoint.ai_cases {
                        let result = suite.and_then(|record| {
                            record.cases.iter().find(|c| c.name == case.question)
                        });
                        ui.horizontal_wrapped(|ui| {
                            match result {
                                Some(result) if result.ok => {
                                    ui.colored_label(theme::success(), "✓");
                                }
                                Some(result) => {
                                    ui.colored_label(theme::danger(), "✗")
                                        .on_hover_text(&result.reason);
                                }
                                None => {}
                            }
                            ui.label(&case.question);
                        });
                        let args: Vec<String> = case
                            .expect_args
                            .iter()
                            .map(|(name, value)| {
                                format!("{name} = {}", crate::agent::board::value_to_text(value))
                            })
                            .collect();
                        theme::caption(
                            ui,
                            &if args.is_empty() {
                                "期望：调这个接口".to_string()
                            } else {
                                format!("期望：调这个接口，{}", args.join("，"))
                            },
                        );
                        if let Some(result) = result.filter(|r| !r.ok) {
                            ui.colored_label(theme::danger(), &result.reason);
                        }
                    }
                });
            theme::caption(
                ui,
                &match suite {
                    Some(record) if !record.model.is_empty() => format!(
                        "每条是一句用户可能的问法与期望的参数；上次用模型 {} 跑。在「试一下 → 说一句话让 AI 调」里全部跑一遍。",
                        record.model
                    ),
                    _ => "每条是一句用户可能的问法与期望的参数；在「试一下 → 说一句话让 AI 调」里全部跑一遍。".to_string(),
                },
            );
        }

        ui.add_space(18.0);
        doc_heading(ui, "返回什么", |_| {});
        returns_ui(ui, page, endpoint);

        ui.add_space(18.0);
        egui::CollapsingHeader::new("在技能里引用")
            .id_salt(("api_doc_reference", &endpoint.id))
            .show(ui, |ui| {
                let reference = format!("http.call:{}", endpoint.id);
                ui.horizontal(|ui| {
                    egui::Frame::new()
                        .fill(theme::canvas())
                        .stroke(egui::Stroke::new(1.0, theme::border()))
                        .corner_radius(egui::CornerRadius::same(6))
                        .inner_margin(egui::Margin::symmetric(10, 4))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(&reference).monospace());
                        });
                    if theme::icon_button(ui, theme::Icon::Copy, "复制").clicked() {
                        ui.ctx().copy_text(reference.clone());
                    }
                });
                theme::caption(ui, "写进技能 SKILL.md 的 tools 里，技能就能调用这个接口。");
            });
    }
    if edit {
        let snapshot = page.store.endpoints[index].clone();
        page.detail.start_editing(&snapshot);
    }
    if let Some(at) = go {
        page.detail.tab = Tab::Try;
        pick_example(page, index, at);
    }
}

/// 查询条件的只读表：参数、类型、必填、说明、样例。说明列换行显示全文。
pub(super) fn params_table(ui: &mut egui::Ui, endpoint: &ApiEndpoint) {
    if endpoint.inputs.is_empty() {
        theme::caption(ui, "不需要查询条件。");
        return;
    }
    const NAME: f32 = 130.0;
    const KIND: f32 = 48.0;
    const REQUIRED: f32 = 44.0;
    const EXAMPLE: f32 = 150.0;
    let describe = flex_width(ui, &[NAME, KIND, REQUIRED, EXAMPLE]);
    table_head(
        ui,
        &[
            ("参数", NAME),
            ("类型", KIND),
            ("必填", REQUIRED),
            ("说明", describe),
            ("样例", EXAMPLE),
        ],
    );
    for input in &endpoint.inputs {
        table_row(ui, |ui| {
            cell(ui, NAME, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(&input.name).monospace()).wrap());
            });
            cell(ui, KIND, |ui| {
                ui.label(input.kind.label());
            });
            cell(ui, REQUIRED, |ui| {
                if input.required {
                    ui.colored_label(theme::danger(), "必填");
                } else {
                    theme::caption(ui, "可选");
                }
            });
            cell(ui, describe, |ui| {
                let text = input.description.trim();
                if text.is_empty() {
                    theme::caption(ui, "（没写说明）");
                } else {
                    ui.add(
                        egui::Label::new(egui::RichText::new(text).color(theme::text_soft()))
                            .wrap(),
                    );
                }
            });
            cell(ui, EXAMPLE, |ui| {
                let example = input.example.trim();
                if !example.is_empty() {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(example)
                                .monospace()
                                .size(theme::font_sizes::SMALL)
                                .color(theme::text_soft()),
                        )
                        .truncate(),
                    )
                    .on_hover_text(example);
                }
            });
        });
    }
}

/// 「返回什么」：每条结果的各项从哪取，加上最近一次测试拿到的样子。
pub(super) fn returns_ui(ui: &mut egui::Ui, page: &ApisPage, endpoint: &ApiEndpoint) {
    ui.label(format!("结果用于：{}", endpoint.destination.label()));
    ui.add_space(4.0);
    egui::CollapsingHeader::new("字段映射与成功判据")
        .id_salt(("api_doc_mapping", &endpoint.id))
        .show(ui, |ui| {
            let mapping = &endpoint.mapping;
            let field = |value: &str| -> egui::RichText {
                if value.trim().is_empty() {
                    egui::RichText::new("没指定，按常见字段名猜").color(theme::text_muted())
                } else {
                    egui::RichText::new(value.trim()).monospace()
                }
            };
            let success = if endpoint.success.is_set() {
                egui::RichText::new(format!(
                    "{} 等于 {}",
                    endpoint.success.pointer.trim(),
                    endpoint.success.equals.trim()
                ))
                .monospace()
            } else {
                egui::RichText::new("HTTP 200 就算成功")
            };
            let list = if mapping.list.trim().is_empty() {
                egui::RichText::new("整个返回")
            } else {
                egui::RichText::new(mapping.list.trim()).monospace()
            };
            let rows = [
                ("条目位置", list),
                ("标题", field(&mapping.title)),
                ("正文", field(&mapping.text)),
                ("出处", field(&mapping.source)),
                ("编号", field(&mapping.id)),
                ("成功判据", success),
            ];
            egui::Frame::new()
                .fill(theme::canvas())
                .stroke(egui::Stroke::new(1.0, theme::border()))
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for (label, value) in rows {
                        ui.horizontal_top(|ui| {
                            cell(ui, 80.0, |ui| {
                                theme::caption(ui, label);
                            });
                            ui.add(egui::Label::new(value).wrap());
                        });
                    }
                });
        });
    ui.add_space(6.0);
    match &page.detail.result {
        Some(trial) if trial.ok() && !trial.items.is_empty() => {
            theme::caption(ui, "这次试调取到的第一条：");
            item_card(ui, 1, &trial.items[0]);
        }
        _ => match page.log.records.get(&endpoint.id) {
            Some(record) => {
                theme::caption(ui, &format!("上次测试 {}：{}", record.at, record.summary));
            }
            None => {
                theme::caption(ui, "还没测过：到「试一下」里发一次请求看看。");
            }
        },
    }
}

pub(super) fn item_card(ui: &mut egui::Ui, number: usize, item: &MappedItem) {
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, theme::border()))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                theme::caption(ui, &format!("#{number}"));
                ui.label(egui::RichText::new(&item.title).strong());
            });
            ui.add(
                egui::Label::new(
                    egui::RichText::new(crate::agent::tools::short(&item.text, 400))
                        .color(theme::text_soft()),
                )
                .wrap(),
            );
            theme::caption(ui, &format!("出处：{} · 编号：{}", item.source, item.id));
        });
    ui.add_space(6.0);
}

/// 就地编辑名称、用途与查询条件。
pub(super) fn edit_ui(ui: &mut egui::Ui, page: &mut ApisPage, index: usize) {
    theme::notice(
        ui,
        theme::Icon::Edit,
        theme::info(),
        theme::accent_soft(),
        "正在编辑说明。改完点「完成」回到阅读视图；改动跟其他设置一起保存。",
    );
    ui.add_space(10.0);
    page.dirty |= forms::info_form(ui, &mut page.store.endpoints[index], "detail");
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        if theme::primary_icon_button(ui, theme::Icon::Check, "完成").clicked() {
            page.detail.editing = None;
        }
        if ui
            .add(theme::secondary_icon_button(theme::Icon::Undo, "放弃修改"))
            .clicked()
        {
            discard_edit(page, index);
        }
    });
}

// —— 试一下 ——
