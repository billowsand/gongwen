//! 公文引用面板：「选择来文」帮用户把《名称》（文号）打对后插进正文；「本篇引用」
//! 从正文识别出全部引用，与稿件库、公文登记簿逐条比对，给出规范写法、改为来源写法、
//! 登记等操作。
//!
//! 正文里只有普通文字（见 `document_reference`），来源关联每次打开面板时现比对，
//! 不存任何 ID。弹窗打开时冻结正文与插入点，正文在弹窗外被改过就拒绝写入。

use crate::document_reference::{self, Citation};
use crate::draft_page::{DraftPage, diff_editor, editor_selection};
use crate::manuscript::{ManuscriptFilter, ManuscriptStore};
use crate::models::ManuscriptStatus;
use crate::{export, theme};
use anyhow::{anyhow, ensure};
use eframe::egui;
use std::ops::Range;

/// 弹窗内容宽度。
const WIDTH: f32 = 520.0;
/// 候选列表固定高度：搜索时列表变短，弹窗不跟着跳。
const LIST_HEIGHT: f32 = 248.0;
const ROW_HEIGHT: f32 = 30.0;

/// 来源在本机的位置。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SourceKey {
    Manuscript(i64),
    Registry(i64),
}

/// 一份可引用的文件：稿件库里的稿件，或公文登记簿里的一条。
pub(crate) struct Source {
    pub(crate) key: SourceKey,
    title: String,
    /// 规范后的发文字号；无文号或未编号为空。
    number: String,
    /// 「本篇引用」里的短标签。
    pub(crate) origin: String,
    /// 选中时预览下方的说明：取自哪一版等。
    note: String,
    /// 更正前的写法（只有登记簿有）。
    aliases: Vec<(String, String)>,
}

impl Source {
    pub(crate) fn text(&self) -> String {
        document_reference::display(&self.title, &self.number)
    }
}

/// 正文里一条引用与来源的比对结果（`usize` 是 `sources` 下标）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Match {
    /// 名称、文号与来源一致。
    Exact(usize),
    /// 写的是登记簿里更正前的写法。
    Outdated(usize),
    /// 文号或名称只对上一半。
    Differs(usize),
    Unregistered,
}

/// 「本篇引用」的一条：同一份文件在正文里的全部出现处。
pub(crate) struct Group {
    pub(crate) title: String,
    pub(crate) number: String,
    pub(crate) ranges: Vec<Range<usize>>,
    /// 写法不规范的出现处。
    pub(crate) nonstandard: Vec<Range<usize>>,
    pub(crate) status: Match,
}

impl Group {
    pub(crate) fn text(&self) -> String {
        document_reference::display(&self.title, &self.number)
    }

    /// 排序用：有问题的在前，未登记其次，一致的最后。
    fn rank(&self) -> u8 {
        match self.status {
            Match::Outdated(_) | Match::Differs(_) => 0,
            _ if !self.nonstandard.is_empty() => 0,
            Match::Unregistered => 1,
            Match::Exact(_) => 2,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    /// 从列表选一份文件插入。
    Browse,
    /// 手工填写名称与文号插入。
    Manual,
    /// 更正公文登记簿里的一条。
    EditRegistry(i64),
    /// 本篇已有引用。
    Current,
}

enum Action {
    Close,
    Choose(usize),
    Commit,
    Insert(String),
    Locate(Range<usize>),
    /// 把这些位置的文字换成新写法；`&str` 是状态栏里的动作名。
    Rewrite(Vec<(Range<usize>, String)>, &'static str),
    Register(Vec<(String, String)>),
    OpenSource(i64),
    EditRegistry(i64),
    DeleteRegistry(i64),
    Back,
}

pub(crate) struct ReferencePicker {
    view: View,
    search: String,
    focus_search: bool,
    index: CitationIndex,
    /// 弹窗冻结期间正文若在别处变动，拒绝用旧字节位置写入。
    baseline: String,
    selection: Range<usize>,
    /// 列表里选中的来源（`sources` 下标）。
    chosen: Option<usize>,
    title: String,
    number: String,
    no_number: bool,
    /// 手工填写时同时存入公文登记簿。
    register: bool,
    /// 删除登记要再点一次确认。
    confirm_delete: bool,
    error: Option<String>,
}

impl ReferencePicker {
    fn fill_form(&mut self, title: &str, number: &str, no_number: bool) {
        self.title = title.to_owned();
        self.number = number.to_owned();
        self.no_number = no_number;
        self.error = None;
        self.confirm_delete = false;
    }

    fn browse(&mut self) {
        self.view = View::Browse;
        self.chosen = None;
        self.fill_form("", "", false);
        self.focus_search = true;
    }

    fn form_view(&self) -> bool {
        matches!(self.view, View::Manual | View::EditRegistry(_))
    }

    /// 预览：列表视图是选中来源的写法，表单视图按当前输入（文号已规范）。
    fn preview(&self) -> Option<String> {
        if self.form_view() {
            let title = document_reference::clean_title(&self.title);
            if title.is_empty() {
                return None;
            }
            let number = if self.no_number {
                String::new()
            } else {
                document_reference::normalize_number(&self.number)
            };
            Some(document_reference::display(&title, &number))
        } else {
            self.chosen.map(|index| self.index.sources[index].text())
        }
    }
}

/// 可引用的来源清单与比对。面板和编辑器共用：编辑器拿它给正文里的引用画线、出悬停卡片。
#[derive(Default)]
pub(crate) struct CitationIndex {
    sources: Vec<Source>,
}

impl CitationIndex {
    pub(crate) fn load(store: &mut ManuscriptStore, own: Option<i64>) -> anyhow::Result<Self> {
        Ok(Self {
            sources: load_sources(store, own)?,
        })
    }

    /// 已知的无文号文件名称：只有这些单独的书名号才认作引用。
    fn untitled(&self, title: &str) -> bool {
        self.sources
            .iter()
            .any(|source| source.number.is_empty() && source.title == title)
    }

    fn citations(&self, markdown: &str) -> Vec<Citation> {
        document_reference::detect(markdown, |title| self.untitled(title))
    }

    /// 按文号为主、名称为辅比对来源。
    fn matching(&self, title: &str, number: &str) -> Match {
        let find = |predicate: &dyn Fn(&Source) -> bool| self.sources.iter().position(predicate);
        if let Some(index) = find(&|source| source.title == title && source.number == number) {
            return Match::Exact(index);
        }
        if let Some(index) = find(&|source| {
            source
                .aliases
                .iter()
                .any(|(old_title, old_number)| old_title == title && old_number == number)
        }) {
            return Match::Outdated(index);
        }
        if !number.is_empty()
            && let Some(index) = find(&|source| source.number == number)
        {
            return Match::Differs(index);
        }
        if let Some(index) = find(&|source| source.title == title && !source.number.is_empty()) {
            return Match::Differs(index);
        }
        Match::Unregistered
    }

    /// 比对上的来源；未登记为空。
    pub(crate) fn source(&self, status: Match) -> Option<&Source> {
        match status {
            Match::Exact(index) | Match::Outdated(index) | Match::Differs(index) => {
                self.sources.get(index)
            }
            Match::Unregistered => None,
        }
    }

    pub(crate) fn groups(&self, markdown: &str) -> Vec<Group> {
        let mut groups: Vec<Group> = Vec::new();
        for citation in self.citations(markdown) {
            let standard = citation.standard(markdown);
            let index = match groups.iter().position(|group| {
                group.title == citation.title && group.number == citation.normalized
            }) {
                Some(index) => index,
                None => {
                    groups.push(Group {
                        status: self.matching(&citation.title, &citation.normalized),
                        title: citation.title.clone(),
                        number: citation.normalized.clone(),
                        ranges: Vec::new(),
                        nonstandard: Vec::new(),
                    });
                    groups.len() - 1
                }
            };
            if !standard {
                groups[index].nonstandard.push(citation.range.clone());
            }
            groups[index].ranges.push(citation.range);
        }
        groups.sort_by_key(Group::rank);
        groups
    }
}

impl DraftPage<'_> {
    pub(crate) fn open_reference_picker(&mut self, ctx: &egui::Context) {
        let baseline = self.doc.generated_markdown.clone();
        let selection = editor_selection(ctx, &baseline).unwrap_or(baseline.len()..baseline.len());
        let own = self.doc.manuscript_id;
        let (index, error) = match self.store.as_deref_mut() {
            Some(store) => match CitationIndex::load(store, own) {
                Ok(index) => (index, None),
                Err(err) => (
                    CitationIndex::default(),
                    Some(format!("读取稿件库失败：{err}")),
                ),
            },
            None => (CitationIndex::default(), None),
        };
        let mut picker = ReferencePicker {
            view: View::Browse,
            search: String::new(),
            focus_search: true,
            index,
            baseline,
            selection,
            chosen: None,
            title: String::new(),
            number: String::new(),
            no_number: false,
            register: true,
            confirm_delete: false,
            error,
        };
        // 正文里已有引用、又没选中文字时，多半是来核对的，直接看「本篇引用」。
        if picker.selection.is_empty() && !picker.index.citations(&picker.baseline).is_empty() {
            picker.view = View::Current;
        }
        self.doc.reference_picker = Some(picker);
    }

    fn picker_store(&mut self) -> anyhow::Result<&mut ManuscriptStore> {
        self.store
            .as_deref_mut()
            .ok_or_else(|| anyhow!("稿件库未打开"))
    }

    fn reload_sources(&mut self, picker: &mut ReferencePicker) -> anyhow::Result<()> {
        let own = self.doc.manuscript_id;
        let chosen = picker.chosen.map(|index| picker.index.sources[index].key);
        picker.index = CitationIndex::load(self.picker_store()?, own)?;
        picker.chosen = chosen.and_then(|key| {
            picker
                .index
                .sources
                .iter()
                .position(|source| source.key == key)
        });
        Ok(())
    }

    /// 在冻结的插入点写入引用文字（有选区则替换选区）。
    fn insert_reference(
        &mut self,
        ctx: &egui::Context,
        picker: &ReferencePicker,
        text: &str,
    ) -> anyhow::Result<()> {
        ensure!(
            self.doc.generated_markdown == picker.baseline,
            "正文已变化，请关闭后重新选择插入位置。"
        );
        let mut updated = picker.baseline.clone();
        updated.replace_range(picker.selection.clone(), text);
        let cursor = picker.selection.start + text.len();
        diff_editor::replace_with_undo(ctx, &mut self.doc.generated_markdown, updated, cursor);
        self.doc.pending_source_jump = Some(cursor);
        *self.status = "已插入公文引用，可按 Ctrl+Z 撤销。".into();
        Ok(())
    }

    /// 改写正文里若干处引用。弹窗不关，冻结的正文与插入点随之更新，便于连着处理下一条。
    fn rewrite_references(
        &mut self,
        ctx: &egui::Context,
        picker: &mut ReferencePicker,
        mut edits: Vec<(Range<usize>, String)>,
    ) -> anyhow::Result<usize> {
        ensure!(
            self.doc.generated_markdown == picker.baseline,
            "正文已变化，请关闭后重新打开公文引用。"
        );
        edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
        let mut updated = picker.baseline.clone();
        let mut shift = 0isize;
        for (range, text) in &edits {
            if range.end <= picker.selection.start {
                shift += text.len() as isize - range.len() as isize;
            }
            updated.replace_range(range.clone(), text);
        }
        let move_by = |at: usize| at.saturating_add_signed(shift).min(updated.len());
        picker.selection = move_by(picker.selection.start)..move_by(picker.selection.end);
        let cursor = edits.last().map_or(0, |(range, _)| range.start);
        diff_editor::replace_with_undo(
            ctx,
            &mut self.doc.generated_markdown,
            updated.clone(),
            cursor,
        );
        picker.baseline = updated;
        Ok(edits.len())
    }

    /// 主按钮。返回 `true` 表示完成、关闭弹窗。
    fn commit_reference(
        &mut self,
        ctx: &egui::Context,
        picker: &mut ReferencePicker,
        editable: bool,
    ) -> anyhow::Result<bool> {
        match picker.view {
            View::EditRegistry(id) => {
                self.picker_store()?.update_registered(
                    id,
                    &picker.title,
                    &picker.number,
                    picker.no_number,
                )?;
                self.reload_sources(picker)?;
                picker.view = View::Browse;
                picker.chosen = picker
                    .index
                    .sources
                    .iter()
                    .position(|source| source.key == SourceKey::Registry(id));
                *self.status = "已更正登记；引用了旧写法的文稿会在「本篇引用」里提示更新。".into();
                Ok(false)
            }
            View::Browse => {
                ensure!(editable, "只读稿件不能插入引用");
                let index = picker
                    .chosen
                    .ok_or_else(|| anyhow!("请先在列表里选一份文件"))?;
                let text = picker.index.sources[index].text();
                self.insert_reference(ctx, picker, &text)?;
                Ok(true)
            }
            View::Manual => {
                ensure!(editable, "只读稿件不能插入引用");
                ensure!(
                    self.doc.generated_markdown == picker.baseline,
                    "正文已变化，请关闭后重新选择插入位置。"
                );
                let (title, number) =
                    document_reference::validate(&picker.title, &picker.number, picker.no_number)?;
                if picker.register {
                    self.picker_store()?
                        .register_document(&title, &number, number.is_empty())?;
                }
                let text = document_reference::display(&title, &number);
                self.insert_reference(ctx, picker, &text)?;
                Ok(true)
            }
            View::Current => Ok(false),
        }
    }

    pub(crate) fn reference_picker_modal(&mut self, ctx: &egui::Context) {
        let Some(mut picker) = self.doc.reference_picker.take() else {
            return;
        };
        let editable = !self.doc.read_only();
        let mut actions = Vec::new();
        let response = egui::Modal::new(egui::Id::new(("document_reference_picker", self.doc.key)))
            .frame(theme::card().inner_margin(egui::Margin::same(18)))
            .show(ctx, |ui| {
                reference_picker_contents(ui, &mut picker, editable, &mut actions);
            });
        let mut close = response.should_close();
        for action in actions {
            let result = match action {
                Action::Close => {
                    close = true;
                    Ok(())
                }
                Action::Choose(index) => {
                    picker.chosen = Some(index);
                    picker.error = None;
                    picker.confirm_delete = false;
                    Ok(())
                }
                Action::Commit => self
                    .commit_reference(ctx, &mut picker, editable)
                    .map(|done| close |= done),
                Action::Insert(text) => self
                    .insert_reference(ctx, &picker, &text)
                    .map(|()| close = true),
                Action::Locate(range) => {
                    self.doc.pending_source_selection = Some(range.clone());
                    self.doc.pending_source_jump = Some(range.start);
                    self.doc.pending_source_reveal = true;
                    if self.doc.preview_mode == super::PreviewMode::Rendered {
                        self.doc.preview_mode = super::PreviewMode::Split;
                    }
                    close = true;
                    Ok(())
                }
                Action::Rewrite(edits, label) => self
                    .rewrite_references(ctx, &mut picker, edits)
                    .map(|count| {
                        *self.status = format!("已{label} {count} 处引用，可按 Ctrl+Z 撤销。");
                    }),
                Action::Register(items) => (|| {
                    let store = self.picker_store()?;
                    for (title, number) in &items {
                        store.register_document(title, number, number.is_empty())?;
                    }
                    self.reload_sources(&mut picker)?;
                    *self.status = format!("已登记 {} 份文件到公文登记簿。", items.len());
                    Ok(())
                })(),
                Action::OpenSource(id) => {
                    self.actions
                        .push(crate::app::DraftAction::OpenManuscript(id));
                    close = true;
                    Ok(())
                }
                Action::EditRegistry(id) => self.picker_store().and_then(|store| {
                    let document = store
                        .get_registered(id)?
                        .ok_or_else(|| anyhow!("登记已不存在"))?;
                    picker.view = View::EditRegistry(id);
                    let no_number = document.number.is_empty();
                    picker.fill_form(&document.title, &document.number, no_number);
                    Ok(())
                }),
                Action::DeleteRegistry(id) => (|| {
                    self.picker_store()?.delete_registered(id)?;
                    picker.chosen = None;
                    self.reload_sources(&mut picker)?;
                    picker.browse();
                    *self.status = "已删除登记；正文里的引用文字不受影响。".into();
                    Ok(())
                })(),
                Action::Back => {
                    let chosen = match picker.view {
                        View::EditRegistry(id) => picker
                            .index
                            .sources
                            .iter()
                            .position(|source| source.key == SourceKey::Registry(id)),
                        _ => None,
                    };
                    picker.browse();
                    picker.chosen = chosen;
                    Ok(())
                }
            };
            if let Err(error) = result {
                picker.error = Some(error.to_string());
            }
        }
        if close {
            // 面板里刚读过稿件库、可能还登记过，编辑框直接用这份清单。
            self.doc.citation_index = Some(picker.index);
        } else {
            self.doc.reference_picker = Some(picker);
        }
    }
}

fn reference_picker_contents(
    ui: &mut egui::Ui,
    picker: &mut ReferencePicker,
    editable: bool,
    actions: &mut Vec<Action>,
) {
    ui.set_width(WIDTH);
    let groups = picker.index.groups(&picker.baseline);
    let heading = match picker.view {
        View::EditRegistry(_) => "更正登记",
        _ => "公文引用",
    };
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(heading)
                .size(theme::font_sizes::HEADING - 1.0)
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            theme::segmented(ui, |ui| {
                // 右到左排布：先加的在最右。
                let current = picker.view == View::Current;
                if ui
                    .selectable_label(current, format!("本篇引用 {}", groups.len()))
                    .clicked()
                    && !current
                {
                    picker.view = View::Current;
                    picker.error = None;
                }
                if ui.selectable_label(!current, "选择来文").clicked() && current {
                    picker.browse();
                }
            });
        });
    });
    ui.add_space(12.0);

    match picker.view {
        View::Current => {
            current_ui(ui, picker, &groups, editable, actions);
            return;
        }
        View::Browse => browse_ui(ui, picker, actions),
        View::Manual | View::EditRegistry(_) => {
            if picker.view == View::Manual {
                if ui.add(egui::Link::new(small("← 从列表选择"))).clicked() {
                    picker.browse();
                }
                ui.add_space(8.0);
            }
            let enabled = editable || matches!(picker.view, View::EditRegistry(_));
            ui.add_enabled_ui(enabled, |ui| form_ui(ui, picker));
        }
    }
    footer_ui(ui, picker, editable, actions);
}

fn list_frame() -> egui::Frame {
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, theme::border()))
        .corner_radius(theme::chrome_radius(theme::PANE_RADIUS))
        .inner_margin(egui::Margin::same(4))
}

fn small(text: &str) -> egui::RichText {
    egui::RichText::new(text).size(theme::font_sizes::SMALL)
}

fn browse_ui(ui: &mut egui::Ui, picker: &mut ReferencePicker, actions: &mut Vec<Action>) {
    let search = ui.add(theme::field(
        &mut picker.search,
        "搜索名称或文号",
        f32::INFINITY,
    ));
    if std::mem::take(&mut picker.focus_search) {
        search.request_focus();
    }
    ui.add_space(8.0);
    list_frame().show(ui, |ui| {
        egui::ScrollArea::vertical()
            .id_salt("reference_candidates")
            .auto_shrink([false, false])
            .min_scrolled_height(LIST_HEIGHT)
            .max_height(LIST_HEIGHT)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                let query = picker.search.trim().to_lowercase();
                let mut count = 0;
                for (index, source) in picker.index.sources.iter().enumerate() {
                    if !query.is_empty()
                        && !format!("{} {}", source.title, source.number)
                            .to_lowercase()
                            .contains(&query)
                    {
                        continue;
                    }
                    count += 1;
                    let tag = matches!(source.key, SourceKey::Registry(_)).then_some("登记簿");
                    let response = source_row(
                        ui,
                        picker.chosen == Some(index),
                        &source.title,
                        &source.number,
                        tag,
                    );
                    if response.clicked() {
                        actions.push(Action::Choose(index));
                    }
                    if response.double_clicked() {
                        actions.push(Action::Commit);
                    }
                }
                if count == 0 {
                    ui.add_space(LIST_HEIGHT / 2.0 - 20.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new(if picker.index.sources.is_empty() {
                                "稿件库和公文登记簿里还没有可引用的文件"
                            } else {
                                "没有匹配的文件"
                            })
                            .color(theme::text_muted()),
                        );
                    });
                }
            });
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui
            .add(egui::Link::new(small("列表里没有？手工填写")))
            .clicked()
        {
            // 搜不到时多半刚输了名称，带过去省得再打一遍。
            let search = picker.search.trim().to_owned();
            picker.browse();
            picker.view = View::Manual;
            picker.title = search;
            picker.register = true;
        }
        let chosen = picker.chosen.map(|index| picker.index.sources[index].key);
        if let Some(SourceKey::Registry(id)) = chosen {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if picker.confirm_delete {
                    if ui
                        .add(egui::Link::new(small("确认删除").color(theme::danger())))
                        .clicked()
                    {
                        actions.push(Action::DeleteRegistry(id));
                    }
                } else if ui
                    .add(egui::Link::new(small("删除登记")))
                    .on_hover_text("从公文登记簿删除；正文里的引用文字不受影响")
                    .clicked()
                {
                    picker.confirm_delete = true;
                }
                if ui.add(egui::Link::new(small("更正登记"))).clicked() {
                    actions.push(Action::EditRegistry(id));
                }
            });
        }
    });
}

fn form_ui(ui: &mut egui::Ui, picker: &mut ReferencePicker) {
    let width = ui.available_width() - 72.0;
    egui::Grid::new("reference_form")
        .num_columns(2)
        .spacing([12.0, 10.0])
        .show(ui, |ui| {
            ui.label("公文名称");
            ui.add(theme::field(&mut picker.title, "不用加书名号", width));
            ui.end_row();

            ui.label("发文字号");
            let number = ui.add_enabled(
                !picker.no_number,
                theme::field(&mut picker.number, "某办函[2026]12号，括号可省略", width),
            );
            if number.lost_focus() {
                picker.number = document_reference::normalize_number(&picker.number);
            }
            ui.end_row();

            ui.label("");
            ui.checkbox(&mut picker.no_number, "该文件没有发文字号");
            ui.end_row();
        });
    if picker.view == View::Manual {
        ui.add_space(4.0);
        ui.checkbox(&mut picker.register, "存入公文登记簿，其他文稿也能直接选用");
    }
}

fn footer_ui(
    ui: &mut egui::Ui,
    picker: &ReferencePicker,
    editable: bool,
    actions: &mut Vec<Action>,
) {
    ui.add_space(14.0);
    theme::hairline(ui);
    ui.add_space(10.0);
    let registry = matches!(picker.view, View::EditRegistry(_));
    let (label, empty) = match picker.view {
        View::EditRegistry(_) => ("登记为", "填写名称后在这里预览"),
        View::Manual => ("将插入", "填写名称后在这里预览"),
        _ => ("将插入", "在列表里选一份文件"),
    };
    theme::caption(ui, label);
    let preview = picker.preview();
    match &preview {
        Some(text) => {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(text)
                        .font(egui::FontId::new(
                            17.0,
                            theme::official_family(theme::FONT_FANGSONG),
                        ))
                        .color(theme::text()),
                )
                .wrap(),
            );
        }
        None => {
            ui.label(egui::RichText::new(empty).color(theme::text_muted()));
        }
    }
    let note = match picker.view {
        View::Browse => picker
            .chosen
            .map(|index| picker.index.sources[index].note.as_str()),
        View::EditRegistry(_) => Some("更正后，引用了旧写法的文稿会提示更新，确认后才改正文。"),
        _ => None,
    };
    if let Some(note) = note {
        ui.add_space(2.0);
        theme::caption(ui, note);
    }
    if let Some(error) = &picker.error {
        ui.add_space(4.0);
        ui.colored_label(theme::danger(), error);
    }
    ui.add_space(14.0);
    let (primary, icon) = if registry {
        ("保存登记", theme::Icon::Save)
    } else if picker.selection.is_empty() {
        ("插入", theme::Icon::Quote)
    } else {
        ("替换选中文字", theme::Icon::Quote)
    };
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let enabled = preview.is_some() && (editable || registry);
            if theme::primary_icon_button_enabled(ui, enabled, icon, primary).clicked() {
                actions.push(Action::Commit);
            }
            // 更正登记是二级步骤，次按钮退回列表；其余关闭弹窗。
            if registry {
                if ui.button("返回").clicked() {
                    actions.push(Action::Back);
                }
            } else if ui.button("取消").clicked() {
                actions.push(Action::Close);
            }
        });
    });
}

fn current_ui(
    ui: &mut egui::Ui,
    picker: &ReferencePicker,
    groups: &[Group],
    editable: bool,
    actions: &mut Vec<Action>,
) {
    if groups.is_empty() {
        ui.allocate_ui(egui::vec2(WIDTH, 150.0), |ui| {
            ui.centered_and_justified(|ui| {
                ui.label(
                    egui::RichText::new(
                        "正文里还没有公文引用。\n直接写《名称》（发文字号）就能识别，\
                         也可以到「选择来文」里选一份插入。",
                    )
                    .color(theme::text_muted()),
                );
            });
        });
    } else {
        let nonstandard = groups
            .iter()
            .flat_map(|group| {
                let text = group.text();
                group
                    .nonstandard
                    .iter()
                    .map(move |range| (range.clone(), text.clone()))
            })
            .collect::<Vec<_>>();
        let unregistered = groups
            .iter()
            .filter(|group| group.status == Match::Unregistered && !group.number.is_empty())
            .map(|group| (group.title.clone(), group.number.clone()))
            .collect::<Vec<_>>();
        if editable && (nonstandard.len() > 1 || unregistered.len() > 1) {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if unregistered.len() > 1
                        && ui
                            .add(egui::Link::new(small(&format!(
                                "全部登记（{}）",
                                unregistered.len()
                            ))))
                            .clicked()
                    {
                        actions.push(Action::Register(unregistered.clone()));
                    }
                    if nonstandard.len() > 1
                        && ui
                            .add(egui::Link::new(small(&format!(
                                "全部改为规范写法（{}）",
                                nonstandard.len()
                            ))))
                            .clicked()
                    {
                        actions.push(Action::Rewrite(nonstandard.clone(), "规范"));
                    }
                });
            });
            ui.add_space(4.0);
        }
        list_frame().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("current_references")
                .auto_shrink([false, true])
                .max_height(340.0)
                .show(ui, |ui| {
                    for (index, group) in groups.iter().enumerate() {
                        if index > 0 {
                            theme::hairline(ui);
                        }
                        ui.push_id(index, |ui| {
                            current_row(ui, picker, group, editable, actions);
                        });
                    }
                });
        });
    }
    if let Some(error) = &picker.error {
        ui.add_space(6.0);
        ui.colored_label(theme::danger(), error);
    }
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("关闭").clicked() {
                actions.push(Action::Close);
            }
        });
    });
}

/// 「本篇引用」的一行：引用文字、来源标签、出现处数、定位 / 再次插入；
/// 有要处理的问题才多一行说明和操作。
fn current_row(
    ui: &mut egui::Ui,
    picker: &ReferencePicker,
    group: &Group,
    editable: bool,
    actions: &mut Vec<Action>,
) {
    let text = group.text();
    let source = picker.index.source(group.status);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if editable && theme::icon_button(ui, theme::Icon::Quote, "在光标处再次插入").clicked()
            {
                actions.push(Action::Insert(text.clone()));
            }
            if theme::icon_button(ui, theme::Icon::Reveal, "定位到正文").clicked() {
                actions.push(Action::Locate(
                    group
                        .ranges
                        .iter()
                        .find(|range| range.start > picker.selection.start)
                        .or_else(|| group.ranges.first())
                        .cloned()
                        .unwrap_or_default(),
                ));
            }
            ui.add_space(6.0);
            theme::caption(ui, &format!("{} 处", group.ranges.len()));
            ui.add_space(6.0);
            let (label, fg, bg) = match group.status {
                Match::Exact(_) => (
                    source.map_or("", |source| source.origin.as_str()),
                    theme::text_soft(),
                    theme::surface_sunk(),
                ),
                Match::Outdated(_) => ("来源已变", theme::warn(), theme::warn_soft()),
                Match::Differs(_) => ("与来源不一致", theme::warn(), theme::warn_soft()),
                Match::Unregistered => ("未登记", theme::text_muted(), theme::surface_sunk()),
            };
            theme::chip(ui, label, fg, bg);
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(&text).truncate());
            });
        });
    });
    // 每条问题一行：说明文字 + 一个操作。
    let mut problems: Vec<(String, &str, Action, bool)> = Vec::new();
    if !group.nonstandard.is_empty() {
        problems.push((
            format!("有 {} 处写法不规范", group.nonstandard.len()),
            "改为规范写法",
            Action::Rewrite(
                group
                    .nonstandard
                    .iter()
                    .map(|range| (range.clone(), text.clone()))
                    .collect(),
                "规范",
            ),
            true,
        ));
    }
    if let (Some(source), Match::Outdated(_) | Match::Differs(_)) = (source, group.status) {
        let lead = if matches!(group.status, Match::Outdated(_)) {
            "登记已更正为"
        } else {
            "来源为"
        };
        problems.push((
            format!("{lead} {}", source.text()),
            "改为来源写法",
            Action::Rewrite(
                group
                    .ranges
                    .iter()
                    .map(|range| (range.clone(), source.text()))
                    .collect(),
                "更新",
            ),
            true,
        ));
    }
    if group.status == Match::Unregistered && !group.number.is_empty() {
        problems.push((
            "稿件库和公文登记簿里都没有".into(),
            "登记",
            Action::Register(vec![(group.title.clone(), group.number.clone())]),
            false,
        ));
    }
    let open = match source.map(|source| source.key) {
        Some(SourceKey::Manuscript(id)) => Some(id),
        _ => None,
    };
    for (index, (message, label, action, warning)) in problems.into_iter().enumerate() {
        ui.horizontal_wrapped(|ui| {
            ui.add_space(6.0);
            let color = if warning {
                theme::warn()
            } else {
                theme::text_muted()
            };
            ui.label(small(&message).color(color));
            if editable && ui.add(egui::Link::new(small(label))).clicked() {
                actions.push(action);
            }
            if index == 0
                && let Some(id) = open
                && ui.add(egui::Link::new(small("打开来源"))).clicked()
            {
                actions.push(Action::OpenSource(id));
            }
        });
    }
    ui.add_space(4.0);
}

/// 候选列表的一行：整行可点，只显示插入后的样子，登记簿的条目右侧标出来。
fn source_row(
    ui: &mut egui::Ui,
    selected: bool,
    title: &str,
    number: &str,
    tag: Option<&str>,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_HEIGHT),
        egui::Sense::click(),
    );
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if !ui.is_rect_visible(rect) {
        return response;
    }
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
        let bar = egui::Rect::from_min_size(
            rect.min + egui::vec2(0.0, 6.0),
            egui::vec2(3.0, rect.height() - 12.0),
        );
        painter.rect_filled(bar, 1.5, theme::accent());
    }
    let mut right = rect.right() - 10.0;
    if let Some(tag) = tag {
        let galley = painter.layout_no_wrap(
            tag.to_owned(),
            egui::FontId::proportional(theme::font_sizes::SMALL),
            theme::text_muted(),
        );
        right -= galley.size().x;
        painter.galley(
            egui::pos2(right, rect.center().y - galley.size().y / 2.0),
            galley,
            theme::text_muted(),
        );
        right -= 12.0;
    }
    let font = egui::FontId::proportional(theme::font_sizes::BODY);
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &format!("《{title}》"),
        0.0,
        egui::TextFormat::simple(font.clone(), theme::text()),
    );
    if !number.is_empty() {
        job.append(
            &format!("（{number}）"),
            0.0,
            egui::TextFormat::simple(font, theme::text_muted()),
        );
    }
    let left = rect.left() + 12.0;
    job.wrap = egui::text::TextWrapping {
        max_width: (right - left).max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = painter.layout_job(job);
    let elided = galley.elided;
    painter.galley(
        egui::pos2(left, rect.center().y - galley.size().y / 2.0),
        galley,
        theme::text(),
    );
    if elided {
        return response.on_hover_text(document_reference::display(title, number));
    }
    response
}

/// 可引用的文件：登记簿在前（专为引用而登记），稿件按已发布 / 已归档优先。
fn load_sources(store: &mut ManuscriptStore, own: Option<i64>) -> anyhow::Result<Vec<Source>> {
    let mut sources = store
        .list_registry()?
        .into_iter()
        .map(|document| Source {
            key: SourceKey::Registry(document.id),
            title: document.title,
            number: document.number,
            origin: "登记簿".into(),
            note: "取自公文登记簿".into(),
            aliases: document.aliases,
        })
        .collect::<Vec<_>>();
    let mut manuscripts = Vec::new();
    for row in store.list(&ManuscriptFilter::default())? {
        if Some(row.id) == own {
            continue;
        }
        let formal = matches!(
            row.status,
            ManuscriptStatus::Published | ManuscriptStatus::Archived
        );
        if let Some(source) = manuscript_source(store, row.id)? {
            manuscripts.push((!formal, source));
        }
    }
    manuscripts.sort_by_key(|(draft, _)| *draft);
    sources.extend(manuscripts.into_iter().map(|(_, source)| source));
    Ok(sources)
}

/// 稿件作为来源：默认取最新提交版，没有提交版时取工作稿并说明。
fn manuscript_source(store: &mut ManuscriptStore, id: i64) -> anyhow::Result<Option<Source>> {
    let Some(live) = store.snapshot_of(id)? else {
        return Ok(None);
    };
    let latest = store.latest_committed_revision(id)?;
    let (input, markdown, origin, note) = if let Some(latest) = latest {
        let version = store
            .get_manuscript_version(id, latest.visible_number)?
            .ok_or_else(|| anyhow!("来源版本已不存在"))?;
        let changed = version.snapshot != live.0 || version.content_markdown != live.1;
        let number = latest.visible_number;
        (
            version.snapshot,
            version.content_markdown,
            format!("稿件 v{number}"),
            format!(
                "取自最新提交版 v{number}{}",
                if changed {
                    "；来源有未提交修改，本次不采用"
                } else {
                    ""
                }
            ),
        )
    } else {
        (
            live.0,
            live.1,
            "稿件 工作稿".into(),
            "尚无提交版本：取当前工作稿的名称与文号".into(),
        )
    };
    let title = document_reference::clean_title(&export::plain_text(&export::document_title(
        &input, &markdown,
    )));
    if title.is_empty() {
        return Ok(None);
    }
    Ok(Some(Source {
        key: SourceKey::Manuscript(id),
        title,
        number: reference_number(&input),
        origin,
        note,
        aliases: Vec::new(),
    }))
}

fn reference_number(input: &crate::models::DraftInput) -> String {
    if !input.kind.has_document_number() {
        return String::new();
    }
    let (code, year, serial) = export::element_display::number_display_parts(input);
    if code.is_empty() || year.is_empty() || serial.is_empty() {
        String::new()
    } else {
        document_reference::normalize_number(&format!("{code}〔{year}〕{serial}号"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manuscript::{ManuscriptStore, ManuscriptUpdate, NewManuscript};
    use crate::models::{DraftInput, TemplateKind, TemplateProfile};

    fn source(key: SourceKey, title: &str, number: &str, aliases: &[(&str, &str)]) -> Source {
        Source {
            key,
            title: title.into(),
            number: number.into(),
            origin: String::new(),
            note: String::new(),
            aliases: aliases
                .iter()
                .map(|(title, number)| ((*title).into(), (*number).into()))
                .collect(),
        }
    }

    #[test]
    fn citations_group_and_match_sources_by_number_then_title() {
        let index = CitationIndex {
            sources: vec![
                source(
                    SourceKey::Registry(1),
                    "关于调整安全检查的函",
                    "某办函〔2026〕13号",
                    &[("关于开展安全检查的函", "某办函〔2026〕12号")],
                ),
                source(
                    SourceKey::Manuscript(7),
                    "建设实施方案",
                    "项办函〔2026〕56号",
                    &[],
                ),
                source(SourceKey::Manuscript(8), "关于报请审定的请示", "", &[]),
            ],
        };
        let markdown = "根据《建设实施方案》（项办函〔2026〕56号）和《建设实施方案》(项办函[2026]56号)，\
                        参照《关于开展安全检查的函》（某办函〔2026〕12号）、\
                        《建设方案》（项办函〔2026〕56号）、《新来文》（某局函〔2026〕1号），\
                        报《关于报请审定的请示》，学习《保守国家秘密法》。";
        let groups = index.groups(markdown);
        let find = |text: &str| {
            groups
                .iter()
                .find(|group| group.text() == text)
                .unwrap_or_else(|| panic!("缺少 {text}"))
        };
        assert_eq!(groups.len(), 5, "法律名称不认作引用");
        let plan = find("《建设实施方案》（项办函〔2026〕56号）");
        assert_eq!(plan.ranges.len(), 2);
        assert_eq!(plan.nonstandard.len(), 1);
        assert_eq!(plan.status, Match::Exact(1));
        assert_eq!(
            find("《关于开展安全检查的函》（某办函〔2026〕12号）").status,
            Match::Outdated(0)
        );
        assert_eq!(
            find("《建设方案》（项办函〔2026〕56号）").status,
            Match::Differs(1)
        );
        assert_eq!(
            find("《新来文》（某局函〔2026〕1号）").status,
            Match::Unregistered
        );
        assert_eq!(find("《关于报请审定的请示》").status, Match::Exact(2));
        // 有问题的排前面，一致且规范的排最后。
        let last = groups.last().unwrap();
        assert!(last.nonstandard.is_empty());
        assert!(matches!(last.status, Match::Exact(_)));
    }

    #[test]
    fn manuscript_source_uses_committed_version_and_explicit_working_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ManuscriptStore::open(&dir.path().join("source.db")).unwrap();
        let mut input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            profile: TemplateProfile::for_kind(TemplateKind::OfficialLetter),
            ..Default::default()
        };
        input.profile.department_code = "某办函".into();
        input.profile.document_year = "2026".into();
        input.profile.document_number = "12".into();
        let markdown = "# 关于开展检查的函\n\n正文。";
        let id = store
            .create(
                &NewManuscript {
                    snapshot: input.clone(),
                    content_markdown: markdown.into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let working = manuscript_source(&mut store, id).unwrap().unwrap();
        assert_eq!(working.text(), "《关于开展检查的函》（某办函〔2026〕12号）");
        assert!(working.note.contains("工作稿"));
        store
            .commit_manuscript_version(id, "提交", "", &input, markdown, "")
            .unwrap();
        input.profile.document_number = "13".into();
        store
            .update(
                id,
                &ManuscriptUpdate {
                    snapshot: input.clone(),
                    content_markdown: "# 关于调整检查的函\n\n修改。".into(),
                    notes: String::new(),
                },
            )
            .unwrap();
        let committed = manuscript_source(&mut store, id).unwrap().unwrap();
        assert_eq!(committed.text(), working.text());
        assert_eq!(committed.origin, "稿件 v1");
        assert!(committed.note.contains("未提交修改"));
        let latest = store.get(id).unwrap().unwrap();
        store
            .commit_manuscript_version(
                id,
                "再提交",
                "",
                &latest.snapshot,
                &latest.content_markdown,
                "",
            )
            .unwrap();
        let updated = manuscript_source(&mut store, id).unwrap().unwrap();
        assert_eq!(updated.text(), "《关于调整检查的函》（某办函〔2026〕13号）");

        input.kind = TemplateKind::WhitePaper;
        input.profile = TemplateProfile::for_kind(input.kind);
        let no_number_id = store
            .create(
                &NewManuscript {
                    snapshot: input,
                    content_markdown: "# 关于报请审定的请示\n\n正文。".into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let no_number = manuscript_source(&mut store, no_number_id)
            .unwrap()
            .unwrap();
        assert_eq!(no_number.text(), "《关于报请审定的请示》");
    }
}
