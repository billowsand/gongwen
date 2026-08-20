//! 会话恢复、自动保存、关闭确认与全局快捷键。
//!
//! 由 src/app.rs 拆分而来：本文件是模块 `app::session`，与其它子模块共享
//! `app` 根模块的私有可见性（`GongwenApp` 结构体与根模块常量仍在 app.rs 中）。

use crate::app::{
    AUTOSAVE_INTERVAL, GongwenApp, NavPage, TabRef, VersionCommitDraft, VersionScope,
    default_version_name, unique_version_name,
};
use crate::draft_page::{DraftSession, editor_id};
use crate::manuscript;
use crate::storage;
use crate::theme;
use eframe::egui;
use std::path::PathBuf;

/// 退出前的汇总确认。上区必须处理，下区只是提醒。
pub(crate) struct ExitPrompt {
    /// 有改动没写库的稿件：`(docs 下标, 是否勾选保存)`。默认全勾。
    unsaved: Vec<(usize, bool)>,
    /// 已存库但没提交版本的稿件：`(docs 下标, 是否勾选提交)`。默认不勾。
    uncommitted: Vec<(usize, bool)>,
}

/// 起草页需要外壳代办的事：这些动作要么改的是外壳自己的状态，要么会
/// 重排 `docs`，在借出会话的那一帧里做不了，统一延到帧末。
pub(crate) enum DraftAction {
    /// 保存当前稿件到稿件库（新建或更新）。
    SaveToLibrary,
    /// 打开提交版本对话框。
    OpenVersionCommit(VersionScope),
    /// 打开“AI 优化”的提示词选择面板。
    OpenAiPromptPicker,
    /// 打开版本对照窗：`to` 与它的上一版比。
    OpenVersionDiff { manuscript_id: i64, to: i64 },
    /// 把已发布的稿件退回草稿，好继续编辑。
    RevertToDraft(i64),
    /// 把某个已提交版本载入当前起草页。
    LoadManuscriptVersion {
        manuscript_id: i64,
        version_number: i64,
    },
    /// 把当前版式记进配置并落盘。
    Persist,
    /// 打开设置页（功能区「输出 → 导出设置」）。
    OpenSettings,
    /// 在应用内打开最近导出的 PDF。
    OpenPdf(PathBuf),
}

impl GongwenApp {
    /// 关闭标签。稿件有未保存改动时先弹确认，别把人写了一半的稿子直接扔掉。
    pub(crate) fn request_close_tab(&mut self, tab: usize) {
        match self.tabs.get(tab) {
            Some(TabRef::Doc(key)) => {
                let Some(index) = self.doc_index_of_key(*key) else {
                    self.close_tab(tab);
                    return;
                };
                if self.docs[index].is_dirty() {
                    self.close_confirm = Some(index);
                } else {
                    self.close_tab(tab);
                }
            }
            Some(TabRef::Page(_) | TabRef::Pdf(_)) => self.close_tab(tab),
            None => {}
        }
    }

    pub(crate) fn close_tab(&mut self, tab: usize) {
        if tab >= self.tabs.len() {
            return;
        }
        if let TabRef::Doc(key) = self.tabs[tab]
            && let Some(index) = self.doc_index_of_key(key)
        {
            self.docs.remove(index);
            if index < self.active_doc {
                self.active_doc -= 1;
            }
            self.active_doc = self.active_doc.min(self.docs.len().saturating_sub(1));
        }
        if let TabRef::Pdf(key) = self.tabs[tab]
            && let Some(index) = self.pdf_index_of_key(key)
        {
            self.pdfs.remove(index);
        }
        self.tabs.remove(tab);
        if self.tabs.is_empty() {
            // 一格不剩就回到稿件管理，而不是停在空白页。
            self.open_page(NavPage::Manuscript);
            return;
        }
        // 关掉当前这格时顺位接管后一格；关掉前面的则保持指着同一格。
        let next = if tab < self.active_tab {
            self.active_tab - 1
        } else {
            self.active_tab
        };
        self.activate_tab(next.min(self.tabs.len() - 1));
        self.remember_session();
    }

    /// 关闭未保存稿件的二次确认。
    pub(crate) fn close_confirm_window(&mut self, ctx: &egui::Context) {
        let Some(index) = self.close_confirm else {
            theme::reset_window_anim(ctx, egui::Id::new("close_confirm_anim"));
            return;
        };
        let Some(doc) = self.docs.get(index) else {
            self.close_confirm = None;
            return;
        };
        let title = doc.title();
        let key = doc.key;
        let mut decision: Option<bool> = None;
        let mut cancel = false;
        let win = egui::Window::new("关闭稿件")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("《{title}》有未保存的改动。"));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if theme::primary_icon_button(ui, theme::Icon::Save, "保存并关闭").clicked()
                    {
                        decision = Some(true);
                    }
                    if ui
                        .add(theme::secondary_icon_button(theme::Icon::Trash, "不保存"))
                        .clicked()
                    {
                        decision = Some(false);
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                });
            });
        if let Some(w) = win {
            theme::window_enter_anim(ctx, egui::Id::new("close_confirm_anim"), &w.response);
        }
        if cancel {
            self.close_confirm = None;
            return;
        }
        let Some(save) = decision else {
            return;
        };
        self.close_confirm = None;
        if save {
            let previous = self.active_doc;
            self.active_doc = index;
            self.save_to_manuscript_library();
            let saved = !self.docs[index].is_dirty();
            self.active_doc = previous;
            if !saved {
                // 保存失败（归档稿、库不可用等），保留标签，状态栏已说明原因。
                return;
            }
        }
        if let Some(tab) = self.tabs.iter().position(|item| *item == TabRef::Doc(key)) {
            self.close_tab(tab);
        }
    }

    /// 关窗请求的入口。有未保存或未提交的稿件就先拦下来问一次。
    pub(crate) fn handle_close_request(&mut self, ctx: &egui::Context) {
        if !ctx.input(|input| input.viewport().close_requested()) {
            return;
        }
        if self.exit_confirmed {
            return;
        }
        // 开着自动保存时先静默存一轮，能不打扰就不打扰。
        self.autosave_all();
        let unsaved: Vec<(usize, bool)> = self
            .docs
            .iter()
            .enumerate()
            .filter(|(_, doc)| doc.is_dirty())
            .map(|(index, _)| (index, true))
            .collect();
        let uncommitted: Vec<(usize, bool)> = self
            .docs
            .iter()
            .enumerate()
            .filter(|(_, doc)| doc.has_uncommitted())
            .map(|(index, _)| (index, false))
            .collect();
        if unsaved.is_empty() && uncommitted.is_empty() {
            self.remember_session();
            self.exit_confirmed = true;
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        self.exit_prompt = Some(ExitPrompt {
            unsaved,
            uncommitted,
        });
    }

    /// 退出汇总框：上区未保存必须处理，下区未提交版本只是提醒。
    pub(crate) fn exit_prompt_window(&mut self, ctx: &egui::Context) {
        let Some(mut prompt) = self.exit_prompt.take() else {
            theme::reset_window_anim(ctx, egui::Id::new("exit_prompt_anim"));
            return;
        };
        let mut decision: Option<bool> = None;
        let mut cancel = false;
        let win = egui::Window::new("退出公文助手")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_min_width(420.0);
                if !prompt.unsaved.is_empty() {
                    ui.label(egui::RichText::new("以下稿件有未保存的改动").strong());
                    for (index, keep) in prompt.unsaved.iter_mut() {
                        let title = self.docs[*index].title();
                        ui.checkbox(keep, format!("保存《{title}》"));
                    }
                    ui.add_space(6.0);
                }
                if !prompt.uncommitted.is_empty() {
                    ui.label(
                        egui::RichText::new("以下稿件已存库，但相对最新版本还有改动").strong(),
                    );
                    ui.weak("不处理也可以，下次打开继续改；勾选则退出前顺手固化一个版本。");
                    for (index, commit) in prompt.uncommitted.iter_mut() {
                        let title = self.docs[*index].title();
                        ui.checkbox(commit, format!("提交《{title}》的新版本"));
                    }
                    ui.add_space(6.0);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if theme::primary_icon_button(ui, theme::Icon::Save, "处理所选并退出").clicked()
                    {
                        decision = Some(true);
                    }
                    if ui
                        .add(theme::secondary_icon_button(theme::Icon::X, "直接退出"))
                        .on_hover_text("放弃所有未保存的改动")
                        .clicked()
                    {
                        decision = Some(false);
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                });
            });
        if let Some(w) = win {
            theme::window_enter_anim(ctx, egui::Id::new("exit_prompt_anim"), &w.response);
        }
        if cancel {
            return;
        }
        let Some(apply) = decision else {
            self.exit_prompt = Some(prompt);
            return;
        };
        if apply {
            self.apply_exit_actions(&prompt);
        }
        self.remember_session();
        self.exit_confirmed = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// 按汇总框的勾选逐篇保存、逐篇提交版本。
    pub(crate) fn apply_exit_actions(&mut self, prompt: &ExitPrompt) {
        let previous = self.active_doc;
        for (index, keep) in &prompt.unsaved {
            if *keep && *index < self.docs.len() {
                self.active_doc = *index;
                self.save_to_manuscript_library();
            }
        }
        for (index, commit) in &prompt.uncommitted {
            if !*commit || *index >= self.docs.len() {
                continue;
            }
            let Some(id) = self.docs[*index].manuscript_id else {
                continue;
            };
            self.active_doc = *index;
            let existing = self
                .manuscript_store
                .as_mut()
                .and_then(|store| store.list_manuscript_versions(id).ok())
                .unwrap_or_default()
                .into_iter()
                .map(|row| row.name)
                .collect::<Vec<_>>();
            let draft = VersionCommitDraft {
                scope: VersionScope::Manuscript(id),
                name: unique_version_name(&existing, &default_version_name()),
                comment: "退出前自动提交".into(),
                error: None,
            };
            if let Err(error) = self.run_version_commit(&draft) {
                self.status = format!("退出前提交版本失败：{error:#}");
            }
        }
        self.active_doc = previous.min(self.docs.len().saturating_sub(1));
    }

    /// 启动时按上次退出的标签重建现场。稿件已被删掉的行静默跳过；
    /// 一格都恢复不出来就保留启动时那篇空白稿。
    pub(crate) fn restore_session(&mut self) {
        let Some(store) = self.manuscript_store.as_mut() else {
            return;
        };
        let Ok((saved, active)) = store.load_open_tabs() else {
            return;
        };
        if saved.is_empty() {
            return;
        }
        let mut tabs: Vec<TabRef> = Vec::new();
        let mut docs: Vec<DraftSession> = Vec::new();
        let mut next_key = 0;
        let mut restored_active = 0;
        for (ord, tab) in saved.iter().enumerate() {
            let item = match tab {
                manuscript::OpenTab::Manuscript(id) => {
                    let Some(store) = self.manuscript_store.as_mut() else {
                        continue;
                    };
                    let Ok(Some(record)) = store.get(*id) else {
                        continue;
                    };
                    let mut session = DraftSession::from_parts(
                        next_key,
                        Some(record.id),
                        record.snapshot,
                        record.content_markdown,
                    );
                    session.record_status = record.status;
                    session.mark_saved();
                    next_key += 1;
                    let key = session.key;
                    docs.push(session);
                    TabRef::Doc(key)
                }
                manuscript::OpenTab::Page(name) => match NavPage::from_key(name) {
                    Some(page) => TabRef::Page(page),
                    None => continue,
                },
            };
            if ord == active {
                restored_active = tabs.len();
            }
            tabs.push(item);
        }
        if tabs.is_empty() {
            return;
        }
        self.docs = docs;
        self.tabs = tabs;
        self.next_doc_key = next_key;
        self.activate_tab(restored_active.min(self.tabs.len() - 1));
        for index in 0..self.docs.len() {
            self.refresh_committed_baseline(index);
            self.draft_page_at(index).revalidate();
        }
        self.status = format!("已恢复上次打开的 {} 个标签。", self.tabs.len());
    }

    /// 把当前标签写进稿件库，供下次启动恢复。未入库的新稿没有身份，跳过。
    pub(crate) fn remember_session(&mut self) {
        let tabs: Vec<manuscript::OpenTab> = self
            .tabs
            .iter()
            .filter_map(|item| match item {
                TabRef::Doc(key) => {
                    let index = self.doc_index_of_key(*key)?;
                    let id = self.docs[index].manuscript_id?;
                    Some(manuscript::OpenTab::Manuscript(id))
                }
                TabRef::Page(page) => Some(manuscript::OpenTab::Page(page.key().to_string())),
                TabRef::Pdf(_) => None,
            })
            .collect();
        // 过滤掉未入库的稿件后下标会错位，按身份重新定位当前这一格。
        let active = self
            .tabs
            .get(self.active_tab)
            .and_then(|item| match item {
                TabRef::Doc(key) => {
                    let index = self.doc_index_of_key(*key)?;
                    let id = self.docs[index].manuscript_id?;
                    tabs.iter()
                        .position(|t| *t == manuscript::OpenTab::Manuscript(id))
                }
                TabRef::Page(page) => tabs
                    .iter()
                    .position(|t| *t == manuscript::OpenTab::Page(page.key().to_string())),
                TabRef::Pdf(_) => None,
            })
            .unwrap_or(0);
        if let Some(store) = self.manuscript_store.as_mut() {
            let _ = store.save_open_tabs(&tabs, active);
        }
    }

    /// 尚未入库的新稿，一旦真的动过就静默建一条草稿记录。此后它就有了身份，
    /// 自动保存、会话恢复、版本链才有落脚点。
    pub(crate) fn auto_create_touched_doc(&mut self) {
        if !self.showing_doc() {
            return;
        }
        let Some(doc) = self.docs.get(self.active_doc) else {
            return;
        };
        if doc.manuscript_id.is_some() || doc.busy || !doc.touched() {
            return;
        }
        self.save_to_manuscript_library();
        self.remember_session();
    }

    /// 定时自动保存：把有改动且已入库的稿件静默写回。
    pub(crate) fn autosave_tick(&mut self) {
        if !self.config.auto_save {
            return;
        }
        if self.last_autosave.elapsed() < AUTOSAVE_INTERVAL {
            return;
        }
        self.last_autosave = std::time::Instant::now();
        self.autosave_all();
    }

    pub(crate) fn autosave_all(&mut self) {
        if !self.config.auto_save {
            return;
        }
        let targets: Vec<usize> = (0..self.docs.len())
            .filter(|&index| {
                let doc = &self.docs[index];
                doc.manuscript_id.is_some() && doc.is_dirty() && !doc.busy
            })
            .collect();
        let previous = self.active_doc;
        for index in targets {
            self.active_doc = index;
            self.save_to_manuscript_library();
        }
        self.active_doc = previous.min(self.docs.len().saturating_sub(1));
    }

    pub(crate) fn persist(&mut self) {
        // 设置页可在没有任何稿件标签时打开（关掉全部 Doc 标签后 docs 为空），
        // 此时不能索引 docs[active_doc]，否则越界 panic 直接退出进程。
        // 先取出所需的不变数据结束对 docs 的借用，再写回 config。
        let current = self
            .active_doc_ref()
            .map(|doc| (doc.draft.kind, doc.draft.profile.clone()));
        if let Some((kind, profile)) = current {
            self.config.last_template = kind;
            self.config.upsert_profile(profile);
        }
        match storage::save(&self.config) {
            Ok(()) => self.status = "配置已保存到本机。".into(),
            Err(error) => self.status = format!("保存配置失败：{error:#}"),
        }
    }

    /// 应用级快捷键要在各个文本框处理输入前消费，避免保存/查找
    /// 被当前聚焦的编辑控件吞掉。
    pub(crate) fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        #[cfg(target_os = "macos")]
        {
            // macOS 标准窗口快捷键：交给 winit/AppKit 执行，状态会通过
            // ViewportInfo 回流，和绿色按钮、菜单栏触发的结果保持一致。
            let minimize = egui::KeyboardShortcut::new(egui::Modifiers::MAC_CMD, egui::Key::M);
            if ctx.input_mut(|input| input.consume_shortcut(&minimize)) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }

            let toggle_fullscreen = egui::KeyboardShortcut::new(
                egui::Modifiers::MAC_CMD | egui::Modifiers::CTRL,
                egui::Key::F,
            );
            if ctx.input_mut(|input| input.consume_shortcut(&toggle_fullscreen)) {
                let fullscreen = ctx
                    .input(|input| input.viewport().fullscreen)
                    .unwrap_or(false);
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
            }
        }

        let save = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::S);
        if ctx.input_mut(|input| input.consume_shortcut(&save)) && self.showing_doc() {
            self.save_to_manuscript_library();
        }

        // 主快捷键+B 与功能区「格式 → 加粗」是同一件事。这里必须确认焦点
        // 确实在审校稿编辑框上，避免焦点在要素表单或查找框时改到正文。
        let bold = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::B);
        if ctx.memory(|memory| memory.has_focus(editor_id()))
            && ctx.input_mut(|input| input.consume_shortcut(&bold))
            && self.showing_doc()
        {
            self.draft_page().toggle_bold(ctx);
        }

        let new_doc = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::N);
        if ctx.input_mut(|input| input.consume_shortcut(&new_doc)) {
            self.new_blank_manuscript();
        }

        if self.showing_doc() && !self.docs.is_empty() {
            let close = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::W);
            if ctx.input_mut(|input| input.consume_shortcut(&close)) {
                self.request_close_tab(self.active_tab);
            }
            // macOS 的 Command+Tab 由系统用于切换应用，因此稿件前后切换特意
            // 保留 Control+Tab / Control+Shift+Tab；其他平台沿用原有组合。
            let next = egui::KeyboardShortcut::new(egui::Modifiers::CTRL, egui::Key::Tab);
            if ctx.input_mut(|input| input.consume_shortcut(&next)) {
                self.activate_doc((self.active_doc + 1) % self.docs.len());
            }
            let prev = egui::KeyboardShortcut::new(
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                egui::Key::Tab,
            );
            if ctx.input_mut(|input| input.consume_shortcut(&prev)) {
                self.activate_doc((self.active_doc + self.docs.len() - 1) % self.docs.len());
            }
        }
        // 主快捷键+1..9 直达第 N 个标签，第 9 个固定指最后一篇。
        for (offset, key) in [
            egui::Key::Num1,
            egui::Key::Num2,
            egui::Key::Num3,
            egui::Key::Num4,
            egui::Key::Num5,
            egui::Key::Num6,
            egui::Key::Num7,
            egui::Key::Num8,
            egui::Key::Num9,
        ]
        .into_iter()
        .enumerate()
        {
            let shortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, key);
            if ctx.input_mut(|input| input.consume_shortcut(&shortcut)) && !self.docs.is_empty() {
                let index = if offset == 8 {
                    self.docs.len() - 1
                } else {
                    offset.min(self.docs.len() - 1)
                };
                self.activate_doc(index);
            }
        }

        let find = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::F);
        if ctx.input_mut(|input| input.consume_shortcut(&find)) && self.showing_doc() {
            if !self.doc().markdown_find.open
                && let Some(state) = egui::TextEdit::load_state(ctx, editor_id())
                && let Some(range) = state.cursor.char_range()
                && !range.is_empty()
            {
                let selected = range.slice_str(&self.doc().generated_markdown);
                if !selected.contains('\n') && selected.chars().count() <= 200 {
                    self.doc_mut().markdown_find.query = selected.to_owned();
                    self.doc_mut().markdown_find.current = 0;
                }
            }
            self.doc_mut().markdown_find.open = true;
            self.doc_mut().markdown_find.focus_query = true;
        }

        // 主快捷键 + / - / 0 调整源码编辑器字号，与编辑器里 Ctrl+滚轮等价。
        // Ctrl+= 与 Ctrl+Shift+=（即 Ctrl++）都算放大。
        if self.showing_doc() {
            let zoom_in = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Equals);
            let zoom_in_shift = egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::Equals,
            );
            let zoom_out = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Minus);
            let zoom_reset = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Num0);
            let current = self.config.editor_font_size;
            let next = ctx.input_mut(|input| {
                if input.consume_shortcut(&zoom_in) || input.consume_shortcut(&zoom_in_shift) {
                    Some(current + 1.0)
                } else if input.consume_shortcut(&zoom_out) {
                    Some(current - 1.0)
                } else if input.consume_shortcut(&zoom_reset) {
                    Some(14.0)
                } else {
                    None
                }
            });
            if let Some(size) = next {
                self.config.editor_font_size = size.clamp(
                    crate::models::EDITOR_FONT_SIZE_MIN,
                    crate::models::EDITOR_FONT_SIZE_MAX,
                );
                let _ = storage::save(&self.config);
            }
        }

        // 查找条与结果抽屉同时打开时，Esc 先交给查找条；否则收起结果抽屉。
        if self.showing_doc()
            && self.doc().result_drawer_open
            && !self.doc().markdown_find.open
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.doc_mut().result_drawer_open = false;
        }
    }
}
