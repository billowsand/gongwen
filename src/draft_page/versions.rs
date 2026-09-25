//! 版本切换、版本行数据与回退。
//!
//! 由 src/draft_page.rs 拆分而来：本文件是模块 `draft_page::versions`，与其它子模块共享
//! `draft_page` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。
//! 第 ④ 期起旧的右侧版本抽屉下线，版本浏览并入版本对照模式左侧的时间轴
//! （`draft_page::timeline` + `version_diff` 的 `timeline_ui`）。

use crate::app::{DraftAction, VersionSwitchPrompt, VersionTarget, version_hover};
use crate::draft_page::{DraftPage, PreviewMode};
use crate::manuscript;
use eframe::egui;
use std::ops::Range;

impl DraftPage<'_> {
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
