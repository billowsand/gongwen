//! 版本对照的共享渲染器：起草页的"版本对照"模式与稿件管理的对照窗共用这一套观感，
//! 两处的增删配色、折叠规则、行号一致，看一眼就知道是同一个东西。
//!
//! 渲染器只负责画和收集交互，不读库、不改状态：调用方给它一份 diff 和一份视图
//! 状态。起草页的统一视图（[`unified_body_ui`]）把点击、悬停交回调用方，由它去
//! 与右栏花脸稿预览联动；稿件管理的分栏视图（[`manuscript_diff_ui`]）只读。

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

/// 一次渲染的呈现参数。
pub struct DiffViewConfig<'a> {
    /// 左栏表头，如 `v3（已提交）`。
    pub old_label: &'a str,
    /// 右栏表头，如 `当前（未提交）`。
    pub new_label: &'a str,
}

/// 渲染整份对照（要素 + 备注 + 正文）。
///
/// 这是稿件管理对照窗与 AI 工作台用的只读分栏视图；起草页的版本对照已改用
/// [`unified_body_ui`] + 花脸稿预览（见 `draft_page::version_diff`），第 ④ 期
/// 稿件管理也会切过去，届时这套分栏下线。
pub fn manuscript_diff_ui(
    ui: &mut egui::Ui,
    diff: &ManuscriptDiff,
    state: &mut DiffViewState,
    config: &DiffViewConfig<'_>,
) {
    let DiffViewConfig {
        old_label,
        new_label,
    } = *config;
    if diff.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.weak(format!("{new_label} 与 {old_label} 一致，没有差异。"));
        });
        return;
    }

    let total_body = diff.body.changed_count;
    ui.horizontal_wrapped(|ui| {
        theme::chip(
            ui,
            &format!("共 {} 处变更", diff.total()),
            theme::accent(),
            theme::accent_soft(),
        );
        ui.checkbox(&mut state.only_changes, "只看变化")
            .on_hover_text("关掉后未改动的段落也全量显示，便于通读整篇");
        if total_body > 1 {
            ui.separator();
            ui.label(
                egui::RichText::new(format!("正文第 {} / {} 处", state.focus + 1, total_body))
                    .color(theme::text_muted()),
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

    egui::ScrollArea::vertical()
        .id_salt("version_diff_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if !diff.fields.is_empty() {
                egui::CollapsingHeader::new(format!("文档要素变化（{} 项）", diff.fields.len()))
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
            body_ui(ui, &diff.body, state, old_label, new_label, column);
        });
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
                        egui::RichText::new(cell_text(&change.after)).color(theme::accent()),
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
    column: f32,
) {
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
                        ui.horizontal_top(|ui| {
                            column_ui(ui, column, |ui| context_cell(ui, line, column));
                            column_ui(ui, column, |ui| context_cell(ui, line, column));
                        });
                    }
                } else {
                    // 省略号用 U+2026：随应用打包的中文字体没有 U+22EF（⋯），
                    // 那个字符会渲染成缺字方框。
                    let label = format!("…… {} 段未修改", context.len());
                    if ui
                        .add(
                            egui::Label::new(egui::RichText::new(label).color(theme::text_muted()))
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
                        change_cell(ui, change, Side::Old, column, focused)
                    });
                    let right = column_ui(ui, column, |ui| {
                        change_cell(ui, change, Side::New, column, focused)
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
                // 点任一侧都把它设为当前聚焦项。
                if clicked {
                    state.focus = change_index;
                }
                ui.add_space(4.0);
                change_index += 1;
            }
        }
    }
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
fn context_cell(ui: &mut egui::Ui, line: &ContextBlock, width: f32) -> egui::Response {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("{:>3}", line.new_line))
                .color(theme::text_muted())
                .monospace(),
        );
        let mut job = egui::text::LayoutJob::default();
        job.wrap.max_width = (width - 60.0).max(60.0);
        job.append(
            &line.text,
            0.0,
            egui::TextFormat {
                font_id: egui::TextStyle::Body.resolve(ui.style()),
                color: theme::text_muted(),
                ..Default::default()
            },
        );
        let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
        ui.add(egui::Label::new(galley))
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
        theme::surface_sunk()
    } else {
        match (side, change.kind) {
            (_, ChangeKind::Replace) => theme::warn_soft(),
            (Side::Old, ChangeKind::Delete) => theme::danger_soft(),
            (Side::New, ChangeKind::Insert) => theme::success_soft(),
            _ => theme::surface_sunk(),
        }
    };
    let frame = egui::Frame::new()
        .fill(fill)
        .stroke(if focused {
            egui::Stroke::new(1.0, theme::accent())
        } else {
            egui::Stroke::new(1.0, theme::border())
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
                .color(theme::text_muted())
                .italics(),
            ));
            return;
        }
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("{line:>3}"))
                    .color(theme::text_muted())
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
    inner
        .response
        .interact(egui::Sense::click())
        .on_hover_text(change.role.label())
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
            color: theme::text(),
            ..Default::default()
        };
        match span.kind {
            SpanKind::Same => {}
            SpanKind::Removed => {
                format.color = theme::danger();
                format.background = theme::danger_soft();
                format.strikethrough = egui::Stroke::new(1.0, theme::danger());
            }
            SpanKind::Added => {
                format.color = theme::success();
                format.background = theme::success_soft();
                format.underline = egui::Stroke::new(1.0, theme::success());
            }
        }
        // 一侧只会出现自己那类标记；另一类若混进来（不该发生）按未变处理。
        if (side == Side::Old && span.kind == SpanKind::Added)
            || (side == Side::New && span.kind == SpanKind::Removed)
        {
            format.background = egui::Color32::TRANSPARENT;
            format.strikethrough = egui::Stroke::NONE;
            format.underline = egui::Stroke::NONE;
            format.color = theme::text();
        }
        job.append(&span.text, 0.0, format);
    }
    blank_placeholder(&mut job, font);
    job
}

/// 空行（或只有空白的行）的变更排出来什么都看不见：补一个淡色「（空行）」，
/// 让只改了空行的那一处也有东西可看、可点。
pub(crate) fn blank_placeholder(job: &mut egui::text::LayoutJob, font: egui::FontId) {
    if !crate::diff::is_blank_line(&job.text) {
        return;
    }
    job.append(
        "（空行）",
        0.0,
        egui::TextFormat {
            font_id: font,
            color: theme::text_muted(),
            italics: true,
            ..Default::default()
        },
    );
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

// ── 统一视图（起草页版本对照的左栏）──────────────────────────────────────────
//
// 方案需求结论第 1 条：删除行红底叠在新增行绿底上方，行号槽左右两列分别是
// 旧版 / 新版行号，另有一条增删色条；未改动区折叠，只在变更块上下各留一行
// 上下文（Zed 的做法）。本期只读，第 ③ 期改成可编辑。

/// 折叠未改动区时，变更块上下各保留几行上下文。
const UNIFIED_CONTEXT: usize = 1;

impl DiffViewState {
    /// 当前聚焦第几处变更（0 基）。
    pub fn focus(&self) -> usize {
        self.focus
    }

    /// 聚焦到第 `index` 处变更；`scroll` 为 true 时下一帧把它滚进视野。
    pub fn set_focus(&mut self, index: usize, scroll: bool) {
        self.focus = index;
        self.scroll_to_focus |= scroll;
    }

    /// 上一处 / 下一处，首尾循环（F7 / Shift+F7）。
    pub fn step(&mut self, forward: bool, total: usize) {
        if total == 0 {
            return;
        }
        let current = self.focus.min(total - 1);
        self.focus = if forward {
            (current + 1) % total
        } else {
            (current + total - 1) % total
        };
        self.scroll_to_focus = true;
    }
}

/// 统一视图一帧里的交互结果。
#[derive(Debug, Default)]
pub struct UnifiedOutput {
    /// 点中的变更序号。
    pub clicked_change: Option<usize>,
    /// 鼠标悬停的变更序号。
    pub hovered_change: Option<usize>,
    /// 点中的未改动上下文行在新版源码里的范围（右侧据此滚到同一处）。
    pub clicked_context: Option<Range<usize>>,
    /// 双击未改动的上下文行，要求切回 Markdown 源码编辑这一行。
    pub edit_source: Option<Range<usize>>,
    /// 双击某处变更，要求切回 Markdown 源码编辑它（纯删除的落点由调用方换算）。
    pub edit_change: Option<usize>,
}

/// 一行的类型：决定底色、色条与行号列。
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Context,
    Removed,
    Added,
}

/// 统一视图：正文 diff 逐行画，变更块可点、可悬停。`highlight` 是另一侧（预览）
/// 悬停所对应的变更，淡色描出来。调用方负责套滚动区。
pub fn unified_body_ui(
    ui: &mut egui::Ui,
    diff: &BodyDiff,
    state: &mut DiffViewState,
    highlight: Option<usize>,
) -> UnifiedOutput {
    let mut output = UnifiedOutput::default();
    if diff.changed_count == 0 {
        ui.weak("正文没有变化。");
        return output;
    }
    state.focus = state.focus.min(diff.changed_count - 1);
    let width = ui.available_width().max(200.0);
    let mut change_index = 0usize;
    for (index, block) in diff.blocks.iter().enumerate() {
        match block {
            DiffBlock::Unchanged(context) => {
                let has_before = index > 0;
                let has_after = index + 1 < diff.blocks.len();
                let expanded = !state.only_changes || state.expanded.contains(&index);
                // 折叠时只留贴着变更块的那几行；剩得不多就干脆全显示。
                let head = if has_before { UNIFIED_CONTEXT } else { 0 };
                let tail = if has_after { UNIFIED_CONTEXT } else { 0 };
                let fold = !expanded && context.len() > head + tail + 1;
                for (line_index, line) in context.iter().enumerate() {
                    if fold && line_index >= head && line_index < context.len() - tail {
                        if line_index == head {
                            let hidden = context.len() - head - tail;
                            if fold_row(ui, width, hidden).clicked() {
                                state.expanded.insert(index);
                            }
                        }
                        continue;
                    }
                    let job = plain_job(ui, &line.text, theme::text_muted(), width);
                    let response = diff_row(
                        ui,
                        width,
                        (Some(line.old_line), Some(line.new_line)),
                        RowKind::Context,
                        job,
                    );
                    let response = response
                        .interact(egui::Sense::click())
                        .on_hover_text("单击：右侧预览滚到这里；双击：到 Markdown 源码里改");
                    if response.double_clicked() {
                        output.edit_source = Some(line.new_range.clone());
                    } else if response.clicked() {
                        output.clicked_context = Some(line.new_range.clone());
                    }
                }
            }
            DiffBlock::Changed(change) => {
                let focused = change_index == state.focus;
                let top = ui.cursor().top();
                let left = ui.cursor().left();
                if change.kind != ChangeKind::Insert {
                    let job = unified_spans_job(ui, &change.before_spans, Side::Old, width);
                    diff_row(
                        ui,
                        width,
                        (Some(change.old_line), None),
                        RowKind::Removed,
                        job,
                    );
                }
                if change.kind != ChangeKind::Delete {
                    let job = unified_spans_job(ui, &change.after_spans, Side::New, width);
                    diff_row(
                        ui,
                        width,
                        (None, Some(change.new_line)),
                        RowKind::Added,
                        job,
                    );
                }
                let rect = egui::Rect::from_min_max(
                    egui::pos2(left, top),
                    egui::pos2(left + width, ui.cursor().top()),
                );
                let response = ui
                    .interact(
                        rect,
                        egui::Id::new(("unified-diff-change", change_index)),
                        egui::Sense::click(),
                    )
                    .on_hover_text(format!(
                        "{}：单击在右侧预览里定位；双击到 Markdown 源码里改",
                        change.role.label()
                    ));
                if response.hovered() {
                    output.hovered_change = Some(change_index);
                }
                if response.double_clicked() {
                    output.edit_change = Some(change_index);
                    output.clicked_change = Some(change_index);
                } else if response.clicked() {
                    output.clicked_change = Some(change_index);
                }
                let outline = if focused {
                    Some(egui::Stroke::new(1.5, theme::accent()))
                } else if highlight == Some(change_index) || response.hovered() {
                    Some(egui::Stroke::new(1.0, theme::accent().gamma_multiply(0.5)))
                } else {
                    None
                };
                if let Some(stroke) = outline {
                    ui.painter()
                        .rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
                }
                if focused && state.scroll_to_focus {
                    ui.scroll_to_rect(rect, Some(egui::Align::Center));
                    state.scroll_to_focus = false;
                }
                change_index += 1;
            }
        }
    }
    output
}

/// 行号槽：两列行号 + 色条 + 增删符号。
const GUTTER_NUMBER: f32 = 34.0;
const GUTTER_BAR: f32 = 3.0;
const GUTTER_SIGN: f32 = 16.0;

fn gutter_width() -> f32 {
    GUTTER_NUMBER * 2.0 + GUTTER_BAR + GUTTER_SIGN + 4.0
}

/// 画一行：行号槽 + 底色 + 已排好版的文字。返回整行的响应（只感知悬停）。
fn diff_row(
    ui: &mut egui::Ui,
    width: f32,
    (old_line, new_line): (Option<usize>, Option<usize>),
    kind: RowKind,
    job: egui::text::LayoutJob,
) -> egui::Response {
    let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
    let height = galley.size().y + 4.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter();
    let (fill, bar, sign) = match kind {
        RowKind::Context => (None, None, ""),
        RowKind::Removed => (Some(theme::danger_soft()), Some(theme::danger()), "−"),
        RowKind::Added => (Some(theme::success_soft()), Some(theme::success()), "+"),
    };
    if let Some(fill) = fill {
        painter.rect_filled(rect, 0.0, fill);
    }
    let number_font = egui::TextStyle::Monospace.resolve(ui.style());
    let muted = theme::text_muted();
    let mut x = rect.left();
    for number in [old_line, new_line] {
        if let Some(number) = number.filter(|number| *number > 0) {
            painter.text(
                egui::pos2(x + GUTTER_NUMBER - 6.0, rect.top() + 2.0),
                egui::Align2::RIGHT_TOP,
                number.to_string(),
                number_font.clone(),
                muted,
            );
        }
        x += GUTTER_NUMBER;
    }
    if let Some(bar) = bar {
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(GUTTER_BAR, height)),
            0.0,
            bar,
        );
    }
    x += GUTTER_BAR;
    if !sign.is_empty() {
        painter.text(
            egui::pos2(x + GUTTER_SIGN / 2.0, rect.top() + 2.0),
            egui::Align2::CENTER_TOP,
            sign,
            number_font,
            bar.unwrap_or(muted),
        );
    }
    x += GUTTER_SIGN + 4.0;
    painter.galley(egui::pos2(x, rect.top() + 2.0), galley, theme::text());
    response
}

/// 折叠行：「…… 展开 N 行未改动 ……」，点一下展开这一段。
fn fold_row(ui: &mut egui::Ui, width: f32, hidden: usize) -> egui::Response {
    let height = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let painter = ui.painter();
    let fill = if response.hovered() {
        theme::surface_hover()
    } else {
        theme::surface_sunk()
    };
    painter.rect_filled(rect, 0.0, fill);
    // 省略号用 U+2026：随应用打包的中文字体没有 U+22EF。
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        format!("…… 展开 {hidden} 行未改动 ……"),
        egui::TextStyle::Body.resolve(ui.style()),
        theme::text_muted(),
    );
    response.on_hover_text("点击展开这段未改动的内容")
}

/// 未改动行的排版任务：灰字，按正文区宽度折行。
fn plain_job(ui: &egui::Ui, text: &str, color: egui::Color32, width: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = (width - gutter_width() - 6.0).max(60.0);
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: egui::TextStyle::Body.resolve(ui.style()),
            color,
            ..Default::default()
        },
    );
    job
}

/// 变更行的排版任务：整行已有红 / 绿底，真正改掉的字再加深一档底色，
/// 删除行的字另画删除线——与 Zed 的词级高亮同一观感。
fn unified_spans_job(
    ui: &egui::Ui,
    spans: &[InlineSpan],
    side: Side,
    width: f32,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = (width - gutter_width() - 6.0).max(60.0);
    let font = egui::TextStyle::Body.resolve(ui.style());
    for span in spans {
        let mut format = egui::TextFormat {
            font_id: font.clone(),
            color: theme::text(),
            ..Default::default()
        };
        match (side, span.kind) {
            (Side::Old, SpanKind::Removed) => {
                format.background = theme::danger().gamma_multiply(0.28);
                format.strikethrough = egui::Stroke::new(1.0, theme::danger());
            }
            (Side::New, SpanKind::Added) => {
                format.background = theme::success().gamma_multiply(0.28);
            }
            _ => {}
        }
        job.append(&span.text, 0.0, format);
    }
    blank_placeholder(&mut job, font);
    job
}
