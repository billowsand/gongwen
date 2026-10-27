//! 表格即工作区：统一搜索、行内编辑、多选移除与即时撤销。
use super::*;
use egui_extras::{Column, TableBuilder};
use std::path::Path;

impl Ime {
    pub(super) fn quick_header(&mut self, ui: &mut egui::Ui) -> Option<PageAction> {
        let typing = text_has_focus(ui.ctx());
        if !self.add_word.is_open() && !self.page.details_open {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
                self.page.search_focus = true;
            }
            if !typing && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z)) {
                let result = self.undo_personal();
                if let Err(error) = result {
                    self.manager.message = format!("撤销失败：{error}");
                }
            }
            if !typing
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete))
                && self.page.tab == Tab::Browse
            {
                self.remove_checked();
            }
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::N)) {
                self.open_add_word("", None);
            }
        }
        ui.horizontal(|ui| {
            ui.heading("输入法词表");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let add = ui.button("＋ 加词");
                if add.clicked() {
                    self.open_add_word("", None);
                    self.position_add_word(add.rect.left_bottom() + egui::vec2(-250.0, 6.0));
                }
                ui.menu_button("导出…", |ui| {
                    if ui.button("当前生效词表 TXT").clicked() {
                        self.export_text(false);
                        ui.close();
                    }
                    if ui.button("当前筛选结果 TXT").clicked() {
                        self.export_text(true);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("个人设置备份 JSON").clicked() {
                        self.export_personal();
                        ui.close();
                    }
                });
                ui.menu_button("导入…", |ui| {
                    if ui.button("公文四码表…").clicked() {
                        self.stage_import(true);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("更新基础词表…").clicked() {
                        self.stage_import(false);
                        ui.close();
                    }
                    if ui.button("恢复个人设置备份…").clicked() {
                        self.stage_backup();
                        ui.close();
                    }
                });
            });
        });
        ui.weak("双击修改，选中后按 Delete 移除，Ctrl+Z 撤销。");
        if self.page.tab == Tab::Browse {
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.page.search)
                        .id_salt("ime_quick_search")
                        .hint_text("搜索文字或编码")
                        .desired_width((ui.available_width() - 260.0).clamp(160.0, 420.0)),
                );
                if self.page.search_focus {
                    response.request_focus();
                    self.page.search_focus = false;
                }
                self.page.stale |= response.changed();
                egui::ComboBox::from_id_salt("ime_quick_source")
                    .selected_text(self.page.source.label())
                    .show_ui(ui, |ui| {
                        let mut sources = vec![
                            Source::All,
                            Source::Named("基础表".into()),
                            Source::Named("公文词表".into()),
                            Source::Named(PERSONAL.into()),
                            Source::Hidden,
                        ];
                        sources.extend(
                            self.table
                                .personal
                                .batches
                                .iter()
                                .map(|b| Source::Named(b.name.clone())),
                        );
                        for source in sources {
                            let label = source.label();
                            self.page.stale |= ui
                                .selectable_value(&mut self.page.source, source, label)
                                .changed();
                        }
                    });
                ui.toggle_value(&mut self.page.filters_open, "更多筛选");
                if (!self.page.search.is_empty()
                    || !self.page.code.is_empty()
                    || !self.page.text.is_empty()
                    || self.page.source != Source::All
                    || self.page.code_len != 0
                    || self.page.word_len != 0
                    || self.page.conflicts
                    || self.page.nonconforming)
                    && ui.small_button("清除筛选").clicked()
                {
                    self.page.search.clear();
                    self.page.code.clear();
                    self.page.text.clear();
                    self.page.source = Source::All;
                    self.page.code_len = 0;
                    self.page.word_len = 0;
                    self.page.conflicts = false;
                    self.page.nonconforming = false;
                    self.page.stale = true;
                }
            });
        }
        // 导入从工具栏唤起，不把用户带到另一个功能分区。
        self.import_preview(ui);
        None
    }

    pub(super) fn quick_feedback(&mut self, ui: &mut egui::Ui) {
        if let Some(error) = &self.load_error {
            ui.colored_label(theme::warn(), error);
        }
        if !self.manager.message.is_empty() || self.manager.undo.last().is_some() {
            let mut undo = false;
            ui.horizontal_wrapped(|ui| {
                ui.label(&self.manager.message);
                if let Some((_, label)) = self.manager.undo.last() {
                    undo = ui
                        .small_button("撤销")
                        .on_hover_text(format!("撤销：{label}"))
                        .clicked();
                }
            });
            if undo && let Err(error) = self.undo_personal() {
                self.manager.message = format!("撤销失败：{error}");
            }
            ui.add_space(4.0);
        }
    }

    /// 导出词表：文件对话框由应用层开，这里只登记请求。
    fn export_text(&mut self, filtered: bool) {
        self.page.request = Some(PageAction::ExportWordTable { filtered });
    }

    /// 把词表写到用户选定的位置。
    pub(crate) fn export_text_to(&mut self, path: &Path, filtered: bool) {
        let text = if filtered {
            self.filtered_rows()
                .into_iter()
                .map(|row| format!("{},{}={}\n", row.code, row.position.unwrap_or(1), row.text))
                .collect::<String>()
        } else {
            self.table.export()
        };
        let result = super::super::data::save_bytes(path, text.as_bytes());
        self.report(result, "词表已导出。");
    }

    /// 导出个人设置：文件对话框由应用层开，这里只登记请求。
    fn export_personal(&mut self) {
        self.page.request = Some(PageAction::ExportPersonalSettings);
    }

    /// 把个人设置写到用户选定的位置。
    pub(crate) fn export_personal_to(&mut self, path: &Path) {
        let result = super::super::data::save_json(path, &self.table.personal);
        self.report(
            result,
            "个人词条、导入表、屏蔽、排序与构词规则已备份；不含基础表。",
        );
    }

    fn remove_checked(&mut self) {
        if self.page.checked.is_empty() {
            if let Some(entry) = self.page.selected.clone()
                && self
                    .page
                    .rows
                    .iter()
                    .any(|r| r.code == entry.code && r.text == entry.text && r.position.is_some())
            {
                let result = self.remove_entries(std::slice::from_ref(&entry));
                self.report(result, &format!("已移除「{}」。", entry.text));
            }
            return;
        }
        let entries: Vec<_> = self
            .page
            .checked
            .iter()
            .map(|(code, text)| Entry {
                code: code.clone(),
                text: text.clone(),
            })
            .collect();
        let result = self.remove_entries(&entries);
        if result.is_ok() {
            self.page.checked.clear();
        }
        self.report(result, &format!("已移除 {} 条词条。", entries.len()));
    }

    pub(super) fn quick_table(&mut self, ui: &mut egui::Ui) {
        let rows = std::mem::take(&mut self.page.rows);
        let mut edit = None;
        let mut remove = None;
        let mut restore = None;
        let mut save = false;
        let mut cancel = false;
        let mut all = !rows.is_empty() && self.page.checked.len() == rows.len();
        let height = (ui.available_height()
            - 32.0
            - theme::CONTROL_HEIGHT
            - 4.0 * ui.spacing().item_spacing.y)
            .max(80.0);
        let word_width = (ui.available_width() * 0.40).clamp(160.0, 560.0);
        let code_width = (ui.available_width() * 0.20).clamp(175.0, 260.0);
        let ledger_painter = ui
            .painter()
            .with_clip_rect(ui.clip_rect().intersect(ui.available_rect_before_wrap()));
        TableBuilder::new(ui)
            .id_salt("ime_quick_rows")
            .striped(!theme::smartisan::active())
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(30.0))
            .column(Column::initial(word_width).at_least(120.0))
            .column(Column::initial(code_width).at_least(175.0))
            .column(Column::remainder().at_least(70.0))
            .column(Column::exact(92.0))
            .max_scroll_height(height)
            .header(32.0, |mut header| {
                header.col(|ui| {
                    theme::smartisan::ledger_header(ui);
                    if ui
                        .checkbox(&mut all, "")
                        .on_hover_text("全选当前结果")
                        .changed()
                    {
                        if all {
                            self.page.checked = rows
                                .iter()
                                .map(|r| (r.code.clone(), r.text.clone()))
                                .collect();
                        } else {
                            self.page.checked.clear();
                        }
                    }
                });
                for title in ["文字", "编码", "来源", "操作"] {
                    header.col(|ui| {
                        theme::smartisan::ledger_header(ui);
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(36.0, rows.len(), |mut row| {
                    let data = &rows[row.index()];
                    let entry = Entry {
                        code: data.code.clone(),
                        text: data.text.clone(),
                    };
                    let key = (data.code.clone(), data.text.clone());
                    let editing = self.page.editing.as_ref() == Some(&entry);
                    row.set_selected(self.page.selected.as_ref() == Some(&entry));
                    row.col(|ui| {
                        let mut checked = self.page.checked.contains(&key);
                        if ui
                            .checkbox(&mut checked, "")
                            .on_hover_text(format!("选择 {} {}", data.text, data.code))
                            .changed()
                        {
                            if checked {
                                self.page.checked.insert(key);
                            } else {
                                self.page.checked.remove(&key);
                            }
                        }
                    });
                    row.col(|ui| {
                        if editing {
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut self.page.edit_text)
                                    .id_salt("ime_inline_text")
                                    .desired_width(ui.available_width()),
                            );
                            if self.page.edit_focus && !self.page.edit_focus_code {
                                response.request_focus();
                                self.page.edit_focus = false;
                            }
                            save |= response.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        } else {
                            let response = ui.add(
                                egui::Label::new(&data.text)
                                    .truncate()
                                    .sense(egui::Sense::click()),
                            );
                            if response.double_clicked() {
                                edit = Some((entry.clone(), false));
                            }
                            if response.clicked() {
                                self.page.selected = Some(entry.clone());
                                surrender_text_focus(ui.ctx());
                            }
                        }
                    });
                    row.col(|ui| {
                        if editing {
                            let response = super::super::exempt(
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.page.edit_code)
                                        .id_salt("ime_inline_code")
                                        .font(egui::TextStyle::Monospace)
                                        .desired_width(90.0),
                                ),
                            );
                            if self.page.edit_focus && self.page.edit_focus_code {
                                response.request_focus();
                                self.page.edit_focus = false;
                            }
                            save |= response.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            save |= ui.small_button("保存").clicked();
                            cancel |= ui.small_button("取消").clicked();
                            cancel |= ui.input(|i| i.key_pressed(egui::Key::Escape));
                        } else {
                            let response = ui.add(
                                egui::Label::new(egui::RichText::new(&data.code).monospace())
                                    .sense(egui::Sense::click()),
                            );
                            if response.double_clicked() {
                                edit = Some((entry.clone(), true));
                            }
                        }
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(data.sources.join("、")).truncate());
                    });
                    row.col(|ui| {
                        if data.position.is_none() {
                            if ui.small_button("恢复").clicked() {
                                restore = Some(entry.clone());
                            }
                        } else {
                            let personal_only = data.sources.iter().all(|s| s == PERSONAL);
                            if ui
                                .small_button(if personal_only { "删除" } else { "移除" })
                                .clicked()
                            {
                                remove = Some(entry.clone());
                            }
                        }
                        ui.menu_button("…", |ui| {
                            if ui.button("编辑").clicked() {
                                edit = Some((entry.clone(), false));
                                ui.close();
                            }
                            if ui.button("候选顺序与查码").clicked() {
                                self.page.selected = Some(entry.clone());
                                self.page.details_open = true;
                                ui.close();
                            }
                        });
                    });
                    theme::smartisan::index_row(
                        &ledger_painter,
                        row.response().rect,
                        self.page.selected.as_ref() == Some(&entry),
                    );
                    if row.response().clicked() {
                        self.page.selected = Some(entry.clone());
                    }
                    if row.response().double_clicked() && edit.is_none() {
                        edit = Some((entry, false));
                    }
                });
            });
        ui.horizontal_wrapped(|ui| {
            ui.weak(format!("共 {} 条", rows.len()));
            ui.label(format!("已选 {} 条", self.page.checked.len()));
            if ui
                .add_enabled(!self.page.checked.is_empty(), egui::Button::new("移除所选"))
                .clicked()
            {
                self.remove_checked();
            }
            ui.weak("修改立即生效，可撤销");
        });
        if rows.is_empty() {
            ui.weak("没有匹配的词条，试试清除筛选，或点击「加词」。");
        }
        self.page.rows = rows;
        if let Some((entry, code_focus)) = edit {
            if self.page.editing.is_some() && self.page.editing.as_ref() != Some(&entry) {
                self.manager.message = "请先保存或取消正在修改的词条。".into();
            } else {
                self.page.edit_code = entry.code.clone();
                self.page.edit_text = entry.text.clone();
                self.page.editing = Some(entry);
                self.page.edit_focus = true;
                self.page.edit_focus_code = code_focus;
            }
        }
        if cancel {
            self.page.editing = None;
        }
        if save && let Some(old) = self.page.editing.clone() {
            let next = Entry {
                code: self.page.edit_code.trim().into(),
                text: self.page.edit_text.trim().into(),
            };
            let result = self.edit_entry(&old, &next);
            if result.is_ok() {
                self.page.editing = None;
                self.page.selected = Some(next);
            }
            self.report(result, "词条已修改。");
        }
        if let Some(entry) = remove {
            let result = self.remove_entries(std::slice::from_ref(&entry));
            self.report(result, &format!("已移除「{}」。", entry.text));
        }
        if let Some(entry) = restore {
            let result = self.set_hidden(&entry, false);
            self.report(result, "词条已恢复。");
        }
    }

    pub(super) fn quick_details(&mut self, ctx: &egui::Context) {
        if !self.page.details_open {
            return;
        }
        let mut open = true;
        egui::Window::new("候选顺序与查码")
            .open(&mut open)
            .default_width(420.0)
            .show(ctx, |ui| {
                self.detail_panel(ui);
            });
        self.page.details_open = open;
    }
}

pub(super) fn text_has_focus(ctx: &egui::Context) -> bool {
    ctx.memory(|m| m.focused())
        .is_some_and(|id| egui::TextEdit::load_state(ctx, id).is_some())
}

fn surrender_text_focus(ctx: &egui::Context) {
    if text_has_focus(ctx) {
        ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
    }
}
