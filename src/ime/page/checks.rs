//! 检查不是审批：逐行修改、忽略，选中的建议一次保存并可一次撤销。
use super::*;
use egui_extras::{Column, TableBuilder};

enum Fix {
    Code(Entry, String),
    Redundant(Entry),
    Hidden(Entry),
    Order(String),
    Inspect(String),
    Missing,
}

struct Issue {
    category: usize,
    text: String,
    code: String,
    suggestion: String,
    note: String,
    fix: Fix,
}

impl Issue {
    fn key(&self) -> (String, String) {
        (
            format!("{}:{}", self.category, self.code),
            self.text.clone(),
        )
    }
    fn ignored_key(&self) -> (String, String, String) {
        let (code, text) = self.key();
        (code, text, self.suggestion.clone())
    }
    fn actionable(&self) -> bool {
        !matches!(self.fix, Fix::Inspect(_) | Fix::Missing)
    }
    fn action_label(&self) -> &'static str {
        match self.fix {
            Fix::Code(..) => "改用建议码",
            Fix::Inspect(_) => "查看与调整",
            Fix::Missing => "更新基础表",
            _ => "清理",
        }
    }
}

impl Ime {
    fn check_rows(&mut self) -> Vec<Issue> {
        if self.page.health.is_none() {
            self.page.health = Some(self.compute_health());
        }
        let health = self.page.health.clone().unwrap_or_default();
        let mut rows = Vec::new();
        for (entry, suggestion, sources) in &health.nonconforming {
            rows.push(Issue {
                category: 1,
                text: entry.text.clone(),
                code: entry.code.clone(),
                suggestion: suggestion.clone(),
                note: format!("与构词规则不符 · {}", sources.join("、")),
                fix: Fix::Code(entry.clone(), suggestion.clone()),
            });
        }
        let overlay = self.table.overlay_report();
        for entry in overlay.redundant {
            rows.push(Issue {
                category: 2,
                text: entry.text.clone(),
                code: entry.code.clone(),
                suggestion: "移除多余个人记录".into(),
                note: "其他来源已收录；清理后仍可输入".into(),
                fix: Fix::Redundant(entry),
            });
        }
        for entry in overlay.stale_hidden {
            rows.push(Issue {
                category: 2,
                text: entry.text.clone(),
                code: entry.code.clone(),
                suggestion: "移除失效屏蔽".into(),
                note: "原词已不存在".into(),
                fix: Fix::Hidden(entry),
            });
        }
        for code in overlay.stale_order {
            rows.push(Issue {
                category: 2,
                text: "失效排序".into(),
                code: code.clone(),
                suggestion: "清理失效排序".into(),
                note: "仅移除已不存在的候选".into(),
                fix: Fix::Order(code),
            });
        }
        for (text, codes) in &health.multi_code {
            rows.push(Issue {
                category: 3,
                text: text.clone(),
                code: codes.join(" / "),
                suggestion: "按需保留".into(),
                note: "同词多码可以是正常用法".into(),
                fix: Fix::Inspect(text.clone()),
            });
        }
        for (code, count) in &health.crowded {
            rows.push(Issue {
                category: 4,
                text: format!("{count} 个候选"),
                code: code.clone(),
                suggestion: "调整候选".into(),
                note: "无需强行消除重码".into(),
                fix: Fix::Inspect(code.clone()),
            });
        }
        for ch in &health.uncoded {
            rows.push(Issue {
                category: 5,
                text: ch.to_string(),
                code: "—".into(),
                suggestion: "更新基础表补字码".into(),
                note: "缺少两码以上的字码，无法自动出码".into(),
                fix: Fix::Missing,
            });
        }
        rows
    }

    pub(super) fn health_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("检查结果");
            if ui.button("重新检查").clicked() {
                self.page.health = None;
                self.page.health_checked.clear();
            }
        });
        ui.weak("点击即可修改；重码、多码不一定需要处理。修改立即生效，可撤销。");
        let all = self.check_rows();
        let ignored = all
            .iter()
            .filter(|row| self.page.health_ignored.contains(&row.ignored_key()))
            .count();
        ui.horizontal_wrapped(|ui| {
            for (category, label) in [
                "全部",
                "编码待核对",
                "重复与失效",
                "同词多码",
                "候选较多",
                "缺少字码",
            ]
            .iter()
            .enumerate()
            {
                let count = all
                    .iter()
                    .filter(|row| {
                        (category == 0 || row.category == category)
                            && !self.page.health_ignored.contains(&row.ignored_key())
                    })
                    .count();
                ui.selectable_value(
                    &mut self.page.health_filter,
                    category,
                    format!("{label} {count}"),
                );
            }
        });
        let rows: Vec<_> = all
            .iter()
            .filter(|row| {
                (self.page.health_filter == 0 || row.category == self.page.health_filter)
                    && !self.page.health_ignored.contains(&row.ignored_key())
            })
            .collect();
        self.page
            .health_checked
            .retain(|key| rows.iter().any(|row| &row.key() == key));
        let mut fix = Vec::new();
        let mut ignore = Vec::new();
        let mut edit = None;
        let mut save = false;
        let mut cancel = false;
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("已选 {} 项", self.page.health_checked.len()));
            let actionable = rows
                .iter()
                .any(|row| self.page.health_checked.contains(&row.key()) && row.actionable());
            if ui
                .add_enabled(actionable, egui::Button::new("处理所选建议"))
                .clicked()
            {
                fix.extend(rows.iter().copied().filter(|row| {
                    self.page.health_checked.contains(&row.key()) && row.actionable()
                }));
            }
            if ui
                .add_enabled(
                    !self.page.health_checked.is_empty(),
                    egui::Button::new("忽略所选"),
                )
                .clicked()
            {
                ignore.extend(
                    rows.iter()
                        .copied()
                        .filter(|row| self.page.health_checked.contains(&row.key())),
                );
            }
        });
        let mut all_checked = !rows.is_empty() && rows.len() == self.page.health_checked.len();
        let table_height = (ui.available_height()
            - 30.0
            - theme::CONTROL_HEIGHT
            - 4.0 * ui.spacing().item_spacing.y)
            .max(80.0);
        let word_width = (ui.available_width() * 0.26).clamp(100.0, 360.0);
        let ledger_painter = ui
            .painter()
            .with_clip_rect(ui.clip_rect().intersect(ui.available_rect_before_wrap()));
        TableBuilder::new(ui)
            .id_salt("ime_quick_checks")
            .striped(!theme::smartisan::active())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(28.0))
            .column(Column::initial(word_width).at_least(100.0))
            .column(Column::initial(145.0).at_least(75.0))
            .column(Column::initial(135.0).at_least(65.0))
            .column(Column::remainder().at_least(90.0))
            .column(Column::exact(170.0))
            .max_scroll_height(table_height)
            .header(30.0, |mut header| {
                header.col(|ui| {
                    theme::smartisan::ledger_header(ui);
                    if ui.checkbox(&mut all_checked, "").changed() {
                        if all_checked {
                            self.page.health_checked = rows.iter().map(|row| row.key()).collect();
                        } else {
                            self.page.health_checked.clear();
                        }
                    }
                });
                for title in ["词条", "当前编码", "建议", "说明", "操作"] {
                    header.col(|ui| {
                        theme::smartisan::ledger_header(ui);
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(36.0, rows.len(), |mut row| {
                    let data = rows[row.index()];
                    row.col(|ui| {
                        let mut checked = self.page.health_checked.contains(&data.key());
                        if ui.checkbox(&mut checked, "").changed() {
                            if checked {
                                self.page.health_checked.insert(data.key());
                            } else {
                                self.page.health_checked.remove(&data.key());
                            }
                        }
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(&data.text).truncate());
                    });
                    row.col(|ui| {
                        if let Fix::Code(entry, _) = &data.fix {
                            if self.page.health_editing.as_ref() == Some(entry) {
                                let response = super::super::exempt(
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.page.health_edit_code)
                                            .id_salt("ime_check_edit")
                                            .desired_width(85.0),
                                    ),
                                );
                                if self.page.health_edit_focus {
                                    response.request_focus();
                                    self.page.health_edit_focus = false;
                                }
                                save |= ui.small_button("保存").clicked()
                                    || (response.lost_focus()
                                        && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                                cancel |= ui.input(|i| i.key_pressed(egui::Key::Escape));
                            } else if ui
                                .add(
                                    egui::Label::new(egui::RichText::new(&data.code).monospace())
                                        .sense(egui::Sense::click()),
                                )
                                .double_clicked()
                            {
                                edit = Some(entry.clone());
                            }
                        } else {
                            ui.monospace(&data.code);
                        }
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(&data.suggestion).truncate());
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(&data.note).truncate())
                            .on_hover_text(&data.note);
                    });
                    row.col(|ui| {
                        if ui.small_button(data.action_label()).clicked() {
                            fix.push(data);
                        }
                        if ui.small_button("忽略").clicked() {
                            ignore.push(data);
                        }
                    });
                    theme::smartisan::index_row(
                        &ledger_painter,
                        row.response().rect,
                        self.page.health_checked.contains(&data.key()),
                    );
                });
            });
        if rows.is_empty() {
            ui.weak("这一类没有待处理项。");
        }
        ui.horizontal(|ui| {
            ui.weak(format!("已忽略 {ignored} 项（仅本次运行）"));
            if ignored > 0 && ui.small_button("重新显示").clicked() {
                self.page.health_ignored.clear();
            }
        });
        for issue in ignore {
            self.page.health_ignored.insert(issue.ignored_key());
        }
        if let Some(entry) = edit {
            self.page.health_edit_code = entry.code.clone();
            self.page.health_editing = Some(entry);
            self.page.health_edit_focus = true;
        }
        if cancel {
            self.page.health_editing = None;
        }
        if save && let Some(old) = self.page.health_editing.clone() {
            let next = Entry {
                code: self.page.health_edit_code.trim().into(),
                text: old.text.clone(),
            };
            let result = self.edit_entry(&old, &next);
            if result.is_ok() {
                self.page.health_editing = None;
            }
            self.report(result, "编码已修改。");
        }
        if !fix.is_empty() {
            let result = self.apply_issues(&fix);
            self.report(result, "所选建议已处理。");
        }
    }

    fn apply_issues(&mut self, rows: &[&Issue]) -> anyhow::Result<()> {
        let overlay = self.table.overlay_report();
        if rows.iter().all(|row| row.category == 2)
            && rows.len()
                == overlay.redundant.len() + overlay.stale_hidden.len() + overlay.stale_order.len()
        {
            return self.cleanup_overlay().map(|_| ());
        }
        let changes: Vec<_> = rows
            .iter()
            .filter_map(|row| {
                if let Fix::Code(entry, code) = &row.fix {
                    Some((
                        entry.clone(),
                        Entry {
                            code: code.clone(),
                            text: entry.text.clone(),
                        },
                    ))
                } else {
                    None
                }
            })
            .collect();
        let mut personal = self.edited_personal(&changes)?;
        for row in rows {
            match &row.fix {
                Fix::Code(..) => {}
                Fix::Redundant(entry) => personal.entries.retain(|e| e != entry),
                Fix::Hidden(entry) => personal.hidden.retain(|e| e != entry),
                Fix::Order(code) => {
                    if let Some(words) = personal.order.get_mut(code) {
                        words.retain(|word| self.table.all(code).iter().any(|c| &c.text == word));
                        if words.len() <= 1 {
                            personal.order.remove(code);
                        }
                    }
                }
                Fix::Inspect(text) => {
                    self.show_lookup_tab(text);
                    if let Some(first) = self.table.lookup(text).first() {
                        self.page.selected = Some(Entry {
                            code: text.clone(),
                            text: first.text.clone(),
                        });
                        self.page.details_open = true;
                    }
                }
                Fix::Missing => self.stage_import(false),
            }
        }
        self.save_personal(
            personal,
            &format!(
                "处理 {} 项词表检查建议",
                rows.iter().filter(|r| r.actionable()).count()
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ime::{ImeSettings, table};

    #[test]
    fn check_batch_applies_immediately_and_one_undo_restores_every_item() {
        let dir = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(dir.path().to_path_buf()));
        let mut ime = Ime::bare(ImeSettings::default());
        ime.table.base = table::parse("vifo,1=指\ndcsc,1=导\nvidc,1=指导", false).entries;
        ime.table.personal.entries = table::parse("vidd,1=指导", false).entries;
        ime.table.personal.hidden = table::parse("xxxx,1=旧词", false).entries;
        ime.table.rebuild();
        ime.rebuild_encoder();
        let before = ime.table.personal.clone();
        let issues = ime.check_rows();
        assert_eq!(issues.len(), 3); // 错码、失效屏蔽与同词多码。
        let fixes: Vec<_> = issues.iter().filter(|r| r.actionable()).collect();
        ime.apply_issues(&fixes).unwrap();
        assert!(ime.table.lookup("vidd").is_empty());
        assert_eq!(ime.table.lookup("vidc")[0].text, "指导");
        assert!(ime.table.personal.hidden.iter().all(|e| e.code != "xxxx"));
        ime.undo_personal().unwrap();
        assert_eq!(ime.table.personal, before);
        assert!(ime.manager.undo.last().is_none());
        crate::storage::set_test_config_dir(None);
    }
}
