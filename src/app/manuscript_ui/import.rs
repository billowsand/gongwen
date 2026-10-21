//! 同步 ZIP 导入预览、分支合并核对与压缩包密码表单。
//!
//! 由 src/app/manuscript_ui.rs 拆分而来：本文件是模块 `app::manuscript_ui::import`，与其它子模块共享
//! `app::manuscript_ui` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::app::manuscript_ui::{
    ImportPreview, MergeReview, PendingMergeDialog, ZipPasswordDialog, ZipPasswordPurpose,
    dialog_notice,
};
use crate::app::{truncate, warn};
use crate::manuscript_io;
use crate::models::VocabularyCategory;
use crate::theme;
use eframe::egui;

/// 导入预览框：左栏逐篇勾选，右栏看当前这篇的对照并选处理方式。
/// 返回 `Some(true)` 确认、`Some(false)` 取消、`None` 继续开着。
pub(crate) fn import_preview_form(
    ui: &mut egui::Ui,
    preview: &mut ImportPreview,
    config: &crate::models::AppConfig,
    body_height: f32,
) -> Option<bool> {
    let total = preview.manifest.records.len();
    let selected = preview
        .selected
        .iter()
        .filter(|selected| **selected)
        .count();
    ui.label(format!(
        "共 {total} 篇，已勾选 {selected} 篇。逐篇选择处理方式后才会写入。"
    ));
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut preview.keyword)
                .hint_text("按标题/文号过滤")
                .desired_width(220.0),
        );
        if ui.button("全选").clicked() {
            preview.selected.fill(true);
        }
        if ui.button("全不选").clicked() {
            preview.selected.fill(false);
        }
    });
    if let Some(vocabulary) = &preview.vocabulary {
        let units = vocabulary
            .entries
            .iter()
            .filter(|entry| entry.category == VocabularyCategory::Unit)
            .count();
        let people = vocabulary.entries.len() - units;
        ui.checkbox(
            &mut preview.merge_vocabulary,
            format!("合并包内标准词库到本机（{units} 个单位、{people} 名人员）"),
        );
    }
    ui.add_space(4.0);
    let keyword = preview.keyword.trim().to_lowercase();
    let list_width = (ui.available_width() * 0.4).clamp(260.0, 380.0);
    // 两栏放进定高的盒子：模态的 Area 不限高，竖分隔线会一路撑到无穷高。
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), body_height),
        egui::Layout::left_to_right(egui::Align::Min),
        |ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(list_width, body_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("import_preview_list")
                        .max_height(body_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for (index, record) in preview.manifest.records.iter().enumerate() {
                                if !keyword.is_empty()
                                    && !record.title.to_lowercase().contains(&keyword)
                                    && !record.doc_number.to_lowercase().contains(&keyword)
                                {
                                    continue;
                                }
                                ui.horizontal(|ui| {
                                    ui.checkbox(&mut preview.selected[index], "");
                                    ui.vertical(|ui| {
                                        let focused = preview.focused == Some(index);
                                        if ui
                                            .selectable_label(focused, truncate(&record.title, 22))
                                            .on_hover_text("在右侧查看对照并选择处理方式")
                                            .clicked()
                                        {
                                            preview.focused = Some(index);
                                        }
                                        ui.weak(format!(
                                            "{} · {} · {} · {}",
                                            record.doc_date,
                                            record.kind.label(),
                                            record.status.label(),
                                            preview.relations[index].relationship.label()
                                        ));
                                    });
                                });
                                ui.add_space(4.0);
                            }
                        });
                },
            );
            ui.separator();
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), body_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("import_preview_detail")
                        .max_height(body_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| match preview.focused {
                            Some(index) => import_preview_details(ui, preview, index, config),
                            None => {
                                ui.weak("点左侧任意一篇，在这里查看对照并选择处理方式。");
                            }
                        });
                },
            );
        },
    );
    ui.add_space(6.0);
    dialog_notice(ui, &preview.notice);
    let mut decision = None;
    ui.horizontal(|ui| {
        if ui.button("确认所选处理").clicked() {
            decision = Some(true);
        }
        if ui.button("取消").clicked() {
            decision = Some(false);
        }
    });
    decision
}

/// 待处理分支的合并框：逐项选冲突、看合并结果与规则复核。点了确认返回 `true`。
pub(crate) fn pending_merge_form(
    ui: &mut egui::Ui,
    dialog: &mut PendingMergeDialog,
    config: &crate::models::AppConfig,
    body_height: f32,
) -> bool {
    ui.label(format!(
        "导入分支 {}，{}",
        &dialog.preview.head[..8.min(dialog.preview.head.len())],
        if dialog.preview.base.is_some() {
            "已找到共同基线"
        } else {
            "无可信共同基线，请人工选择"
        }
    ));
    ui.weak(format!(
        "导入版本保存于 {}",
        dialog.preview.incoming.created_at
    ));
    if let Some(status) = dialog.preview.imported_status {
        ui.checkbox(
            &mut dialog.take_status,
            format!("同时采用导入侧生命周期状态：{}", status.label()),
        );
    }
    ui.add_space(4.0);
    egui::ScrollArea::vertical()
        .id_salt("pending_merge_body")
        .max_height(body_height)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            let proposal = &dialog.preview.proposal;
            let mut index = 0;
            for conflict in &proposal.field_conflicts {
                ui.group(|ui| {
                    ui.strong(if conflict.path.is_empty() {
                        "全部行文要素".to_string()
                    } else {
                        format!("要素：{}", conflict.path.join(" / "))
                    });
                    ui.label(format!("本机：{}", conflict.local));
                    ui.label(format!("导入：{}", conflict.incoming));
                    ui.checkbox(&mut dialog.choices[index], "采用导入侧");
                });
                index += 1;
            }
            if let Some((local, incoming)) = &proposal.notes_conflict {
                ui.group(|ui| {
                    ui.strong("备注冲突");
                    ui.label(format!("本机：{}", truncate(local, 150)));
                    ui.label(format!("导入：{}", truncate(incoming, 150)));
                    ui.checkbox(&mut dialog.choices[index], "采用导入侧");
                });
                index += 1;
            }
            for chunk in &proposal.markdown {
                if let crate::manuscript::merge::MarkdownChunk::Conflict {
                    base,
                    local,
                    incoming,
                } = chunk
                {
                    ui.group(|ui| {
                        ui.strong("正文冲突");
                        if !base.is_empty() {
                            ui.weak(format!("共同基线：{}", truncate(base, 200)));
                        }
                        ui.label(format!("本机：{}", truncate(local, 200)));
                        ui.label(format!("导入：{}", truncate(incoming, 200)));
                        ui.checkbox(&mut dialog.choices[index], "采用导入侧");
                    });
                    index += 1;
                }
            }
            if let Ok((snapshot, merged, _)) = proposal.resolve(&dialog.choices) {
                ui.strong("合并后正文");
                if let Some(edited) = &mut dialog.markdown_override {
                    ui.add(
                        egui::TextEdit::multiline(edited)
                            .desired_rows(12)
                            .desired_width(f32::INFINITY),
                    );
                    if ui.button("重置为逐项选择的结果").clicked() {
                        *edited = merged.clone();
                    }
                } else {
                    let mut display = merged.clone();
                    ui.add(
                        egui::TextEdit::multiline(&mut display)
                            .interactive(false)
                            .desired_rows(12)
                            .desired_width(f32::INFINITY),
                    );
                    if ui.button("手工调整正文").clicked() {
                        dialog.markdown_override = Some(merged.clone());
                    }
                }
                let candidate = dialog.markdown_override.as_deref().unwrap_or(&merged);
                let hash = crate::manuscript::sync::payload_hash(&snapshot, candidate, "")
                    .unwrap_or_default();
                if dialog
                    .review
                    .as_ref()
                    .is_none_or(|review| review.hash != hash)
                {
                    dialog.review = build_merge_review(config, &snapshot, candidate).ok();
                }
                if let Some(review) = &dialog.review {
                    merge_review_ui(ui, review);
                }
            }
        });
    ui.add_space(6.0);
    dialog_notice(ui, &dialog.notice);
    let mut confirm = false;
    ui.horizontal(|ui| {
        if ui.button("确认生成合并版本").clicked() {
            confirm = true;
        }
        if ui.button("稍后处理").clicked() {
            ui.close();
        }
    });
    confirm
}

/// ZIP 密码框的表单部分。点了提交按钮返回 `true`，校验与执行由调用方做。
pub(crate) fn zip_password_form(
    ui: &mut egui::Ui,
    dialog: &mut ZipPasswordDialog,
    importing: bool,
) -> bool {
    match &dialog.purpose {
        ZipPasswordPurpose::Import(path) => {
            // 路径是暂存副本，文件名与用户双击 / 选中的那个相同。
            if let Some(name) = path.file_name() {
                ui.label(egui::RichText::new(name.to_string_lossy()).strong());
            }
            ui.label("必须输入正确密码后才能读取和导入稿件包。");
        }
        ZipPasswordPurpose::PdfExport(..) => {
            ui.label("本次导出的 PDF 压缩包将使用 AES-256 加密，所有文件均受密码保护。");
        }
        _ => {
            ui.label("本次导出的稿件包将使用 AES-256 加密，所有文件均受密码保护。");
        }
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("密码");
        // 密码不走应用内输入法：否则敲的字母会明文出现在候选窗里
        crate::ime::exempt(
            ui.add(
                egui::TextEdit::singleline(&mut dialog.password)
                    .password(!dialog.show_password)
                    .desired_width(260.0),
            ),
        );
    });
    if !importing {
        ui.horizontal(|ui| {
            ui.label("确认");
            crate::ime::exempt(
                ui.add(
                    egui::TextEdit::singleline(&mut dialog.confirmation)
                        .password(!dialog.show_password)
                        .desired_width(260.0),
                ),
            );
        });
        ui.weak(
            "至少 10 位，并包含大写字母、小写字母、数字、符号或中文中的至少三类；不能使用常见口令、连续或重复字符。",
        );
    }
    ui.horizontal(|ui| {
        ui.checkbox(&mut dialog.remember, "记住密码")
            .on_hover_text("保存在本机受限权限文件中，下次仍会显示确认窗口");
        ui.checkbox(&mut dialog.show_password, "显示密码");
    });
    if matches!(
        dialog.purpose,
        ZipPasswordPurpose::FilteredExport(_) | ZipPasswordPurpose::SelectedExport(_)
    ) {
        ui.checkbox(
            &mut dialog.include_send_package,
            "连同呈批件的送批材料一起导出",
        )
        .on_hover_text(
            "把所选呈批件挂着的函稿、研究报告等随行件也打进包里；不勾时只带清单，对方本机没有这些稿件时显示为「本机未找到」",
        );
    }
    if let Some(error) = &dialog.error {
        ui.colored_label(warn(), error);
    }
    ui.add_space(10.0);
    let mut submit = false;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !dialog.password.is_empty(),
                theme::icon_text_button(
                    if importing {
                        theme::Icon::PackageOpen
                    } else {
                        theme::Icon::Package
                    },
                    if importing {
                        "解密并预览"
                    } else {
                        "继续导出"
                    },
                ),
            )
            .clicked()
        {
            submit = true;
        }
        if ui.button("取消").clicked() {
            ui.close();
        }
    });
    submit
}

pub(crate) fn import_action_label(action: &manuscript_io::sync::ImportAction) -> &'static str {
    use manuscript_io::sync::ImportAction;
    match action {
        ImportAction::Skip => "跳过",
        ImportAction::New => "作为新稿件导入",
        ImportAction::UseIncoming { .. } => "采用导入版",
        ImportAction::Merge { .. } => "合并为新版本",
        ImportAction::Pending => "暂存待处理分支",
        ImportAction::LinkPending { .. } => "关联并暂存分支",
        ImportAction::Copy => "另存可编辑副本",
    }
}

pub(crate) fn import_preview_details(
    ui: &mut egui::Ui,
    preview: &mut ImportPreview,
    index: usize,
    config: &crate::models::AppConfig,
) {
    use manuscript_io::sync::{ImportAction, Relationship};
    let record = &preview.manifest.records[index];
    let relation = &preview.relations[index];
    ui.strong(format!(
        "《{}》：{}",
        record.title,
        relation.relationship.label()
    ));
    ui.label(format!(
        "拟执行：{}",
        import_action_label(&preview.actions[index])
    ));
    let package_diff = preview.package_diffs[index];
    if let Some(diff) = package_diff
        && matches!(
            preview.actions[index],
            ImportAction::Skip | ImportAction::Pending | ImportAction::LinkPending { .. }
        )
    {
        ui.weak(format!(
            "两边送批材料清单不同（本机 {} 件、导入 {} 件）；要采用导入版，请选「采用导入版」或「生成合并版本」后勾选。",
            diff.local, diff.incoming
        ));
    }
    if let Some(local_hash) = &relation.local_hash {
        ui.weak(format!(
            "本机内容：{}；导入内容：{}",
            &local_hash[..12],
            &relation.incoming_hash[..12]
        ));
    }
    if let Some(base) = &relation.base {
        ui.weak(format!(
            "共同基线：{} · {}",
            base.created_at,
            &base.revision_uuid[..8]
        ));
    }
    if let Some(local) = &relation.local_markdown {
        ui.collapsing("本机正文全文", |ui| {
            let mut text = local.clone();
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .interactive(false)
                    .desired_rows(10)
                    .desired_width(f32::INFINITY),
            );
        });
    }
    ui.collapsing("导入正文全文", |ui| {
        let mut text = record.content_markdown.clone();
        ui.add(
            egui::TextEdit::multiline(&mut text)
                .interactive(false)
                .desired_rows(10)
                .desired_width(f32::INFINITY),
        );
    });
    ui.horizontal_wrapped(|ui| {
        if ui.button("跳过").clicked() {
            preview.actions[index] = ImportAction::Skip;
        }
        if matches!(
            relation.relationship,
            Relationship::New | Relationship::Legacy
        ) && ui.button("作为新稿导入").clicked()
        {
            preview.actions[index] = ImportAction::New;
        }
        if matches!(
            relation.relationship,
            Relationship::IncomingAhead | Relationship::Equivalent | Relationship::Same
        ) && ui.button("采用导入版及新增附件").clicked()
        {
            preview.actions[index] = ImportAction::UseIncoming {
                take_status: false,
                take_package: manuscript_io::sync::default_take_package(
                    relation.relationship,
                    package_diff,
                ),
            };
        }
        if relation.relationship == Relationship::Diverged
            && let Some(proposal) = &relation.proposal
            && ui.button("生成合并版本").clicked()
        {
            preview.actions[index] = ImportAction::Merge {
                choices: vec![false; proposal.conflict_count()],
                markdown_override: None,
                take_status: false,
                take_package: manuscript_io::sync::default_take_package(
                    relation.relationship,
                    package_diff,
                ),
            };
        }
        if relation.local_id.is_some()
            && !matches!(relation.relationship, Relationship::Archived)
            && ui.button("暂存分支").clicked()
        {
            preview.actions[index] = ImportAction::Pending;
        }
        if matches!(
            relation.relationship,
            Relationship::Archived | Relationship::NoBase
        ) && ui.button("另存副本").clicked()
        {
            preview.actions[index] = ImportAction::Copy;
        }
    });
    if matches!(
        relation.relationship,
        Relationship::New | Relationship::Legacy
    ) && !relation.candidates.is_empty()
    {
        ui.weak("标题或文号相近，仅供人工判断，不会自动关联：");
        for &candidate in &relation.candidates {
            if ui
                .button(format!("关联本机稿件 #{candidate} 并暂存"))
                .clicked()
            {
                preview.actions[index] = ImportAction::LinkPending {
                    local_id: candidate,
                };
            }
        }
    }
    if let ImportAction::UseIncoming {
        take_status,
        take_package,
    }
    | ImportAction::Merge {
        take_status,
        take_package,
        ..
    } = &mut preview.actions[index]
        && let Some(local_id) = relation.local_id
    {
        ui.checkbox(
            take_status,
            format!(
                "同时采用导入侧生命周期状态（本机稿件 #{local_id} → {}）",
                record.status.label()
            ),
        );
        // 清单增删不产生版本，分不出哪边新：两边不同时由人决定。
        if let Some(diff) = package_diff {
            ui.checkbox(
                take_package,
                format!(
                    "送批材料清单也采用导入版（本机 {} 件 → 导入 {} 件）",
                    diff.local, diff.incoming
                ),
            );
        }
    }
    if let (
        Some(proposal),
        ImportAction::Merge {
            choices,
            markdown_override,
            ..
        },
    ) = (&relation.proposal, &mut preview.actions[index])
    {
        ui.label(format!(
            "技术冲突 {} 处；每处默认保留本机内容。",
            proposal.conflict_count()
        ));
        let mut choice_index = 0;
        for conflict in &proposal.field_conflicts {
            ui.group(|ui| {
                ui.strong(format!("要素：{}", conflict.path.join(" / ")));
                ui.label(format!("本机：{}", conflict.local));
                ui.label(format!("导入：{}", conflict.incoming));
                ui.checkbox(&mut choices[choice_index], "采用导入侧");
            });
            choice_index += 1;
        }
        if let Some((local, incoming)) = &proposal.notes_conflict {
            ui.group(|ui| {
                ui.strong("备注冲突");
                ui.label(format!("本机：{}", truncate(local, 150)));
                ui.label(format!("导入：{}", truncate(incoming, 150)));
                ui.checkbox(&mut choices[choice_index], "采用导入侧");
            });
            choice_index += 1;
        }
        for chunk in &proposal.markdown {
            if let crate::manuscript::merge::MarkdownChunk::Conflict {
                base,
                local,
                incoming,
            } = chunk
            {
                ui.group(|ui| {
                    ui.strong("正文冲突");
                    ui.weak(format!("共同基线：{}", truncate(base, 200)));
                    ui.label(format!("本机：{}", truncate(local, 200)));
                    ui.label(format!("导入：{}", truncate(incoming, 200)));
                    ui.checkbox(&mut choices[choice_index], "采用导入侧");
                });
                choice_index += 1;
            }
        }
        if let Ok((snapshot, merged, _)) = proposal.resolve(choices) {
            ui.strong("合并后正文");
            if let Some(edited) = markdown_override {
                ui.add(
                    egui::TextEdit::multiline(edited)
                        .desired_rows(12)
                        .desired_width(f32::INFINITY),
                );
                if ui.button("重置为逐项选择的结果").clicked() {
                    *edited = merged.clone();
                }
            } else {
                let mut display = merged.clone();
                ui.add(
                    egui::TextEdit::multiline(&mut display)
                        .interactive(false)
                        .desired_rows(12)
                        .desired_width(f32::INFINITY),
                );
                if ui.button("手工调整正文").clicked() {
                    *markdown_override = Some(merged.clone());
                }
            }
            let candidate = markdown_override.as_deref().unwrap_or(&merged);
            let hash =
                crate::manuscript::sync::payload_hash(&snapshot, candidate, "").unwrap_or_default();
            if preview.reviews[index]
                .as_ref()
                .is_none_or(|review| review.hash != hash)
            {
                preview.reviews[index] = build_merge_review(config, &snapshot, candidate).ok();
            }
            if let Some(review) = &preview.reviews[index] {
                merge_review_ui(ui, review);
            }
        }
    }
}

pub(crate) fn build_merge_review(
    config: &crate::models::AppConfig,
    snapshot: &crate::models::DraftInput,
    markdown: &str,
) -> anyhow::Result<MergeReview> {
    let hash = crate::manuscript::sync::payload_hash(snapshot, markdown, "")?;
    let parsed_blocks = crate::export::parse_markdown(markdown).len();
    let warnings = crate::validator::validate(
        snapshot,
        markdown,
        &config.vocabulary,
        &config.security_rules,
    );
    let mut notes =
        crate::proofread_rules::check_with_config(snapshot, markdown, &config.proofread);
    notes.extend(crate::proofread::Lexicon::resolved(&config.proofread).check(markdown));
    Ok(MergeReview {
        hash,
        parsed_blocks,
        warnings,
        notes: notes
            .into_iter()
            .map(|note| format!("{}：{}", note.level.label(), note.message))
            .collect(),
    })
}

pub(crate) fn merge_review_ui(ui: &mut egui::Ui, review: &MergeReview) {
    ui.label(format!(
        "合并候选已解析并复核：{} 个区块，{} 条要素提示，{} 条文字提示。",
        review.parsed_blocks,
        review.warnings.len(),
        review.notes.len()
    ));
    ui.collapsing("查看复核详情", |ui| {
        for warning in &review.warnings {
            ui.colored_label(warn(), warning);
        }
        for note in &review.notes {
            ui.colored_label(warn(), note);
        }
    });
}
