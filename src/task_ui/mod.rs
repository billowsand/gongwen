//! 任务助手页面：顶栏 + 视图栏 / 列表 / 详情三栏，各栏自己滚动。
//!
//! 布局、取舍与分期见 `docs/task-assistant-ui-redesign.md`。页面只读写任务数据，
//! 不接触公文正文；数据库读取经 `cache` 按数据代号缓存，代号不变的帧不查库。
use crate::task_assistant::{Store, Task};
use crate::theme;
use std::collections::{HashMap, HashSet};

mod cache;
mod categories;
mod date;
mod detail;
mod fill;
mod form;
mod inbox;
mod list;
mod materials;
mod nav;
mod owner;

use crate::task_assistant::extract::FilledValue;
#[cfg(test)]
pub(crate) use fill::apply_fill;
pub(crate) use fill::{FillField, FillRequest, receive_fill};
pub(crate) use inbox::{ExtractionOutcome, receive_extraction};
pub(crate) use nav::View;

/// 视图栏固定宽度；窗口窄于 `NAV_MIN_PAGE` 时视图栏收成列表上方的下拉框。
const NAV_WIDTH: f32 = 168.0;
const NAV_MIN_PAGE: f32 = 900.0;
/// 窄于这个宽度时列表与详情不再并排，改为单栏切换。
const SPLIT_MIN_PAGE: f32 = 650.0;
/// 提示条停留的秒数；错误不自动消失，要么被下一次成功操作替换，要么手动关掉。
const NOTICE_SECONDS: f64 = 5.0;

/// 有未保存表单时，用户又点了「新建任务」或「编辑为新任务」：先问一句再替换。
#[derive(Clone)]
pub(super) enum Replace {
    Blank,
    Candidate(String, Box<Task>),
    Edit(Box<Task>),
}

#[derive(Default)]
pub(crate) struct State {
    pub selected: Option<String>,
    pub db_error: Option<String>,
    pub draft: Option<Task>,
    pub query: String,
    pub category: String,
    pub view: View,
    pub owner_filter: String,
    pub owner_query: String,
    pub sort_deadline: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
    notice_since: Option<(String, f64)>,
    pub progress_text: String,
    /// 「更新进展」框对应的任务与预填时的进展。
    progress_task: Option<(String, String)>,
    pub candidate_targets: HashMap<String, String>,
    pub candidate_edits: HashMap<String, crate::task_assistant::extract::Proposal>,
    /// 候选卡里负责人搜索框的文字，按候选 id 分开。
    candidate_owner_queries: HashMap<String, String>,
    /// 采纳时报过「过期 / 已变化」的候选；只有它们显示「以当前版本重新核对」。
    expired_candidates: HashSet<String>,
    pub delete_confirm: Option<String>,
    pub candidate_create_id: Option<String>,
    pub cancel_reason: String,
    /// 详情里状态下拉选了「已取消」、正等待填写理由的任务。
    status_cancel: Option<String>,
    /// 状态切换失败的提示，显示在该任务的状态下拉旁边。
    status_error: Option<(String, String)>,
    form_error: Option<String>,
    prerequisite_query: String,
    replace_draft: Option<Replace>,
    source_view: Option<(String, String)>,
    categories_open: bool,
    category_rename: Option<(String, String)>,
    category_delete: Option<(String, String)>,
    pub new_category: String,
    pub source_title: String,
    pub source_text: String,
    /// 标题留空时程序取首行填进去的那个值；原文再改时若标题没被人动过就一起清掉。
    source_title_auto: Option<String>,
    /// 最近一次材料识别的结果，写在材料卡「识别」按钮旁边。
    pub extraction_result: Option<String>,
    /// 上面那条结果对应的材料（有新建议时），结果旁的「只看这批」按它筛选。
    extraction_source: Option<String>,
    /// 刚粘贴的材料之前识别过：材料卡里显示「查看 / 重新识别」卡片。
    pub known_source: Option<crate::task_assistant::SourceRow>,
    /// 待应用层开始的重新识别（材料 id）。
    pub request_rerun: Option<String>,
    /// 「待整理建议」只看这份材料的建议。
    pub inbox_source: Option<String>,
    /// 「材料」视图里选中的材料。
    pub material_selected: Option<String>,
    /// 选中材料的原文快照（id、标题、正文）；快照不会改，按 id 缓存一份。
    material_text: Option<(String, String, String)>,
    material_delete: Option<String>,
    focus_material: bool,
    pub knowledge_id: Option<i64>,
    knowledge_docs: Option<Vec<(i64, String)>>,
    pub request_extract: Option<(String, String, String)>,
    pub extraction_busy: bool,
    pub discard_extraction: bool,
    pub request_knowledge: Option<i64>,
    pub history_candidates: bool,
    pub extraction_error: Option<String>,
    pub request_fill: Option<FillRequest>,
    pub fill_text: String,
    pub fill_busy: bool,
    pub fill_discard: bool,
    pub fill_error: Option<String>,
    pub fill_owner_note: Option<String>,
    pub fill_owner_id: Option<u64>,
    pub fill_owner_suggestion_id: Option<u64>,
    pub fill_marks: HashMap<FillField, FilledValue>,
    pub fill_suggestions: HashMap<FillField, FilledValue>,
    pub fill_before: Option<Task>,
    pub fill_source: Option<String>,
    pub fill_summary: Option<(usize, usize)>,
    fill_open_more: bool,
    fill_focus: bool,
    cache: cache::Cache,
}

impl State {
    /// 操作成功：换上新提示，并清掉上一次操作留下的错误。
    fn ok(&mut self, message: impl Into<String>) {
        self.error = None;
        self.notice = Some(message.into());
    }
    fn fail(&mut self, message: impl Into<String>) {
        self.notice = None;
        self.error = Some(message.into());
    }
    /// 打开一张新表单；已有未保存表单时先确认。
    fn open_draft(&mut self, replace: Replace) {
        if self.draft.is_some() {
            self.replace_draft = Some(replace);
            return;
        }
        self.apply_replace(replace);
    }
    fn apply_replace(&mut self, replace: Replace) {
        self.clear_fill();
        self.form_error = None;
        self.prerequisite_query.clear();
        self.owner_query.clear();
        self.cancel_reason.clear();
        self.progress_task = None;
        match replace {
            Replace::Blank => {
                self.fill_focus = true;
                self.candidate_create_id = None;
                self.draft = Some(Task::blank());
                self.selected = None;
            }
            Replace::Candidate(id, task) => {
                self.candidate_create_id = Some(id);
                self.draft = Some(*task);
                self.selected = None;
            }
            Replace::Edit(task) => {
                self.candidate_create_id = None;
                self.draft = Some(*task);
            }
        }
    }
    fn close_draft(&mut self) {
        self.clear_fill();
        self.draft = None;
        self.candidate_create_id = None;
        self.form_error = None;
        self.prerequisite_query.clear();
        self.cancel_reason.clear();
    }
    fn clear_fill(&mut self) {
        self.request_fill = None;
        self.fill_text.clear();
        self.fill_busy = false;
        self.fill_discard = false;
        self.fill_error = None;
        self.fill_owner_note = None;
        self.fill_owner_id = None;
        self.fill_owner_suggestion_id = None;
        self.fill_marks.clear();
        self.fill_suggestions.clear();
        self.fill_before = None;
        self.fill_source = None;
        self.fill_summary = None;
        self.fill_open_more = false;
        self.fill_focus = false;
    }
}

/// 缓存里取出的一帧数据：渲染期间与 `State` 分开借用，帧末放回。
pub(super) struct Snapshot<'a> {
    pub tasks: Vec<Task>,
    pub categories: Vec<String>,
    pub candidates: Vec<crate::task_assistant::CandidateRow>,
    pub sources: Vec<crate::task_assistant::SourceRow>,
    pub today: String,
    /// 借用配置里的标准词库：每帧都要重绘，不复制整份词库。
    pub vocabulary: &'a [crate::models::VocabularyEntry],
}

impl Snapshot<'_> {
    pub fn task(&self, id: &str) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    store: Option<&mut Store>,
    knowledge: Option<&mut crate::knowledge::KnowledgeStore>,
    vocabulary: &[crate::models::VocabularyEntry],
    state: &mut State,
) {
    let Some(store) = store else {
        ui.heading("任务助手");
        theme::notice(
            ui,
            theme::Icon::TriangleAlert,
            theme::warn(),
            theme::warn_soft(),
            state
                .db_error
                .as_deref()
                .unwrap_or("任务数据库暂不可用，请检查稿件库路径与权限。"),
        );
        return;
    };
    if let Err(e) = state.cache.refresh(store) {
        state.error = Some(e);
    }
    let data = Snapshot {
        tasks: std::mem::take(&mut state.cache.tasks),
        categories: std::mem::take(&mut state.cache.categories),
        candidates: std::mem::take(&mut state.cache.candidates),
        sources: std::mem::take(&mut state.cache.sources),
        today: chrono::Local::now()
            .date_naive()
            .format("%Y-%m-%d")
            .to_string(),
        vocabulary,
    };
    page(ui, store, knowledge, state, &data);
    state.cache.tasks = data.tasks;
    state.cache.categories = data.categories;
    state.cache.candidates = data.candidates;
    state.cache.sources = data.sources;
}

fn page(
    ui: &mut egui::Ui,
    store: &mut Store,
    knowledge: Option<&mut crate::knowledge::KnowledgeStore>,
    state: &mut State,
    data: &Snapshot,
) {
    let page_rect = ui.max_rect();
    top_bar(ui, store, state);
    messages(ui, state);
    ui.add_space(6.0);
    let size = ui.available_size();
    let with_nav = size.x >= NAV_MIN_PAGE;
    if size.x < SPLIT_MIN_PAGE {
        single_column(ui, store, knowledge, state, data, size.y);
    } else {
        ui.horizontal_top(|ui| {
            if with_nav {
                column(ui, NAV_WIDTH, size.y, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("task_nav")
                        .auto_shrink([false, false])
                        .show(ui, |ui| nav::sidebar(ui, state, data));
                });
                ui.separator();
            }
            let rest = ui.available_width();
            let share = if matches!(state.view, View::Inbox | View::Materials) {
                0.55
            } else {
                0.42
            };
            let list_width = (rest * share).clamp(260.0, 560.0);
            column(ui, list_width, size.y, |ui| {
                middle(ui, store, knowledge, state, data, !with_nav, size.y)
            });
            ui.separator();
            let detail_width = ui.available_width();
            column(ui, detail_width, size.y, |ui| {
                right(ui, store, state, data, size.y)
            });
        });
    }
    dialogs(ui, store, state, data);
    toast(ui, state, page_rect);
}

fn column(ui: &mut egui::Ui, width: f32, height: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_width(width);
            ui.set_min_height(height);
            add(ui);
        },
    );
}

/// 中栏：普通视图是任务列表，「待整理建议」视图是材料输入与候选卡。
fn middle(
    ui: &mut egui::Ui,
    store: &mut Store,
    knowledge: Option<&mut crate::knowledge::KnowledgeStore>,
    state: &mut State,
    data: &Snapshot,
    compact_nav: bool,
    height: f32,
) {
    if compact_nav {
        nav::dropdown(ui, state, data);
        ui.add_space(4.0);
    }
    if state.view == View::Inbox {
        egui::ScrollArea::vertical()
            .id_salt("task_inbox")
            .auto_shrink([false, false])
            .max_height(ui.available_height().min(height))
            .show(ui, |ui| inbox::show(ui, store, knowledge, state, data));
    } else if state.view == View::Materials {
        egui::ScrollArea::vertical()
            .id_salt("task_materials")
            .auto_shrink([false, false])
            .max_height(ui.available_height().min(height))
            .show(ui, |ui| materials::list(ui, state, data));
    } else {
        list::show(ui, store, state, data);
    }
}

/// 右栏：有未保存表单时是表单，否则是所选任务的详情。
fn right(ui: &mut egui::Ui, store: &mut Store, state: &mut State, data: &Snapshot, height: f32) {
    if let Some(mut draft) = state.draft.take() {
        let keep = form::show(ui, &mut draft, store, state, data, height);
        // 表单里可能已经确认替换成了另一张草稿，那张优先。
        if keep && state.draft.is_none() {
            state.draft = Some(draft);
        }
    } else if state.view == View::Materials {
        egui::ScrollArea::vertical()
            .id_salt("task_material_detail")
            .auto_shrink([false, false])
            .show(ui, |ui| materials::detail(ui, store, state, data));
    } else {
        egui::ScrollArea::vertical()
            .id_salt("task_detail")
            .auto_shrink([false, false])
            .show(ui, |ui| detail::show(ui, store, state, data));
    }
}

/// 窄窗口：列表与详情二选一，详情顶上带「返回列表」。
fn single_column(
    ui: &mut egui::Ui,
    store: &mut Store,
    knowledge: Option<&mut crate::knowledge::KnowledgeStore>,
    state: &mut State,
    data: &Snapshot,
    height: f32,
) {
    let showing = if state.view == View::Materials {
        state
            .material_selected
            .as_deref()
            .is_some_and(|id| data.sources.iter().any(|s| s.id == id))
    } else {
        state
            .selected
            .as_deref()
            .is_some_and(|id| data.task(id).is_some())
    };
    if state.draft.is_some() || showing {
        if state.draft.is_none()
            && ui
                .add(theme::icon_text_button(theme::Icon::Undo, "返回列表"))
                .clicked()
        {
            if state.view == View::Materials {
                state.material_selected = None;
            } else {
                state.selected = None;
            }
        }
        let height = ui.available_height().min(height);
        right(ui, store, state, data, height);
    } else {
        middle(ui, store, knowledge, state, data, true, height);
    }
}

fn top_bar(ui: &mut egui::Ui, store: &mut Store, state: &mut State) {
    ui.horizontal(|ui| {
        ui.heading("任务助手");
        ui.add_space(8.0);
        let width = (ui.available_width() - 300.0).clamp(140.0, 460.0);
        ui.add(theme::field(
            &mut state.query,
            "搜索任务、负责人、进展、材料原文",
            width,
        ));
        if !state.query.is_empty()
            && theme::icon_button(ui, theme::Icon::SearchClear, "清除搜索").clicked()
        {
            state.query.clear();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            egui::containers::menu::MenuButton::from_button(theme::secondary_icon_button(
                theme::Icon::Menu,
                "更多",
            ))
            .ui(ui, |ui| more_menu(ui, store, state));
            if theme::primary_icon_button(ui, theme::Icon::Plus, "新建任务").clicked() {
                state.open_draft(Replace::Blank);
            }
            if ui
                .add(theme::secondary_icon_button(
                    theme::Icon::Paperclip,
                    "粘贴材料",
                ))
                .on_hover_text("把通知、纪要、周报贴进来，AI 拆成待核对的建议")
                .clicked()
            {
                state.view = View::Inbox;
                state.focus_material = true;
            }
        });
    });
}

fn more_menu(ui: &mut egui::Ui, store: &mut Store, state: &mut State) {
    ui.set_min_width(220.0);
    if ui
        .add(theme::menu_item(theme::Icon::Folder, "管理分类…"))
        .clicked()
    {
        state.categories_open = true;
        ui.close();
    }
    ui.separator();
    let mut auto = state.cache.auto;
    if ui
        .checkbox(&mut auto, "新入库资料自动整理")
        .on_hover_text("只处理之后新入库的资料，结果进「待整理建议」，不会自动采纳")
        .changed()
    {
        match store.set_auto(auto) {
            Ok(()) => state.ok(if auto {
                "已开启：之后新入库的资料会自动整理"
            } else {
                "已关闭新入库资料自动整理"
            }),
            Err(e) => state.fail(format!("保存自动整理设置失败：{e:#}")),
        }
    }
    ui.checkbox(&mut state.history_candidates, "显示已处理的建议");
}

/// 页顶只放常驻可关的错误；成功提示在 [`toast`] 里浮在页面底部，几秒后自己消失。
fn messages(ui: &mut egui::Ui, state: &mut State) {
    let now = ui.input(|i| i.time);
    match (&state.notice, &state.notice_since) {
        (Some(text), Some((shown, since))) if text == shown => {
            if now - since > NOTICE_SECONDS {
                state.notice = None;
                state.notice_since = None;
            } else {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs_f64(
                        NOTICE_SECONDS - (now - since) + 0.05,
                    ));
            }
        }
        (Some(text), _) => state.notice_since = Some((text.clone(), now)),
        (None, _) => state.notice_since = None,
    }
    if let Some(e) = state.error.clone() {
        ui.add_space(4.0);
        if banner(
            ui,
            theme::Icon::TriangleAlert,
            theme::danger(),
            theme::danger_soft(),
            &e,
            "关闭",
        ) {
            state.error = None;
        }
    }
    if state.view != View::Inbox
        && let Some(e) = state.extraction_error.clone()
    {
        ui.add_space(4.0);
        if banner(
            ui,
            theme::Icon::TriangleAlert,
            theme::warn(),
            theme::warn_soft(),
            &format!("材料整理：{e}"),
            "去看看",
        ) {
            state.view = View::Inbox;
        }
    }
}

/// 成功提示浮在页面底部居中，不占布局：出现和消失都不会把三栏往下挤一下再弹回。
/// 点一下提前关掉。
fn toast(ui: &mut egui::Ui, state: &mut State, page: egui::Rect) {
    let Some(text) = state.notice.clone() else {
        return;
    };
    let width = (page.width() - 32.0).clamp(160.0, 460.0);
    let clicked = egui::Area::new(egui::Id::new("task_toast"))
        .order(egui::Order::Foreground)
        .movable(false)
        .pivot(egui::Align2::CENTER_BOTTOM)
        .fixed_pos(page.center_bottom() - egui::vec2(0.0, 16.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .fill(theme::success_soft())
                .corner_radius(theme::chrome_radius(8))
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_max_width(width);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 7.0;
                        ui.add(theme::Icon::Check.image_sized(14.0).tint(theme::success()));
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(text)
                                    .color(theme::success())
                                    .size(theme::font_sizes::SMALL),
                            )
                            .wrap(),
                        );
                    });
                })
                .response
                .interact(egui::Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
        })
        .inner;
    if clicked {
        state.notice = None;
    }
}

/// 带一个动作按钮的淡底提示条，返回按钮是否被点。
pub(super) fn banner(
    ui: &mut egui::Ui,
    icon: theme::Icon,
    fg: egui::Color32,
    bg: egui::Color32,
    text: &str,
    action: &str,
) -> bool {
    let mut clicked = false;
    egui::Frame::new()
        .fill(bg)
        .corner_radius(theme::chrome_radius(7))
        .inner_margin(egui::Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(icon.image_sized(14.0).tint(fg));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    clicked = ui.small_button(action).clicked();
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(text)
                                    .color(fg)
                                    .size(theme::font_sizes::SMALL),
                            )
                            .wrap(),
                        );
                    });
                });
            });
        });
    clicked
}

/// 历史与列表里的时间：今年的省掉年份，解析不了的原样显示。
pub(super) fn short_time(rfc3339: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(rfc3339) {
        Ok(at) => {
            let at = at.with_timezone(&chrono::Local);
            if at.format("%Y").to_string() == chrono::Local::now().format("%Y").to_string() {
                at.format("%m-%d %H:%M").to_string()
            } else {
                at.format("%Y-%m-%d %H:%M").to_string()
            }
        }
        Err(_) => rfc3339.to_owned(),
    }
}

/// 列表与详情里的日期：今年的省掉年份。
pub(super) fn short_date(date: &str, today: &str) -> String {
    if date.len() == 10 && today.len() == 10 && date[..4] == today[..4] {
        date[5..].to_owned()
    } else {
        date.to_owned()
    }
}

/// 相对今天的天数，日期不合法时为 None。
pub(super) fn days_from(today: &str, date: &str) -> Option<i64> {
    let parse = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok();
    Some((parse(date)? - parse(today)?).num_days())
}

fn dialogs(ui: &mut egui::Ui, store: &mut Store, state: &mut State, data: &Snapshot) {
    if let Some(replace) = state.replace_draft.clone() {
        egui::Window::new("当前表单未保存")
            .id(egui::Id::new("task_replace_draft"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                ui.label("放弃正在编辑的内容，打开新的表单？");
                ui.horizontal(|ui| {
                    if ui
                        .add(theme::warning_icon_button(theme::Icon::X, "放弃并继续"))
                        .clicked()
                    {
                        state.replace_draft = None;
                        state.close_draft();
                        state.apply_replace(replace);
                    }
                    if ui.button("继续编辑").clicked() {
                        state.replace_draft = None;
                    }
                });
            });
    }
    if let Some(id) = state.delete_confirm.clone() {
        egui::Window::new("确认移入回收站")
            .id(egui::Id::new("task_delete_confirm"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                if let Some(t) = data.task(&id) {
                    ui.label(format!("将「{}」移入回收站。任务不会级联删除。", t.name));
                    let children = data
                        .tasks
                        .iter()
                        .filter(|x| x.parent_id.as_deref() == Some(&id))
                        .count();
                    let dependents = data
                        .tasks
                        .iter()
                        .filter(|x| x.prerequisites.contains(&id))
                        .count();
                    if children + dependents > 0 {
                        theme::caption(
                            ui,
                            &format!(
                                "{children} 个子任务会变为独立任务，{dependents} 个后续任务会解除该前置并标记待核对。"
                            ),
                        );
                    }
                }
                ui.horizontal(|ui| {
                    if ui
                        .add(theme::warning_icon_button(theme::Icon::Trash, "移入回收站"))
                        .clicked()
                    {
                        match store.soft_delete(&id) {
                            Ok(()) => state.ok("任务已移入回收站，关联任务待核对"),
                            Err(e) => state.fail(format!("删除失败：{e:#}")),
                        }
                        state.delete_confirm = None;
                    }
                    if ui.button("取消").clicked() {
                        state.delete_confirm = None;
                    }
                });
            });
    }
    if let Some((title, content)) = state.source_view.clone() {
        let mut open = true;
        egui::Window::new("来源原文")
            .id(egui::Id::new("task_source_view"))
            .open(&mut open)
            .collapsible(false)
            .default_width(520.0)
            .show(ui.ctx(), |ui| {
                ui.strong(title);
                theme::caption(ui, "识别时保存的快照，不随原资料后来的修改而变。");
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(420.0)
                    .show(ui, |ui| ui.add(egui::Label::new(content).wrap()));
            });
        if !open {
            state.source_view = None;
        }
    }
    categories::window(ui, store, state, data);
    materials::delete_dialog(ui, store, state, data);
}

#[cfg(test)]
mod eval;
#[cfg(test)]
mod tests;
