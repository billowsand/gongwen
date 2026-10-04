//! AI 管理页「风格」分区（`docs/ai-agent-workbench.md` 16.15 C）：风格档案的学习与管理。
//!
//! - 列出全部档案：启用 / 停用、样稿有更新的提示、编辑、删除，导出导入；
//! - 「新建风格」：从稿件库与知识库里挑 2–10 篇样稿，后台学出一份档案草稿，看过、改过点「保存」才生效；
//!   写法描述里出现样稿事实的标出来让用户删；
//! - 编辑：名称、说明、适用文种与场合、设为哪些文种的默认、写法描述、增删范例；可以「重新学习」
//!   （照原来的样稿再学一次，保留名称、文种、场合这些用户定的东西）。
//!
//! 学习要调模型，放在后台线程，界面每帧取一次结果。

use super::settings::setting_row;
use crate::agent::backend::LmBackend;
use crate::agent::board::RefSource;
use crate::agent::style::{
    self, Learned, MAX_SAMPLES, Sample, StyleBook, StyleExample, StyleProfile,
};
use crate::agent::tools::{KnowledgeSearch, ManuscriptSource, RagSearch, SqliteManuscripts};
use crate::app::GongwenApp;
use crate::models::TemplateKind;
use crate::theme;
use eframe::egui;
use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, atomic::AtomicBool};
use std::time::Duration;

/// 分区状态。纯当次会话，不进配置。
#[derive(Default)]
pub(crate) struct StylesPage {
    loaded: bool,
    book: StyleBook,
    /// 档案 id → 样稿后来改过的那几篇。
    stale: BTreeMap<String, Vec<String>>,
    selected: Option<String>,
    /// 编辑中的档案（新学的或改着的），点保存才写回。
    editor: Option<Editor>,
    /// 挑样稿。
    picker: Option<Picker>,
    learning: Option<Receiver<LearnOutcome>>,
    phase: String,
    confirm_remove: Option<String>,
    message: Option<(bool, String)>,
}

struct Editor {
    profile: StyleProfile,
    /// 适用场合的输入框（顿号分隔）。
    occasions: String,
    /// 写法描述里出现的样稿事实。
    flagged: Vec<String>,
    /// 还没存过（新学的）。
    fresh: bool,
}

impl Editor {
    fn new(profile: StyleProfile, flagged: Vec<String>, fresh: bool) -> Self {
        Self {
            occasions: profile.occasions.join("、"),
            profile,
            flagged,
            fresh,
        }
    }
}

#[derive(Default)]
struct Picker {
    query: String,
    catalog: Vec<Entry>,
    chosen: Vec<(RefSource, i64)>,
    /// 重新学习时保留的档案。
    relearn: Option<StyleProfile>,
}

#[derive(Clone)]
struct Entry {
    source: RefSource,
    id: i64,
    title: String,
    kind: TemplateKind,
}

/// 学习的结果，或进度。
enum LearnOutcome {
    Phase(String),
    Done(Box<Result<(Learned, Option<StyleProfile>), String>>),
}

/// 稿件库与知识库的全部文档（挑样稿用）。
fn catalog() -> Vec<Entry> {
    let Ok(path) = crate::storage::manuscript_db_path() else {
        return Vec::new();
    };
    let mut out: Vec<Entry> = crate::manuscript::ManuscriptStore::open(&path)
        .and_then(|mut store| store.list(&crate::manuscript::ManuscriptFilter::default()))
        .unwrap_or_default()
        .into_iter()
        .filter(|row| !row.title.trim().is_empty())
        .map(|row| Entry {
            source: RefSource::Manuscript,
            id: row.id,
            title: row.title,
            kind: row.kind,
        })
        .collect();
    out.extend(
        crate::knowledge::KnowledgeStore::open(&path)
            .and_then(|mut store| store.list_docs(None))
            .unwrap_or_default()
            .into_iter()
            .map(|row| Entry {
                source: RefSource::Knowledge,
                id: row.id,
                title: row.title,
                kind: row.kind,
            }),
    );
    out
}

/// 读一篇样稿的正文与文种。
fn read_sample(
    source: RefSource,
    id: i64,
    kb: &dyn KnowledgeSearch,
) -> anyhow::Result<Option<Sample>> {
    Ok(match source {
        RefSource::Manuscript => SqliteManuscripts.read(id, None)?.map(|doc| Sample {
            source,
            id,
            title: doc.title,
            kind: Some(doc.kind),
            text: doc.markdown,
        }),
        RefSource::Knowledge => kb.read(id)?.map(|(title, text)| Sample {
            source,
            id,
            title,
            kind: None,
            text,
        }),
    })
}

fn knowledge(config: &crate::models::AppConfig) -> RagSearch {
    RagSearch {
        enabled: true,
        rag: config.rag.clone(),
        chat: config.lm_studio.clone(),
        kind_filter: None,
    }
}

/// 各档案的样稿有没有改过。
fn stale_map(book: &StyleBook, config: &crate::models::AppConfig) -> BTreeMap<String, Vec<String>> {
    let kb = knowledge(config);
    book.styles
        .iter()
        .map(|profile| {
            let changed = style::stale_sources(profile, &|source| {
                read_sample(source.source, source.id, &kb)
                    .ok()
                    .flatten()
                    .map(|sample| style::content_hash(&sample.text))
            });
            (profile.id.clone(), changed)
        })
        .filter(|(_, changed)| !changed.is_empty())
        .collect()
}

impl GongwenApp {
    pub(super) fn styles_section_ui(&mut self, ui: &mut egui::Ui) {
        if !self.styles_page.loaded {
            self.reload_styles();
        }
        self.poll_style_learning(ui.ctx());
        let page = &mut self.styles_page;
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            if theme::primary_icon_button_enabled(
                ui,
                page.learning.is_none(),
                theme::Icon::Plus,
                "新建风格",
            )
            .on_hover_text("从稿件库、知识库里挑 2–10 篇同类稿子，学出一份写法风格")
            .clicked()
            {
                page.picker = Some(Picker {
                    catalog: catalog(),
                    ..Picker::default()
                });
                page.editor = None;
            }
            if theme::icon_button(ui, theme::Icon::FileUp, "导入风格档案…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("风格档案", &["json"])
                    .pick_file()
            {
                action = Some(PageAction::Import(path));
            }
            if theme::icon_button(ui, theme::Icon::FileDown, "导出全部风格档案…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("风格档案", &["json"])
                    .set_file_name("风格档案.json")
                    .save_file()
            {
                action = Some(PageAction::Export(path));
            }
            ui.weak("也可以在 AI 侧栏 @ 几篇稿子，说「学一下这几篇的风格」。");
        });
        if let Some((ok, text)) = &page.message {
            ui.colored_label(
                if *ok {
                    theme::success()
                } else {
                    theme::danger()
                },
                text,
            );
        }
        if page.learning.is_some() {
            ui.horizontal(|ui| {
                theme::spinner(ui, 14.0, theme::accent());
                ui.weak(if page.phase.is_empty() {
                    "正在学习…"
                } else {
                    &page.phase
                });
            });
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        ui.add_space(6.0);

        if let Some(picker) = page.picker.as_mut() {
            match picker_ui(ui, picker) {
                Some(true) => action = Some(PageAction::Learn),
                Some(false) => page.picker = None,
                None => {}
            }
            ui.add_space(8.0);
            ui.separator();
        }

        if page.book.styles.is_empty() && page.editor.is_none() {
            ui.weak("还没有风格档案。");
        }
        for profile in &mut page.book.styles {
            ui.horizontal(|ui| {
                if ui
                    .checkbox(&mut profile.enabled, "")
                    .on_hover_text("停用的不参与自动挑选")
                    .changed()
                {
                    action = Some(PageAction::Persist);
                }
                let selected = page.selected.as_deref() == Some(profile.id.as_str());
                if ui
                    .selectable_label(selected, egui::RichText::new(&profile.name).strong())
                    .clicked()
                {
                    action = Some(PageAction::Edit(profile.id.clone()));
                }
                let kinds = if profile.kinds.is_empty() {
                    "不限文种".to_string()
                } else {
                    profile
                        .kinds
                        .iter()
                        .map(|k| k.label())
                        .collect::<Vec<_>>()
                        .join("、")
                };
                ui.weak(format!(
                    "{kinds} · 场合：{} · 用过 {} 次",
                    if profile.occasions.is_empty() {
                        "未写".to_string()
                    } else {
                        profile.occasions.join("、")
                    },
                    profile.uses
                ));
                if let Some(changed) = page.stale.get(&profile.id) {
                    theme::chip(ui, "样稿有更新", theme::warn(), theme::warn_soft()).on_hover_text(
                        format!("《{}》学过之后改了，可以重新学习", changed.join("》《")),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if page.confirm_remove.as_deref() == Some(profile.id.as_str()) {
                        if ui.button("取消").clicked() {
                            page.confirm_remove = None;
                        }
                        if theme::danger_icon_button(ui, theme::Icon::Trash, "确认删除").clicked()
                        {
                            action = Some(PageAction::Remove(profile.id.clone()));
                        }
                    } else if theme::icon_button(ui, theme::Icon::Trash, "删除这份风格").clicked()
                    {
                        page.confirm_remove = Some(profile.id.clone());
                    }
                });
            });
        }

        if let Some(editor) = page.editor.as_mut() {
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(6.0);
            match editor_ui(ui, editor, page.learning.is_some()) {
                Some(EditorAction::Save) => action = Some(PageAction::Save),
                Some(EditorAction::Cancel) => action = Some(PageAction::CloseEditor),
                Some(EditorAction::Relearn) => action = Some(PageAction::Relearn),
                None => {}
            }
        }

        if let Some(action) = action {
            self.apply_styles_action(action);
        }
    }

    fn reload_styles(&mut self) {
        let page = &mut self.styles_page;
        page.loaded = true;
        match StyleBook::load() {
            Ok(book) => page.book = book,
            Err(error) => page.message = Some((false, format!("{error:#}"))),
        }
        page.stale = stale_map(&page.book, &self.config);
    }

    fn poll_style_learning(&mut self, ctx: &egui::Context) {
        let page = &mut self.styles_page;
        let Some(receiver) = &page.learning else {
            return;
        };
        loop {
            match receiver.try_recv() {
                Ok(LearnOutcome::Phase(phase)) => page.phase = phase,
                Ok(LearnOutcome::Done(result)) => {
                    page.learning = None;
                    page.phase.clear();
                    match *result {
                        Ok((learned, keep)) => {
                            let mut profile = learned.profile;
                            let fresh = keep.is_none();
                            if let Some(old) = keep {
                                // 重新学习：用户定的名称、文种、场合、默认、启用都留着。
                                profile.id = old.id;
                                profile.name = old.name;
                                profile.note = old.note;
                                profile.kinds = old.kinds;
                                profile.occasions = old.occasions;
                                profile.default_for = old.default_for;
                                profile.enabled = old.enabled;
                                profile.uses = old.uses;
                            }
                            page.selected = Some(profile.id.clone());
                            page.message =
                                Some((true, "学好了，看一下、改一改，点「保存」才生效。".into()));
                            page.editor = Some(Editor::new(profile, learned.flagged, fresh));
                        }
                        Err(error) => page.message = Some((false, format!("学习失败：{error}"))),
                    }
                    ctx.request_repaint();
                    return;
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    page.learning = None;
                    page.message = Some((false, "学习中断了".into()));
                    return;
                }
            }
        }
    }

    fn start_style_learning(&mut self, chosen: Vec<(RefSource, i64)>, keep: Option<StyleProfile>) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.styles_page.learning = Some(rx);
        self.styles_page.phase = "读样稿…".into();
        self.styles_page.message = None;
        let config = self.config.clone();
        std::thread::spawn(move || {
            let kb = knowledge(&config);
            let mut samples = Vec::new();
            for (source, id) in chosen.into_iter().take(MAX_SAMPLES) {
                if let Ok(Some(sample)) = read_sample(source, id, &kb)
                    && !sample.text.trim().is_empty()
                {
                    samples.push(sample);
                }
            }
            let model = LmBackend::new(&config, Arc::new(AtomicBool::new(false)));
            let phase_tx = tx.clone();
            let result = style::learn(&model, &samples, &config.vocabulary, &mut |phase| {
                let _ = phase_tx.send(LearnOutcome::Phase(phase));
            })
            .map(|learned| (learned, keep))
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send(LearnOutcome::Done(Box::new(result)));
        });
    }

    fn apply_styles_action(&mut self, action: PageAction) {
        let page = &mut self.styles_page;
        match action {
            PageAction::Persist => {
                if let Err(error) = page.book.save() {
                    page.message = Some((false, format!("保存失败：{error:#}")));
                }
            }
            PageAction::Edit(id) => {
                page.selected = Some(id.clone());
                page.confirm_remove = None;
                page.editor = page
                    .book
                    .get(&id)
                    .cloned()
                    .map(|profile| Editor::new(profile, Vec::new(), false));
            }
            PageAction::Remove(id) => {
                page.book.remove(&id);
                page.confirm_remove = None;
                if page.selected.as_deref() == Some(id.as_str()) {
                    page.selected = None;
                    page.editor = None;
                }
                page.message = Some(match page.book.save() {
                    Ok(()) => (true, "已删除。".into()),
                    Err(error) => (false, format!("保存失败：{error:#}")),
                });
            }
            PageAction::Save => {
                if let Some(editor) = page.editor.as_mut() {
                    editor.profile.occasions = editor
                        .occasions
                        .split(['、', '，', ',', '；', ';', ' '])
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    if editor.profile.name.trim().is_empty() {
                        page.message = Some((false, "先给这份风格起个名字。".into()));
                        return;
                    }
                    let name = editor.profile.name.clone();
                    page.book.upsert(editor.profile.clone());
                    editor.fresh = false;
                    page.message = Some(match page.book.save() {
                        Ok(()) => (true, format!("已保存风格「{name}」。")),
                        Err(error) => (false, format!("保存失败：{error:#}")),
                    });
                    page.stale = stale_map(&page.book, &self.config);
                }
            }
            PageAction::CloseEditor => {
                page.editor = None;
                page.selected = None;
            }
            PageAction::Learn => {
                if let Some(picker) = page.picker.take() {
                    self.start_style_learning(picker.chosen, picker.relearn);
                }
            }
            PageAction::Relearn => {
                if let Some(editor) = &page.editor {
                    let chosen = editor
                        .profile
                        .sources
                        .iter()
                        .map(|source| (source.source, source.id))
                        .collect();
                    let keep = editor.profile.clone();
                    self.start_style_learning(chosen, Some(keep));
                }
            }
            PageAction::Import(path) => {
                page.message = Some(
                    match page.book.import(&path).and_then(|count| {
                        page.book.save()?;
                        Ok(count)
                    }) {
                        Ok(count) => (true, format!("导入了 {count} 份风格档案。")),
                        Err(error) => (false, format!("导入失败：{error:#}")),
                    },
                );
                page.stale = stale_map(&page.book, &self.config);
            }
            PageAction::Export(path) => {
                page.message = Some(match page.book.export(&path) {
                    Ok(()) => (true, format!("已导出到 {}", path.display())),
                    Err(error) => (false, format!("导出失败：{error:#}")),
                });
            }
        }
    }
}

enum PageAction {
    Persist,
    Edit(String),
    Remove(String),
    Save,
    CloseEditor,
    Learn,
    Relearn,
    Import(std::path::PathBuf),
    Export(std::path::PathBuf),
}

enum EditorAction {
    Save,
    Cancel,
    Relearn,
}

/// 挑样稿：搜索框 + 勾选列表。返回 Some(true) 开始学，Some(false) 取消。
fn picker_ui(ui: &mut egui::Ui, picker: &mut Picker) -> Option<bool> {
    let mut result = None;
    ui.label(egui::RichText::new("挑样稿（2–10 篇同类稿子）").strong());
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut picker.query)
                .hint_text("按标题筛选")
                .desired_width(260.0),
        );
        let count = picker.chosen.len();
        if theme::primary_icon_button_enabled(
            ui,
            count >= 1,
            theme::Icon::Sparkles,
            &format!("开始学习（已选 {count} 篇）"),
        )
        .on_disabled_hover_text("至少选一篇；同类的两篇以上学得更准")
        .clicked()
        {
            result = Some(true);
        }
        if ui.button("取消").clicked() {
            result = Some(false);
        }
    });
    let query = picker.query.trim().to_string();
    egui::ScrollArea::vertical()
        .id_salt("style_picker")
        .max_height(240.0)
        .show(ui, |ui| {
            for source in [RefSource::Manuscript, RefSource::Knowledge] {
                let items: Vec<&Entry> = picker
                    .catalog
                    .iter()
                    .filter(|e| {
                        e.source == source && (query.is_empty() || e.title.contains(&query))
                    })
                    .take(200)
                    .collect();
                if items.is_empty() {
                    continue;
                }
                ui.label(
                    egui::RichText::new(source.label())
                        .small()
                        .color(theme::text_muted()),
                );
                for entry in items {
                    let key = (entry.source, entry.id);
                    let mut checked = picker.chosen.contains(&key);
                    let full = picker.chosen.len() >= MAX_SAMPLES && !checked;
                    if ui
                        .add_enabled(
                            !full,
                            egui::Checkbox::new(
                                &mut checked,
                                format!("{}（{}）", entry.title, entry.kind.label()),
                            ),
                        )
                        .changed()
                    {
                        if checked {
                            picker.chosen.push(key);
                        } else {
                            picker.chosen.retain(|k| *k != key);
                        }
                    }
                }
            }
        });
    result
}

fn editor_ui(ui: &mut egui::Ui, editor: &mut Editor, busy: bool) -> Option<EditorAction> {
    let mut result = None;
    let profile = &mut editor.profile;
    if editor.fresh {
        theme::chip(
            ui,
            "新学的，还没保存",
            theme::accent(),
            theme::accent_soft(),
        );
    }
    if !editor.flagged.is_empty() {
        theme::card().fill(theme::warn_soft()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(format!(
                "写法描述里出现了样稿里的这些事实，只学写法不学事实，建议删掉：{}",
                editor.flagged.join("、")
            ));
        });
    }
    setting_row(ui, "名称", None, |ui| {
        ui.add(egui::TextEdit::singleline(&mut profile.name).desired_width(240.0));
    });
    setting_row(ui, "说明", None, |ui| {
        ui.add(egui::TextEdit::singleline(&mut profile.note).desired_width(360.0));
    });
    setting_row(
        ui,
        "适用文种",
        Some("一个都不勾表示不限文种；自动挑风格时文种必须对得上"),
        |ui| {
            ui.horizontal_wrapped(|ui| {
                for kind in TemplateKind::ALL {
                    let mut on = profile.kinds.contains(&kind);
                    if ui.checkbox(&mut on, kind.label()).changed() {
                        if on {
                            profile.kinds.push(kind);
                        } else {
                            profile.kinds.retain(|k| *k != kind);
                        }
                    }
                }
            });
        },
    );
    setting_row(
        ui,
        "适用场合",
        Some("关键词，顿号分隔；你的原话或标题里出现就加分，如「讲话、调研报告、对下部署」"),
        |ui| {
            ui.add(egui::TextEdit::singleline(&mut editor.occasions).desired_width(360.0));
        },
    );
    setting_row(
        ui,
        "设为默认",
        Some("这些文种分不出用哪份风格时，默认用它"),
        |ui| {
            ui.horizontal_wrapped(|ui| {
                for kind in TemplateKind::ALL {
                    let mut on = profile.default_for.contains(&kind);
                    if ui.checkbox(&mut on, kind.label()).changed() {
                        if on {
                            profile.default_for.push(kind);
                        } else {
                            profile.default_for.retain(|k| *k != kind);
                        }
                    }
                }
            });
        },
    );
    ui.label(egui::RichText::new("写法描述").strong());
    ui.add(
        egui::TextEdit::multiline(&mut profile.description)
            .desired_rows(8)
            .desired_width(f32::INFINITY),
    );
    ui.add_space(4.0);
    ui.label(egui::RichText::new("统计特征（程序算）").strong());
    for line in profile.stats.lines() {
        ui.weak(line);
    }
    ui.add_space(4.0);
    ui.label(egui::RichText::new("范例（只学写法，内容不会照搬）").strong());
    let mut drop = None;
    for (index, example) in profile.examples.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.weak(format!("{}·《{}》", example.role, example.source));
            if theme::icon_button(ui, theme::Icon::X, "删掉这段范例").clicked() {
                drop = Some(index);
            }
        });
        ui.add(
            egui::TextEdit::multiline(&mut example.text)
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
    }
    if let Some(index) = drop {
        profile.examples.remove(index);
    }
    if ui.small_button("加一段范例").clicked() {
        profile.examples.push(StyleExample {
            role: "正文".into(),
            ..StyleExample::default()
        });
    }
    ui.add_space(4.0);
    if !profile.sources.is_empty() {
        let titles: Vec<String> = profile
            .sources
            .iter()
            .map(|s| format!("《{}》", s.title))
            .collect();
        ui.weak(format!("学自：{}", titles.join("")));
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if theme::primary_icon_button(ui, theme::Icon::Save, "保存").clicked() {
            result = Some(EditorAction::Save);
        }
        if ui
            .button(if editor.fresh { "放弃" } else { "收起" })
            .clicked()
        {
            result = Some(EditorAction::Cancel);
        }
        if !profile.sources.is_empty()
            && ui
                .add_enabled(
                    !busy,
                    theme::secondary_icon_button(theme::Icon::Refresh, "重新学习"),
                )
                .on_hover_text("照原来的样稿再学一次；名称、文种、场合这些你定的保留")
                .clicked()
        {
            result = Some(EditorAction::Relearn);
        }
    });
    result
}
