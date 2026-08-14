//! 标准词库页：词库树、单位/人员编辑与 xlsx 导入导出。
//!
//! 由 src/app.rs 拆分而来：本文件是模块 `app::vocabulary`，与其它子模块共享
//! `app` 根模块的私有可见性（`GongwenApp` 结构体与根模块常量仍在 app.rs 中）。

use crate::storage;
use crate::theme;
use crate::units;
use crate::vocabulary_xlsx;
use crate::models::{VocabularyCategory, VocabularyEntry, split_units};
use crate::units::{UnitDisplay};
use std::collections::{BTreeMap};
use eframe::egui;
use crate::app::{warn, GongwenApp, VersionScope, unique_name, vocabulary_depths, vocabulary_matches, wrapped_hint};

/// 同理，词库树上的增删和排序也要等本帧渲染完再改动 `Vec`。
pub(crate) enum VocabAction {
    /// 新增单位。`parent` 为空表示顶层单位。
    AddUnit {
        parent: String,
    },
    /// 在指定单位下新增人员。
    AddPerson {
        unit: String,
    },
    /// 删除词条；删除单位时连同其下级单位与人员一并删除。
    Delete(u64),
    /// 在同级之间上移/下移，随后重排层级编码。
    MoveUp(u64),
    MoveDown(u64),
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

impl GongwenApp {
    pub(crate) fn import_vocabulary_xlsx(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Excel 词库", &["xlsx"])
            .pick_file()
        else {
            return;
        };
        self.vocabulary_import_conflicts = None;
        let result = (|| -> Result<(usize, vocabulary_xlsx::MergeReport), String> {
            let existing_codes: Vec<String> = self
                .config
                .vocabulary
                .iter()
                .filter(|e| e.category == VocabularyCategory::Unit)
                .map(|e| e.code.trim().to_string())
                .filter(|c| !c.is_empty())
                .collect();
            let report =
                vocabulary_xlsx::parse(&path, &existing_codes).map_err(|error| error.summary)?;
            let parsed_count = report.entries.len();
            let merge = vocabulary_xlsx::merge(&mut self.config.vocabulary, report.entries);
            units::normalize(&mut self.config.vocabulary);
            storage::save(&self.config).map_err(|error| format!("保存词库失败：{error:#}"))?;
            Ok((parsed_count, merge))
        })();
        match result {
            Ok((parsed, merge)) => {
                self.status = format!(
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
                let existing_codes: Vec<String> = self
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
                self.vocabulary_import_conflicts = if conflicts.is_empty() {
                    None
                } else {
                    Some(conflicts)
                };
                self.status = format!("词库导入失败：{summary}");
            }
        }
    }

    pub(crate) fn export_vocabulary_xlsx(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Excel", &["xlsx"])
            .set_file_name("公文助手标准词库.xlsx")
            .save_file()
        else {
            return;
        };
        let existing_codes: Vec<String> = self
            .config
            .vocabulary
            .iter()
            .filter(|e| e.category == VocabularyCategory::Unit)
            .map(|e| e.code.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect();
        let result = vocabulary_xlsx::to_xlsx(&self.config.vocabulary, &path, &existing_codes);
        match result {
            Ok(()) => self.status = format!("词库已导出到 {}。", path.display()),
            Err(error) => self.status = format!("词库导出失败：{error}"),
        }
    }

    pub(crate) fn export_blank_vocabulary_template(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Excel", &["xlsx"])
            .set_file_name("公文助手标准词库模板.xlsx")
            .save_file()
        else {
            return;
        };
        let codes: Vec<String> = self
            .config
            .vocabulary
            .iter()
            .filter(|e| e.category == VocabularyCategory::Unit)
            .map(|e| e.code.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect();
        let result = vocabulary_xlsx::to_xlsx(&[], &path, &codes);
        match result {
            Ok(()) => self.status = format!("空白词库模板已导出到 {}。", path.display()),
            Err(error) => self.status = format!("模板导出失败：{error}"),
        }
    }

    pub(crate) fn renormalize_vocabulary(&mut self) {
        units::rebuild_parents_from_codes(&mut self.config.vocabulary);
        units::normalize(&mut self.config.vocabulary);
    }

    pub(crate) fn vocabulary_ui(&mut self, ui: &mut egui::Ui) {
        let mut action = None;
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
                    "{unit_count} 个单位 · {person_count} 名人员 · 数据仅保存在本机"
                ));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary_icon_button(ui, theme::Icon::Save, "保存更改").clicked() {
                    self.persist();
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
                });
            }
            ui.separator();
            ui.add(
                egui::TextEdit::singleline(&mut self.vocabulary_filter)
                    .hint_text("搜索单位、简称、机关代字、姓名或电话")
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
                "点击节点在右侧编辑；行尾按钮调整同级顺序".to_string()
            } else {
                format!("匹配 {} 行", rows.len())
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
                        "点击上方“顶级单位”开始建库，或从 Markdown 批量导入。"
                    } else {
                        "试试缩短关键词，或清除搜索条件。"
                    });
                });
                ui.add_space(12.0);
            });
            return;
        }

        for row in rows {
            let entry = &self.config.vocabulary[row.index];
            let id = row.id;
            let name = if entry.canonical.trim().is_empty() {
                "（未命名）".to_string()
            } else {
                entry.canonical.trim().to_string()
            };
            let code = entry.code.trim().to_string();
            let selected = self.vocabulary_selected == Some(id);
            let collapsed = self.vocabulary_collapsed.contains(&id);
            // 单位层级只显示机关代字；人员显示职务、电话及承办上级单位权限。
            let detail = if row.is_unit {
                let mut parts = Vec::new();
                if !entry.department_code.trim().is_empty() {
                    parts.push(format!("代字 {}", entry.department_code.trim()));
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

            // 新行淡入 + 行悬停背景过渡。背景要垫在内容底下，所以先在绘制列表里
            // 占一个槽位，等这一行排完拿到真实矩形再回填（`Frame` 也是这么做的）。
            // 不能先 `allocate_exact_size` 一块 30px 高的矩形当悬停区再往里画：
            // `Ui::scope_dyn` 收尾会 `advance_cursor_after_rect`，同一段高度被推进
            // 两次，行距直接翻倍。
            let seen_t = ui.ctx().animate_bool_with_time(
                egui::Id::new(("vocab_row_seen", id)),
                true,
                theme::anim::SLOW,
            );
            let row_bg = ui.painter().add(egui::Shape::Noop);
            let row_rect = ui
                .scope(|ui| {
                    ui.set_opacity(seen_t);
                    ui.horizontal(|ui| {
                        ui.add_space(row.depth as f32 * 16.0);
                        if row.is_unit && row.has_children {
                            if theme::icon_button(
                                ui,
                                if collapsed {
                                    theme::Icon::ChevronRight
                                } else {
                                    theme::Icon::ChevronDown
                                },
                                if collapsed { "展开" } else { "折叠" },
                            )
                            .clicked()
                            {
                                if collapsed {
                                    self.vocabulary_collapsed.remove(&id);
                                } else {
                                    self.vocabulary_collapsed.insert(id);
                                }
                            }
                        } else {
                            ui.add_space(28.0);
                        }

                        ui.monospace(if code.is_empty() { "--" } else { code.as_str() });
                        let label = if orphan {
                            egui::RichText::new(&name).color(warn())
                        } else {
                            egui::RichText::new(&name)
                        };
                        let response = ui.selectable_label(selected, label);
                        let response = if orphan {
                            response.on_hover_text("该人员没有所属单位，请在右侧指定")
                        } else {
                            response
                        };
                        if response.clicked() {
                            self.vocabulary_selected = Some(id);
                            self.vocabulary_delete_confirm = None;
                        }
                        if !detail.is_empty() {
                            ui.weak(detail);
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::icon_button(ui, theme::Icon::ArrowDown, "下移")
                                .on_hover_text("与后一个同级交换")
                                .clicked()
                            {
                                *action = Some(VocabAction::MoveDown(id));
                            }
                            if theme::icon_button(ui, theme::Icon::ArrowUp, "上移")
                                .on_hover_text("与前一个同级交换")
                                .clicked()
                            {
                                *action = Some(VocabAction::MoveUp(id));
                            }
                        });
                    });
                })
                .response
                .rect;
            // 悬停用几何判断而非 response.hovered()：行里的按钮和标签会把交互抢走，
            // 指针落在它们上面时整行反而算「未悬停」，背景会一闪一闪。
            let hover_t = ui.ctx().animate_bool_with_time(
                egui::Id::new(("vocab_row_hover", id)),
                ui.rect_contains_pointer(row_rect),
                theme::anim::FAST,
            );
            if hover_t > 0.01 {
                let bg = theme::canvas().lerp_to_gamma(theme::surface_hover(), hover_t);
                ui.painter().set(
                    row_bg,
                    egui::epaint::RectShape::filled(
                        row_rect.expand2(egui::vec2(4.0, 2.0)),
                        egui::CornerRadius::same(6),
                        bg.gamma_multiply(seen_t),
                    ),
                );
            }
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

        let mut structure_changed = false;
        let width = (ui.available_width() - 96.0).clamp(180.0, 420.0);
        let is_unit = self.config.vocabulary[index].category == VocabularyCategory::Unit;
        let display = UnitDisplay::new(&self.config.vocabulary);
        let heading = if is_unit {
            display.full_name(&self.config.vocabulary[index].code)
        } else {
            let unit = self.config.vocabulary[index].unit.trim().to_string();
            if unit.is_empty() {
                self.config.vocabulary[index].canonical.trim().to_string()
            } else {
                format!(
                    "{} · {}",
                    self.config.vocabulary[index].canonical.trim(),
                    display.full_name(&unit)
                )
            }
        };

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
        ui.add(egui::Label::new(egui::RichText::new(&heading).heading()).wrap());
        if is_unit {
            ui.weak("以上是本单位在公文中展开后的全称。");
        }
        ui.add_space(8.0);

        if is_unit {
            structure_changed |= self.vocabulary_unit_editor(ui, index, width);
        } else {
            structure_changed |= self.vocabulary_person_editor(ui, index, width);
        }

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);

        let canonical = self.config.vocabulary[index].canonical.trim().to_string();
        let unit_code = self.config.vocabulary[index].code.trim().to_string();
        if is_unit {
            let children = units::child_units(&self.config.vocabulary, &unit_code).len();
            let people = units::unit_people(&self.config.vocabulary, &unit_code).len();
            ui.weak(format!("下属 {children} 个单位 · {people} 名人员"));
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add(theme::icon_text_button(theme::Icon::FolderPlus, "下级单位"))
                    .on_hover_text("新单位的上级自动设为本单位")
                    .clicked()
                {
                    *action = Some(VocabAction::AddUnit {
                        parent: unit_code.clone(),
                    });
                }
                if ui
                    .add(theme::icon_text_button(theme::Icon::UserPlus, "人员"))
                    .on_hover_text("新人员自动归属本单位")
                    .clicked()
                {
                    *action = Some(VocabAction::AddPerson {
                        unit: unit_code.clone(),
                    });
                }
            });
            ui.add_space(6.0);
        }

        let doomed = self.vocabulary_delete_confirm == Some(id);
        if doomed {
            let (units_count, people_count) = if is_unit {
                let indices = units::subtree_indices(&self.config.vocabulary, index);
                let units_count = indices
                    .iter()
                    .filter(|i| self.config.vocabulary[**i].category == VocabularyCategory::Unit)
                    .count();
                (units_count, indices.len() - units_count)
            } else {
                (0, 1)
            };
            ui.group(|ui| {
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
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new("确认删除").color(warn()),
                        ))
                        .clicked()
                    {
                        *action = Some(VocabAction::Delete(id));
                    }
                    if ui.button("取消").clicked() {
                        self.vocabulary_delete_confirm = None;
                    }
                });
            });
        } else if ui
            .add(theme::warning_icon_button(
                theme::Icon::Trash,
                if is_unit {
                    "删除单位"
                } else {
                    "删除人员"
                },
            ))
            .clicked()
        {
            self.vocabulary_delete_confirm = Some(id);
        }
        ui.add_space(4.0);
        structure_changed
    }

    pub(crate) fn vocabulary_unit_editor(&mut self, ui: &mut egui::Ui, index: usize, width: f32) -> bool {
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

        egui::Grid::new(("unit_editor", index))
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("单位名称");
                let renamed = ui
                    .add(
                        egui::TextEdit::singleline(&mut self.config.vocabulary[index].canonical)
                            .hint_text("本级名称，不含上级；如“新闻舆论处”")
                            .desired_width(width),
                    )
                    .changed();
                ui.end_row();
                structure_changed |= renamed;

                ui.label("上级单位");
                let parent = &mut self.config.vocabulary[index].parent;
                let previous_parent = parent.clone();
                egui::ComboBox::from_id_salt(("unit_parent", index))
                    .selected_text(&current_parent_label)
                    .width(width)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(parent.trim().is_empty(), "（顶层单位）")
                            .clicked()
                        {
                            parent.clear();
                        }
                        for (code, label) in &parent_options {
                            if ui
                                .selectable_label(parent.as_str() == code, label)
                                .clicked()
                            {
                                *parent = code.clone();
                            }
                        }
                    });
                structure_changed |= *parent != previous_parent;
                ui.end_row();

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

                ui.label("机关代字");
                ui.add(
                    egui::TextEdit::singleline(&mut self.config.vocabulary[index].department_code)
                        .hint_text("如“某教函”；选中本单位发文时自动带出")
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

    pub(crate) fn vocabulary_person_editor(&mut self, ui: &mut egui::Ui, index: usize, width: f32) -> bool {
        let mut structure_changed = false;
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

        egui::Grid::new(("person_editor", index))
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
                let unit = &mut self.config.vocabulary[index].unit;
                let previous_unit = unit.clone();
                egui::ComboBox::from_id_salt(("person_unit", index))
                    .selected_text(&current_unit_label)
                    .width(width)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(unit.trim().is_empty(), "（未归属）")
                            .clicked()
                        {
                            unit.clear();
                        }
                        for (code, label) in &unit_options {
                            if ui.selectable_label(unit.as_str() == code, label).clicked() {
                                *unit = code.clone();
                            }
                        }
                    });
                structure_changed |= *unit != previous_unit;
                ui.end_row();

                ui.label("承办上级单位");
                ui.checkbox(
                    &mut self.config.vocabulary[index].can_handle_parent_unit,
                    "可在上级单位的公函版记中作为联系人",
                );
                ui.end_row();

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

        ui.add_space(4.0);
        wrapped_hint(
            ui,
            "公函联系人按承办单位过滤；勾选“承办上级单位”后，也可在所属单位的上级单位版记中作为联系人。白头件呈报领导取落款单位及其各级上级单位的领导。",
            width + 96.0,
        );
        structure_changed
    }

    pub(crate) fn apply_vocab_action(&mut self, action: VocabAction) {
        match action {
            VocabAction::AddUnit { parent } => {
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
                self.config.vocabulary.push(VocabularyEntry {
                    id,
                    category: VocabularyCategory::Unit,
                    canonical: unique_name(
                        &self.config.vocabulary,
                        VocabularyCategory::Unit,
                        "新单位",
                    ),
                    parent,
                    ..Default::default()
                });
                self.vocabulary_selected = Some(id);
                self.vocabulary_delete_confirm = None;
            }
            VocabAction::AddPerson { unit } => {
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
                    ),
                    unit,
                    ..Default::default()
                });
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
            VocabAction::MoveUp(id) | VocabAction::MoveDown(id) => {
                let up = matches!(action, VocabAction::MoveUp(_));
                let Some(index) = self
                    .config
                    .vocabulary
                    .iter()
                    .position(|entry| entry.id == id)
                else {
                    return;
                };
                let entry = &self.config.vocabulary[index];
                // 同级 = 同一个上级下的单位，或同一个单位下的人员。
                let siblings = if entry.category == VocabularyCategory::Unit {
                    units::child_units(&self.config.vocabulary, entry.parent.trim())
                } else {
                    units::unit_people(&self.config.vocabulary, entry.unit.trim())
                };
                let Some(position) = siblings.iter().position(|value| *value == index) else {
                    return;
                };
                let target = if up {
                    position.checked_sub(1)
                } else {
                    (position + 1 < siblings.len()).then_some(position + 1)
                };
                if let Some(target) = target {
                    self.config.vocabulary.swap(index, siblings[target]);
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
    }
}
