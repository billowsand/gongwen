//! 视图栏：取代原来的「视图 / 状态 / 包含归档与回收站」三个重叠的筛选控件。
use super::{Snapshot, State};
use crate::task_assistant::{Status, Task};
use crate::theme;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum View {
    Today,
    #[default]
    Active,
    Waiting,
    Review,
    Inbox,
    Materials,
    Done,
    Cancelled,
    Archived,
    Deleted,
}

pub(super) const MAIN: [View; 6] = [
    View::Today,
    View::Active,
    View::Waiting,
    View::Review,
    View::Inbox,
    View::Materials,
];
pub(super) const CLOSED: [View; 4] = [View::Done, View::Cancelled, View::Archived, View::Deleted];

impl View {
    pub fn label(self) -> &'static str {
        match self {
            Self::Today => "今天与逾期",
            Self::Active => "进行中的工作",
            Self::Waiting => "等待 · 阻塞",
            Self::Review => "待核对",
            Self::Inbox => "待整理建议",
            Self::Materials => "材料",
            Self::Done => "已完成",
            Self::Cancelled => "已取消",
            Self::Archived => "归档",
            Self::Deleted => "回收站",
        }
    }
    fn icon(self) -> theme::Icon {
        match self {
            Self::Today => theme::Icon::Calendar,
            Self::Active => theme::Icon::List,
            Self::Waiting => theme::Icon::History,
            Self::Review => theme::Icon::TriangleAlert,
            Self::Inbox => theme::Icon::Sparkles,
            Self::Materials => theme::Icon::Paperclip,
            Self::Done => theme::Icon::SquareCheck,
            Self::Cancelled => theme::Icon::X,
            Self::Archived => theme::Icon::Archive,
            Self::Deleted => theme::Icon::Trash,
        }
    }
    /// 任务是否属于这个视图。「待整理建议」列的是候选，不是任务。
    pub fn matches(self, all: &[Task], t: &Task, today: &str) -> bool {
        let live = !t.archived && !t.deleted;
        let open = live && !t.status.terminal();
        match self {
            Self::Today => open && t.deadline.as_deref().is_some_and(|d| d <= today),
            Self::Active => open,
            Self::Waiting => open && (t.status == Status::Waiting || held_up(all, t)),
            Self::Review => live && t.needs_review,
            Self::Inbox | Self::Materials => false,
            Self::Done => live && t.status == Status::Done,
            Self::Cancelled => live && t.status == Status::Cancelled,
            Self::Archived => t.archived && !t.deleted,
            Self::Deleted => t.deleted,
        }
    }
}

/// 被卡住：状态是阻塞，或有前置还没确认完成。
///
/// 不直接用 `relations::blocked`：它把「待核对」也算阻塞（那是进入进行中的闸门），
/// 列表里待核对另有标签与视图，再标一次阻塞是重复提示。
pub(super) fn held_up(all: &[Task], t: &Task) -> bool {
    t.status == Status::Blocked
        || t.prerequisites.iter().any(|id| {
            !all.iter()
                .any(|p| &p.id == id && !p.deleted && p.status == Status::Done && !p.needs_review)
        })
}

/// 候选是否还等人处理。
pub(super) fn pending_candidate(state: &str) -> bool {
    matches!(state, "pending" | "later")
}

fn count(view: View, data: &Snapshot) -> usize {
    if view == View::Materials {
        return data.sources.len();
    }
    if view == View::Inbox {
        return data
            .candidates
            .iter()
            .filter(|c| pending_candidate(&c.4))
            .count();
    }
    data.tasks
        .iter()
        .filter(|t| view.matches(&data.tasks, t, &data.today))
        .count()
}

/// 数量的颜色：只有要人马上处理的几项着色，0 不显示。
fn count_color(view: View, data: &Snapshot) -> egui::Color32 {
    match view {
        View::Today => {
            let overdue = data.tasks.iter().any(|t| {
                View::Active.matches(&data.tasks, t, &data.today)
                    && t.deadline
                        .as_deref()
                        .is_some_and(|d| d < data.today.as_str())
            });
            if overdue {
                theme::danger()
            } else {
                theme::warn()
            }
        }
        View::Review => theme::warn(),
        View::Inbox => theme::accent(),
        _ => theme::text_muted(),
    }
}

pub(super) fn sidebar(ui: &mut egui::Ui, state: &mut State, data: &Snapshot) {
    ui.spacing_mut().item_spacing.y = 2.0;
    for view in MAIN {
        view_row(ui, state, data, view);
    }
    ui.add_space(8.0);
    theme::caption(ui, "已结束");
    for view in CLOSED {
        view_row(ui, state, data, view);
    }
    ui.add_space(8.0);
    theme::caption(ui, "分类");
    for name in &data.categories {
        let n = data
            .tasks
            .iter()
            .filter(|t| &t.category == name && View::Active.matches(&data.tasks, t, &data.today))
            .count();
        let selected = &state.category == name;
        if row(
            ui,
            selected,
            theme::Icon::Folder,
            name,
            (n > 0).then(|| (n, theme::text_muted())),
        )
        .clicked()
        {
            state.category = if selected {
                String::new()
            } else {
                name.clone()
            };
            if matches!(state.view, View::Inbox | View::Materials) {
                state.view = View::Active;
            }
        }
    }
    if row(ui, false, theme::Icon::Settings, "管理分类…", None).clicked() {
        state.categories_open = true;
    }
}

fn view_row(ui: &mut egui::Ui, state: &mut State, data: &Snapshot, view: View) {
    let n = count(view, data);
    let shown = (n > 0 && !CLOSED.contains(&view)).then(|| (n, count_color(view, data)));
    if row(ui, state.view == view, view.icon(), view.label(), shown).clicked() {
        state.view = view;
        // 从视图栏进「待整理建议」看的是全部建议，不沿用「只看某份材料」。
        if view == View::Inbox {
            state.inbox_source = None;
        }
    }
}

/// 视图栏的一行：整行可点，左图标、中文字、右数量。
fn row(
    ui: &mut egui::Ui,
    selected: bool,
    icon: theme::Icon,
    label: &str,
    count: Option<(usize, egui::Color32)>,
) -> egui::Response {
    let size = egui::vec2(ui.available_width(), 28.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, label)
    });
    if ui.is_rect_visible(rect) {
        let fill = if selected {
            Some(theme::accent_soft())
        } else if response.hovered() {
            Some(theme::surface_hover())
        } else {
            None
        };
        if let Some(fill) = fill {
            ui.painter()
                .rect_filled(rect, theme::chrome_radius(6), fill);
        }
        let ink = if selected {
            theme::accent()
        } else {
            theme::text()
        };
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 16.0, rect.center().y),
            egui::vec2(14.0, 14.0),
        );
        icon.image_sized(14.0).tint(ink).paint_at(ui, icon_rect);
        let mut right = rect.right() - 8.0;
        if let Some((n, color)) = count {
            let galley = ui.painter().layout_no_wrap(
                n.to_string(),
                egui::FontId::proportional(theme::font_sizes::SMALL),
                color,
            );
            right -= galley.size().x;
            ui.painter().galley(
                egui::pos2(right, rect.center().y - galley.size().y / 2.0),
                galley,
                color,
            );
            right -= 6.0;
        }
        let text = ui.painter().layout(
            label.to_owned(),
            egui::FontId::proportional(theme::font_sizes::BODY),
            ink,
            (right - rect.left() - 30.0).max(10.0),
        );
        ui.painter().galley(
            egui::pos2(rect.left() + 30.0, rect.center().y - text.size().y / 2.0),
            text,
            ink,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// 窄窗口里视图栏收成一个下拉框，分类另起一个下拉框。
pub(super) fn dropdown(ui: &mut egui::Ui, state: &mut State, data: &Snapshot) {
    ui.horizontal_wrapped(|ui| {
        let label = |view: View| {
            let n = count(view, data);
            if n > 0 && !CLOSED.contains(&view) {
                format!("{} · {n}", view.label())
            } else {
                view.label().to_owned()
            }
        };
        egui::ComboBox::from_id_salt("task_view")
            .selected_text(label(state.view))
            .show_ui(ui, |ui| {
                for view in MAIN.into_iter().chain(CLOSED) {
                    ui.selectable_value(&mut state.view, view, label(view));
                }
            });
        egui::ComboBox::from_id_salt("task_filter_category")
            .selected_text(if state.category.is_empty() {
                "全部分类"
            } else {
                &state.category
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.category, String::new(), "全部分类");
                for c in &data.categories {
                    ui.selectable_value(&mut state.category, c.clone(), c);
                }
            });
    });
}
