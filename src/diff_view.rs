//! 版本对照的共享渲染器：起草页的"版本对照"模式与稿件管理的对照窗共用这一套观感，
//! 两处的增删配色、折叠规则、行号一致，看一眼就知道是同一个东西。
//!
//! 渲染器只负责画和收集点击，不读库、不改状态：调用方给它一个 `ManuscriptDiff`
//! 和一份视图状态，它返回用户点中的那处改动在**新版正文**中的字节范围，
//! 由调用方决定是跳源码还是别的动作。

use crate::diff::{
    BlockChange, BodyDiff, ChangeKind, ContextBlock, DiffBlock, FieldChange, InlineSpan,
    ManuscriptDiff, SpanKind,
};
use crate::theme;
use std::collections::BTreeSet;
use std::ops::Range;

/// 对照视图的可交互状态：折叠开关、已展开的折叠块、当前聚焦第几处改动。
pub struct DiffViewState {
    /// 只显示变化处；关掉后未改动的段落也全量铺开。
    pub only_changes: bool,
    /// 已手动展开的折叠块（按块在 `blocks` 中的序号）。
    expanded: BTreeSet<usize>,
    /// 当前聚焦的改动序号（0 基），供上一处 / 下一处导航。
    focus: usize,
    /// 下一帧把聚焦项滚进可视区。
    scroll_to_focus: bool,
}

impl Default for DiffViewState {
    fn default() -> Self {
        Self {
            // 默认只看变化：公文动辄几十段，全量铺开等于让人自己找。
            only_changes: true,
            expanded: BTreeSet::new(),
            focus: 0,
            scroll_to_focus: false,
        }
    }
}

impl DiffViewState {
    /// 切换对照对象（换版本、换稿件）时重置，免得折叠状态与新内容错位。
    pub fn reset(&mut self) {
        self.expanded.clear();
        self.focus = 0;
        self.scroll_to_focus = false;
    }
}

/// 一次渲染里用户触发的动作。
#[derive(Debug, Clone)]
pub enum DiffViewAction {
    /// 点了某处改动，要求跳到新版正文的这个位置。
    JumpToSource(Range<usize>),
}

/// 一次渲染的呈现参数。
pub struct DiffViewConfig<'a> {
    /// 左栏表头，如 `v3（已提交）`。
    pub old_label: &'a str,
    /// 右栏表头，如 `当前（未提交）`。
    pub new_label: &'a str,
    /// 点击改动能否跳到 Markdown 源码。只有起草页的对照能跳——稿件管理看的
    /// 可能是另一篇稿件，跳到起草页的编辑框会落到无关的位置。
    pub allow_jump: bool,
}

/// 渲染整份对照（要素 + 备注 + 正文）。
pub fn manuscript_diff_ui(
    ui: &mut egui::Ui,
    diff: &ManuscriptDiff,
    state: &mut DiffViewState,
    config: &DiffViewConfig<'_>,
) -> Option<DiffViewAction> {
    let DiffViewConfig {
        old_label,
        new_label,
        allow_jump,
    } = *config;
    if diff.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.weak(format!("{new_label} 与 {old_label} 一致，没有差异。"));
        });
        return None;
    }

    let total_body = diff.body.changed_count;
    ui.horizontal_wrapped(|ui| {
        theme::chip(
            ui,
            &format!("共 {} 处变更", diff.total()),
            theme::ACCENT,
            theme::ACCENT_SOFT,
        );
        ui.checkbox(&mut state.only_changes, "只看变化")
            .on_hover_text("关掉后未改动的段落也全量显示，便于通读整篇");
        if total_body > 1 {
            ui.separator();
            ui.label(
                egui::RichText::new(format!("正文第 {} / {} 处", state.focus + 1, total_body))
                    .color(theme::TEXT_MUTED),
            );
            if theme::icon_button(ui, theme::Icon::ArrowUp, "上一处修改").clicked() {
                state.focus = if state.focus == 0 {
                    total_body - 1
                } else {
                    state.focus - 1
                };
                state.scroll_to_focus = true;
            }
            if theme::icon_button(ui, theme::Icon::ArrowDown, "下一处修改").clicked() {
                state.focus = (state.focus + 1) % total_body;
                state.scroll_to_focus = true;
            }
        }
    });
    if total_body > 0 {
        state.focus = state.focus.min(total_body - 1);
    } else {
        state.focus = 0;
    }

    // 两栏宽度必须在进入 ScrollArea 之前算：滚动内容内部的 available_width 首帧可能是
    // 无穷大，那会让两栏退化成一行不换行的长文本，把右栏整个顶出窗口。
    let total = ui.available_width();
    let column = if total.is_finite() && total > 260.0 {
        ((total - ui.spacing().item_spacing.x - 8.0) / 2.0).max(120.0)
    } else {
        320.0
    };

    let mut action = None;
    egui::ScrollArea::vertical()
        .id_salt("version_diff_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if !diff.fields.is_empty() {
                egui::CollapsingHeader::new(format!("公文要素变化（{} 项）", diff.fields.len()))
                    .default_open(true)
                    .show(ui, |ui| {
                        field_changes_table(ui, &diff.fields, old_label, new_label)
                    });
                ui.add_space(4.0);
            }
            if let Some(notes) = &diff.notes {
                egui::CollapsingHeader::new("备注变化")
                    .default_open(false)
                    .show(ui, |ui| {
                        field_changes_table(ui, std::slice::from_ref(notes), old_label, new_label)
                    });
                ui.add_space(4.0);
            }
            if diff.body.changed_count == 0 {
                ui.weak("正文没有变化。");
                return;
            }
            ui.add_space(2.0);
            action = body_ui(
                ui, &diff.body, state, old_label, new_label, allow_jump, column,
            );
        });
    action
}

/// 字段变更表：方向由列头写死，旧版永远在左。配置版本对照也用它。
pub fn field_changes_table(
    ui: &mut egui::Ui,
    changes: &[FieldChange],
    old_label: &str,
    new_label: &str,
) {
    egui::Grid::new(ui.next_auto_id())
        .striped(true)
        .num_columns(3)
        .min_col_width(72.0)
        .show(ui, |ui| {
            ui.strong("字段");
            ui.strong(old_label);
            ui.strong(new_label);
            ui.end_row();
            for change in changes {
                ui.label(change.label);
                ui.add(egui::Label::new(cell_text(&change.before)).truncate())
                    .on_hover_text(cell_hover(&change.before));
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(cell_text(&change.after)).color(theme::ACCENT),
                    )
                    .truncate(),
                )
                .on_hover_text(cell_hover(&change.after));
                ui.end_row();
            }
        });
}

/// 正文两栏对照。左旧右新，同一行左右对齐；未改动段落折叠成一行。
fn body_ui(
    ui: &mut egui::Ui,
    diff: &BodyDiff,
    state: &mut DiffViewState,
    old_label: &str,
    new_label: &str,
    allow_jump: bool,
    column: f32,
) -> Option<DiffViewAction> {
    let mut action = None;
    let mut change_index = 0usize;
    // 不用 Grid：Grid 会把一行两栏压到同一个行高，内容较多的一侧最后几行直接被裁掉
    // ——而改动往往正好让新版多出一行，一裁就把改动本身裁没了。这里自己排两栏：
    // horizontal_top 保证顶部对齐，固定宽度的子区域让两侧各自按需长高。
    ui.horizontal_top(|ui| {
        column_ui(ui, column, |ui| ui.strong(old_label));
        column_ui(ui, column, |ui| ui.strong(new_label));
    });
    for (index, block) in diff.blocks.iter().enumerate() {
        match block {
            DiffBlock::Unchanged(context) => {
                let expanded = !state.only_changes || state.expanded.contains(&index);
                if expanded {
                    for line in context {
                        // 未改动的段两侧一模一样，左右各画一次。
                        let mut clicked = false;
                        ui.horizontal_top(|ui| {
                            let left = column_ui(ui, column, |ui| {
                                context_cell(ui, line, column, allow_jump)
                            });
                            let right = column_ui(ui, column, |ui| {
                                context_cell(ui, line, column, allow_jump)
                            });
                            clicked = left.clicked() || right.clicked();
                        });
                        if allow_jump && clicked {
                            action = Some(DiffViewAction::JumpToSource(line.new_range.clone()));
                        }
                    }
                } else {
                    // 省略号用 U+2026：随应用打包的中文字体没有 U+22EF（⋯），
                    // 那个字符会渲染成缺字方框。
                    let label = format!("…… {} 段未修改", context.len());
                    if ui
                        .add(
                            egui::Label::new(egui::RichText::new(label).color(theme::TEXT_MUTED))
                                .sense(egui::Sense::click()),
                        )
                        .on_hover_text("点击展开这段未修改的内容")
                        .clicked()
                    {
                        state.expanded.insert(index);
                    }
                }
            }
            DiffBlock::Changed(change) => {
                let focused = change_index == state.focus;
                let mut clicked = false;
                let mut left_response = None;
                ui.horizontal_top(|ui| {
                    let left = column_ui(ui, column, |ui| {
                        change_cell(ui, change, Side::Old, column, focused, allow_jump)
                    });
                    let right = column_ui(ui, column, |ui| {
                        change_cell(ui, change, Side::New, column, focused, allow_jump)
                    });
                    clicked = left.clicked() || right.clicked();
                    left_response = Some(left);
                });
                if focused
                    && state.scroll_to_focus
                    && let Some(left) = left_response
                {
                    left.scroll_to_me(Some(egui::Align::Center));
                    state.scroll_to_focus = false;
                }
                // 点任一侧都跳到新版正文的对应位置；纯删除段在新版里没有位置，
                // 这时只把它设为当前聚焦项。
                if clicked {
                    state.focus = change_index;
                    if let Some(range) = change.new_range.clone().filter(|_| allow_jump) {
                        action = Some(DiffViewAction::JumpToSource(range));
                    }
                }
                ui.add_space(4.0);
                change_index += 1;
            }
        }
    }
    action
}

/// 一栏：固定宽度、内容自顶向下排。两栏都走这里，同一行左右才对得齐。
fn column_ui<R>(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            // 光给 desired_size 不够：最终占位按内容算，窄内容（比如表头）会缩到
            // 一起，右栏就跑到左栏紧邻的位置去了。
            ui.set_min_width(width);
            add(ui)
        },
    )
    .inner
}

/// 未改动的一段：灰字、带行号，与改动行的行号列对齐。
fn context_cell(
    ui: &mut egui::Ui,
    line: &ContextBlock,
    width: f32,
    allow_jump: bool,
) -> egui::Response {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("{:>3}", line.new_line))
                .color(theme::TEXT_MUTED)
                .monospace(),
        );
        let mut job = egui::text::LayoutJob::default();
        job.wrap.max_width = (width - 60.0).max(60.0);
        job.append(
            &line.text,
            0.0,
            egui::TextFormat {
                font_id: egui::TextStyle::Body.resolve(ui.style()),
                color: theme::TEXT_MUTED,
                ..Default::default()
            },
        );
        let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
        let label = egui::Label::new(galley);
        if allow_jump {
            ui.add(label.sense(egui::Sense::click()))
                .on_hover_text("点击跳到 Markdown 源码对应位置")
        } else {
            ui.add(label)
        }
    })
    .inner
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Old,
    New,
}

/// 一处改动的单侧格子：行号 + 字级上色的文字，整格可点。
fn change_cell(
    ui: &mut egui::Ui,
    change: &BlockChange,
    side: Side,
    width: f32,
    focused: bool,
    allow_jump: bool,
) -> egui::Response {
    let (spans, line, absent) = match side {
        Side::Old => (
            &change.before_spans,
            change.old_line,
            change.kind == ChangeKind::Insert,
        ),
        Side::New => (
            &change.after_spans,
            change.new_line,
            change.kind == ChangeKind::Delete,
        ),
    };
    let fill = if absent {
        theme::SURFACE_SUNK
    } else {
        match (side, change.kind) {
            (_, ChangeKind::Replace) => theme::WARN_SOFT,
            (Side::Old, ChangeKind::Delete) => theme::DANGER_SOFT,
            (Side::New, ChangeKind::Insert) => theme::SUCCESS_SOFT,
            _ => theme::SURFACE_SUNK,
        }
    };
    let frame = egui::Frame::new()
        .fill(fill)
        .stroke(if focused {
            egui::Stroke::new(1.0, theme::ACCENT)
        } else {
            egui::Stroke::new(1.0, theme::BORDER)
        })
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(6, 4));
    let inner = frame.show(ui, |ui| {
        ui.set_width(width - 16.0);
        if absent {
            // 另一侧新增 / 删除时，这一侧留一个占位格，两栏才不会错行。
            ui.add(egui::Label::new(
                egui::RichText::new(match change.kind {
                    ChangeKind::Insert => "（本版没有这一段）",
                    _ => "（已删除）",
                })
                .color(theme::TEXT_MUTED)
                .italics(),
            ));
            return;
        }
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("{line:>3}"))
                    .color(theme::TEXT_MUTED)
                    .monospace(),
            );
            // 必须先排好版再交给 Label：直接把 LayoutJob 交给 Label，egui 会用
            // 当前可用宽度覆盖 job 里的换行宽度，而 Grid 单元格里的"可用宽度"
            // 并不受列宽约束，结果就是整段不换行、把右栏顶出窗口。
            let galley = ui
                .ctx()
                .fonts_mut(|fonts| fonts.layout_job(spans_job(ui, spans, side, width - 60.0)));
            ui.add(egui::Label::new(galley));
        });
    });
    let hover = match (allow_jump, &change.new_range) {
        (true, Some(_)) => format!("{}：点击跳到 Markdown 源码对应位置", change.role.label()),
        (true, None) => format!(
            "{}：该段在新版里已删除，没有可跳的位置",
            change.role.label()
        ),
        (false, _) => change.role.label().to_string(),
    };
    inner
        .response
        .interact(egui::Sense::click())
        .on_hover_text(hover)
}

/// 把字级片段拼成一个排版任务：同一段文字里，改掉的字才有底色和删除线。
/// 用 `LayoutJob` 而不是并排一串 Label，才能在中文长句里正常换行。
fn spans_job(
    ui: &egui::Ui,
    spans: &[InlineSpan],
    side: Side,
    wrap_width: f32,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = wrap_width.max(60.0);
    let font = egui::TextStyle::Body.resolve(ui.style());
    for span in spans {
        let mut format = egui::TextFormat {
            font_id: font.clone(),
            color: theme::TEXT,
            ..Default::default()
        };
        match span.kind {
            SpanKind::Same => {}
            SpanKind::Removed => {
                format.color = theme::DANGER;
                format.background = theme::DANGER_SOFT;
                format.strikethrough = egui::Stroke::new(1.0, theme::DANGER);
            }
            SpanKind::Added => {
                format.color = theme::SUCCESS;
                format.background = theme::SUCCESS_SOFT;
                format.underline = egui::Stroke::new(1.0, theme::SUCCESS);
            }
        }
        // 一侧只会出现自己那类标记；另一类若混进来（不该发生）按未变处理。
        if (side == Side::Old && span.kind == SpanKind::Added)
            || (side == Side::New && span.kind == SpanKind::Removed)
        {
            format.background = egui::Color32::TRANSPARENT;
            format.strikethrough = egui::Stroke::NONE;
            format.underline = egui::Stroke::NONE;
            format.color = theme::TEXT;
        }
        job.append(&span.text, 0.0, format);
    }
    job
}

/// 表格单元格里的摘要：换行折成空格、超长截断。多行值会把表格行撑得很高，
/// 把同一行的其它单元格顶出可视区，所以一律先压成一行。
fn cell_text(text: &str) -> String {
    if text.trim().is_empty() {
        return "—".to_string();
    }
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = flat.chars().take(48).collect();
    if flat.chars().count() > 48 {
        out.push('…');
    }
    out
}

/// 单元格的悬停全文，同样限长，免得整段糊满屏幕。
fn cell_hover(text: &str) -> String {
    if text.trim().is_empty() {
        return "（空）".to_string();
    }
    let mut out: String = text.chars().take(600).collect();
    if text.chars().count() > 600 {
        out.push('…');
    }
    out
}
