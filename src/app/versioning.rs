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
use crate::diff_view::{DiffViewConfig, DiffViewState};
use crate::draft_page::{DraftSession, LoadedVersion};
use crate::manuscript::{ManuscriptUpdate, VersionRecord};
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
    pub(crate) view: DiffViewState,
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
        let versions = self
            .manuscript_store
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
            diff.view.reset();
        }
        if let Some(number) = picked_to {
            diff.to = Some(number);
            // 新侧往前挪时旧侧可能变得不再更早，顺手把它退回上一版。
            if diff.from.is_some_and(|old| old >= number) {
                diff.from = (number > 1).then_some(number - 1);
            }
            diff.view.reset();
        }
        if picked_from.is_some() || picked_to.is_some() {
            return; // 选择变了：下一帧按新选择重画，免得这一帧算旧的。
        }

        let old = from
            .and_then(|number| {
                self.manuscript_store
                    .as_mut()
                    .and_then(|store| store.get_manuscript_version(manuscript_id, number).ok())
                    .flatten()
            })
            .map(diff::ContentSnapshot::from)
            .unwrap_or_else(|| {
                diff::ContentSnapshot::new(DraftInput::default(), String::new(), String::new())
            });
        let Some(new_record) = self
            .manuscript_store
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
        let old_label = from.map_or_else(|| "（空白）".to_string(), |number| format!("v{number}"));
        let new_label = format!("v{to}");
        let report = diff::manuscript_diff(&old, &new_record.into());
        let config = DiffViewConfig {
            old_label: &old_label,
            new_label: &new_label,
            // 这里看的可能不是起草页正在编辑的那篇稿件，跳源码会落到无关位置。
            allow_jump: false,
            // 这里还没接花脸稿的后台导出任务，先不显示按钮。
            allow_export: false,
        };
        diff_view::manuscript_diff_ui(ui, &report, &mut diff.view, &config);
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
                view: DiffViewState::default(),
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
