//! AI 管理「技能」：Pi 标准技能的管理入口。
//!
//! 管理的是 `<配置目录>/pi/skills/<name>/` 里的标准 SKILL.md 与完整技能包：查看、
//! 编辑、新建、复制内置、启停、恢复内置、删除、导入导出。旧流程技能目录只在页面上
//! 提示“仍保留但不供 Pi 执行”，不读取、不自动转换、不覆盖。
//!
//! 所有文件对话框都只记下意图，由应用层用 `request_file` 发起；页面自身不阻塞主线程。

#[cfg(test)]
mod tests;

use crate::ai_services::skill_package::Package;
use crate::app::GongwenApp;
use crate::app::file_dialog::FileDialogSpec;
use crate::pi_agent::pi_skills::{self, Catalog, Entry};
use crate::theme;
use eframe::egui;
use std::collections::BTreeMap;
use std::path::Path;

/// 技能页要开的文件对话框。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PiSkillDialog {
    ImportPackage,
    ImportFolder,
    Export(String),
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Filter {
    #[default]
    All,
    Builtin,
    Mine,
    Disabled,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Mode {
    #[default]
    Overview,
    Files,
}

struct Buffer {
    text: String,
    saved: String,
}

#[derive(Default)]
pub(super) struct PiSkillsPage {
    loaded: bool,
    catalog: Catalog,
    selected: Option<String>,
    mode: Mode,
    /// 当前选中技能的完整包（文件树与只读预览用）。
    package: Option<(String, Package)>,
    file: String,
    binary: bool,
    /// 按技能和文件路径保存编辑缓冲；切技能、切文件、返回概览都不丢未保存文本。
    buffers: BTreeMap<(String, String), Buffer>,
    search: String,
    filter: Filter,
    new_name: String,
    new_display: String,
    create_open: bool,
    copy_source: Option<String>,
    confirm_delete: bool,
    message: Option<(bool, String)>,
    dialog_request: Option<PiSkillDialog>,
}

impl GongwenApp {
    pub(super) fn skills_section_ui(&mut self, ui: &mut egui::Ui) {
        self.agent_settings.skills.ui(ui);
        self.run_pi_skill_dialog();
    }

    /// 技能页请求的文件对话框：对话框在后台线程开，主线程不阻塞。
    fn run_pi_skill_dialog(&mut self) {
        let Some(request) = self.agent_settings.skills.dialog_request.take() else {
            return;
        };
        match request {
            PiSkillDialog::ImportPackage => {
                self.request_file(
                    FileDialogSpec::pick_file().filter("标准技能包", &["zip", "skill"]),
                    |app, path| {
                        let Some(path) = path else {
                            return;
                        };
                        app.agent_settings.skills.import(&path);
                    },
                );
            }
            PiSkillDialog::ImportFolder => {
                self.request_file(FileDialogSpec::pick_folder(), |app, path| {
                    let Some(path) = path else {
                        return;
                    };
                    app.agent_settings.skills.import(&path);
                });
            }
            PiSkillDialog::Export(name) => {
                // 用户技能可能附带参考材料；公开内置技能照常直接导出。
                let user_skill = self
                    .agent_settings
                    .skills
                    .catalog
                    .get(&name)
                    .is_none_or(|entry| entry.user);
                if user_skill && self.auxiliary_requires_protection() {
                    let operation = format!("导出技能包：{name}");
                    let archive_name = format!("{name}.7z");
                    self.request_protected_archive(operation, archive_name, move || {
                        let package =
                            pi_skills::store::load_package(&name).map_err(anyhow::Error::msg)?;
                        Ok(vec![(format!("{name}.zip"), package.zip_bytes(&name)?)])
                    });
                    return;
                }
                let spec = FileDialogSpec::save_file()
                    .file_name(format!("{name}.zip"))
                    .filter("标准技能包", &["zip"]);
                self.request_file(spec, move |app, path| {
                    let Some(path) = path else {
                        return;
                    };
                    app.agent_settings.skills.export_skill(&name, &path);
                });
            }
        }
    }
}

impl PiSkillsPage {
    fn reload_catalog(&mut self) {
        self.catalog = pi_skills::load_catalog();
        self.loaded = true;
        if self
            .selected
            .as_ref()
            .is_none_or(|name| self.catalog.get(name).is_none())
        {
            self.selected = self
                .catalog
                .entries
                .iter()
                .find(|entry| entry.loadable())
                .or_else(|| self.catalog.entries.first())
                .map(|entry| entry.name.clone());
            self.mode = Mode::Overview;
            self.package = None;
        }
    }

    fn reload(&mut self) {
        self.reload_catalog();
        // 只保留仍有未保存修改的缓冲；已保存的直接从磁盘重读。
        let known = self
            .catalog
            .entries
            .iter()
            .map(|entry| entry.name.clone())
            .collect::<std::collections::BTreeSet<_>>();
        self.buffers
            .retain(|(name, _), buffer| buffer.text != buffer.saved && known.contains(name));
        self.package = None;
        self.confirm_delete = false;
    }

    fn select(&mut self, name: &str) {
        if self.selected.as_deref() != Some(name) {
            self.selected = Some(name.to_string());
            self.mode = Mode::Overview;
            self.package = None;
            self.file = "SKILL.md".into();
            self.binary = false;
            self.confirm_delete = false;
            self.message = None;
        }
    }

    fn current(&self) -> Option<&Entry> {
        self.selected
            .as_deref()
            .and_then(|name| self.catalog.get(name))
    }

    fn dirty(&self, name: &str) -> bool {
        self.buffers
            .iter()
            .any(|((skill, _), buffer)| skill == name && buffer.text != buffer.saved)
    }

    fn ensure_package(&mut self) -> bool {
        let Some(name) = self.current().map(|entry| entry.name.clone()) else {
            return false;
        };
        if self
            .package
            .as_ref()
            .is_none_or(|(current, _)| current != &name)
        {
            match pi_skills::store::load_package(&name) {
                Ok(package) => self.package = Some((name, package)),
                Err(error) => {
                    self.message = Some((false, format!("读取技能包失败：{error}")));
                    return false;
                }
            }
        }
        true
    }

    fn open_file(&mut self, file: &str) {
        self.file = file.to_string();
        self.binary = false;
        self.mode = Mode::Files;
        let Some(name) = self.selected.clone() else {
            return;
        };
        if self.buffers.contains_key(&(name.clone(), file.to_string())) {
            return;
        }
        if !self.ensure_package() {
            return;
        }
        let Some((_, package)) = &self.package else {
            return;
        };
        let Some(bytes) = package.files.get(file) else {
            self.message = Some((false, format!("技能包里没有 {file}。")));
            return;
        };
        match String::from_utf8(bytes.clone()) {
            Ok(text) => {
                self.buffers.insert(
                    (name, file.to_string()),
                    Buffer {
                        text: text.clone(),
                        saved: text,
                    },
                );
            }
            Err(_) => {
                self.binary = true;
                self.message = Some((
                    false,
                    format!(
                        "{file} 是二进制文件（{} 字节），不能在文本编辑器里修改；导出包时会原样保留。",
                        bytes.len()
                    ),
                ));
            }
        }
    }

    fn save_current(&mut self) {
        let Some(name) = self.selected.clone() else {
            return;
        };
        let Some(entry) = self.current() else {
            return;
        };
        if !entry.user {
            self.message = Some((
                false,
                "内置技能是只读的。请先「复制为我的技能」再编辑。".into(),
            ));
            return;
        }
        let Some(text) = self
            .buffers
            .get(&(name.clone(), self.file.clone()))
            .map(|buffer| buffer.text.clone())
        else {
            return;
        };
        match pi_skills::store::save_text(&name, &self.file, &text) {
            Ok(()) => {
                if let Some(buffer) = self.buffers.get_mut(&(name.clone(), self.file.clone())) {
                    buffer.saved = text;
                }
                self.message = Some((true, format!("已保存 {}", self.file)));
                self.reload_catalog();
                self.package = None;
            }
            Err(error) => {
                // 失败保留编辑缓冲，只在消息里说明原因。
                self.message = Some((false, format!("没有保存：{error}")));
            }
        }
    }

    fn toggle(&mut self, name: &str, enabled: bool) {
        match pi_skills::store::set_enabled(name, enabled) {
            Ok(()) => {
                self.reload_catalog();
                self.message = Some((
                    true,
                    format!("已{}「{name}」", if enabled { "启用" } else { "停用" }),
                ));
            }
            Err(error) => self.message = Some((false, error)),
        }
    }

    fn copy_builtin(&mut self, name: &str) {
        match pi_skills::store::copy_builtin(name) {
            Ok(()) => {
                self.reload_catalog();
                self.open_file("SKILL.md");
                self.message = Some((true, "已创建我的副本，可以直接编辑。".into()));
            }
            Err(error) => self.message = Some((false, error)),
        }
    }

    fn restore_builtin(&mut self, name: &str) {
        match pi_skills::store::restore_builtin(name) {
            Ok(()) => {
                self.buffers.retain(|(skill, _), _| skill != name);
                self.reload_catalog();
                self.package = None;
                self.mode = Mode::Overview;
                self.message = Some((true, "已恢复内置版本。".into()));
            }
            Err(error) => self.message = Some((false, error)),
        }
    }

    fn delete(&mut self, name: &str) {
        match pi_skills::store::delete(name) {
            Ok(()) => {
                self.buffers.retain(|(skill, _), _| skill != name);
                self.selected = None;
                self.package = None;
                self.mode = Mode::Overview;
                self.reload_catalog();
                self.message = Some((true, "已删除。".into()));
            }
            Err(error) => self.message = Some((false, error)),
        }
    }

    fn import(&mut self, path: &Path) {
        if self
            .buffers
            .values()
            .any(|buffer| buffer.text != buffer.saved)
        {
            self.message = Some((false, "请先保存或放弃未保存的修改，再导入技能。".into()));
            return;
        }
        match pi_skills::store::import(path) {
            Ok(ids) => {
                let list = ids.join("、");
                self.buffers.clear();
                self.reload_catalog();
                if let Some(first) = ids.first() {
                    self.select(first);
                }
                self.message = Some((true, format!("已导入：{list}")));
            }
            Err(error) => self.message = Some((false, format!("导入失败：{error}"))),
        }
    }

    fn export_skill(&mut self, name: &str, path: &Path) {
        match pi_skills::store::export(name, path) {
            Ok(path) => self.message = Some((true, format!("已导出到 {}", path.display()))),
            Err(error) => self.message = Some((false, format!("导出失败：{error}"))),
        }
    }

    fn visible_entries(&self) -> Vec<&Entry> {
        let query = self.search.trim().to_lowercase();
        self.catalog
            .entries
            .iter()
            .filter(|entry| {
                let by_filter = match self.filter {
                    Filter::All => true,
                    Filter::Builtin => entry.builtin,
                    Filter::Mine => entry.user,
                    Filter::Disabled => !entry.enabled || entry.error.is_some(),
                };
                by_filter
                    && (query.is_empty()
                        || entry.name.to_lowercase().contains(&query)
                        || entry.label.to_lowercase().contains(&query)
                        || entry.description.to_lowercase().contains(&query))
            })
            .collect()
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        ui.style_mut().visuals.widgets.inactive.bg_fill = theme::surface();
        ui.style_mut().visuals.widgets.inactive.weak_bg_fill = theme::surface();
        if !self.loaded {
            self.reload();
        }
        ui.weak("AI 管理 / 技能");
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new("Pi 标准技能")
                    .size(theme::font_sizes::HEADING + 4.0)
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary_icon_button(ui, theme::Icon::Plus, "新建技能").clicked() {
                    self.copy_source = None;
                    self.create_open = true;
                }
                ui.menu_button("导入技能", |ui| {
                    if ui.button("导入标准技能包…").clicked() {
                        ui.close();
                        self.dialog_request = Some(PiSkillDialog::ImportPackage);
                    }
                    if ui.button("导入技能文件夹…").clicked() {
                        ui.close();
                        self.dialog_request = Some(PiSkillDialog::ImportFolder);
                    }
                });
                if ui
                    .add(theme::icon_text_button(theme::Icon::Refresh, "刷新"))
                    .clicked()
                {
                    self.reload();
                    self.message = Some((true, "已重新读取技能目录。".into()));
                }
            });
        });
        ui.weak(
            "标准 SKILL.md：YAML frontmatter + 自然语言步骤。Pi 只执行这里的技能，不执行旧流程。",
        );
        ui.add_space(8.0);
        if let Some((ok, text)) = self.message.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(if ok { theme::success() } else { theme::warn() }, text);
                if ui.small_button("×").clicked() {
                    self.message = None;
                }
            });
        }
        if !self.catalog.notes.is_empty() {
            egui::CollapsingHeader::new(format!("{} 项加载提示", self.catalog.notes.len())).show(
                ui,
                |ui| {
                    for note in &self.catalog.notes {
                        ui.colored_label(theme::warn(), note);
                    }
                },
            );
        }
        if self.catalog.legacy_flow {
            ui.add_space(4.0);
            ui.colored_label(
                theme::text_muted(),
                "旧流程技能仍保留在配置目录 skills/，但 Pi 不会执行；需要时请改写为标准 SKILL.md 后导入。",
            );
        }
        ui.add_space(8.0);
        self.catalog_detail_ui(ui);
        self.create_dialog(ui.ctx());
    }

    fn catalog_detail_ui(&mut self, ui: &mut egui::Ui) {
        let size = ui.available_size();
        if size.x < 680.0 {
            if self.selected.is_some() {
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("← 返回技能列表").color(theme::accent()),
                        )
                        .frame_when_inactive(false),
                    )
                    .clicked()
                {
                    self.selected = None;
                    self.mode = Mode::Overview;
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
                |ui| self.detail_ui(ui),
            );
        });
    }

    fn catalog_ui(&mut self, ui: &mut egui::Ui) {
        ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text("搜索技能名或说明…")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.filter, Filter::All, "全部");
            ui.selectable_value(&mut self.filter, Filter::Builtin, "内置");
            ui.selectable_value(&mut self.filter, Filter::Mine, "我的");
            ui.selectable_value(&mut self.filter, Filter::Disabled, "停用 / 有问题");
        });
        ui.add_space(6.0);
        let entries = self
            .visible_entries()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let mut next = None;
        egui::ScrollArea::vertical()
            .id_salt("pi_skill_catalog")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                if entries.is_empty() {
                    ui.add_space(16.0);
                    ui.weak("没有匹配的技能。");
                }
                for entry in &entries {
                    let selected = self.selected.as_deref() == Some(entry.name.as_str());
                    let width = ui.available_width();
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(width, 62.0), egui::Sense::click());
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
                    let color = if entry.error.is_some() {
                        theme::warn()
                    } else if entry.enabled {
                        theme::success()
                    } else {
                        theme::text_muted()
                    };
                    ui.painter().circle_filled(
                        egui::pos2(rect.right() - 12.0, rect.top() + 18.0),
                        3.0,
                        color,
                    );
                    let text_rect = rect.shrink2(egui::vec2(12.0, 7.0));
                    ui.scope_builder(egui::UiBuilder::new().max_rect(text_rect), |ui| {
                        ui.set_width((width - 30.0).max(0.0));
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Label::new(egui::RichText::new(&entry.label).strong())
                                    .truncate()
                                    // 卡片整块是一个可点选项：标签不可框选，否则悬停变
                                    // 文本光标且点击被文本选择吃掉，卡片点不中。
                                    .selectable(false),
                            );
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(origin_label(entry))
                                        .small()
                                        .color(theme::text_muted()),
                                )
                                .selectable(false),
                            );
                            if self.dirty(&entry.name) {
                                ui.add(
                                    egui::Label::new(egui::RichText::new("●").color(theme::warn()))
                                        .selectable(false),
                                );
                            }
                        });
                        let detail = if let Some(error) = &entry.error {
                            error.clone()
                        } else {
                            entry.description.clone()
                        };
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(detail)
                                    .small()
                                    .color(theme::text_soft()),
                            )
                            .truncate()
                            .selectable(false),
                        );
                    });
                    if response
                        // 整块是选项，悬停显示手指而不是默认箭头。
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(if let Some(error) = &entry.error {
                            error.clone()
                        } else {
                            format!(
                                "{}\n{}",
                                entry.description,
                                if entry.enabled {
                                    "已启用"
                                } else {
                                    "已停用"
                                }
                            )
                        })
                        .clicked()
                    {
                        next = Some(entry.name.clone());
                    }
                }
            });
        if let Some(name) = next {
            self.select(&name);
        }
    }

    fn detail_ui(&mut self, ui: &mut egui::Ui) {
        let Some(entry) = self.current().cloned() else {
            ui.add_space(24.0);
            ui.weak("从左边选一个技能，或新建一个。");
            return;
        };
        egui::ScrollArea::vertical()
            .id_salt("pi_skill_detail")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                if self.mode == Mode::Files {
                    self.files_ui(ui, &entry);
                } else {
                    self.overview_ui(ui, &entry);
                }
            });
    }

    fn overview_ui(&mut self, ui: &mut egui::Ui, entry: &Entry) {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(&entry.label).size(20.0).strong());
            badge(ui, origin_label(entry));
            if entry.enabled {
                badge(ui, "已启用");
            } else {
                badge(ui, "已停用");
            }
            if self.dirty(&entry.name) {
                ui.colored_label(theme::warn(), "有未保存修改");
            }
        });
        ui.weak(&entry.name);
        ui.add_space(6.0);
        if let Some(error) = &entry.error {
            ui.colored_label(theme::warn(), error);
            ui.add_space(6.0);
        }
        if entry.description.is_empty() {
            ui.weak("没有说明。");
        } else {
            ui.label(&entry.description);
        }
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            let mut enabled = entry.enabled;
            if ui.checkbox(&mut enabled, "启用").changed() {
                self.toggle(&entry.name, enabled);
            }
            if entry.user {
                if ui
                    .add(theme::icon_text_button(theme::Icon::Edit, "编辑 SKILL.md"))
                    .clicked()
                {
                    self.open_file("SKILL.md");
                }
                if ui
                    .add(theme::icon_text_button(theme::Icon::Copy, "复制为新技能"))
                    .clicked()
                {
                    if self.dirty(&entry.name) {
                        self.message = Some((false, "请先保存或放弃该技能的修改，再复制。".into()));
                    } else {
                        self.copy_source = Some(entry.name.clone());
                        self.new_name.clear();
                        self.new_display.clear();
                        self.create_open = true;
                    }
                }
            } else if ui
                .add(theme::icon_text_button(theme::Icon::Copy, "复制为我的技能"))
                .clicked()
            {
                self.copy_builtin(&entry.name);
            }
            if entry.overridden()
                && ui
                    .add(theme::icon_text_button(theme::Icon::Undo, "恢复内置"))
                    .clicked()
            {
                self.restore_builtin(&entry.name);
            }
            if entry.user
                && ui
                    .add(theme::icon_text_button(theme::Icon::Trash, "删除"))
                    .clicked()
            {
                self.confirm_delete = true;
            }
            if ui
                .add(theme::icon_text_button(theme::Icon::FileDown, "导出包…"))
                .clicked()
            {
                self.dialog_request = Some(PiSkillDialog::Export(entry.name.clone()));
            }
        });
        if self.confirm_delete {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(
                    theme::warn(),
                    format!(
                        "确定删除「{}」的{}？",
                        entry.name,
                        if entry.builtin {
                            "我的副本（删除后恢复内置）"
                        } else {
                            "整个技能包"
                        }
                    ),
                );
                if ui.button("确认删除").clicked() {
                    self.delete(&entry.name);
                }
                if ui.button("取消").clicked() {
                    self.confirm_delete = false;
                }
            });
        }
        ui.add_space(12.0);
        if !entry.user {
            ui.colored_label(
                theme::text_muted(),
                "内置技能只读。要修改请先复制为我的技能；同名用户副本会覆盖内置版本。",
            );
        }
        ui.add_space(12.0);
        if self.ensure_package() {
            let Some((_, package)) = self.package.as_ref() else {
                return;
            };
            let files = file_rows(package);
            ui.label(egui::RichText::new("技能包文件").strong());
            ui.weak(format!(
                "{} 个文件；references / assets / scripts 原样保留，脚本只是文件，不会被执行。",
                files.len()
            ));
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .id_salt(("pi_skill_files_overview", entry.name.clone()))
                .max_height(180.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (relative, bytes) in &files {
                        if ui
                            .selectable_label(false, format!("{relative}（{bytes} 字节）"))
                            .clicked()
                        {
                            self.open_file(relative);
                        }
                    }
                });
        }
    }

    fn files_ui(&mut self, ui: &mut egui::Ui, entry: &Entry) {
        ui.horizontal_wrapped(|ui| {
            if ui
                .add(
                    egui::Button::new(egui::RichText::new("← 返回概览").color(theme::accent()))
                        .frame_when_inactive(false),
                )
                .clicked()
            {
                self.mode = Mode::Overview;
                self.binary = false;
            }
            ui.label(egui::RichText::new(&entry.label).strong());
            ui.weak(&entry.name);
            if self.dirty(&entry.name) {
                ui.colored_label(theme::warn(), "有未保存修改");
            }
        });
        ui.add_space(6.0);
        if !self.ensure_package() {
            return;
        }
        let Some((_, package)) = self.package.as_ref() else {
            return;
        };
        let files = file_rows(package);
        let size = ui.available_size();
        if size.x >= 650.0 {
            let width = (size.x * 0.28).clamp(200.0, 300.0);
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(width, size.y),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(width);
                        self.tree_ui(ui, entry, &files, (size.y - 20.0).max(80.0));
                    },
                );
                ui.separator();
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), size.y),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.editor_ui(ui, entry),
                );
            });
        } else {
            egui::CollapsingHeader::new(format!("技能包文件 · {}", self.file))
                .id_salt("pi_skill_tree_narrow")
                .show(ui, |ui| self.tree_ui(ui, entry, &files, 180.0));
            self.editor_ui(ui, entry);
        }
    }

    fn tree_ui(
        &mut self,
        ui: &mut egui::Ui,
        entry: &Entry,
        files: &[(String, usize)],
        height: f32,
    ) {
        ui.label(egui::RichText::new("技能包文件").strong());
        ui.weak(format!("{} 个文件", files.len()));
        ui.add_space(4.0);
        let mut next = None;
        egui::ScrollArea::vertical()
            .id_salt(("pi_skill_tree", entry.name.clone()))
            .auto_shrink([false; 2])
            .max_height(height)
            .show(ui, |ui| {
                for (relative, _) in files {
                    let selected = self.file == *relative;
                    if ui
                        .selectable_label(selected, relative)
                        .on_hover_text(relative)
                        .clicked()
                    {
                        next = Some(relative.clone());
                    }
                }
            });
        if let Some(file) = next {
            self.open_file(&file);
        }
    }

    fn editor_ui(&mut self, ui: &mut egui::Ui, entry: &Entry) {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(&self.file).strong());
            if entry.user {
                if ui
                    .add(theme::icon_text_button(theme::Icon::Save, "保存"))
                    .clicked()
                {
                    self.save_current();
                }
                let key = (entry.name.clone(), self.file.clone());
                if let Some(buffer) = self.buffers.get_mut(&key)
                    && buffer.text != buffer.saved
                    && ui.button("放弃修改").clicked()
                {
                    buffer.text.clone_from(&buffer.saved);
                }
            } else {
                ui.weak("内置只读；复制为我的技能后可编辑。");
            }
        });
        ui.add_space(4.0);
        if self.binary {
            ui.colored_label(
                theme::text_muted(),
                "二进制文件，不能在文本编辑器里查看或修改。",
            );
            return;
        }
        if !entry.user {
            ui.weak("内置技能需要编辑时，请先回到概览点「复制为我的技能」。");
        }
        let name = entry.name.clone();
        let file = self.file.clone();
        let Some(text) = self
            .buffers
            .get_mut(&(name.clone(), file.clone()))
            .map(|buffer| &mut buffer.text)
        else {
            ui.weak("这个文件还没有可编辑的文本缓冲。");
            return;
        };
        let height = (ui.available_height() - 56.0).clamp(160.0, 900.0);
        egui::ScrollArea::both()
            .id_salt(("pi_skill_editor", name, file))
            .auto_shrink([false; 2])
            .max_height(height)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(text)
                        .code_editor()
                        .interactive(entry.user)
                        .desired_rows(20)
                        .desired_width(f32::INFINITY)
                        .hint_text("在这里编辑标准 SKILL.md 或辅助文件"),
                );
            });
    }

    fn create_dialog(&mut self, ctx: &egui::Context) {
        if !self.create_open {
            return;
        }
        let mut open = true;
        egui::Window::new(if self.copy_source.is_some() {
            "复制 Pi 技能"
        } else {
            "新建 Pi 技能"
        })
        .id(egui::Id::unique("pi_skill_create"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label("技能标识（目录名，同时是 frontmatter 的 name）");
            ui.add(
                egui::TextEdit::singleline(&mut self.new_name)
                    .hint_text("例如 data-brief")
                    .desired_width(320.0),
            );
            ui.weak("小写字母、数字与单个短横线，最长 64；不能用路径、设备名或公共语法包名。");
            ui.add_space(6.0);
            ui.label("显示名（可选）");
            ui.add(
                egui::TextEdit::singleline(&mut self.new_display)
                    .hint_text("例如 数据简报")
                    .desired_width(320.0),
            );
            ui.add_space(8.0);
            let valid = pi_skills::valid_name(self.new_name.trim());
            if theme::primary_icon_button_enabled(ui, valid, theme::Icon::FilePlus, "创建并编辑")
                .clicked()
            {
                let name = self.new_name.trim().to_string();
                let display = self.new_display.trim().to_string();
                let display = (!display.is_empty()).then_some(display.as_str());
                let result = match self.copy_source.as_deref() {
                    Some(source) => pi_skills::store::copy_to(source, &name, display),
                    None => pi_skills::store::create(&name, display),
                };
                match result {
                    Ok(()) => {
                        self.create_open = false;
                        self.new_name.clear();
                        self.new_display.clear();
                        self.reload_catalog();
                        self.select(&name);
                        self.open_file("SKILL.md");
                        self.message = Some((true, "已创建，可以开始编辑 SKILL.md。".into()));
                    }
                    Err(error) => self.message = Some((false, error)),
                }
            }
        });
        self.create_open &= open;
    }
}

fn file_rows(package: &Package) -> Vec<(String, usize)> {
    package
        .files
        .iter()
        .map(|(path, bytes)| (path.clone(), bytes.len()))
        .collect()
}

fn origin_label(entry: &Entry) -> &'static str {
    if entry.overridden() {
        "已覆盖内置"
    } else if entry.builtin {
        "内置"
    } else if entry.user {
        "我的"
    } else {
        "未知"
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
