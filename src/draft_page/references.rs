//! 公文引用选择器：冻结插入点与来源快照，确认后一次性写正文及引用定义。

use crate::document_reference::{self, Reference, References};
use crate::draft_page::{DraftPage, diff_editor, editor_selection};
use crate::manuscript::ManuscriptFilter;
use crate::models::{ManuscriptStatus, TemplateKind};
use crate::{export, theme};
use eframe::egui;
use std::ops::Range;

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
        self.doc.reference_picker = Some(ReferencePicker {
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
        });
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
        picker.title = title.clone();
        picker.number = number.clone();
        picker.no_number = !input.kind.has_document_number();
        picker.selected = Some(Reference {
            id: uuid::Uuid::new_v4().to_string(),
            title,
            number,
            no_number: picker.no_number,
            document_uuid: Some(document_uuid),
            revision_uuid,
        });
        picker.description = description;
        picker.error = None;
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
        let refs = References::read(&self.doc.generated_markdown);
        let response = egui::Modal::new(egui::Id::new(("document_reference_picker", self.doc.key)))
            .frame(theme::card())
            .show(ctx, |ui| {
                ui.set_width(580.0);
                ui.heading("公文引用");
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(picker.tab == Tab::Library, "从稿件库选择")
                        .clicked()
                    {
                        picker.tab = Tab::Library;
                    }
                    if ui
                        .selectable_label(picker.tab == Tab::Manual, "手工登记来文")
                        .clicked()
                    {
                        picker.tab = Tab::Manual;
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
                                        source = Some(candidate.id);
                                    }
                                }
                                if count == 0 {
                                    ui.weak("没有匹配稿件，可切换到手工登记来文。");
                                }
                            });
                    }
                    Tab::Manual => {}
                    Tab::Current => {
                        egui::ScrollArea::vertical()
                            .id_salt("current_references")
                            .max_height(220.0)
                            .show(ui, |ui| {
                                for reference in refs.items.values() {
                                    let positions = document_reference::occurrences(
                                        &self.doc.generated_markdown,
                                    )
                                    .into_iter()
                                    .filter(|(_, id)| id == &reference.id)
                                    .map(|(range, _)| range)
                                    .collect::<Vec<_>>();
                                    ui.group(|ui| {
                                        ui.label(reference.display());
                                        ui.horizontal(|ui| {
                                            ui.weak(format!("使用 {} 处", positions.len()));
                                            if ui.button("再次插入").clicked() {
                                                picker.selected = Some(reference.clone());
                                                picker.title = reference.title.clone();
                                                picker.number = reference.number.clone();
                                                picker.no_number = reference.no_number;
                                                picker.description =
                                                    "复用本篇已确认的引用快照".into();
                                            }
                                            if !positions.is_empty() && ui.button("定位").clicked()
                                            {
                                                locate = positions.first().cloned();
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
                    picker.tab == Tab::Manual,
                    egui::TextEdit::singleline(&mut picker.title).desired_width(f32::INFINITY),
                );
                ui.horizontal(|ui| {
                    ui.label("完整发文字号");
                    ui.add_enabled(
                        picker.tab == Tab::Manual,
                        egui::Checkbox::new(&mut picker.no_number, "该文件无文号"),
                    );
                });
                ui.add_enabled(
                    picker.tab == Tab::Manual && !picker.no_number,
                    egui::TextEdit::singleline(&mut picker.number)
                        .hint_text("例如：某办函〔2026〕12号")
                        .desired_width(f32::INFINITY),
                );
                ui.weak(&picker.description);
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
                            egui::Button::new(if picker.selection.is_empty() {
                                "插入引用"
                            } else {
                                "替换选中文字"
                            }),
                        )
                        .clicked()
                    {
                        insert = true;
                    }
                    if ui.button("关闭").clicked() {
                        close = true;
                    }
                });
            });
        if let Some(id) = source
            && let Err(error) = self.choose_reference_source(&mut picker, id)
        {
            picker.error = Some(error.to_string());
        }
        if let Some(range) = locate {
            self.doc.pending_source_selection = Some(range.clone());
            self.doc.pending_source_jump = Some(range.start);
            close = true;
        }
        if insert {
            let reference = if picker.tab == Tab::Manual {
                Reference::manual(&picker.title, &picker.number, picker.no_number)
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
                    let token = reference.token();
                    let mut updated = picker.baseline.clone();
                    updated.replace_range(picker.selection.clone(), &token);
                    let cursor = picker.selection.start + token.len();
                    let updated = document_reference::put(&updated, &reference);
                    diff_editor::replace_with_undo(
                        ctx,
                        &mut self.doc.generated_markdown,
                        updated,
                        cursor,
                    );
                    self.doc.pending_source_jump = Some(cursor);
                    *self.status = "已插入公文引用，可按 Ctrl+Z 撤销。".into();
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
