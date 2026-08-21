//! 标签、导航与文档/PDF 的打开、切换与关闭管理。
//!
//! 由 src/app.rs 拆分而来：本文件是模块 `app::tabs`，与其它子模块共享
//! `app` 根模块的私有可见性（`GongwenApp` 结构体与根模块常量仍在 app.rs 中）。

use crate::app::{
    DraftAction, GongwenApp, VersionDiffState, VersionScope, WorkerResult, open_in_os, reveal_in_os,
};
use crate::diff_view::DiffViewState;
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
    Vocabulary,
    Proofread,
    AiPrompts,
    Knowledge,
    Settings,
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
            Self::Vocabulary => "vocabulary",
            Self::Proofread => "proofread",
            Self::AiPrompts => "ai_prompts",
            Self::Knowledge => "knowledge",
            Self::Settings => "settings",
        }
    }

    pub(crate) fn from_key(key: &str) -> Option<Self> {
        match key {
            "manuscript" => Some(Self::Manuscript),
            "vocabulary" => Some(Self::Vocabulary),
            "proofread" => Some(Self::Proofread),
            "ai_prompts" => Some(Self::AiPrompts),
            "knowledge" => Some(Self::Knowledge),
            "settings" => Some(Self::Settings),
            _ => None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Manuscript => "稿件管理",
            Self::Vocabulary => "标准词库",
            Self::Proofread => "校对词表",
            Self::AiPrompts => "AI 管理",
            Self::Knowledge => "知识库",
            Self::Settings => "设置",
        }
    }

    pub(crate) fn icon(self) -> theme::Icon {
        match self {
            Self::Manuscript => theme::Icon::Library,
            Self::Vocabulary => theme::Icon::Book,
            Self::Proofread => theme::Icon::SquareCheck,
            Self::AiPrompts => theme::Icon::WandSparkles,
            Self::Knowledge => theme::Icon::PackageOpen,
            Self::Settings => theme::Icon::Settings,
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
        if let Some(index) = self.pdfs.iter().position(|pdf| pdf.path() == path)
            && let Some(tab) = self
                .tabs
                .iter()
                .position(|item| *item == TabRef::Pdf(self.pdfs[index].key))
        {
            self.activate_tab(tab);
            return;
        }
        let key = self.next_pdf_key;
        self.next_pdf_key += 1;
        self.pdfs
            .push(PdfSession::new(key, path, title, self.sender.clone()));
        self.tabs.push(TabRef::Pdf(key));
        self.activate_tab(self.tabs.len() - 1);
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
                PdfViewerAction::OpenExternal(path) => match open_in_os(&path) {
                    Ok(()) => self.status = format!("已用系统程序打开 {}。", path.display()),
                    Err(error) => self.status = format!("打开 PDF 失败：{error}"),
                },
                PdfViewerAction::Reveal(path) => match reveal_in_os(&path) {
                    Ok(()) => self.status = format!("已定位 {}。", path.display()),
                    Err(error) => self.status = format!("定位 PDF 失败：{error}"),
                },
                // 打印要弹系统对话框并逐页光栅化，可能花几秒，放后台线程，
                // 主界面照常可用；结果经 WorkerResult 回状态栏。
                PdfViewerAction::Print(path) => {
                    self.status = format!("正在准备打印 {}…", path.display());
                    let tx = self.sender.clone();
                    let ctx = ctx.clone();
                    std::thread::spawn(move || {
                        let result = crate::print_pdf::print_pdf(&path);
                        let _ = tx.send(WorkerResult::PdfPrinted { path, result });
                        // 主线程可能正闲着睡着，主动敲醒它。
                        ctx.request_repaint();
                    });
                }
            }
        }
    }

    /// 同一篇稿件不重复打开：已经开着就切过去。
    pub(crate) fn focus_manuscript(&mut self, id: i64) -> bool {
        let found = self
            .docs
            .iter()
            .position(|doc| doc.manuscript_id == Some(id));
        match found {
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
            || self.docs.iter().any(|doc| doc.busy)
            || self.pdfs.iter().any(PdfSession::busy)
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
        }
    }

    /// 帧末执行起草页回传的动作。
    pub(crate) fn apply_draft_actions(&mut self) {
        for action in std::mem::take(&mut self.draft_actions) {
            match action {
                DraftAction::SaveToLibrary => self.save_to_manuscript_library(),
                DraftAction::OpenVersionCommit(scope) => self.open_version_commit(scope),
                DraftAction::OpenAiWorkbench { selection } => self.open_ai_workbench(selection),
                DraftAction::OpenVersionDiff { manuscript_id, to } => {
                    self.version_diff = Some(VersionDiffState {
                        scope: VersionScope::Manuscript(manuscript_id),
                        from: (to > 1).then_some(to - 1),
                        to: Some(to),
                        to_is_current_config: false,
                        view: DiffViewState::default(),
                    });
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
                DraftAction::OpenSettings => self.open_page(NavPage::Settings),
                DraftAction::OpenPdf(path) => self.open_pdf(path, None),
            }
        }
    }
}
