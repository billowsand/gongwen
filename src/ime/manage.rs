//! 词表管理的各个分区：导入与升级、导入表来源、个人层体检、修改记录、构词规则。
//!
//! 基础表只读，只能整体导入更新；日常修改都在个人层（见 `ops.rs`）。
use super::{
    data, history,
    page::PageAction,
    session::Ime,
    table::{self, BaseDiff, Entry, OverlayReport, Parsed},
};
use eframe::egui;
use std::path::Path;

enum RulesAction {
    Edit,
    Save,
    Cancel,
    Reset,
}

#[derive(Clone)]
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
    pub(super) undo: super::undo::UndoStack,
    /// 词表变了：查询结果、导入预览、体检与修改记录都要重算。
    pub dirty: bool,
    staged: Option<Staged>,
    pub message: String,
    history: Vec<history::Record>,
    loaded: bool,
    rules_edit: Option<[String; 3]>,
    backup: Option<table::Personal>,
}

impl Ime {
    /// 词表变了之后重算预览、体检与修改记录。每帧调一次，没变是空操作。
    pub(super) fn refresh_manager(&mut self) {
        if self.manager.loaded && !self.manager.dirty {
            return;
        }
        if let Some(staged) = &mut self.manager.staged {
            staged.summary = self.table.preview(&staged.parsed, staged.base);
        }
        self.manager.history = data::directory()
            .map(|dir| history::list(&dir))
            .unwrap_or_default();
        self.manager.loaded = true;
        // `dirty` 由词表页看过之后清掉。
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
            if ui.button("导出最终词表").clicked() {
                self.page.request = Some(PageAction::ExportFinalWordTable);
            }
            if ui.button("导出个人备份").clicked() {
                self.page.request = Some(PageAction::ExportPersonalBackup);
            }
            if ui.button("恢复个人备份").clicked() {
                self.stage_backup();
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
            if parsed.invalid > 0 {
                ui.weak("无效行将跳过；取消后可修正原文件再导入。");
            }
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
                    .add_enabled(
                        !parsed.entries.is_empty(),
                        egui::Button::new(if *base {
                            "确认替换基础表"
                        } else {
                            "导入有效词条"
                        }),
                    )
                    .clicked();
                cancel = ui.button("取消").clicked();
            });
        });
        if confirm {
            let Some(Staged {
                name, base, parsed, ..
            }) = self.manager.staged.clone()
            else {
                return;
            };
            let result = if base {
                self.replace_base(parsed.entries)
            } else {
                self.add_batch(name, parsed.entries)
            };
            if result.is_ok() {
                self.manager.staged = None;
            }
            self.report(
                result,
                if base {
                    "基础词表已更新；可在更多 → 导入与来源恢复上一版。"
                } else {
                    "词表已导入，立即生效；可撤销本次导入。"
                },
            );
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

    /// 导入词表：文件对话框由应用层开，这里只登记请求。
    pub(super) fn stage_import(&mut self, four: bool) {
        self.page.request = Some(PageAction::ImportWordTable { four });
    }

    /// 把最终生效的词表写到用户选定的位置。
    pub(crate) fn export_final_word_table_to(&mut self, path: &Path) {
        self.manager.message = match data::save_bytes(path, self.table.export().as_bytes()) {
            Ok(()) => "最终词表已导出。".into(),
            Err(e) => e.to_string(),
        };
    }

    /// 把个人层备份写到用户选定的位置。
    pub(crate) fn export_personal_backup_to(&mut self, path: &Path) {
        self.manager.message = match data::save_json(path, &self.table.personal) {
            Ok(()) => "公文导入表、个人词条、屏蔽、排序与构词规则已备份。".into(),
            Err(e) => e.to_string(),
        };
    }

    /// 读取用户选中的词表，暂存待确认。
    pub(crate) fn stage_import_from(&mut self, path: &Path, four: bool) {
        match crate::text_file::read_to_string(path) {
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
        self.manager.undo.clear();
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

    /// 先展示备份覆盖范围，确认后才写入；取消文件选择不显示成功。
    pub(super) fn stage_backup(&mut self) {
        self.page.request = Some(PageAction::RestoreWordTableBackup);
    }

    /// 读取用户选中的备份，暂存待确认。
    pub(crate) fn stage_backup_from(&mut self, path: &Path) {
        let result = (|| -> anyhow::Result<table::Personal> {
            let bytes = crate::file_limits::read_bytes(
                path,
                crate::file_limits::MAX_TEXT_BYTES,
                "个人词表备份",
            )?;
            let personal = serde_json::from_slice(&bytes)?;
            super::ops::validate(&personal)?;
            Ok(personal)
        })();
        match result {
            Ok(personal) => self.manager.backup = Some(personal),
            Err(error) => self.manager.message = format!("备份读取失败：{error}"),
        }
    }

    pub(super) fn import_preview(&mut self, ui: &mut egui::Ui) {
        self.staged_ui(ui);
        if let Some(personal) = &self.manager.backup {
            let mut confirm = false;
            let mut cancel = false;
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.strong("恢复个人设置备份");
                ui.label(format!(
                    "将覆盖当前个人设置：个人词条 {} → {} 条，导入表 {} → {} 个。",
                    self.table.personal.entries.len(),
                    personal.entries.len(),
                    self.table.personal.batches.len(),
                    personal.batches.len()
                ));
                ui.weak("屏蔽、候选排序和构词规则也会覆盖；基础表不变。恢复前会保留当前设置。");
                ui.horizontal(|ui| {
                    confirm = ui.button("确认恢复").clicked();
                    cancel = ui.button("取消").clicked();
                });
            });
            if cancel {
                self.manager.backup = None;
            }
            if confirm {
                let result = self.apply_backup();
                self.report(result, "个人设置已恢复。");
            }
        }
    }

    fn apply_backup(&mut self) -> anyhow::Result<()> {
        let Some(personal) = self.manager.backup.clone() else {
            return Ok(());
        };
        if self.storage_ok {
            self.save_personal(personal, "恢复个人备份")?;
        } else {
            let dir = data::directory()?;
            // 文件已损坏时仍保留原始字节，不能用空内存快照覆盖它。
            history::record(&dir, "恢复个人备份")?;
            data::save_json(&dir.join("tables.json"), &personal)?;
            self.table.personal = personal;
            self.storage_ok = true;
            self.manager.undo.clear();
            self.table.rebuild();
            self.rebuild_encoder();
            self.manager.dirty = true;
            self.refresh(true);
        }
        self.load_error = None;
        self.manager.backup = None;
        Ok(())
    }

    pub(super) fn report(&mut self, result: anyhow::Result<()>, success: &str) {
        self.manager.message = match result {
            Ok(()) => success.into(),
            Err(error) => format!("操作失败：{error}"),
        };
    }
}
