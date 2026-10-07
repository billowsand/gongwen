//! 标题聚焦编辑：把光标所在那一级的标题集中到一个对话框里，对照着统一改，
//! 改完一次写回正文。
//!
//! 默认只列同一个上级标题下的兄弟标题（「二、」下面的（一）到（四）），可以切到
//! 全文这一级。对话框里只改标题文字：`#` 层级、手写的旧编号、研究报告行尾的
//! 锚点 `{#…}` 都原样留在源码里，编号仍由程序生成。

use crate::draft_page::{
    DraftPage, PreviewMode, editor_cursor, line_at_byte, line_ranges, markdown_heading_level,
    navigator,
};
use crate::export;
use crate::theme;
use eframe::egui;
use std::ops::Range;

/// 摘录每节正文开头多少字，给改标题时对照上下文。
const EXCERPT_CHARS: usize = 34;
/// 同组至少这么多条才提示字数不齐；两三条标题谈不上「多数」。
const LENGTH_HINT_MIN_ITEMS: usize = 3;
/// 与同组中位字数相差超过这么多字就标出来。
const LENGTH_HINT_TOLERANCE: usize = 2;
/// 大字模式下标题的字号：投屏时后排也看得清。
const LARGE_FONT_SIZE: f32 = 30.0;

/// 对话框里的一条标题。
#[derive(Debug, Clone)]
pub(crate) struct FocusItem {
    /// 可改的那段文字在源码里的字节范围（不含 `#`、手写旧编号与行尾锚点）。
    text_range: Range<usize>,
    /// 程序生成的编号，只读显示。
    number: Option<String>,
    original: String,
    text: String,
    /// 上级标题在导航条目里的下标；顶层标题没有上级。
    parent: Option<usize>,
    parent_label: Option<String>,
    /// 与光标所在标题同属一个上级。
    sibling: bool,
    /// 这一节正文的开头，已去掉 Markdown 标记。
    excerpt: String,
    /// 这条标题在纸上的字面，大字模式按它排，领导看到的就是印出来的样子。
    family: &'static str,
}

impl FocusItem {
    fn modified(&self) -> bool {
        self.text.trim() != self.original
    }

    fn cleaned(&self) -> String {
        self.text.replace(['\r', '\n'], "").trim().to_string()
    }
}

/// 一次聚焦编辑的全部状态。打开时从正文切出来，写回或取消后丢掉。
#[derive(Debug, Clone)]
pub(crate) struct HeadingFocus {
    /// 打开时的正文。写回前核对，正文在此期间被别处改过就不写，免得按旧位置改错行。
    snapshot: String,
    /// 打开时的光标，写回后按改动长度折算回去。
    cursor: usize,
    /// 对话框标题栏里说明是哪一级，例如 `“（一）”这一级`。
    level_label: String,
    items: Vec<FocusItem>,
    /// 光标所在那条标题在 `items` 里的下标。
    current: usize,
    whole_document: bool,
    /// 下一帧要把焦点给哪一条。
    focus_row: Option<usize>,
    /// 最近一次在改的那一条。点「大字」会让输入框失焦，切换后把焦点还给它。
    last_row: Option<usize>,
}

impl HeadingFocus {
    fn sibling_count(&self) -> usize {
        self.items.iter().filter(|item| item.sibling).count()
    }

    fn visible(&self) -> impl Iterator<Item = usize> + '_ {
        self.items
            .iter()
            .enumerate()
            .filter(move |(_, item)| self.whole_document || item.sibling)
            .map(|(index, _)| index)
    }

    fn modified_count(&self) -> usize {
        self.items.iter().filter(|item| item.modified()).count()
    }

    fn has_empty(&self) -> bool {
        self.items.iter().any(|item| item.cleaned().is_empty())
    }

    /// 同组（同一上级）的中位字数，用来标出长短明显不齐的标题。
    fn typical_length(&self, parent: Option<usize>) -> Option<usize> {
        let mut lengths = self
            .items
            .iter()
            .filter(|item| item.parent == parent)
            .map(|item| visible_chars(&item.text))
            .collect::<Vec<_>>();
        if lengths.len() < LENGTH_HINT_MIN_ITEMS {
            return None;
        }
        lengths.sort_unstable();
        Some(lengths[lengths.len() / 2])
    }
}

/// 排到纸上的字数：不计 Markdown 标记与空白。
fn visible_chars(text: &str) -> usize {
    export::plain_text(text)
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .count()
}

/// 标题行里可改的那段文字：跳过缩进、`#` 与其后的空白，去掉行尾锚点；带编号的
/// 标题再跳过人工写的旧编号（解析器与导航都会清掉它，纸上不印）。
fn editable_range(line_text: &str, numbered: bool) -> Range<usize> {
    let indent = line_text.len() - line_text.trim_start().len();
    let hashes = line_text[indent..]
        .bytes()
        .take_while(|byte| *byte == b'#')
        .count();
    let after_hashes = indent + hashes;
    let body_start = after_hashes
        + (line_text[after_hashes..].len() - line_text[after_hashes..].trim_start().len());
    let body = line_text[body_start..].trim_end();
    let (rest, _) = export::crossref::split_label(body);
    let body_end = body_start + rest.len();
    if !numbered {
        return body_start..body_end;
    }
    let cleaned = export::clean_heading_number(rest);
    let start = if rest.ends_with(cleaned.as_str()) {
        body_end - cleaned.len()
    } else {
        body_start
    };
    start..body_end
}

/// 标题下面第一行正文，去掉标记后截短。遇到下一个标题就停。
fn section_excerpt(markdown: &str, ranges: &[Range<usize>], heading_line: usize) -> String {
    for range in ranges.iter().skip(heading_line + 1) {
        let line = &markdown[range.clone()];
        if markdown_heading_level(line).is_some() {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("<!--")
            || crate::draft_page::is_table_separator_line(trimmed)
        {
            continue;
        }
        let stripped = trimmed.trim_start_matches(['>', '-', '*', '|', ' ']);
        let plain = export::plain_text(stripped);
        let plain = plain.trim();
        if plain.is_empty() {
            continue;
        }
        let mut excerpt = plain.chars().take(EXCERPT_CHARS).collect::<String>();
        if plain.chars().count() > EXCERPT_CHARS {
            excerpt.push('…');
        }
        return excerpt;
    }
    String::new()
}

/// 按光标位置切出一次聚焦编辑。`entries` 取自导航（与目录、预览同一套编号）。
///
/// 光标之前没有标题时返回 None。只收源码里确实以 `#` 起头的条目：研究报告
/// 导航里的「摘要」挂在摘要首段上，不是一行标题，改不了。
pub(crate) fn collect(
    markdown: &str,
    entries: &[navigator::NavEntry],
    cursor: usize,
) -> Option<HeadingFocus> {
    let ranges = line_ranges(markdown);
    // (导航下标, 源码行号)
    let headings = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let line = line_at_byte(&ranges, entry.line.start);
            let range = &ranges[line];
            (range.start == entry.line.start
                && markdown_heading_level(&markdown[range.clone()]).is_some())
            .then_some((index, line))
        })
        .collect::<Vec<_>>();

    // 与源码目录同一种建树法：最近的、层级更浅的标题是上级。
    let mut parents = Vec::with_capacity(headings.len());
    let mut stack: Vec<usize> = Vec::new();
    for &(index, _) in &headings {
        while stack
            .last()
            .is_some_and(|&top| entries[top].level >= entries[index].level)
        {
            stack.pop();
        }
        parents.push(stack.last().copied());
        stack.push(index);
    }

    let current = headings
        .iter()
        .rposition(|&(index, _)| entries[index].line.start <= cursor)?;
    let (current_index, _) = headings[current];
    let level = entries[current_index].level;
    let current_parent = parents[current];

    let mut items = Vec::new();
    let mut current_item = 0;
    for (position, &(index, line)) in headings.iter().enumerate() {
        let entry = &entries[index];
        if entry.level != level {
            continue;
        }
        if position == current {
            current_item = items.len();
        }
        let line_range = ranges[line].clone();
        let local = editable_range(&markdown[line_range.clone()], entry.number.is_some());
        let text_range = line_range.start + local.start..line_range.start + local.end;
        let original = markdown[text_range.clone()].to_string();
        let parent = parents[position];
        items.push(FocusItem {
            text_range,
            number: entry.number.clone(),
            text: original.clone(),
            original,
            parent,
            parent_label: parent.map(|parent| navigator::label_text(&entries[parent])),
            sibling: parent == current_parent,
            excerpt: section_excerpt(markdown, &ranges, line),
            family: entry.family,
        });
    }

    // 用同组第一条的编号给这一级起名：「“（一）”这一级」比「“（三）”这一级」好认。
    let first_sibling = items
        .iter()
        .position(|item| item.sibling)
        .unwrap_or(current_item);
    let level_label = match &items[first_sibling].number {
        Some(number) => format!("“{}”这一级", number.trim()),
        None if level <= 1 => "标题这一级".to_string(),
        None => "这一级".to_string(),
    };
    Some(HeadingFocus {
        snapshot: markdown.to_string(),
        cursor,
        level_label,
        items,
        current: current_item,
        whole_document: false,
        focus_row: Some(current_item),
        last_row: None,
    })
}

/// 把若干段替换写回正文，并把光标折算到改后的位置：改动在光标前面的按长度差
/// 平移，光标落在被改的那段里就放到新文字末尾。
fn apply_edits(text: &str, edits: &[(Range<usize>, String)], cursor: usize) -> (String, usize) {
    let mut sorted = edits.iter().collect::<Vec<_>>();
    sorted.sort_by_key(|(range, _)| range.start);
    let cursor = cursor.min(text.len());
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut new_cursor = None;
    for (range, replacement) in sorted {
        let gap_start = out.len();
        out.push_str(&text[last..range.start]);
        if new_cursor.is_none() && cursor < range.start {
            new_cursor = Some(gap_start + cursor - last);
        }
        out.push_str(replacement);
        if new_cursor.is_none() && cursor <= range.end {
            new_cursor = Some(out.len());
        }
        last = range.end;
    }
    let tail_start = out.len();
    out.push_str(&text[last..]);
    let new_cursor = new_cursor.unwrap_or(tail_start + cursor - last);
    (out, new_cursor)
}

impl DraftPage<'_> {
    /// 只在 Markdown 与 Markdown 对照两种模式下可用：这两种模式里编辑的就是源码。
    pub(crate) fn heading_focus_available(&self) -> bool {
        !self.doc.read_only()
            && matches!(
                self.doc.preview_mode,
                PreviewMode::Source | PreviewMode::Split
            )
    }

    /// 功能区「开始」「格式」两处共用的入口按钮。
    pub(crate) fn heading_focus_button(&mut self, ui: &mut egui::Ui) {
        let shortcut = theme::primary_shortcut("Shift+H");
        if ui
            .add_enabled(
                self.heading_focus_available(),
                theme::icon_text_button(theme::Icon::Heading, "标题聚焦"),
            )
            .on_hover_text(format!(
                "把光标所在这一级的标题集中到一个框里统一改，改完一次写回（{shortcut}）"
            ))
            .on_disabled_hover_text(if self.doc.read_only() {
                "只读稿件不能修改".to_string()
            } else {
                format!("在 Markdown 或 Markdown 对照模式下可用（{shortcut}）")
            })
            .clicked()
        {
            self.open_heading_focus(ui.ctx());
        }
    }

    /// 按编辑框记着的光标打开标题聚焦编辑。
    pub(crate) fn open_heading_focus(&mut self, ctx: &egui::Context) {
        if !self.heading_focus_available() || self.doc.heading_focus.is_some() {
            return;
        }
        let markdown = &self.doc.generated_markdown;
        let Some(cursor) = editor_cursor(ctx, markdown) else {
            *self.status = "先把光标放进要改的那一节，再打开标题聚焦编辑。".into();
            return;
        };
        let entries =
            navigator::collect_entries(markdown, &self.config.numbering, self.doc.draft.kind);
        match collect(markdown, &entries, cursor) {
            Some(focus) => self.doc.heading_focus = Some(focus),
            None => {
                *self.status = if entries.is_empty() {
                    "稿中还没有标题：用 #、##、### 等标记标题。".into()
                } else {
                    "光标上方没有标题：把光标放进某一节正文或标题行里。".into()
                };
            }
        }
    }

    /// 写回对话框里改过的标题。
    fn apply_heading_focus(&mut self, focus: &HeadingFocus) -> bool {
        if self.doc.generated_markdown != focus.snapshot {
            *self.status = "正文在编辑期间有变动，没有写回；请重新打开标题聚焦编辑。".into();
            return true;
        }
        let edits = focus
            .items
            .iter()
            .filter(|item| item.modified())
            .map(|item| (item.text_range.clone(), item.cleaned()))
            .collect::<Vec<_>>();
        if edits.is_empty() {
            return true;
        }
        let (updated, cursor) = apply_edits(&self.doc.generated_markdown, &edits, focus.cursor);
        self.doc.generated_markdown = updated;
        self.doc.pending_source_selection = None;
        self.doc.pending_source_jump = Some(cursor);
        *self.status = format!("已改写 {} 条标题。", edits.len());
        true
    }

    pub(crate) fn heading_focus_modal(&mut self, ctx: &egui::Context) {
        let Some(mut focus) = self.doc.heading_focus.take() else {
            return;
        };
        let mut apply = false;
        let mut cancel = false;
        let mut large = self.config.heading_focus_large;
        let response = egui::Modal::new(egui::Id::new(("heading_focus", self.doc.key)))
            .frame(theme::card())
            .show(ctx, |ui| {
                // 大字模式给投屏用，尽量铺满屏幕宽度，长标题也不必折断。
                let width = if large {
                    (ctx.content_rect().width() * 0.85).clamp(640.0, 1600.0)
                } else {
                    640.0
                };
                ui.set_width(width);
                // 先于各输入框认下主快捷键+回车：单行输入框见回车会自己丢焦点。
                if ui.input_mut(|input| {
                    input.consume_shortcut(&egui::KeyboardShortcut::new(
                        egui::Modifiers::COMMAND,
                        egui::Key::Enter,
                    ))
                }) {
                    apply = true;
                }
                heading_focus_header(ui, &mut focus, &mut large);
                ui.add_space(8.0);
                heading_focus_rows(ui, &mut focus, large);
                if !large {
                    ui.add_space(6.0);
                    theme::caption(
                        ui,
                        "编号由程序生成，这里只改文字；字数与同组多数相差较大的会标黄，便于对仗。                         回车跳到下一条。",
                    );
                }
                ui.add_space(10.0);
                heading_focus_footer(ui, &focus, &mut apply, &mut cancel);
            });
        if large != self.config.heading_focus_large {
            self.config.heading_focus_large = large;
            let _ = crate::storage::save(self.config);
        }
        // 点遮罩关窗只在没改动时生效，免得手一滑丢掉一整组修改；Esc 与取消照常关。
        let escape = response.is_top_modal
            && !response.any_popup_open
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let backdrop = response.backdrop_response.clicked() && focus.modified_count() == 0;
        if apply && !focus.has_empty() {
            self.apply_heading_focus(&focus);
            return;
        }
        if cancel || escape || backdrop {
            return;
        }
        self.doc.heading_focus = Some(focus);
    }
}

fn heading_focus_header(ui: &mut egui::Ui, focus: &mut HeadingFocus, large: &mut bool) {
    ui.horizontal(|ui| {
        ui.add(theme::Icon::Heading.image().tint(theme::accent()));
        ui.heading("标题聚焦编辑");
        let shown = focus.visible().count();
        theme::chip(
            ui,
            &format!("{} · {shown} 条", focus.level_label),
            theme::accent(),
            theme::accent_soft(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(theme::icon_text_button(theme::Icon::ZoomIn, "大字").selected(*large))
                .on_hover_text(if *large {
                    "回到常规排版：显示字数、正文开头与说明"
                } else {
                    "投屏用：标题按纸上字体放大，不显示字数、正文开头与说明"
                })
                .clicked()
            {
                *large = !*large;
                focus.focus_row = focus.last_row.or(Some(focus.current));
            }
        });
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let parent = focus.items[focus.current].parent_label.clone();
        if focus.whole_document {
            theme::caption(ui, "全文这一级，按上级分组");
        } else {
            match parent {
                Some(parent) => {
                    theme::caption(ui, "上级：");
                    ui.label(parent);
                }
                None => {
                    theme::caption(ui, "顶层标题");
                }
            }
        }
        let siblings = focus.sibling_count();
        let total = focus.items.len();
        if total > siblings {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .selectable_label(focus.whole_document, format!("全文这一级（{total}）"))
                    .clicked()
                {
                    focus.whole_document = true;
                }
                if ui
                    .selectable_label(!focus.whole_document, format!("同一上级下（{siblings}）"))
                    .clicked()
                {
                    focus.whole_document = false;
                }
            });
        }
    });
}

fn heading_focus_rows(ui: &mut egui::Ui, focus: &mut HeadingFocus, large: bool) {
    const COUNT_WIDTH: f32 = 44.0;
    const REVERT_WIDTH: f32 = 24.0;
    // 大字模式：编号栏按字号放宽，行距拉开，字数与下方的提示行都不画。
    let number_width = if large { LARGE_FONT_SIZE * 4.2 } else { 64.0 };
    let row_height = if large {
        LARGE_FONT_SIZE * 1.6
    } else {
        ui.spacing().interact_size.y
    };
    let visible = focus.visible().collect::<Vec<_>>();
    let focus_row = focus.focus_row.take();
    let mut next_focus = None;
    let mut last_parent = None;
    theme::popup_scroll(ui.ctx().content_rect().height() * if large { 0.7 } else { 0.55 })
        .id_salt("heading_focus_rows")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            if large {
                ui.spacing_mut().item_spacing.y = 10.0;
            }
            for (position, &index) in visible.iter().enumerate() {
                let typical = focus.typical_length(focus.items[index].parent);
                let item = &mut focus.items[index];
                if focus.whole_document && (position == 0 || last_parent != Some(item.parent)) {
                    ui.add_space(if position == 0 { 0.0 } else { 6.0 });
                    let label = format!(
                        "上级：{}",
                        item.parent_label.as_deref().unwrap_or("（顶层）")
                    );
                    if large {
                        ui.label(
                            egui::RichText::new(label)
                                .size(LARGE_FONT_SIZE * 0.6)
                                .color(theme::text_muted()),
                        );
                    } else {
                        theme::caption(ui, &label);
                    }
                }
                last_parent = Some(item.parent);
                let current = index == focus.current;
                let frame = egui::Frame::new()
                    .inner_margin(if large {
                        egui::Margin::symmetric(10, 8)
                    } else {
                        egui::Margin::symmetric(6, 4)
                    })
                    .corner_radius(theme::chrome_radius(4))
                    .fill(if current {
                        theme::accent_soft().gamma_multiply(0.45)
                    } else {
                        egui::Color32::TRANSPARENT
                    });
                let font = large.then(|| {
                    egui::FontId::new(LARGE_FONT_SIZE, theme::official_family(item.family))
                });
                frame.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(number_width, row_height),
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                // 投屏时没有「原文」行，改过的条目靠编号变色认出来。
                                let color = if large && item.modified() {
                                    theme::accent()
                                } else {
                                    theme::text_muted()
                                };
                                let mut number = egui::RichText::new(
                                    item.number.as_deref().map_or("", str::trim),
                                )
                                .color(color);
                                if let Some(font) = &font {
                                    number = number.font(font.clone());
                                }
                                ui.label(number);
                            },
                        );
                        let reserved = if large {
                            REVERT_WIDTH
                        } else {
                            COUNT_WIDTH + REVERT_WIDTH
                        };
                        let width =
                            (ui.available_width() - reserved - ui.spacing().item_spacing.x * 2.0)
                                .max(120.0);
                        let id = ui.id().with(("heading_focus_text", index));
                        let mut field = theme::field(&mut item.text, "标题文字", width).id(id);
                        if let Some(font) = &font {
                            field = field.font(font.clone());
                        }
                        let response = ui.add(field);
                        if response.has_focus() {
                            focus.last_row = Some(index);
                        }
                        if focus_row == Some(index) {
                            response.request_focus();
                            response.scroll_to_me(None);
                            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
                                let end = egui::text::CCursor::new(item.text.chars().count());
                                state
                                    .cursor
                                    .set_char_range(Some(egui::text::CCursorRange::one(end)));
                                state.store(ui.ctx(), id);
                            }
                        }
                        if response.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter))
                        {
                            next_focus = visible.get(position + 1).copied();
                        }
                        if !large {
                            count_label(ui, item, typical, COUNT_WIDTH);
                        }
                        if item.modified() {
                            if theme::icon_button(ui, theme::Icon::Undo, "还原这一条").clicked()
                            {
                                item.text = item.original.clone();
                            }
                        } else {
                            ui.add_space(REVERT_WIDTH);
                        }
                    });
                    if !large {
                        note_line(ui, item, number_width);
                    }
                });
            }
        });
    focus.focus_row = next_focus;
}

/// 常规模式右侧的字数：清空标红，与同组中位数相差较大的标黄。
fn count_label(ui: &mut egui::Ui, item: &FocusItem, typical: Option<usize>, width: f32) {
    let count = visible_chars(&item.text);
    let uneven = typical.is_some_and(|typical| count.abs_diff(typical) > LENGTH_HINT_TOLERANCE);
    let color = if item.cleaned().is_empty() {
        theme::danger()
    } else if uneven {
        theme::warn()
    } else {
        theme::text_muted()
    };
    let label = ui.add_sized(
        [width, ui.spacing().interact_size.y],
        egui::Label::new(egui::RichText::new(format!("{count} 字")).color(color)),
    );
    if uneven && let Some(typical) = typical {
        label.on_hover_text(format!("同组多数标题约 {typical} 字，这条相差较大"));
    }
}

/// 常规模式标题下方的一行：清空时提示，改过显示原文，否则是这一节正文的开头。
fn note_line(ui: &mut egui::Ui, item: &FocusItem, indent: f32) {
    let (note, color) = if item.cleaned().is_empty() {
        ("标题不能为空".to_string(), theme::danger())
    } else if item.modified() {
        (format!("原文：{}", item.original), theme::text_muted())
    } else {
        (item.excerpt.clone(), theme::text_muted())
    };
    if note.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        ui.add_space(indent + ui.spacing().item_spacing.x);
        ui.add(
            egui::Label::new(
                egui::RichText::new(note)
                    .size(theme::font_sizes::SMALL)
                    .color(color),
            )
            .truncate(),
        );
    });
}

fn heading_focus_footer(
    ui: &mut egui::Ui,
    focus: &HeadingFocus,
    apply: &mut bool,
    cancel: &mut bool,
) {
    ui.horizontal(|ui| {
        let modified = focus.modified_count();
        if focus.has_empty() {
            ui.colored_label(theme::danger(), "有标题被清空了，补上文字才能写回");
        } else if modified == 0 {
            theme::caption(ui, "还没有改动");
        } else {
            ui.label(format!("已改 {modified} 条"));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let enter = theme::primary_shortcut("Enter");
            if theme::primary_icon_button_enabled(
                ui,
                modified > 0 && !focus.has_empty(),
                theme::Icon::SquareCheck,
                "写回正文",
            )
            .on_hover_text(format!("把改过的标题一次写回源码（{enter}）"))
            .clicked()
            {
                *apply = true;
            }
            if ui
                .button("取消")
                .on_hover_text("放弃这次修改（Esc）")
                .clicked()
            {
                *cancel = true;
            }
            // AI 润色下一步接入：候选要走修订建议与确定性闸门，这里先留位置。
            ui.add_enabled(
                false,
                theme::icon_text_button(theme::Icon::Sparkles, "AI 润色"),
            )
            .on_disabled_hover_text("即将提供：由本机模型逐条给出候选，经闸门核对后由你逐条采纳");
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{NumberingConfig, TemplateKind};

    fn focus_at(text: &str, at: &str, kind: TemplateKind) -> HeadingFocus {
        let entries = navigator::collect_entries(text, &NumberingConfig::default(), kind);
        collect(text, &entries, text.find(at).expect("光标位置在文中")).expect("应能聚焦")
    }

    fn texts(focus: &HeadingFocus) -> Vec<(String, bool)> {
        focus
            .items
            .iter()
            .map(|item| (item.original.clone(), item.sibling))
            .collect()
    }

    const DOC: &str = "# 关于做好某项工作的通知\n\n\
        ## 总体要求\n\n总体要求正文。\n\n\
        ## 主要任务\n\n### 强化组织领导\n\n各地要高度重视。\n\n### 完善制度机制\n\n健全制度。\n\n\
        ## 保障措施\n\n### 加强督促检查\n\n定期调度。\n";

    #[test]
    fn lists_siblings_under_the_same_parent_and_the_whole_level() {
        let focus = focus_at(DOC, "健全制度", TemplateKind::OfficialLetter);
        assert_eq!(focus.level_label, "“（一）”这一级");
        assert_eq!(
            texts(&focus),
            vec![
                ("强化组织领导".to_string(), true),
                ("完善制度机制".to_string(), true),
                ("加强督促检查".to_string(), false),
            ]
        );
        assert_eq!(focus.current, 1);
        assert_eq!(focus.sibling_count(), 2);
        assert_eq!(focus.items[0].parent_label.as_deref(), Some("二、主要任务"));
        assert_eq!(focus.items[0].excerpt, "各地要高度重视。");
    }

    #[test]
    fn a_cursor_on_the_heading_line_picks_that_heading() {
        let focus = focus_at(DOC, "## 保障措施", TemplateKind::OfficialLetter);
        assert_eq!(focus.level_label, "“一、”这一级");
        assert_eq!(focus.items.len(), 3);
        assert!(focus.items.iter().all(|item| item.sibling));
        assert_eq!(focus.current, 2);
    }

    #[test]
    fn nothing_to_focus_before_the_first_heading() {
        let text = "开头正文。\n\n## 总体要求\n";
        let entries = navigator::collect_entries(
            text,
            &NumberingConfig::default(),
            TemplateKind::OfficialLetter,
        );
        assert!(collect(text, &entries, 0).is_none());
    }

    #[test]
    fn edits_keep_hashes_manual_numbers_and_anchors() {
        assert_eq!(
            editable_range("## 一、总体要求", true),
            "## 一、".len().."## 一、总体要求".len()
        );
        let line = "### 研究方法 {#sec:method}  ";
        let range = editable_range(line, true);
        assert_eq!(&line[range], "研究方法");
        let title = "# 关于**改革**的通知";
        assert_eq!(&title[editable_range(title, false)], "关于**改革**的通知");
        // 只写了编号的标题：可改部分为空，编号原样留着。
        let bare = "## 一、";
        let range = editable_range(bare, true);
        assert!(range.is_empty() && range.start == bare.len());
    }

    #[test]
    fn writes_back_only_changed_headings_and_moves_the_cursor_along() {
        let text = "## 一、总体要求\n正文甲。\n## 主要任务\n正文乙。\n";
        let mut focus = focus_at(text, "正文乙", TemplateKind::OfficialLetter);
        focus.items[0].text = "  总的要求  ".to_string();
        focus.items[1].text = "重点任务和工作安排".to_string();
        let edits = focus
            .items
            .iter()
            .filter(|item| item.modified())
            .map(|item| (item.text_range.clone(), item.cleaned()))
            .collect::<Vec<_>>();
        let (out, cursor) = apply_edits(text, &edits, focus.cursor);
        assert_eq!(
            out,
            "## 一、总的要求\n正文甲。\n## 重点任务和工作安排\n正文乙。\n"
        );
        assert_eq!(&out[cursor..], "正文乙。\n");
    }

    #[test]
    fn a_cursor_inside_an_edited_heading_lands_at_its_new_end() {
        let text = "## 总体要求\n## 主要任务\n";
        let at = text.find("要求").unwrap();
        let (out, cursor) = apply_edits(
            text,
            &[(3..3 + "总体要求".len(), "总的要求和原则".to_string())],
            at,
        );
        assert_eq!(&out[..cursor], "## 总的要求和原则");
    }

    #[test]
    fn research_headings_keep_anchors_and_skip_the_abstract() {
        let text = "<!-- [正文] -->\n\n# 研究报告\n\n## 背景 {#sec:bg}\n\n背景正文。\n\n## 方法\n\n方法正文。\n";
        let focus = focus_at(text, "方法正文", TemplateKind::ResearchReport);
        assert_eq!(focus.level_label, "“第1章”这一级");
        assert_eq!(
            focus
                .items
                .iter()
                .map(|item| item.original.as_str())
                .collect::<Vec<_>>(),
            vec!["背景", "方法"]
        );
        let edits = vec![(focus.items[0].text_range.clone(), "研究背景".to_string())];
        let (out, _) = apply_edits(text, &edits, 0);
        assert!(out.contains("## 研究背景 {#sec:bg}\n"));
    }

    #[test]
    fn uneven_lengths_are_measured_against_the_group_median() {
        let text = "## 甲\n### 一二三四五六\n### 一二三四五七\n### 一二三四五八九十一二\n";
        let focus = focus_at(text, "一二三四五六", TemplateKind::OfficialLetter);
        let typical = focus.typical_length(focus.items[0].parent).unwrap();
        assert_eq!(typical, 6);
        assert!(visible_chars(&focus.items[2].text).abs_diff(typical) > LENGTH_HINT_TOLERANCE);
    }

    /// 起草页的一个最小外壳：只画标题聚焦对话框，光标事先放在 `at` 那里。
    struct DialogHarness {
        ctx: egui::Context,
        doc: crate::draft_page::DraftSession,
        config: crate::models::AppConfig,
        sender: std::sync::mpsc::Sender<crate::app::WorkerResult>,
        _keep: std::sync::mpsc::Receiver<crate::app::WorkerResult>,
        status: String,
        version_switch: Option<crate::app::VersionSwitchPrompt>,
        revert_confirm: Option<(i64, i64)>,
        metrics: crate::metrics::Metrics,
        actions: Vec<crate::app::DraftAction>,
        export_links: crate::draft_page::ExportLinks,
    }

    impl DialogHarness {
        fn new(markdown: &str, at: &str, large: bool) -> Self {
            let ctx = egui::Context::default();
            theme::configure_icons(&ctx);
            theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
            let config = crate::models::AppConfig {
                last_template: TemplateKind::OfficialLetter,
                heading_focus_large: large,
                ..Default::default()
            };
            let mut doc =
                crate::draft_page::DraftSession::with_markdown(1, &config, markdown.to_string());
            doc.preview_mode = PreviewMode::Source;
            let mut state = egui::text_edit::TextEditState::default();
            let chars = markdown[..markdown.find(at).expect("光标位置在文中")]
                .chars()
                .count();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(chars),
                )));
            state.store(&ctx, crate::draft_page::editor_id());
            let (sender, _keep) = std::sync::mpsc::channel();
            Self {
                ctx,
                doc,
                config,
                sender,
                _keep,
                status: String::new(),
                version_switch: None,
                revert_confirm: None,
                metrics: crate::metrics::Metrics::default(),
                actions: Vec::new(),
                export_links: crate::draft_page::ExportLinks::default(),
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>, open: bool) -> egui::FullOutput {
            self.ctx.clone().run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200.0, 800.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let mut page = DraftPage {
                        doc: &mut self.doc,
                        config: &mut self.config,
                        store: None,
                        sender: &self.sender,
                        status: &mut self.status,
                        version_switch: &mut self.version_switch,
                        revert_confirm: &mut self.revert_confirm,
                        metrics: &mut self.metrics,
                        actions: &mut self.actions,
                        export_links: &mut self.export_links,
                    };
                    if open {
                        page.open_heading_focus(ui.ctx());
                    }
                    page.heading_focus_modal(ui.ctx());
                },
            )
        }
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// 本帧画出来的全部文字，每段附带字号。
    fn painted(output: &egui::FullOutput) -> Vec<(String, f32)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, f32)>) {
            match shape {
                egui::Shape::Text(text) => {
                    let size = text
                        .galley
                        .job
                        .sections
                        .first()
                        .map_or(0.0, |section| section.format.font_id.size);
                    out.push((text.galley.text().to_string(), size));
                }
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| walk(shape, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    /// 端到端：按光标打开对话框，焦点落在光标所在那一条，改字后主快捷键+回车写回。
    #[test]
    fn the_dialog_opens_on_the_current_heading_and_writes_back() {
        let mut harness = DialogHarness::new(DOC, "健全制度", false);
        harness.frame(Vec::new(), true);
        let focus = harness.doc.heading_focus.as_ref().expect("对话框应已打开");
        assert_eq!(focus.items[focus.current].original, "完善制度机制");
        harness.frame(Vec::new(), false);
        let mut typing = vec![key(egui::Key::Backspace, egui::Modifiers::NONE); 2];
        typing.push(egui::Event::Text("体系建设".to_string()));
        harness.frame(typing, false);
        let focus = harness
            .doc
            .heading_focus
            .as_ref()
            .expect("改字时对话框仍开着");
        assert_eq!(focus.items[1].text, "完善制度体系建设");
        assert_eq!(focus.modified_count(), 1);

        harness.frame(vec![key(egui::Key::Enter, egui::Modifiers::COMMAND)], false);
        let markdown = &harness.doc.generated_markdown;
        assert!(harness.doc.heading_focus.is_none(), "写回后对话框应关闭");
        assert!(markdown.contains("\n### 完善制度体系建设\n"));
        assert!(markdown.contains("\n### 强化组织领导\n"));
        let jump = harness
            .doc
            .pending_source_jump
            .expect("光标应回到原来那一节");
        assert!(markdown[jump..].starts_with("健全制度"));
    }

    /// 大字模式给投屏用：标题放大，字数、正文开头与说明一概不画；常规模式照常都有。
    #[test]
    fn large_mode_enlarges_headings_and_drops_the_notes() {
        let visible = |large: bool| {
            let mut harness = DialogHarness::new(DOC, "健全制度", large);
            harness.frame(Vec::new(), true);
            painted(&harness.frame(Vec::new(), false))
        };
        let has = |texts: &[(String, f32)], needle: &str| {
            texts.iter().any(|(text, _)| text.contains(needle))
        };

        let normal = visible(false);
        assert!(has(&normal, "各地要高度重视"), "常规模式显示正文开头");
        assert!(has(&normal, "6 字"), "常规模式显示字数");
        assert!(has(&normal, "编号由程序生成"), "常规模式显示说明");

        let large = visible(true);
        assert!(!has(&large, "各地要高度重视"));
        assert!(!has(&large, "6 字"));
        assert!(!has(&large, "编号由程序生成"));
        let heading = large
            .iter()
            .find(|(text, _)| text == "完善制度机制")
            .expect("标题照常画出");
        assert_eq!(heading.1, LARGE_FONT_SIZE);
    }
}
