//! 版本抽屉、版本对照、版本切换与回退。
//!
//! 由 src/draft_page.rs 拆分而来：本文件是模块 `draft_page::versions`，与其它子模块共享
//! `draft_page` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::app::{
    DraftAction, VersionSwitchPrompt, VersionTarget, summarize, truncate, version_hover,
};
use crate::diff;
use crate::diff_view;
use crate::diff_view::{DiffViewAction, DiffViewConfig};
use crate::draft_page::{DraftPage, PreviewMode};
use crate::manuscript;
use crate::models::DraftInput;
use crate::theme;
use eframe::egui;
use std::ops::Range;

impl DraftPage<'_> {
    /// 右侧版本抽屉：本篇的版本链在起草页里就地看完，不再跳去稿件管理。
    /// 返回是否点了关闭按钮——关闭请求由 `create_ui` 在面板动画之外落地，
    /// 闭包内直接改 `self.doc.versions_open` 会被局部副本写回覆盖。
    pub(crate) fn versions_drawer(&mut self, ui: &mut egui::Ui) -> bool {
        let mut close_requested = false;
        ui.horizontal(|ui| {
            ui.strong("版本历史");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, theme::Icon::X, "收起版本历史").clicked() {
                    close_requested = true;
                }
            });
        });
        ui.separator();
        let Some(id) = self.doc.manuscript_id else {
            ui.weak("这篇还没保存到稿件库，先点“保存”。");
            return close_requested;
        };
        let versions = self.draft_version_rows();
        if versions.is_empty() {
            ui.weak("还没有提交过版本。点标题栏的“提交版本”固化当前内容。");
            return close_requested;
        }
        let current = self.current_version_target();
        let mut load: Option<i64> = None;
        let mut diff: Option<i64> = None;
        egui::ScrollArea::vertical()
            .id_salt("draft_versions_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // 最新版在最上：回看历史通常从近往远找。
                for row in versions.iter().rev() {
                    let active = current == VersionTarget::Version(row.version_number);
                    let frame = if active {
                        theme::card().fill(theme::accent_soft())
                    } else {
                        theme::card()
                    };
                    frame.show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!("v{}", row.version_number)).strong(),
                            );
                            ui.label(truncate(&row.name, 14));
                            if row.is_latest {
                                theme::chip(ui, "最新", theme::success(), theme::success_soft());
                            }
                        });
                        if !row.comment.trim().is_empty() {
                            ui.weak(summarize(&row.comment, 40));
                        }
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(!active, egui::Button::new("载入编辑").small())
                                .on_hover_text("把这一版内容载入起草页继续改；提交会追加为新版本")
                                .clicked()
                            {
                                load = Some(row.version_number);
                            }
                            if ui
                                .add(egui::Button::new("与上一版对照").small())
                                .on_hover_text(version_hover(row))
                                .clicked()
                            {
                                diff = Some(row.version_number);
                            }
                        });
                    });
                    ui.add_space(4.0);
                }
            });
        if let Some(version_number) = load {
            self.request_version_switch(VersionTarget::Version(version_number));
        }
        if let Some(to) = diff {
            self.actions.push(DraftAction::OpenVersionDiff {
                manuscript_id: id,
                to,
            });
        }
        close_requested
    }

    /// 起草页的"版本对照"模式：左侧是已提交的基准版本，右侧是当前未提交内容。
    /// 只读——点某处改动会切回 Markdown 模式并把光标定到那一段。
    pub(crate) fn version_diff_mode_ui(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.doc.manuscript_id else {
            self.diff_placeholder(
                ui,
                "这篇稿件还没有保存到稿件库。先点“保存到稿件库”，再提交一个版本，才有可对照的基准。",
            );
            return;
        };
        let versions = self.draft_version_rows();
        if versions.is_empty() {
            self.diff_placeholder(
                ui,
                "这篇稿件还没有提交过版本。点“提交版本”固化第一版之后，这里会逐字标出后续修改。",
            );
            return;
        }
        let latest = versions.last().map_or(1, |row| row.version_number);
        // 基准默认跟着最新版走：提交一版之后对照自动以新版为基准，不用手动换选。
        let base = self
            .doc
            .draft_diff
            .base
            .filter(|number| versions.iter().any(|row| row.version_number == *number))
            .unwrap_or(latest);

        let mut picked_base = None;
        ui.horizontal_wrapped(|ui| {
            ui.label("基准版本");
            let base_label = versions
                .iter()
                .find(|row| row.version_number == base)
                .map_or_else(
                    || format!("v{base}"),
                    |row| {
                        format!(
                            "v{} · {}{}",
                            row.version_number,
                            row.name,
                            if row.is_latest { " · 最新" } else { "" }
                        )
                    },
                );
            egui::ComboBox::from_id_salt("draft_diff_base")
                .selected_text(base_label)
                .width(220.0)
                .show_ui(ui, |ui| {
                    for row in versions.iter().rev() {
                        if ui
                            .selectable_label(
                                row.version_number == base,
                                format!(
                                    "v{} · {}{}",
                                    row.version_number,
                                    row.name,
                                    if row.is_latest { " · 最新" } else { "" }
                                ),
                            )
                            .on_hover_text(version_hover(row))
                            .clicked()
                        {
                            picked_base = Some(row.version_number);
                        }
                    }
                })
                .response
                .on_hover_text("换成更早的版本，可以看到从那一版至今累计改了什么");
            ui.label("→");
            ui.strong("当前未提交内容");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(theme::icon_text_button(
                        theme::Icon::RotateCcw,
                        "回退到基准版",
                    ))
                    .on_hover_text("用基准版内容覆盖当前稿件内容（需二次确认）")
                    .clicked()
                {
                    *self.revert_confirm = Some((id, base));
                }
            });
        });
        if let Some(number) = picked_base {
            self.doc.draft_diff.base = Some(number);
            self.doc.draft_diff.view.reset();
        }
        ui.separator();

        self.sync_draft_diff(id, base);
        let old_label = format!("v{base}（已提交）");
        let config = DiffViewConfig {
            old_label: &old_label,
            new_label: "当前（未提交）",
            allow_jump: true,
        };
        // 缓存与视图状态是同一个结构体的两个字段，分别借用互不冲突。
        let Some((_, report)) = &self.doc.draft_diff.cache else {
            return;
        };
        let action =
            diff_view::manuscript_diff_ui(ui, report, &mut self.doc.draft_diff.view, &config);
        if let Some(DiffViewAction::JumpToSource(range)) = action {
            self.jump_to_source(range);
        }
    }

    /// 版本对照模式下的空状态提示。
    pub(crate) fn diff_placeholder(&self, ui: &mut egui::Ui, message: &str) {
        ui.add_space(28.0);
        ui.vertical_centered(|ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(message).color(theme::text_muted()))
                    .wrap_mode(egui::TextWrapMode::Wrap),
            );
        });
    }

    /// 重算起草页对照结果，输入没变就复用上一帧的。这个模式每帧都要渲染，
    /// 长稿边打字边全量 diff 会掉帧。
    pub(crate) fn sync_draft_diff(&mut self, id: i64, base: i64) {
        let notes = self
            .store
            .as_deref()
            .and_then(|store| store.notes_of(id).ok())
            .flatten()
            .unwrap_or_default();
        let draft_json = serde_json::to_string(&self.doc.draft).unwrap_or_default();
        let key = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::hash::DefaultHasher::new();
            id.hash(&mut hasher);
            base.hash(&mut hasher);
            self.doc.generated_markdown.hash(&mut hasher);
            notes.hash(&mut hasher);
            draft_json.hash(&mut hasher);
            hasher.finish()
        };
        if self
            .doc
            .draft_diff
            .cache
            .as_ref()
            .is_some_and(|(cached, _)| *cached == key)
        {
            return;
        }
        let old = self
            .store
            .as_deref_mut()
            .and_then(|store| store.get_manuscript_version(id, base).ok())
            .flatten()
            .map(diff::ContentSnapshot::from)
            .unwrap_or_else(|| {
                diff::ContentSnapshot::new(DraftInput::default(), String::new(), String::new())
            });
        let new = diff::ContentSnapshot::new(
            self.doc.draft.clone(),
            self.doc.generated_markdown.clone(),
            notes,
        );
        self.doc.draft_diff.cache = Some((key, diff::manuscript_diff(&old, &new)));
    }

    /// 切回 Markdown 源码并把光标 / 选区定位到给定范围。
    pub(crate) fn jump_to_source(&mut self, range: Range<usize>) {
        self.doc.preview_mode = PreviewMode::Source;
        self.select_find_match(Some(range));
    }

    /// 起草页的版本切换下拉：紧跟"提交版本"，让"改完提交"与"回看旧版"在同一处闭合。
    pub(crate) fn version_switch_picker(&mut self, ui: &mut egui::Ui) {
        let versions = self.draft_version_rows();
        let enabled = self.doc.manuscript_id.is_some() && !versions.is_empty();
        let current = self.current_version_target();
        // 功能区寸土寸金：这里只显示版本号，完整名称交给悬停说明。
        let label = match current {
            VersionTarget::Version(number) => format!("v{number}"),
            VersionTarget::Working => "未提交".to_string(),
        };
        let mut picked = None;
        ui.add_enabled_ui(enabled, |ui| {
            let response = egui::ComboBox::from_id_salt("draft_version_switch")
                .selected_text(label)
                .width(84.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(current == VersionTarget::Working, "当前未提交内容")
                        .on_hover_text("稿件库中这篇稿件的当前内容（尚未固化为版本）")
                        .clicked()
                    {
                        picked = Some(VersionTarget::Working);
                    }
                    ui.separator();
                    // 最新版在最上：回看历史时通常是从近往远找。
                    for row in versions.iter().rev() {
                        let text = format!(
                            "v{} · {}{}",
                            row.version_number,
                            row.name,
                            if row.is_latest { " · 最新" } else { "" }
                        );
                        if ui
                            .selectable_label(
                                current == VersionTarget::Version(row.version_number),
                                text,
                            )
                            .on_hover_text(version_hover(row))
                            .clicked()
                        {
                            picked = Some(VersionTarget::Version(row.version_number));
                        }
                    }
                })
                .response;
            if self.doc.manuscript_id.is_none() {
                response.on_hover_text("先“保存到稿件库”，才能切换版本");
            } else if versions.is_empty() {
                response.on_hover_text("这篇稿件还没有提交过版本，先点“提交版本”");
            } else {
                response.on_hover_text("切换起草页显示的版本；有未提交修改时会先问你怎么处置");
            }
        });
        if let Some(target) = picked {
            self.request_version_switch(target);
        }
    }

    /// 起草页当前稿件的版本列表；没打开稿件时为空。
    pub(crate) fn draft_version_rows(&mut self) -> Vec<manuscript::VersionRow> {
        let Some(id) = self.doc.manuscript_id else {
            return Vec::new();
        };
        self.store
            .as_deref_mut()
            .and_then(|store| store.list_manuscript_versions(id).ok())
            .unwrap_or_default()
    }

    /// 起草页内容当前对应哪个版本：载入过某版就是它，否则是活稿行。
    pub(crate) fn current_version_target(&self) -> VersionTarget {
        match (&self.doc.loaded_version, self.doc.manuscript_id) {
            (Some(loaded), Some(id)) if loaded.manuscript_id == id => {
                VersionTarget::Version(loaded.version_number)
            }
            _ => VersionTarget::Working,
        }
    }

    /// 请求切换版本：先看内存内容相对来源有没有改动，有就弹三选确认，没有就直接切。
    pub(crate) fn request_version_switch(&mut self, target: VersionTarget) {
        let Some(id) = self.doc.manuscript_id else {
            return;
        };
        let current = self.current_version_target();
        if current == target {
            return;
        }
        if self.draft_has_unsaved_edits(id) {
            let base_label = match current {
                VersionTarget::Version(number) => format!("v{number}"),
                VersionTarget::Working => "稿件库中的当前稿".to_string(),
            };
            *self.version_switch = Some(VersionSwitchPrompt {
                manuscript_id: id,
                target,
                base_label,
            });
        } else {
            self.apply_version_switch(id, target);
        }
    }

    /// 内存里的内容相对"它的来源"有没有改动：载入了某版就跟那一版比，否则跟活稿行比。
    /// 按来源比而不是一律跟最新版比，没动过手的切换才不会被反复追问。
    pub(crate) fn draft_has_unsaved_edits(&mut self, id: i64) -> bool {
        let origin = match self.current_version_target() {
            VersionTarget::Version(number) => self
                .store
                .as_deref_mut()
                .and_then(|store| store.get_manuscript_version(id, number).ok())
                .flatten()
                .map(|record| (record.snapshot, record.content_markdown)),
            VersionTarget::Working => self
                .store
                .as_deref()
                .and_then(|store| store.snapshot_of(id).ok())
                .flatten(),
        };
        let Some((snapshot, content)) = origin else {
            return false;
        };
        content != self.doc.generated_markdown
            || serde_json::to_string(&snapshot).ok() != serde_json::to_string(&self.doc.draft).ok()
    }

    pub(crate) fn apply_version_switch(&mut self, id: i64, target: VersionTarget) {
        match target {
            VersionTarget::Version(number) => {
                self.actions.push(DraftAction::LoadManuscriptVersion {
                    manuscript_id: id,
                    version_number: number,
                })
            }
            VersionTarget::Working => self.load_working_copy(id),
        }
    }

    /// 切回"当前未提交内容"：从活稿行重载。活稿行是最后一次"保存到稿件库"的结果，
    /// 也就是离开历史版本后唯一还能取回的工作区。
    pub(crate) fn load_working_copy(&mut self, id: i64) {
        let loaded = self
            .store
            .as_deref()
            .and_then(|store| store.snapshot_of(id).ok())
            .flatten();
        match loaded {
            Some((snapshot, content)) => {
                self.doc.draft = snapshot;
                self.doc.generated_markdown = content;
                self.doc.loaded_version = None;
                self.doc.output_files.clear();
                self.doc.export_error = None;
                self.doc.draft_diff.view.reset();
                self.revalidate();
                *self.status = "已切回未提交内容（稿件库中这篇稿件的当前内容）。".into();
            }
            None => *self.status = "稿件不存在或稿件库不可用。".into(),
        }
    }
}
