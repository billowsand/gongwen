//! 技能概览与包级操作。流程和工具均来自实际配置，不生成装饰性假数据。

use super::*;
use crate::agent::skill::{RefUse, StepSpec};
use crate::agent::tools;

impl SkillsPage {
    pub(super) fn current(&self) -> Option<Skill> {
        self.selected
            .as_ref()
            .and_then(|id| self.skills.iter().find(|s| &s.id == id))
            .cloned()
    }

    pub(super) fn detail_ui(&mut self, ui: &mut egui::Ui) {
        let Some(skill) = self.current() else {
            ui.add_space(28.0);
            ui.heading("选择一个技能");
            ui.weak("在左侧浏览技能，查看用途、工作流程和技能文件。");
            return;
        };
        egui::ScrollArea::vertical()
            .id_salt(("skill_detail", &skill.id))
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                self.skill_header(ui, &skill);
                ui.add_space(12.0);
                panel(ui, "工作流程", |ui| {
                    if skill.flow.is_empty() {
                        ui.weak("此技能未配置流程。");
                    }
                    flow_ui(ui, &skill.flow, 0);
                });
                ui.add_space(10.0);
                if ui.available_width() > 620.0 {
                    ui.columns(2, |columns| {
                        references_ui(&mut columns[0], &skill);
                        tools_ui(&mut columns[1], &skill);
                    });
                } else {
                    references_ui(ui, &skill);
                    ui.add_space(10.0);
                    tools_ui(ui, &skill);
                }
                ui.add_space(10.0);
                panel(ui, "触发词", |ui| {
                    if skill.triggers.is_empty() {
                        ui.weak("在 AI 侧栏输入 / 主动选择此技能。");
                    }
                    ui.horizontal_wrapped(|ui| {
                        for trigger in &skill.triggers {
                            badge(ui, trigger);
                        }
                    });
                });
                ui.add_space(12.0);
                self.validation_ui(ui, &skill);
            });
    }

    pub(super) fn skill_header(&mut self, ui: &mut egui::Ui, skill: &Skill) {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(&skill.name)
                    .size(theme::font_sizes::HEADING)
                    .strong(),
            );
            badge(ui, origin_label(skill));
            badge(ui, Category::of(skill).label());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut enabled = skill.enabled;
                if enabled_toggle(ui, &mut enabled).changed() {
                    match skill_files::set_enabled(&skill.id, enabled) {
                        Ok(()) => self.reload(),
                        Err(error) => self.message = Some((false, format!("{error:#}"))),
                    }
                }
            });
        });
        ui.label(
            egui::RichText::new(&skill.id)
                .monospace()
                .small()
                .color(theme::text_muted()),
        );
        ui.add_space(6.0);
        if self.files_mode {
            ui.add(egui::Label::new(&skill.description).truncate())
                .on_hover_text(&skill.description);
        } else {
            ui.label(&skill.description);
        }
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            let editable = skill_files::has_user_file(&skill.id);
            if ui
                .add(theme::icon_text_button(
                    if editable {
                        theme::Icon::Edit
                    } else {
                        theme::Icon::Copy
                    },
                    if editable {
                        "编辑技能文件"
                    } else {
                        "复制并编辑"
                    },
                ))
                .clicked()
            {
                if editable {
                    self.files_mode = true;
                    self.source_mode = true;
                } else {
                    match package::copy_for_editing(&skill.id) {
                        Ok(()) => {
                            self.files_mode = true;
                            self.source_mode = true;
                            self.reload();
                            self.message =
                                Some((true, "已复制整个技能包，修改后保存即可生效。".into()));
                        }
                        Err(error) => self.message = Some((false, format!("复制失败：{error:#}"))),
                    }
                }
            }
            if ui
                .add_enabled(
                    !self.dirty(&skill.id),
                    theme::icon_text_button(theme::Icon::FileDown, "导出技能包"),
                )
                .on_disabled_hover_text("请先保存或放弃该技能的修改，再导出整个包")
                .clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .set_file_name(format!("{}.skill", skill.id))
                    .add_filter("技能包", &["skill"])
                    .save_file()
            {
                self.message = Some(match skill_files::export(&skill.id, &path) {
                    Ok(path) => (true, format!("已导出整个技能包：{}", path.display())),
                    Err(error) => (false, format!("导出失败：{error:#}")),
                });
            }
            if editable {
                ui.menu_button("更多", |ui| {
                    let label = if skill::builtin_text(&skill.id).is_some() {
                        "恢复内置版本…"
                    } else {
                        "删除技能…"
                    };
                    if ui.button(label).clicked() {
                        self.confirm_remove = true;
                        ui.close();
                    }
                });
            }
        });
        if self.confirm_remove {
            theme::card().show(ui, |ui| {
                ui.colored_label(
                    theme::warn(),
                    "将删除此技能的整个本地目录和未保存的修改。内置技能会恢复原版。",
                );
                ui.horizontal(|ui| {
                    if theme::danger_icon_button(ui, theme::Icon::Trash, "确认删除本地技能包")
                        .clicked()
                    {
                        match skill_files::remove(&skill.id) {
                            Ok(()) => {
                                self.buffers.retain(|(id, _), _| id != &skill.id);
                                self.confirm_remove = false;
                                self.files_mode = false;
                                self.reload();
                                self.message = Some((true, "已删除本地技能包。".into()));
                            }
                            Err(error) => {
                                self.message = Some((false, format!("删除失败：{error:#}")))
                            }
                        }
                    }
                    if ui.button("取消").clicked() {
                        self.confirm_remove = false;
                    }
                });
            });
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if tab(ui, "概览", !self.files_mode).clicked() {
                self.files_mode = false;
            }
            if tab(ui, "技能文件", self.files_mode).clicked() {
                self.files_mode = true;
            }
            if self.dirty(&skill.id) {
                ui.colored_label(theme::warn(), "有未保存的文件修改");
            }
        });
        ui.separator();
    }

    pub(super) fn validation_ui(&self, ui: &mut egui::Ui, skill: &Skill) {
        let problems = skill_files::problems_of(skill);
        let missing = skill::missing_apis(skill, &self.apis);
        if problems.is_empty() && missing.is_empty() {
            ui.colored_label(theme::success(), "✓ 配置校验通过");
            ui.weak("可在 AI 侧栏输入 / 选择此技能。");
        } else {
            for problem in problems {
                ui.colored_label(theme::warn(), problem);
            }
            if !missing.is_empty() {
                ui.colored_label(
                    theme::warn(),
                    format!("待配置的数据接口：{}", missing.join("、")),
                );
            }
        }
    }
}

fn tab(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let response = ui.add(
        egui::Button::new(egui::RichText::new(label).color(if selected {
            theme::accent()
        } else {
            theme::text_soft()
        }))
        .frame(false),
    );
    if selected {
        let y = response.rect.bottom();
        ui.painter().line_segment(
            [
                egui::pos2(response.rect.left(), y),
                egui::pos2(response.rect.right(), y),
            ],
            egui::Stroke::new(2.0, theme::accent()),
        );
    }
    response
}

fn references_ui(ui: &mut egui::Ui, skill: &Skill) {
    panel(ui, "引用与材料", |ui| {
        if skill.uses_knowledge() {
            ui.label("知识库");
            ui.weak("检索政策、历史稿件与背景资料作为证据。");
            ui.add_space(8.0);
        }
        ui.label("引用文章");
        ui.weak(match skill.references {
            RefUse::Evidence => "作为证据，可引用并核验来源。",
            RefUse::Baseline => "第一篇作为基准稿，其余作为证据。",
            RefUse::Material => "作为本次任务材料。",
            RefUse::Letter => "作为需要答复的来函。",
            RefUse::Sample => "作为学习风格的样稿。",
        });
        if !skill.hint.is_empty() {
            ui.add_space(8.0);
            ui.label("输入提示");
            ui.weak(&skill.hint);
        }
    });
}

fn enabled_toggle(ui: &mut egui::Ui, enabled: &mut bool) -> egui::Response {
    let (rect, mut response) = ui.allocate_exact_size(egui::vec2(38.0, 22.0), egui::Sense::click());
    if response.clicked() {
        *enabled = !*enabled;
        response.mark_changed();
    }
    let amount = ui.ctx().animate_bool(response.id, *enabled);
    ui.painter().rect_filled(
        rect,
        11,
        if *enabled {
            theme::accent()
        } else {
            theme::border_strong()
        },
    );
    let x = egui::lerp(rect.left() + 11.0..=rect.right() - 11.0, amount);
    ui.painter()
        .circle_filled(egui::pos2(x, rect.center().y), 8.0, theme::surface());
    ui.weak(if *enabled { "已启用" } else { "已停用" });
    response.on_hover_text("停用后不出现在 AI 侧栏的技能列表中")
}

fn tools_ui(ui: &mut egui::Ui, skill: &Skill) {
    panel(ui, "可用工具", |ui| {
        if skill.tools.is_empty() {
            ui.weak("未声明工具权限。");
        }
        for id in skill.tools.iter().take(4) {
            tool_row(ui, id);
        }
        if skill.tools.len() > 4 {
            egui::CollapsingHeader::new(format!("查看其余 {} 个工具", skill.tools.len() - 4))
                .id_salt(("skill_tools", &skill.id))
                .show(ui, |ui| {
                    for id in skill.tools.iter().skip(4) {
                        tool_row(ui, id);
                    }
                });
        }
    });
}

fn tool_row(ui: &mut egui::Ui, id: &str) {
    let description = tools::describe(id).unwrap_or_else(|| format!("未知工具：{id}"));
    let name = match id {
        "doc.elements" => "读取公文要素",
        "doc.read" => "读取正文",
        "doc.outline" => "读取文章结构",
        "kb.search" => "检索知识库",
        "check.placeholders" => "检查待核实信息",
        "check.facts" => "核查关键事实",
        "llm.generate" => "调用写作模型",
        "ws.write" => "写入 AI 工作稿",
        "ws.replace" => "修改 AI 工作稿",
        "ask.choice" => "向用户澄清",
        "finding.add" => "记录审阅意见",
        _ => id,
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(name).on_hover_text(&description);
        if let Some(tool) = tools::find(id) {
            ui.label(
                egui::RichText::new(tool.permission().label())
                    .small()
                    .color(theme::text_muted()),
            );
        }
    });
    ui.label(
        egui::RichText::new(id)
            .monospace()
            .small()
            .color(theme::text_muted()),
    )
    .on_hover_text(description);
    ui.add_space(6.0);
}

fn flow_ui(ui: &mut egui::Ui, steps: &[StepSpec], depth: usize) {
    ui.horizontal_wrapped(|ui| {
        for (index, step) in steps.iter().enumerate() {
            if index > 0 {
                ui.weak("›");
            }
            egui::Frame::new()
                .fill(theme::surface_sunk())
                .corner_radius(6)
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.set_max_width(160.0);
                    ui.label(
                        egui::RichText::new(format!("{:02}  {}", index + 1, step_name(step)))
                            .strong(),
                    );
                    if let Some(condition) = &step.when {
                        let label = match condition.as_str() {
                            Some("has_sources") => "有资料时",
                            Some("has_text") => "有正文时",
                            _ => "按条件执行",
                        };
                        ui.label(
                            egui::RichText::new(label)
                                .small()
                                .color(theme::text_muted()),
                        )
                        .on_hover_text(condition.to_string());
                    }
                });
        }
    });
    if depth < 8 {
        for step in steps.iter().filter(|s| !s.body.is_empty()) {
            egui::CollapsingHeader::new(format!("{} · 子流程", step_name(step)))
                .id_salt((depth, step.label()))
                .show(ui, |ui| flow_ui(ui, &step.body, depth + 1));
        }
    }
}

fn step_name(step: &StepSpec) -> String {
    let name = step.step.as_deref().unwrap_or_default();
    match name {
        "clarify" => "澄清要求",
        "plan" => "预研",
        "retrieve" => "检索证据",
        "generate" => "起草正文",
        "gap_loop" => "补全缺口",
        "verify" => "核验引用",
        "ask" => "确认待核实信息",
        "agent" => "自主任务",
        "for_each" => "逐项处理",
        _ => return step.label(),
    }
    .into()
}
