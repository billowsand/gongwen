//! 公文引用选择器：冻结插入点与来源快照，确认后一次性写正文及引用定义。

use crate::document_reference::{self, Reference, References};
use crate::draft_page::{DraftPage, diff_editor, editor_selection};
use crate::manuscript::ManuscriptFilter;
use crate::models::{ManuscriptStatus, TemplateKind};
use crate::{export, theme};
use eframe::egui;
use std::collections::BTreeMap;
use std::ops::Range;

struct SourceState {
    id: Option<i64>,
    latest: Option<Reference>,
    description: String,
}

struct Candidate {
    id: i64,
    title: String,
    number: String,
    kind: TemplateKind,
    status: ManuscriptStatus,
}

#[derive(Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Library,
    Manual,
    Current,
}

pub(crate) struct ReferencePicker {
    tab: Tab,
    search: String,
    kind: Option<TemplateKind>,
    candidates: Vec<Candidate>,
    /// 弹窗冻结期间正文若变动，拒绝用旧字节位置写入。
    baseline: String,
    selection: Range<usize>,
    selected: Option<Reference>,
    title: String,
    number: String,
    no_number: bool,
    description: String,
    error: Option<String>,
    editing: Option<String>,
    sources: BTreeMap<String, SourceState>,
}

impl ReferencePicker {
    fn select_snapshot(&mut self, reference: &Reference, editing: bool) {
        self.editing = editing.then(|| reference.id.clone());
        self.selected = Some(reference.clone());
        self.title = reference.title.clone();
        self.number = reference.number.clone();
        self.no_number = reference.no_number;
        self.error = None;
    }
}

impl DraftPage<'_> {
    pub(crate) fn open_reference_picker(&mut self, ctx: &egui::Context) {
        let baseline = self.doc.generated_markdown.clone();
        let selection = editor_selection(ctx, &baseline).unwrap_or(baseline.len()..baseline.len());
        let mut candidates = Vec::new();
        let mut error = None;
        if let Some(store) = self.store.as_deref_mut() {
            match store.list(&ManuscriptFilter::default()) {
                Ok(rows) => {
                    for row in rows {
                        if Some(row.id) == self.doc.manuscript_id {
                            continue;
                        }
                        match store.snapshot_of(row.id) {
                            Ok(Some((snapshot, _))) => candidates.push(Candidate {
                                id: row.id,
                                title: row.title,
                                kind: row.kind,
                                status: row.status,
                                number: reference_number(&snapshot),
                            }),
                            Ok(None) => {}
                            Err(err) => {
                                error = Some(format!("读取候选失败：{err}"));
                                break;
                            }
                        }
                    }
                    candidates.sort_by_key(|candidate| {
                        !matches!(
                            candidate.status,
                            ManuscriptStatus::Published | ManuscriptStatus::Archived
                        )
                    });
                }
                Err(err) => error = Some(format!("读取稿件库失败：{err}")),
            }
        }
        let mut picker = ReferencePicker {
            tab: Tab::Library,
            search: String::new(),
            kind: None,
            candidates,
            baseline,
            selection,
            selected: None,
            title: String::new(),
            number: String::new(),
            no_number: false,
            description: String::new(),
            error,
            editing: None,
            sources: BTreeMap::new(),
        };
        self.refresh_reference_sources(&mut picker);
        self.doc.reference_picker = Some(picker);
    }

    fn refresh_reference_sources(&mut self, picker: &mut ReferencePicker) {
        picker.sources.clear();
        for reference in References::read(&self.doc.generated_markdown)
            .items
            .values()
        {
            let Some(uuid) = &reference.document_uuid else {
                continue;
            };
            let state = (|| -> anyhow::Result<SourceState> {
                let store = self
                    .store
                    .as_deref_mut()
                    .ok_or_else(|| anyhow::anyhow!("稿件库未打开"))?;
                let Some(id) = store.find_document_uuid(uuid)? else {
                    return Ok(SourceState {
                        id: None,
                        latest: None,
                        description: "本机未找到来源，仍使用本篇保存的引用快照。".into(),
                    });
                };
                let (latest, description) = source_reference(store, id)?;
                let changed = latest.title != reference.title
                    || latest.number != reference.number
                    || latest.no_number != reference.no_number;
                Ok(SourceState {
                    id: Some(id),
                    latest: Some(latest),
                    description: if changed {
                        format!("来源名称或文号已变化；{description}，本篇尚未更新。")
                    } else {
                        format!("来源名称、文号一致；{description}。")
                    },
                })
            })()
            .unwrap_or_else(|error| SourceState {
                id: None,
                latest: None,
                description: format!("来源暂不可读取：{error}；引用快照仍可使用。"),
            });
            picker.sources.insert(reference.id.clone(), state);
        }
    }

    fn choose_reference_source(
        &mut self,
        picker: &mut ReferencePicker,
        id: i64,
    ) -> anyhow::Result<()> {
        let store = self
            .store
            .as_deref_mut()
            .ok_or_else(|| anyhow::anyhow!("稿件库未打开"))?;
        let (reference, description) = source_reference(store, id)?;
        picker.title = reference.title.clone();
        picker.number = reference.number.clone();
        picker.no_number = reference.no_number;
        picker.selected = Some(reference);
        picker.description = description;
        picker.error = None;
        picker.editing = None;
        Ok(())
    }

    pub(crate) fn reference_picker_modal(&mut self, ctx: &egui::Context) {
        let Some(mut picker) = self.doc.reference_picker.take() else {
            return;
        };
        let editable = !self.doc.read_only();
        let mut close = false;
        let mut insert = false;
        let mut source = None;
        let mut locate = None;
        let mut plain = None;
        let mut open_source = None;
        let mut recheck = false;
        let mut update_source = None;
        let refs = References::read(&self.doc.generated_markdown);
        let response = egui::Modal::new(egui::Id::new(("document_reference_picker", self.doc.key)))
            .frame(theme::card())
            .show(ctx, |ui| {
                self.reference_picker_contents(
                    ui,
                    &mut picker,
                    &refs,
                    editable,
                    &mut insert,
                    &mut close,
                    &mut source,
                    &mut locate,
                    &mut plain,
                    &mut open_source,
                    &mut recheck,
                    &mut update_source,
                );
            });
        if recheck {
            self.refresh_reference_sources(&mut picker);
        }
        if let Some((id, reference_id)) = update_source {
            if let Err(error) = self.choose_reference_source(&mut picker, id) {
                picker.error = Some(error.to_string());
            } else if let Some(reference) = &mut picker.selected {
                reference.id = reference_id.clone();
                picker.editing = Some(reference_id);
                picker
                    .description
                    .push_str("；核对预览后点击保存，本篇所有出现位置一起更新。");
            }
        }
        if let Some(id) = open_source {
            self.actions
                .push(crate::app::DraftAction::OpenManuscript(id));
            close = true;
        }
        if let Some(id) = source {
            let editing = picker.editing.clone();
            if let Err(error) = self.choose_reference_source(&mut picker, id) {
                picker.error = Some(error.to_string());
            } else if let Some(reference_id) = editing
                && let Some(reference) = &mut picker.selected
            {
                reference.id = reference_id.clone();
                picker.editing = Some(reference_id);
            }
        }
        if let Some(range) = locate {
            self.doc.pending_source_selection = Some(range.clone());
            self.doc.pending_source_jump = Some(range.start);
            self.doc.pending_source_reveal = true;
            if self.doc.preview_mode == super::PreviewMode::Rendered {
                self.doc.preview_mode = super::PreviewMode::Split;
            }
            close = true;
        }
        if let Some(reference) = plain
            && editable
        {
            let updated = document_reference::to_plain(&self.doc.generated_markdown, &reference);
            let cursor = document_reference::occurrences(&self.doc.generated_markdown)
                .into_iter()
                .find(|(_, id)| id == &reference.id)
                .map_or(0, |(range, _)| range.start);
            diff_editor::replace_with_undo(ctx, &mut self.doc.generated_markdown, updated, cursor);
            self.doc.pending_source_jump = Some(cursor);
            *self.status = "已将该引用的全部出现位置转为普通文字，可按 Ctrl+Z 撤销。".into();
            close = true;
        }
        if insert && editable {
            let reference = if picker.tab == Tab::Manual || picker.editing.is_some() {
                Reference::manual(&picker.title, &picker.number, picker.no_number).map(
                    |mut reference| {
                        if let Some(id) = &picker.editing {
                            reference.id = id.clone();
                            if let Some(selected) = &picker.selected
                                && selected.title == reference.title
                                && selected.number == reference.number
                                && selected.no_number == reference.no_number
                            {
                                reference.document_uuid = selected.document_uuid.clone();
                                reference.revision_uuid = selected.revision_uuid.clone();
                            }
                        }
                        reference
                    },
                )
            } else {
                picker
                    .selected
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("请先选择引用文件"))
            };
            match reference.and_then(|reference| {
                reference.validate()?;
                Ok(reference)
            }) {
                Ok(reference) if self.doc.generated_markdown == picker.baseline => {
                    let mut updated = picker.baseline.clone();
                    let cursor = if picker.editing.is_some() {
                        picker.selection.start
                    } else {
                        let token = reference.token();
                        updated.replace_range(picker.selection.clone(), &token);
                        picker.selection.start + token.len()
                    };
                    let updated = document_reference::put(&updated, &reference);
                    diff_editor::replace_with_undo(
                        ctx,
                        &mut self.doc.generated_markdown,
                        updated,
                        cursor,
                    );
                    self.doc.pending_source_jump = Some(cursor);
                    *self.status = if picker.editing.is_some() {
                        "已更新本篇引用快照，可按 Ctrl+Z 撤销。"
                    } else {
                        "已插入公文引用，可按 Ctrl+Z 撤销。"
                    }
                    .into();
                    close = true;
                }
                Ok(_) => picker.error = Some("正文已变化，请关闭后重新选择插入位置。".into()),
                Err(error) => picker.error = Some(error.to_string()),
            }
        }
        if !close && !response.should_close() {
            self.doc.reference_picker = Some(picker);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn reference_picker_contents(
        &self,
        ui: &mut egui::Ui,
        picker: &mut ReferencePicker,
        refs: &References,
        editable: bool,
        insert: &mut bool,
        close: &mut bool,
        source: &mut Option<i64>,
        locate: &mut Option<Range<usize>>,
        plain: &mut Option<Reference>,
        open_source: &mut Option<i64>,
        recheck: &mut bool,
        update_source: &mut Option<(i64, String)>,
    ) {
        ui.set_width(580.0);
        ui.heading("公文引用");
        ui.horizontal(|ui| {
            if ui
                .selectable_label(picker.tab == Tab::Library, "从稿件库选择")
                .clicked()
            {
                picker.tab = Tab::Library;
                picker.editing = None;
            }
            if ui
                .selectable_label(picker.tab == Tab::Manual, "手工登记来文")
                .clicked()
            {
                picker.tab = Tab::Manual;
                picker.editing = None;
                picker.selected = None;
                picker.title.clear();
                picker.number.clear();
                picker.no_number = false;
                picker.description =
                    "适用于外单位来文、纸质件等，请按原件核对名称和完整发文字号。".into();
            }
            if ui
                .selectable_label(
                    picker.tab == Tab::Current,
                    format!("本篇引用（{}）", refs.items.len()),
                )
                .clicked()
            {
                picker.tab = Tab::Current;
                *recheck = true;
            }
        });
        ui.separator();
        match picker.tab {
            Tab::Library => {
                ui.add(
                    egui::TextEdit::singleline(&mut picker.search)
                        .hint_text("按公文名称或完整发文字号搜索")
                        .desired_width(f32::INFINITY),
                );
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut picker.kind, None, "全部");
                    for kind in [
                        TemplateKind::OfficialLetter,
                        TemplateKind::WhitePaper,
                        TemplateKind::RedHeadApproval,
                    ] {
                        ui.selectable_value(&mut picker.kind, Some(kind), kind.label());
                    }
                });
                egui::ScrollArea::vertical()
                    .id_salt("reference_candidates")
                    .max_height(220.0)
                    .show(ui, |ui| {
                        let query = picker.search.trim().to_lowercase();
                        let mut count = 0;
                        for candidate in &picker.candidates {
                            if picker.kind.is_some_and(|kind| kind != candidate.kind)
                                || !format!("{} {}", candidate.title, candidate.number)
                                    .to_lowercase()
                                    .contains(&query)
                            {
                                continue;
                            }
                            count += 1;
                            let label = format!(
                                "{}\n{} · {} · {}",
                                candidate.title,
                                if candidate.number.is_empty() {
                                    "无文号或未编文号"
                                } else {
                                    &candidate.number
                                },
                                candidate.kind.label(),
                                candidate.status.label()
                            );
                            if ui.selectable_label(false, label).clicked() {
                                *source = Some(candidate.id);
                            }
                        }
                        if count == 0 {
                            ui.weak("没有匹配稿件，可切换到手工登记来文。");
                        }
                    });
            }
            Tab::Manual => {}
            Tab::Current => {
                if ui.button("重新核对来源").clicked() {
                    *recheck = true;
                }
                let occurrences = document_reference::occurrences(&self.doc.generated_markdown);
                egui::ScrollArea::vertical()
                    .id_salt("current_references")
                    .max_height(220.0)
                    .show(ui, |ui| {
                        for reference in refs.items.values() {
                            let positions = occurrences.iter()
                                .filter(|(_, id)| id == &reference.id)
                                .map(|(range, _)| range.clone())
                                .collect::<Vec<_>>();
                            ui.group(|ui| {
                                ui.label(reference.display());
                                if let Some(state) = picker.sources.get(&reference.id) {
                                    ui.weak(&state.description);
                                    if let Some(latest) = &state.latest
                                        && (latest.title != reference.title
                                            || latest.number != reference.number
                                            || latest.no_number != reference.no_number)
                                    {
                                        ui.colored_label(theme::warn(), format!("来源现为：{}", latest.display()));
                                    }
                                    ui.horizontal_wrapped(|ui| {
                                        if let Some(id) = state.id {
                                            if ui.button("打开来源稿件").clicked() {
                                                *open_source = Some(id);
                                            }
                                            if ui.add_enabled(editable, egui::Button::new("核对并更新引用")).clicked() {
                                                *update_source = Some((id, reference.id.clone()));
                                            }
                                        }
                                    });
                                } else {
                                    ui.weak("手工登记来文");
                                }
                                ui.horizontal_wrapped(|ui| {
                                    ui.weak(format!("使用 {} 处", positions.len()));
                                    if ui.add_enabled(editable, egui::Button::new("再次插入")).clicked() {
                                        picker.select_snapshot(reference, false);
                                        picker.description = "复用本篇已确认的引用快照".into();
                                    }
                                    if !positions.is_empty() && ui.button("定位").clicked() {
                                        *locate = positions.iter()
                                            .find(|range| range.start > picker.selection.start)
                                            .or_else(|| positions.first()).cloned();
                                    }
                                    if ui.add_enabled(editable, egui::Button::new("编辑快照")).clicked() {
                                        picker.select_snapshot(reference, true);
                                        picker.description = format!("修改将影响本篇 {} 处引用；手工改名或改号会解除来源关联。", positions.len());
                                    }
                                    if ui.add_enabled(editable, egui::Button::new("替换引用")).clicked() {
                                        picker.tab = Tab::Library;
                                        picker.select_snapshot(reference, true);
                                        picker.description = "选择另一份来源，再核对并保存；本篇所有出现位置一起替换。".into();
                                    }
                                    let label = if positions.is_empty() { "移除未使用条目" } else { "全部转为普通文字" };
                                    if ui.add_enabled(editable, egui::Button::new(label)).clicked() {
                                        *plain = Some(reference.clone());
                                    }
                                });
                            });
                        }
                        if refs.items.is_empty() {
                            ui.weak("本篇还没有登记公文引用。");
                        }
                        for issue in &refs.issues {
                            ui.colored_label(theme::warn(), &issue.message);
                        }
                    });
            }
        }
        ui.separator();
        ui.label("公文名称");
        ui.add_enabled(
            (picker.tab == Tab::Manual || picker.editing.is_some()) && editable,
            egui::TextEdit::singleline(&mut picker.title).desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            ui.label("完整发文字号");
            ui.add_enabled(
                (picker.tab == Tab::Manual || picker.editing.is_some()) && editable,
                egui::Checkbox::new(&mut picker.no_number, "该文件无文号"),
            );
        });
        ui.add_enabled(
            (picker.tab == Tab::Manual || picker.editing.is_some())
                && !picker.no_number
                && editable,
            egui::TextEdit::singleline(&mut picker.number)
                .hint_text("例如：某办函〔2026〕12号")
                .desired_width(f32::INFINITY),
        );
        ui.weak(&picker.description);
        if let Some(id) = &picker.editing
            && let Some(old) = refs.items.get(id)
        {
            ui.weak(format!("本篇当前：{}", old.display()));
        }
        if !picker.title.is_empty() {
            ui.label(format!(
                "引用预览：{}",
                if picker.no_number {
                    format!("《{}》", picker.title)
                } else {
                    format!("《{}》（{}）", picker.title, picker.number)
                }
            ));
        }
        if let Some(error) = &picker.error {
            ui.colored_label(theme::warn(), error);
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    editable && !picker.title.is_empty(),
                    egui::Button::new(if picker.editing.is_some() {
                        "保存引用修改"
                    } else if picker.selection.is_empty() {
                        "插入引用"
                    } else {
                        "替换选中文字"
                    }),
                )
                .clicked()
            {
                *insert = true;
            }
            if ui.button("关闭").clicked() {
                *close = true;
            }
        });
    }
}

fn source_reference(
    store: &mut crate::manuscript::ManuscriptStore,
    id: i64,
) -> anyhow::Result<(Reference, String)> {
    let (document_uuid, _) = store.document_identity(id)?;
    let live = store
        .snapshot_of(id)?
        .ok_or_else(|| anyhow::anyhow!("来源稿件已不存在"))?;
    let latest = store.latest_committed_revision(id)?;
    let (input, markdown, revision_uuid, description) = if let Some(latest) = latest {
        let version = store
            .get_manuscript_version(id, latest.visible_number)?
            .ok_or_else(|| anyhow::anyhow!("来源版本已不存在"))?;
        let changed = version.snapshot != live.0 || version.content_markdown != live.1;
        (
            version.snapshot,
            version.content_markdown,
            Some(latest.revision_uuid),
            format!(
                "引用最新提交版 v{}{}",
                latest.visible_number,
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
            None,
            "尚无提交版本：本次引用当前工作稿的名称、文号快照".into(),
        )
    };
    let title = export::plain_text(&export::document_title(&input, &markdown));
    let number = reference_number(&input);
    Ok((
        Reference {
            id: uuid::Uuid::new_v4().to_string(),
            title,
            number,
            no_number: !input.kind.has_document_number(),
            document_uuid: Some(document_uuid),
            revision_uuid,
        },
        description,
    ))
}

fn reference_number(input: &crate::models::DraftInput) -> String {
    if !input.kind.has_document_number() {
        return String::new();
    }
    let (code, year, serial) = export::element_display::number_display_parts(input);
    if code.is_empty() || year.is_empty() || serial.is_empty() {
        String::new()
    } else {
        format!("{code}〔{year}〕{serial}号")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manuscript::{ManuscriptStore, ManuscriptUpdate, NewManuscript};
    use crate::models::{DraftInput, TemplateProfile};

    #[test]
    fn document_reference_source_uses_committed_version_and_explicit_working_fallback() {
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
        let (working, notice) = source_reference(&mut store, id).unwrap();
        working.validate().unwrap();
        assert_eq!(working.number, "某办函〔2026〕12号");
        assert_eq!(working.title, "关于开展检查的函");
        assert!(working.revision_uuid.is_none());
        assert!(notice.contains("工作稿"));
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
        let (committed, notice) = source_reference(&mut store, id).unwrap();
        assert_eq!(committed.title, working.title);
        assert_eq!(committed.number, working.number);
        assert!(committed.revision_uuid.is_some());
        assert_eq!(committed.document_uuid, working.document_uuid);
        assert!(notice.contains("未提交修改"));
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
        let (updated, _) = source_reference(&mut store, id).unwrap();
        assert_eq!(updated.title, "关于调整检查的函");
        assert_eq!(updated.number, "某办函〔2026〕13号");
        assert_ne!(updated.revision_uuid, committed.revision_uuid);

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
        let (no_number, _) = source_reference(&mut store, no_number_id).unwrap();
        no_number.validate().unwrap();
        assert!(no_number.no_number);
        assert_eq!(no_number.display(), "《关于报请审定的请示》");
    }
}
