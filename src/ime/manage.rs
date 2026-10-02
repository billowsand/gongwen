//! 词表管理的各个分区：导入与升级、导入表来源、个人层体检、修改记录、构词规则。
//!
//! 基础表只读，只能整体导入更新；日常修改都在个人层（见 `ops.rs`）。
use super::{
    data, history,
    session::Ime,
    table::{self, BaseDiff, Candidate, Entry, OverlayReport, Parsed},
};
use crate::theme;
use eframe::egui;

enum RulesAction {
    Edit,
    Save,
    Cancel,
    Reset,
}

struct Staged {
    name: String,
    base: bool,
    parsed: Parsed,
    summary: (usize, usize, usize),
    /// 升级基础表时与当前基础表的差异，以及升级后个人层里会多出的多余、失效记录。
    upgrade: Option<(BaseDiff, OverlayReport)>,
}

#[derive(Default)]
pub(super) struct Manager {
    query: String,
    conflicts: bool,
    /// 词表变了：搜索结果、导入预览、体检与修改记录都要重算。
    pub dirty: bool,
    searched: bool,
    results: Vec<(String, Candidate)>,
    code: String,
    text: String,
    old: Option<Entry>,
    staged: Option<Staged>,
    pub message: String,
    history: Vec<history::Record>,
    overlay: OverlayReport,
    loaded: bool,
    rules_edit: Option<[String; 3]>,
}

impl Ime {
    /// 设置页里的词表管理。
    pub(crate) fn manager_ui(&mut self, ui: &mut egui::Ui) {
        self.refresh_manager();
        self.message_ui(ui);
        self.import_section(ui);
        self.batches_section(ui);
        self.rules_summary(ui);
        self.search_section(ui);
        self.editor_section(ui);
        self.overlay_section(ui);
        self.history_section(ui);
    }

    /// 词表变了之后重算预览、体检与修改记录。每帧调一次，没变是空操作。
    pub(super) fn refresh_manager(&mut self) {
        if self.manager.loaded && !self.manager.dirty {
            return;
        }
        if let Some(staged) = &mut self.manager.staged {
            staged.summary = self.table.preview(&staged.parsed, staged.base);
        }
        self.manager.overlay = self.table.overlay_report();
        self.manager.history = data::directory()
            .map(|dir| history::list(&dir))
            .unwrap_or_default();
        self.manager.loaded = true;
        // 搜索结果由 `search_section` 按 `dirty` 自己重算后清掉标记。
    }

    pub(super) fn message_ui(&mut self, ui: &mut egui::Ui) {
        if !self.manager.message.is_empty() {
            ui.label(&self.manager.message);
        }
        if let Some(error) = &self.load_error {
            ui.colored_label(theme::warn(), error);
        }
    }

    /// 导入、升级与备份。
    pub(super) fn import_section(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("导入 / 升级基础表").clicked() {
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
                    Ok(()) => "公文导入表、个人词条、屏蔽、排序与构词规则已备份。".into(),
                    Err(e) => e.to_string(),
                };
            }
            if ui.button("恢复个人备份").clicked() {
                let result = self.restore_backup();
                self.report(result, "个人备份已恢复；恢复前的状态可在修改记录里找回。");
            }
            if ui
                .button("撤销上次基础表升级")
                .on_hover_text("换回 base.previous.txt；个人层不受影响")
                .clicked()
            {
                let result = (|| -> anyhow::Result<()> {
                    let path = data::directory()?.join("base.previous.txt");
                    let entries =
                        table::parse(&crate::text_file::read_to_string(&path)?, false).entries;
                    anyhow::ensure!(!entries.is_empty(), "上次基础表没有有效记录");
                    self.replace_base(entries)
                })();
                self.report(result, "基础表已换回上一版。");
            }
        });
        self.staged_ui(ui);
    }

    fn staged_ui(&mut self, ui: &mut egui::Ui) {
        let Some(Staged {
            name,
            base,
            parsed,
            summary: (added, existing, conflicts),
            upgrade,
        }) = self.manager.staged.as_ref()
        else {
            return;
        };
        let mut confirm = false;
        let mut cancel = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(format!(
                "{name}：有效 {} 条，新增 {added} 条，已有 {existing} 条，涉及重码 {conflicts} 组；\
                 文件内重复 {} 条，无效 {} 行。",
                parsed.entries.len(),
                parsed.duplicates,
                parsed.invalid
            ));
            if *base {
                if let Some((diff, overlay)) = upgrade {
                    ui.label(format!(
                        "与当前基础表相比：新增 {} 条，删除 {} 条，换位 {} 条。",
                        diff.added, diff.removed, diff.moved
                    ));
                    if overlay.is_empty() {
                        ui.weak("个人词条、屏蔽与排序都保留，升级后没有多余或失效的记录。");
                    } else {
                        ui.weak(format!(
                            "个人词条、屏蔽与排序都保留；升级后{}，可在体检里清理。",
                            overlay.describe()
                        ));
                    }
                }
                ui.weak("确认后替换基础表，原基础表保存为 base.previous.txt。");
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
            ui.horizontal(|ui| {
                confirm = ui
                    .add_enabled(!parsed.entries.is_empty(), egui::Button::new("确认导入"))
                    .clicked();
                cancel = ui.button("取消").clicked();
            });
        });
        if confirm {
            let Staged {
                name, base, parsed, ..
            } = self.manager.staged.take().expect("上面已确认有暂存");
            let result = if base {
                self.replace_base(parsed.entries)
            } else {
                self.add_batch(name, parsed.entries)
            };
            self.report(result, "词表已导入，立即生效。");
        } else if cancel {
            self.manager.staged = None;
        }
    }

    /// 导入的公文四码表：分批启停与撤销。
    pub(super) fn batches_section(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.label("导入的公文表（停用保留词条；撤销整批移除）");
        if self.table.personal.batches.is_empty() {
            ui.weak("还没有导入过。");
        }
        let batches: Vec<(String, String, bool, usize)> = self
            .table
            .personal
            .batches
            .iter()
            .map(|b| (b.id.clone(), b.name.clone(), b.enabled, b.entries.len()))
            .collect();
        for (id, name, enabled, count) in batches {
            let mut checked = enabled;
            let mut remove = false;
            ui.horizontal(|ui| {
                ui.checkbox(&mut checked, &name);
                ui.weak(format!("{count} 条"));
                remove = ui.button("撤销导入").clicked();
            });
            if checked != enabled || remove {
                let result = self.update_batch(&id, (!remove).then_some(checked));
                self.report(result, "公文导入表已更新。");
            }
        }
    }

    /// 个人层体检：多余与失效记录，一键清理。
    pub(super) fn overlay_section(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        let overlay = self.manager.overlay.clone();
        if overlay.is_empty() {
            ui.weak("个人层没有多余或失效的记录。");
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("个人层：{}。", overlay.describe()));
            if ui
                .button("清理")
                .on_hover_text("删掉已被其他来源收录的个人词条、失效屏蔽，排序里去掉已不存在的词")
                .clicked()
            {
                let result = self.cleanup_overlay();
                self.manager.message = match result {
                    Ok(count) => format!("已清理 {count} 项；可在修改记录里回退。"),
                    Err(error) => format!("操作失败：{error}"),
                };
            }
        });
    }

    /// 最近的修改记录，可以逐条回退。
    pub(super) fn history_section(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.label(format!("修改记录（最近 {} 次）", history::KEEP));
        if self.manager.history.is_empty() {
            ui.weak("还没有修改记录。");
            return;
        }
        let records = self.manager.history.clone();
        let mut restore = None;
        egui::ScrollArea::vertical()
            .id_salt("ime_history")
            .max_height(200.0)
            .show(ui, |ui| {
                for record in &records {
                    ui.horizontal(|ui| {
                        ui.monospace(&record.time);
                        ui.label(&record.label);
                        if ui
                            .small_button("回到此前")
                            .on_hover_text("个人层回到这次修改之前；回退本身也会记一笔")
                            .clicked()
                        {
                            restore = Some(record.id.clone());
                        }
                    });
                }
            });
        if let Some(id) = restore {
            let result = self.restore_history(&id);
            self.report(result, "已回退；回退前的状态也记在修改记录里。");
        }
    }

    /// 构词规则与它在基础表词组上的命中率；可以手动改写。
    pub(super) fn rules_summary(&mut self, ui: &mut egui::Ui) {
        self.rules_overview(ui);
        let mut action = None;
        match &mut self.manager.rules_edit {
            None => {
                ui.horizontal(|ui| {
                    if ui.small_button("改写规则…").clicked() {
                        action = Some(RulesAction::Edit);
                    }
                    if self.encoder.overridden && ui.small_button("恢复按基础表推算").clicked()
                    {
                        action = Some(RulesAction::Reset);
                    }
                });
            }
            Some(texts) => {
                ui.horizontal_wrapped(|ui| {
                    for (class, text) in texts.iter_mut().enumerate() {
                        ui.label(super::encoder::RULE_LABELS[class]);
                        super::exempt(
                            ui.add(
                                egui::TextEdit::singleline(text)
                                    .desired_width(90.0)
                                    .font(egui::TextStyle::Monospace),
                            ),
                        );
                    }
                    if ui.button("保存规则").clicked() {
                        action = Some(RulesAction::Save);
                    }
                    if ui.button("取消").clicked() {
                        action = Some(RulesAction::Cancel);
                    }
                });
                ui.weak("每两个字母一组：大写选字（A 首字、B 次字、Z 末字），小写选码（a 首码、b 第二码、z 末码）。");
            }
        }
        match action {
            Some(RulesAction::Edit) => self.manager.rules_edit = Some(self.encoder.rules.texts()),
            Some(RulesAction::Cancel) => self.manager.rules_edit = None,
            Some(RulesAction::Reset) => {
                let result = self.set_rules(None);
                self.report(result, "构词规则已恢复为按基础表推算。");
            }
            Some(RulesAction::Save) => {
                let texts = self.manager.rules_edit.clone().unwrap_or_default();
                let result = self.set_rules(Some(texts.map(|t| t.trim().to_string())));
                if result.is_ok() {
                    self.manager.rules_edit = None;
                }
                self.report(result, "构词规则已保存，加词出码立即按新规则。");
            }
            None => {}
        }
    }

    fn rules_overview(&self, ui: &mut egui::Ui) {
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

    fn search_section(&mut self, ui: &mut egui::Ui) {
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
                        }
                        if ui
                            .small_button(if hidden { "恢复" } else { "屏蔽" })
                            .clicked()
                        {
                            let result = self.set_hidden(&entry, !hidden);
                            self.report(result, "词条状态已更新。");
                        }
                        if ui.small_button("首选").clicked() {
                            self.reorder(&code, &candidate.text, 0);
                        }
                        if ui.small_button("↑").clicked() {
                            self.reorder(&code, &candidate.text, -1);
                        }
                        if ui.small_button("↓").clicked() {
                            self.reorder(&code, &candidate.text, 1);
                        }
                        if candidate.sources.iter().any(|s| s == table::PERSONAL)
                            && ui.small_button("删除个人词").clicked()
                        {
                            let result = self.remove_personal(&entry);
                            self.report(result, "个人词条已删除。");
                        }
                    });
                }
            });
        ui.weak("每次最多显示 200 条；输入编码可查看该码的全部候选来源和顺序。");
        if ui.button("恢复当前编码默认顺序").clicked() {
            let code = self.manager.query.trim().to_string();
            let result = self.reset_order(&code);
            self.report(result, "当前编码顺序已恢复。");
        }
    }

    fn editor_section(&mut self, ui: &mut egui::Ui) {
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
            if ui.button("保存").clicked() {
                self.save_editor();
            }
            if ui.button("清空").clicked() {
                self.manager.old = None;
                self.manager.code.clear();
                self.manager.text.clear();
            }
        });
        ui.weak(
            "词条为 1–4 码。基础表只读：改动基础表里的词会屏蔽原词、另存一条个人词条，\
             升级基础表时不会丢。",
        );
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
                let upgrade = (!four).then(|| self.upgrade_preview(&parsed.entries));
                self.manager.staged = Some(Staged {
                    name: path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    base: !four,
                    parsed,
                    summary,
                    upgrade,
                });
            }
            Err(error) => self.manager.message = error.to_string(),
        }
    }

    /// 换成新基础表后的差异与个人层影响：拿新表与现有个人层另建一张表来算。
    fn upgrade_preview(&self, entries: &[Entry]) -> (BaseDiff, OverlayReport) {
        let next = table::Table::with(
            entries.to_vec(),
            self.table.document.clone(),
            self.table.personal.clone(),
        );
        (
            table::base_diff(&self.table.base, entries),
            next.overlay_report(),
        )
    }

    /// 整体替换基础表。旧表存为 `base.previous.txt`，个人层原样保留。
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

    /// 从备份恢复个人层。个人文件读坏时也能用，所以不走 `save_personal` 的读坏检查。
    fn restore_backup(&mut self) -> anyhow::Result<()> {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("词表备份", &["json"])
            .pick_file()
        else {
            return Ok(());
        };
        let personal = serde_json::from_slice::<table::Personal>(&std::fs::read(path)?)?;
        super::ops::validate(&personal)?;
        let dir = data::directory()?;
        // 恢复前的个人文件记进修改记录，误选了备份还能找回。
        history::record(&dir, "恢复个人备份")?;
        data::save_json(&dir.join("tables.json"), &personal)?;
        self.table.personal = personal;
        self.storage_ok = true;
        self.load_error = None;
        self.table.rebuild();
        self.rebuild_encoder();
        self.manager.dirty = true;
        self.refresh(true);
        Ok(())
    }

    fn save_editor(&mut self) {
        let entry = Entry {
            code: self.manager.code.trim().into(),
            text: self.manager.text.trim().into(),
        };
        let result = match self.manager.old.clone() {
            Some(old) => self.edit_entry(&old, &entry),
            None => self.add_entry(&entry, false),
        };
        if result.is_ok() {
            self.manager.old = None;
            self.manager.text.clear();
            self.manager.code.clear();
        }
        self.report(result, "词条已保存，立即生效。");
    }

    fn reorder(&mut self, code: &str, text: &str, delta: isize) {
        let result = self.move_word(code, text, delta);
        self.report(result, "候选顺序已保存。");
    }

    pub(super) fn report(&mut self, result: anyhow::Result<()>, success: &str) {
        self.manager.message = match result {
            Ok(()) => success.into(),
            Err(error) => format!("操作失败：{error}"),
        };
    }
}
