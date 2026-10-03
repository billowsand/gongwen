//! 送批材料面板：呈批件挂随行件的清单、各件版本状态与增删排序。
//!
//! 数据层在 `manuscript::send_package`，方案见 `docs/send-package-design.md`。
//! 面板只负责组织：不碰任何件的正文与行文要素。各件的版本状态查库得来，
//! 每隔几秒自动重查一次——别的标签里提交了版本，这里不用手动刷新也能跟上。
//!
//! 合并导出分两步：点「导出合并 PDF…」先生成导出计划（每件用哪一版、哪件不能进包）
//! 给人确认；确认后整份计划交给后台线程逐件编译、合并，确认之后再提交的版本不会混进来。

use crate::app::{
    GongwenApp, VersionDiffState, VersionScope, WorkerResult, short_date, status_color, summarize,
};
use crate::manuscript::send_package::{
    AddBlock, ExportRecord, ManuscriptBrief, PackageItemState, SendPackagePlan,
};
use crate::manuscript::{ManuscriptFilter, ManuscriptRow, ManuscriptStore, ManuscriptUpdate};
use crate::manuscript_io::send_package::SendPackageOutcome;
use crate::models::{ManuscriptStatus, TemplateKind};
use crate::theme;
use anyhow::Context as _;
use eframe::egui;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// 面板状态自动重查的间隔。
const REFRESH_INTERVAL: Duration = Duration::from_secs(3);
/// 清单区最高这么高，再多就滚动；展开候选区时让出一半，窗口别顶出屏幕。
const ROWS_MAX_HEIGHT: f32 = 460.0;
const ROWS_MAX_HEIGHT_PICKING: f32 = 240.0;
/// 添加稿件的候选列表最多列这么多条，再多请用关键词缩小范围。
const PICKER_LIMIT: usize = 200;
/// 导出前确认清单最高这么高，件多了就滚动。
const CONFIRM_MAX_HEIGHT: f32 = 220.0;

#[cfg(test)]
#[path = "send_package_ui_tests.rs"]
mod tests;

/// 面板里的一行：主件或随行件。
struct PackageRow {
    /// 随行件的稿件 UUID；主件行为空。
    item_uuid: String,
    /// 本机找不到的随行件为 `None`。
    brief: Option<ManuscriptBrief>,
    state: Option<PackageItemState>,
    /// 最近一次导出里这一件用的版本号；没导出过或当时不在清单里为 `None`。
    last_exported: Option<i64>,
    /// 主件归档时钉住的版本：外层 `None` 为没钉；内层同 `revision_number`。
    pinned: Option<Option<Option<i64>>>,
}

impl egui_dnd::DragDropItem for &PackageRow {
    fn id(&self) -> egui::Id {
        egui::Id::new(&self.item_uuid)
    }
}

/// 导出前的确认：计划已生成，等人勾选目录页、选保存位置。
struct ExportConfirm {
    plan: SendPackagePlan,
    with_toc: bool,
}

/// 卡片里展开的行内提交表单：同一时刻只开一张。
struct CommitForm {
    manuscript_id: i64,
    name: String,
    comment: String,
    /// 刚展开，等下一次绘制时把焦点给版本名输入框。
    focus: bool,
    error: Option<String>,
}

struct SortUndo {
    before: Vec<String>,
    after: Vec<String>,
}

/// 后台导出线程发回的消息。
pub(crate) enum SendPackageEvent {
    Progress {
        text: String,
    },
    Done {
        owner_id: i64,
        with_toc: bool,
        result: Result<SendPackageOutcome, String>,
    },
}

/// 正在后台进行的合并导出。放在应用上而不是面板上：导出途中关掉面板，
/// 导出照样跑完、照样记账。
pub(crate) struct SendPackageExportJob {
    owner_id: i64,
    progress: String,
}

/// 添加稿件的候选区。
#[derive(Default)]
struct Picker {
    keyword: String,
    kind: Option<TemplateKind>,
    /// 上次查询用的条件；与当前不同就重查。
    applied: Option<(String, Option<TemplateKind>)>,
    rows: Vec<(ManuscriptRow, Option<AddBlock>)>,
    truncated: bool,
}

pub(crate) struct SendPackagePanel {
    owner_id: i64,
    owner: Option<PackageRow>,
    items: Vec<PackageRow>,
    picker: Option<Picker>,
    confirm: Option<ExportConfirm>,
    /// 历次导出记录，最新的在前。
    exports: Vec<ExportRecord>,
    /// 最近一次导出失败的完整原因；状态栏一行放不下。
    export_error: Option<String>,
    loaded_at: Option<Instant>,
    error: Option<String>,
    /// 拖动期间暂停定时重读，保证落点索引对应同一份清单。
    dragging: bool,
    sort_undo: Option<SortUndo>,
    commit_form: Option<CommitForm>,
}

impl SendPackagePanel {
    fn new(owner_id: i64) -> Self {
        Self {
            owner_id,
            owner: None,
            items: Vec::new(),
            picker: None,
            confirm: None,
            exports: Vec::new(),
            export_error: None,
            loaded_at: None,
            error: None,
            dragging: false,
            sort_undo: None,
            commit_form: None,
        }
    }

    /// 主件与随行件里需要提交版本的件（从未提交、有未提交修改），连同默认版本名。
    fn pending_commits(&self, unsaved_in_editor: &dyn Fn(i64) -> bool) -> Vec<(i64, String)> {
        self.owner
            .iter()
            .chain(self.items.iter())
            .filter_map(|row| {
                let (brief, state) = (row.brief.as_ref()?, row.state.as_ref()?);
                let needs = row.pinned.is_none()
                    && brief.status != ManuscriptStatus::Archived
                    && (state.latest.is_none()
                        || state.has_uncommitted
                        || unsaved_in_editor(brief.id));
                needs.then(|| (brief.id, default_version_name(state)))
            })
            .collect()
    }

    /// 不能进包的件数：找不到稿件、从未提交过版本，或钉住的版本在本机找不到。
    fn blocked_count(&self) -> usize {
        self.owner
            .iter()
            .chain(self.items.iter())
            .filter(|row| match row.pinned {
                Some(pin) => pin.is_none(),
                None => row
                    .state
                    .as_ref()
                    .is_none_or(|state| state.latest.is_none()),
            })
            .count()
    }

    fn archived(&self) -> bool {
        self.owner
            .as_ref()
            .and_then(|row| row.brief.as_ref())
            .is_some_and(|brief| brief.status == ManuscriptStatus::Archived)
    }

    fn move_item(
        &mut self,
        store: &mut ManuscriptStore,
        from: usize,
        to: usize,
    ) -> anyhow::Result<()> {
        if from == to || from >= self.items.len() || to >= self.items.len() {
            return Ok(());
        }
        let mut order: Vec<String> = self.items.iter().map(|row| row.item_uuid.clone()).collect();
        let before = order.clone();
        let moved = order.remove(from);
        order.insert(to, moved);
        self.save_order(store, &order)?;
        self.sort_undo = Some(SortUndo {
            before,
            after: order,
        });
        Ok(())
    }

    fn save_order(&mut self, store: &mut ManuscriptStore, order: &[String]) -> anyhow::Result<()> {
        store.reorder_send_package(self.owner_id, order)?;
        // 确认清单是导出计划的快照，排序后须重新生成，不能沿用旧顺序。
        if let Some(mut confirm) = self.confirm.take() {
            confirm.plan = store.send_package_plan(self.owner_id)?;
            self.confirm = Some(confirm);
        }
        Ok(())
    }

    fn undo_sort(&mut self, store: &mut ManuscriptStore) -> anyhow::Result<()> {
        let Some(undo) = &self.sort_undo else {
            return Ok(());
        };
        let current: Vec<_> = store
            .send_package_items(self.owner_id)?
            .into_iter()
            .map(|item| item.item_uuid)
            .collect();
        if current != undo.after {
            self.sort_undo = None;
            anyhow::bail!("清单已发生变化，无法撤销上一次排序。");
        }
        let before = undo.before.clone();
        self.save_order(store, &before)?;
        self.sort_undo = None;
        Ok(())
    }

    /// 从库里重读主件与清单。出错时保留上次的内容，只记下错误。
    fn reload(&mut self, store: &ManuscriptStore) {
        let result =
            (|| -> anyhow::Result<()> {
                let exports = store.send_package_exports(self.owner_id)?;
                let exported = |uuid: &str| {
                    exports.first().and_then(|record| {
                        record
                            .items
                            .iter()
                            .find(|item| item.document_uuid == uuid)
                            .and_then(|item| item.visible_number)
                    })
                };
                let (owner_uuid, _) = store.document_identity(self.owner_id)?;
                let pinned = |uuid: Option<String>| -> anyhow::Result<Option<Option<Option<i64>>>> {
                    uuid.map(|uuid| store.revision_number(&uuid)).transpose()
                };
                let mut owner = load_row(store, String::new(), Some(self.owner_id))?;
                owner.last_exported = exported(&owner_uuid);
                owner.pinned = pinned(store.owner_pin(self.owner_id)?)?;
                let mut items = Vec::new();
                for item in store.send_package_items(self.owner_id)? {
                    let mut row = load_row(store, item.item_uuid, item.manuscript_id)?;
                    row.last_exported = exported(&row.item_uuid);
                    row.pinned = pinned(item.pinned_revision_uuid)?;
                    items.push(row);
                }
                self.owner = Some(owner);
                if self.sort_undo.as_ref().is_some_and(|undo| {
                    !items.iter().map(|row| &row.item_uuid).eq(undo.after.iter())
                }) {
                    self.sort_undo = None;
                }
                self.items = items;
                self.exports = exports;
                Ok(())
            })();
        self.error = result.err().map(|error| format!("{error:#}"));
        self.loaded_at = Some(Instant::now());
        if let Some(picker) = &mut self.picker {
            // 清单变了，候选的可选性也跟着变。
            picker.applied = None;
        }
    }
}

fn load_row(
    store: &ManuscriptStore,
    item_uuid: String,
    id: Option<i64>,
) -> anyhow::Result<PackageRow> {
    let brief = match id {
        Some(id) => store.manuscript_brief(id)?,
        None => None,
    };
    let state = match &brief {
        Some(brief) => Some(store.send_package_item_state(brief.id)?),
        None => None,
    };
    Ok(PackageRow {
        item_uuid,
        brief,
        state,
        last_exported: None,
        pinned: None,
    })
}

/// 首次提交叫「初稿」，之后叫「修订稿」。
fn default_version_name(state: &PackageItemState) -> String {
    if state.latest.is_none() {
        "初稿"
    } else {
        "修订稿"
    }
    .into()
}

/// 面板里点出的动作，帧末统一执行。
enum PanelAction {
    /// 在卡片里展开提交表单。
    OpenCommit {
        manuscript_id: i64,
        name: String,
    },
    CancelCommit,
    Commit {
        manuscript_id: i64,
        name: String,
        comment: String,
    },
    /// 把所有待提交的件各提交一版，用默认版本名。
    CommitAll,
    Refresh,
    UndoSort,
    Move {
        from: usize,
        to: usize,
    },
    Remove(String),
    Open(i64),
    Add(i64),
    TogglePicker,
    /// 对照某件上次导出用的版本与当前最新提交版。
    Diff {
        manuscript_id: i64,
        from: i64,
        to: i64,
    },
    /// 生成导出计划，进入确认。
    PlanExport,
    CancelExport,
    /// 确认后选保存位置、开始导出。
    StartExport,
    OpenFile(PathBuf),
    Close,
}

impl GongwenApp {
    /// 打开某篇呈批件的送批材料面板。
    pub(crate) fn open_send_package(&mut self, owner_id: i64) {
        let mut panel = SendPackagePanel::new(owner_id);
        match self.manuscript_store.as_ref() {
            Some(store) => panel.reload(store),
            None => {
                self.status = "稿件库不可用，无法打开送批材料。".into();
                return;
            }
        }
        self.send_package = Some(panel);
    }

    pub(crate) fn send_package_window(&mut self, ctx: &egui::Context) {
        let Some(mut panel) = self.send_package.take() else {
            return;
        };
        if !panel.dragging
            && panel
                .loaded_at
                .is_none_or(|at| at.elapsed() >= REFRESH_INTERVAL)
            && let Some(store) = self.manuscript_store.as_ref()
        {
            panel.reload(store);
        }
        // 界面空闲时不重绘，定时刷新也就停了；面板开着就按刷新间隔叫醒一次。
        ctx.request_repaint_after(REFRESH_INTERVAL);
        if let (Some(picker), Some(store)) = (&mut panel.picker, self.manuscript_store.as_mut()) {
            refresh_picker(picker, store, panel.owner_id);
        }
        // 起草页里还没存库的改动同样不会进包，一并算作「未提交」。
        let unsaved_in_editor = |id: i64| {
            self.docs
                .iter()
                .any(|doc| doc.manuscript_id == Some(id) && doc.is_dirty())
        };
        let archived = panel.archived();
        let pending = panel.pending_commits(&unsaved_in_editor);
        let blocked = panel.blocked_count();
        let total = usize::from(panel.owner.is_some()) + panel.items.len();
        let exporting = self
            .send_package_export
            .as_ref()
            .map(|job| (job.owner_id == panel.owner_id, job.progress.clone()));
        let mut keep = true;
        let mut action = None;

        egui::Window::new("送批材料")
            .id(egui::Id::new("send_package_window"))
            .open(&mut keep)
            .collapsible(false)
            .resizable(true)
            .default_width(640.0)
            .min_width(480.0)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if archived {
                        theme::chip(ui, "已归档 · 只读", theme::text_muted(), theme::surface());
                    }
                    ui.weak(if archived {
                        "导出时每件取归档时钉住的版本，按下面顺序合并成一个 PDF。"
                    } else {
                        "导出时每件取最新提交版，按下面顺序合并成一个 PDF。"
                    });
                });
                if let Some(error) = &panel.error {
                    ui.colored_label(theme::danger(), format!("读取失败：{error}"));
                }
                if panel.owner.is_some() {
                    let mut parts = vec![format!("共 {total} 件")];
                    if blocked > 0 {
                        parts.push(format!("{blocked} 件不能进包"));
                    }
                    if !pending.is_empty() {
                        parts.push(format!("{} 件待提交", pending.len()));
                    }
                    let ready = blocked == 0 && pending.is_empty();
                    if ready {
                        parts.push("可导出".into());
                    }
                    ui.label(
                        egui::RichText::new(parts.join(" · "))
                            .color(if ready { theme::success() } else { theme::warn() }),
                    );
                }
                ui.add_space(4.0);

                egui::ScrollArea::vertical()
                    .id_salt("send_package_rows")
                    .auto_shrink([false, true])
                    .max_height(if panel.picker.is_some() || panel.confirm.is_some() {
                        ROWS_MAX_HEIGHT_PICKING
                    } else {
                        ROWS_MAX_HEIGHT
                    })
                    .show(ui, |ui| {
                        if let Some(owner) = &panel.owner {
                            row_card(
                                ui,
                                owner,
                                RowRole::Owner,
                                None,
                                &unsaved_in_editor,
                                &mut panel.commit_form,
                                &mut action,
                            );
                        }
                        if panel.items.is_empty() {
                            ui.add_space(4.0);
                            ui.weak("还没有送批材料。");
                        }
                        panel.dragging = items_ui(
                            ui,
                            panel.owner_id,
                            &panel.items,
                            !archived,
                            &unsaved_in_editor,
                            &mut panel.commit_form,
                            &mut action,
                        );
                    });

                ui.add_space(6.0);
                if let Some(confirm) = &mut panel.confirm {
                    confirm_ui(ui, confirm, &unsaved_in_editor, &mut action);
                    return;
                }
                let picking = panel.picker.is_some();
                ui.horizontal(|ui| {
                    match &exporting {
                        Some((true, progress)) => {
                            ui.spinner();
                            ui.label(progress);
                        }
                        Some((false, _)) => {
                            ui.add_enabled(
                                false,
                                theme::icon_text_button(theme::Icon::FileDown, "导出合并 PDF…"),
                            )
                            .on_disabled_hover_text("另一件呈批件的送批材料正在导出，请稍候");
                        }
                        None if blocked > 0 => {
                            ui.add_enabled(
                                false,
                                theme::icon_text_button(theme::Icon::FileDown, "导出合并 PDF…"),
                            )
                            .on_disabled_hover_text(format!(
                                "有 {blocked} 件还不能进包（尚未提交版本或找不到稿件）"
                            ));
                        }
                        None => {
                            if ui
                                .add(theme::icon_text_button(
                                    theme::Icon::FileDown,
                                    "导出合并 PDF…",
                                ))
                                .on_hover_text("先列出每件要用的版本给你确认，再逐件编译并合并")
                                .clicked()
                            {
                                action = Some(PanelAction::PlanExport);
                            }
                        }
                    }
                    if exporting.is_none()
                        && pending.len() >= 2
                        && ui
                            .add(theme::icon_text_button(theme::Icon::GitCommit, "全部提交"))
                            .on_hover_text(format!(
                                "给 {} 件待提交的稿件各提交一个版本（首次叫「初稿」，之后叫「修订稿」）",
                                pending.len()
                            ))
                            .clicked()
                    {
                        action = Some(PanelAction::CommitAll);
                    }
                    if !archived
                        && ui
                            .add(theme::icon_text_button(
                                if picking {
                                    theme::Icon::ChevronUp
                                } else {
                                    theme::Icon::FilePlus
                                },
                                if picking { "收起" } else { "添加稿件…" },
                            ))
                            .clicked()
                    {
                        action = Some(PanelAction::TogglePicker);
                    }
                    if !archived && panel.sort_undo.is_some() && ui.small_button("撤销排序").clicked() {
                        action = Some(PanelAction::UndoSort);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::icon_button(ui, theme::Icon::Refresh, "刷新各件状态")
                            .clicked()
                        {
                            action = Some(PanelAction::Refresh);
                        }
                    });
                });
                if let Some(error) = &panel.export_error {
                    ui.colored_label(theme::danger(), format!("上次导出失败，没有生成文件：{error}"));
                }
                if !archived && let Some(picker) = &mut panel.picker {
                    picker_ui(ui, picker, &mut action);
                }
                history_ui(ui, &panel.exports, &mut action);
            });
        if !keep {
            action = Some(PanelAction::Close);
        }
        self.send_package = Some(panel);
        if let Some(action) = action {
            self.apply_send_package_action(action);
        }
    }

    /// 提交一个稿件版本。稿件在某个标签里开着就提交编辑器里的当前内容（含未保存的修改），
    /// 否则提交稿件库里存的内容。
    fn commit_manuscript_from_panel(
        &mut self,
        id: i64,
        name: &str,
        comment: &str,
    ) -> anyhow::Result<()> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "版本名称不能为空");
        let comment = comment.trim();
        let open = self
            .docs
            .iter()
            .position(|doc| doc.manuscript_id == Some(id));
        let store = self
            .manuscript_store
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
        let notes = store.notes_of(id)?.context("稿件不存在，无法提交版本")?;
        let (snapshot, content) = match open {
            Some(index) => {
                let doc = &self.docs[index];
                let pair = (doc.draft.clone(), doc.generated_markdown.clone());
                store.update(
                    id,
                    &ManuscriptUpdate {
                        snapshot: pair.0.clone(),
                        content_markdown: pair.1.clone(),
                        notes: notes.clone(),
                    },
                )?;
                pair
            }
            None => {
                let record = store.get(id)?.context("稿件不存在，无法提交版本")?;
                (record.snapshot, record.content_markdown)
            }
        };
        store.commit_manuscript_version(id, name, comment, &snapshot, &content, &notes)?;
        if let Some(index) = open {
            let doc = &mut self.docs[index];
            doc.loaded_version = None;
            doc.mark_saved();
            self.refresh_committed_baseline(index);
        }
        self.manuscript_dirty = true;
        Ok(())
    }

    /// 提交类动作：要动编辑器标签，单独处理，不与面板借用缠在一起。
    fn apply_commit_action(&mut self, action: PanelAction) {
        let single = matches!(action, PanelAction::Commit { .. });
        let targets: Vec<(i64, String, String)> = match action {
            PanelAction::Commit {
                manuscript_id,
                name,
                comment,
            } => vec![(manuscript_id, name, comment)],
            PanelAction::CommitAll => {
                let Some(panel) = self.send_package.as_ref() else {
                    return;
                };
                let unsaved = |id: i64| {
                    self.docs
                        .iter()
                        .any(|doc| doc.manuscript_id == Some(id) && doc.is_dirty())
                };
                panel
                    .pending_commits(&unsaved)
                    .into_iter()
                    .map(|(id, name)| (id, name, String::new()))
                    .collect()
            }
            _ => return,
        };
        let mut done = 0;
        let mut errors = Vec::new();
        for (id, name, comment) in targets {
            match self.commit_manuscript_from_panel(id, &name, &comment) {
                Ok(()) => done += 1,
                Err(error) => {
                    let title = self
                        .manuscript_store
                        .as_ref()
                        .and_then(|store| store.manuscript_brief(id).ok().flatten())
                        .map(|brief| brief.title)
                        .unwrap_or_default();
                    errors.push((id, title, format!("{error:#}")));
                }
            }
        }
        if let Some(panel) = self.send_package.as_mut() {
            match (single, errors.first()) {
                (true, Some((_, _, error))) => {
                    if let Some(form) = &mut panel.commit_form {
                        form.error = Some(error.clone());
                    }
                }
                _ => panel.commit_form = None,
            }
        }
        self.status = match (done, errors.as_slice()) {
            (_, []) if single => "已提交版本。".into(),
            (_, []) => format!("已为 {done} 件稿件各提交一个版本。"),
            (_, [(_, title, error)]) if single => format!("提交《{title}》失败：{error}"),
            (_, errors) => format!(
                "已提交 {done} 件，{} 件失败：{}",
                errors.len(),
                errors
                    .iter()
                    .map(|(_, title, error)| format!("《{title}》{error}"))
                    .collect::<Vec<_>>()
                    .join("；")
            ),
        };
        self.reload_detail();
        if let (Some(panel), Some(store)) =
            (self.send_package.as_mut(), self.manuscript_store.as_ref())
        {
            panel.reload(store);
        }
    }

    fn apply_send_package_action(&mut self, action: PanelAction) {
        if matches!(action, PanelAction::Commit { .. } | PanelAction::CommitAll) {
            self.apply_commit_action(action);
            return;
        }
        let Some(panel) = self.send_package.as_mut() else {
            return;
        };
        let owner_id = panel.owner_id;
        let after = action_kind_after(&action);
        let result: anyhow::Result<Option<String>> = (|| {
            let store = self
                .manuscript_store
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
            match action {
                PanelAction::Refresh => Ok(None),
                PanelAction::OpenCommit {
                    manuscript_id,
                    name,
                } => {
                    panel.commit_form = Some(CommitForm {
                        manuscript_id,
                        name,
                        comment: String::new(),
                        focus: true,
                        error: None,
                    });
                    Ok(None)
                }
                PanelAction::CancelCommit => {
                    panel.commit_form = None;
                    Ok(None)
                }
                PanelAction::Commit { .. } | PanelAction::CommitAll => Ok(None),
                PanelAction::UndoSort => {
                    panel.undo_sort(store)?;
                    Ok(Some("已撤销上一次排序。".into()))
                }
                PanelAction::Move { from, to } => {
                    panel.move_item(store, from, to)?;
                    Ok(None)
                }
                PanelAction::Remove(uuid) => {
                    store.remove_send_package_item(owner_id, &uuid)?;
                    Ok(Some("已从送批材料中移除（稿件本身不受影响）。".into()))
                }
                PanelAction::Add(id) => {
                    store.add_send_package_item(owner_id, id)?;
                    let title = store
                        .manuscript_brief(id)?
                        .map(|brief| brief.title)
                        .unwrap_or_default();
                    Ok(Some(format!("已把《{title}》加入送批材料。")))
                }
                PanelAction::TogglePicker => {
                    panel.picker = match panel.picker.take() {
                        Some(_) => None,
                        None => Some(Picker::default()),
                    };
                    Ok(None)
                }
                PanelAction::PlanExport => {
                    panel.picker = None;
                    panel.export_error = None;
                    panel.confirm = Some(ExportConfirm {
                        plan: store.send_package_plan(owner_id)?,
                        with_toc: panel.confirm.as_ref().is_some_and(|c| c.with_toc),
                    });
                    Ok(None)
                }
                PanelAction::CancelExport => {
                    panel.confirm = None;
                    Ok(None)
                }
                PanelAction::Open(_)
                | PanelAction::Close
                | PanelAction::Diff { .. }
                | PanelAction::StartExport
                | PanelAction::OpenFile(_) => Ok(None),
            }
        })();
        match result {
            Ok(message) => {
                if let Some(message) = message {
                    self.status = message;
                }
            }
            Err(error) => self.status = format!("送批材料操作失败：{error:#}"),
        }
        match after {
            After::Close => self.send_package = None,
            After::Open(id) => self.open_in_editor(id),
            After::Diff {
                manuscript_id,
                from,
                to,
            } => {
                self.version_diff = Some(VersionDiffState {
                    scope: VersionScope::Manuscript(manuscript_id),
                    from: Some(from),
                    to: Some(to),
                    to_is_current_config: false,
                    pair: crate::version_pair_view::VersionPairViewState::default(),
                });
            }
            After::StartExport => self.start_send_package_export(),
            After::OpenFile(path) => self.open_pdf(path, None),
            After::Reload => {
                // 清单或引用关系变了，稿件库详情卡里的送批材料 / 被引用也要跟着刷新。
                self.reload_detail();
                if let (Some(panel), Some(store)) =
                    (self.send_package.as_mut(), self.manuscript_store.as_ref())
                {
                    panel.reload(store);
                }
            }
            After::Nothing => {}
        }
    }
}

enum After {
    Close,
    Open(i64),
    Diff {
        manuscript_id: i64,
        from: i64,
        to: i64,
    },
    StartExport,
    OpenFile(PathBuf),
    Reload,
    Nothing,
}

fn action_kind_after(action: &PanelAction) -> After {
    match action {
        PanelAction::Close => After::Close,
        PanelAction::Open(id) => After::Open(*id),
        PanelAction::Diff {
            manuscript_id,
            from,
            to,
        } => After::Diff {
            manuscript_id: *manuscript_id,
            from: *from,
            to: *to,
        },
        PanelAction::StartExport => After::StartExport,
        PanelAction::OpenFile(path) => After::OpenFile(path.clone()),
        PanelAction::Refresh
        | PanelAction::UndoSort
        | PanelAction::Move { .. }
        | PanelAction::Remove(_)
        | PanelAction::Add(_) => After::Reload,
        PanelAction::TogglePicker
        | PanelAction::PlanExport
        | PanelAction::CancelExport
        | PanelAction::OpenCommit { .. }
        | PanelAction::CancelCommit
        | PanelAction::Commit { .. }
        | PanelAction::CommitAll => After::Nothing,
    }
}

#[derive(Clone, Copy)]
enum RowRole {
    Owner,
    Item { index: usize, editable: bool },
}

/// 只把随行件放进拖放列表，主件固定在前；松手后才通过原有事务保存顺序。
fn items_ui(
    ui: &mut egui::Ui,
    owner_id: i64,
    items: &[PackageRow],
    editable: bool,
    unsaved_in_editor: &dyn Fn(i64) -> bool,
    form: &mut Option<CommitForm>,
    action: &mut Option<PanelAction>,
) -> bool {
    let count = items.len();
    let id = ("send_package_sort", owner_id);
    // 换一份拖放状态即可取消手势；松开鼠标前不允许重新触发拖动。
    let cancel_id = egui::Id::new((id, "cancel"));
    let cancel = ui.input(|input| input.key_pressed(egui::Key::Escape));
    let pressed = ui.input(|input| input.pointer.any_down());
    let (generation, cancelled) = ui.data_mut(|data| {
        let state = data.get_temp_mut_or_default::<(u64, bool)>(cancel_id);
        if cancel {
            state.0 = state.0.wrapping_add(1);
            state.1 = true;
        }
        let cancelled = state.1;
        if !pressed {
            state.1 = false;
        }
        (state.0, cancelled)
    });
    if !editable || count < 2 || cancelled {
        for (index, row) in items.iter().enumerate() {
            row_card(
                ui,
                row,
                RowRole::Item { index, editable },
                None,
                unsaved_in_editor,
                form,
                action,
            );
        }
        return cancelled && pressed;
    }
    let top = ui.cursor().top();
    let response = egui_dnd::dnd(ui, (id, generation))
        .with_touch_config(Some(egui_dnd::DragDropConfig::touch_scroll()))
        .show(items.iter(), |ui, row, handle, state| {
            row_card(
                ui,
                row,
                RowRole::Item {
                    index: state.index,
                    editable,
                },
                Some(handle),
                unsaved_in_editor,
                form,
                action,
            );
        });
    if let Some(update) = response.final_update()
        && ui
            .input(|input| input.pointer.interact_pos())
            .is_some_and(|pos| {
                ui.clip_rect().contains(pos) && pos.y >= top && pos.y <= ui.cursor().top()
            })
    {
        // 库返回原清单中的插入缝隙（末尾可等于件数）；原有 Move 使用移除后的索引。
        let to = if update.to > update.from {
            update.to - 1
        } else {
            update.to
        };
        if update.from != to {
            *action = Some(PanelAction::Move {
                from: update.from,
                to,
            });
        }
    }
    response.is_dragging() || response.is_evaluating_drag()
}

/// 一行的卡片：第一行只有序号与标题（主角），第二行是文种、状态等弱化的元信息，
/// 需要处理的事用一枚徽标加一个按钮说清，不再整句红字。
fn row_card(
    ui: &mut egui::Ui,
    row: &PackageRow,
    role: RowRole,
    handle: Option<egui_dnd::Handle<'_>>,
    unsaved_in_editor: &dyn Fn(i64) -> bool,
    form: &mut Option<CommitForm>,
    action: &mut Option<PanelAction>,
) {
    let frame = match role {
        RowRole::Owner => theme::card().fill(theme::accent_soft()),
        RowRole::Item { .. } => theme::card(),
    };
    let hover_id = egui::Id::new(("send_package_card_hover", &row.item_uuid));
    // 悬停状态取上一帧的：卡片高度本帧才定，指针移动自会触发下一帧重绘。
    let hovered = ui
        .data(|data| data.get_temp::<bool>(hover_id))
        .unwrap_or(false);
    let card = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            if let Some(handle) = handle {
                handle.ui(ui, |ui| {
                    ui.add(theme::Icon::Grip.image().tint(theme::text_soft()))
                        .on_hover_text("按住拖动调整顺序，Esc 取消");
                });
            }
            match role {
                RowRole::Owner => {
                    theme::chip(ui, "主件", theme::accent(), theme::surface());
                }
                RowRole::Item { index, .. } => {
                    ui.strong(format!("{}.", index + 1));
                }
            }
            // 右侧按钮平时不画，但位置要留着，标题才不会在悬停时被顶掉。
            let reserve = match role {
                RowRole::Item { editable: true, .. } => 80.0,
                _ => 40.0,
            };
            match &row.brief {
                Some(brief) => {
                    ui.scope(|ui| {
                        ui.set_max_width((ui.available_width() - reserve).max(60.0));
                        ui.add(
                            egui::Label::new(egui::RichText::new(&brief.title).strong().size(15.5))
                                .truncate(),
                        )
                        .on_hover_text(&brief.title);
                    });
                }
                None => {
                    ui.colored_label(theme::warn(), "本机未找到该稿件");
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !hovered {
                    return;
                }
                if matches!(role, RowRole::Item { editable: true, .. })
                    && theme::danger_icon_button(ui, theme::Icon::Trash, "移出清单（不删除稿件）")
                        .clicked()
                {
                    *action = Some(PanelAction::Remove(row.item_uuid.clone()));
                }
                if let Some(brief) = &row.brief
                    && theme::icon_button(ui, theme::Icon::Open, "在公文标签中打开").clicked()
                {
                    *action = Some(PanelAction::Open(brief.id));
                }
            });
        });

        let (Some(brief), Some(state)) = (&row.brief, &row.state) else {
            ui.weak("关联照旧保留：从同步包导入该稿件后自动恢复；不再需要可以移除。");
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.weak(brief.kind.label());
            ui.weak("·");
            ui.colored_label(status_color(brief.status), brief.status.label());
            // 归档后钉住了版本：导出只用这一版，之后的提交与未提交修改都与送批材料无关。
            if let Some(pinned) = row.pinned {
                match pinned {
                    Some(Some(number)) => {
                        theme::chip(
                            ui,
                            &format!("钉住 v{number}"),
                            theme::success(),
                            theme::success_soft(),
                        );
                    }
                    Some(None) => {
                        theme::chip(
                            ui,
                            "钉住导出时的版本",
                            theme::success(),
                            theme::success_soft(),
                        );
                    }
                    None => {
                        theme::chip(
                            ui,
                            "钉住的版本在本机找不到",
                            theme::danger(),
                            theme::danger_soft(),
                        );
                    }
                }
                ui.weak("归档时固定");
                return;
            }
            let can_commit = brief.status != ManuscriptStatus::Archived;
            let commit_button =
                |ui: &mut egui::Ui, primary: bool, action: &mut Option<PanelAction>| {
                    let clicked = if primary {
                        theme::primary_icon_button(ui, theme::Icon::GitCommit, "提交版本").clicked()
                    } else {
                        ui.add(theme::icon_text_button(
                            theme::Icon::GitCommit,
                            "提交新版本",
                        ))
                        .clicked()
                    };
                    if clicked {
                        *action = Some(PanelAction::OpenCommit {
                            manuscript_id: brief.id,
                            name: default_version_name(state),
                        });
                    }
                };
            match &state.latest {
                Some(latest) => {
                    let mut line = format!("v{}", latest.visible_number);
                    if !latest.name.trim().is_empty() {
                        line.push_str(&format!("「{}」", latest.name.trim()));
                    }
                    line.push_str(&format!(" · {}", short_date(&latest.created_at)));
                    if !latest.comment.trim().is_empty() {
                        line.push_str(&format!(" · {}", summarize(&latest.comment, 40)));
                    }
                    ui.weak("·");
                    ui.weak(line);
                    if let Some(exported) = row.last_exported
                        && exported != latest.visible_number
                    {
                        theme::chip(ui, "导出后有新版本", theme::warn(), theme::warn_soft())
                            .on_hover_text(format!("上次导出用的是 v{exported}"));
                        if ui
                            .small_button(format!("对照 v{exported} → v{}", latest.visible_number))
                            .on_hover_text("用版本对照（花脸稿）看这期间改了什么")
                            .clicked()
                        {
                            *action = Some(PanelAction::Diff {
                                manuscript_id: brief.id,
                                from: exported,
                                to: latest.visible_number,
                            });
                        }
                    }
                    if state.has_uncommitted || unsaved_in_editor(brief.id) {
                        theme::chip(ui, "有未提交修改", theme::warn(), theme::warn_soft())
                            .on_hover_text(format!(
                                "这些修改不会进包，导出将使用 v{}",
                                latest.visible_number
                            ));
                        if can_commit {
                            commit_button(ui, false, action);
                        }
                    }
                }
                None => {
                    theme::chip(
                        ui,
                        "未提交 · 不能进包",
                        theme::danger(),
                        theme::danger_soft(),
                    );
                    if can_commit {
                        commit_button(ui, true, action);
                    }
                }
            }
            if state.has_pending_branch {
                theme::chip(ui, "有待处理同步分支", theme::warn(), theme::warn_soft())
                    .on_hover_text("导出用的是本机这一支");
            }
        });
        if let Some(open) = form.as_mut().filter(|f| f.manuscript_id == brief.id) {
            commit_form_ui(ui, open, action);
        }
    });
    let now = ui.rect_contains_pointer(card.response.rect);
    ui.data_mut(|data| data.insert_temp(hover_id, now));
    if now != hovered {
        ui.ctx().request_repaint();
    }
    ui.add_space(4.0);
}

/// 卡片里的行内提交表单：版本名、备注，回车即提交。
fn commit_form_ui(ui: &mut egui::Ui, form: &mut CommitForm, action: &mut Option<PanelAction>) {
    ui.add_space(2.0);
    let mut submit = false;
    ui.horizontal_wrapped(|ui| {
        ui.label("版本名");
        let name = ui.add(theme::field(&mut form.name, "必填", 110.0));
        if form.focus {
            name.request_focus();
            form.focus = false;
        }
        ui.label("备注");
        let comment = ui.add(theme::field(&mut form.comment, "可选", 180.0));
        let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
        if (name.lost_focus() || comment.lost_focus()) && enter {
            submit = true;
        }
        if ui
            .add_enabled(
                !form.name.trim().is_empty(),
                theme::primary_button_widget(theme::Icon::GitCommit, "提交"),
            )
            .clicked()
        {
            submit = true;
        }
        if ui.button("取消").clicked() {
            *action = Some(PanelAction::CancelCommit);
        }
    });
    if submit && !form.name.trim().is_empty() {
        *action = Some(PanelAction::Commit {
            manuscript_id: form.manuscript_id,
            name: form.name.clone(),
            comment: form.comment.clone(),
        });
    }
    if let Some(error) = &form.error {
        ui.colored_label(theme::danger(), error);
    }
}

/// 候选条件变了就重查。每条候选附上能否加入的判定，不能加的置灰并说明原因。
fn refresh_picker(picker: &mut Picker, store: &mut ManuscriptStore, owner_id: i64) {
    let wanted = (picker.keyword.trim().to_string(), picker.kind);
    if picker.applied.as_ref() == Some(&wanted) {
        return;
    }
    let filter = ManuscriptFilter {
        keyword: wanted.0.clone(),
        kind: wanted.1,
        ..Default::default()
    };
    let rows = store.list(&filter).unwrap_or_default();
    picker.truncated = rows.len() > PICKER_LIMIT;
    picker.rows = rows
        .into_iter()
        .filter(|row| row.id != owner_id)
        .take(PICKER_LIMIT)
        .map(|row| {
            let block = store
                .send_package_add_block(owner_id, row.id)
                .unwrap_or(Some(AddBlock::CandidateMissing));
            (row, block)
        })
        .collect();
    picker.applied = Some(wanted);
}

fn picker_ui(ui: &mut egui::Ui, picker: &mut Picker, action: &mut Option<PanelAction>) {
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut picker.keyword)
                    .hint_text("标题、文号或备注")
                    .desired_width(220.0),
            );
            egui::ComboBox::from_id_salt("send_package_picker_kind")
                .selected_text(picker.kind.map(|kind| kind.label()).unwrap_or("全部文种"))
                .width(140.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut picker.kind, None, "全部文种");
                    for kind in TemplateKind::ALL {
                        ui.selectable_value(&mut picker.kind, Some(kind), kind.label());
                    }
                });
        });
        ui.add_space(4.0);
        if picker.rows.is_empty() {
            ui.weak("没有符合条件的稿件。");
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("send_package_picker_rows")
            .max_height(240.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (row, block) in &picker.rows {
                    ui.horizontal(|ui| {
                        let added = ui
                            .add_enabled_ui(block.is_none(), |ui| ui.small_button("添加"))
                            .inner;
                        let added = match block {
                            Some(block) => added.on_disabled_hover_text(block.reason()),
                            None => added,
                        };
                        if added.clicked() {
                            *action = Some(PanelAction::Add(row.id));
                        }
                        ui.weak(row.kind.label());
                        let title = if row.title.trim().is_empty() {
                            "（无标题）"
                        } else {
                            row.title.as_str()
                        };
                        ui.add(egui::Label::new(title).truncate())
                            .on_hover_text(format!(
                                "{title}\n{} · {}",
                                row.status.label(),
                                short_date(&row.updated_at)
                            ));
                    });
                }
                if picker.truncated {
                    ui.weak(format!("只列出前 {PICKER_LIMIT} 篇，请用关键词缩小范围。"));
                }
            });
    });
}

/// 可换行的浅色说明文字。
fn wrapped_soft(ui: &mut egui::Ui, text: &str) {
    ui.add(
        egui::Label::new(egui::RichText::new(text).color(theme::text_soft()))
            .wrap_mode(egui::TextWrapMode::Wrap),
    );
}

/// 导出前的确认清单：每件用哪一版、有什么要注意、哪件不能进包。
fn confirm_ui(
    ui: &mut egui::Ui,
    confirm: &mut ExportConfirm,
    unsaved_in_editor: &dyn Fn(i64) -> bool,
    action: &mut Option<PanelAction>,
) {
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.strong("导出前确认");
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("send_package_confirm_rows")
            .max_height(CONFIRM_MAX_HEIGHT)
            .auto_shrink([false, true])
            .show(ui, |ui| confirm_rows(ui, &confirm.plan, unsaved_in_editor));
        ui.add_space(6.0);
        ui.checkbox(&mut confirm.with_toc, "前面加一页送批材料目录");
        wrapped_soft(
            ui,
            "每件从奇数页开始：页数为奇数的件后面自动补一张空白页，双面打印后各件能拆开分别装订。各件页码保持原样。",
        );
        let ready = confirm.plan.ready();
        if !ready {
            ui.colored_label(
                theme::danger(),
                "有不能进包的件：处理好后点「重新检查」，或把它移出清单。",
            );
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            // 置灰的主按钮文字几乎看不清，不能导出时退回普通按钮样式。
            let export = if ready {
                ui.add(theme::primary_button_widget(
                    theme::Icon::FileDown,
                    "选择位置并导出",
                ))
            } else {
                ui.add_enabled(
                    false,
                    theme::icon_text_button(theme::Icon::FileDown, "选择位置并导出"),
                )
            };
            if export.clicked() {
                *action = Some(PanelAction::StartExport);
            }
            if ui
                .add(theme::icon_text_button(theme::Icon::Refresh, "重新检查"))
                .on_hover_text("重新读取各件的最新提交版")
                .clicked()
            {
                *action = Some(PanelAction::PlanExport);
            }
            if ui.button("取消").clicked() {
                *action = Some(PanelAction::CancelExport);
            }
        });
    });
}

/// 确认清单的逐件行。
fn confirm_rows(
    ui: &mut egui::Ui,
    plan: &SendPackagePlan,
    unsaved_in_editor: &dyn Fn(i64) -> bool,
) {
    for (index, entry) in plan.entries.iter().enumerate() {
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("{}.", index + 1));
            if index == 0 {
                theme::chip(ui, "主件", theme::accent(), theme::surface());
            }
            let title = if entry.title.is_empty() {
                "（本机未找到的稿件）"
            } else {
                entry.title.as_str()
            };
            ui.add(egui::Label::new(title).truncate())
                .on_hover_text(title);
            match &entry.revision {
                Some(revision) => {
                    let version = match revision.visible_number {
                        Some(number) => format!("v{number}"),
                        None => "钉住的版本".into(),
                    };
                    let label = if entry.pinned {
                        format!("{version}（归档时钉住）")
                    } else {
                        version
                    };
                    theme::chip(ui, &label, theme::success(), theme::success_soft());
                }
                None => {
                    theme::chip(ui, "不能进包", theme::danger(), theme::danger_soft());
                }
            }
        });
        if let Some(blocker) = entry.blocker {
            ui.colored_label(theme::danger(), format!("    {}", blocker.reason()));
        }
        let unsaved = entry.manuscript_id.is_some_and(unsaved_in_editor);
        if entry.revision.is_some() && (entry.has_uncommitted || unsaved) {
            ui.colored_label(theme::warn(), "    有未提交的修改，这些修改不会进包。");
        }
        if entry.has_pending_branch {
            ui.colored_label(theme::warn(), "    有待处理的同步分支，用的是本机这一支。");
        }
    }
}

/// 历次导出记录：时间、页数、文件，展开看当时每件的版本与页数。
fn history_ui(ui: &mut egui::Ui, exports: &[ExportRecord], action: &mut Option<PanelAction>) {
    if exports.is_empty() {
        return;
    }
    ui.add_space(6.0);
    egui::CollapsingHeader::new(format!("导出记录（{}）", exports.len()))
        .id_salt("send_package_history")
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("send_package_history_rows")
                .max_height(200.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for record in exports {
                        let path = PathBuf::from(&record.output_path);
                        let name = path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| record.output_path.clone());
                        let toc = if record.with_toc { " · 含目录" } else { "" };
                        egui::CollapsingHeader::new(format!(
                            "{} · {} 页{toc} · {name}",
                            short_date(&record.exported_at),
                            record.total_pages
                        ))
                        .id_salt(("send_package_export", record.id))
                        .show(ui, |ui| {
                            for item in &record.items {
                                let version = item
                                    .visible_number
                                    .map_or_else(|| "钉住的版本".into(), |n| format!("v{n}"));
                                ui.label(format!(
                                    "{}. {} · {version} · {} 页",
                                    item.sort_order + 1,
                                    summarize(&item.title, 30),
                                    item.page_count
                                ));
                            }
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&record.output_path).weak(),
                                    )
                                    .truncate(),
                                );
                                if path.exists() {
                                    if ui.small_button("打开").clicked() {
                                        *action = Some(PanelAction::OpenFile(path.clone()));
                                    }
                                } else {
                                    ui.weak("（文件已不在）");
                                }
                            });
                        });
                    }
                });
        });
}

impl GongwenApp {
    /// 确认后选保存位置，把计划交给后台线程编译合并。
    fn start_send_package_export(&mut self) {
        if self.send_package_export.is_some() {
            self.status = "已有送批材料正在导出，请稍候。".into();
            return;
        }
        let Some(panel) = self.send_package.as_mut() else {
            return;
        };
        let Some(confirm) = panel.confirm.as_ref() else {
            return;
        };
        if !confirm.plan.ready() {
            return;
        }
        let default_name = format!(
            "{}（送批材料）.pdf",
            crate::export::safe_filename(confirm.plan.owner_title())
        );
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PDF", &["pdf"])
            .set_file_name(&default_name)
            .save_file()
        else {
            return;
        };
        let path = if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
        {
            path
        } else {
            path.with_extension("pdf")
        };
        self.run_send_package_export(path);
    }

    /// 把面板上已确认的计划交给后台线程，导出到 `path`。
    fn run_send_package_export(&mut self, path: PathBuf) {
        let Some(panel) = self.send_package.as_mut() else {
            return;
        };
        let Some(confirm) = panel.confirm.take() else {
            return;
        };
        let owner_id = panel.owner_id;
        let with_toc = confirm.with_toc;
        let plan = confirm.plan;
        let vocabulary = self.config.vocabulary.clone();
        let fonts = self.config.fonts.clone();
        let numbering = self.config.numbering;
        let tx = self.sender.clone();
        self.send_package_export = Some(SendPackageExportJob {
            owner_id,
            progress: "正在准备导出…".into(),
        });
        self.status = format!("正在导出《{}》的送批材料…", plan.owner_title());
        std::thread::spawn(move || {
            let display = crate::units::UnitDisplay::new(&vocabulary);
            let (fonts, _warnings) = crate::system_fonts::resolve(&fonts);
            let progress_tx = tx.clone();
            let result = crate::manuscript_io::send_package::export_send_package(
                &plan,
                with_toc,
                &path,
                |snapshot, markdown, stem| {
                    crate::manuscript_io::compile_snapshot_pdf(
                        snapshot, markdown, &display, &fonts, &numbering, stem,
                    )
                },
                |text| {
                    let _ =
                        progress_tx.send(WorkerResult::SendPackage(SendPackageEvent::Progress {
                            text: text.to_string(),
                        }));
                },
            );
            let _ = tx.send(WorkerResult::SendPackage(SendPackageEvent::Done {
                owner_id,
                with_toc,
                result: result.map_err(|error| format!("{error:#}")),
            }));
        });
    }

    /// 后台导出线程的消息：更新进度，或收尾——写导出记录、在应用内打开成品。
    pub(crate) fn handle_send_package_event(&mut self, event: SendPackageEvent) {
        match event {
            SendPackageEvent::Progress { text } => {
                if let Some(job) = self.send_package_export.as_mut() {
                    job.progress = text.clone();
                }
                self.status = text;
            }
            SendPackageEvent::Done {
                owner_id,
                with_toc,
                result,
            } => {
                self.send_package_export = None;
                match result {
                    Ok(outcome) => {
                        let blanks: i64 = outcome
                            .items
                            .iter()
                            .map(|item| item.blank_pages)
                            .sum::<i64>()
                            + outcome.toc.map_or(0, |toc| toc.blanks as i64);
                        let recorded = self.manuscript_store.as_mut().map(|store| {
                            store.record_send_package_export(
                                owner_id,
                                &outcome.path.to_string_lossy(),
                                with_toc,
                                outcome.total_pages as i64,
                                &outcome.items,
                            )
                        });
                        let title = outcome
                            .items
                            .first()
                            .map(|item| item.title.clone())
                            .unwrap_or_default();
                        let mut message = format!(
                            "已导出《{title}》的送批材料：{} 件，共 {} 页（含 {blanks} 张双面打印补白页），{}。",
                            outcome.items.len(),
                            outcome.total_pages,
                            outcome.path.display()
                        );
                        if let Some(Err(error)) = recorded {
                            message.push_str(&format!("但导出记录没写进去：{error:#}"));
                        }
                        self.status = message;
                        if let (Some(panel), Some(store)) =
                            (self.send_package.as_mut(), self.manuscript_store.as_ref())
                            && panel.owner_id == owner_id
                        {
                            panel.reload(store);
                        }
                        // 应用内的 PDF 标签自带打印、系统打开与定位。
                        self.open_pdf(outcome.path, Some(format!("{title}（送批材料）")));
                    }
                    Err(error) => {
                        self.status = format!("送批材料导出失败，没有生成文件：{error}");
                        if let Some(panel) = self.send_package.as_mut()
                            && panel.owner_id == owner_id
                        {
                            panel.export_error = Some(error);
                        }
                    }
                }
            }
        }
    }

    /// 送批材料正在后台导出。
    pub(crate) fn send_package_exporting(&self) -> bool {
        self.send_package_export.is_some()
    }
}
