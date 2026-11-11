//! 标签、导航与文档/PDF 的打开、切换与关闭管理。
//!
//! 由 src/app.rs 拆分而来：本文件是模块 `app::tabs`，与其它子模块共享
//! `app` 根模块的私有可见性（`GongwenApp` 结构体与根模块常量仍在 app.rs 中）。

use crate::app::{DraftAction, GongwenApp, open_in_os, reveal_in_os};
use crate::draft_page::{DocKey, DraftPage, DraftSession};
use crate::models::ManuscriptStatus;
use crate::pdf_viewer::{PdfAction as PdfViewerAction, PdfKey, PdfSession};
use crate::theme;
use eframe::egui;
use std::path::PathBuf;

/// 导航行上的常驻页面。起草不在其中——它由打开的稿件派生成标签。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NavPage {
    Manuscript,
    TaskAssistant,
    Vocabulary,
    Proofread,
    Lexicon,
    ImeTable,
    AiPrompts,
    Knowledge,
    Wiki,
    Settings,
    Help,
}

/// 标签栏上的一格：一篇打开的稿件，或一个导航页。稿件和导航页共用同一条
/// 标签栏——看设置不必丢掉稿件上下文，关掉设置就回到稿子。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TabRef {
    Doc(DocKey),
    Page(NavPage),
    Pdf(PdfKey),
}

impl NavPage {
    /// 写进稿件库的稳定标识，用于会话恢复。改名会让旧记录失配，别改。
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Manuscript => "manuscript",
            Self::TaskAssistant => "task_assistant",
            Self::Vocabulary => "vocabulary",
            Self::Proofread => "proofread",
            Self::Lexicon => "lexicon",
            Self::ImeTable => "ime_table",
            Self::AiPrompts => "ai_prompts",
            Self::Knowledge => "knowledge",
            Self::Wiki => "wiki",
            Self::Settings => "settings",
            Self::Help => "help",
        }
    }

    pub(crate) fn from_key(key: &str) -> Option<Self> {
        match key {
            "manuscript" => Some(Self::Manuscript),
            "task_assistant" => Some(Self::TaskAssistant),
            "vocabulary" => Some(Self::Vocabulary),
            "proofread" => Some(Self::Proofread),
            "lexicon" => Some(Self::Lexicon),
            "ime_table" => Some(Self::ImeTable),
            "ai_prompts" => Some(Self::AiPrompts),
            "knowledge" => Some(Self::Knowledge),
            "wiki" => Some(Self::Wiki),
            "settings" => Some(Self::Settings),
            "help" => Some(Self::Help),
            _ => None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Manuscript => "稿件管理",
            Self::TaskAssistant => "任务助手",
            Self::Vocabulary => "标准词库",
            Self::Proofread => "校对词表",
            Self::Lexicon => "公文词表",
            Self::ImeTable => "输入法词表",
            Self::AiPrompts => "AI 管理",
            Self::Knowledge => "资料库",
            Self::Wiki => "知识库",
            Self::Settings => "设置",
            Self::Help => "使用帮助",
        }
    }

    pub(crate) fn icon(self) -> theme::Icon {
        match self {
            Self::Manuscript => theme::Icon::Library,
            Self::TaskAssistant => theme::Icon::List,
            Self::Vocabulary => theme::Icon::Book,
            Self::Proofread => theme::Icon::SquareCheck,
            Self::Lexicon => theme::Icon::Type,
            Self::ImeTable => theme::Icon::Hash,
            Self::AiPrompts => theme::Icon::WandSparkles,
            Self::Knowledge => theme::Icon::PackageOpen,
            Self::Wiki => theme::Icon::BookmarkCheck,
            Self::Settings => theme::Icon::Settings,
            Self::Help => theme::Icon::HelpCircle,
        }
    }
}

impl GongwenApp {
    /// 当前标签是不是稿件。
    pub(crate) fn showing_doc(&self) -> bool {
        matches!(self.tabs.get(self.active_tab), Some(TabRef::Doc(_)))
    }

    pub(crate) fn doc_index_of_key(&self, key: DocKey) -> Option<usize> {
        self.docs.iter().position(|doc| doc.key == key)
    }

    /// 按 key 取稿件标题；稿件已不在时返回空串，供确认框文案用。
    pub(crate) fn doc_title_of_key(&self, key: DocKey) -> String {
        self.doc_index_of_key(key)
            .map(|index| self.docs[index].title())
            .unwrap_or_default()
    }

    /// 按稿件库 ID 找到已打开的稿件，不切换标签。
    pub(crate) fn doc_index_of_manuscript(&self, id: i64) -> Option<usize> {
        self.docs
            .iter()
            .position(|doc| doc.manuscript_id == Some(id))
    }

    pub(crate) fn pdf_index_of_key(&self, key: PdfKey) -> Option<usize> {
        self.pdfs.iter().position(|pdf| pdf.key == key)
    }

    /// 切到某一格标签。稿件标签同时把 `active_doc` 指过去。
    pub(crate) fn activate_tab(&mut self, tab: usize) {
        let Some(&item) = self.tabs.get(tab) else {
            return;
        };
        // 切走之前先把手上这篇写回去，免得改了半天停在别的标签上没存。
        if tab != self.active_tab {
            self.autosave_all();
        }
        self.active_tab = tab;
        if let TabRef::Doc(key) = item
            && let Some(index) = self.doc_index_of_key(key)
        {
            self.active_doc = index;
        }
    }

    /// 切到某一篇稿件（按 `docs` 下标）。
    pub(crate) fn activate_doc(&mut self, index: usize) {
        let Some(key) = self.docs.get(index).map(|doc| doc.key) else {
            return;
        };
        if let Some(tab) = self.tabs.iter().position(|item| *item == TabRef::Doc(key)) {
            self.activate_tab(tab);
        }
    }

    /// 新开一个稿件标签并切过去。
    pub(crate) fn open_doc(&mut self, mut session: DraftSession) {
        session.key = self.next_doc_key;
        self.next_doc_key += 1;
        let key = session.key;
        self.docs.push(session);
        self.tabs.push(TabRef::Doc(key));
        self.activate_tab(self.tabs.len() - 1);
        self.draft_page().revalidate();
        self.remember_session();
    }

    /// 打开导航页。同一个页面不重复开，已经在就切过去。
    pub(crate) fn open_page(&mut self, page: NavPage) {
        match self
            .tabs
            .iter()
            .position(|item| *item == TabRef::Page(page))
        {
            Some(tab) => self.activate_tab(tab),
            None => {
                self.tabs.push(TabRef::Page(page));
                self.activate_tab(self.tabs.len() - 1);
                self.remember_session();
            }
        }
    }

    /// 在应用内打开 PDF。同一路径已经开着时只切换标签，不重复占用纹理。
    pub(crate) fn open_pdf(&mut self, path: PathBuf, title: Option<String>) {
        self.open_pdf_with_hints(path, title, crate::print::PrintHints::default());
    }

    /// 打开 PDF，并带上调用方知道的打印提示（稿件按双面排、送批材料合并件）。
    /// 同一路径已经开着时把提示更新上去，不重复开标签。
    pub(crate) fn open_pdf_with_hints(
        &mut self,
        path: PathBuf,
        title: Option<String>,
        hints: crate::print::PrintHints,
    ) {
        if let Some(index) = self
            .pdfs
            .iter()
            .position(|pdf| !pdf.is_memory() && pdf.path() == path)
        {
            // 不带提示的再次打开（如送批材料面板里「打开文件」）不能把已知的提示抹掉。
            if hints != crate::print::PrintHints::default() {
                self.pdfs[index].set_print_hints(hints);
            }
            if let Some(tab) = self
                .tabs
                .iter()
                .position(|item| *item == TabRef::Pdf(self.pdfs[index].key))
            {
                self.activate_tab(tab);
            }
            return;
        }
        let key = self.next_pdf_key;
        self.next_pdf_key += 1;
        let mut session = PdfSession::new(key, path, title, self.sender.clone());
        session.set_print_hints(hints);
        self.pdfs.push(session);
        self.tabs.push(TabRef::Pdf(key));
        self.activate_tab(self.tabs.len() - 1);
    }

    /// 稿件附件直接从 SQLCipher 解密后的字节预览，不写临时文件。
    pub(crate) fn open_pdf_memory(&mut self, id: i64, title: String, bytes: Vec<u8>) {
        let identity = PathBuf::from(format!("attachment-{id}"));
        if let Some(pdf) = self
            .pdfs
            .iter()
            .find(|pdf| pdf.is_memory() && pdf.path() == identity)
            && let Some(tab) = self
                .tabs
                .iter()
                .position(|tab| *tab == TabRef::Pdf(pdf.key))
        {
            self.activate_tab(tab);
            return;
        }
        let key = self.next_pdf_key;
        self.next_pdf_key += 1;
        self.pdfs.push(PdfSession::from_memory(
            key,
            identity,
            title,
            bytes,
            self.sender.clone(),
        ));
        self.tabs.push(TabRef::Pdf(key));
        self.activate_tab(self.tabs.len() - 1);
    }

    /// 导出后直接打印：打开 PDF（带上打印提示），等它打开完自动弹一次打印对话框。
    ///
    /// 同一路径已经开着时先关掉旧标签再重开：「覆盖同名」策略下刚导出的文件就写在
    /// 原处，旧标签里是上一版的页面与页数，切过去会预览旧内容、按旧页数校验页码。
    pub(crate) fn open_pdf_for_printing(&mut self, path: PathBuf, hints: crate::print::PrintHints) {
        self.reopen_exported_pdf(path.clone(), hints);
        if let Some(pdf) = self
            .pdfs
            .iter_mut()
            .find(|pdf| !pdf.is_memory() && pdf.path() == path)
        {
            pdf.request_print_when_ready_confirmed();
        }
    }

    /// 打开刚导出的 PDF。同一路径已经开着时先关掉旧标签再重开：「覆盖同名」策略下
    /// 新文件就写在原处，旧标签里还是上一版的页面。
    pub(crate) fn reopen_exported_pdf(&mut self, path: PathBuf, hints: crate::print::PrintHints) {
        if let Some(tab) = self.tabs.iter().position(|item| {
            matches!(item, TabRef::Pdf(key)
                if self.pdfs.iter().any(|pdf| pdf.key == *key && !pdf.is_memory() && pdf.path() == path))
        }) {
            self.close_tab(tab);
        }
        self.open_pdf_with_hints(path, None, hints);
    }

    pub(crate) fn pdf_ui(&mut self, key: PdfKey, ui: &mut egui::Ui) {
        let Some(index) = self.pdf_index_of_key(key) else {
            ui.centered_and_justified(|ui| {
                ui.label("这个 PDF 标签已经关闭。");
            });
            return;
        };
        let ctx = ui.ctx().clone();
        let actions = self.pdfs[index].ui(ui);
        self.apply_pdf_actions(actions, &ctx);
    }

    pub(crate) fn apply_pdf_actions(&mut self, actions: Vec<PdfViewerAction>, ctx: &egui::Context) {
        for action in actions {
            match action {
                PdfViewerAction::OpenExternal(path) => {
                    self.request_output_with_warning(
                        format!("用系统程序打开 PDF：{}", path.display()),
                        super::output_guard::EXTERNAL_OPEN_WARNING,
                        move |app| {
                            app.status = match open_in_os(&path) {
                                Ok(()) => format!("已用系统程序打开 {}。", path.display()),
                                Err(error) => format!("打开 PDF 失败：{error}"),
                            };
                        },
                    );
                }
                PdfViewerAction::Reveal(path) => match reveal_in_os(&path) {
                    Ok(()) => self.status = format!("已定位 {}。", path.display()),
                    Err(error) => self.status = format!("定位 PDF 失败：{error}"),
                },
                PdfViewerAction::Print(request) => self.open_print_dialog(request, ctx),
                PdfViewerAction::PrintConfirmed(request) => {
                    self.open_print_dialog_confirmed(request, ctx)
                }
                PdfViewerAction::ReleaseMemory {
                    bytes,
                    mut request,
                    print,
                } => {
                    if print {
                        self.open_print_dialog(*request, ctx);
                        continue;
                    }
                    let ctx = ctx.clone();
                    let name = crate::export::safe_filename(&request.title);
                    let name = if name.to_ascii_lowercase().ends_with(".pdf") {
                        name
                    } else {
                        format!("{name}.pdf")
                    };
                    self.request_output_with_warning(
                        format!(
                            "{} PDF 附件：{}。请选择 PDF 的保存位置。",
                            if print {
                                "另存并打印"
                            } else {
                                "另存并系统打开"
                            },
                            request.title
                        ),
                        if print {
                            super::output_guard::PRINT_WARNING
                        } else {
                            super::output_guard::DOCUMENT_WARNING
                        },
                        move |app| {
                            app.request_file(
                                super::file_dialog::FileDialogSpec::save_file()
                                    .filter("PDF", &["pdf"])
                                    .file_name(name),
                                move |app, path| {
                                    let Some(path) = path else { return };
                                    let result =
                                        crate::export::protection::write_final(&path, &bytes);
                                    if let Err(error) = result {
                                        app.status = format!("保存 PDF 附件失败：{error:#}");
                                        return;
                                    }
                                    if print {
                                        request.path = path;
                                        // 保存+打印风险已经在上面确认过，不再重复弹框。
                                        app.open_print_dialog_confirmed(*request, &ctx);
                                    } else {
                                        app.request_output_with_warning(
                                            format!("用系统程序打开 PDF 附件：{}", path.display()),
                                            super::output_guard::EXTERNAL_OPEN_WARNING,
                                            move |app| {
                                                app.status = match open_in_os(&path) {
                                                    Ok(()) => {
                                                        "已保存 PDF 附件并用系统程序打开。".into()
                                                    }
                                                    Err(error) => {
                                                        format!(
                                                            "PDF 已保存，但系统打开失败：{error}"
                                                        )
                                                    }
                                                };
                                            },
                                        );
                                    }
                                },
                            );
                        },
                    );
                }
            }
        }
    }

    /// 同一篇稿件不重复打开：已经开着就切过去。
    pub(crate) fn focus_manuscript(&mut self, id: i64) -> bool {
        match self.doc_index_of_manuscript(id) {
            Some(index) => {
                self.activate_doc(index);
                true
            }
            None => false,
        }
    }

    /// 刷新某篇的“最新已提交版本”基线。打开稿件、保存、提交版本、载入版本
    /// 之后都要刷一次，否则标签上的空心圈会停在旧结论上。
    pub(crate) fn refresh_committed_baseline(&mut self, index: usize) {
        let Some(id) = self.docs.get(index).and_then(|doc| doc.manuscript_id) else {
            if let Some(doc) = self.docs.get_mut(index) {
                doc.set_committed_baseline(None);
            }
            return;
        };
        let latest = self
            .manuscript_store
            .as_ref()
            .and_then(|store| store.latest_manuscript_content(id).ok())
            .flatten()
            .map(|(json, content, _)| (json, content));
        if let Some(doc) = self.docs.get_mut(index) {
            doc.set_committed_baseline(latest);
        }
    }

    /// 稿件的生命周期状态在别处改过之后，把已打开的标签同步过来：
    /// 发布/归档会让标签转只读，退回草稿会让它重新可编辑。
    pub(crate) fn sync_record_status(&mut self, id: i64) {
        let status = self
            .manuscript_store
            .as_mut()
            .and_then(|store| store.get(id).ok())
            .flatten()
            .map(|record| record.status);
        let Some(status) = status else {
            return;
        };
        for doc in self.docs.iter_mut().filter(|d| d.manuscript_id == Some(id)) {
            doc.record_status = status;
        }
    }

    /// 稿件在管理页被删除或归档后，已打开的标签就不再对应库里那条记录了。
    /// 断开关联而不是关掉标签：内容还在，用户可以另存为新稿。
    pub(crate) fn detach_docs_of(&mut self, id: i64) {
        for doc in self.docs.iter_mut().filter(|d| d.manuscript_id == Some(id)) {
            doc.manuscript_id = None;
            doc.loaded_version = None;
            doc.saved_baseline = None;
            doc.committed_baseline = None;
        }
    }

    /// 全局任务或任意一篇稿件的任务在跑。
    pub(crate) fn any_busy(&self) -> bool {
        self.busy
            || self.knowledge_busy
            || self.lexicon_busy
            || self.task_ui.extraction_busy
            || self.task_fill_rx.is_some()
            || self.wiki_frame_rx.is_some()
            || self.wiki_write_rx.is_some()
            || !self.task_auto_queue.is_empty()
            || self.backup_busy
            || self.docs.iter().any(|doc| doc.busy)
            || self.pdfs.iter().any(PdfSession::busy)
            || self.manuscript_pdf_export_busy
            || self.manuscript_package_export_busy
            || self.package_staging_busy
            || self.import_preparation.is_some()
            || self.send_package_exporting()
    }

    /// 当前标签是稿件时返回它；停在导航页时为 None。
    pub(crate) fn active_doc_ref(&self) -> Option<&DraftSession> {
        self.docs.get(self.active_doc)
    }

    pub(crate) fn doc(&self) -> &DraftSession {
        &self.docs[self.active_doc]
    }

    pub(crate) fn doc_mut(&mut self) -> &mut DraftSession {
        &mut self.docs[self.active_doc]
    }

    /// 借出当前会话与它需要的应用级资源，组成起草页这一帧的执行上下文。
    /// 各字段是**互不相交**的可变借用，所以能同时借出而不冲突。
    pub(crate) fn draft_page(&mut self) -> DraftPage<'_> {
        self.draft_page_at(self.active_doc)
    }

    pub(crate) fn draft_page_at(&mut self, index: usize) -> DraftPage<'_> {
        DraftPage {
            doc: &mut self.docs[index],
            config: &mut self.config,
            store: self.manuscript_store.as_mut(),
            sender: &self.sender,
            status: &mut self.status,
            version_switch: &mut self.version_switch,
            revert_confirm: &mut self.revert_confirm,
            actions: &mut self.draft_actions,
            export_links: &mut self.export_links,
            metrics: &mut self.metrics,
        }
    }

    /// 帧末执行起草页回传的动作。
    pub(crate) fn apply_draft_actions(&mut self, ctx: &egui::Context) {
        for action in std::mem::take(&mut self.draft_actions) {
            match action {
                DraftAction::SaveToLibrary => self.save_to_manuscript_library(),
                DraftAction::OpenSendPackage(id) => self.open_send_package(id),
                DraftAction::OpenManuscript(id) => self.open_in_editor(id),
                DraftAction::OpenVersionCommit(scope) => self.open_version_commit(scope),
                DraftAction::OpenStyleSettings => {
                    self.ai_section = super::AiSection::Styles;
                    self.open_page(NavPage::AiPrompts);
                }
                DraftAction::RevertToDraft(id) => {
                    self.transition_status(id, ManuscriptStatus::Draft);
                    self.sync_record_status(id);
                }
                DraftAction::LoadManuscriptVersion {
                    manuscript_id,
                    version_number,
                } => self.load_manuscript_version(manuscript_id, version_number),
                DraftAction::Persist => self.persist(),
                DraftAction::ConfirmPlaintextOutput {
                    operation,
                    warning,
                    action,
                } => {
                    self.request_output_with_warning(operation, warning, action);
                }
                DraftAction::CopyFile { path } => {
                    let wake = ctx.clone();
                    self.request_output_with_warning(
                        "复制导出文件到系统剪贴板",
                        super::output_guard::CLIPBOARD_WARNING,
                        move |app| app.start_file_copy(path, wake),
                    );
                }

                DraftAction::ImportDocument { key } => {
                    let ctx = ctx.clone();
                    self.request_file(crate::doc_import::file_dialog_spec(), move |app, path| {
                        let Some(path) = path else {
                            return;
                        };
                        let Some(index) = app.doc_index_of_key(key) else {
                            return;
                        };
                        app.draft_page_at(index).import_document_from(&ctx, &path);
                    });
                }
                DraftAction::ImportWritingSources { key, role } => {
                    let ctx = ctx.clone();
                    self.request_files(
                        crate::doc_import::knowledge_files_spec(),
                        move |app, paths| {
                            let Some(paths) = paths else { return };
                            let Some(index) = app.doc_index_of_key(key) else {
                                return;
                            };
                            if app.docs[index].read_only()
                                || app.docs[index].busy
                                || app.docs[index].writing.running
                            {
                                app.status = "当前稿件只读或已有任务进行中，请稍后导入材料".into();
                                return;
                            }
                            app.docs[index].writing.import_files(paths, role, ctx);
                            app.docs[index].busy = true;
                        },
                    );
                }
                DraftAction::ExportWritingArtifact { name, text } => {
                    let encrypted = match crate::export::protection::encryption_enabled() {
                        Ok(enabled) => enabled,
                        Err(error) => {
                            self.status = format!("导出设置读取失败：{error:#}");
                            continue;
                        }
                    };
                    let name = if encrypted {
                        std::path::Path::new(&name)
                            .with_extension("7z")
                            .to_string_lossy()
                            .into_owned()
                    } else {
                        name
                    };
                    let spec = super::file_dialog::FileDialogSpec::save_file()
                        .file_name(name)
                        .filter(
                            if encrypted { "加密 7z" } else { "Markdown" },
                            &[if encrypted { "7z" } else { "md" }],
                        );
                    self.request_document_output("导出写作成果 Markdown", move |app| {
                        app.request_file(spec, move |app, path| {
                            if let Some(path) = path {
                                app.status = match crate::export::protection::write_final(
                                    &if encrypted {
                                        path.with_extension("md")
                                    } else {
                                        path.clone()
                                    },
                                    text.as_bytes(),
                                ) {
                                    Ok(target) => format!("写作成果已导出到 {}", target.display()),
                                    Err(error) => format!("导出写作成果失败：{error}"),
                                };
                            }
                        });
                    });
                }
                DraftAction::InsertImages { key } => {
                    let ctx = ctx.clone();
                    self.request_files(crate::images::pick_files_spec(), move |app, files| {
                        let Some(files) = files else {
                            return;
                        };
                        let Some(index) = app.doc_index_of_key(key) else {
                            return;
                        };
                        app.draft_page_at(index).insert_images_from(&ctx, &files);
                    });
                }
                DraftAction::SaveExportCopy {
                    key,
                    kind,
                    mut name,
                } => {
                    let encrypted = match crate::export::protection::encryption_enabled() {
                        Ok(enabled) => enabled,
                        Err(error) => {
                            self.status = format!("导出设置读取失败：{error:#}");
                            continue;
                        }
                    };
                    if encrypted
                        && let Err(error) = crate::export::protection::Protection::current()
                    {
                        self.status = format!("无法导出：{error:#}");
                        continue;
                    }
                    let extension = if encrypted && kind == crate::draft_page::ExportKind::Typst {
                        name = name.trim_end_matches(".zip").to_owned() + ".7z";
                        "7z"
                    } else {
                        kind.extension()
                    };
                    let spec = super::file_dialog::FileDialogSpec::save_file()
                        .file_name(name)
                        .filter(kind.label(), &[extension]);
                    self.request_document_output(
                        "按当前正文和表单重新导出到所选位置",
                        move |app| {
                            app.request_file(spec, move |app, target| {
                                let Some(target) = target else {
                                    return;
                                };
                                let Some(index) = app.doc_index_of_key(key) else {
                                    return;
                                };
                                app.draft_page_at(index).export_current_to(kind, target);
                            });
                        },
                    );
                }
                DraftAction::ImportBibliography { key } => {
                    self.request_file(
                        super::file_dialog::FileDialogSpec::pick_file().filter("BibTeX", &["bib"]),
                        move |app, path| {
                            let Some(path) = path else {
                                return;
                            };
                            let Some(index) = app.doc_index_of_key(key) else {
                                return;
                            };
                            app.draft_page_at(index)
                                .import_bibliography_from(path, false);
                        },
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 会话恢复靠 `key` ↔ `from_key` 对上号。漏登记一个页面，关掉重开时
    /// 那格标签就静默消失了——不会报错，只是没了。
    #[test]
    fn 每个导航页的键都能往返() {
        for page in [
            NavPage::Manuscript,
            NavPage::TaskAssistant,
            NavPage::Vocabulary,
            NavPage::Proofread,
            NavPage::Lexicon,
            NavPage::ImeTable,
            NavPage::AiPrompts,
            NavPage::Knowledge,
            NavPage::Settings,
            NavPage::Help,
        ] {
            assert_eq!(
                NavPage::from_key(page.key()),
                Some(page),
                "{} 的键没登记全",
                page.label()
            );
            assert!(!page.label().is_empty());
        }
    }
}
