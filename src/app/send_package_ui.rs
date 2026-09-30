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
use crate::manuscript::{ManuscriptFilter, ManuscriptRow, ManuscriptStore};
use crate::manuscript_io::send_package::SendPackageOutcome;
use crate::models::{ManuscriptStatus, TemplateKind};
use crate::theme;
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

/// 面板里的一行：主件或随行件。
struct PackageRow {
    /// 随行件的稿件 UUID；主件行为空。
    item_uuid: String,
    /// 本机找不到的随行件为 `None`。
    brief: Option<ManuscriptBrief>,
    state: Option<PackageItemState>,
    /// 最近一次导出里这一件用的版本号；没导出过或当时不在清单里为 `None`。
    last_exported: Option<i64>,
}

/// 导出前的确认：计划已生成，等人勾选目录页、选保存位置。
struct ExportConfirm {
    plan: SendPackagePlan,
    with_toc: bool,
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
        }
    }

    fn archived(&self) -> bool {
        self.owner
            .as_ref()
            .and_then(|row| row.brief.as_ref())
            .is_some_and(|brief| brief.status == ManuscriptStatus::Archived)
    }

    /// 从库里重读主件与清单。出错时保留上次的内容，只记下错误。
    fn reload(&mut self, store: &ManuscriptStore) {
        let result = (|| -> anyhow::Result<()> {
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
            let mut owner = load_row(store, String::new(), Some(self.owner_id))?;
            owner.last_exported = exported(&owner_uuid);
            let mut items = Vec::new();
            for item in store.send_package_items(self.owner_id)? {
                let mut row = load_row(store, item.item_uuid, item.manuscript_id)?;
                row.last_exported = exported(&row.item_uuid);
                items.push(row);
            }
            self.owner = Some(owner);
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
    })
}

/// 面板里点出的动作，帧末统一执行。
enum PanelAction {
    Refresh,
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
        if panel
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
                if archived {
                    theme::chip(ui, "已归档 · 只读", theme::text_muted(), theme::surface());
                }
                wrapped_soft(
                    ui,
                    "随呈批件一起送批的独立稿件（函稿、普通公文、研究报告等）。导出时每件取最新提交版，按下面的顺序合并成一个 PDF。",
                );
                if let Some(error) = &panel.error {
                    ui.colored_label(theme::danger(), format!("读取失败：{error}"));
                }
                ui.add_space(6.0);

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
                            row_card(ui, owner, RowRole::Owner, &unsaved_in_editor, &mut action);
                        }
                        if panel.items.is_empty() {
                            ui.add_space(4.0);
                            ui.weak("还没有送批材料。");
                        }
                        let count = panel.items.len();
                        for (index, row) in panel.items.iter().enumerate() {
                            let role = RowRole::Item {
                                index,
                                count,
                                editable: !archived,
                            };
                            row_card(ui, row, role, &unsaved_in_editor, &mut action);
                        }
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

    fn apply_send_package_action(&mut self, action: PanelAction) {
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
                PanelAction::Move { from, to } => {
                    let mut order: Vec<String> = panel
                        .items
                        .iter()
                        .map(|row| row.item_uuid.clone())
                        .collect();
                    if from >= order.len() || to >= order.len() {
                        return Ok(None);
                    }
                    let moved = order.remove(from);
                    order.insert(to, moved);
                    store.reorder_send_package(owner_id, &order)?;
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
        | PanelAction::Move { .. }
        | PanelAction::Remove(_)
        | PanelAction::Add(_) => After::Reload,
        PanelAction::TogglePicker | PanelAction::PlanExport | PanelAction::CancelExport => {
            After::Nothing
        }
    }
}

#[derive(Clone, Copy)]
enum RowRole {
    Owner,
    Item {
        index: usize,
        count: usize,
        editable: bool,
    },
}

/// 一行的卡片：文种、标题、最新提交版，以及需要注意的状态。
fn row_card(
    ui: &mut egui::Ui,
    row: &PackageRow,
    role: RowRole,
    unsaved_in_editor: &dyn Fn(i64) -> bool,
    action: &mut Option<PanelAction>,
) {
    let frame = match role {
        RowRole::Owner => theme::card().fill(theme::accent_soft()),
        RowRole::Item { .. } => theme::card(),
    };
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            match role {
                RowRole::Owner => {
                    theme::chip(ui, "主件", theme::accent(), theme::surface());
                }
                RowRole::Item { index, .. } => {
                    ui.strong(format!("{}.", index + 1));
                }
            }
            match &row.brief {
                Some(brief) => {
                    theme::chip(ui, brief.kind.label(), theme::text_soft(), theme::surface());
                    ui.add(egui::Label::new(egui::RichText::new(&brief.title).strong()).truncate())
                        .on_hover_text(&brief.title);
                }
                None => {
                    ui.colored_label(theme::warn(), "本机未找到该稿件");
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let RowRole::Item {
                    index,
                    count,
                    editable: true,
                } = role
                {
                    if theme::danger_icon_button(ui, theme::Icon::Trash, "移出清单（不删除稿件）")
                        .clicked()
                    {
                        *action = Some(PanelAction::Remove(row.item_uuid.clone()));
                    }
                    if ui
                        .add_enabled_ui(index + 1 < count, |ui| {
                            theme::icon_button(ui, theme::Icon::ArrowDown, "下移")
                        })
                        .inner
                        .clicked()
                    {
                        *action = Some(PanelAction::Move {
                            from: index,
                            to: index + 1,
                        });
                    }
                    if ui
                        .add_enabled_ui(index > 0, |ui| {
                            theme::icon_button(ui, theme::Icon::ArrowUp, "上移")
                        })
                        .inner
                        .clicked()
                    {
                        *action = Some(PanelAction::Move {
                            from: index,
                            to: index - 1,
                        });
                    }
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
                ui.horizontal_wrapped(|ui| {
                    ui.weak(line);
                    ui.colored_label(status_color(brief.status), brief.status.label());
                });
                if let Some(exported) = row.last_exported
                    && exported != latest.visible_number
                {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(
                            theme::warn(),
                            format!("上次导出用的是 v{exported}，之后又提交了新版本。"),
                        );
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
                    });
                }
                if state.has_uncommitted || unsaved_in_editor(brief.id) {
                    ui.colored_label(
                        theme::danger(),
                        format!(
                            "有未提交的修改，导出将使用 v{}；需要的话先提交版本。",
                            latest.visible_number
                        ),
                    );
                }
            }
            None => {
                ui.horizontal_wrapped(|ui| {
                    ui.weak("尚无提交版本");
                    ui.colored_label(status_color(brief.status), brief.status.label());
                });
                ui.colored_label(
                    theme::danger(),
                    "从未提交过版本，不能进包：请先提交一个版本。",
                );
            }
        }
        if state.has_pending_branch {
            ui.colored_label(theme::warn(), "有待处理的同步分支：导出用的是本机这一支。");
        }
    });
    ui.add_space(4.0);
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
