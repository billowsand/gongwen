//! 技能包目录树与文件编辑缓冲；内置文件只读，文本源码可编辑，图片可预览。

use super::*;
use std::collections::BTreeSet;

impl SkillsPage {
    pub(super) fn files_ui(&mut self, ui: &mut egui::Ui) {
        let Some(skill) = self.current() else {
            self.files_mode = false;
            return;
        };
        if ui
            .add(
                egui::Button::new(egui::RichText::new("← 返回技能列表").color(theme::accent()))
                    .frame_when_inactive(false),
            )
            .clicked()
        {
            self.files_mode = false;
        }
        self.skill_header(ui, &skill);
        if !self.files_mode {
            return;
        }
        if self.package.as_ref().is_none_or(|(id, _)| id != &skill.id) {
            match package::load(&skill.id) {
                Ok(files) => self.package = Some((skill.id.clone(), files)),
                Err(error) => {
                    ui.colored_label(theme::warn(), format!("读取技能包失败：{error:#}"));
                    return;
                }
            }
        }
        let Some((id, mut files)) = self.package.take() else {
            return;
        };
        if !files.files.contains_key(&self.file) {
            self.file = "SKILL.md".into();
        }
        ui.add_space(8.0);
        let size = ui.available_size();
        let writable = skill_files::has_user_file(&id);
        if size.x >= 650.0 {
            let width = (size.x * 0.25).clamp(210.0, 280.0);
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(width, size.y),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(width);
                        self.tree_ui(ui, &id, &files, writable, (size.y - 196.0).max(80.0));
                    },
                );
                ui.separator();
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), size.y),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        self.file_ui(ui, &id, &mut files, writable);
                    },
                );
            });
        } else {
            egui::CollapsingHeader::new(format!("技能文件 · {}", self.file))
                .id_salt("narrow_file_tree")
                .show(ui, |ui| self.tree_ui(ui, &id, &files, writable, 180.0));
            self.file_ui(ui, &id, &mut files, writable);
        }
        self.package = Some((id.clone(), files));
        self.new_file_dialog(ui.ctx(), &id);
    }

    fn tree_ui(
        &mut self,
        ui: &mut egui::Ui,
        id: &str,
        files: &package::Package,
        writable: bool,
        height: f32,
    ) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("技能文件").strong());
            ui.weak(format!("{} 个文件", files.files.len()));
        });
        ui.add_space(6.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.file_search)
                .hint_text("搜索文件…")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(6.0);
        let query = self.file_search.trim().to_lowercase();
        let paths = files
            .files
            .keys()
            .filter(|name| query.is_empty() || name.to_lowercase().contains(&query))
            .cloned()
            .collect::<Vec<_>>();
        let mut next = None;
        egui::ScrollArea::vertical()
            .id_salt(("skill_tree", id))
            .auto_shrink([false; 2])
            .max_height(height)
            .show(ui, |ui| {
                if paths.is_empty() {
                    ui.weak("没有匹配的文件。");
                }
                self.tree_branch(ui, id, "", &paths, &mut next);
            });
        self.reveal_file = false;
        if let Some(path) = next {
            self.file = path;
        }
        ui.separator();
        if writable
            && ui
                .add(theme::icon_text_button(theme::Icon::FilePlus, "新建文件"))
                .clicked()
        {
            self.file_create_open = true;
        }
        if ui
            .add_enabled(
                writable,
                theme::icon_text_button(theme::Icon::Folder, "在文件夹中打开"),
            )
            .on_disabled_hover_text("内置技能保存在程序中，复制后可打开本地目录")
            .clicked()
        {
            match package::folder(id)
                .map_err(|e| e.to_string())
                .and_then(|path| crate::app::widgets::open_in_os(&path))
            {
                Ok(()) => {}
                Err(error) => self.message = Some((false, error)),
            }
        }
        ui.add_space(6.0);
        ui.weak("浏览整个技能包，SKILL.md 是入口。");
    }

    fn tree_branch(
        &self,
        ui: &mut egui::Ui,
        id: &str,
        prefix: &str,
        paths: &[String],
        next: &mut Option<String>,
    ) {
        let directories = paths
            .iter()
            .filter_map(|path| {
                path.strip_prefix(prefix)?
                    .split_once('/')
                    .map(|(dir, _)| dir.to_string())
            })
            .collect::<BTreeSet<_>>();
        for path in paths.iter().filter(|path| {
            path.strip_prefix(prefix)
                .is_some_and(|rest| !rest.contains('/'))
        }) {
            let name = path.strip_prefix(prefix).unwrap_or(path);
            let dirty = self
                .buffers
                .get(&(id.to_string(), path.clone()))
                .is_some_and(|buffer| buffer.text != buffer.saved);
            let label = format!(
                "{}{}{}",
                name,
                if name == "SKILL.md" {
                    "  · 入口"
                } else {
                    ""
                },
                if dirty { "  ●" } else { "" }
            );
            let response = ui
                .add(
                    egui::Button::new((
                        theme::Icon::FileTypeDoc
                            .image()
                            .fit_to_exact_size(egui::vec2(16.0, 16.0)),
                        label,
                        egui::Atom::grow(),
                    ))
                    .image_tint_follows_text_color(true)
                    .selected(&self.file == path)
                    .frame_when_inactive(&self.file == path)
                    .min_size(egui::vec2(ui.available_width(), 30.0)),
                )
                .on_hover_text(path);
            if response.clicked() {
                *next = Some(path.clone());
            }
        }
        for dir in directories {
            let child = format!("{prefix}{dir}/");
            let mut header = egui::CollapsingHeader::new(format!("{dir}/"))
                .id_salt((id, &child))
                .default_open(true);
            if !self.file_search.is_empty() || (self.reveal_file && self.file.starts_with(&child)) {
                header = header.open(Some(true));
            }
            header.show(ui, |ui| self.tree_branch(ui, id, &child, paths, next));
        }
    }

    fn file_ui(
        &mut self,
        ui: &mut egui::Ui,
        id: &str,
        files: &mut package::Package,
        writable: bool,
    ) {
        let path = self.file.clone();
        let Some(bytes) = files.files.get(&path) else {
            return;
        };
        let text = std::str::from_utf8(bytes)
            .ok()
            .filter(|text| !text.contains('\0'));
        let is_text = text.is_some();
        let image = is_image(&path);
        let markdown = path.to_lowercase().ends_with(".md");
        let key = (id.to_string(), path.clone());
        if let Some(text) = text.filter(|_| !image || path.to_lowercase().ends_with(".svg")) {
            self.buffers.entry(key.clone()).or_insert_with(|| Buffer {
                text: text.to_string(),
                saved: text.to_string(),
            });
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(format!("{id} / {path}")).strong());
            if text.is_some() && (markdown || image) {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.selectable_value(&mut self.source_mode, true, "源码");
                    ui.selectable_value(&mut self.source_mode, false, "预览");
                });
            }
        });
        ui.weak(format!(
            "{} · {:.1} KB{}",
            if markdown {
                "Markdown"
            } else if image {
                "图片"
            } else if text.is_some() {
                "文本"
            } else {
                "资源文件"
            },
            bytes.len() as f32 / 1024.0,
            if path == "SKILL.md" {
                " · 入口文件"
            } else {
                ""
            }
        ));
        if !writable {
            egui::Frame::new()
                .fill(theme::surface_sunk())
                .corner_radius(6)
                .inner_margin(8)
                .show(ui, |ui| {
                    ui.weak("内置文件只读，复制整个技能包后可编辑。");
                });
        }
        ui.add_space(8.0);
        let height = (ui.available_height() - if writable { 86.0 } else { 34.0 }).max(80.0);
        let mut link = None;
        egui::ScrollArea::both()
            .id_salt(("skill_file_content", id, &path, self.source_mode))
            .auto_shrink([false; 2])
            .max_height(height)
            .show(ui, |ui| {
                ui.set_min_width((ui.available_width() - 12.0).max(0.0));
                if image && !self.source_mode {
                    ui.add(
                        egui::Image::from_bytes(image_uri(id, &path, bytes), bytes.clone())
                            .max_width(ui.available_width())
                            .shrink_to_fit(),
                    );
                } else if let Some(buffer) = self.buffers.get_mut(&key) {
                    if self.source_mode || !markdown {
                        ui.add(
                            egui::TextEdit::multiline(&mut buffer.text)
                                .id(egui::Id::new(("skill_source", id, &path)))
                                .code_editor()
                                .interactive(writable)
                                .desired_rows(24)
                                .desired_width(f32::INFINITY),
                        );
                    } else {
                        let width = ui.available_width().min(940.0);
                        theme::card().inner_margin(20).show(ui, |ui| {
                            ui.set_width((width - 40.0).max(80.0));
                            link = super::markdown::preview(ui, &buffer.text, &path, files);
                        });
                    }
                } else if image {
                    ui.add(
                        egui::Image::from_bytes(image_uri(id, &path, bytes), bytes.clone())
                            .max_width(ui.available_width())
                            .shrink_to_fit(),
                    );
                } else {
                    ui.heading("资源文件");
                    ui.weak("此格式暂不支持内嵌预览，可在技能文件夹中查看。");
                }
            });
        if let Some(target) = link {
            self.file = target;
            self.file_search.clear();
            self.reveal_file = true;
        }
        ui.separator();
        if writable {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add(theme::icon_text_button(
                        theme::Icon::SquareCheck,
                        "校验技能",
                    ))
                    .clicked()
                {
                    self.message = Some(match self.check_entry(id, files) {
                        Ok(problems) if problems.is_empty() => {
                            (true, "技能入口配置校验通过。".into())
                        }
                        Ok(problems) => (false, problems.join("；")),
                        Err(error) => (false, error),
                    });
                }
                let dirty = self.dirty(id);
                if theme::primary_icon_button_enabled(ui, dirty, theme::Icon::Save, "保存修改")
                    .clicked()
                {
                    self.save_buffers(id, files);
                }
                if dirty {
                    ui.menu_button("放弃修改", |ui| {
                        ui.weak("放弃此技能所有文件的未保存修改？");
                        if ui.button("确认放弃").clicked() {
                            for ((skill, _), buffer) in &mut self.buffers {
                                if skill == id {
                                    buffer.text.clone_from(&buffer.saved);
                                }
                            }
                            ui.close();
                        }
                    });
                    ui.colored_label(theme::warn(), "有未保存的修改");
                }
            });
        }
        ui.weak(format!(
            "{path}  |  {}  |  {}",
            if is_text { "UTF-8" } else { "二进制" },
            if writable { "我的技能" } else { "只读" }
        ));
    }

    fn check_entry(&self, id: &str, files: &package::Package) -> Result<Vec<String>, String> {
        let key = (id.to_string(), "SKILL.md".to_string());
        let text = self
            .buffers
            .get(&key)
            .map(|b| b.text.as_str())
            .or_else(|| {
                files
                    .files
                    .get("SKILL.md")
                    .and_then(|bytes| std::str::from_utf8(bytes).ok())
            })
            .ok_or("无法读取技能入口")?;
        skill_files::check_text(id, text)
    }

    fn save_buffers(&mut self, id: &str, files: &mut package::Package) {
        let result = (|| -> anyhow::Result<Vec<String>> {
            let problems = self.check_entry(id, files).map_err(anyhow::Error::msg)?;
            let keys = self
                .buffers
                .iter()
                .filter(|((skill, _), buffer)| skill == id && buffer.text != buffer.saved)
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            // 先检测外部修改，防止浏览旧缓冲后覆盖外部编辑器的新内容。
            let disk = package::load(id)?;
            for key in &keys {
                let buffer = &self.buffers[key];
                anyhow::ensure!(
                    disk.files.get(&key.1).map(Vec::as_slice) == Some(buffer.saved.as_bytes()),
                    "{} 已在外部修改。请备份当前修改，再放弃修改并刷新技能。",
                    key.1
                );
            }
            for key in keys {
                let buffer = self.buffers.get_mut(&key).expect("编辑缓冲");
                package::save_file(id, &key.1, &buffer.text)?;
                buffer.saved.clone_from(&buffer.text);
                files.files.insert(key.1, buffer.text.as_bytes().to_vec());
            }
            Ok(problems)
        })();
        match result {
            Ok(problems) => {
                self.reload();
                self.message = Some((
                    problems.is_empty(),
                    if problems.is_empty() {
                        "已保存，下次发送时生效。".into()
                    } else {
                        format!("已保存，但配置有问题：{}", problems.join("；"))
                    },
                ));
            }
            Err(error) => self.message = Some((false, format!("保存失败：{error:#}"))),
        }
    }

    fn new_file_dialog(&mut self, ctx: &egui::Context, id: &str) {
        if !self.file_create_open {
            return;
        }
        let mut open = true;
        egui::Window::new("新建技能文件")
            .id(egui::Id::new("skill_file_create"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("相对路径");
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_path)
                        .hint_text("例如 references/引用规范.md")
                        .desired_width(340.0),
                );
                ui.weak("子目录会自动创建。");
                let path = package::relative_path(self.new_path.trim());
                if ui
                    .add_enabled(path.is_ok(), egui::Button::new("创建并编辑"))
                    .clicked()
                {
                    let name = path.expect("已校验路径");
                    let result = (|| -> anyhow::Result<()> {
                        anyhow::ensure!(
                            !package::load(id)?
                                .files
                                .keys()
                                .any(|path| path.eq_ignore_ascii_case(&name)),
                            "此文件已存在"
                        );
                        package::save_file(id, &name, "")?;
                        Ok(())
                    })();
                    match result {
                        Ok(()) => {
                            self.file = name;
                            self.source_mode = true;
                            self.package = None;
                            self.new_path.clear();
                            self.file_create_open = false;
                        }
                        Err(error) => self.message = Some((false, format!("{error:#}"))),
                    }
                }
            });
        self.file_create_open &= open;
    }
}

pub(super) fn is_image(path: &str) -> bool {
    path.rsplit('.').next().is_some_and(|extension| {
        matches!(
            extension.to_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg"
        )
    })
}

pub(super) fn image_uri(id: &str, path: &str, bytes: &[u8]) -> String {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hash);
    format!("bytes://skill/{id}/{:x}/{path}", hash.finish())
}
