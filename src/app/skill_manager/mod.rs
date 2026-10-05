//! 独立的技能管理工作区：目录、概览和完整技能包浏览，不依赖稿件状态。

mod browser;
mod catalog;
mod detail;
mod markdown;
#[cfg(test)]
mod tests;

use crate::agent::api::ApiStore;
use crate::agent::skill::{self, Skill};
use crate::agent::skill_files::{self, package};
use crate::app::GongwenApp;
use crate::theme;
use eframe::egui;
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct SkillsPage {
    loaded: bool,
    skills: Vec<Skill>,
    apis: ApiStore,
    notes: Vec<String>,
    selected: Option<String>,
    search: String,
    origin: Origin,
    category: Category,
    files_mode: bool,
    package: Option<(String, package::Package)>,
    file: String,
    file_search: String,
    source_mode: bool,
    reveal_file: bool,
    /// 按技能和相对路径保存编辑缓冲；切文件、技能、返回概览均不丢修改。
    buffers: BTreeMap<(String, String), Buffer>,
    new_id: String,
    create_open: bool,
    new_path: String,
    file_create_open: bool,
    confirm_remove: bool,
    message: Option<(bool, String)>,
}

struct Buffer {
    text: String,
    saved: String,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Origin {
    #[default]
    All,
    Builtin,
    Mine,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Category {
    #[default]
    All,
    Draft,
    Revise,
    Review,
    Other,
}

impl GongwenApp {
    pub(super) fn skills_section_ui(&mut self, ui: &mut egui::Ui) {
        self.agent_settings.skills.ui(ui);
    }
}

impl SkillsPage {
    fn reload(&mut self) {
        self.buffers.retain(|_, buffer| buffer.text != buffer.saved);
        (self.skills, self.notes) = skill::load_all();
        self.apis = ApiStore::load().unwrap_or_default();
        self.loaded = true;
        if self
            .selected
            .as_ref()
            .is_none_or(|id| !self.skills.iter().any(|s| &s.id == id))
        {
            self.selected = self.skills.first().map(|s| s.id.clone());
        }
        self.package = None;
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        // 常规操作保持纸面底色，只让选中状态与主操作使用强调色。
        ui.style_mut().visuals.widgets.inactive.bg_fill = theme::surface();
        ui.style_mut().visuals.widgets.inactive.weak_bg_fill = theme::surface();
        if !self.loaded {
            self.reload();
        }
        ui.weak("AI 管理 / 技能");
        ui.add_space(6.0);
        if !self.files_mode {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new("技能管理")
                        .size(theme::font_sizes::HEADING + 4.0)
                        .strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::primary_icon_button(ui, theme::Icon::Plus, "新建技能").clicked() {
                        self.create_open = true;
                    }
                    ui.menu_button("导入技能", |ui| {
                        if ui.button("导入技能包…").clicked() {
                            ui.close();
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("技能包", &["skill", "zip"])
                                .pick_file()
                            {
                                self.import(&path);
                            }
                        }
                        if ui.button("导入技能文件夹…").clicked() {
                            ui.close();
                            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                self.import(&path);
                            }
                        }
                    });
                });
            });
            ui.weak("管理 AI 的写作方法与工作流程");
            ui.add_space(10.0);
            ui.separator();
        }
        if let Some((ok, text)) = self.message.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(if ok { theme::success() } else { theme::warn() }, text);
                if ui.small_button("×").clicked() {
                    self.message = None;
                }
            });
        }
        if !self.notes.is_empty() {
            egui::CollapsingHeader::new(format!("{} 项加载提示", self.notes.len())).show(
                ui,
                |ui| {
                    for note in &self.notes {
                        ui.colored_label(theme::warn(), note);
                    }
                },
            );
        }
        ui.add_space(8.0);
        if self.files_mode {
            self.files_ui(ui);
        } else {
            self.catalog_detail_ui(ui);
        }
        self.create_dialog(ui.ctx());
    }

    fn import(&mut self, path: &std::path::Path) {
        if self.buffers.values().any(|b| b.text != b.saved) {
            self.message = Some((false, "请先保存或放弃未保存的修改，再导入技能。".into()));
            return;
        }
        match skill_files::import(path) {
            Ok((ids, notes)) => {
                self.message = Some((
                    !ids.is_empty() && notes.is_empty(),
                    format!("已导入 {} 个技能。{}", ids.len(), notes.join("；")),
                ));
                self.buffers.clear();
                self.selected = ids.first().cloned().or(self.selected.take());
                self.reload();
            }
            Err(error) => self.message = Some((false, format!("导入失败：{error:#}"))),
        }
    }

    fn select(&mut self, id: &str) {
        if self.selected.as_deref() != Some(id) {
            self.selected = Some(id.to_string());
            self.package = None;
            self.file = "SKILL.md".into();
            self.file_search.clear();
            self.confirm_remove = false;
        }
    }

    fn dirty(&self, id: &str) -> bool {
        self.buffers
            .iter()
            .any(|((skill, _), buffer)| skill == id && buffer.text != buffer.saved)
    }

    fn create_dialog(&mut self, ctx: &egui::Context) {
        if !self.create_open {
            return;
        }
        let mut open = true;
        egui::Window::new("新建技能")
            .id(egui::Id::new("skill_create"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("技能标识");
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_id)
                        .hint_text("例如 data-brief")
                        .desired_width(320.0),
                );
                ui.weak("使用英文字母、数字、短横线或下划线；创建后可在文件中修改中文名称。");
                ui.add_space(8.0);
                if theme::primary_icon_button_enabled(
                    ui,
                    skill_files::valid_id(self.new_id.trim()),
                    theme::Icon::FilePlus,
                    "创建并编辑",
                )
                .clicked()
                {
                    let id = self.new_id.trim().to_string();
                    match skill_files::create(&id) {
                        Ok(_) => {
                            self.select(&id);
                            self.files_mode = true;
                            self.source_mode = true;
                            self.create_open = false;
                            self.new_id.clear();
                            self.reload();
                        }
                        Err(error) => self.message = Some((false, format!("{error:#}"))),
                    }
                }
            });
        self.create_open &= open;
    }
}

fn origin_label(skill: &Skill) -> &'static str {
    if skill.origin == "内置" {
        "内置"
    } else if skill::builtin_text(&skill.id).is_some() {
        "已改写内置"
    } else {
        "我的"
    }
}

fn badge(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .fill(theme::surface_sunk())
        .corner_radius(5)
        .inner_margin(egui::Margin::symmetric(7, 3))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(text)
                    .size(theme::font_sizes::SMALL)
                    .color(theme::text_soft()),
            );
        });
}

fn panel(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    theme::card().inner_margin(14).show(ui, |ui| {
        ui.set_width((ui.available_width() - 2.0).max(0.0));
        ui.label(egui::RichText::new(title).strong());
        ui.add_space(8.0);
        body(ui);
    });
}
