//! 词表管理：导入预览、基础表维护、重码排序与来源撤销。
use super::{
    data,
    session::Ime,
    table::{self, Batch, Candidate, Entry, Parsed},
};
use eframe::egui;

struct Staged {
    name: String,
    base: bool,
    parsed: Parsed,
    summary: (usize, usize, usize),
}

#[derive(Default)]
pub(super) struct Manager {
    query: String,
    conflicts: bool,
    pub dirty: bool,
    searched: bool,
    results: Vec<(String, Candidate)>,
    code: String,
    text: String,
    edit_base: bool,
    old: Option<Entry>,
    staged: Option<Staged>,
    message: String,
}

impl Ime {
    pub(crate) fn manager_ui(&mut self, ui: &mut egui::Ui) {
        if self.manager.dirty
            && let Some(Staged {
                base,
                parsed,
                summary,
                ..
            }) = &mut self.manager.staged
        {
            *summary = self.table.preview(parsed, *base);
        }
        ui.horizontal_wrapped(|ui| {
            if ui.button("导入 / 更新基础表").clicked() {
                self.stage_import(false);
            }
            if ui.button("导入公文四码表").clicked() {
                self.stage_import(true);
            }
            if ui.button("导出最终词表").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .set_file_name("词表输入法-最终词表.txt")
                    .save_file()
            {
                self.manager.message = match data::save_bytes(&path, self.table.export().as_bytes())
                {
                    Ok(()) => "最终词表已导出。".into(),
                    Err(e) => e.to_string(),
                };
            }
            if ui.button("导出个人备份").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .set_file_name("词表输入法-个人备份.json")
                    .save_file()
            {
                self.manager.message = match data::save_json(&path, &self.table.personal) {
                    Ok(()) => "公文导入表、个人词条、屏蔽与排序已备份。".into(),
                    Err(e) => e.to_string(),
                };
            }
            if ui.button("恢复个人备份").clicked() {
                let result = (|| -> anyhow::Result<()> {
                    let Some(path) = rfd::FileDialog::new()
                        .add_filter("词表备份", &["json"])
                        .pick_file()
                    else {
                        return Ok(());
                    };
                    let personal =
                        serde_json::from_slice::<table::Personal>(&std::fs::read(path)?)?;
                    anyhow::ensure!(
                        personal
                            .entries
                            .iter()
                            .chain(personal.hidden.iter())
                            .all(|e| table::valid_entry(e, false))
                            && personal
                                .batches
                                .iter()
                                .all(|b| b.entries.iter().all(|e| table::valid_entry(e, true))),
                        "备份中有无效词条"
                    );
                    // 备份恢复前先保存当前版本，避免误选文件后无法找回。
                    let dir = data::directory()?;
                    if dir.join("tables.json").is_file() {
                        data::save_bytes(
                            &dir.join("tables.previous.json"),
                            &std::fs::read(dir.join("tables.json"))?,
                        )?;
                    }
                    data::save_json(&dir.join("tables.json"), &personal)?;
                    self.table.personal = personal;
                    self.storage_ok = true;
                    self.load_error = None;
                    self.table.rebuild();
                    self.rebuild_encoder();
                    self.manager.dirty = true;
                    self.refresh(true);
                    Ok(())
                })();
                self.report(result, "个人备份已恢复。");
            }
        });
        if let Some(Staged {
            name,
            base,
            parsed,
            summary: (added, existing, conflicts),
        }) = self.manager.staged.as_ref()
        {
            ui.label(format!("{name}：有效 {} 条，新增 {added} 条，已有 {existing} 条，涉及重码 {conflicts} 组；文件内重复 {} 条，无效 {} 行。",parsed.entries.len(),parsed.duplicates,parsed.invalid));
            if *base {
                ui.label("确认后替换基础表，原基础表保存在 base.previous.txt。");
            }
            ui.collapsing("查看词条与当前候选（前 20 条）", |ui| {
                for entry in parsed.entries.iter().take(20) {
                    let existing: Vec<String> = self
                        .table
                        .lookup(&entry.code)
                        .into_iter()
                        .map(|c| c.text)
                        .collect();
                    ui.label(format!(
                        "{} → {}；当前候选：{}",
                        entry.code,
                        entry.text,
                        existing.join("、")
                    ));
                }
            });
            let mut confirm = false;
            let mut cancel = false;
            ui.horizontal(|ui| {
                confirm = ui
                    .add_enabled(!parsed.entries.is_empty(), egui::Button::new("确认导入"))
                    .clicked();
                cancel = ui.button("取消").clicked();
            });
            if confirm {
                let Staged {
                    name, base, parsed, ..
                } = self.manager.staged.take().unwrap();
                let result = if base {
                    self.replace_base(parsed.entries)
                } else {
                    let mut personal = self.table.personal.clone();
                    personal.batches.push(Batch {
                        id: uuid::Uuid::new_v4().to_string(),
                        name,
                        enabled: true,
                        entries: parsed.entries,
                    });
                    self.save_personal(personal)
                };
                self.report(result, "词表已导入，立即生效。");
            } else if cancel {
                self.manager.staged = None;
            }
        }
        if !self.manager.message.is_empty() {
            ui.label(&self.manager.message);
        }
        if let Some(error) = &self.load_error {
            ui.colored_label(crate::theme::warn(), error);
        }
        self.rules_summary(ui);

        ui.add_space(8.0);
        ui.label("导入的公文表（停用保留词条；撤销整批移除）");
        let batches = self.table.personal.batches.clone();
        for batch in batches {
            let mut enabled = batch.enabled;
            let mut remove = false;
            ui.horizontal(|ui| {
                ui.checkbox(&mut enabled, &batch.name);
                ui.weak(format!("{} 条", batch.entries.len()));
                remove = ui.button("撤销导入").clicked();
            });
            if enabled != batch.enabled || remove {
                let mut personal = self.table.personal.clone();
                if remove {
                    personal.batches.retain(|b| b.id != batch.id);
                } else if let Some(b) = personal.batches.iter_mut().find(|b| b.id == batch.id) {
                    b.enabled = enabled;
                }
                let result = self.save_personal(personal);
                self.report(result, "公文词表已更新。");
            }
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("查词 / 查码");
            // 编码与汉字都可查询，但查询框不接管键盘；可切系统输入或粘贴汉字。
            if super::exempt(
                ui.add(
                    egui::TextEdit::singleline(&mut self.manager.query)
                        .hint_text("输入完整编码或文字")
                        .desired_width(190.0),
                ),
            )
            .changed()
            {
                self.manager.dirty = true;
            }
            if ui
                .checkbox(&mut self.manager.conflicts, "只看重码")
                .changed()
            {
                self.manager.dirty = true;
            }
        });
        if self.manager.dirty || !self.manager.searched {
            self.manager.results = self
                .table
                .search(self.manager.query.trim(), self.manager.conflicts);
            self.manager.dirty = false;
            self.manager.searched = true;
        }
        let results = self.manager.results.clone();
        egui::ScrollArea::vertical()
            .id_salt("ime_table_results")
            .max_height(230.0)
            .show(ui, |ui| {
                for (code, candidate) in results {
                    let entry = Entry {
                        code: code.clone(),
                        text: candidate.text.clone(),
                    };
                    let hidden = self.table.hidden(&code, &candidate.text);
                    ui.horizontal_wrapped(|ui| {
                        ui.monospace(&code);
                        ui.label(&candidate.text);
                        ui.weak(candidate.sources.join("、"));
                        if ui.small_button("修改").clicked() {
                            self.manager.code = code.clone();
                            self.manager.text = candidate.text.clone();
                            self.manager.old = Some(entry.clone());
                            self.manager.edit_base =
                                candidate.sources.iter().any(|s| s == "基础表");
                        }
                        if candidate.sources.iter().any(|source| source == "基础表")
                            && ui.small_button("删除基础词").clicked()
                        {
                            let mut entries = self.table.base.clone();
                            entries.retain(|e| e != &entry);
                            let result = self.replace_base(entries);
                            self.report(result, "基础词条已删除，可撤销上次基础表修改。");
                        }
                        if ui
                            .small_button(if hidden { "恢复" } else { "停用" })
                            .clicked()
                        {
                            let mut personal = self.table.personal.clone();
                            if hidden {
                                personal.hidden.retain(|e| e != &entry);
                            } else {
                                personal.hidden.push(entry.clone());
                            }
                            let result = self.save_personal(personal);
                            self.report(result, "词条状态已更新。");
                        }
                        if ui.small_button("首选").clicked() {
                            self.move_word(&code, &candidate.text, 0);
                        }
                        if ui.small_button("↑").clicked() {
                            self.move_word(&code, &candidate.text, -1);
                        }
                        if ui.small_button("↓").clicked() {
                            self.move_word(&code, &candidate.text, 1);
                        }
                        if candidate.sources.iter().any(|s| s == "个人词条")
                            && ui.small_button("删除个人词").clicked()
                        {
                            let mut personal = self.table.personal.clone();
                            personal.entries.retain(|e| e != &entry);
                            let result = self.save_personal(personal);
                            self.report(result, "个人词条已删除。");
                        }
                    });
                }
            });
        ui.weak("每次最多显示 200 条；输入编码可查看该码的全部候选来源和顺序。");
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(if self.manager.old.is_some() {
                "修改词条"
            } else {
                "添加词条"
            });
            super::exempt(
                ui.add(
                    egui::TextEdit::singleline(&mut self.manager.code)
                        .hint_text("编码")
                        .desired_width(70.0),
                ),
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.manager.text)
                    .hint_text("上屏文字")
                    .desired_width(180.0),
            );
            ui.checkbox(&mut self.manager.edit_base, "写入基础表");
            if ui.button("保存").clicked() {
                self.save_editor();
            }
            if ui.button("清空").clicked() {
                self.manager.old = None;
                self.manager.code.clear();
                self.manager.text.clear();
            }
        });
        ui.weak("基础表与个人词条接受 1–4 码，公文导入表使用四码。基础表修改后保存在用户目录，不覆盖初始资源文件。");
        ui.horizontal(|ui| {
            if ui.button("撤销上次基础表修改").clicked() {
                let result = (|| -> anyhow::Result<()> {
                    let path = data::directory()?.join("base.previous.txt");
                    let entries =
                        table::parse(&crate::text_file::read_to_string(&path)?, false).entries;
                    anyhow::ensure!(!entries.is_empty(), "上次基础表没有有效记录");
                    self.replace_base(entries)
                })();
                self.report(result, "基础表已恢复。");
            }
            if ui.button("恢复当前编码默认顺序").clicked() {
                let mut personal = self.table.personal.clone();
                personal.order.remove(self.manager.query.trim());
                let result = self.save_personal(personal);
                self.report(result, "当前编码顺序已恢复。");
            }
        });
    }
    /// 构词规则与它在基础表词组上的命中率。
    pub(super) fn rules_summary(&self, ui: &mut egui::Ui) {
        let encoder = &self.encoder;
        ui.add_space(8.0);
        ui.label(if encoder.overridden {
            "构词规则（手动指定）"
        } else {
            "构词规则（按基础表推算）"
        });
        let inferred = encoder.inferred.texts();
        for (class, text) in encoder.rules.texts().iter().enumerate() {
            let stat = encoder.stats[class];
            let rate = if stat.checked == 0 {
                "基础表中没有这类词组".to_string()
            } else {
                format!(
                    "基础表 {} 个词组中相符 {}（{:.1}%）",
                    stat.checked,
                    stat.matched,
                    stat.matched as f64 * 100.0 / stat.checked as f64
                )
            };
            let note = if encoder.overridden && inferred[class] != *text {
                format!("；推算结果为 {}", inferred[class])
            } else {
                String::new()
            };
            ui.weak(format!(
                "{} {text}：{}；{rate}{note}",
                super::encoder::RULE_LABELS[class],
                encoder.rules.0[class].describe()
            ));
        }
    }
    fn stage_import(&mut self, four: bool) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("词表", &["txt", "tsv"])
            .pick_file()
        else {
            return;
        };
        match crate::text_file::read_to_string(&path) {
            Ok(text) => {
                let parsed = table::parse(&text, four);
                let summary = self.table.preview(&parsed, !four);
                self.manager.staged = Some(Staged {
                    name: path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    base: !four,
                    parsed,
                    summary,
                });
            }
            Err(error) => self.manager.message = error.to_string(),
        }
    }
    fn replace_base(&mut self, entries: Vec<Entry>) -> anyhow::Result<()> {
        let path = data::directory()?.join("base.txt");
        if path.exists() {
            data::save_bytes(
                &path.with_file_name("base.previous.txt"),
                &std::fs::read(&path)?,
            )?;
        }
        data::save_base(&entries)?;
        self.table.base = entries;
        if self.storage_ok {
            self.load_error = None;
        }
        self.table.rebuild();
        self.rebuild_encoder();
        self.refresh(true);
        self.manager.dirty = true;
        Ok(())
    }
    fn save_editor(&mut self) {
        let entry = Entry {
            code: self.manager.code.trim().into(),
            text: self.manager.text.trim().into(),
        };
        if !table::valid_entry(&entry, false) {
            self.manager.message =
                "编码应为 1–4 个小写字母，文字不能为空或包含换行 / 制表符。".into();
            return;
        }
        let result = if self.manager.edit_base {
            let mut entries = self.table.base.clone();
            if let Some(index) = self
                .manager
                .old
                .as_ref()
                .and_then(|old| entries.iter().position(|e| e == old))
            {
                entries[index] = entry.clone();
            } else if !entries.contains(&entry) {
                entries.push(entry);
            }
            let mut seen = std::collections::BTreeSet::new();
            entries.retain(|e| seen.insert((e.code.clone(), e.text.clone())));
            self.replace_base(entries)
        } else {
            let mut personal = self.table.personal.clone();
            if let Some(old) = &self.manager.old {
                personal.entries.retain(|e| e != old);
                if !personal.hidden.contains(old) {
                    personal.hidden.push(old.clone());
                }
            }
            personal.hidden.retain(|e| e != &entry);
            if !personal.entries.contains(&entry) {
                personal.entries.push(entry);
            }
            self.save_personal(personal)
        };
        if result.is_ok() {
            self.manager.old = None;
            self.manager.text.clear();
            self.manager.code.clear();
        }
        self.report(result, "词条已保存，立即生效。");
    }
    fn move_word(&mut self, code: &str, text: &str, delta: isize) {
        let mut order: Vec<String> = self
            .table
            .all(code)
            .iter()
            .map(|c| c.text.clone())
            .collect();
        if let Some(index) = order.iter().position(|w| w == text) {
            let target = if delta == 0 {
                0
            } else {
                (index as isize + delta).clamp(0, order.len().saturating_sub(1) as isize) as usize
            };
            let word = order.remove(index);
            order.insert(target, word);
            let mut personal = self.table.personal.clone();
            personal.order.insert(code.into(), order);
            let result = self.save_personal(personal);
            self.report(result, "候选顺序已保存。");
        }
    }
    fn report(&mut self, result: anyhow::Result<()>, success: &str) {
        self.manager.message = match result {
            Ok(()) => success.into(),
            Err(error) => format!("操作失败：{error}"),
        };
    }
}
