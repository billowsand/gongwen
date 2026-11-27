//! 发文登记簿页面：池栏、表格、详情、补录与取号。
//!
//! 数据在 `manuscript::ledger`；这里只做页面与动作，不直接拼 SQL。发文登记簿是
//! 管理面而不是临时对话框：它有自己的标签页、筛选、详情与编辑，起草页文号行的
//! 「取号卡」只借用同一个池查询。

use crate::app::GongwenApp;
use crate::app::NavPage;
use crate::app::file_dialog::FileDialogSpec;
use crate::app::jobs::WorkerResult;
use crate::app::output_guard::PII_EXPORT_WARNING;
use crate::app::widgets::{SelectOption, single_select};
use crate::export::protection::{self, Protection};
use crate::manuscript::ManuscriptStore;
use crate::manuscript::ledger::{
    self, DuplicateGroup, GapGroup, IssueEntry, IssueFilter, NewIssue,
};
use crate::modal::{self, Dismiss};
use crate::models::{SecurityLevel, VocabularyCategory, VocabularyEntry};
use crate::platform::doc::NumberingScheme;
use crate::theme;
use crate::units::{UnitDisplay, identity};
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub(crate) type ManualIssue = NewIssue;

/// 台账的池键：单位 + 编号方案 + 年度。单位在比较前统一归一成词条身份，
/// 避免同一单位的 `@unit:` 引用与手填名称被拆成两个池。
#[derive(Debug, Clone, PartialEq, Eq)]
struct PoolKey {
    unit: String,
    scheme: String,
    year: String,
}

#[derive(Debug, Clone)]
struct PoolSummary {
    key: PoolKey,
    /// 池栏第一行：单位 · 类型；年度放在第二行，窄栏里才不会被挤断。
    rail_label: String,
    /// 完整标签，供窄屏下拉与“当前池”使用。
    label: String,
    count: usize,
    duplicates: usize,
    missing: Vec<u32>,
}

pub(crate) struct IssueLedgerState {
    entries: Vec<IssueEntry>,
    /// 原始单位写法 → 归一后的池键。每次重算台账时一并刷新，避免每帧逐行解析词库。
    unit_keys: HashMap<String, String>,
    duplicates: Vec<DuplicateGroup>,
    gaps: Vec<GapGroup>,
    pools: Vec<PoolSummary>,
    selected_pool: Option<PoolKey>,
    keyword: String,
    selected: Option<i64>,
    /// 外部要打开的条目（例如从稿件跳到它的台账行）：重新载入后落到 `selected`。
    pending_selection: Option<i64>,
    /// 外部要按稿件定位的台账行：重新载入后按 manuscript_id 找到并选中。
    pending_manuscript: Option<i64>,
    detail_modal: Option<i64>,
    editing: Option<ManualEdit>,
    confirm_delete: Option<IssueEntry>,
    problems_open: bool,
    /// 条目 id → 尚未保存的备注草稿。切换条目不丢，保存成功才清。
    note_edits: HashMap<i64, String>,
    pub(crate) notice: Option<String>,
    pub(crate) needs_load: bool,
    pub(crate) syncing: bool,
}

impl Default for IssueLedgerState {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            unit_keys: HashMap::new(),
            duplicates: Vec::new(),
            gaps: Vec::new(),
            pools: Vec::new(),
            selected_pool: None,
            keyword: String::new(),
            selected: None,
            pending_selection: None,
            pending_manuscript: None,
            detail_modal: None,
            editing: None,
            confirm_delete: None,
            problems_open: false,
            note_edits: HashMap::new(),
            notice: None,
            needs_load: true,
            syncing: false,
        }
    }
}

enum LedgerAction {
    Refresh,
    Sync,
    Export,
    SaveManual {
        id: Option<i64>,
        form: Box<ManualIssue>,
    },
    Delete(i64),
    SetStatus {
        id: i64,
        status: &'static str,
    },
    SetNote {
        id: i64,
        note: String,
    },
    OpenManuscript(i64),
}

struct ManualEdit {
    id: Option<i64>,
    form: ManualIssue,
    unit_manual: BTreeSet<String>,
    error: Option<String>,
}

impl ManualEdit {
    fn new(
        vocabulary: &[VocabularyEntry],
        unit: String,
        scheme: NumberingScheme,
        year: String,
        code: String,
    ) -> Self {
        let mut unit_manual = BTreeSet::new();
        if !unit.trim().is_empty()
            && identity::resolve(vocabulary, VocabularyCategory::Unit, &unit).is_none()
        {
            unit_manual.insert("issue_manual_unit".to_owned());
        }
        Self {
            id: None,
            form: ManualIssue {
                unit,
                scheme,
                code,
                year,
                status: "used".into(),
                ..Default::default()
            },
            unit_manual,
            error: None,
        }
    }

    fn from_entry(entry: &IssueEntry, vocabulary: &[VocabularyEntry]) -> Self {
        let resolved = identity::resolve(vocabulary, VocabularyCategory::Unit, &entry.unit);
        let unit = resolved
            .as_ref()
            .map(|entry| entry.key())
            .unwrap_or_else(|| entry.unit.clone());
        let mut unit_manual = BTreeSet::new();
        if resolved.is_none() && !entry.unit.trim().is_empty() {
            unit_manual.insert("issue_manual_unit".to_owned());
        }
        Self {
            id: Some(entry.id),
            form: ManualIssue {
                unit,
                scheme: NumberingScheme::from_str(&entry.scheme).unwrap_or(NumberingScheme::Letter),
                code: entry.code.clone(),
                year: entry.year.clone(),
                serial: entry.serial.clone(),
                title: entry.title.clone(),
                doc_date: entry.doc_date.clone(),
                security: entry.security.clone(),
                status: entry.status.clone(),
                note: entry.note.clone(),
            },
            unit_manual,
            error: None,
        }
    }

    fn fields_ui(
        &mut self,
        ui: &mut egui::Ui,
        vocabulary: &[VocabularyEntry],
        entries: &[IssueEntry],
        allow_free_text: bool,
    ) {
        let display = UnitDisplay::new(vocabulary);
        let options: Vec<SelectOption> = vocabulary
            .iter()
            .filter(|entry| entry.category == VocabularyCategory::Unit)
            .map(|entry| SelectOption {
                value: entry.key(),
                label: display.name_for(&entry.key(), false),
                full: display.full_name(&entry.key()),
                search_terms: format!("{} {}", entry.code, entry.canonical),
                parent: String::new(),
                depth: 0,
            })
            .collect();

        egui::Grid::new("issue_manual_grid")
            .num_columns(2)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                ui.label("类型");
                egui::ComboBox::from_id_salt("issue_manual_scheme")
                    .selected_text(scheme_label(self.form.scheme))
                    .width(180.0)
                    .show_ui(ui, |ui| {
                        for scheme in [
                            NumberingScheme::Letter,
                            NumberingScheme::Approval,
                            NumberingScheme::Minutes,
                        ] {
                            ui.selectable_value(
                                &mut self.form.scheme,
                                scheme,
                                scheme_label(scheme),
                            );
                        }
                    });
                ui.end_row();

                ui.label("状态");
                egui::ComboBox::from_id_salt("issue_manual_status")
                    .selected_text(status_label(&self.form.status))
                    .width(180.0)
                    .show_ui(ui, |ui| {
                        for (value, label) in
                            [("used", "已用"), ("reserved", "占号"), ("void", "废号")]
                        {
                            ui.selectable_value(&mut self.form.status, value.to_string(), label);
                        }
                    });
                ui.end_row();

                ui.label("单位");
                let width = 360.0;
                single_select(
                    ui,
                    "issue_manual_unit",
                    &mut self.form.unit,
                    &options,
                    vocabulary,
                    &mut self.unit_manual,
                    allow_free_text,
                    width,
                    "选择或手填登记单位",
                );
                ui.end_row();

                ui.label("代字");
                ui.add(
                    egui::TextEdit::singleline(&mut self.form.code)
                        .hint_text("纪要可留空")
                        .desired_width(180.0),
                );
                ui.end_row();

                ui.label("年度");
                ui.add(
                    egui::TextEdit::singleline(&mut self.form.year)
                        .hint_text("2026")
                        .char_limit(4)
                        .desired_width(100.0),
                );
                ui.end_row();

                ui.label("序号");
                ui.add(
                    egui::TextEdit::singleline(&mut self.form.serial)
                        .hint_text("12")
                        .desired_width(100.0),
                );
                ui.end_row();

                ui.label("日期");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.form.doc_date)
                            .hint_text("YYYY-MM-DD")
                            .desired_width(150.0),
                    );
                    if ui.small_button("今天").clicked() {
                        self.form.doc_date = chrono::Local::now().format("%Y-%m-%d").to_string();
                    }
                });
                ui.end_row();

                ui.label("标题");
                ui.add(egui::TextEdit::singleline(&mut self.form.title).desired_width(360.0));
                ui.end_row();

                ui.label("密级");
                egui::ComboBox::from_id_salt("issue_manual_security")
                    .selected_text(if self.form.security.trim().is_empty() {
                        SecurityLevel::Unmarked.label()
                    } else {
                        self.form.security.trim()
                    })
                    .width(180.0)
                    .show_ui(ui, |ui| {
                        for level in SecurityLevel::ALL {
                            let label = level.label();
                            if ui
                                .selectable_label(
                                    self.form.security.trim() == level.marking(),
                                    label,
                                )
                                .clicked()
                            {
                                self.form.security = level.marking().to_string();
                            }
                        }
                    });
                ui.end_row();

                ui.label("备注");
                ui.add(
                    egui::TextEdit::multiline(&mut self.form.note)
                        .desired_rows(2)
                        .desired_width(360.0),
                );
                ui.end_row();
            });

        if let Some(conflict) = conflict_for(entries, &self.form, self.id, vocabulary) {
            ui.add_space(4.0);
            ui.colored_label(
                theme::warn(),
                format!(
                    "该号已登记给《{}》；仍可保存（纸质台账可能还没录全）。",
                    conflict
                ),
            );
        }
    }

    fn validate(&mut self, vocabulary: &[VocabularyEntry]) -> Result<(), String> {
        self.form.unit = unit_pool_key(vocabulary, &self.form.unit);
        if self.form.unit.trim().is_empty() {
            return Err("请填写登记单位".into());
        }
        let year = self.form.year.trim();
        if year.len() != 4 || !year.chars().all(|ch| ch.is_ascii_digit()) {
            return Err("年度应为四位数字，例如 2026".into());
        }
        let serial = self.form.serial.trim();
        if serial.is_empty() || !serial.chars().all(|ch| ch.is_ascii_digit()) {
            return Err("序号只能填写数字".into());
        }
        if !self.form.doc_date.trim().is_empty()
            && chrono::NaiveDate::parse_from_str(self.form.doc_date.trim(), "%Y-%m-%d").is_err()
        {
            return Err("日期应为真实的 YYYY-MM-DD".into());
        }
        if !ledger::valid_status(&self.form.status) {
            return Err("未知的登记状态".into());
        }
        Ok(())
    }
}

impl GongwenApp {
    /// 从稿件管理页或菜单打开发文登记簿。
    pub(crate) fn open_issue_ledger(&mut self) {
        self.issue_ledger.needs_load = true;
        self.open_page(NavPage::IssueLedger);
    }

    /// 从起草页文号行打开：顺手把池筛到当前稿件，省得再手选单位/年度/类型。
    pub(crate) fn open_issue_ledger_for_active_draft(&mut self) {
        let scope = self.active_doc_ref().map(|doc| {
            (
                ledger::effective_issue_unit(&doc.draft).to_string(),
                NumberingScheme::for_kind(doc.draft.kind),
                doc.draft.document_year(),
            )
        });
        if let Some((unit, scheme, year)) = scope {
            let unit = unit_pool_key(&self.config.vocabulary, &unit);
            if let Some(scheme) = scheme {
                self.issue_ledger.focus_pool(unit, scheme, year);
            } else {
                self.issue_ledger.selected_pool = None;
            }
        }
        if let Some(manuscript_id) = self.active_doc_ref().and_then(|doc| doc.manuscript_id) {
            self.issue_ledger.focus_manuscript(manuscript_id);
        }
        self.open_page(NavPage::IssueLedger);
    }

    pub(crate) fn issue_ledger_page(&mut self, ui: &mut egui::Ui) {
        if self.manuscript_store.is_none() {
            ui.colored_label(theme::warn(), "稿件库不可用，发文登记簿暂不能打开。");
            return;
        }
        let mut state = std::mem::take(&mut self.issue_ledger);
        if state.needs_load {
            self.reload_issue_ledger(&mut state);
        }
        let action = state.ui(ui, &self.config.vocabulary, self.config.allow_free_text);
        self.issue_ledger = state;
        if let Some(action) = action {
            self.apply_ledger_action(action, ui.ctx());
        }
    }

    fn reload_issue_ledger(&mut self, state: &mut IssueLedgerState) {
        state.needs_load = false;
        let Some(store) = self.manuscript_store.as_ref() else {
            state.notice = Some("稿件库不可用，台账暂不能查询。".into());
            return;
        };
        match store.list_issue_entries(&IssueFilter::default()) {
            Ok(entries) => state.entries = entries,
            Err(error) => state.notice = Some(format!("查询台账失败：{error:#}")),
        }
        state.recompute(&self.config.vocabulary);
        state.retain_selection();
        if let Some(id) = state.pending_selection.take()
            && state.entries.iter().any(|entry| entry.id == id)
        {
            state.selected = Some(id);
        }
        if let Some(manuscript_id) = state.pending_manuscript.take()
            && let Some(entry) = state
                .entries
                .iter()
                .find(|entry| entry.manuscript_id == Some(manuscript_id))
        {
            state.selected = Some(entry.id);
        }
    }

    fn apply_ledger_action(&mut self, action: LedgerAction, ctx: &egui::Context) {
        match action {
            LedgerAction::Refresh => self.issue_ledger.needs_load = true,
            LedgerAction::Sync => self.start_issue_ledger_sync(ctx),
            LedgerAction::Export => {
                let mut entries = self.issue_ledger.visible_entries();
                // 台账导出按“年度 → 类型 → 单位 → 序号”的正式顺序，不跟界面上的
                // 倒序排列走；同一池内就是序号升序，存档和打印都顺眼。
                entries.sort_by(|left, right| {
                    left.year
                        .cmp(&right.year)
                        .then_with(|| scheme_order(&left.scheme).cmp(&scheme_order(&right.scheme)))
                        .then_with(|| left.unit.cmp(&right.unit))
                        .then_with(|| {
                            left.serial
                                .parse::<u32>()
                                .unwrap_or(0)
                                .cmp(&right.serial.parse::<u32>().unwrap_or(0))
                        })
                });
                if entries.is_empty() {
                    self.issue_ledger.notice = Some("当前没有可导出的登记。".into());
                } else {
                    let vocabulary = self.config.vocabulary.clone();
                    self.export_issue_register(entries, vocabulary);
                }
            }
            LedgerAction::SaveManual { id, form } => {
                let mut form = *form;
                let resolved = self.resolve_issue_unit(&form.unit);
                form.unit = resolved;
                let result = match self.manuscript_store.as_mut() {
                    Some(store) => match id {
                        Some(id) => store.update_manual_issue(id, &form),
                        None => store.add_manual_issue(&form).map(|_| ()),
                    },
                    None => Err(anyhow::anyhow!("稿件库不可用")),
                };
                match result {
                    Ok(()) => {
                        self.issue_ledger.editing = None;
                        self.issue_ledger.notice = Some(if id.is_some() {
                            "已保存补录修改。".into()
                        } else {
                            "已补录一条台账。".into()
                        });
                        self.issue_ledger.needs_load = true;
                    }
                    Err(error) => {
                        let message = format!("保存失败：{error:#}");
                        self.issue_ledger.notice = Some(message.clone());
                        if let Some(edit) = self.issue_ledger.editing.as_mut() {
                            edit.error = Some(message);
                        }
                    }
                }
            }
            LedgerAction::Delete(id) => {
                let result = self
                    .manuscript_store
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))
                    .and_then(|store| store.delete_issue(id));
                match result {
                    Ok(()) => {
                        self.issue_ledger.notice = Some("已删除补录条目。".into());
                        self.issue_ledger.selected = None;
                        self.issue_ledger.detail_modal = None;
                        self.issue_ledger.needs_load = true;
                    }
                    Err(error) => self.issue_ledger.notice = Some(format!("删除失败：{error:#}")),
                }
            }
            LedgerAction::SetStatus { id, status } => {
                let result = self
                    .manuscript_store
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))
                    .and_then(|store| store.set_issue_status(id, status));
                match result {
                    Ok(()) => {
                        self.issue_ledger.notice =
                            Some(format!("已标为{}。", status_label(status)));
                        self.issue_ledger.needs_load = true;
                    }
                    Err(error) => self.issue_ledger.notice = Some(format!("改状态失败：{error:#}")),
                }
            }
            LedgerAction::SetNote { id, note } => {
                let result = self
                    .manuscript_store
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))
                    .and_then(|store| store.set_issue_note(id, &note));
                match result {
                    Ok(()) => {
                        self.issue_ledger.notice = Some("备注已保存。".into());
                        self.issue_ledger.needs_load = true;
                        self.issue_ledger.note_edits.remove(&id);
                    }
                    Err(error) => {
                        self.issue_ledger.notice = Some(format!("保存备注失败：{error:#}"))
                    }
                }
            }
            LedgerAction::OpenManuscript(id) => self.open_in_editor(id),
        }
    }

    fn start_issue_ledger_sync(&mut self, ctx: &egui::Context) {
        if self.issue_ledger.syncing {
            return;
        }
        let path = match crate::storage::manuscript_db_path() {
            Ok(path) => path,
            Err(error) => {
                self.issue_ledger.notice = Some(format!("找不到稿件库：{error:#}"));
                return;
            }
        };
        self.issue_ledger.syncing = true;
        self.status = "正在从稿件库同步发文登记簿…".into();
        let tx = self.sender.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<usize> {
                let mut store = ManuscriptStore::open(&path)?;
                store.sync_issue_register()
            })()
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send(WorkerResult::IssueLedgerSynced(result));
            ctx.request_repaint();
        });
    }

    fn resolve_issue_unit(&self, raw: &str) -> String {
        unit_pool_key(&self.config.vocabulary, raw)
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
}

impl IssueLedgerState {
    /// 打开补录表单。页面头部与截图共用同一条入口。
    #[cfg(all(test, feature = "ai-workbench"))]
    pub(crate) fn open_compose(&mut self, vocabulary: &[VocabularyEntry]) {
        self.problems_open = false;
        self.confirm_delete = None;
        self.start_add(vocabulary);
    }

    /// 打开重号/断号清单。
    #[cfg(all(test, feature = "ai-workbench"))]
    pub(crate) fn open_problems(&mut self) {
        self.editing = None;
        self.confirm_delete = None;
        self.problems_open = true;
    }

    /// 打开台账后直接选中某条；条目还不存在时等下一次重载再落。
    #[cfg(all(test, feature = "ai-workbench"))]
    pub(crate) fn select_entry(&mut self, id: i64) {
        self.pending_selection = Some(id);
        self.needs_load = true;
    }

    /// 从某篇稿件跳过来时，重新载入后选中它对应的台账行。
    pub(crate) fn focus_manuscript(&mut self, manuscript_id: i64) {
        self.pending_manuscript = Some(manuscript_id);
        self.needs_load = true;
    }

    fn focus_pool(&mut self, unit: String, scheme: NumberingScheme, year: String) {
        self.selected_pool = Some(PoolKey {
            unit,
            scheme: scheme.as_str().to_string(),
            year,
        });
        self.keyword.clear();
        self.selected = None;
        self.needs_load = true;
    }

    fn retain_selection(&mut self) {
        if let Some(id) = self.selected
            && !self.entries.iter().any(|entry| entry.id == id)
        {
            self.selected = None;
        }
        if let Some(id) = self.detail_modal
            && !self.entries.iter().any(|entry| entry.id == id)
        {
            self.detail_modal = None;
        }
        self.note_edits
            .retain(|id, _| self.entries.iter().any(|entry| entry.id == *id));
        if let Some(entry) = &self.confirm_delete
            && !self.entries.iter().any(|item| item.id == entry.id)
        {
            self.confirm_delete = None;
        }
    }

    fn selected_pool_label(&self, vocabulary: &[VocabularyEntry]) -> Option<String> {
        let pool = self.selected_pool.as_ref()?;
        Some(
            self.pools
                .iter()
                .find(|item| item.key == *pool)
                .map(|item| item.label.clone())
                .unwrap_or_else(|| {
                    let scheme = NumberingScheme::from_str(&pool.scheme)
                        .map(scheme_short)
                        .unwrap_or("未知");
                    format!(
                        "{} · {} · {}",
                        UnitDisplay::new(vocabulary).full_name(&pool.unit),
                        scheme,
                        pool.year
                    )
                }),
        )
    }

    fn pool_unit_of(&self, unit: &str) -> String {
        self.unit_keys
            .get(unit)
            .cloned()
            .unwrap_or_else(|| unit.trim().to_string())
    }

    /// 重算单位归一缓存、重号/断号与池清单。台账页不直接用 SQL 里的 raw 单位分组：
    /// 同一单位在自动行里是 `@unit:` 引用、在旧手工行里可能是名称，归一后才是同一个池。
    fn recompute(&mut self, vocabulary: &[VocabularyEntry]) {
        self.unit_keys.clear();
        for entry in &self.entries {
            self.unit_keys
                .entry(entry.unit.clone())
                .or_insert_with(|| unit_pool_key(vocabulary, &entry.unit));
        }
        self.rebuild_problems();
        self.rebuild_pools(vocabulary);
    }

    fn rebuild_problems(&mut self) {
        let mut grouped: HashMap<(String, String, String), Vec<usize>> = HashMap::new();
        for (index, entry) in self.entries.iter().enumerate() {
            grouped
                .entry((
                    self.pool_unit_of(&entry.unit),
                    entry.scheme.clone(),
                    entry.year.clone(),
                ))
                .or_default()
                .push(index);
        }
        let mut duplicates = Vec::new();
        let mut gaps = Vec::new();
        for ((unit, scheme, year), indices) in grouped {
            let mut counts: BTreeMap<u32, (usize, Vec<String>)> = BTreeMap::new();
            let mut present = BTreeSet::new();
            let mut max = 0;
            for index in indices {
                let entry = &self.entries[index];
                let Ok(value) = entry.serial.trim().parse::<u32>() else {
                    continue;
                };
                // 废号仍然占着这个序号：它让断号清单闭嘴，但不参与重号统计。
                present.insert(value);
                max = max.max(value);
                if entry.status != "void" {
                    let slot = counts.entry(value).or_default();
                    slot.0 += 1;
                    slot.1.push(entry.title.clone());
                }
            }
            for (serial, (count, titles)) in counts {
                if count > 1 {
                    duplicates.push(DuplicateGroup {
                        unit: unit.clone(),
                        scheme: scheme.clone(),
                        year: year.clone(),
                        serial: serial.to_string(),
                        count,
                        titles: titles.join(" | "),
                    });
                }
            }
            let missing: Vec<u32> = (1..=max).filter(|value| !present.contains(value)).collect();
            if !missing.is_empty() {
                gaps.push(GapGroup {
                    unit: unit.clone(),
                    scheme: scheme.clone(),
                    year: year.clone(),
                    missing,
                });
            }
        }
        duplicates.sort_by(|left, right| {
            right
                .year
                .cmp(&left.year)
                .then_with(|| scheme_order(&left.scheme).cmp(&scheme_order(&right.scheme)))
                .then_with(|| left.unit.cmp(&right.unit))
                .then_with(|| {
                    left.serial
                        .parse::<u32>()
                        .unwrap_or(0)
                        .cmp(&right.serial.parse::<u32>().unwrap_or(0))
                })
        });
        gaps.sort_by(|left, right| {
            right
                .year
                .cmp(&left.year)
                .then_with(|| scheme_order(&left.scheme).cmp(&scheme_order(&right.scheme)))
                .then_with(|| left.unit.cmp(&right.unit))
        });
        self.duplicates = duplicates;
        self.gaps = gaps;
    }

    fn rebuild_pools(&mut self, vocabulary: &[VocabularyEntry]) {
        let display = UnitDisplay::new(vocabulary);
        let mut map: HashMap<(String, String, String), PoolSummary> = HashMap::new();
        for entry in &self.entries {
            let key = PoolKey {
                unit: self.pool_unit_of(&entry.unit),
                scheme: entry.scheme.clone(),
                year: entry.year.clone(),
            };
            let summary = map
                .entry((key.unit.clone(), key.scheme.clone(), key.year.clone()))
                .or_insert_with(|| PoolSummary {
                    key,
                    rail_label: String::new(),
                    label: String::new(),
                    count: 0,
                    duplicates: 0,
                    missing: Vec::new(),
                });
            summary.count += 1;
        }
        for group in &self.duplicates {
            if let Some(summary) =
                map.get_mut(&(group.unit.clone(), group.scheme.clone(), group.year.clone()))
            {
                summary.duplicates += 1;
            }
        }
        for group in &self.gaps {
            if let Some(summary) =
                map.get_mut(&(group.unit.clone(), group.scheme.clone(), group.year.clone()))
            {
                summary.missing = group.missing.clone();
            }
        }
        let mut pools: Vec<PoolSummary> = map.into_values().collect();
        for pool in &mut pools {
            let scheme = NumberingScheme::from_str(&pool.key.scheme)
                .map(scheme_short)
                .unwrap_or("未知");
            pool.rail_label = format!("{} · {}", display.full_name(&pool.key.unit), scheme);
            pool.label = format!("{} · {}", pool.rail_label, pool.key.year);
        }
        pools.sort_by(|left, right| {
            right
                .key
                .year
                .cmp(&left.key.year)
                .then_with(|| scheme_order(&left.key.scheme).cmp(&scheme_order(&right.key.scheme)))
                .then_with(|| left.label.cmp(&right.label))
        });
        self.pools = pools;
    }

    fn visible_entries(&self) -> Vec<IssueEntry> {
        self.visible_indices()
            .into_iter()
            .map(|index| self.entries[index].clone())
            .collect()
    }

    fn visible_indices(&self) -> Vec<usize> {
        let keyword = self.keyword.trim();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                let in_pool = self.selected_pool.as_ref().is_none_or(|pool| {
                    pool.scheme == entry.scheme
                        && pool.year == entry.year
                        && pool.unit == self.pool_unit_of(&entry.unit)
                });
                if !in_pool {
                    return false;
                }
                if keyword.is_empty() {
                    return true;
                }
                entry.title.contains(keyword)
                    || entry.note.contains(keyword)
                    || entry.serial == keyword
                    || entry.number_label().contains(keyword)
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        vocabulary: &[VocabularyEntry],
        allow_free_text: bool,
    ) -> Option<LedgerAction> {
        let show_rail = ui.available_width() >= 720.0;
        let wide = ui.available_width() >= 1080.0;
        let mut action = None;

        self.header_ui(ui, vocabulary, show_rail, &mut action);
        if let Some(notice) = &self.notice {
            ui.colored_label(theme::text_muted(), notice);
        }
        ui.add_space(6.0);

        if wide
            && self.editing.is_none()
            && self.confirm_delete.is_none()
            && !self.problems_open
            && let Some(id) = self.selected
        {
            egui::Panel::right("issue_ledger_detail")
                .resizable(false)
                .default_size(318.0)
                .size_range(318.0..=318.0)
                .frame(theme::panel(theme::canvas(), 10))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("issue_ledger_detail_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if let Some(next) = self.detail_ui(ui, vocabulary, id, true) {
                                action = Some(next);
                            }
                        });
                });
        }
        if show_rail {
            egui::Panel::left("issue_ledger_pools")
                .resizable(false)
                .default_size(240.0)
                .size_range(240.0..=240.0)
                .frame(theme::panel(theme::canvas(), 10))
                .show(ui, |ui| {
                    self.pools_ui(ui);
                });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                self.search_ui(ui, vocabulary);
                ui.add_space(4.0);
                if let Some(next) = self.table_ui(ui, vocabulary, wide) {
                    action = Some(next);
                }
            });

        if let Some(next) = self.manual_modal(ui.ctx(), vocabulary, allow_free_text) {
            action = Some(next);
        }
        if let Some(next) = self.delete_modal(ui.ctx()) {
            action = Some(next);
        }
        self.problems_modal(ui.ctx(), vocabulary);
        if !wide
            && !self.problems_open
            && self.editing.is_none()
            && self.confirm_delete.is_none()
            && let Some(next) = self.detail_modal_ui(ui.ctx(), vocabulary)
        {
            action = Some(next);
        }
        action
    }

    fn header_ui(
        &mut self,
        ui: &mut egui::Ui,
        vocabulary: &[VocabularyEntry],
        show_rail: bool,
        action: &mut Option<LedgerAction>,
    ) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading("发文登记簿");
                ui.weak(format!(
                    "共 {} 条 · {} 个池 · 重号 {} 组 · 断号 {} 组",
                    self.entries.len(),
                    self.pools.len(),
                    self.duplicates.len(),
                    self.gaps.len()
                ));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(theme::icon_text_button(theme::Icon::Plus, "补录纸质件"))
                    .on_hover_text("登记没有对应稿件的纸质件，也可以补登废号")
                    .clicked()
                {
                    self.start_add(vocabulary);
                }
                let sync_label = if self.syncing {
                    "同步中…"
                } else {
                    "同步稿件库"
                };
                if ui
                    .add_enabled(
                        !self.syncing,
                        theme::icon_text_button(theme::Icon::Refresh, sync_label),
                    )
                    .on_hover_text(
                        "把稿件库里所有有文号的公函/呈批件/纪要同步进来；手工补录的行不动",
                    )
                    .clicked()
                {
                    *action = Some(LedgerAction::Sync);
                }
                if ui
                    .add(theme::icon_text_button(theme::Icon::FileDown, "导出 Excel"))
                    .on_hover_text("导出当前筛选结果；编号、单位名与勾选的加密导出沿用原规则")
                    .clicked()
                {
                    *action = Some(LedgerAction::Export);
                }
                if ui
                    .add(theme::icon_text_button(theme::Icon::RotateCcw, "刷新"))
                    .on_hover_text("重新读台账，不重建自动行")
                    .clicked()
                {
                    *action = Some(LedgerAction::Refresh);
                }
                if !self.duplicates.is_empty() || !self.gaps.is_empty() {
                    let label = format!("问题 {}", self.duplicates.len() + self.gaps.len());
                    if ui
                        .add(theme::icon_text_button(theme::Icon::TriangleAlert, &label))
                        .on_hover_text("重号与断号；可点进池、补登废号")
                        .clicked()
                    {
                        self.problems_open = true;
                    }
                }
            });
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("搜索");
            ui.add(
                egui::TextEdit::singleline(&mut self.keyword)
                    .hint_text("标题、编号或备注")
                    .desired_width(260.0),
            );
            if !self.keyword.is_empty() && ui.small_button("清除").clicked() {
                self.keyword.clear();
            }
            if !show_rail {
                ui.separator();
                ui.label("池");
                let selected = self
                    .selected_pool_label(vocabulary)
                    .unwrap_or_else(|| "全部".into());
                egui::ComboBox::from_id_salt("issue_pool_combo")
                    .selected_text(selected)
                    .width(260.0)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(self.selected_pool.is_none(), "全部")
                            .clicked()
                        {
                            self.selected_pool = None;
                        }
                        for pool in &self.pools {
                            if ui
                                .selectable_label(
                                    self.selected_pool.as_ref() == Some(&pool.key),
                                    &pool.label,
                                )
                                .clicked()
                            {
                                self.selected_pool = Some(pool.key.clone());
                            }
                        }
                    });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let visible = self.visible_indices().len();
                ui.weak(format!("当前 {visible} 条"));
                if self.syncing {
                    ui.spinner();
                }
            });
        });
    }

    fn pools_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.strong("池");
        ui.add_space(2.0);
        ui.weak("单位 · 类型 · 年度");
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .id_salt("issue_ledger_pool_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if ui
                    .selectable_label(
                        self.selected_pool.is_none(),
                        format!("全部 · {} 条", self.entries.len()),
                    )
                    .clicked()
                {
                    self.selected_pool = None;
                }
                ui.add_space(4.0);
                let pools = self.pools.clone();
                for pool in &pools {
                    let selected = self.selected_pool.as_ref() == Some(&pool.key);
                    if ui.selectable_label(selected, &pool.rail_label).clicked() {
                        self.selected_pool = Some(pool.key.clone());
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.weak(&pool.key.year);
                        ui.weak(format!("· {} 条", pool.count));
                        if pool.duplicates > 0 {
                            ui.colored_label(theme::warn(), format!("· 重号 {}", pool.duplicates));
                        }
                        if !pool.missing.is_empty() {
                            ui.colored_label(
                                theme::text_muted(),
                                format!("· 缺 {}", summarize_numbers(&pool.missing)),
                            );
                        }
                    });
                    ui.add_space(2.0);
                }
                if pools.is_empty() {
                    ui.weak("还没有登记。可以补录纸质件，或同步稿件库。");
                }
            });
    }

    fn search_ui(&mut self, ui: &mut egui::Ui, vocabulary: &[VocabularyEntry]) {
        let Some(label) = self.selected_pool_label(vocabulary) else {
            return;
        };
        ui.horizontal(|ui| {
            ui.weak(format!("当前池：{label}"));
            if ui.small_button("全部池").clicked() {
                self.selected_pool = None;
            }
        });
    }

    fn table_ui(
        &mut self,
        ui: &mut egui::Ui,
        vocabulary: &[VocabularyEntry],
        wide: bool,
    ) -> Option<LedgerAction> {
        let visible = self.visible_indices();
        if visible.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                if !self.keyword.trim().is_empty() || self.selected_pool.is_some() {
                    ui.weak("没有匹配的登记。清空搜索或切换池试试。");
                } else {
                    ui.weak("还没有登记。");
                    ui.weak("可以「补录纸质件」，或点「同步稿件库」把旧稿补进来。");
                }
            });
            return None;
        }
        let compact = ui.available_width() < 760.0;
        let mut clicked: Option<i64> = None;
        let mut double: Option<IssueEntry> = None;
        let mut action = None;
        let mut table = TableBuilder::new(ui)
            .id_salt(("issue_ledger_table", compact))
            .striped(!theme::smartisan::active())
            .auto_shrink([false, false])
            .resizable(!compact)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .sense(egui::Sense::click());
        table = table
            .column(Column::exact(48.0).clip(true))
            .column(Column::exact(150.0).clip(true))
            .column(Column::remainder().at_least(140.0).clip(true))
            .column(Column::exact(88.0).clip(true))
            .column(Column::exact(58.0).clip(true));
        if !compact {
            table = table
                .column(Column::exact(78.0).clip(true))
                .column(Column::exact(58.0).clip(true))
                .column(Column::exact(52.0).clip(true));
        }
        table
            .header(26.0, |mut header| {
                for label in if compact {
                    vec!["序号", "编号", "标题", "日期", "状态"]
                } else {
                    vec![
                        "序号", "编号", "标题", "日期", "状态", "类型", "密级", "来源",
                    ]
                } {
                    header.col(|ui| {
                        theme::smartisan::ledger_header(ui);
                        ui.strong(label);
                    });
                }
                header
                    .response()
                    .on_hover_text("单击选中；双击打开稿件（手工补录则打开编辑）");
            })
            .body(|body| {
                body.rows(30.0, visible.len(), |mut row| {
                    let index = visible[row.index()];
                    let entry = &self.entries[index];
                    row.set_selected(self.selected == Some(entry.id));
                    let duplicated = self.duplicates.iter().any(|group| {
                        group.unit == self.pool_unit_of(&entry.unit)
                            && group.scheme == entry.scheme
                            && group.year == entry.year
                            && group.serial == entry.serial
                    });
                    row.col(|ui| {
                        ui.label(entry.serial.trim());
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(entry.number_label()).truncate());
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(&entry.title).truncate())
                            .on_hover_text(&entry.title);
                    });
                    row.col(|ui| {
                        ui.label(entry.doc_date.trim());
                    });
                    row.col(|ui| {
                        ui.horizontal(|ui| {
                            if duplicated {
                                ui.add(
                                    theme::Icon::TriangleAlert
                                        .image()
                                        .tint(theme::warn())
                                        .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                )
                                .on_hover_text("同池重号");
                            }
                            ui.label(entry.status_label());
                        });
                    });
                    if !compact {
                        row.col(|ui| {
                            ui.label(
                                NumberingScheme::from_str(&entry.scheme)
                                    .map(scheme_label)
                                    .unwrap_or(""),
                            );
                        });
                        row.col(|ui| {
                            ui.label(entry.security.trim());
                        });
                        row.col(|ui| {
                            ui.label(if entry.manuscript_id.is_some() {
                                "稿件"
                            } else {
                                "手工"
                            });
                        });
                    }
                    let response = row.response();
                    if response.double_clicked() {
                        double = Some(entry.clone());
                    } else if response.clicked() {
                        clicked = Some(entry.id);
                    }
                    if !entry.note.trim().is_empty() {
                        response.on_hover_text(format!("备注：{}", entry.note.trim()));
                    }
                });
            });
        if let Some(id) = clicked {
            self.selected = Some(id);
            if !wide {
                self.detail_modal = Some(id);
            }
            ui.ctx().request_repaint();
        }
        if let Some(entry) = double {
            if let Some(manuscript_id) = entry.manuscript_id {
                action = Some(LedgerAction::OpenManuscript(manuscript_id));
            } else {
                self.editing = Some(ManualEdit::from_entry(&entry, vocabulary));
            }
        }
        action
    }

    fn detail_modal_ui(
        &mut self,
        ctx: &egui::Context,
        vocabulary: &[VocabularyEntry],
    ) -> Option<LedgerAction> {
        let id = self.detail_modal?;
        let mut action = None;
        let response = modal::dialog(
            ctx,
            egui::Id::unique("issue_ledger_detail_modal"),
            "台账条目",
            460.0,
            Dismiss::EscOrBackdrop,
            |ui| {
                action = self.detail_ui(ui, vocabulary, id, false);
            },
        );
        if response.dismissed
            || matches!(action, Some(LedgerAction::OpenManuscript(_)))
            || self.editing.is_some()
            || self.confirm_delete.is_some()
        {
            self.detail_modal = None;
        }
        action
    }

    fn detail_ui(
        &mut self,
        ui: &mut egui::Ui,
        vocabulary: &[VocabularyEntry],
        id: i64,
        close_selected: bool,
    ) -> Option<LedgerAction> {
        let entry = self.entries.iter().find(|entry| entry.id == id).cloned()?;
        let display = UnitDisplay::new(vocabulary);
        let mut action = None;
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.strong(if entry.title.trim().is_empty() {
                    "（未填标题）"
                } else {
                    entry.title.trim()
                });
                ui.weak(entry.number_label());
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("关闭").clicked() {
                    if close_selected {
                        self.selected = None;
                    } else {
                        self.detail_modal = None;
                    }
                }
            });
        });
        ui.separator();
        egui::Grid::new(("issue_detail_grid", id))
            .num_columns(2)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                ui.weak("类型");
                ui.label(
                    NumberingScheme::from_str(&entry.scheme)
                        .map(scheme_label)
                        .unwrap_or("未知"),
                );
                ui.end_row();
                ui.weak("单位");
                ui.label(display.full_name(&entry.unit));
                ui.end_row();
                ui.weak("年度");
                ui.label(entry.year.trim());
                ui.end_row();
                ui.weak("日期");
                ui.label(if entry.doc_date.trim().is_empty() {
                    "—"
                } else {
                    entry.doc_date.trim()
                });
                ui.end_row();
                ui.weak("密级");
                ui.label(if entry.security.trim().is_empty() {
                    "—"
                } else {
                    entry.security.trim()
                });
                ui.end_row();
                ui.weak("来源");
                ui.label(if entry.manuscript_id.is_some() {
                    "稿件（内容随稿件更新）"
                } else {
                    "手工补录"
                });
                ui.end_row();
                ui.weak("状态");
                ui.horizontal(|ui| {
                    for (value, label) in [("used", "已用"), ("reserved", "占号"), ("void", "废号")]
                    {
                        let text = if entry.status == value {
                            format!("✓ {label}")
                        } else {
                            label.to_string()
                        };
                        if ui.selectable_label(entry.status == value, text).clicked()
                            && entry.status != value
                        {
                            action = Some(LedgerAction::SetStatus { id, status: value });
                        }
                    }
                });
                ui.end_row();
            });
        ui.add_space(6.0);
        ui.weak("备注（不参与重号/断号）");
        let note = self
            .note_edits
            .entry(id)
            .or_insert_with(|| entry.note.clone());
        ui.add(
            egui::TextEdit::multiline(note)
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
        if ui.button("保存备注").clicked() {
            action = Some(LedgerAction::SetNote {
                id,
                note: note.clone(),
            });
        }
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            if let Some(manuscript_id) = entry.manuscript_id
                && ui
                    .add(theme::icon_text_button(theme::Icon::Open, "打开稿件"))
                    .clicked()
            {
                action = Some(LedgerAction::OpenManuscript(manuscript_id));
            }
            if entry.manuscript_id.is_none() {
                if ui
                    .add(theme::icon_text_button(theme::Icon::Edit, "编辑补录"))
                    .clicked()
                {
                    self.editing = Some(ManualEdit::from_entry(&entry, vocabulary));
                }
                if ui
                    .add(theme::icon_text_button(theme::Icon::Trash, "删除补录"))
                    .clicked()
                {
                    self.confirm_delete = Some(entry.clone());
                }
            }
        });
        if entry.manuscript_id.is_some() {
            ui.add_space(4.0);
            ui.weak("标题、日期、密级等来自稿件；这里只改登记状态和备注。");
        }
        action
    }

    fn manual_modal(
        &mut self,
        ctx: &egui::Context,
        vocabulary: &[VocabularyEntry],
        allow_free_text: bool,
    ) -> Option<LedgerAction> {
        let mut edit = self.editing.take()?;
        let mut action = None;
        let mut close = false;
        let response = modal::dialog(
            ctx,
            egui::Id::unique("issue_manual_modal"),
            if edit.id.is_some() {
                "编辑补录"
            } else {
                "补录纸质件"
            },
            560.0,
            Dismiss::EscOnly,
            |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("issue_manual_scroll")
                    .max_height(500.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        edit.fields_ui(ui, vocabulary, &self.entries, allow_free_text);
                    });
                if let Some(error) = &edit.error {
                    ui.add_space(4.0);
                    ui.colored_label(theme::danger(), error);
                }
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("保存").clicked() {
                        match edit.validate(vocabulary) {
                            Ok(()) => {
                                edit.error = None;
                                action = Some(LedgerAction::SaveManual {
                                    id: edit.id,
                                    form: Box::new(edit.form.clone()),
                                });
                            }
                            Err(error) => edit.error = Some(error),
                        }
                    }
                    if ui.button("取消").clicked() {
                        close = true;
                    }
                });
            },
        );
        if response.dismissed {
            close = true;
        }
        if !close {
            self.editing = Some(edit);
        }
        action
    }

    fn delete_modal(&mut self, ctx: &egui::Context) -> Option<LedgerAction> {
        let entry = self.confirm_delete.clone()?;
        let mut action = None;
        let mut close = false;
        let response = modal::dialog(
            ctx,
            egui::Id::unique("issue_delete_modal"),
            "删除补录条目",
            420.0,
            Dismiss::EscOrBackdrop,
            |ui| {
                ui.label(format!(
                    "将从发文登记簿删除 {}（{}）。",
                    if entry.title.trim().is_empty() {
                        "（未填标题）"
                    } else {
                        entry.title.trim()
                    },
                    entry.number_label()
                ));
                ui.label("稿件与正文不受影响；删除后不能恢复。");
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new("删除").color(theme::danger()),
                        ))
                        .clicked()
                    {
                        action = Some(LedgerAction::Delete(entry.id));
                        close = true;
                    }
                    if ui.button("取消").clicked() {
                        close = true;
                    }
                });
            },
        );
        if response.dismissed {
            close = true;
        }
        if close {
            self.confirm_delete = None;
        }
        action
    }

    fn problems_modal(&mut self, ctx: &egui::Context, vocabulary: &[VocabularyEntry]) {
        if !self.problems_open {
            return;
        }
        let duplicates = self.duplicates.clone();
        let gaps = self.gaps.clone();
        let mut close = false;
        let response = modal::dialog(
            ctx,
            egui::Id::unique("issue_problems_modal"),
            "重号与断号",
            540.0,
            Dismiss::EscOrBackdrop,
            |ui| {
                egui::ScrollArea::vertical()
                    .max_height(460.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.strong("重号");
                        if duplicates.is_empty() {
                            ui.weak("没有重号。");
                        }
                        for group in &duplicates {
                            let scheme = NumberingScheme::from_str(&group.scheme)
                                .map(scheme_label)
                                .unwrap_or("");
                            let unit = UnitDisplay::new(vocabulary).full_name(&group.unit);
                            let label = format!(
                                "{} · {} · {} 年第 {} 号（{} 条）",
                                unit, scheme, group.year, group.serial, group.count
                            );
                            if ui.link(label).on_hover_text(&group.titles).clicked() {
                                self.selected_pool = Some(PoolKey {
                                    unit: group.unit.clone(),
                                    scheme: group.scheme.clone(),
                                    year: group.year.clone(),
                                });
                                // 用“12号”而不是完整编号：同池不同代字也能一起看到，
                                // 又不会把 1 号扩成 10、11 号。
                                self.keyword = format!("{}号", group.serial);
                                close = true;
                            }
                            ui.add_space(2.0);
                        }
                        ui.add_space(8.0);
                        ui.strong("断号");
                        ui.weak("点数字补登为废号；纸质件已有的号请用「补录纸质件」。");
                        if gaps.is_empty() {
                            ui.weak("没有断号。");
                        }
                        for group in &gaps {
                            let scheme = NumberingScheme::from_str(&group.scheme)
                                .map(scheme_label)
                                .unwrap_or("");
                            let unit = UnitDisplay::new(vocabulary).full_name(&group.unit);
                            ui.horizontal_wrapped(|ui| {
                                ui.label(format!("{} · {} · {} 缺", unit, scheme, group.year));
                                for number in group.missing.iter().take(20) {
                                    if ui
                                        .small_button(number.to_string())
                                        .on_hover_text("补登为废号")
                                        .clicked()
                                    {
                                        self.start_void(group, *number, vocabulary);
                                        close = true;
                                    }
                                }
                                if group.missing.len() > 20 {
                                    ui.weak("…");
                                }
                            });
                            ui.add_space(4.0);
                        }
                        if duplicates.is_empty() && gaps.is_empty() {
                            ui.weak("没有需要处理的池。");
                        }
                    });
                ui.add_space(10.0);
                if ui.button("关闭").clicked() {
                    close = true;
                }
            },
        );
        if response.dismissed {
            close = true;
        }
        if close {
            self.problems_open = false;
        }
    }

    fn start_add(&mut self, vocabulary: &[VocabularyEntry]) {
        let (unit, scheme, year, code) = if let Some(pool) = &self.selected_pool {
            let scheme = NumberingScheme::from_str(&pool.scheme).unwrap_or(NumberingScheme::Letter);
            let code = self
                .entries
                .iter()
                .find(|entry| {
                    pool.unit == self.pool_unit_of(&entry.unit)
                        && pool.scheme == entry.scheme
                        && pool.year == entry.year
                })
                .map(|entry| entry.code.clone())
                .unwrap_or_default();
            (pool.unit.clone(), scheme, pool.year.clone(), code)
        } else {
            (
                String::new(),
                NumberingScheme::Letter,
                chrono::Local::now().format("%Y").to_string(),
                String::new(),
            )
        };
        self.editing = Some(ManualEdit::new(vocabulary, unit, scheme, year, code));
    }

    fn start_void(&mut self, group: &GapGroup, serial: u32, vocabulary: &[VocabularyEntry]) {
        let scheme = NumberingScheme::from_str(&group.scheme).unwrap_or(NumberingScheme::Letter);
        let code = self
            .entries
            .iter()
            .find(|entry| {
                entry.scheme == group.scheme
                    && entry.year == group.year
                    && unit_pool_key(vocabulary, &entry.unit)
                        == unit_pool_key(vocabulary, &group.unit)
            })
            .map(|entry| entry.code.clone())
            .unwrap_or_default();
        let mut edit = ManualEdit::new(
            vocabulary,
            group.unit.clone(),
            scheme,
            group.year.clone(),
            code,
        );
        edit.form.serial = serial.to_string();
        edit.form.status = "void".into();
        edit.form.note = "补登废号".into();
        self.editing = Some(edit);
    }
}

fn unit_pool_key(vocabulary: &[VocabularyEntry], unit: &str) -> String {
    identity::resolve(vocabulary, VocabularyCategory::Unit, unit)
        .map(|entry| entry.key())
        .unwrap_or_else(|| unit.trim().to_string())
}

fn scheme_label(scheme: NumberingScheme) -> &'static str {
    match scheme {
        NumberingScheme::Letter => "公函",
        NumberingScheme::Approval => "红头呈批件",
        NumberingScheme::Minutes => "会议纪要",
    }
}

fn scheme_short(scheme: NumberingScheme) -> &'static str {
    match scheme {
        NumberingScheme::Letter => "公函",
        NumberingScheme::Approval => "呈批",
        NumberingScheme::Minutes => "纪要",
    }
}

fn scheme_order(scheme: &str) -> u8 {
    match scheme {
        "letter" => 0,
        "approval" => 1,
        "minutes" => 2,
        _ => 3,
    }
}

fn status_label(status: &str) -> &'static str {
    match status {
        "reserved" => "占号",
        "void" => "废号",
        _ => "已用",
    }
}

fn summarize_numbers(numbers: &[u32]) -> String {
    if numbers.len() > 12 {
        format!(
            "{}…",
            numbers
                .iter()
                .take(12)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("、")
        )
    } else {
        numbers
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("、")
    }
}

fn conflict_for(
    entries: &[IssueEntry],
    form: &NewIssue,
    editing_id: Option<i64>,
    vocabulary: &[VocabularyEntry],
) -> Option<String> {
    let serial = form.serial.trim();
    if serial.is_empty() {
        return None;
    }
    let unit = unit_pool_key(vocabulary, &form.unit);
    entries
        .iter()
        .find(|entry| {
            Some(entry.id) != editing_id
                && entry.serial.trim() == serial
                && entry.scheme == form.scheme.as_str()
                && entry.year.trim() == form.year.trim()
                && entry.status != "void"
                && unit_pool_key(vocabulary, &entry.unit) == unit
        })
        .map(|entry| {
            if entry.title.trim().is_empty() {
                entry.number_label()
            } else {
                entry.title.clone()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(unit: &str, serial: &str, status: &str, title: &str) -> IssueEntry {
        IssueEntry {
            id: 0,
            manuscript_id: None,
            unit: unit.into(),
            scheme: "letter".into(),
            code: "某函".into(),
            year: "2026".into(),
            serial: serial.into(),
            title: title.into(),
            kind: "OfficialLetter".into(),
            doc_date: "2026-01-01".into(),
            security: "内部".into(),
            status: status.into(),
            note: String::new(),
        }
    }

    fn unit_vocabulary() -> Vec<VocabularyEntry> {
        vec![VocabularyEntry {
            identity: "abc".into(),
            category: VocabularyCategory::Unit,
            canonical: "甲单位".into(),
            code: "00".into(),
            ..Default::default()
        }]
    }

    #[test]
    fn ref_and_name_forms_of_one_unit_share_a_pool_and_a_duplicate() {
        let mut state = IssueLedgerState::default();
        let mut first = entry("@unit:abc", "1", "used", "第一篇");
        first.id = 1;
        first.manuscript_id = Some(1);
        let mut second = entry("甲单位", "1", "used", "第二篇");
        second.id = 2;
        state.entries = vec![first, second];
        state.recompute(&unit_vocabulary());
        assert_eq!(state.pools.len(), 1);
        assert_eq!(state.pools[0].count, 2);
        assert_eq!(state.pools[0].duplicates, 1);
        assert_eq!(state.duplicates.len(), 1);
        assert!(state.gaps.is_empty());
    }

    #[test]
    fn void_number_closes_the_gap_but_stays_out_of_duplicates() {
        let mut state = IssueLedgerState::default();
        let mut one = entry("甲单位", "1", "used", "第一篇");
        one.id = 1;
        let mut two = entry("甲单位", "2", "void", "补登废号");
        two.id = 2;
        let mut duplicate_void = entry("甲单位", "1", "void", "作废副本");
        duplicate_void.id = 3;
        state.entries = vec![one, two, duplicate_void];
        state.recompute(&unit_vocabulary());
        assert!(state.gaps.is_empty(), "2 号被废号占住，不应再报断号");
        assert!(state.duplicates.is_empty(), "废号不参与重号统计");
    }
}
