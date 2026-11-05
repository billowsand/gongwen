//! AI 管理页「风格」分区（`docs/ai-agent-workbench.md` 16.15 C）：风格档案的学习与管理。
//!
//! - 列出全部档案：启用 / 停用、样稿有更新的提示、编辑、删除，导出导入；
//! - 「新建风格」：选择库内或上传样稿，由同一Pi Durable内核执行风格学习Skill，交付JSON草稿；
//!   看过、改过点「保存」才生效，不运行额外事实核查；
//! - 编辑：名称、说明、适用文种与场合、设为哪些文种的默认、写法描述、增删范例；可以「重新学习」
//!   （照原来的样稿再学一次，保留名称、文种、场合这些用户定的东西）。
//!
//! 学习要调模型，放在后台线程，界面每帧取一次结果。

use super::file_dialog::FileDialogSpec;
use super::settings::setting_row;
use crate::ai_services::manuscripts::{ManuscriptSource, SqliteManuscripts};
use crate::ai_services::references::RefSource;
use crate::ai_services::style::{
    self, Learned, MAX_SAMPLES, Sample, StyleBook, StyleExample, StyleProfile,
};
use crate::app::GongwenApp;
use crate::models::TemplateKind;
use crate::theme;
use eframe::egui;
use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
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
    cancel: Option<Arc<AtomicBool>>,
    phase: String,
    confirm_remove: Option<String>,
    message: Option<(bool, String)>,
}

impl Drop for StylesPage {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl StylesPage {
    /// 关窗时应用层统一取消仍在跑的风格学习。
    pub(crate) fn cancel_learning(&self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    /// 是否有一轮风格学习还没收尾（关窗时要等它退出 Node）。
    pub(crate) fn learning_in_progress(&self) -> bool {
        self.learning.is_some()
    }
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

#[derive(Default, Clone)]
struct Picker {
    query: String,
    catalog: Vec<Entry>,
    chosen: Vec<(RefSource, i64)>,
    /// 重新学习时保留的档案。
    relearn: Option<StyleProfile>,
    files: Vec<std::path::PathBuf>,
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
fn read_sample(source: RefSource, id: i64) -> anyhow::Result<Option<Sample>> {
    Ok(match source {
        RefSource::Manuscript => SqliteManuscripts.read(id, None)?.map(|doc| Sample {
            source,
            id,
            title: doc.title,
            kind: Some(doc.kind),
            text: doc.markdown,
        }),
        RefSource::Knowledge => {
            crate::knowledge::KnowledgeStore::open(&crate::storage::manuscript_db_path()?)?
                .get_doc_content(id)?
                .map(|(title, text)| Sample {
                    source,
                    id,
                    title,
                    kind: None,
                    text,
                })
        }
    })
}

/// 各档案的样稿有没有改过。
fn stale_map(book: &StyleBook) -> BTreeMap<String, Vec<String>> {
    book.styles
        .iter()
        .map(|profile| {
            let changed = style::stale_sources(profile, &|source| {
                read_sample(source.source, source.id)
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
        // 要开文件对话框的请求单独记：路径要等用户挑完才有，页面只发请求。
        let mut file_request = None;
        ui.horizontal_wrapped(|ui| {
            if theme::primary_icon_button_enabled(
                ui,
                page.learning.is_none(),
                theme::Icon::Plus,
                "新建风格",
            )
            .on_hover_text("从稿件库、知识库里挑 1–10 篇同类稿子，学出一份写法风格")
            .clicked()
            {
                page.picker = Some(Picker {
                    catalog: catalog(),
                    ..Picker::default()
                });
                page.editor = None;
            }
            if ui
                .add_enabled(
                    page.learning.is_none(),
                    theme::secondary_icon_button(theme::Icon::FileUp, "导入风格档案…"),
                )
                .clicked()
            {
                file_request = Some(PageAction::ChooseImportFile);
            }
            if theme::icon_button(ui, theme::Icon::FileDown, "导出全部风格档案…").clicked()
            {
                file_request = Some(PageAction::ChooseExportFile);
            }
            ui.weak("由 Pi 的风格学习技能生成；可选库内样稿，也可上传文件，查看后保存。");
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
                if ui.button("停止学习").clicked() {
                    if let Some(cancel) = &page.cancel {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    page.phase = "正在停止 Pi 风格学习…".into();
                }
                ui.weak(if page.phase.is_empty() {
                    "正在学习…"
                } else {
                    &page.phase
                });
            });
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        ui.add_space(6.0);

        let can_pick = page.learning.is_none();
        if let Some(picker) = page.picker.as_mut() {
            if ui
                .add_enabled(can_pick, egui::Button::new("上传样稿文件…"))
                .clicked()
            {
                file_request = Some(PageAction::ChooseSamples);
            }
            match ui
                .add_enabled_ui(can_pick, |ui| picker_ui(ui, picker))
                .inner
            {
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
                if page.learning.is_some() {
                    ui.disable();
                }
                if ui
                    .checkbox(&mut profile.enabled, "")
                    .on_hover_text("停用的不参与自动挑选")
                    .changed()
                {
                    action = Some(PageAction::Persist);
                }
                let selected = page.selected.as_deref() == Some(profile.id.as_str());
                if ui
                    .add_enabled(
                        page.learning.is_none(),
                        egui::Button::selectable(
                            selected,
                            egui::RichText::new(&profile.name).strong(),
                        ),
                    )
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
            match ui
                .add_enabled_ui(page.learning.is_none(), |ui| editor_ui(ui, editor, false))
                .inner
            {
                Some(EditorAction::Save) => action = Some(PageAction::Save),
                Some(EditorAction::Cancel) => action = Some(PageAction::CloseEditor),
                Some(EditorAction::Relearn) => action = Some(PageAction::Relearn),
                None => {}
            }
        }

        if let Some(action) = action {
            self.apply_styles_action(action);
        }
        if let Some(action) = file_request {
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
        page.stale = stale_map(&page.book);
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
                    if page
                        .cancel
                        .take()
                        .is_some_and(|cancel| cancel.load(Ordering::Relaxed))
                    {
                        page.message = Some((false, "Pi 风格学习已停止，本轮结果未采纳。".into()));
                        return;
                    }
                    match *result {
                        Ok((learned, keep)) => {
                            page.picker = None;
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
                    page.cancel = None;
                    page.message = Some((false, "Pi 风格学习中断了，原风格未改变。".into()));
                    return;
                }
            }
        }
    }

    fn start_style_learning(
        &mut self,
        chosen: Vec<(RefSource, i64)>,
        files: Vec<std::path::PathBuf>,
        keep: Option<StyleProfile>,
    ) {
        if self.styles_page.learning.is_some() {
            return;
        }
        if let Err(error) = self.config.pi_chat() {
            self.styles_page.message = Some((false, format!("无法学习风格：{error}")));
            return;
        }
        // 风格学习同样起一轮 Pi：和正文写作共用全局门禁，避免两个 Node 抢同一个模型。
        if crate::pi_agent::writing_active() {
            self.styles_page.message =
                Some((false, "有 Pi 写作正在运行，等它结束后再学习风格。".into()));
            return;
        }
        let writing = match crate::pi_agent::begin_writing() {
            Ok(writing) => writing,
            Err(error) => {
                self.styles_page.message = Some((false, error));
                return;
            }
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        self.styles_page.cancel = Some(cancel.clone());
        self.styles_page.learning = Some(rx);
        self.styles_page.phase = "读取样稿，准备 Pi…".into();
        self.styles_page.message = None;
        let config = Arc::new(self.config.clone());
        std::thread::spawn(move || {
            let _writing = writing;
            let result = (|| -> Result<_, String> {
                if chosen.len() + files.len() > MAX_SAMPLES {
                    return Err("一次最多选择10篇样稿。".into());
                }
                let mut samples = Vec::new();
                for (source, id) in chosen {
                    if cancel.load(Ordering::Relaxed) {
                        return Err("已停止".into());
                    }
                    let sample = read_sample(source, id)
                        .map_err(|e| format!("读取样稿失败：{e:#}"))?
                        .ok_or("选择的样稿已不存在，请重新选择。")?;
                    if sample.text.len() > 512 * 1024 {
                        return Err(format!("样稿《{}》超过512 KiB，请缩减材料。", sample.title));
                    }
                    if sample.text.trim().is_empty() {
                        return Err(format!("样稿《{}》为空，未启动Pi。", sample.title));
                    }
                    samples.push(sample);
                }
                if let Some(old) = &keep {
                    samples.extend(crate::pi_agent::style_learning::uploaded_samples(
                        &old.learning_workspace,
                    )?);
                }
                for path in files {
                    if cancel.load(Ordering::Relaxed) {
                        return Err("已停止".into());
                    }
                    let text = crate::doc_import::to_knowledge_markdown(&path)
                        .map_err(|e| format!("读取上传样稿失败：{e:#}"))?;
                    if text.trim().is_empty() || text.len() > 512 * 1024 {
                        return Err("上传样稿为空或转换后超过512 KiB，请缩减材料。".into());
                    }
                    // id=0仅用于共享统计函数；持久化来源由Pi工作区记录，不冒充知识库条目。
                    samples.push(Sample {
                        source: RefSource::Knowledge,
                        id: 0,
                        title: crate::doc_import::file_label(&path),
                        kind: None,
                        text,
                    });
                }
                crate::pi_agent::style_learning::learn(config, samples, cancel, |message| {
                    let _ = tx.send(LearnOutcome::Phase(message));
                })
            })()
            .map(|learned| (learned, keep));
            let _ = tx.send(LearnOutcome::Done(Box::new(result)));
        });
    }

    fn apply_styles_action(&mut self, action: PageAction) {
        // 要开对话框的两种先走：它们要拿整个应用（页面状态在 self 里）。
        match action {
            PageAction::ChooseSamples => {
                self.request_files(crate::doc_import::knowledge_files_spec(), |app, paths| {
                    let Some(paths) = paths else {
                        return;
                    };
                    if let Some(picker) = &mut app.styles_page.picker {
                        let mut files = picker.files.clone();
                        for path in paths {
                            if !files.contains(&path) {
                                files.push(path);
                            }
                        }
                        if picker.chosen.len() + files.len() > MAX_SAMPLES {
                            app.styles_page.message =
                                Some((false, "一次最多选10篇样稿；本批文件未加入。".into()));
                        } else {
                            picker.files = files;
                        }
                    }
                });
                return;
            }
            PageAction::ChooseImportFile => {
                self.request_file(
                    FileDialogSpec::pick_file().filter("风格档案", &["json"]),
                    |app, path| {
                        let Some(path) = path else {
                            return;
                        };
                        app.apply_styles_action(PageAction::Import(path));
                    },
                );
                return;
            }
            PageAction::ChooseExportFile => {
                let spec = FileDialogSpec::save_file()
                    .filter("风格档案", &["json"])
                    .file_name("风格档案.json");
                let count = self.styles_page.book.styles.len();
                // 风格档案含样稿原文片段与稿件标题，加密工作区里写成普通文件要先确认风险。
                self.request_output_with_warning(
                    format!("导出 {count} 份风格档案（含样稿原文片段与稿件标题）"),
                    super::output_guard::PII_EXPORT_WARNING,
                    move |app| {
                        app.request_file(spec, |app, path| {
                            let Some(path) = path else {
                                return;
                            };
                            app.apply_styles_action(PageAction::Export(path));
                        });
                    },
                );
                return;
            }
            _ => {}
        }
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
                    let mut updated = page.book.clone();
                    updated.upsert(editor.profile.clone());
                    page.message = Some(match updated.save() {
                        Ok(()) => {
                            page.book = updated;
                            editor.fresh = false;
                            (true, format!("已保存风格「{name}」。"))
                        }
                        Err(error) => (false, format!("保存失败：{error:#}")),
                    });
                    page.stale = stale_map(&page.book);
                }
            }
            PageAction::CloseEditor => {
                page.editor = None;
                page.selected = None;
            }
            PageAction::Learn => {
                if let Some(picker) = page.picker.clone() {
                    self.start_style_learning(picker.chosen, picker.files, picker.relearn);
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
                    self.start_style_learning(chosen, Vec::new(), Some(keep));
                }
            }
            PageAction::Import(path) => self.import_styles(path),
            PageAction::Export(path) => self.export_styles(path),
            // 要开对话框的两种已经在前面处理过了。
            PageAction::ChooseImportFile
            | PageAction::ChooseExportFile
            | PageAction::ChooseSamples => {}
        }
    }

    /// 导入一份风格档案。
    fn import_styles(&mut self, path: std::path::PathBuf) {
        let page = &mut self.styles_page;
        page.message = Some(
            match page.book.import(&path).and_then(|count| {
                page.book.save()?;
                Ok(count)
            }) {
                Ok(count) => (true, format!("导入了 {count} 份风格档案。")),
                Err(error) => (false, format!("导入失败：{error:#}")),
            },
        );
        page.stale = stale_map(&page.book);
    }

    /// 导出全部风格档案到一个文件。
    fn export_styles(&mut self, path: std::path::PathBuf) {
        let page = &mut self.styles_page;
        page.message = Some(match page.book.export(&path) {
            Ok(()) => (true, format!("已导出到 {}", path.display())),
            Err(error) => (false, format!("导出失败：{error:#}")),
        });
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
    /// 上传样稿，路径经应用层异步文件对话框返回。
    ChooseSamples,
    /// 导入已有风格档案。
    ChooseImportFile,
    /// 请求打开「导出风格档案」的文件对话框（路径要等用户挑完才有）。
    ChooseExportFile,
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
    ui.label(egui::RichText::new("挑样稿（1–10 篇同类稿子，建议至少两篇）").strong());
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut picker.query)
                .hint_text("按标题筛选")
                .desired_width(260.0),
        );
        let count = picker.chosen.len() + picker.files.len();
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
    let mut remove = None;
    for (index, path) in picker.files.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(format!("上传样稿：{}", crate::doc_import::file_label(path)));
            if ui.small_button("移除").clicked() {
                remove = Some(index);
            }
        });
    }
    if let Some(index) = remove {
        picker.files.remove(index);
    }
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
                    let full = picker.chosen.len() + picker.files.len() >= MAX_SAMPLES && !checked;
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
    crate::app::widgets::bounded_text_edit(
        ui,
        "style_description",
        10,
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
        crate::app::widgets::bounded_text_edit(
            ui,
            ("style_example", index),
            6,
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
        if (!profile.sources.is_empty() || !profile.learning_workspace.is_empty())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learning_requires_pi_and_never_falls_back_to_the_auxiliary_model() {
        let mut app = crate::app::management_samples::fixture();
        app.config.pi_model = Default::default();
        app.start_style_learning(Vec::new(), Vec::new(), None);
        assert!(app.styles_page.learning.is_none());
        assert!(
            app.styles_page
                .message
                .as_ref()
                .unwrap()
                .1
                .contains("智能写作")
        );
        assert!(app.styles_page.editor.is_none());
    }

    #[test]
    fn cancelled_queued_style_result_is_not_installed_or_written_into_a_document() {
        let mut app = crate::app::management_samples::fixture();
        let before = app.docs[0].generated_markdown.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(LearnOutcome::Done(Box::new(Ok((
            Learned {
                profile: StyleProfile {
                    name: "不该采纳".into(),
                    ..Default::default()
                },
                flagged: Vec::new(),
            },
            None,
        )))))
        .unwrap();
        app.styles_page.learning = Some(rx);
        app.styles_page.cancel = Some(Arc::new(AtomicBool::new(true)));
        app.poll_style_learning(&egui::Context::default());
        assert!(app.styles_page.editor.is_none());
        assert!(app.styles_page.book.styles.is_empty());
        assert_eq!(app.docs[0].generated_markdown, before);
    }

    #[test]
    fn relearning_preserves_user_metadata_and_only_save_changes_the_style_book() {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                crate::storage::set_test_config_dir(None);
            }
        }
        let temp = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(temp.path().to_owned()));
        let _reset = Reset;
        let mut app = crate::app::management_samples::fixture();
        let old = StyleProfile {
            id: "existing".into(),
            name: "用户名称".into(),
            note: "用户说明".into(),
            enabled: false,
            uses: 9,
            description: "旧描述".into(),
            ..Default::default()
        };
        app.styles_page.book.upsert(old.clone());
        app.styles_page.book.save().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(LearnOutcome::Done(Box::new(Ok((
            Learned {
                profile: StyleProfile {
                    name: "模型名称".into(),
                    description: "Pi学到的新写法".into(),
                    learning_workspace: "pi/workspaces/doc-0/run-new".into(),
                    ..Default::default()
                },
                flagged: Vec::new(),
            },
            Some(old),
        )))))
        .unwrap();
        app.styles_page.learning = Some(rx);
        app.poll_style_learning(&egui::Context::default());
        let editor = app.styles_page.editor.as_ref().unwrap();
        assert_eq!(editor.profile.name, "用户名称");
        assert!(!editor.profile.enabled);
        assert_eq!(editor.profile.uses, 9);
        assert_eq!(StyleBook::load().unwrap().styles[0].description, "旧描述");
        app.apply_styles_action(PageAction::Save);
        assert_eq!(
            StyleBook::load().unwrap().styles[0].description,
            "Pi学到的新写法"
        );
    }
}
