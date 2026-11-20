//! 发文字号台账窗口：三个池、手工补录、重号/断号提示。
//!
//! 数据在 `manuscript::ledger`；这里只做窗口与动作，不直接拼 SQL。

use crate::app::GongwenApp;
use crate::app::file_dialog::FileDialogSpec;
use crate::app::jobs::WorkerResult;
use crate::app::output_guard::PII_EXPORT_WARNING;
use crate::export::protection::{self, Protection};
use crate::manuscript::ledger::{DuplicateGroup, GapGroup, IssueEntry, IssueFilter, NewIssue};
use crate::models::VocabularyEntry;
use crate::platform::doc::NumberingScheme;
use crate::theme;
use crate::units::UnitDisplay;
use eframe::egui;

pub(crate) type ManualIssue = NewIssue;

pub(crate) struct IssueLedgerState {
    entries: Vec<IssueEntry>,
    duplicates: Vec<DuplicateGroup>,
    gaps: Vec<GapGroup>,
    filter: IssueFilter,
    adding: Option<ManualIssue>,
    notice: Option<String>,
    needs_load: bool,
}

impl Default for IssueLedgerState {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            duplicates: Vec::new(),
            gaps: Vec::new(),
            filter: IssueFilter::default(),
            adding: None,
            notice: None,
            needs_load: true,
        }
    }
}

enum LedgerAction {
    Refresh,
    Sync,
    Export,
    Add(ManualIssue),
    Delete(i64),
    Close,
}

fn scheme_label(scheme: NumberingScheme) -> &'static str {
    match scheme {
        NumberingScheme::Letter => "公函",
        NumberingScheme::Approval => "红头呈批件",
        NumberingScheme::Minutes => "会议纪要",
    }
}

fn scheme_from_filter(value: Option<&str>) -> Option<NumberingScheme> {
    value.and_then(NumberingScheme::from_str)
}

impl GongwenApp {
    pub(crate) fn open_issue_ledger(&mut self) {
        self.issue_ledger = Some(IssueLedgerState::default());
    }

    pub(crate) fn issue_ledger_window(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.issue_ledger.take() else {
            return;
        };
        if state.needs_load {
            self.reload_issue_ledger(&mut state);
        }
        let mut open = true;
        let mut action = None;
        egui::Window::new("发文登记簿")
            .open(&mut open)
            .default_width(960.0)
            .min_width(720.0)
            .resizable(true)
            .show(ctx, |ui| {
                action = state.ui(ui, &self.config.vocabulary);
            });
        if !open || matches!(action, Some(LedgerAction::Close)) {
            return;
        }
        self.issue_ledger = Some(state);
        if let Some(action) = action {
            self.apply_ledger_action(action);
        }
    }

    fn export_issue_register(
        &mut self,
        entries: Vec<IssueEntry>,
        vocabulary: Vec<VocabularyEntry>,
    ) {
        let save = move |app: &mut Self| {
            let protection = match Protection::current() {
                Ok(protection) => protection,
                Err(error) => {
                    app.status = format!("无法导出发文登记簿：{error:#}");
                    return;
                }
            };
            app.request_file_with_context(
                FileDialogSpec::save_file()
                    .filter("Excel", &["xlsx"])
                    .file_name("公文助手发文登记簿.xlsx"),
                move |app, path, ctx| {
                    if let Some(path) = path {
                        app.start_issue_register_export(
                            ctx,
                            entries.clone(),
                            vocabulary.clone(),
                            path.with_extension("xlsx"),
                            protection,
                        );
                    }
                },
            );
        };
        if protection::encryption_enabled() {
            save(self);
        } else {
            self.request_output_with_warning("导出发文登记簿 Excel", PII_EXPORT_WARNING, save);
        }
    }

    fn start_issue_register_export(
        &mut self,
        ctx: egui::Context,
        entries: Vec<IssueEntry>,
        vocabulary: Vec<VocabularyEntry>,
        path: std::path::PathBuf,
        protection: Option<Protection>,
    ) {
        self.status = "正在生成发文登记簿…".into();
        let tx = self.sender.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let bytes = crate::export::issue_register::xlsx_bytes(&entries, &vocabulary)?;
                let path = protection::write_final_with(protection.as_ref(), &path, &bytes)?;
                Ok::<_, anyhow::Error>(format!("发文登记簿已导出到 {}。", path.display()))
            })()
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send(WorkerResult::IssueRegisterExported(result));
            ctx.request_repaint();
        });
    }

    fn reload_issue_ledger(&mut self, state: &mut IssueLedgerState) {
        state.needs_load = false;
        let Some(store) = self.manuscript_store.as_ref() else {
            state.notice = Some("稿件库不可用，台账暂不能查询。".into());
            return;
        };
        match store.list_issue_entries(&state.filter) {
            Ok(entries) => state.entries = entries,
            Err(error) => state.notice = Some(format!("查询台账失败：{error:#}")),
        }
        state.duplicates = store.duplicate_issue_groups().unwrap_or_default();
        state.gaps = store.issue_gap_groups().unwrap_or_default();
    }

    fn apply_ledger_action(&mut self, action: LedgerAction) {
        let Some(mut state) = self.issue_ledger.take() else {
            return;
        };
        let mut reload = false;
        match action {
            LedgerAction::Refresh => reload = true,
            LedgerAction::Sync => match self.manuscript_store.as_mut() {
                Some(store) => match store.sync_issue_register() {
                    Ok(count) => {
                        state.notice = Some(format!("已从稿件库同步 {count} 篇有文号的稿件。"));
                        reload = true;
                    }
                    Err(error) => state.notice = Some(format!("同步稿件库失败：{error:#}")),
                },
                None => state.notice = Some("稿件库不可用。".into()),
            },
            LedgerAction::Export => {
                let entries = state.entries.clone();
                let vocabulary = self.config.vocabulary.clone();
                self.export_issue_register(entries, vocabulary);
            }
            LedgerAction::Add(form) => match self.manuscript_store.as_mut() {
                Some(store) => match store.add_manual_issue(&form) {
                    Ok(_) => {
                        state.adding = None;
                        state.notice = Some("已补录一条台账。".into());
                        reload = true;
                    }
                    Err(error) => state.notice = Some(format!("补录失败：{error:#}")),
                },
                None => state.notice = Some("稿件库不可用。".into()),
            },
            LedgerAction::Delete(id) => match self.manuscript_store.as_mut() {
                Some(store) => match store.delete_issue(id) {
                    Ok(()) => {
                        state.notice = Some("已删除手工补录条目。".into());
                        reload = true;
                    }
                    Err(error) => state.notice = Some(format!("删除失败：{error:#}")),
                },
                None => state.notice = Some("稿件库不可用。".into()),
            },
            LedgerAction::Close => {}
        }
        if reload {
            self.reload_issue_ledger(&mut state);
        }
        self.issue_ledger = Some(state);
    }
}

impl IssueLedgerState {
    fn ui(&mut self, ui: &mut egui::Ui, vocabulary: &[VocabularyEntry]) -> Option<LedgerAction> {
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            ui.label("年度");
            ui.add(
                egui::TextEdit::singleline(&mut self.filter.year)
                    .hint_text("全部")
                    .desired_width(72.0),
            );
            ui.label("类型");
            egui::ComboBox::from_id_salt("issue_ledger_scheme")
                .selected_text(
                    self.filter
                        .scheme
                        .as_deref()
                        .and_then(NumberingScheme::from_str)
                        .map_or("全部", scheme_label),
                )
                .width(110.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(self.filter.scheme.is_none(), "全部")
                        .clicked()
                    {
                        self.filter.scheme = None;
                    }
                    for scheme in [
                        NumberingScheme::Letter,
                        NumberingScheme::Approval,
                        NumberingScheme::Minutes,
                    ] {
                        if ui
                            .selectable_label(
                                self.filter.scheme.as_deref() == Some(scheme.as_str()),
                                scheme_label(scheme),
                            )
                            .clicked()
                        {
                            self.filter.scheme = Some(scheme.as_str().to_string());
                        }
                    }
                });
            ui.label("单位");
            ui.add(
                egui::TextEdit::singleline(&mut self.filter.unit)
                    .hint_text("单位编码或名称")
                    .desired_width(150.0),
            );
            if ui.button("查询").clicked() {
                action = Some(LedgerAction::Refresh);
            }
            if ui.button("导出 Excel").clicked() {
                action = Some(LedgerAction::Export);
            }
            if ui
                .button("同步稿件库")
                .on_hover_text("把稿件库里所有有文号的公函/呈批件/纪要同步进来；手工补录的行不动")
                .clicked()
            {
                action = Some(LedgerAction::Sync);
            }
            if self.adding.is_none() && ui.button("补录纸质件").clicked() {
                self.adding = Some(ManualIssue {
                    scheme: NumberingScheme::Letter,
                    unit: String::new(),
                    code: String::new(),
                    year: String::new(),
                    serial: String::new(),
                    title: String::new(),
                    doc_date: String::new(),
                    security: "内部".into(),
                    note: String::new(),
                });
            }
            if ui.button("关闭").clicked() {
                action = Some(LedgerAction::Close);
            }
        });

        if let Some(notice) = &self.notice {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(notice).color(theme::text_muted()));
        }

        let mut cancel_manual = false;
        if let Some(form) = &mut self.adding {
            ui.add_space(6.0);
            ui.group(|ui| {
                egui::Grid::new("issue_ledger_manual")
                    .num_columns(4)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("类型");
                        egui::ComboBox::from_id_salt("issue_manual_scheme")
                            .selected_text(scheme_label(form.scheme))
                            .width(120.0)
                            .show_ui(ui, |ui| {
                                for scheme in [
                                    NumberingScheme::Letter,
                                    NumberingScheme::Approval,
                                    NumberingScheme::Minutes,
                                ] {
                                    ui.selectable_value(
                                        &mut form.scheme,
                                        scheme,
                                        scheme_label(scheme),
                                    );
                                }
                            });
                        ui.label("单位");
                        ui.add(
                            egui::TextEdit::singleline(&mut form.unit)
                                .hint_text("如：某局")
                                .desired_width(180.0),
                        );
                        ui.end_row();

                        ui.label("代字");
                        ui.add(
                            egui::TextEdit::singleline(&mut form.code)
                                .hint_text("纪要可留空")
                                .desired_width(120.0),
                        );
                        ui.label("年度");
                        ui.add(
                            egui::TextEdit::singleline(&mut form.year)
                                .hint_text("2026")
                                .desired_width(72.0),
                        );
                        ui.end_row();

                        ui.label("序号");
                        ui.add(
                            egui::TextEdit::singleline(&mut form.serial)
                                .hint_text("12")
                                .desired_width(72.0),
                        );
                        ui.label("日期");
                        ui.add(
                            egui::TextEdit::singleline(&mut form.doc_date)
                                .hint_text("YYYY-MM-DD")
                                .desired_width(120.0),
                        );
                        ui.end_row();

                        ui.label("标题");
                        ui.add(egui::TextEdit::singleline(&mut form.title).desired_width(360.0));
                        ui.label("密级");
                        ui.add(egui::TextEdit::singleline(&mut form.security).desired_width(90.0));
                        ui.end_row();

                        ui.label("备注");
                        ui.add(egui::TextEdit::singleline(&mut form.note).desired_width(360.0));
                        ui.horizontal(|ui| {
                            if ui.button("保存补录").clicked() {
                                action = Some(LedgerAction::Add(form.clone()));
                            }
                            if ui.button("取消").clicked() {
                                cancel_manual = true;
                            }
                        });
                        ui.end_row();
                    });
            });
        }

        if cancel_manual {
            self.adding = None;
        }
        ui.add_space(8.0);
        if !self.duplicates.is_empty() {
            let first = &self.duplicates[0];
            ui.colored_label(
                theme::warn(),
                format!(
                    "重号 {} 组：{} {} {} 年第 {} 号（{} 条）",
                    self.duplicates.len(),
                    display_unit(vocabulary, &first.unit),
                    scheme_from_filter(Some(&first.scheme)).map_or("", scheme_label),
                    first.year,
                    first.serial,
                    first.count
                ),
            );
        }
        if !self.gaps.is_empty() {
            let first = &self.gaps[0];
            let missing = first
                .missing
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("、");
            ui.colored_label(
                theme::text_muted(),
                format!(
                    "断号 {} 组：{} {} {} 年缺 {}",
                    self.gaps.len(),
                    display_unit(vocabulary, &first.unit),
                    scheme_from_filter(Some(&first.scheme)).map_or("", scheme_label),
                    first.year,
                    missing
                ),
            );
        }

        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .max_height(360.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("issue_ledger_rows")
                    .striped(true)
                    .num_columns(8)
                    .spacing([10.0, 4.0])
                    .show(ui, |ui| {
                        for header in [
                            "年度", "编号", "单位", "标题", "日期", "密级", "状态", "来源",
                        ] {
                            ui.strong(header);
                        }
                        ui.end_row();
                        if self.entries.is_empty() {
                            ui.label("（没有登记）");
                            ui.end_row();
                        }
                        for entry in &self.entries {
                            ui.label(&entry.year);
                            ui.label(entry.number_label());
                            ui.label(display_unit(vocabulary, &entry.unit));
                            ui.label(&entry.title);
                            ui.label(&entry.doc_date);
                            ui.label(&entry.security);
                            ui.label(entry.status_label());
                            if entry.manuscript_id.is_some() {
                                ui.label("稿件");
                            } else {
                                ui.horizontal(|ui| {
                                    ui.label("手工");
                                    if ui.small_button("删除").clicked() {
                                        action = Some(LedgerAction::Delete(entry.id));
                                    }
                                });
                            }
                            ui.end_row();
                        }
                    });
            });

        action
    }
}

fn display_unit(vocabulary: &[VocabularyEntry], unit: &str) -> String {
    if unit.trim().is_empty() {
        return "未登记单位".into();
    }
    UnitDisplay::new(vocabulary).full_name(unit)
}
