//! 版本管理：提交版本、版本对照、回退与配置版本。
//!
//! 由 src/app.rs 拆分而来：本文件是模块 `app::versioning`，与其它子模块共享
//! `app` 根模块的私有可见性（`GongwenApp` 结构体与根模块常量仍在 app.rs 中）。

use crate::app::{
    GongwenApp, accent, default_version_name, short_date, unique_version_name, version_hover,
    version_label, warn,
};
use crate::diff;
use crate::diff_view;
use crate::draft_page::{DraftSession, LoadedVersion};
use crate::manuscript::{ManuscriptStore, ManuscriptUpdate, VersionRecord};
use crate::models::DraftInput;
use crate::storage;
use crate::theme;
use crate::units;
use anyhow::Context;
use eframe::egui;

/// 提交版本对话框打开的版本链：某篇稿件，或全局配置。
#[derive(Debug, Clone)]
pub(crate) enum VersionScope {
    Manuscript(i64),
    Config,
}

/// 提交版本对话框的输入：版本名称、注释、最近一次提交尝试的错误。
pub(crate) struct VersionCommitDraft {
    pub(crate) scope: VersionScope,
    pub(crate) name: String,
    pub(crate) comment: String,
    pub(crate) error: Option<String>,
}

/// 版本对照窗的选版状态。方向由字段名固定：`from` 恒为旧版、`to` 恒为新版，
/// 所以"变更前/变更后"不可能再被选反。
pub(crate) struct VersionDiffState {
    pub(crate) scope: VersionScope,
    /// 旧侧版本号；稿件的 v1 没有上一版，此时为 None，整篇算新增。
    pub(crate) from: Option<i64>,
    /// 新侧版本号。
    pub(crate) to: Option<i64>,
    /// 仅配置版用：新侧取"当前配置"而不是某个已提交版本。稿件版没有这个选项
    /// ——详情页看的稿件未必是起草页正在编辑的那篇，拿起草页内容当新侧会串稿。
    pub(crate) to_is_current_config: bool,
    /// 稿件版的对照视图：共享只读版本对组件，按 (稿件, 旧版, 新版) 缓存。
    pub(crate) pair: crate::version_pair_view::VersionPairViewState,
}

/// 版本切换的目标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VersionTarget {
    /// 回到稿件库活稿行（"当前未提交内容"）。
    Working,
    /// 某个已提交版本。
    Version(i64),
}

/// 切换版本前的三选确认：当前内容相对最新版有未提交修改，先问怎么处置。
pub(crate) struct VersionSwitchPrompt {
    pub(crate) manuscript_id: i64,
    pub(crate) target: VersionTarget,
    /// 当前内容所基于的版本号，用于文案。
    pub(crate) base_label: String,
}

impl GongwenApp {
    /// 切换版本前的三选确认：提交为新版本后切换 / 丢弃修改并切换 / 取消。
    pub(crate) fn version_switch_window(&mut self, ctx: &egui::Context) {
        let Some(prompt) = self.version_switch.take() else {
            theme::reset_window_anim(ctx, egui::Id::new("version_switch_anim"));
            return;
        };
        let target_label = match prompt.target {
            VersionTarget::Version(number) => format!("v{number}"),
            VersionTarget::Working => "未提交内容".to_string(),
        };
        let mut commit_first = false;
        let mut discard = false;
        let mut cancel = false;
        let win = egui::Window::new("切换版本")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("当前内容相对{}有未提交修改。", prompt.base_label));
                ui.colored_label(warn(), format!("直接切到{target_label}会丢弃这些修改。"));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if theme::primary_icon_button(ui, theme::Icon::GitCommit, "提交为新版本后切换")
                        .on_hover_text("先把当前修改固化为一个新版本，再切过去，什么都不丢")
                        .clicked()
                    {
                        commit_first = true;
                    }
                    if ui
                        .add(theme::warning_icon_button(
                            theme::Icon::Undo,
                            "丢弃修改并切换",
                        ))
                        .on_hover_text("丢弃当前未提交的修改，直接切到目标版本")
                        .clicked()
                    {
                        discard = true;
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                });
            });
        if let Some(w) = win {
            theme::window_enter_anim(ctx, egui::Id::new("version_switch_anim"), &w.response);
        }
        if commit_first {
            self.switch_after_commit = Some(prompt.target);
            self.open_version_commit(VersionScope::Manuscript(prompt.manuscript_id));
            return;
        }
        if discard {
            self.draft_page()
                .apply_version_switch(prompt.manuscript_id, prompt.target);
            return;
        }
        if !cancel {
            self.version_switch = Some(prompt);
        }
    }

    /// 打开提交版本对话框：预填默认时间戳版本名（同名自动加序号）、空注释。
    pub(crate) fn open_version_commit(&mut self, scope: VersionScope) {
        let base = default_version_name();
        let name = match &scope {
            VersionScope::Manuscript(id) => {
                let names = self
                    .manuscript_store
                    .as_mut()
                    .and_then(|store| store.list_manuscript_versions(*id).ok())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|row| row.name)
                    .collect::<Vec<_>>();
                unique_version_name(&names, &base)
            }
            VersionScope::Config => {
                let names = self
                    .manuscript_store
                    .as_mut()
                    .and_then(|store| store.list_config_versions().ok())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|row| row.name)
                    .collect::<Vec<_>>();
                unique_version_name(&names, &base)
            }
        };
        self.version_commit = Some(VersionCommitDraft {
            scope,
            name,
            comment: String::new(),
            error: None,
        });
    }

    /// 提交版本对话框（稿件版 / 配置版共用）。
    pub(crate) fn version_commit_window(&mut self, ctx: &egui::Context) {
        let Some(mut draft) = self.version_commit.take() else {
            theme::reset_window_anim(ctx, egui::Id::new("version_commit_anim"));
            return;
        };
        // 实时预览：相对上一版本是否有变更（与名称/注释无关，先算出来避免闭包借用冲突）。
        let has_changes = match &draft.scope {
            VersionScope::Manuscript(id) => {
                let snapshot = self.doc().draft.clone();
                let content = self.doc().generated_markdown.clone();
                let notes = self
                    .manuscript_store
                    .as_ref()
                    .and_then(|store| store.notes_of(*id).ok())
                    .flatten()
                    .unwrap_or_default();
                self.manuscript_store
                    .as_mut()
                    .and_then(|store| {
                        store
                            .manuscript_version_changed(*id, &snapshot, &content, &notes)
                            .ok()
                    })
                    .unwrap_or(true)
            }
            VersionScope::Config => {
                let config = self.config.clone();
                self.manuscript_store
                    .as_mut()
                    .and_then(|store| store.config_version_changed(&config).ok())
                    .unwrap_or(true)
            }
        };
        let mut close = false;
        let mut submit = false;
        let win = egui::Window::new("提交版本")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("版本名称（默认时间戳，可修改）");
                ui.add(
                    egui::TextEdit::singleline(&mut draft.name)
                        .desired_width(380.0)
                        .hint_text("如 2026-08-07 09:35"),
                );
                ui.label("注释");
                ui.add(
                    egui::TextEdit::multiline(&mut draft.comment)
                        .desired_rows(3)
                        .desired_width(380.0),
                );
                if !has_changes {
                    ui.colored_label(warn(), "相对上一版本没有内容变更，不能提交。");
                }
                if let Some(error) = &draft.error {
                    ui.colored_label(warn(), error);
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(has_changes, egui::Button::new("提交"))
                        .on_hover_text("提交后固化为一个新版本，追加在版本链末尾")
                        .clicked()
                    {
                        submit = true;
                    }
                    if ui.button("取消").clicked() {
                        close = true;
                    }
                });
            });
        if let Some(w) = win {
            theme::window_enter_anim(ctx, egui::Id::new("version_commit_anim"), &w.response);
        }
        if close {
            // 取消提交时也放弃"提交后切换"，免得下次提交莫名跳版本。
            self.switch_after_commit = None;
            return; // 关闭：丢弃草稿。
        }
        if submit {
            match self.run_version_commit(&draft) {
                Ok(message) => {
                    self.doc_mut().loaded_version = None;
                    self.doc_mut().mark_saved();
                    self.refresh_committed_baseline(self.active_doc);
                    self.manuscript_dirty = true;
                    self.status = message;
                    self.doc_mut().draft_diff.view.reset();
                    // "提交为新版本后切换"：提交成功了才真正切过去。
                    if let Some(target) = self.switch_after_commit.take()
                        && let VersionScope::Manuscript(id) = &draft.scope
                    {
                        self.draft_page().apply_version_switch(*id, target);
                    }
                    // 成功：不恢复 draft，对话框关闭。
                }
                Err(error) => {
                    draft.error = Some(format!("{error:#}"));
                    self.version_commit = Some(draft);
                }
            }
        } else {
            self.version_commit = Some(draft);
        }
    }

    /// 执行提交：先同步活稿行 / 配置，再写入版本链。返回状态消息或错误。
    pub(crate) fn run_version_commit(
        &mut self,
        draft: &VersionCommitDraft,
    ) -> anyhow::Result<String> {
        let name = draft.name.trim();
        anyhow::ensure!(!name.is_empty(), "版本名称不能为空");
        let comment = draft.comment.trim();
        match &draft.scope {
            VersionScope::Manuscript(id) => {
                let id = *id;
                let snapshot = self.doc().draft.clone();
                let content = self.doc().generated_markdown.clone();
                let store = self
                    .manuscript_store
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
                let notes = store.notes_of(id)?.context("稿件不存在，无法提交版本")?;
                store.update(
                    id,
                    &ManuscriptUpdate {
                        snapshot: snapshot.clone(),
                        content_markdown: content.clone(),
                        notes: notes.clone(),
                    },
                )?;
                let row = store
                    .commit_manuscript_version(id, name, comment, &snapshot, &content, &notes)?;
                Ok(format!(
                    "已提交版本《{}》（v{}）。",
                    row.name, row.version_number
                ))
            }
            VersionScope::Config => {
                storage::save(&self.config)?;
                let store = self
                    .manuscript_store
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
                let row = store.commit_config_version(name, comment, &self.config)?;
                Ok(format!(
                    "已提交配置版本《{}》（v{}）。",
                    row.name, row.version_number
                ))
            }
        }
    }

    /// 版本对照窗（稿件版 / 配置版共用）。
    pub(crate) fn version_diff_window(&mut self, ctx: &egui::Context) {
        let Some(mut diff) = self.version_diff.take() else {
            theme::reset_window_anim(ctx, egui::Id::new("version_diff_anim"));
            return;
        };
        let scope = diff.scope.clone();
        // 关闭按钮交给标题栏：正文对照是个撑满高度的滚动区，放在它下面的页脚
        // 会被顶出可视区，点不到。
        let mut open = true;
        let win = egui::Window::new("版本对照")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(900.0)
            .default_height(620.0)
            // 不设上限时滚动区会把窗口撑满整个屏幕高度。
            .max_height(760.0)
            .show(ctx, |ui| match scope {
                VersionScope::Manuscript(id) => self.manuscript_diff_ui(ui, id, &mut diff),
                VersionScope::Config => self.config_diff_ui(ui, &mut diff),
            });
        if let Some(w) = win {
            theme::window_enter_anim(ctx, egui::Id::new("version_diff_anim"), &w.response);
        }
        if !open {
            return;
        }
        self.version_diff = Some(diff);
    }

    pub(crate) fn manuscript_diff_ui(
        &mut self,
        ui: &mut egui::Ui,
        manuscript_id: i64,
        diff: &mut VersionDiffState,
    ) {
        let store = self.manuscript_store.as_mut();
        let display = units::UnitDisplay::new(&self.config.vocabulary);
        manuscript_diff_ui_impl(
            ui,
            store,
            &display,
            &self.config.numbering,
            manuscript_id,
            diff,
        );
    }

    pub(crate) fn config_diff_ui(&mut self, ui: &mut egui::Ui, diff: &mut VersionDiffState) {
        let versions = self
            .manuscript_store
            .as_mut()
            .and_then(|store| store.list_config_versions().ok())
            .unwrap_or_default();
        if versions.is_empty() {
            ui.weak("还没有配置版本。在“标准词库”页点“提交配置版本”开始记录历史。");
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label("从");
            let from_label = diff
                .from
                .and_then(|n| versions.iter().find(|v| v.version_number == n))
                .map(|v| format!("v{} · {}", v.version_number, v.name))
                .unwrap_or_else(|| "请选择".to_string());
            egui::ComboBox::from_id_salt(("cdiff_from", 0))
                .selected_text(from_label)
                .width(240.0)
                .show_ui(ui, |ui| {
                    for v in &versions {
                        ui.selectable_value(
                            &mut diff.from,
                            Some(v.version_number),
                            format!("v{} · {} {}", v.version_number, v.name, v.comment),
                        );
                    }
                });
            ui.checkbox(&mut diff.to_is_current_config, "到当前配置");
            if !diff.to_is_current_config {
                ui.label("到");
                let to_label = diff
                    .to
                    .and_then(|n| versions.iter().find(|v| v.version_number == n))
                    .map(|v| format!("v{} · {}", v.version_number, v.name))
                    .unwrap_or_else(|| "请选择".to_string());
                egui::ComboBox::from_id_salt(("cdiff_to", 0))
                    .selected_text(to_label)
                    .width(240.0)
                    .show_ui(ui, |ui| {
                        for v in &versions {
                            ui.selectable_value(
                                &mut diff.to,
                                Some(v.version_number),
                                format!("v{} · {} {}", v.version_number, v.name, v.comment),
                            );
                        }
                    });
            }
        });
        let from_num = diff
            .from
            .filter(|n| versions.iter().any(|v| v.version_number == *n));
        let to_num = if diff.to_is_current_config {
            None
        } else {
            diff.to
                .filter(|n| versions.iter().any(|v| v.version_number == *n))
        };
        let old = from_num.and_then(|n| {
            self.manuscript_store
                .as_mut()
                .and_then(|store| store.get_config_version(n).ok())
                .flatten()
        });
        let new = if diff.to_is_current_config {
            Some(self.config.clone())
        } else {
            to_num.and_then(|n| {
                self.manuscript_store
                    .as_mut()
                    .and_then(|store| store.get_config_version(n).ok())
                    .flatten()
            })
        };
        let (Some(a), Some(b)) = (old, new) else {
            ui.weak("请选择两个版本进行对照。");
            return;
        };
        let old_label = from_num.map_or_else(|| "变更前".to_string(), |n| format!("v{n}"));
        let new_label = if diff.to_is_current_config {
            "当前配置".to_string()
        } else {
            to_num.map_or_else(|| "变更后".to_string(), |n| format!("v{n}"))
        };
        let report = diff::config_changes(&a, &b);
        ui.separator();
        ui.strong("词库变更");
        if report.vocabulary.is_empty() {
            ui.weak("词库无变化。");
        } else {
            for change in &report.vocabulary {
                let color = match change.action {
                    "新增" => theme::success(),
                    "删除" => theme::warn(),
                    _ => theme::accent(),
                };
                ui.horizontal(|ui| {
                    ui.colored_label(
                        color,
                        format!("{}·{}", change.category.label(), change.action),
                    );
                    ui.label(&change.label);
                });
                for field in &change.changes {
                    ui.horizontal(|ui| {
                        ui.add_space(18.0);
                        ui.label(field.label);
                        ui.weak(if field.before.is_empty() {
                            "—".to_string()
                        } else {
                            field.before.clone()
                        });
                        ui.label("→");
                        ui.colored_label(
                            accent(),
                            if field.after.is_empty() {
                                "—".to_string()
                            } else {
                                field.after.clone()
                            },
                        );
                    });
                }
            }
        }
        ui.separator();
        ui.strong("版式变更");
        if report.profiles.is_empty() {
            ui.weak("版式无变化。");
        } else {
            for kind in &report.profiles {
                ui.strong(kind.kind.label());
                diff_view::field_changes_table(ui, &kind.changes, &old_label, &new_label);
            }
        }
        ui.separator();
        ui.strong("设置变更");
        if report.settings.is_empty() {
            ui.weak("设置无变化。");
        } else {
            diff_view::field_changes_table(ui, &report.settings, &old_label, &new_label);
        }
    }

    /// 配置版本历史窗：列表 + 应用（二次确认）+ 对照。
    pub(crate) fn config_versions_window(&mut self, ctx: &egui::Context) {
        if !self.config_versions_open {
            theme::reset_window_anim(ctx, egui::Id::new("config_versions_anim"));
            return;
        }
        let versions = self
            .manuscript_store
            .as_mut()
            .and_then(|store| store.list_config_versions().ok())
            .unwrap_or_default();
        let mut close = false;
        let mut open_diff: Option<i64> = None;
        let win = egui::Window::new("配置版本历史")
            .collapsible(false)
            .resizable(true)
            .default_width(700.0)
            .default_height(460.0)
            .show(ctx, |ui| {
                if versions.is_empty() {
                    ui.weak("还没有配置版本。修改词库或设置后点“提交配置版本”开始记录历史。");
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("config_versions_list")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for v in &versions {
                                ui.horizontal(|ui| {
                                    ui.label(format!("v{}", v.version_number));
                                    ui.label(&v.name);
                                    if !v.comment.is_empty() {
                                        ui.weak(&v.comment);
                                    }
                                    ui.weak(short_date(&v.created_at));
                                    if v.is_latest {
                                        ui.weak("最新");
                                    }
                                    if ui
                                        .add(theme::icon_text_button(
                                            theme::Icon::RotateCcw,
                                            "应用",
                                        ))
                                        .on_hover_text("用该版本替换当前配置（词库、版式、设置）")
                                        .clicked()
                                    {
                                        self.config_apply_confirm = Some(v.version_number);
                                    }
                                    if theme::icon_button(ui, theme::Icon::Compare, "对照版本")
                                        .clicked()
                                    {
                                        open_diff = Some(v.version_number);
                                    }
                                });
                            }
                        });
                }
                if let Some(n) = self.config_apply_confirm {
                    ui.add_space(6.0);
                    ui.group(|ui| {
                        ui.colored_label(
                            warn(),
                            format!("将用配置版本 v{n} 替换当前配置，未保存的词库更改会丢失。"),
                        );
                        ui.horizontal(|ui| {
                            if ui
                                .add(egui::Button::new(
                                    egui::RichText::new("确认应用").color(warn()),
                                ))
                                .clicked()
                            {
                                match self.apply_config_version(n) {
                                    Ok(message) => self.status = message,
                                    Err(error) => {
                                        self.status = format!("应用配置版本失败：{error:#}")
                                    }
                                }
                            }
                            if ui.button("取消").clicked() {
                                self.config_apply_confirm = None;
                            }
                        });
                    });
                }
                if theme::icon_button(ui, theme::Icon::X, "关闭窗口").clicked() {
                    close = true;
                }
            });
        if let Some(w) = win {
            theme::window_enter_anim(ctx, egui::Id::new("config_versions_anim"), &w.response);
        }
        if let Some(n) = open_diff {
            self.version_diff = Some(VersionDiffState {
                scope: VersionScope::Config,
                // 配置也按"从旧到新"：v1 没有上一版时跟当前配置比。
                from: (n > 1).then_some(n - 1).or(Some(n)),
                to: Some(n),
                to_is_current_config: n <= 1,
                pair: crate::version_pair_view::VersionPairViewState::default(),
            });
        }
        if close {
            self.config_versions_open = false;
            self.config_apply_confirm = None;
        }
    }

    /// 应用配置版本：覆盖内存配置、整理词库、写回 config.json。
    pub(crate) fn apply_config_version(&mut self, version_number: i64) -> anyhow::Result<String> {
        let store = self
            .manuscript_store
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
        let config = store
            .get_config_version(version_number)?
            .context("配置版本不存在")?;
        self.config = config;
        units::normalize(&mut self.config.vocabulary);
        storage::save(&self.config)?;
        self.config_apply_confirm = None;
        Ok(format!("已应用配置版本 v{version_number}。"))
    }

    /// 回退到某版本：用该版内容覆盖活稿行，并载入起草页。与"载入编辑"的差别就在
    /// 这一步写库——载入只是看看，回退是把稿件库里的当前稿改回去。版本链不动，
    /// 之后提交仍是追加新版本。
    pub(crate) fn revert_to_version(&mut self, manuscript_id: i64, version_number: i64) {
        let result: anyhow::Result<VersionRecord> = (|| {
            let store = self
                .manuscript_store
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
            let record = store
                .get_manuscript_version(manuscript_id, version_number)?
                .context("版本不存在或已被删除")?;
            let notes = store
                .notes_of(manuscript_id)?
                .context("稿件不存在，无法回退")?;
            store.update(
                manuscript_id,
                &ManuscriptUpdate {
                    snapshot: record.snapshot.clone(),
                    content_markdown: record.content_markdown.clone(),
                    notes,
                },
            )?;
            Ok(record)
        })();
        self.revert_confirm = None;
        match result {
            Ok(record) => {
                if !self.focus_manuscript(manuscript_id) {
                    self.open_doc(DraftSession::from_parts(
                        0,
                        Some(manuscript_id),
                        record.snapshot.clone(),
                        record.content_markdown.clone(),
                    ));
                }
                self.doc_mut().draft = record.snapshot;
                self.doc_mut().generated_markdown = record.content_markdown;
                self.doc_mut().manuscript_id = Some(manuscript_id);
                // 内容已经写回活稿行，就是"当前未提交内容"，不再挂历史版本横幅。
                self.doc_mut().loaded_version = None;
                self.doc_mut().reset_transient();
                self.doc_mut().mark_saved();
                self.refresh_committed_baseline(self.active_doc);
                self.manuscript_detail = None;
                self.manuscript_dirty = true;
                self.doc_mut().draft_diff.view.reset();
                self.draft_page().revalidate();
                let next = self
                    .manuscript_store
                    .as_mut()
                    .and_then(|store| store.list_manuscript_versions(manuscript_id).ok())
                    .and_then(|rows| rows.last().map(|row| row.version_number + 1))
                    .unwrap_or(1);
                self.status =
                    format!("已回退到 v{version_number} 的内容；继续修改后提交将追加为 v{next}。");
            }
            Err(error) => self.status = format!("回退失败：{error:#}"),
        }
    }

    /// "回退到该版本"的二次确认：会覆盖活稿行里未提交的内容，值得问一句。
    pub(crate) fn revert_confirm_window(&mut self, ctx: &egui::Context) {
        let Some((manuscript_id, version_number)) = self.revert_confirm else {
            theme::reset_window_anim(ctx, egui::Id::new("revert_confirm_anim"));
            return;
        };
        let mut confirm = false;
        let mut cancel = false;
        let win = egui::Window::new("回退到该版本")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!(
                    "将用 v{version_number} 的内容覆盖这篇稿件的当前内容。"
                ));
                ui.colored_label(warn(), "当前未提交的修改会丢失；已提交的版本不受影响。");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(theme::warning_icon_button(
                            theme::Icon::RotateCcw,
                            "确认回退",
                        ))
                        .clicked()
                    {
                        confirm = true;
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                });
            });
        if let Some(w) = win {
            theme::window_enter_anim(ctx, egui::Id::new("revert_confirm_anim"), &w.response);
        }
        if confirm {
            self.revert_to_version(manuscript_id, version_number);
        } else if cancel {
            self.revert_confirm = None;
        }
    }

    /// 把某版本载入起草页继续编辑（不改版本链、不改活稿行）。
    pub(crate) fn load_manuscript_version(&mut self, manuscript_id: i64, version_number: i64) {
        let result: anyhow::Result<Option<VersionRecord>> = (|| {
            let store = self
                .manuscript_store
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
            store.get_manuscript_version(manuscript_id, version_number)
        })();
        match result {
            Ok(Some(record)) => {
                let name = record.name.clone();
                // 从详情页载入旧版时这篇未必开着；先把它的标签找出来或开出来，
                // 免得把版本内容写到别人的稿子上。
                if !self.focus_manuscript(manuscript_id) {
                    self.open_doc(DraftSession::from_parts(
                        0,
                        Some(manuscript_id),
                        record.snapshot.clone(),
                        record.content_markdown.clone(),
                    ));
                }
                self.doc_mut().draft = record.snapshot;
                self.doc_mut().generated_markdown = record.content_markdown;
                self.doc_mut().manuscript_id = Some(manuscript_id);
                self.doc_mut().loaded_version = Some(LoadedVersion {
                    manuscript_id,
                    version_number,
                    name,
                });
                self.doc_mut().reset_transient();
                self.manuscript_detail = None;
                self.draft_page().revalidate();
                self.status =
                    format!("已载入版本 v{version_number}，可在起草页继续修改后提交为新版本。");
            }
            Ok(None) => self.status = "版本不存在或已被删除。".into(),
            Err(error) => self.status = format!("载入版本失败：{error:#}"),
        }
    }
}

/// 稿件版本对照窗本体（抽成自由函数便于单测）：保留「从 vA 到 vB」两个选择器，
/// 对照区改用与起草页时间轴相同的只读版本对组件（左统一 diff + 右花脸稿预览）。
/// 这里看的未必是起草页正在编辑的那篇稿件，组件双击定位源码的返回值一律忽略。
#[allow(clippy::too_many_arguments)]
pub(crate) fn manuscript_diff_ui_impl(
    ui: &mut egui::Ui,
    mut store: Option<&mut ManuscriptStore>,
    display: &units::UnitDisplay<'_>,
    numbering: &crate::models::NumberingConfig,
    manuscript_id: i64,
    diff: &mut VersionDiffState,
) {
    let versions = store
        .as_mut()
        .and_then(|store| store.list_manuscript_versions(manuscript_id).ok())
        .unwrap_or_default();
    if versions.is_empty() {
        ui.weak("该稿件还没有版本。到起草页点“提交版本”开始记录历史。");
        return;
    }
    let numbers: Vec<i64> = versions.iter().map(|row| row.version_number).collect();
    let latest = numbers.last().copied().unwrap_or(1);
    // 新侧兜底到最新版；旧侧兜底到它的上一版（v1 没有上一版，此时旧侧为空）。
    let to = diff
        .to
        .filter(|number| numbers.contains(number))
        .unwrap_or(latest);
    let from = diff
        .from
        .filter(|number| numbers.contains(number) && *number < to);
    let mut picked_from: Option<Option<i64>> = None;
    let mut picked_to = None;
    ui.horizontal_wrapped(|ui| {
        ui.label("从")
            .on_hover_text("左旧右新：左侧选旧版本，右侧选新版本。");
        let from_label = from.map_or_else(
            || "（空白，整篇算新增）".to_string(),
            |number| version_label(&versions, number),
        );
        egui::ComboBox::from_id_salt(("vdiff_from", manuscript_id))
            .selected_text(from_label)
            .width(230.0)
            .show_ui(ui, |ui| {
                // 只能选比新侧更早的版本：方向永远是旧→新，选不出颠倒的组合。
                if ui
                    .selectable_label(from.is_none(), "（空白，整篇算新增）")
                    .clicked()
                {
                    picked_from = Some(None);
                }
                for row in versions.iter().rev().filter(|row| row.version_number < to) {
                    if ui
                        .selectable_label(
                            from == Some(row.version_number),
                            version_label(&versions, row.version_number),
                        )
                        .on_hover_text(version_hover(row))
                        .clicked()
                    {
                        picked_from = Some(Some(row.version_number));
                    }
                }
            });
        ui.label("到");
        egui::ComboBox::from_id_salt(("vdiff_to", manuscript_id))
            .selected_text(version_label(&versions, to))
            .width(230.0)
            .show_ui(ui, |ui| {
                for row in versions.iter().rev() {
                    if ui
                        .selectable_label(
                            to == row.version_number,
                            version_label(&versions, row.version_number),
                        )
                        .on_hover_text(version_hover(row))
                        .clicked()
                    {
                        picked_to = Some(row.version_number);
                    }
                }
            });
    });
    if let Some(number) = picked_from {
        diff.from = number;
    }
    if let Some(number) = picked_to {
        diff.to = Some(number);
        // 新侧往前挪时旧侧可能变得不再更早，顺手把它退回上一版。
        if diff.from.is_some_and(|old| old >= number) {
            diff.from = (number > 1).then_some(number - 1);
        }
    }
    if picked_from.is_some() || picked_to.is_some() {
        return; // 选择变了：下一帧按新选择重画，免得这一帧算旧的。
    }

    let old = from
        .and_then(|number| {
            store
                .as_mut()
                .and_then(|store| store.get_manuscript_version(manuscript_id, number).ok())
                .flatten()
        })
        .map(diff::ContentSnapshot::from)
        .unwrap_or_else(|| {
            diff::ContentSnapshot::new(DraftInput::default(), String::new(), String::new())
        });
    let Some(new_record) = store
        .as_mut()
        .and_then(|store| store.get_manuscript_version(manuscript_id, to).ok())
        .flatten()
    else {
        ui.weak("版本不存在或已被删除。");
        return;
    };
    ui.weak(format!(
        "v{}《{}》{}（{}）",
        new_record.version_number,
        new_record.name,
        if new_record.comment.is_empty() {
            ""
        } else {
            new_record.comment.as_str()
        },
        short_date(&new_record.created_at),
    ));
    ui.separator();
    let key = crate::version_pair_view::VersionPairKey {
        manuscript_id,
        old_version_number: from,
        new_version_number: to,
    };
    if !diff.pair.matches(key) {
        let new = diff::ContentSnapshot::from(new_record);
        diff.pair.set_pair(key, &old, &new, display);
    }
    let old_label = from.map_or_else(|| "（空白）".to_string(), |number| format!("v{number}"));
    let new_label = format!("v{to}");
    let _edit_source = diff.pair.show(
        ui,
        egui::Id::new(("manuscript_pair", key)),
        &old_label,
        &new_label,
        display,
        numbering,
    );
}

#[cfg(test)]
mod tests {
    //! 稿件管理版本对照窗：内存库 + 真 egui 上下文画一帧的冒烟检查。

    use super::*;
    use crate::manuscript::NewManuscript;
    use crate::models::{FontConfig, ManuscriptStatus, NumberingConfig};
    use std::path::Path;

    const OLD: &str = "# 关于报送材料的函\n\n请于八月十日前报送材料。\n\n保留段落。";
    const NEW: &str =
        "# 关于报送材料的函\n\n请于八月十五日前报送材料。\n\n保留段落。\n\n新增段落。";

    fn texts(output: &egui::FullOutput) -> String {
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(shape) => Some(shape.galley.text().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 对照窗改用共享只读版本对组件后：两个选择器还在，左统一 diff、右花脸稿
    /// 两栏都出字，哨兵不漏到纸上，敲字不改库里的版本内容。
    #[test]
    fn manuscript_diff_window_draws_the_shared_readonly_pair_view() {
        let ctx = egui::Context::default();
        theme::configure_icons(&ctx);
        theme::configure_fonts(&ctx, &FontConfig::default());
        let mut store = ManuscriptStore::open(Path::new(":memory:")).unwrap();
        let snapshot = DraftInput::default();
        let id = store
            .create(
                &NewManuscript {
                    snapshot: snapshot.clone(),
                    content_markdown: OLD.into(),
                    notes: String::new(),
                    status: ManuscriptStatus::Draft,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        store
            .commit_manuscript_version(id, "送审稿", "", &snapshot, OLD, "")
            .unwrap();
        store
            .commit_manuscript_version(id, "定稿", "按反馈修改", &snapshot, NEW, "")
            .unwrap();

        let mut diff = VersionDiffState {
            scope: VersionScope::Manuscript(id),
            from: Some(1),
            to: Some(2),
            to_is_current_config: false,
            pair: crate::version_pair_view::VersionPairViewState::default(),
        };
        let mut clock = 0.0;
        let mut frame = |store: &mut ManuscriptStore,
                         diff: &mut VersionDiffState,
                         events: Vec<egui::Event>|
         -> egui::FullOutput {
            clock += 0.05;
            ctx.clone().run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1600.0, 1000.0),
                    )),
                    time: Some(clock),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let display = units::UnitDisplay::new(&[]);
                    manuscript_diff_ui_impl(
                        ui,
                        Some(store),
                        &display,
                        &NumberingConfig::default(),
                        id,
                        diff,
                    );
                },
            )
        };

        let output = frame(&mut store, &mut diff, Vec::new());
        let text = texts(&output);
        assert!(text.contains('从'), "从 / 到选择器：{text}");
        assert!(text.contains('到'), "{text}");
        assert!(text.contains("v2《定稿》按反馈修改"), "{text}");
        // 共享组件的头部与两侧内容。
        assert!(text.contains("v1 → v2"), "{text}");
        assert!(text.contains("请于八月十日前报送材料。"), "{text}");
        assert!(text.contains("请于八月十五日前报送材料。"), "{text}");
        assert!(text.contains("新增段落。"), "{text}");
        assert!(
            !text.chars().any(crate::export::is_redline_sentinel),
            "哨兵不能漏到纸上：{text}"
        );

        // 只读：对照窗里敲字不会写进库里的版本。
        let stored = store
            .get_manuscript_version(id, 2)
            .unwrap()
            .unwrap()
            .content_markdown;
        frame(
            &mut store,
            &mut diff,
            vec![egui::Event::Text("不得编辑".into())],
        );
        assert_eq!(
            store
                .get_manuscript_version(id, 2)
                .unwrap()
                .unwrap()
                .content_markdown,
            stored
        );
    }
}
