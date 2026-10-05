//! 可搜索、按用途和来源筛选的技能目录；两栏各自滚动。

use super::*;

impl Category {
    pub(super) const GROUPS: [Self; 4] = [Self::Draft, Self::Revise, Self::Review, Self::Other];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::All => "全部",
            Self::Draft => "起草",
            Self::Revise => "修改",
            Self::Review => "审核",
            Self::Other => "其他",
        }
    }

    pub(super) fn of(skill: &Skill) -> Self {
        match skill.id.as_str() {
            "research-draft" | "policy-report" | "imitate" | "material" | "reply-letter" => {
                Self::Draft
            }
            "polish" | "condense" | "tone" | "normalize" => Self::Revise,
            "review" | "fact-check" | "extract" | "policy-basis" => Self::Review,
            _ => Self::Other,
        }
    }
}

impl SkillsPage {
    pub(super) fn catalog_detail_ui(&mut self, ui: &mut egui::Ui) {
        let size = ui.available_size();
        // 窄窗口改为列表 / 详情导航，保证内容区不会被挤成不可读的第三栏。
        if size.x < 680.0 {
            if self.selected.is_some() {
                if ui.button("← 返回技能列表").clicked() {
                    self.selected = None;
                }
                self.detail_ui(ui);
            } else {
                self.catalog_ui(ui);
            }
            return;
        }
        let left = (size.x * 0.30).clamp(240.0, 330.0);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(left, size.y),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(left);
                    self.catalog_ui(ui);
                },
            );
            ui.separator();
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), size.y),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    self.detail_ui(ui);
                },
            );
        });
    }

    fn matches(&self, skill: &Skill) -> bool {
        let origin = match self.origin {
            Origin::All => true,
            Origin::Builtin => skill.origin == "内置",
            Origin::Mine => skill.origin != "内置",
        };
        let query = self.search.trim().to_lowercase();
        origin
            && (self.category == Category::All || self.category == Category::of(skill))
            && (query.is_empty()
                || format!(
                    "{} {} {} {}",
                    skill.name,
                    skill.id,
                    skill.description,
                    skill.triggers.join(" ")
                )
                .to_lowercase()
                .contains(&query))
    }

    pub(super) fn catalog_ui(&mut self, ui: &mut egui::Ui) {
        ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text("搜索技能…")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(
                &mut self.origin,
                Origin::All,
                format!("全部 {}", self.skills.len()),
            );
            ui.selectable_value(&mut self.origin, Origin::Builtin, "内置");
            ui.selectable_value(&mut self.origin, Origin::Mine, "我的");
        });
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.category, Category::All, "全部");
            for group in Category::GROUPS {
                ui.selectable_value(&mut self.category, group, group.label());
            }
        });
        ui.add_space(6.0);
        let height = (ui.available_height() - 38.0).max(90.0);
        let mut next = None;
        egui::ScrollArea::vertical()
            .id_salt("skill_catalog")
            .auto_shrink([false; 2])
            .max_height(height)
            .show(ui, |ui| {
                let mut count = 0;
                for group in Category::GROUPS {
                    let skills = self
                        .skills
                        .iter()
                        .filter(|s| Category::of(s) == group && self.matches(s))
                        .collect::<Vec<_>>();
                    if skills.is_empty() {
                        continue;
                    }
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(format!("{} · {}", group.label(), skills.len()))
                            .small()
                            .color(theme::text_muted()),
                    );
                    ui.add_space(4.0);
                    for skill in skills {
                        count += 1;
                        let selected = self.selected.as_deref() == Some(&skill.id);
                        let width = ui.available_width();
                        let (rect, response) =
                            ui.allocate_exact_size(egui::vec2(width, 64.0), egui::Sense::click());
                        let fill = if selected {
                            theme::accent_soft()
                        } else if response.hovered() {
                            theme::surface_hover()
                        } else {
                            theme::surface()
                        };
                        ui.painter().rect_filled(rect, 6, fill);
                        if selected {
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())),
                                2,
                                theme::accent(),
                            );
                        }
                        let text_rect = rect.shrink2(egui::vec2(12.0, 7.0));
                        let issues = skill_files::problems_of(skill);
                        let missing = skill::missing_apis(skill, &self.apis);
                        let color = if !issues.is_empty() || !missing.is_empty() {
                            theme::warn()
                        } else if skill.enabled {
                            theme::success()
                        } else {
                            theme::text_muted()
                        };
                        ui.painter().circle_filled(
                            egui::pos2(rect.right() - 12.0, rect.top() + 20.0),
                            3.0,
                            color,
                        );
                        ui.scope_builder(egui::UiBuilder::new().max_rect(text_rect), |ui| {
                            ui.set_width((width - 32.0).max(0.0));
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::Label::new(egui::RichText::new(&skill.name).strong())
                                        .truncate(),
                                );
                                ui.label(
                                    egui::RichText::new(origin_label(skill))
                                        .small()
                                        .color(theme::text_muted()),
                                );
                                if self.dirty(&skill.id) {
                                    ui.colored_label(theme::warn(), "●");
                                }
                            });
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&skill.description)
                                        .small()
                                        .color(theme::text_soft()),
                                )
                                .truncate(),
                            );
                        });
                        if response
                            .on_hover_text(format!(
                                "{}\n{}{}",
                                skill.description,
                                if skill.enabled {
                                    "已启用"
                                } else {
                                    "已停用"
                                },
                                if issues.is_empty() && missing.is_empty() {
                                    ""
                                } else {
                                    " · 配置待处理"
                                }
                            ))
                            .clicked()
                        {
                            next = Some(skill.id.clone());
                        }
                    }
                }
                if count == 0 {
                    ui.add_space(20.0);
                    ui.label("没有匹配的技能");
                    ui.weak("尝试其他关键词或调整筛选条件。");
                }
            });
        if let Some(id) = next {
            self.select(&id);
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.weak(format!(
                "{} 个技能 · {} 个已启用",
                self.skills.len(),
                self.skills.iter().filter(|s| s.enabled).count()
            ));
            if theme::icon_button(ui, theme::Icon::Refresh, "刷新技能").clicked() {
                self.reload();
            }
        });
    }
}
