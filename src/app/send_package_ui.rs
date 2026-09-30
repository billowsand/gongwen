//! 送批材料面板：呈批件挂随行件的清单、各件版本状态与增删排序。
//!
//! 数据层在 `manuscript::send_package`，方案见 `docs/send-package-design.md`。
//! 面板只负责组织：不碰任何件的正文与行文要素。各件的版本状态查库得来，
//! 每隔几秒自动重查一次——别的标签里提交了版本，这里不用手动刷新也能跟上。

use crate::app::{GongwenApp, short_date, status_color, summarize};
use crate::manuscript::send_package::{AddBlock, ManuscriptBrief, PackageItemState};
use crate::manuscript::{ManuscriptFilter, ManuscriptRow, ManuscriptStore};
use crate::models::{ManuscriptStatus, TemplateKind};
use crate::theme;
use eframe::egui;
use std::time::{Duration, Instant};

/// 面板状态自动重查的间隔。
const REFRESH_INTERVAL: Duration = Duration::from_secs(3);
/// 清单区最高这么高，再多就滚动；展开候选区时让出一半，窗口别顶出屏幕。
const ROWS_MAX_HEIGHT: f32 = 460.0;
const ROWS_MAX_HEIGHT_PICKING: f32 = 240.0;
/// 添加稿件的候选列表最多列这么多条，再多请用关键词缩小范围。
const PICKER_LIMIT: usize = 200;

/// 面板里的一行：主件或随行件。
struct PackageRow {
    /// 随行件的稿件 UUID；主件行为空。
    item_uuid: String,
    /// 本机找不到的随行件为 `None`。
    brief: Option<ManuscriptBrief>,
    state: Option<PackageItemState>,
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
            let owner = load_row(store, String::new(), Some(self.owner_id))?;
            let mut items = Vec::new();
            for item in store.send_package_items(self.owner_id)? {
                items.push(load_row(store, item.item_uuid, item.manuscript_id)?);
            }
            self.owner = Some(owner);
            self.items = items;
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
    })
}

/// 面板里点出的动作，帧末统一执行。
enum PanelAction {
    Refresh,
    Move { from: usize, to: usize },
    Remove(String),
    Open(i64),
    Add(i64),
    TogglePicker,
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
                ui.add(
                        egui::Label::new(
                            egui::RichText::new(
                                "随呈批件一起送批的独立稿件（函稿、普通公文、研究报告等）。导出时每件取最新提交版，按下面的顺序合并。",
                            )
                            .color(theme::text_soft()),
                        )
                        .wrap_mode(egui::TextWrapMode::Wrap),
                    );
                if let Some(error) = &panel.error {
                    ui.colored_label(theme::danger(), format!("读取失败：{error}"));
                }
                ui.add_space(6.0);

                egui::ScrollArea::vertical()
                    .id_salt("send_package_rows")
                    .auto_shrink([false, true])
                    .max_height(if panel.picker.is_some() {
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
                let picking = panel.picker.is_some();
                ui.horizontal(|ui| {
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
                if archived {
                    return;
                }
                if let Some(picker) = &mut panel.picker {
                    picker_ui(ui, picker, &mut action);
                }
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
                PanelAction::Open(_) | PanelAction::Close => Ok(None),
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
    Reload,
    Nothing,
}

fn action_kind_after(action: &PanelAction) -> After {
    match action {
        PanelAction::Close => After::Close,
        PanelAction::Open(id) => After::Open(*id),
        PanelAction::Refresh
        | PanelAction::Move { .. }
        | PanelAction::Remove(_)
        | PanelAction::Add(_) => After::Reload,
        PanelAction::TogglePicker => After::Nothing,
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
