//! 标准词库页：词库树、单位/人员编辑与 xlsx 导入导出。
//!
//! 由 src/app.rs 拆分而来：本文件是模块 `app::vocabulary`，与其它子模块共享
//! `app` 根模块的私有可见性（`GongwenApp` 结构体与根模块常量仍在 app.rs 中）。

use super::file_dialog::FileDialogSpec;
use crate::app::{
    GongwenApp, VersionScope, VocabularyMoveDraft, unique_name, vocabulary_depths,
    vocabulary_matches, warn, wrapped_hint,
};
use crate::modal::{self, Dismiss};
use crate::models::{VocabularyCategory, VocabularyEntry, VocabularySetupStatus, split_units};
use crate::storage;
use crate::theme;
use crate::units;
use crate::units::{SiblingPosition, UnitDisplay};
use crate::vocabulary_xlsx;
use eframe::egui;
use std::collections::BTreeMap;

#[path = "vocabulary_drag.rs"]
mod drag;
pub(super) use drag::SortUndo;

/// 同理，词库树上的增删和排序也要等本帧渲染完再改动 `Vec`。
pub(crate) enum VocabAction {
    /// 新增单位。`parent` 为空表示顶层单位。
    AddUnit {
        parent: String,
        position: SiblingPosition,
    },
    /// 在指定单位下新增人员。
    AddPerson {
        unit: String,
        position: SiblingPosition,
    },
    /// 删除词条；删除单位时连同其下级单位与人员一并删除。
    Delete(u64),
    /// 撤销最近一次同级排序。
    UndoSort,
    /// 把词条精确放到同级列表中的指定位置。
    Place { id: u64, position: SiblingPosition },
    /// 改变所属层级并放到目标同级列表中的精确位置。
    Relocate {
        id: u64,
        destination: String,
        position: SiblingPosition,
    },
    /// 清空当前标准词库。
    Clear,
}

/// 词库树上的一行：单位按层级缩进，人员是所属单位下面的末端子节点。
pub(crate) struct TreeRow {
    index: usize,
    id: u64,
    depth: usize,
    is_unit: bool,
    has_children: bool,
}

/// 词库树的行高与列宽。每一行按同一套网格摆放：行首拖动手柄、逐级缩进、展开箭头槽、
/// 类别图标、名称与说明、右侧编号列、最右的「＋」。展开箭头槽对所有行都预留，
/// 有没有下级都不会让同级名称错位。
const TREE_ROW_HEIGHT: f32 = 28.0;
const TREE_INDENT: f32 = 18.0;
const TREE_GRIP: f32 = 18.0;
const TREE_CHEVRON: f32 = 16.0;
const TREE_ICON: f32 = 16.0;
const TREE_MENU: f32 = 22.0;
const TREE_MIN_TEXT: f32 = 72.0;

/// 行内「＋」与右键共用的菜单：新增下级、新增同级、本级内移动。
fn vocabulary_row_menu(
    ui: &mut egui::Ui,
    row: &TreeRow,
    code: &str,
    parent_code: &str,
    owner_code: &str,
    action: &mut Option<VocabAction>,
) {
    let id = row.id;
    let mut pick = |ui: &mut egui::Ui, label: &str, chosen: VocabAction| {
        if ui.button(label).clicked() {
            *action = Some(chosen);
            ui.close();
        }
    };
    if row.is_unit {
        pick(
            ui,
            "添加下级单位",
            VocabAction::AddUnit {
                parent: code.to_string(),
                position: SiblingPosition::Last,
            },
        );
        pick(
            ui,
            "添加人员",
            VocabAction::AddPerson {
                unit: code.to_string(),
                position: SiblingPosition::Last,
            },
        );
        ui.separator();
        pick(
            ui,
            "在本单位上方添加同级单位",
            VocabAction::AddUnit {
                parent: parent_code.to_string(),
                position: SiblingPosition::Before(id),
            },
        );
        pick(
            ui,
            "在本单位下方添加同级单位",
            VocabAction::AddUnit {
                parent: parent_code.to_string(),
                position: SiblingPosition::After(id),
            },
        );
    } else {
        pick(
            ui,
            "在本人员上方添加人员",
            VocabAction::AddPerson {
                unit: owner_code.to_string(),
                position: SiblingPosition::Before(id),
            },
        );
        pick(
            ui,
            "在本人员下方添加人员",
            VocabAction::AddPerson {
                unit: owner_code.to_string(),
                position: SiblingPosition::After(id),
            },
        );
    }
    ui.separator();
    pick(
        ui,
        "移到本级最前",
        VocabAction::Place {
            id,
            position: SiblingPosition::First,
        },
    );
    pick(
        ui,
        "移到本级最后",
        VocabAction::Place {
            id,
            position: SiblingPosition::Last,
        },
    );
}

/// 单行文字放进固定矩形；放不下时省略号截断，悬停看全文。
fn tree_label(ui: &mut egui::Ui, rect: egui::Rect, text: egui::RichText, align: egui::Align) {
    if rect.width() <= 0.0 {
        return;
    }
    let layout = if align == egui::Align::Max {
        egui::Layout::right_to_left(egui::Align::Center)
    } else {
        egui::Layout::left_to_right(egui::Align::Center)
    };
    ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(layout))
        .add(egui::Label::new(text).truncate().selectable(false));
}

impl GongwenApp {
    pub(crate) fn import_vocabulary_xlsx(&mut self) {
        self.request_file(
            FileDialogSpec::pick_file().filter("Excel 词库", &["xlsx"]),
            |app, path| {
                let Some(path) = path else {
                    return;
                };
                app.vocabulary_import_conflicts = None;
                let result = (|| -> Result<(usize, vocabulary_xlsx::MergeReport), String> {
                    let mut staged_config = app.config.clone();
                    let existing_codes: Vec<String> = app
                        .config
                        .vocabulary
                        .iter()
                        .filter(|e| e.category == VocabularyCategory::Unit)
                        .map(|e| e.code.trim().to_string())
                        .filter(|c| !c.is_empty())
                        .collect();
                    let report = vocabulary_xlsx::parse(&path, &existing_codes)
                        .map_err(|error| error.summary)?;
                    let parsed_count = report.entries.len();
                    let merge =
                        vocabulary_xlsx::merge(&mut staged_config.vocabulary, report.entries)?;
                    units::normalize(&mut staged_config.vocabulary);
                    if !staged_config.vocabulary.is_empty() {
                        staged_config.vocabulary_setup = VocabularySetupStatus::Completed;
                    }
                    storage::save(&staged_config)
                        .map_err(|error| format!("保存词库失败：{error:#}"))?;
                    app.config = staged_config;
                    Ok((parsed_count, merge))
                })();
                match result {
                    Ok((parsed, merge)) => {
                        app.vocabulary_dirty = false;
                        app.status = format!(
                            "已从“{}”解析 {} 条：新增 {} 条、更新 {} 条、未变化 {} 条。",
                            path.file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("Excel 词库"),
                            parsed,
                            merge.added,
                            merge.updated,
                            merge.unchanged
                        );
                    }
                    Err(summary) => {
                        // 重新解析 conflict 列表用于渲染。如要优化可把 conflicts 放在错误里。
                        let existing_codes: Vec<String> = app
                            .config
                            .vocabulary
                            .iter()
                            .filter(|e| e.category == VocabularyCategory::Unit)
                            .map(|e| e.code.trim().to_string())
                            .filter(|c| !c.is_empty())
                            .collect();
                        let conflicts = vocabulary_xlsx::parse(&path, &existing_codes)
                            .err()
                            .map(|e| e.conflicts)
                            .unwrap_or_default();
                        app.vocabulary_import_conflicts = if conflicts.is_empty() {
                            None
                        } else {
                            Some(conflicts)
                        };
                        app.status = format!("词库导入失败：{summary}");
                    }
                }
            },
        );
    }

    pub(crate) fn export_vocabulary_xlsx(&mut self) {
        self.request_file(
            FileDialogSpec::save_file()
                .filter("Excel", &["xlsx"])
                .file_name("公文助手标准词库.xlsx"),
            |app, path| {
                let Some(path) = path else {
                    return;
                };
                let existing_codes: Vec<String> = app
                    .config
                    .vocabulary
                    .iter()
                    .filter(|e| e.category == VocabularyCategory::Unit)
                    .map(|e| e.code.trim().to_string())
                    .filter(|c| !c.is_empty())
                    .collect();
                let result =
                    vocabulary_xlsx::to_xlsx(&app.config.vocabulary, &path, &existing_codes);
                match result {
                    Ok(()) => app.status = format!("词库已导出到 {}。", path.display()),
                    Err(error) => app.status = format!("词库导出失败：{error}"),
                }
            },
        );
    }

    pub(crate) fn export_blank_vocabulary_template(&mut self) {
        self.request_file(
            FileDialogSpec::save_file()
                .filter("Excel", &["xlsx"])
                .file_name("公文助手标准词库模板.xlsx"),
            |app, path| {
                let Some(path) = path else {
                    return;
                };
                let codes: Vec<String> = app
                    .config
                    .vocabulary
                    .iter()
                    .filter(|e| e.category == VocabularyCategory::Unit)
                    .map(|e| e.code.trim().to_string())
                    .filter(|c| !c.is_empty())
                    .collect();
                let result = vocabulary_xlsx::to_xlsx(&[], &path, &codes);
                match result {
                    Ok(()) => app.status = format!("空白词库模板已导出到 {}。", path.display()),
                    Err(error) => app.status = format!("模板导出失败：{error}"),
                }
            },
        );
    }

    pub(crate) fn renormalize_vocabulary(&mut self) {
        units::identity::restore_owners(&mut self.config.vocabulary);
        units::rebuild_parents_from_codes(&mut self.config.vocabulary);
        units::normalize(&mut self.config.vocabulary);
    }

    pub(crate) fn vocabulary_ui(&mut self, ui: &mut egui::Ui) {
        if self.config.vocabulary_setup == VocabularySetupStatus::Pending
            && self.config.vocabulary.is_empty()
        {
            self.vocabulary_setup_ui(ui);
            return;
        }
        if let Err(error) = units::identity::validate(&self.config.vocabulary) {
            ui.colored_label(warn(), format!("词库需核对：{error}"));
        }
        let mut action = None;
        if self
            .vocabulary_sort_undo
            .as_ref()
            .is_some_and(|undo| !undo.valid(&self.config.vocabulary))
        {
            self.vocabulary_sort_undo = None;
        }
        let mut structure_changed = false;
        let unit_count = self
            .config
            .vocabulary
            .iter()
            .filter(|entry| entry.category == VocabularyCategory::Unit)
            .count();
        let person_count = self.config.vocabulary.len() - unit_count;

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading("标准词库");
                ui.weak(format!(
                    "{unit_count} 个单位 · {person_count} 名人员 · {}",
                    if self.vocabulary_dirty {
                        "有未保存更改"
                    } else {
                        "已保存到本机"
                    }
                ));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary_icon_button(ui, theme::Icon::Save, "保存更改").clicked() {
                    self.persist();
                    if self.status.starts_with("配置已保存") {
                        self.vocabulary_dirty = false;
                    }
                }
                if theme::icon_button(ui, theme::Icon::History, "版本历史")
                    .on_hover_text("查看全局配置版本（词库、版式、设置），可应用回滚或对照")
                    .clicked()
                {
                    self.config_versions_open = true;
                }
                if ui
                    .add(theme::icon_text_button(
                        theme::Icon::GitCommit,
                        "提交配置版本",
                    ))
                    .on_hover_text(
                        "把当前词库、版式与设置固化为一个配置版本（需相对上一版本有变更）",
                    )
                    .clicked()
                {
                    self.open_version_commit(VersionScope::Config);
                }
                ui.menu_image_text_button(
                    theme::Icon::ArrowUpDown.image().tint(theme::text_soft()),
                    "导入 / 导出",
                    |ui| {
                        if ui
                            .add(theme::icon_text_button(theme::Icon::FileUp, "导入 Excel"))
                            .on_hover_text("选择本机 .xlsx 词库，按编码优先合并")
                            .clicked()
                        {
                            ui.close();
                            self.import_vocabulary_xlsx();
                        }
                        if ui
                            .add(theme::icon_text_button(theme::Icon::FileDown, "导出 Excel"))
                            .on_hover_text("把当前词库导出为 Excel 模板，含下拉与冻结")
                            .clicked()
                        {
                            ui.close();
                            self.export_vocabulary_xlsx();
                        }
                        ui.separator();
                        if ui
                            .add(theme::icon_text_button(
                                theme::Icon::FileDown,
                                "下载空白模板",
                            ))
                            .on_hover_text("导出空白模板（仅表头 + 数据验证）")
                            .clicked()
                        {
                            ui.close();
                            self.export_blank_vocabulary_template();
                        }
                    },
                );
            });
        });
        ui.add_space(8.0);

        ui.horizontal_wrapped(|ui| {
            if ui
                .add(theme::icon_text_button(theme::Icon::Building, "顶级单位"))
                .on_hover_text("新增一个没有上级的单位；选中单位后可在右侧继续加下级和人员")
                .clicked()
            {
                action = Some(VocabAction::AddUnit {
                    parent: String::new(),
                    position: SiblingPosition::Last,
                });
            }
            ui.separator();
            ui.add(
                egui::TextEdit::singleline(&mut self.vocabulary_filter)
                    .hint_text("搜索单位、简称、呈批或发函代字、姓名或电话")
                    .desired_width(280.0),
            );
            if !self.vocabulary_filter.is_empty()
                && theme::icon_button(ui, theme::Icon::SearchClear, "清除搜索").clicked()
            {
                self.vocabulary_filter.clear();
            }
            ui.separator();
            if theme::icon_button(ui, theme::Icon::Expand, "展开全部").clicked() {
                self.vocabulary_collapsed.clear();
            }
            if theme::icon_button(ui, theme::Icon::Collapse, "折叠全部").clicked() {
                self.vocabulary_collapsed = self
                    .config
                    .vocabulary
                    .iter()
                    .filter(|entry| entry.category == VocabularyCategory::Unit)
                    .map(|entry| entry.id)
                    .collect();
            }
            ui.separator();
            if ui
                .add(theme::warning_icon_button(theme::Icon::Trash, "清空词库"))
                .clicked()
            {
                self.vocabulary_clear_confirm = true;
            }
            if self.vocabulary_sort_undo.is_some() && ui.small_button("撤销排序").clicked() {
                action = Some(VocabAction::UndoSort);
            }
        });

        if self.vocabulary_clear_confirm {
            ui.add_space(6.0);
            ui.group(|ui| {
                ui.colored_label(
                    warn(),
                    "将清空当前标准词库中的全部单位和人员。此操作尚未保存到磁盘。",
                );
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new("确认清空").color(warn()),
                        ))
                        .clicked()
                    {
                        action = Some(VocabAction::Clear);
                    }
                    if ui.button("取消").clicked() {
                        self.vocabulary_clear_confirm = false;
                    }
                });
            });
        }

        let conflicts_snapshot = self.vocabulary_import_conflicts.clone();
        if let Some(conflicts) = &conflicts_snapshot
            && !conflicts.is_empty()
        {
            ui.add_space(6.0);
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        warn(),
                        format!("导入失败（{} 项冲突）：修正后重新上传", conflicts.len()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::icon_button(ui, theme::Icon::X, "关闭提示").clicked() {
                            self.vocabulary_import_conflicts = None;
                        }
                    });
                });
                let mut by_sheet: BTreeMap<&'static str, Vec<&vocabulary_xlsx::Conflict>> =
                    BTreeMap::new();
                for c in conflicts {
                    by_sheet.entry(c.sheet).or_default().push(c);
                }
                for (sheet, list) in by_sheet {
                    ui.strong(sheet);
                    for c in list {
                        ui.label(format!(
                            "  • 第 {} 行 「{}」=「{}」：{}",
                            c.row, c.field, c.current_value, c.message
                        ));
                    }
                }
            });
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        let filter = self.vocabulary_filter.trim().to_lowercase();
        let rows = self.vocabulary_rows(&filter);
        // 窄窗口放不下左右两栏，改成树在上、编辑区在下。
        if ui.available_width() < 880.0 {
            egui::ScrollArea::vertical()
                .id_salt("vocabulary_single_column")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    self.vocabulary_tree_ui(ui, &rows, &filter, &mut action);
                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(6.0);
                    structure_changed |= self.vocabulary_editor_ui(ui, &mut action);
                });
        } else {
            let tree_width = (ui.available_width() * 0.52).clamp(340.0, 620.0);
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(tree_width);
                    egui::ScrollArea::vertical()
                        .id_salt("vocabulary_tree")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            self.vocabulary_tree_ui(ui, &rows, &filter, &mut action);
                        });
                });
                ui.separator();
                ui.vertical(|ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("vocabulary_editor")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            structure_changed |= self.vocabulary_editor_ui(ui, &mut action);
                        });
                });
            });
        }

        if let Some(action) = action {
            self.apply_vocab_action(action);
        } else if structure_changed {
            units::normalize(&mut self.config.vocabulary);
            self.vocabulary_dirty = true;
        }
        self.vocabulary_move_window(ui.ctx());
    }

    /// 首次运行建库引导：先帮助用户完成一个可工作的最小词库，再进入完整编辑器。
    pub(crate) fn vocabulary_setup_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.heading("建立您的标准词库");
            ui.weak("先创建一个顶级单位，后续再按组织层级补充下级单位和人员。");
            ui.add_space(18.0);

            ui.horizontal(|ui| {
                for (number, label, active) in [
                    ("1", "创建根单位", true),
                    ("2", "补充层级与人员", false),
                    ("3", "检查并保存", false),
                ] {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(118.0, 70.0), egui::Sense::hover());
                    let center = egui::pos2(rect.center().x, rect.top() + 22.0);
                    ui.painter().circle_filled(
                        center,
                        18.0,
                        if active {
                            theme::accent()
                        } else {
                            theme::surface_hover()
                        },
                    );
                    ui.painter().text(
                        center,
                        egui::Align2::CENTER_CENTER,
                        number,
                        egui::FontId::proportional(15.0),
                        if active {
                            theme::accent_text()
                        } else {
                            theme::text_soft()
                        },
                    );
                    ui.painter().text(
                        egui::pos2(rect.center().x, rect.bottom() - 10.0),
                        egui::Align2::CENTER_BOTTOM,
                        label,
                        egui::FontId::proportional(13.0),
                        if active {
                            theme::text()
                        } else {
                            theme::text_soft()
                        },
                    );
                }
            });
            ui.add_space(12.0);
        });

        let card_width = ui.available_width().min(620.0);
        ui.allocate_ui_with_layout(
            egui::vec2(card_width, 0.0),
            egui::Layout::top_down(egui::Align::Center),
            |ui| {
                ui.group(|ui| {
                    ui.set_min_width((card_width - 24.0).max(280.0));
                    ui.strong("第一步：您的顶级单位叫什么？");
                    ui.weak("请填写单位规范全称，层级关系单独维护；系统不再拼接上级名称。");
                    ui.add_space(8.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.vocabulary_setup_name)
                            .hint_text("例如：中共××市委宣传部")
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(10.0);
                    ui.horizontal_wrapped(|ui| {
                        let valid = !self.vocabulary_setup_name.trim().is_empty();
                        if ui
                            .add_enabled(valid, egui::Button::new("创建并进入词库编辑"))
                            .clicked()
                        {
                            let name = self.vocabulary_setup_name.trim().to_string();
                            self.apply_vocab_action(VocabAction::AddUnit {
                                parent: String::new(),
                                position: SiblingPosition::Last,
                            });
                            if let Some(id) = self.vocabulary_selected
                                && let Some(entry) = self
                                    .config
                                    .vocabulary
                                    .iter_mut()
                                    .find(|entry| entry.id == id)
                            {
                                entry.canonical = name;
                            }
                            self.config.vocabulary_setup = VocabularySetupStatus::Completed;
                            self.vocabulary_dirty = true;
                            self.persist();
                            if self.status.starts_with("配置已保存") {
                                self.vocabulary_dirty = false;
                            }
                        }
                        if ui.button("从 Excel 导入").clicked() {
                            self.import_vocabulary_xlsx();
                        }
                        if ui.button("下载空白模板").clicked() {
                            self.export_blank_vocabulary_template();
                        }
                        if ui.button("暂不建立").clicked() {
                            self.config.vocabulary_setup = VocabularySetupStatus::Skipped;
                            self.persist();
                        }
                    });
                    ui.add_space(6.0);
                    ui.weak(
                        "也可以下载空白 Excel 模板批量填写后再导入；引导不会锁住任何专业字段。",
                    );
                });
            },
        );
    }

    pub(crate) fn vocabulary_move_window(&mut self, ctx: &egui::Context) {
        let Some(mut draft) = self.vocabulary_move.take() else {
            return;
        };
        let Some(entry) = self
            .config
            .vocabulary
            .iter()
            .find(|entry| entry.id == draft.id)
            .cloned()
        else {
            return;
        };
        let is_unit = entry.category == VocabularyCategory::Unit;
        let blocked = if is_unit {
            self.config
                .vocabulary
                .iter()
                .position(|candidate| candidate.id == draft.id)
                .map(|index| {
                    units::subtree_indices(&self.config.vocabulary, index)
                        .into_iter()
                        .filter_map(|index| {
                            let candidate = &self.config.vocabulary[index];
                            (candidate.category == VocabularyCategory::Unit)
                                .then(|| candidate.code.trim().to_string())
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let display = UnitDisplay::new(&self.config.vocabulary);
        let destinations = self
            .config
            .vocabulary
            .iter()
            .filter(|candidate| candidate.category == VocabularyCategory::Unit)
            .filter(|candidate| !blocked.contains(&candidate.code.trim().to_string()))
            .map(|candidate| {
                (
                    candidate.code.trim().to_string(),
                    display.full_name(&candidate.code),
                )
            })
            .collect::<Vec<_>>();
        let siblings = self
            .config
            .vocabulary
            .iter()
            .filter(|candidate| candidate.id != draft.id && candidate.category == entry.category)
            .filter(|candidate| {
                if is_unit {
                    candidate.parent.trim() == draft.destination.trim()
                } else {
                    candidate.unit.trim() == draft.destination.trim()
                }
            })
            .map(|candidate| (candidate.id, candidate.canonical.trim().to_string()))
            .collect::<Vec<_>>();

        let destination_label = if draft.destination.is_empty() {
            if is_unit {
                "（顶层单位）"
            } else {
                "（未归属）"
            }
            .to_string()
        } else {
            destinations
                .iter()
                .find(|(code, _)| code == &draft.destination)
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| draft.destination.clone())
        };
        let position_label = match draft.position {
            SiblingPosition::First => "本级最前".to_string(),
            SiblingPosition::Last => "本级最后".to_string(),
            SiblingPosition::Before(anchor) => siblings
                .iter()
                .find(|(id, _)| *id == anchor)
                .map(|(_, name)| format!("“{name}”之前"))
                .unwrap_or_else(|| "本级最后".to_string()),
            SiblingPosition::After(anchor) => siblings
                .iter()
                .find(|(id, _)| *id == anchor)
                .map(|(_, name)| format!("“{name}”之后"))
                .unwrap_or_else(|| "本级最后".to_string()),
        };

        // 有下拉的表单：点遮罩常常只是想收起下拉，不能顺手把整个框关掉，只认 Esc。
        let dialog = modal::dialog(
            ctx,
            egui::Id::new("vocabulary_move_window"),
            "精确移动",
            430.0,
            Dismiss::EscOnly,
            |ui| {
                ui.strong(format!("移动“{}”", entry.canonical.trim()));
                ui.weak(if is_unit {
                    "选择新的上级单位和准确落点；下级单位及人员会随本单位一起移动。"
                } else {
                    "选择新的所属单位和准确落点。"
                });
                ui.add_space(10.0);
                egui::Grid::new("vocabulary_move_grid")
                    .num_columns(2)
                    .spacing([12.0, 10.0])
                    .show(ui, |ui| {
                        ui.label(if is_unit { "新上级" } else { "所属单位" });
                        egui::ComboBox::from_id_salt("vocabulary_move_destination")
                            .selected_text(destination_label)
                            .width(320.0)
                            .show_ui(ui, |ui| {
                                let empty_label = if is_unit {
                                    "（顶层单位）"
                                } else {
                                    "（未归属）"
                                };
                                if ui
                                    .selectable_label(draft.destination.is_empty(), empty_label)
                                    .clicked()
                                {
                                    draft.destination.clear();
                                    draft.position = SiblingPosition::Last;
                                }
                                for (code, name) in &destinations {
                                    if ui
                                        .selectable_label(
                                            draft.destination == *code,
                                            format!("{code} · {name}"),
                                        )
                                        .clicked()
                                    {
                                        draft.destination = code.clone();
                                        draft.position = SiblingPosition::Last;
                                    }
                                }
                            });
                        ui.end_row();

                        ui.label("放置位置");
                        egui::ComboBox::from_id_salt("vocabulary_move_position")
                            .selected_text(position_label)
                            .width(320.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut draft.position,
                                    SiblingPosition::First,
                                    "本级最前",
                                );
                                ui.selectable_value(
                                    &mut draft.position,
                                    SiblingPosition::Last,
                                    "本级最后",
                                );
                                for (anchor, name) in &siblings {
                                    ui.selectable_value(
                                        &mut draft.position,
                                        SiblingPosition::Before(*anchor),
                                        format!("“{name}”之前"),
                                    );
                                    ui.selectable_value(
                                        &mut draft.position,
                                        SiblingPosition::After(*anchor),
                                        format!("“{name}”之后"),
                                    );
                                }
                            });
                        ui.end_row();
                    });
                ui.add_space(12.0);
                let mut confirm = false;
                ui.horizontal(|ui| {
                    if ui.button("确认移动").clicked() {
                        confirm = true;
                    }
                    if ui.button("取消").clicked() {
                        ui.close();
                    }
                });
                confirm
            },
        );

        if dialog.inner {
            self.apply_vocab_action(VocabAction::Relocate {
                id: draft.id,
                destination: draft.destination,
                position: draft.position,
            });
        } else if !dialog.dismissed {
            self.vocabulary_move = Some(draft);
        }
    }

    /// 把词库摊平成树形界面要画的行：单位按层级缩进，人员挂在所属单位下面。
    /// 词库本身已由 `units::normalize` 排成深度优先顺序，这里只补层级和折叠。
    pub(crate) fn vocabulary_rows(&self, filter: &str) -> Vec<TreeRow> {
        let vocab = &self.config.vocabulary;
        let depths = vocabulary_depths(vocab);
        let mut rows: Vec<TreeRow> = Vec::with_capacity(vocab.len());
        for (index, entry) in vocab.iter().enumerate() {
            rows.push(TreeRow {
                index,
                id: entry.id,
                depth: depths[index],
                is_unit: entry.category == VocabularyCategory::Unit,
                has_children: false,
            });
        }
        // 深度优先顺序下，下一行更深就说明本行有子节点。
        for position in 0..rows.len() {
            rows[position].has_children = rows
                .get(position + 1)
                .is_some_and(|next| next.depth > rows[position].depth);
        }

        if !filter.is_empty() {
            let mut visible = rows
                .iter()
                .map(|row| vocabulary_matches(&vocab[row.index], filter))
                .collect::<Vec<_>>();
            // 命中的节点要连同它的各级上级一起显示，否则看不出它挂在哪儿。
            for position in (0..rows.len()).rev() {
                if !visible[position] {
                    continue;
                }
                let mut depth = rows[position].depth;
                let mut ancestor = position;
                while depth > 0 && ancestor > 0 {
                    ancestor -= 1;
                    if rows[ancestor].depth < depth {
                        visible[ancestor] = true;
                        depth = rows[ancestor].depth;
                    }
                }
            }
            // 搜索期间忽略折叠状态，免得命中项藏在折叠的分支里。
            return rows
                .into_iter()
                .zip(visible)
                .filter(|(_, visible)| *visible)
                .map(|(row, _)| row)
                .collect();
        }

        let mut result = Vec::with_capacity(rows.len());
        let mut skip_below: Option<usize> = None;
        for row in rows {
            if let Some(depth) = skip_below {
                if row.depth > depth {
                    continue;
                }
                skip_below = None;
            }
            let collapse_here =
                row.is_unit && row.has_children && self.vocabulary_collapsed.contains(&row.id);
            let depth = row.depth;
            result.push(row);
            if collapse_here {
                skip_below = Some(depth);
            }
        }
        result
    }

    pub(crate) fn vocabulary_tree_ui(
        &mut self,
        ui: &mut egui::Ui,
        rows: &[TreeRow],
        filter: &str,
        action: &mut Option<VocabAction>,
    ) {
        ui.horizontal(|ui| {
            ui.strong("单位层级");
            ui.weak(if filter.is_empty() {
                "悬停行首出现手柄，拖动调整同级顺序；右键行可新增或移动".to_string()
            } else {
                format!("匹配 {} 行 · 清除搜索后可拖动排序", rows.len())
            });
        });
        ui.add_space(4.0);

        if rows.is_empty() {
            ui.group(|ui| {
                ui.add_space(12.0);
                ui.vertical_centered(|ui| {
                    ui.strong(if filter.is_empty() {
                        "词库还是空的"
                    } else {
                        "没有找到匹配的单位或人员"
                    });
                    ui.weak(if filter.is_empty() {
                        "点击上方“顶级单位”开始建库，或从 Excel 批量导入。"
                    } else {
                        "试试缩短关键词，或清除搜索条件。"
                    });
                    if filter.is_empty()
                        && self.config.vocabulary_setup == VocabularySetupStatus::Skipped
                        && ui.button("重新打开建库引导").clicked()
                    {
                        self.config.vocabulary_setup = VocabularySetupStatus::Pending;
                    }
                });
                ui.add_space(12.0);
            });
            return;
        }

        let tree_top = ui.cursor().top();
        let dragged = if filter.is_empty() {
            egui::DragAndDrop::payload::<drag::Payload>(ui.ctx()).map(|payload| payload.0)
        } else {
            // 搜索条件改变后当前可见清单已不完整，停止尚未结束的拖动。
            egui::DragAndDrop::clear_payload(ui.ctx());
            None
        };
        let code_font = egui::TextStyle::Monospace.resolve(ui.style());
        let body_font = egui::TextStyle::Body.resolve(ui.style());
        let measure = |ui: &egui::Ui, text: &str, font: &egui::FontId| {
            ui.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE)
                    .size()
                    .x
            })
        };
        // 编号列按当前可见行里最长的编号定宽，所有行右对齐到同一条线。
        let code_column_width = rows
            .iter()
            .map(|row| {
                let code = self.config.vocabulary[row.index].code.trim();
                measure(ui, if code.is_empty() { "--" } else { code }, &code_font)
            })
            .fold(0.0_f32, f32::max)
            .min(ui.available_width() * 0.25);
        // 行与行紧挨着排，缩进参考线才能上下连成一条；排完恢复原来的行距。
        let row_spacing = ui.spacing().item_spacing.y;
        ui.spacing_mut().item_spacing.y = 0.0;
        for row in drag::visible_rows(rows, dragged) {
            let entry = &self.config.vocabulary[row.index];
            let id = row.id;
            let name = if entry.canonical.trim().is_empty() {
                "（未命名）".to_string()
            } else {
                entry.canonical.trim().to_string()
            };
            let code = entry.code.trim().to_string();
            let parent_code = entry.parent.trim().to_string();
            let owner_code = entry.unit.trim().to_string();
            let selected = self.vocabulary_selected == Some(id);
            let collapsed = self.vocabulary_collapsed.contains(&id) || dragged == Some(id);
            // 单位层级并列显示两类代字；人员显示职务、电话及承办权限。
            let detail = if row.is_unit {
                let mut parts = Vec::new();
                if !entry.department_code.trim().is_empty() {
                    parts.push(format!("函 {}", entry.department_code.trim()));
                }
                if !entry.approval_department_code.trim().is_empty() {
                    parts.push(format!("呈 {}", entry.approval_department_code.trim()));
                }
                if entry.seal_on_behalf {
                    parts.push("代章".to_string());
                }
                parts.join(" · ")
            } else {
                let mut parts = Vec::new();
                if !entry.position.trim().is_empty() {
                    parts.push(entry.position.trim().to_string());
                }
                if !entry.phone.trim().is_empty() {
                    parts.push(entry.phone.trim().to_string());
                }
                if entry.can_handle_parent_unit {
                    parts.push("可承办上级单位".to_string());
                }
                if parts.is_empty() {
                    "未维护职务和电话".to_string()
                } else {
                    parts.join(" · ")
                }
            };
            let orphan = !row.is_unit && entry.unit.trim().is_empty();

            // 先按固定行高占住整行，再在子 Ui 里按坐标摆各列。子 Ui 不推进外层光标，
            // 不会像在纵向布局里套 `right_to_left` 那样把剩余高度整个占掉，也不会重复推进行距。
            let (row_rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), TREE_ROW_HEIGHT),
                egui::Sense::hover(),
            );
            let row_response = ui.interact(
                row_rect,
                egui::Id::new(("vocab_row", id)),
                egui::Sense::click(),
            );
            let mut row_ui = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt(("vocab_row_ui", id))
                    .max_rect(row_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            // 新行淡入。
            let seen_t = ui.ctx().animate_bool_with_time(
                egui::Id::new(("vocab_row_seen", id)),
                true,
                theme::anim::SLOW,
            );
            row_ui.multiply_opacity(seen_t);
            // 悬停用几何判断而非 response.hovered()：行里的按钮会把交互抢走，
            // 指针落在它们上面时整行反而算「未悬停」，背景会一闪一闪。
            let hovered = ui.rect_contains_pointer(row_rect) || dragged == Some(id);
            let hover_t = ui.ctx().animate_bool_with_time(
                egui::Id::new(("vocab_row_hover", id)),
                hovered,
                theme::anim::FAST,
            );

            let painter = row_ui.painter().clone();
            // 选中整行铺强调浅色；悬停从页面底色渐变过去，避免一进一出闪一下。
            let background = if selected {
                Some(theme::accent_soft())
            } else if hover_t > 0.01 {
                let base = if theme::smartisan::active() {
                    theme::surface()
                } else {
                    theme::canvas()
                };
                Some(base.lerp_to_gamma(theme::surface_hover(), hover_t))
            } else {
                None
            };
            if let Some(fill) = background {
                painter.rect_filled(row_rect.shrink2(egui::vec2(0.0, 1.0)), 6, fill);
            }
            theme::smartisan::index_row(&painter, row_rect, selected);

            // 横向网格：缩进太深时压缩缩进，保证名称至少留出 TREE_MIN_TEXT。
            let fixed = TREE_GRIP
                + TREE_CHEVRON
                + 2.0
                + TREE_ICON
                + 6.0
                + TREE_MIN_TEXT
                + 10.0
                + code_column_width
                + 6.0
                + TREE_MENU;
            let max_indent = (row_rect.width() - fixed).max(0.0);
            let indent_step = if row.depth == 0 {
                TREE_INDENT
            } else {
                TREE_INDENT.min(max_indent / row.depth as f32)
            };
            let left = row_rect.left() + 2.0;
            let center_y = row_rect.center().y;
            let column = |x: f32, width: f32| {
                egui::Rect::from_min_max(
                    egui::pos2(x, row_rect.top()),
                    egui::pos2(x + width, row_rect.bottom()),
                )
            };
            let grip_rect = column(left, TREE_GRIP);
            let level_left = |level: usize| left + TREE_GRIP + level as f32 * indent_step;
            let chevron_rect = column(level_left(row.depth), TREE_CHEVRON);
            let icon_rect = egui::Rect::from_center_size(
                egui::pos2(chevron_rect.right() + 2.0 + TREE_ICON / 2.0, center_y),
                egui::Vec2::splat(TREE_ICON),
            );
            let menu_rect = column(row_rect.right() - TREE_MENU, TREE_MENU);
            let code_rect = column(
                menu_rect.left() - 6.0 - code_column_width,
                code_column_width,
            );
            let text_left = icon_rect.right() + 6.0;
            let text_right = (code_rect.left() - 10.0).max(text_left);

            // 缩进参考线：每一级上级在它的展开箭头中线处画一条细竖线，层级一眼可数。
            for level in 0..row.depth {
                let x = level_left(level) + TREE_CHEVRON / 2.0;
                painter.vline(
                    x,
                    row_rect.y_range(),
                    egui::Stroke::new(1.0, theme::border()),
                );
            }

            // 拖动手柄平时隐藏，悬停本行或拖动中才显出来；位置始终预留。
            if filter.is_empty() {
                let mut grip_ui =
                    row_ui.new_child(egui::UiBuilder::new().max_rect(grip_rect).layout(
                        egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                    ));
                grip_ui.multiply_opacity(hover_t);
                drag::handle(&mut grip_ui, id);
            }

            // 展开箭头：无边框，只在有下级的单位上画；搜索时折叠状态不起作用，一律画成展开。
            if row.is_unit && row.has_children {
                let toggle = row_ui.interact(
                    chevron_rect,
                    egui::Id::new(("vocab_toggle", id)),
                    if filter.is_empty() {
                        egui::Sense::click()
                    } else {
                        egui::Sense::hover()
                    },
                );
                let icon = if collapsed && filter.is_empty() {
                    theme::Icon::ChevronRight
                } else {
                    theme::Icon::ChevronDown
                };
                let tint = if toggle.hovered() {
                    theme::text()
                } else {
                    theme::text_muted()
                };
                icon.image_sized(14.0).tint(tint).paint_at(
                    &row_ui,
                    egui::Rect::from_center_size(chevron_rect.center(), egui::Vec2::splat(14.0)),
                );
                let toggle = if filter.is_empty() {
                    toggle.on_hover_text(if collapsed { "展开" } else { "折叠" })
                } else {
                    toggle
                };
                if toggle.clicked() || (row_response.double_clicked() && filter.is_empty()) {
                    if collapsed {
                        self.vocabulary_collapsed.remove(&id);
                    } else {
                        self.vocabulary_collapsed.insert(id);
                    }
                }
            }

            // 类别图标：单位是楼，人员是人；没挂单位的人员标警示色。
            let icon_tint = if orphan {
                warn()
            } else if selected {
                theme::accent()
            } else {
                theme::text_muted()
            };
            let icon = if row.is_unit {
                theme::Icon::Building
            } else {
                theme::Icon::User
            };
            icon.image_sized(TREE_ICON)
                .tint(icon_tint)
                .paint_at(&row_ui, icon_rect);

            // 名称优先：说明文字只用名称排完后剩下的宽度，但名称最多吃掉六成，
            // 免得长名称把职务、代字整个挤没。
            let text_width = text_right - text_left;
            let name_full = measure(&row_ui, &name, &body_font);
            let name_width = if detail.is_empty() {
                text_width
            } else {
                let detail_full = measure(&row_ui, &detail, &body_font);
                name_full
                    .min((text_width - 12.0 - detail_full).max(text_width * 0.6))
                    .min(text_width)
            };
            let mut name_text = egui::RichText::new(&name);
            name_text = if orphan {
                name_text.color(warn())
            } else if selected {
                name_text.color(theme::accent())
            } else if row.depth == 0 && row.is_unit {
                name_text.strong()
            } else {
                name_text.color(theme::text())
            };
            tree_label(
                &mut row_ui,
                column(text_left, name_width),
                name_text,
                egui::Align::Min,
            );
            if !detail.is_empty() {
                let detail_left = text_left + name_width.min(name_full) + 12.0;
                tree_label(
                    &mut row_ui,
                    column(detail_left, (text_right - detail_left).max(0.0)),
                    egui::RichText::new(&detail)
                        .size(theme::font_sizes::SMALL)
                        .color(theme::text_muted()),
                    egui::Align::Min,
                );
            }
            tree_label(
                &mut row_ui,
                code_rect,
                egui::RichText::new(if code.is_empty() { "--" } else { code.as_str() })
                    .monospace()
                    .color(theme::text_muted()),
                egui::Align::Max,
            );

            // 「＋」平时隐藏，悬停或选中本行才显出来；菜单打开期间按钮一直在，弹出层不会被关掉。
            let menu_opacity = if selected { 1.0 } else { hover_t };
            let mut menu_ui = row_ui.new_child(egui::UiBuilder::new().max_rect(menu_rect).layout(
                egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
            ));
            menu_ui.multiply_opacity(menu_opacity);
            let plus = menu_ui
                .add(
                    egui::Button::image(theme::Icon::Plus.image_sized(14.0))
                        .image_tint_follows_text_color(true)
                        .frame_when_inactive(false),
                )
                .on_hover_text("新增或移动（也可右键本行）");
            egui::Popup::menu(&plus).show(|ui| {
                vocabulary_row_menu(ui, row, &code, &parent_code, &owner_code, action);
            });

            let row_response = if orphan {
                row_response.on_hover_text("该人员没有所属单位，请在右侧指定")
            } else {
                row_response
            };
            row_response.context_menu(|ui| {
                vocabulary_row_menu(ui, row, &code, &parent_code, &owner_code, action);
            });
            if row_response.clicked() {
                self.vocabulary_selected = Some(id);
                self.vocabulary_delete_confirm = None;
            }

            // 拖到目标行上半部表示插到它前面，下半部表示插到它后面。
            // 只接受同级词条，跨层级移动必须走精确移动面板，避免误改组织关系。
            if filter.is_empty()
                && let Some(drop) = drag::drop_target(ui, row_rect, id, &self.config.vocabulary)
            {
                *action = Some(drop);
            }
        }
        ui.spacing_mut().item_spacing.y = row_spacing;
        if filter.is_empty() {
            let tree_rect = egui::Rect::from_min_max(
                egui::pos2(ui.min_rect().left(), tree_top),
                egui::pos2(ui.min_rect().right(), ui.cursor().top()),
            );
            drag::edge_scroll(ui, tree_rect);
        }
    }

    /// 右侧编辑区。返回 `true` 表示改动了层级（改名或换上级），需要重排词库。
    pub(crate) fn vocabulary_editor_ui(
        &mut self,
        ui: &mut egui::Ui,
        action: &mut Option<VocabAction>,
    ) -> bool {
        let Some(id) = self.vocabulary_selected else {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.strong("未选中词条");
                ui.weak("在左侧点选一个单位或人员即可编辑。");
            });
            return false;
        };
        let Some(index) = self
            .config
            .vocabulary
            .iter()
            .position(|entry| entry.id == id)
        else {
            self.vocabulary_selected = None;
            return false;
        };
        let before = self.config.vocabulary[index].clone();

        let mut structure_changed = false;
        let width = (ui.available_width() - 170.0).clamp(180.0, 520.0);
        let is_unit = self.config.vocabulary[index].category == VocabularyCategory::Unit;
        let display = UnitDisplay::new(&self.config.vocabulary);
        let heading = if is_unit {
            display.full_name(&self.config.vocabulary[index].code)
        } else {
            self.config.vocabulary[index].canonical.trim().to_string()
        };
        let subtitle = if is_unit {
            "以上是词库维护的本单位全称，不拼接上级名称。".to_string()
        } else {
            let entry = &self.config.vocabulary[index];
            let unit = if entry.unit.trim().is_empty() {
                "未归属单位".to_string()
            } else {
                display.full_name(&entry.unit)
            };
            if entry.position.trim().is_empty() {
                unit
            } else {
                format!("{} · {unit}", entry.position.trim())
            }
        };

        theme::card()
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.visuals_mut().striped = false;
                ui.horizontal(|ui| {
                    egui::Frame::new()
                        .fill(theme::accent_soft())
                        .corner_radius(egui::CornerRadius::same(22))
                        .inner_margin(egui::Margin::symmetric(12, 7))
                        .show(ui, |ui| {
                            let mark = if is_unit {
                                "单".to_string()
                            } else {
                                self.config.vocabulary[index]
                                    .canonical
                                    .chars()
                                    .next()
                                    .unwrap_or('人')
                                    .to_string()
                            };
                            ui.label(
                                egui::RichText::new(mark)
                                    .size(24.0)
                                    .strong()
                                    .color(theme::accent()),
                            );
                        });
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.strong(if is_unit { "单位" } else { "人员" })
                                .on_hover_text("改动会立即反映在起草页；点击右上角“保存更改”写入本机配置。");
                            if is_unit {
                                ui.label("层级编码");
                                let prev_code = self.config.vocabulary[index].code.clone();
                                let resp = ui.add(
                                    egui::TextEdit::singleline(&mut self.config.vocabulary[index].code)
                                        .hint_text("留空由系统按位置自动补；前缀即上级编码")
                                        .desired_width(160.0),
                                );
                                if resp.changed() {
                                    self.config.vocabulary[index].code =
                                        self.config.vocabulary[index].code.trim().to_string();
                                    structure_changed = true;
                                }
                                if resp.lost_focus() && self.config.vocabulary[index].code != prev_code {
                                    self.renormalize_vocabulary();
                                }
                                if ui
                                    .button("整理")
                                    .on_hover_text("按编码重新建上下级并排序")
                                    .clicked()
                                {
                                    self.renormalize_vocabulary();
                                    structure_changed = true;
                                }
                                // 编码重复红字提示。
                                let code = self.config.vocabulary[index].code.trim();
                                if !code.is_empty() {
                                    let dup = self.config.vocabulary.iter().enumerate().any(|(i, e)| {
                                        i != index
                                            && e.category == VocabularyCategory::Unit
                                            && e.code.trim() == code
                                    });
                                    if dup {
                                        ui.colored_label(warn(), "编码重复");
                                    }
                                }
                            } else {
                                ui.weak(format!("单位内编码 {}", self.config.vocabulary[index].code));
                            }
                        });
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&heading).size(22.0).strong(),
                            )
                            .wrap(),
                        );
                        ui.weak(&subtitle);
                    });
                });
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(10.0);
                if is_unit {
                    structure_changed |= self.vocabulary_unit_editor(ui, index, width);
                } else {
                    structure_changed |= self.vocabulary_person_editor(ui, index, width);
                }

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);

                ui.weak(format!("身份编码：{}", self.config.vocabulary[index].identity));
                let canonical = self.config.vocabulary[index].canonical.trim().to_string();
                let unit_code = self.config.vocabulary[index].code.trim().to_string();
                if is_unit {
                    let children = units::child_units(&self.config.vocabulary, &unit_code).len();
                    let people = units::unit_people(&self.config.vocabulary, &unit_code).len();
                    ui.weak(format!("下属 {children} 个单位 · {people} 名人员"));
                    ui.add_space(8.0);
                }

                let mut delete_button = None;
                ui.horizontal_wrapped(|ui| {
                    if is_unit {
                        if ui
                            .add(theme::icon_text_button(theme::Icon::FolderPlus, "下级单位"))
                            .on_hover_text("新单位的上级自动设为本单位")
                            .clicked()
                        {
                            *action = Some(VocabAction::AddUnit {
                                parent: unit_code.clone(),
                                position: SiblingPosition::Last,
                            });
                        }
                        if ui
                            .add(theme::icon_text_button(theme::Icon::UserPlus, "人员"))
                            .on_hover_text("新人员自动归属本单位")
                            .clicked()
                        {
                            *action = Some(VocabAction::AddPerson {
                                unit: unit_code.clone(),
                                position: SiblingPosition::Last,
                            });
                        }
                    }
                    if ui
                        .add(theme::icon_text_button(
                            theme::Icon::ArrowUpDown,
                            "精确移动",
                        ))
                        .on_hover_text(if is_unit {
                            "选择新的上级单位，并精确放到某个同级单位之前或之后"
                        } else {
                            "选择新的所属单位，并精确放到某个人员之前或之后"
                        })
                        .clicked()
                    {
                        self.vocabulary_move = Some(VocabularyMoveDraft {
                            id,
                            destination: if is_unit {
                                self.config.vocabulary[index].parent.clone()
                            } else {
                                self.config.vocabulary[index].unit.clone()
                            },
                            position: SiblingPosition::Last,
                        });
                    }
                    ui.add_space(12.0);
                    let response = ui.add(theme::warning_icon_button(
                        theme::Icon::Trash,
                        if is_unit { "删除单位" } else { "删除人员" },
                    ));
                    if response.clicked() {
                        // 与稿件管理一致：再点一次删除按钮收起确认气泡。
                        self.vocabulary_delete_confirm =
                            if self.vocabulary_delete_confirm == Some(id) {
                                None
                            } else {
                                Some(id)
                            };
                    }
                    delete_button = Some(response);
                });

                if self.vocabulary_delete_confirm == Some(id)
                    && let Some(anchor) = delete_button
                {
                    let (units_count, people_count) = if is_unit {
                        let indices = units::subtree_indices(&self.config.vocabulary, index);
                        let units_count = indices
                            .iter()
                            .filter(|i| {
                                self.config.vocabulary[**i].category == VocabularyCategory::Unit
                            })
                            .count();
                        (units_count, indices.len() - units_count)
                    } else {
                        (0, 1)
                    };
                    let mut open = true;
                    egui::Popup::from_response(&anchor)
                        .id(anchor.id.with("delete_confirm"))
                        .open_bool(&mut open)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .align(egui::RectAlign::BOTTOM_END)
                        .align_alternatives(&[egui::RectAlign::TOP_END])
                        .show(|ui| {
                            ui.colored_label(
                                warn(),
                                if is_unit {
                                    format!(
                                        "将删除本单位及其下级：共 {units_count} 个单位、{people_count} 名人员"
                                    )
                                } else {
                                    format!("将删除人员“{canonical}”")
                                },
                            );
                            ui.weak("删除后不可恢复。");
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                if ui.button("取消").clicked() {
                                    ui.close();
                                }
                                if ui
                                    .add(theme::warning_icon_button(
                                        theme::Icon::Trash,
                                        "确认删除",
                                    ))
                                    .clicked()
                                {
                                    *action = Some(VocabAction::Delete(id));
                                    ui.close();
                                }
                            });
                        });
                    if !open {
                        self.vocabulary_delete_confirm = None;
                    }
                }
            });
        if let Some(index) = self
            .config
            .vocabulary
            .iter()
            .position(|entry| entry.id == id)
        {
            let entry = &self.config.vocabulary[index];
            if entry.category == VocabularyCategory::Unit
                && self.config.vocabulary.iter().any(|other| {
                    other.id != id
                        && other.category == VocabularyCategory::Unit
                        && other.parent == entry.parent
                        && other.canonical.trim() == entry.canonical.trim()
                })
            {
                self.config.vocabulary[index].canonical = before.canonical.clone();
                self.status = "同一父节点下已有同名单位，名称未更改".into();
            }
        }
        if self
            .config
            .vocabulary
            .iter()
            .find(|entry| entry.id == id)
            .is_some_and(|entry| entry != &before)
        {
            self.vocabulary_dirty = true;
        }
        structure_changed
    }

    pub(crate) fn vocabulary_unit_editor(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        width: f32,
    ) -> bool {
        let mut structure_changed = false;
        // 上级不能选自己或自己的下级，否则会形成环。
        let blocked = units::subtree_indices(&self.config.vocabulary, index)
            .into_iter()
            .map(|i| self.config.vocabulary[i].code.trim().to_string())
            .collect::<Vec<_>>();
        let display = UnitDisplay::new(&self.config.vocabulary);
        let parent_options = self
            .config
            .vocabulary
            .iter()
            .filter(|entry| entry.category == VocabularyCategory::Unit)
            .map(|entry| {
                (
                    entry.code.trim().to_string(),
                    format!("{} · {}", entry.code.trim(), display.full_name(&entry.code)),
                )
            })
            .filter(|(code, _)| !code.is_empty() && !blocked.contains(code))
            .collect::<Vec<_>>();
        let current_parent_label = self.config.vocabulary[index]
            .parent
            .trim()
            .is_empty()
            .then(|| "（顶层单位）".to_string())
            .or_else(|| {
                parent_options
                    .iter()
                    .find(|(code, _)| code == self.config.vocabulary[index].parent.trim())
                    .map(|(_, label)| label.clone())
            })
            .unwrap_or_else(|| self.config.vocabulary[index].parent.clone());
        let previous_parent = self.config.vocabulary[index].parent.clone();
        let mut selected_parent = previous_parent.clone();
        let entry_id = self.config.vocabulary[index].id;

        ui.strong("组织关系");
        ui.add_space(8.0);
        egui::Grid::new(("unit_editor", index))
            .striped(false)
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("单位名称");
                let renamed = ui
                    .add(
                        egui::TextEdit::singleline(&mut self.config.vocabulary[index].canonical)
                            .hint_text("单位规范全称，如“中央网信办新闻舆论处”")
                            .desired_width(width),
                    )
                    .changed();
                ui.end_row();
                structure_changed |= renamed;

                ui.label("上级单位");
                egui::ComboBox::from_id_salt(("unit_parent", index))
                    .selected_text(&current_parent_label)
                    .width(width)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(selected_parent.trim().is_empty(), "（顶层单位）")
                            .clicked()
                        {
                            selected_parent.clear();
                        }
                        for (code, label) in &parent_options {
                            if ui
                                .selectable_label(selected_parent.as_str() == code, label)
                                .clicked()
                            {
                                selected_parent = code.clone();
                            }
                        }
                    });
                ui.end_row();
            });
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(10.0);
        ui.strong("公文用名");
        ui.add_space(8.0);
        egui::Grid::new(("unit_document_editor", index))
            .striped(false)
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("简称");
                ui.add(
                    egui::TextEdit::singleline(&mut self.config.vocabulary[index].abbr)
                        .hint_text("如“新舆处”；留空时回落单位名称")
                        .desired_width(width),
                );
                ui.end_row();

                ui.label("外部名称");
                ui.add(
                    egui::TextEdit::singleline(&mut self.config.vocabulary[index].external_name)
                        .hint_text("对外函件使用；留空时回退单位名称并在审核中提醒")
                        .desired_width(width),
                );
                ui.end_row();

                ui.label("发函代字");
                ui.add(
                    egui::TextEdit::singleline(&mut self.config.vocabulary[index].department_code)
                        .hint_text("如“某教函”；用于公函文号")
                        .desired_width(width),
                );
                ui.end_row();

                ui.label("呈批代字");
                ui.add(
                    egui::TextEdit::singleline(
                        &mut self.config.vocabulary[index].approval_department_code,
                    )
                    .hint_text("如“某教呈”；用于红头呈批件文号")
                    .desired_width(width),
                );
                ui.end_row();

                ui.label("是否代章");
                ui.checkbox(
                    &mut self.config.vocabulary[index].seal_on_behalf,
                    "该单位落款时自动标注“（代章）”",
                )
                .on_hover_text(
                    "仅公函选择该单位作为落款单位时自动标注；联合发文按主发文单位判断。电话通知等其他文种不盖章，不适用。",
                );
                ui.end_row();

            });
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(10.0);
        ui.strong("职能任务");
        ui.add_space(4.0);
        wrapped_hint(
            ui,
            "主要职责、分管事项，一行一项。AI 分配任务、推荐牵头或责任单位时按它对号。",
            width + 96.0,
        );
        ui.add_space(4.0);
        ui.add(
            egui::TextEdit::multiline(&mut self.config.vocabulary[index].duties)
                .hint_text("如：负责公共数据归集、共享与开放；统筹政务信息化项目建设")
                .desired_rows(3)
                .desired_width(width + 96.0),
        );
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(10.0);
        ui.strong("检索与备注");
        ui.add_space(8.0);
        egui::Grid::new(("unit_search_editor", index))
            .striped(false)
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("别名 / 常见错写");
                let mut aliases = self.config.vocabulary[index].aliases.join("、");
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut aliases)
                            .hint_text("多个名称用顿号分隔，用于成稿后查错")
                            .desired_width(width),
                    )
                    .changed()
                {
                    self.config.vocabulary[index].aliases = split_units(&aliases);
                }
                ui.end_row();

                ui.label("备注");
                ui.add(
                    egui::TextEdit::singleline(&mut self.config.vocabulary[index].note)
                        .hint_text("可填写适用范围或使用说明")
                        .desired_width(width),
                );
                ui.end_row();
            });

        if selected_parent != previous_parent {
            self.vocabulary_move = Some(VocabularyMoveDraft {
                id: entry_id,
                destination: selected_parent,
                position: SiblingPosition::Last,
            });
        }

        ui.add_space(4.0);
        let name = self.config.vocabulary[index].canonical.trim().to_string();
        if name.is_empty() {
            ui.colored_label(warn(), "单位名称为空：下级单位和人员无法挂靠。");
        }
        let abbr = UnitDisplay::new(&self.config.vocabulary)
            .abbr_spaced(&self.config.vocabulary[index].code);
        ui.weak(format!("版记承办单位用简称；电话通知落款显示为“{abbr}”。"));
        structure_changed
    }

    pub(crate) fn vocabulary_person_editor(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        width: f32,
    ) -> bool {
        let structure_changed = false;
        let display = UnitDisplay::new(&self.config.vocabulary);
        let unit_options = self
            .config
            .vocabulary
            .iter()
            .filter(|entry| entry.category == VocabularyCategory::Unit)
            .map(|entry| {
                (
                    entry.code.trim().to_string(),
                    format!("{} · {}", entry.code.trim(), display.full_name(&entry.code)),
                )
            })
            .filter(|(code, _)| !code.is_empty())
            .collect::<Vec<_>>();
        let current_unit_label = self.config.vocabulary[index]
            .unit
            .trim()
            .is_empty()
            .then(|| "（未归属）".to_string())
            .or_else(|| {
                unit_options
                    .iter()
                    .find(|(code, _)| code == self.config.vocabulary[index].unit.trim())
                    .map(|(_, label)| label.clone())
            })
            .unwrap_or_else(|| self.config.vocabulary[index].unit.clone());
        let previous_unit = self.config.vocabulary[index].unit.clone();
        let mut selected_unit = previous_unit.clone();
        let entry_id = self.config.vocabulary[index].id;

        ui.strong("任职与联系");
        ui.add_space(8.0);
        egui::Grid::new(("person_editor", index))
            .striped(false)
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("姓名");
                ui.add(
                    egui::TextEdit::singleline(&mut self.config.vocabulary[index].canonical)
                        .hint_text("只填姓名，如“王庭”")
                        .desired_width(width),
                );
                ui.end_row();

                ui.label("职务");
                ui.add(
                    egui::TextEdit::singleline(&mut self.config.vocabulary[index].position)
                        .hint_text("如“主任”“副主任”")
                        .desired_width(width),
                );
                ui.end_row();

                ui.label("联系电话");
                ui.add(
                    egui::TextEdit::singleline(&mut self.config.vocabulary[index].phone)
                        .hint_text("与姓名一一绑定，起草页自动带出")
                        .desired_width(width),
                );
                ui.end_row();

                ui.label("所属单位");
                egui::ComboBox::from_id_salt(("person_unit", index))
                    .selected_text(&current_unit_label)
                    .width(width)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(selected_unit.trim().is_empty(), "（未归属）")
                            .clicked()
                        {
                            selected_unit.clear();
                        }
                        for (code, label) in &unit_options {
                            if ui
                                .selectable_label(selected_unit.as_str() == code, label)
                                .clicked()
                            {
                                selected_unit = code.clone();
                            }
                        }
                    });
                ui.end_row();
            });
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(10.0);
        ui.strong("公文联系人");
        ui.add_space(8.0);
        egui::Grid::new(("person_contact_editor", index))
            .striped(false)
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("承办上级单位");
                ui.checkbox(
                    &mut self.config.vocabulary[index].can_handle_parent_unit,
                    "可在上级单位的公函版记中作为联系人",
                );
                ui.end_row();
            });
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(10.0);
        ui.strong("个人简介");
        ui.add_space(4.0);
        wrapped_hint(
            ui,
            "分管领域、专长、经历。AI 推荐联系人、负责人时参考。",
            width + 96.0,
        );
        ui.add_space(4.0);
        ui.add(
            egui::TextEdit::multiline(&mut self.config.vocabulary[index].profile)
                .hint_text("如：分管数据资源处，长期从事数据共享与安全管理")
                .desired_rows(3)
                .desired_width(width + 96.0),
        );
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(10.0);
        ui.strong("检索与备注");
        ui.add_space(8.0);
        egui::Grid::new(("person_search_editor", index))
            .striped(false)
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("别名 / 常见错写");
                let mut aliases = self.config.vocabulary[index].aliases.join("、");
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut aliases)
                            .hint_text("多个写法用顿号分隔")
                            .desired_width(width),
                    )
                    .changed()
                {
                    self.config.vocabulary[index].aliases = split_units(&aliases);
                }
                ui.end_row();

                ui.label("备注");
                ui.add(
                    egui::TextEdit::singleline(&mut self.config.vocabulary[index].note)
                        .hint_text("可填写适用场合或使用说明")
                        .desired_width(width),
                );
                ui.end_row();
            });

        if selected_unit != previous_unit {
            self.vocabulary_move = Some(VocabularyMoveDraft {
                id: entry_id,
                destination: selected_unit,
                position: SiblingPosition::Last,
            });
        }

        ui.add_space(4.0);
        wrapped_hint(
            ui,
            "公函联系人按承办单位过滤；勾选“承办上级单位”后，也可在所属单位的上级单位版记中作为联系人。白头件呈报领导取落款单位及其各级上级单位的领导。",
            width + 96.0,
        );
        structure_changed
    }

    pub(crate) fn apply_vocab_action(&mut self, action: VocabAction) {
        if !matches!(action, VocabAction::Place { .. } | VocabAction::UndoSort) {
            self.vocabulary_sort_undo = None;
        }
        match action {
            VocabAction::AddUnit { parent, position } => {
                // 挂在折叠的上级下面时先展开，否则新节点看不见。
                if let Some(entry) = self
                    .config
                    .vocabulary
                    .iter()
                    .find(|entry| {
                        entry.category == VocabularyCategory::Unit
                            && entry.code.trim() == parent.trim()
                    })
                    .map(|entry| entry.id)
                {
                    self.vocabulary_collapsed.remove(&entry);
                }
                let id = units::next_id(&self.config.vocabulary);
                let code = units::next_unit_code(&self.config.vocabulary, &parent);
                self.config.vocabulary.push(VocabularyEntry {
                    id,
                    category: VocabularyCategory::Unit,
                    code,
                    canonical: unique_name(
                        &self.config.vocabulary,
                        VocabularyCategory::Unit,
                        "新单位",
                        &parent,
                    ),
                    parent,
                    ..Default::default()
                });
                if let Err(error) = units::place_sibling(&mut self.config.vocabulary, id, position)
                {
                    self.status = format!("新增单位后排序失败：{error}");
                }
                self.vocabulary_selected = Some(id);
                self.vocabulary_delete_confirm = None;
            }
            VocabAction::AddPerson { unit, position } => {
                if let Some(entry) = self
                    .config
                    .vocabulary
                    .iter()
                    .find(|entry| {
                        entry.category == VocabularyCategory::Unit
                            && entry.code.trim() == unit.trim()
                    })
                    .map(|entry| entry.id)
                {
                    self.vocabulary_collapsed.remove(&entry);
                }
                let id = units::next_id(&self.config.vocabulary);
                self.config.vocabulary.push(VocabularyEntry {
                    id,
                    category: VocabularyCategory::Person,
                    canonical: unique_name(
                        &self.config.vocabulary,
                        VocabularyCategory::Person,
                        "新人员",
                        &unit,
                    ),
                    unit,
                    ..Default::default()
                });
                if let Err(error) = units::place_sibling(&mut self.config.vocabulary, id, position)
                {
                    self.status = format!("新增人员后排序失败：{error}");
                }
                self.vocabulary_selected = Some(id);
                self.vocabulary_delete_confirm = None;
            }
            VocabAction::Delete(id) => {
                let Some(index) = self
                    .config
                    .vocabulary
                    .iter()
                    .position(|entry| entry.id == id)
                else {
                    return;
                };
                let doomed = if self.config.vocabulary[index].category == VocabularyCategory::Unit {
                    units::subtree_indices(&self.config.vocabulary, index)
                } else {
                    vec![index]
                };
                for position in doomed.into_iter().rev() {
                    self.config.vocabulary.remove(position);
                }
                self.vocabulary_selected = None;
                self.vocabulary_delete_confirm = None;
            }
            VocabAction::Place { id, position } => {
                match SortUndo::place(&mut self.config.vocabulary, id, position) {
                    Ok(Some(undo)) => {
                        self.vocabulary_sort_undo = Some(undo);
                        self.status = "已调整同级顺序；点击“保存更改”写入本机配置。".into();
                    }
                    Ok(None) => return,
                    Err(error) => {
                        self.status = format!("调整顺序失败：{error}");
                        return;
                    }
                }
            }
            VocabAction::UndoSort => {
                let Some(undo) = self.vocabulary_sort_undo.take() else {
                    return;
                };
                if let Err(error) = undo.restore(&mut self.config.vocabulary) {
                    self.status = error;
                    return;
                }
                self.status = "已撤销排序；点击“保存更改”写入本机配置。".into();
            }
            VocabAction::Relocate {
                id,
                destination,
                position,
            } => {
                match units::relocate_entry(&mut self.config.vocabulary, id, &destination, position)
                {
                    Ok(()) => {
                        self.vocabulary_selected = Some(id);
                        self.status = "已调整所属层级与位置；点击“保存更改”写入本机配置。".into();
                    }
                    Err(error) => self.status = format!("移动失败：{error}"),
                }
            }
            VocabAction::Clear => {
                self.config.vocabulary.clear();
                self.vocabulary_selected = None;
                self.vocabulary_collapsed.clear();
                self.vocabulary_delete_confirm = None;
                self.vocabulary_clear_confirm = false;
                self.vocabulary_import_conflicts = None;
                self.status = "当前标准词库已清空；点击“保存更改”写入本机配置。".into();
            }
        }
        units::normalize(&mut self.config.vocabulary);
        self.vocabulary_dirty = true;
    }
}
