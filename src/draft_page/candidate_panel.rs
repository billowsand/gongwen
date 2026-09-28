//! 候选区面板：编辑区底部的一行折叠条，点开后左栏按来源小节列出条目，右栏
//! 显示选中那条的全文，可以只挑其中一部分插回正文。
//!
//! 右栏是一个真正的编辑框：拖选任意文字，双击选一句（到句末标点为止，egui
//! 自带的双击只选到逗号），三击选一段，也可以直接修改候选内容。

use crate::draft_page::DraftPage;
use crate::draft_page::candidates::{
    Candidate, group_candidates, keep_selection_on_secondary, menu_entry, selection_before_show,
    selection_bytes, sentence_range, size_label, time_label, visible_chars,
};
use crate::models::{EDITOR_FONT_SIZE_MAX, EDITOR_FONT_SIZE_MIN};
use crate::theme;
use eframe::egui;
use std::ops::Range;

const PANEL_DEFAULT_HEIGHT: f32 = 290.0;
const PANEL_MIN_HEIGHT: f32 = 170.0;
const LIST_DEFAULT_WIDTH: f32 = 250.0;
const ROW_HEIGHT: f32 = 46.0;
/// 条目悬停时最多预览这么多字。
const HOVER_PREVIEW_CHARS: usize = 240;

fn candidate_editor_id(key: u64) -> egui::Id {
    egui::Id::new(("candidate_editor", key))
}

/// 面板里点出来的动作。面板画的时候借着候选内容，动作等画完再统一执行。
enum PanelAction {
    Toggle,
    Select(u64),
    Insert(u64, Option<Range<usize>>),
    Copy(String),
    Delete(u64, Option<Range<usize>>),
    Clear,
    Undo,
    Hint(&'static str),
}

impl DraftPage<'_> {
    /// 编辑区底部的候选区。只在有源码编辑框的显示方式下出现。
    pub(crate) fn candidate_panel_ui(&mut self, ui: &mut egui::Ui) {
        let open = self.doc.candidates.open;
        // 最高只占编辑区七成，正文总要留出能写字的地方。
        let max_height = (ui.available_height() * 0.7).max(PANEL_MIN_HEIGHT);
        let panel = if open {
            // v2：v1 的内容比面板高，面板每帧被撑高一点，旧版记住的高度作废。
            egui::Panel::bottom("candidate_panel_v2")
                .resizable(true)
                .default_size(PANEL_DEFAULT_HEIGHT.min(max_height))
                .size_range(PANEL_MIN_HEIGHT..=max_height)
        } else {
            egui::Panel::bottom("candidate_bar_v1").resizable(false)
        };
        let mut actions = Vec::new();
        panel
            .frame(
                egui::Frame::new()
                    .fill(theme::surface())
                    .inner_margin(egui::Margin::symmetric(10, 5)),
            )
            .show(ui, |ui| {
                self.candidate_header_ui(ui, &mut actions);
                if open {
                    ui.add_space(4.0);
                    // 可拖动的面板会按内容的实际高度长大；内容一旦比面板高哪怕一个
                    // 像素，下一帧面板就被撑高、内容跟着变高，于是自己一路往上长。
                    // 所以正文部分画在一块定死大小、不向外报尺寸的子区域里，
                    // 放不下的裁掉，面板高度只由拖动决定。
                    let rect = ui.available_rect_before_wrap();
                    let mut body = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(rect)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                    );
                    body.set_clip_rect(rect.intersect(ui.clip_rect()));
                    self.candidate_body_ui(&mut body, &mut actions);
                    ui.allocate_rect(rect, egui::Sense::hover());
                }
            });
        for action in actions {
            self.run_panel_action(ui.ctx(), action);
        }
    }

    /// 折叠条：展开/收起、条数、撤销入口与清空。
    fn candidate_header_ui(&self, ui: &mut egui::Ui, actions: &mut Vec<PanelAction>) {
        let state = &self.doc.candidates;
        let count = state.items.len();
        let editable = !self.doc.read_only();
        ui.horizontal(|ui| {
            let icon = if state.open {
                theme::Icon::ChevronDown
            } else {
                theme::Icon::ChevronUp
            };
            let label = if count == 0 {
                "候选区".to_string()
            } else {
                format!("候选区 · {count} 条")
            };
            let hover = if state.open {
                "收起候选区"
            } else {
                "展开候选区：写稿时暂时移出正文、以后可能还要用的文字"
            };
            let toggle = egui::Button::image_and_text(icon.image(), label)
                .image_tint_follows_text_color(true)
                .frame(false);
            if ui.add(toggle).on_hover_text(hover).clicked() {
                actions.push(PanelAction::Toggle);
            }
            if count == 0 && editable {
                ui.weak(format!(
                    "选中正文后右键「移入候选区」，或按 {}",
                    theme::primary_shortcut("Shift+X")
                ));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if state.open && count > 0 && editable {
                    ui.menu_button("清空", |ui| {
                        ui.label(format!("清空本篇全部 {count} 条候选？"));
                        ui.weak("清空后可以在折叠条上撤销。");
                        if ui
                            .add(theme::warning_icon_button(theme::Icon::Trash, "清空"))
                            .clicked()
                        {
                            actions.push(PanelAction::Clear);
                            ui.close();
                        }
                    });
                }
                if let Some(undo) = state
                    .undo
                    .as_ref()
                    .filter(|undo| undo.valid_for(&self.doc.generated_markdown))
                {
                    if ui.link("撤销").clicked() {
                        actions.push(PanelAction::Undo);
                    }
                    ui.colored_label(theme::success(), undo.label);
                    // 到点让撤销入口自己消失。
                    ui.ctx().request_repaint_after(undo.remaining());
                }
            });
        });
    }

    fn candidate_body_ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<PanelAction>) {
        if self.doc.candidates.items.is_empty() {
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                ui.weak("还没有候选文字。");
                ui.weak(format!(
                    "写稿时觉得某段话眼下用不上，选中它，右键「移入候选区」或按 {}。",
                    theme::primary_shortcut("Shift+X")
                ));
                ui.weak("以后需要时在这里挑出要用的部分，插回正文光标处。");
            });
            return;
        }
        let state = &mut self.doc.candidates;
        state.sections.refresh(
            &self.doc.generated_markdown,
            &self.config.numbering,
            self.doc.draft.kind,
        );
        if state
            .selected
            .is_none_or(|key| state.position(key).is_none())
        {
            state.selected = state.items.first().map(|item| item.key);
        }
        egui::Panel::left("candidate_list_v1")
            .resizable(true)
            .default_size(LIST_DEFAULT_WIDTH)
            .size_range(180.0..=460.0)
            .frame(egui::Frame::new().inner_margin(egui::Margin {
                right: 6,
                ..egui::Margin::ZERO
            }))
            .show(ui, |ui| self.candidate_list_ui(ui, actions));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(egui::Margin {
                left: 8,
                ..egui::Margin::ZERO
            }))
            .show(ui, |ui| self.candidate_detail_ui(ui, actions));
    }

    /// 左栏：按来源小节分组的条目。
    fn candidate_list_ui(&self, ui: &mut egui::Ui, actions: &mut Vec<PanelAction>) {
        let state = &self.doc.candidates;
        let groups = group_candidates(&state.items, &state.sections.tops);
        egui::ScrollArea::vertical()
            .id_salt("candidate_list_scroll")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for group in &groups {
                    let title =
                        egui::RichText::new(format!("{}（{}）", group.label, group.members.len()))
                            .color(if group.missing {
                                theme::text_muted()
                            } else {
                                theme::text_soft()
                            });
                    let header = egui::CollapsingHeader::new(title)
                        .id_salt(("candidate_group", group.key.as_str()))
                        .default_open(true)
                        .show_unindented(ui, |ui| {
                            for &index in &group.members {
                                let item = &state.items[index];
                                let selected = state.selected == Some(item.key);
                                if candidate_row(ui, item, selected).clicked() {
                                    actions.push(PanelAction::Select(item.key));
                                }
                            }
                        });
                    if group.missing {
                        header
                            .header_response
                            .on_hover_text("这个小节已经不在正文里了，显示的是移入时的标题");
                    }
                }
            });
    }

    /// 右栏：选中条目的全文，挑一部分插回正文。
    fn candidate_detail_ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<PanelAction>) {
        let Some(key) = self.doc.candidates.selected else {
            return;
        };
        let Some(index) = self.doc.candidates.position(key) else {
            return;
        };
        let ctx = ui.ctx().clone();
        let editable = !self.doc.read_only();
        let id = candidate_editor_id(key);
        let selection = {
            let text = &self.doc.candidates.items[index].record.text;
            selection_bytes(&ctx, id, text).filter(|range| !range.is_empty())
        };

        // Ctrl+Enter 插入选中：抢在编辑框之前吃掉，否则它会插进一个换行。
        let insert_shortcut =
            egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Enter);
        if ctx.memory(|memory| memory.has_focus(id))
            && ctx.input_mut(|input| input.consume_shortcut(&insert_shortcut))
            && editable
        {
            actions.push(match &selection {
                Some(range) => PanelAction::Insert(key, Some(range.clone())),
                None => PanelAction::Hint("先在候选里选中要插入的部分。"),
            });
        }

        let item = &self.doc.candidates.items[index];
        ui.horizontal(|ui| {
            let label = match &selection {
                Some(range) => format!(
                    "插入选中 · {} 字",
                    visible_chars(&item.record.text[range.clone()])
                ),
                None => "插入选中".to_string(),
            };
            if theme::primary_icon_button_enabled(
                ui,
                editable && selection.is_some(),
                theme::Icon::ArrowUp,
                &label,
            )
            .on_hover_text(format!(
                "插到正文光标处，并从候选里删去这部分（{}）",
                theme::primary_shortcut("Enter")
            ))
            .on_disabled_hover_text("先在下面选中要用的部分：拖选，或双击选一句、三击选一段")
            .clicked()
            {
                actions.push(PanelAction::Insert(key, selection.clone()));
            }
            if ui
                .add_enabled(editable, egui::Button::new("整条插入"))
                .on_hover_text("整条插到正文光标处，并从候选区移走")
                .clicked()
            {
                actions.push(PanelAction::Insert(key, None));
            }
            let copy_hover = if selection.is_some() {
                "复制选中的部分，候选保留"
            } else {
                "复制整条，候选保留"
            };
            if ui
                .add(theme::icon_text_button(theme::Icon::Copy, "复制"))
                .on_hover_text(copy_hover)
                .clicked()
            {
                let copied = selection
                    .clone()
                    .map_or(item.record.text.as_str(), |range| &item.record.text[range]);
                actions.push(PanelAction::Copy(copied.to_string()));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if editable
                    && theme::danger_icon_button(ui, theme::Icon::Trash, "删除整条候选").clicked()
                {
                    actions.push(PanelAction::Delete(key, None));
                }
                if editable
                    && theme::icon_button_enabled(
                        ui,
                        selection.is_some(),
                        theme::Icon::Eraser,
                        "从候选里删去选中的部分",
                    )
                    .clicked()
                {
                    actions.push(PanelAction::Delete(key, selection.clone()));
                }
                let meta = [
                    item.record.subsection.clone(),
                    time_label(&item.record.created_at),
                ]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
                ui.add(egui::Label::new(egui::RichText::new(meta).weak()).truncate());
            });
        });
        ui.add_space(4.0);

        let font_size = self
            .config
            .editor_font_size
            .clamp(EDITOR_FONT_SIZE_MIN, EDITOR_FONT_SIZE_MAX);
        let font = egui::FontId::new(
            font_size,
            egui::FontFamily::Name(theme::EDITOR_FONT_FAMILY.into()),
        );
        // 全文框的高度要精确扣掉卡片边距、与提示行之间的间距和提示行本身
        // （横排一行至少一个控件高），多出一点就会把可拖动的面板撑高。
        let card = theme::card();
        let hint_row = ui.spacing().interact_size.y;
        let text_height = (ui.available_height()
            - card.total_margin().sum().y
            - ui.spacing().item_spacing.y
            - hint_row)
            .max(40.0);
        let before = selection_before_show(&ctx, id);
        let mut changed = false;
        let mut lost_focus = false;
        let mut menu = None;
        card.show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("candidate_detail_scroll", key))
                .auto_shrink([false; 2])
                .max_height(text_height)
                .show(ui, |ui| {
                    let text = &mut self.doc.candidates.items[index].record.text;
                    let mut read_only_text = text.as_str();
                    let buffer: &mut dyn egui::TextBuffer =
                        if editable { text } else { &mut read_only_text };
                    let output = egui::TextEdit::multiline(&mut *buffer)
                        .id(id)
                        .font(font)
                        .frame(egui::Frame::NONE)
                        .desired_width(f32::INFINITY)
                        .desired_rows(4)
                        .show(ui);
                    keep_selection_on_secondary(ui, &output, before, id);
                    if output.response.double_clicked()
                        && let Some(range) = output.cursor_range
                    {
                        // egui 已按「词」选中；以词首为准扩到整句。
                        let start = range.primary.index.0.min(range.secondary.index.0);
                        let sentence = sentence_range(buffer.as_str(), start);
                        let mut state = output.state.clone();
                        state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::two(
                                egui::text::CCursor::new(sentence.start),
                                egui::text::CCursor::new(sentence.end),
                            )));
                        state.store(ui.ctx(), id);
                    }
                    changed = output.response.changed();
                    lost_focus = output.response.lost_focus();
                    menu = candidate_context_menu(&output.response, editable, selection.is_some());
                });
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let hint = if editable {
                "拖选任意文字 · 双击选一句 · 三击选一段 · 可以直接修改"
            } else {
                "只读稿件：候选只能查看和复制"
            };
            ui.label(egui::RichText::new(hint).small().weak());
        });

        if changed {
            // 手改过候选，之前的撤销点对不上了。
            self.doc.candidates.undo = None;
            self.doc.candidates.mark_dirty(true);
        }
        let text = &self.doc.candidates.items[index].record.text;
        if lost_focus && text.trim().is_empty() {
            actions.push(PanelAction::Delete(key, None));
        }
        match menu {
            Some(CandidateMenu::Insert) => {
                actions.push(PanelAction::Insert(key, selection.clone()));
            }
            Some(CandidateMenu::InsertWhole) => actions.push(PanelAction::Insert(key, None)),
            Some(CandidateMenu::Copy) => {
                let copied = selection.map_or(text.as_str(), |range| &text[range]);
                actions.push(PanelAction::Copy(copied.to_string()));
            }
            Some(CandidateMenu::DeletePart) => {
                actions.push(PanelAction::Delete(key, selection));
            }
            Some(CandidateMenu::DeleteWhole) => actions.push(PanelAction::Delete(key, None)),
            None => {}
        }
    }

    fn run_panel_action(&mut self, ctx: &egui::Context, action: PanelAction) {
        match action {
            PanelAction::Toggle => self.doc.candidates.open = !self.doc.candidates.open,
            PanelAction::Select(key) => self.doc.candidates.selected = Some(key),
            PanelAction::Insert(key, part) => self.insert_candidate(ctx, key, part),
            PanelAction::Copy(text) => {
                ctx.copy_text(text);
                *self.status = "已复制，候选保留。".into();
            }
            PanelAction::Delete(key, part) => self.delete_candidate(ctx, key, part),
            PanelAction::Clear => self.clear_candidates(ctx),
            PanelAction::Undo => self.undo_candidates(),
            PanelAction::Hint(message) => *self.status = message.into(),
        }
    }
}

/// 右栏编辑框右键菜单里的动作。
enum CandidateMenu {
    Insert,
    InsertWhole,
    Copy,
    DeletePart,
    DeleteWhole,
}

fn candidate_context_menu(
    response: &egui::Response,
    editable: bool,
    has_selection: bool,
) -> Option<CandidateMenu> {
    let mut action = None;
    response.context_menu(|ui| {
        ui.set_min_width(200.0);
        if menu_entry(
            ui,
            editable && has_selection,
            "插入到正文",
            &theme::primary_shortcut("Enter"),
        ) {
            action = Some(CandidateMenu::Insert);
        }
        if menu_entry(ui, true, "复制", &theme::primary_shortcut("C")) {
            action = Some(CandidateMenu::Copy);
        }
        if menu_entry(ui, editable && has_selection, "从候选里删去这部分", "") {
            action = Some(CandidateMenu::DeletePart);
        }
        ui.separator();
        if menu_entry(ui, editable, "整条插入", "") {
            action = Some(CandidateMenu::InsertWhole);
        }
        if menu_entry(ui, editable, "删除整条", "") {
            action = Some(CandidateMenu::DeleteWhole);
        }
        if action.is_some() {
            ui.close();
        }
    });
    action
}

/// 左栏的一行：正文开头一行，下面一行小字写小节、时间与篇幅。
fn candidate_row(ui: &mut egui::Ui, item: &Candidate, selected: bool) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let fill = if selected {
            theme::accent_soft()
        } else if response.hovered() {
            theme::surface_hover()
        } else {
            egui::Color32::TRANSPARENT
        };
        painter.rect_filled(rect, 6.0, fill);
        if selected {
            painter.rect_filled(
                egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())),
                0.0,
                theme::accent(),
            );
        }
        let inner = rect.shrink2(egui::vec2(10.0, 5.0));
        let preview = item
            .record
            .text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let title = painter.layout_job(single_line(
            &preview,
            egui::FontId::proportional(14.0),
            theme::text(),
            inner.width(),
        ));
        let meta = [
            item.record.subsection.clone(),
            time_label(&item.record.created_at),
            size_label(&item.record.text),
        ]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
        let meta = painter.layout_job(single_line(
            &meta,
            egui::FontId::proportional(11.5),
            theme::text_muted(),
            inner.width(),
        ));
        let title_height = title.size().y;
        painter.galley(inner.min, title, theme::text());
        painter.galley(
            inner.min + egui::vec2(0.0, title_height + 2.0),
            meta,
            theme::text_muted(),
        );
    }
    let preview: String = item.record.text.chars().take(HOVER_PREVIEW_CHARS).collect();
    let preview = if item.record.text.chars().count() > HOVER_PREVIEW_CHARS {
        format!("{preview}……")
    } else {
        preview
    };
    response.on_hover_text(preview)
}

/// 单行排版，放不下的部分用省略号截断。
fn single_line(
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    width: f32,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat::simple(font, color),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: width,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    job
}

#[cfg(test)]
mod tests {
    //! 候选区的端到端检查：真库（内存）、真 egui 上下文，按真实的鼠标事件
    //! 右键移入、在候选里双击选句、插回正文、撤销。

    use super::*;
    use crate::app::{DraftAction, VersionSwitchPrompt, WorkerResult};
    use crate::draft_page::{DraftSession, ExportLinks, editor_id};
    use crate::manuscript::{ManuscriptStore, NewManuscript};
    use crate::models::{AppConfig, ManuscriptStatus};
    use std::path::Path;
    use std::sync::mpsc::{Receiver, Sender};

    const DOC: &str = "# 关于加强统筹的报告\n\n## 主要做法\n\n成立工作专班，每月召开一次调度会。各单位确定一名联络员。\n\n## 下一步打算\n\n继续推进。\n";

    struct Harness {
        ctx: egui::Context,
        doc: DraftSession,
        config: AppConfig,
        store: ManuscriptStore,
        id: i64,
        sender: Sender<WorkerResult>,
        status: String,
        version_switch: Option<VersionSwitchPrompt>,
        revert_confirm: Option<(i64, i64)>,
        actions: Vec<DraftAction>,
        export_links: ExportLinks,
        metrics: crate::metrics::Metrics,
        clock: f64,
        _receiver: Receiver<WorkerResult>,
    }

    impl Harness {
        fn new() -> Self {
            let ctx = egui::Context::default();
            theme::configure_icons(&ctx);
            theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
            let config = AppConfig::default();
            let mut store = ManuscriptStore::open(Path::new(":memory:")).unwrap();
            let id = store
                .create(
                    &NewManuscript {
                        content_markdown: DOC.into(),
                        status: ManuscriptStatus::Draft,
                        ..Default::default()
                    },
                    None,
                )
                .unwrap();
            let mut doc = DraftSession::with_markdown(1, &config, DOC.into());
            doc.manuscript_id = Some(id);
            let (sender, receiver) = std::sync::mpsc::channel();
            let mut harness = Self {
                ctx,
                doc,
                config,
                store,
                id,
                sender,
                status: String::new(),
                version_switch: None,
                revert_confirm: None,
                actions: Vec::new(),
                export_links: ExportLinks::default(),
                metrics: crate::metrics::Metrics::default(),
                clock: 0.0,
                _receiver: receiver,
            };
            harness.frame(Vec::new());
            harness.frame(Vec::new());
            harness
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
            self.clock += 0.05;
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 900.0),
                )),
                time: Some(self.clock),
                events,
                ..Default::default()
            };
            self.ctx.clone().run_ui(raw, |ui| {
                let mut page = DraftPage {
                    doc: &mut self.doc,
                    config: &mut self.config,
                    store: Some(&mut self.store),
                    sender: &self.sender,
                    status: &mut self.status,
                    version_switch: &mut self.version_switch,
                    revert_confirm: &mut self.revert_confirm,
                    actions: &mut self.actions,
                    export_links: &mut self.export_links,
                    metrics: &mut self.metrics,
                };
                page.sync_candidates(ui.ctx());
                page.preview_ui(ui);
            })
        }

        fn button(
            &mut self,
            at: egui::Pos2,
            button: egui::PointerButton,
            pressed: bool,
        ) -> egui::FullOutput {
            self.frame(vec![egui::Event::PointerButton {
                pos: at,
                button,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }])
        }

        fn click(&mut self, at: egui::Pos2) -> egui::FullOutput {
            self.frame(vec![egui::Event::PointerMoved(at)]);
            self.button(at, egui::PointerButton::Primary, true);
            self.button(at, egui::PointerButton::Primary, false)
        }

        /// 把源码编辑框的选区设成正文里的 `picked`（为空串时只放光标到 `at` 之后）。
        fn select_in_editor(&mut self, picked: &str) {
            let text = &self.doc.generated_markdown;
            let byte = text.find(picked).unwrap();
            let start = text[..byte].chars().count();
            let end = start + picked.chars().count();
            let mut state = egui::TextEdit::load_state(&self.ctx, editor_id()).unwrap();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(start),
                    egui::text::CCursor::new(end),
                )));
            state.store(&self.ctx, editor_id());
        }

        fn editor_selection(&self) -> Option<String> {
            let range =
                crate::draft_page::editor_selection(&self.ctx, &self.doc.generated_markdown)?;
            Some(self.doc.generated_markdown[range].to_string())
        }

        fn rect_of(&self, id: egui::Id) -> egui::Rect {
            self.ctx.read_response(id).expect("控件应已画出").rect
        }
    }

    /// 画面上字面恰好是 `text` 的那段文字的中心。
    fn text_at(output: &egui::FullOutput, matches: impl Fn(&str) -> bool) -> Option<egui::Pos2> {
        output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(shape) if matches(shape.galley.text()) => {
                    Some(shape.visual_bounding_rect().center())
                }
                _ => None,
            })
    }

    /// 在源码编辑框里右键：返回菜单弹出后的那一帧。
    fn right_click_editor(harness: &mut Harness) -> egui::FullOutput {
        let rect = harness.rect_of(editor_id());
        let at = rect.min + egui::vec2(24.0, 8.0);
        harness.frame(vec![egui::Event::PointerMoved(at)]);
        harness.button(at, egui::PointerButton::Secondary, true);
        harness.button(at, egui::PointerButton::Secondary, false);
        harness.frame(Vec::new())
    }

    #[test]
    fn right_click_keeps_the_selection_and_moves_it_into_candidates() {
        let mut harness = Harness::new();
        let picked = "成立工作专班，每月召开一次调度会。各单位确定一名联络员。";
        harness.select_in_editor(picked);
        let output = right_click_editor(&mut harness);
        // 右键点在选区外，选区也不能丢，否则菜单里的「移入候选区」无从谈起。
        assert_eq!(harness.editor_selection().as_deref(), Some(picked));
        let item = text_at(&output, |text| text == "移入候选区").expect("右键菜单应列出移入候选区");
        harness.click(item);

        assert!(!harness.doc.generated_markdown.contains(picked));
        assert!(
            harness
                .doc
                .generated_markdown
                .contains("## 主要做法\n\n## 下一步打算")
        );
        let candidate = &harness.doc.candidates.items[0].record;
        assert_eq!(candidate.text, picked);
        assert_eq!(candidate.section, "主要做法");
        assert_eq!(candidate.section_label, "一、主要做法");

        harness.frame(Vec::new());
        let saved = harness.store.load_candidates(harness.id).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].text, picked);
    }

    #[test]
    fn expanded_panel_keeps_its_height_across_frames() {
        let mut harness = Harness::new();
        harness.select_in_editor("成立工作专班，每月召开一次调度会。");
        let output = right_click_editor(&mut harness);
        let item = text_at(&output, |text| text == "移入候选区").unwrap();
        harness.click(item);
        harness.doc.candidates.open = true;
        harness.frame(Vec::new());
        harness.frame(Vec::new());
        let key = harness.doc.candidates.items[0].key;
        let settled = harness.rect_of(candidate_editor_id(key));
        let mut output = harness.frame(Vec::new());
        for _ in 0..30 {
            output = harness.frame(Vec::new());
        }
        // 面板内容不能比面板高：否则每帧把可拖动的底栏再撑高一点，自己往上长。
        assert_eq!(harness.rect_of(candidate_editor_id(key)), settled);
        // 底部的提示行完整画在窗口里，没有被裁掉一半。
        let hint = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(shape)
                    if shape.galley.text().starts_with("拖选任意文字") =>
                {
                    Some((shape.visual_bounding_rect(), clipped.clip_rect))
                }
                _ => None,
            })
            .expect("应画出操作提示");
        assert!(hint.1.contains_rect(hint.0), "提示行被裁掉：{hint:?}");
        assert!(hint.0.bottom() <= 900.0);
    }

    #[test]
    fn double_click_picks_a_sentence_and_inserts_only_that_part() {
        let mut harness = Harness::new();
        let picked = "成立工作专班，每月召开一次调度会。各单位确定一名联络员。";
        harness.select_in_editor(picked);
        let output = right_click_editor(&mut harness);
        let item = text_at(&output, |text| text == "移入候选区").unwrap();
        harness.click(item);
        harness.doc.candidates.open = true;
        harness.frame(Vec::new());
        harness.frame(Vec::new());

        // 正文光标放到「继续推进。」后面。
        harness.select_in_editor("");
        let text = &harness.doc.generated_markdown;
        let end = text[..text.find("继续推进。").unwrap() + "继续推进。".len()]
            .chars()
            .count();
        let mut state = egui::TextEdit::load_state(&harness.ctx, editor_id()).unwrap();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(end),
            )));
        state.store(&harness.ctx, editor_id());

        // 在候选全文的第一个字上双击：选中第一句，而不是只到逗号。
        let key = harness.doc.candidates.items[0].key;
        let rect = harness.rect_of(candidate_editor_id(key));
        let at = rect.min + egui::vec2(6.0, 8.0);
        // 隔开前面几次点击，免得 egui 把它们连成三击。
        harness.clock += 1.0;
        harness.frame(vec![egui::Event::PointerMoved(at)]);
        for pressed in [true, false, true, false] {
            harness.button(at, egui::PointerButton::Primary, pressed);
        }
        let output = harness.frame(Vec::new());
        let text = &harness.doc.candidates.items[0].record.text;
        let selected = selection_bytes(&harness.ctx, candidate_editor_id(key), text)
            .map(|range| text[range].to_string());
        assert_eq!(
            selected.as_deref(),
            Some("成立工作专班，每月召开一次调度会。")
        );
        let button = text_at(&output, |text| text.starts_with("插入选中 · "))
            .expect("选中一句后插入按钮应标出字数");
        harness.click(button);

        assert!(
            harness
                .doc
                .generated_markdown
                .contains("继续推进。成立工作专班，每月召开一次调度会。\n")
        );
        assert_eq!(
            harness.doc.candidates.items[0].record.text,
            "各单位确定一名联络员。"
        );
        let after_insert = harness.doc.generated_markdown.clone();

        // 撤销：正文与候选一起退回插入之前。
        let output = harness.frame(Vec::new());
        let undo = text_at(&output, |text| text == "撤销").expect("插入后应出现撤销入口");
        harness.click(undo);
        assert_ne!(harness.doc.generated_markdown, after_insert);
        assert!(!harness.doc.generated_markdown.contains("继续推进。成立"));
        assert_eq!(harness.doc.candidates.items[0].record.text, picked);
    }
}
