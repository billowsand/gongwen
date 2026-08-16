//! AI 提示词管理页与「AI 优化」提示词选择面板。
//!
//! 由 src/app.rs 拆分而来：本文件是模块 `app::ai_prompts`，与其它子模块共享
//! `app` 根模块的私有可见性（`GongwenApp` 结构体与根模块常量仍在 app.rs 中）。

use crate::app::{GongwenApp, accent, summarize, warn};
use crate::models::{AiPrompt, TemplateKind, builtin_ai_prompts};
use crate::prompt;
use crate::theme;
use eframe::egui;

/// 点“AI 优化”后弹出的提示词选择面板。`custom` 是一次性指令，用完即弃，
/// 不进提示词库。
#[derive(Default)]
pub(crate) struct AiPromptPicker {
    keyword: String,
    custom: String,
    /// 只列出适用于该文种的提示词；面板打开那一刻的文种，中途切换不影响。
    kind: TemplateKind,
}

/// AI 管理页右侧的编辑区。改动先落在这里，点“保存更改”才写回配置。
pub(crate) struct AiPromptDraft {
    /// 新建时为 None，保存时才分配 id。
    id: Option<u32>,
    name: String,
    instruction: String,
    kinds: Vec<TemplateKind>,
    builtin_key: String,
    error: Option<String>,
}

impl AiPromptDraft {
    pub(crate) fn from_entry(entry: &AiPrompt) -> Self {
        Self {
            id: Some(entry.id),
            name: entry.name.clone(),
            instruction: entry.instruction.clone(),
            kinds: entry.kinds.clone(),
            builtin_key: entry.builtin_key.clone(),
            error: None,
        }
    }

    pub(crate) fn blank() -> Self {
        Self {
            id: None,
            name: String::new(),
            instruction: String::new(),
            kinds: vec![],
            builtin_key: String::new(),
            error: None,
        }
    }
}

impl GongwenApp {
    /// 打开提示词选择面板。面板记下打开那一刻的文种，只列适用条目。
    pub(crate) fn open_ai_prompt_picker(&mut self) {
        if self.doc().busy {
            return;
        }
        if !self.draft_page().can_optimize() {
            self.status = "还没有可优化的内容：请先在右侧粘贴稿件，或填写写作素材。".into();
            return;
        }
        self.ai_prompt_picker = Some(AiPromptPicker {
            kind: self.doc().draft.kind,
            ..Default::default()
        });
    }

    /// 提示词管理页：左侧列表，右侧编辑区，底部常驻内置输出标准的只读预览。
    pub(crate) fn ai_prompts_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading("AI 管理");
                let builtin = self
                    .config
                    .ai_prompts
                    .iter()
                    .filter(|entry| entry.is_builtin())
                    .count();
                ui.weak(format!(
                    "{} 条优化提示词（内置 {builtin} 条）· 输出格式标准内置生效，不可关闭 · 仅保存在本机",
                    self.config.ai_prompts.len()
                ));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary_icon_button(ui, theme::Icon::Save, "保存更改").clicked() {
                    self.persist();
                }
                if ui
                    .add(theme::icon_text_button(theme::Icon::FilePlus, "新建提示词"))
                    .on_hover_text("新增一条自定义优化提示词")
                    .clicked()
                {
                    self.ai_prompt_selected = None;
                    self.ai_prompt_editor = Some(AiPromptDraft::blank());
                }
            });
        });
        ui.add_space(8.0);

        egui::ScrollArea::vertical()
            .id_salt("ai_prompts")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let available = ui.available_width();
                // 列表和编辑区各占一半；窄窗口下让列表优先，编辑区自然收窄。
                let list_width = (available * 0.42).clamp(240.0, 460.0);
                ui.horizontal_top(|ui| {
                    // 高度给 0 让它按内容撑开：这里已经在滚动区里，若按
                    // available_height 预留，列表会独占整屏，把下面的标准预览挤出可视区。
                    ui.allocate_ui_with_layout(
                        egui::vec2(list_width, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| self.ai_prompt_list_ui(ui),
                    );
                    // 这里不用 ui.separator()：横向布局里的分隔线会撑满可视高度，
                    // 把下面的标准预览顶出滚动区。
                    ui.add_space(12.0);
                    ui.vertical(|ui| self.ai_prompt_editor_ui(ui));
                });
                ui.add_space(12.0);
                self.output_contract_preview_ui(ui);
            });
    }

    pub(crate) fn ai_prompt_list_ui(&mut self, ui: &mut egui::Ui) {
        let mut edit: Option<u32> = None;
        let mut duplicate: Option<u32> = None;
        let mut restore: Option<u32> = None;
        let mut move_up: Option<usize> = None;
        let mut move_down: Option<usize> = None;
        let mut delete: Option<u32> = None;
        let last = self.config.ai_prompts.len().saturating_sub(1);

        for (index, entry) in self.config.ai_prompts.iter().enumerate() {
            let selected = self.ai_prompt_selected == Some(entry.id);
            let frame = if selected {
                theme::card().fill(theme::accent_soft())
            } else {
                theme::card()
            };
            frame.show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&entry.name).strong());
                    if entry.is_builtin() {
                        theme::chip(ui, "内置", theme::info(), theme::surface_sunk());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if entry.is_builtin() {
                            if theme::icon_button(ui, theme::Icon::RotateCcw, "恢复默认")
                                .on_hover_text("把这条内置提示词还原为出厂内容")
                                .clicked()
                            {
                                restore = Some(entry.id);
                            }
                        } else if theme::icon_button(ui, theme::Icon::Trash, "删除").clicked() {
                            delete = Some(entry.id);
                        }
                        if theme::icon_button(ui, theme::Icon::Copy, "复制一份")
                            .on_hover_text("以这条为底稿新建一条可自由修改的提示词")
                            .clicked()
                        {
                            duplicate = Some(entry.id);
                        }
                        if theme::icon_button_enabled(
                            ui,
                            index < last,
                            theme::Icon::ArrowDown,
                            "下移",
                        )
                        .clicked()
                        {
                            move_down = Some(index);
                        }
                        if theme::icon_button_enabled(ui, index > 0, theme::Icon::ArrowUp, "上移")
                            .on_hover_text("列表顺序就是选择面板里的顺序")
                            .clicked()
                        {
                            move_up = Some(index);
                        }
                    });
                });
                ui.weak(entry.kinds_label());
                let preview = if entry.instruction.trim().is_empty() {
                    "（无附加指令：只按内置标准做格式规整）".to_string()
                } else {
                    summarize(&entry.instruction, 60)
                };
                ui.label(egui::RichText::new(preview).color(theme::text_soft()));
                if ui
                    .add(theme::icon_text_button(theme::Icon::Edit, "编辑"))
                    .clicked()
                {
                    edit = Some(entry.id);
                }
            });
            ui.add_space(6.0);
        }

        if self.config.ai_prompts.is_empty() {
            ui.weak("提示词库为空。点右上角“新建提示词”添加一条。");
        }

        if let Some(index) = move_up {
            self.config.ai_prompts.swap(index, index - 1);
        }
        if let Some(index) = move_down {
            self.config.ai_prompts.swap(index, index + 1);
        }
        if let Some(id) = edit
            && let Some(entry) = self.config.ai_prompt(id)
        {
            self.ai_prompt_editor = Some(AiPromptDraft::from_entry(entry));
            self.ai_prompt_selected = Some(id);
        }
        if let Some(id) = duplicate {
            self.duplicate_ai_prompt(id);
        }
        if let Some(id) = restore {
            self.restore_builtin_ai_prompt(id);
        }
        if let Some(id) = delete {
            self.ai_prompt_delete_confirm = Some(id);
        }
        self.ai_prompt_delete_confirm_ui(ui);
    }

    /// 删除确认就地展开，不再弹窗；确认后同时清掉可能正在编辑它的编辑区。
    pub(crate) fn ai_prompt_delete_confirm_ui(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.ai_prompt_delete_confirm else {
            return;
        };
        let Some(name) = self.config.ai_prompt(id).map(|entry| entry.name.clone()) else {
            self.ai_prompt_delete_confirm = None;
            return;
        };
        theme::card().fill(theme::danger_soft()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.colored_label(theme::danger(), format!("删除提示词“{name}”？"));
            ui.horizontal(|ui| {
                if ui
                    .add(theme::warning_icon_button(theme::Icon::Trash, "确认删除"))
                    .clicked()
                {
                    self.config.ai_prompts.retain(|entry| entry.id != id);
                    if self.ai_prompt_selected == Some(id) {
                        self.ai_prompt_selected = None;
                        self.ai_prompt_editor = None;
                    }
                    self.ai_prompt_delete_confirm = None;
                    self.status = format!("已删除提示词“{name}”。记得点“保存更改”。");
                }
                if ui.button("取消").clicked() {
                    self.ai_prompt_delete_confirm = None;
                }
            });
        });
    }

    pub(crate) fn ai_prompt_editor_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut draft) = self.ai_prompt_editor.take() else {
            ui.add_space(16.0);
            ui.weak("在左侧选一条提示词编辑，或新建一条。");
            return;
        };
        let mut close = false;
        let mut submit = false;

        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(if draft.id.is_some() {
                        "编辑提示词"
                    } else {
                        "新建提示词"
                    })
                    .strong(),
                );
                if !draft.builtin_key.is_empty() {
                    theme::chip(ui, "内置", theme::info(), theme::surface_sunk());
                }
            });
            ui.add_space(6.0);

            ui.label("名称");
            ui.add(
                egui::TextEdit::singleline(&mut draft.name)
                    .hint_text("例如：精简篇幅")
                    .desired_width(ui.available_width()),
            );
            ui.add_space(8.0);

            ui.label("适用文种")
                .on_hover_text("一个都不勾表示所有文种通用；勾选后只在对应文种的选择面板里出现。");
            ui.horizontal_wrapped(|ui| {
                for kind in TemplateKind::ALL {
                    let mut checked = draft.kinds.contains(&kind);
                    if ui.checkbox(&mut checked, kind.label()).changed() {
                        if checked {
                            draft.kinds.push(kind);
                        } else {
                            draft.kinds.retain(|item| *item != kind);
                        }
                    }
                }
            });
            ui.add_space(8.0);

            ui.label("优化指令").on_hover_ui(|ui| {
                ui.label("只写“这次要模型做什么”。");
                ui.label(
                    "输出的 Markdown 结构、表格写法、不得输出版记落款等\
要求由内置标准强制，无需也无法在这里改。",
                );
            });
            ui.add(
                egui::TextEdit::multiline(&mut draft.instruction)
                    .hint_text("留空表示只按内置标准做格式规整")
                    .desired_width(ui.available_width())
                    .desired_rows(10),
            );

            if let Some(error) = &draft.error {
                ui.add_space(4.0);
                ui.colored_label(warn(), error);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if theme::primary_icon_button(ui, theme::Icon::Save, "应用到提示词库")
                    .on_hover_text("写回提示词库；仍需点右上角“保存更改”落盘")
                    .clicked()
                {
                    submit = true;
                }
                if ui.button("取消").clicked() {
                    close = true;
                }
            });
        });

        if submit {
            match self.apply_ai_prompt_draft(&draft) {
                Ok(id) => {
                    self.ai_prompt_selected = Some(id);
                    self.status = "提示词已更新。点右上角“保存更改”写入本机配置。".into();
                    return;
                }
                Err(message) => draft.error = Some(message),
            }
        }
        if !close {
            self.ai_prompt_editor = Some(draft);
        }
    }

    /// 把编辑区内容写回提示词库。名称必填且同名会挡下——选择面板只显示名称，
    /// 重名了没法区分。
    pub(crate) fn apply_ai_prompt_draft(&mut self, draft: &AiPromptDraft) -> Result<u32, String> {
        let name = draft.name.trim();
        if name.is_empty() {
            return Err("请填写提示词名称。".into());
        }
        if self
            .config
            .ai_prompts
            .iter()
            .any(|entry| entry.name.trim() == name && Some(entry.id) != draft.id)
        {
            return Err(format!("已有同名提示词“{name}”，请换一个名称。"));
        }
        // 勾选顺序不确定，按文种固有顺序归一，列表文案才稳定。
        let kinds = TemplateKind::ALL
            .into_iter()
            .filter(|kind| draft.kinds.contains(kind))
            .collect::<Vec<_>>();

        match draft.id {
            Some(id) => {
                let entry = self
                    .config
                    .ai_prompts
                    .iter_mut()
                    .find(|entry| entry.id == id)
                    .ok_or_else(|| "提示词已不存在，请重新选择。".to_string())?;
                entry.name = name.to_string();
                entry.instruction = draft.instruction.trim().to_string();
                entry.kinds = kinds;
                Ok(id)
            }
            None => {
                let id = self.config.next_ai_prompt_id();
                self.config.ai_prompts.push(AiPrompt {
                    id,
                    name: name.to_string(),
                    instruction: draft.instruction.trim().to_string(),
                    kinds,
                    builtin_key: String::new(),
                });
                Ok(id)
            }
        }
    }

    /// 复制出来的副本一律是自定义条目（不带 builtin_key），可以随便改和删。
    pub(crate) fn duplicate_ai_prompt(&mut self, id: u32) {
        let Some(source) = self.config.ai_prompt(id).cloned() else {
            return;
        };
        let mut name = format!("{} 副本", source.name);
        let mut suffix = 2;
        while self
            .config
            .ai_prompts
            .iter()
            .any(|entry| entry.name == name)
        {
            name = format!("{} 副本{suffix}", source.name);
            suffix += 1;
        }
        let new_id = self.config.next_ai_prompt_id();
        self.config.ai_prompts.push(AiPrompt {
            id: new_id,
            name,
            instruction: source.instruction,
            kinds: source.kinds,
            builtin_key: String::new(),
        });
        self.ai_prompt_selected = Some(new_id);
        if let Some(entry) = self.config.ai_prompt(new_id) {
            self.ai_prompt_editor = Some(AiPromptDraft::from_entry(entry));
        }
        self.status = "已复制一份可自由修改的提示词。".into();
    }

    /// 内置项改坏了可以还原：按 builtin_key 找出厂内容覆盖回去，id 和排序不变。
    pub(crate) fn restore_builtin_ai_prompt(&mut self, id: u32) {
        let Some(key) = self
            .config
            .ai_prompt(id)
            .map(|entry| entry.builtin_key.clone())
        else {
            return;
        };
        let Some(defaults) = builtin_ai_prompts()
            .into_iter()
            .find(|entry| entry.builtin_key == key)
        else {
            return;
        };
        let Some(entry) = self
            .config
            .ai_prompts
            .iter_mut()
            .find(|entry| entry.id == id)
        else {
            return;
        };
        entry.name = defaults.name;
        entry.instruction = defaults.instruction;
        entry.kinds = defaults.kinds;
        if self.ai_prompt_selected == Some(id)
            && let Some(entry) = self.config.ai_prompt(id)
        {
            self.ai_prompt_editor = Some(AiPromptDraft::from_entry(entry));
        }
        self.status = "已恢复该内置提示词的出厂内容。".into();
    }

    /// 把内置输出标准原样摊开给用户看。它是不可编辑的，但藏着不说会让人
    /// 怀疑自定义提示词到底还受不受约束。
    pub(crate) fn output_contract_preview_ui(&mut self, ui: &mut egui::Ui) {
        let contract_header = egui::CollapsingHeader::new("查看内置输出格式标准（只读）")
            .id_salt("output_contract_preview")
            .show(ui, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("文种");
                    egui::ComboBox::from_id_salt("contract_kind")
                        .selected_text(self.ai_contract_preview_kind.label())
                        .show_ui(ui, |ui| {
                            for kind in TemplateKind::ALL {
                                ui.selectable_value(
                                    &mut self.ai_contract_preview_kind,
                                    kind,
                                    kind.label(),
                                );
                            }
                        });
                });
                ui.add_space(6.0);
                let mut contract = prompt::output_contract(self.ai_contract_preview_kind);
                ui.add(
                    egui::TextEdit::multiline(&mut contract)
                        .desired_width(ui.available_width())
                        .desired_rows(16)
                        .interactive(false),
                );
            });
        contract_header.header_response.on_hover_text(
            "下面这段会自动拼在每条提示词之后，并声明优先级更高：\
自定义指令与它冲突时，一律以它为准。",
        );
    }

    /// “AI 优化”的提示词选择面板。列出适用当前文种的条目，单击即执行；
    /// 底部可以写一条只用一次的临时指令。
    pub(crate) fn ai_prompt_picker_window(&mut self, ctx: &egui::Context) {
        let Some(mut picker) = self.ai_prompt_picker.take() else {
            theme::reset_window_anim(ctx, egui::Id::new("ai_prompt_picker_anim"));
            return;
        };
        // 审校稿为空就是从零起草，非空就是改现有稿件：同一个面板，两种口径。
        let drafting = self
            .active_doc_ref()
            .is_none_or(|doc| doc.generated_markdown.trim().is_empty());
        let mut chosen: Option<(String, String)> = None;
        let mut close = false;

        let win = egui::Window::new(if drafting {
            "AI 起草"
        } else {
            "AI 优化"
        })
            .collapsible(false)
            .resizable(true)
            .default_width(460.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.weak(format!("当前文种：{}", picker.kind.label()))
                    .on_hover_text("输出格式标准始终生效，不受下面的指令影响。");
                if drafting {
                    ui.colored_label(
                        accent(),
                        "当前审校稿为空：下面写明要起草什么，将结合左侧公文要素从零生成。",
                    );
                }
                ui.add_space(6.0);
                ui.add(
                    egui::TextEdit::singleline(&mut picker.keyword)
                        .hint_text("按名称筛选")
                        .desired_width(ui.available_width()),
                );
                ui.add_space(6.0);

                let keyword = picker.keyword.trim().to_lowercase();
                let matches = self
                    .config
                    .ai_prompts
                    .iter()
                    .filter(|entry| entry.applies_to(picker.kind))
                    .filter(|entry| {
                        keyword.is_empty() || entry.name.to_lowercase().contains(&keyword)
                    })
                    .cloned()
                    .collect::<Vec<_>>();

                egui::ScrollArea::vertical()
                    .id_salt("ai_prompt_picker")
                    .max_height(280.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        if matches.is_empty() {
                            ui.weak("没有适用于当前文种的提示词。可以在下面临时写一条，或到“AI 管理”页新增。");
                        }
                        for entry in &matches {
                            let last_used = self.config.last_ai_prompt == entry.id;
                            let frame = if last_used {
                                theme::card().fill(theme::accent_soft())
                            } else {
                                theme::card()
                            };
                            let response = frame
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new(&entry.name).strong());
                                        if last_used {
                                            theme::chip(
                                                ui,
                                                "上次使用",
                                                theme::accent(),
                                                theme::surface_sunk(),
                                            );
                                        }
                                    });
                                    let preview = if entry.instruction.trim().is_empty() {
                                        "只按内置标准做格式规整，不改措辞。".to_string()
                                    } else {
                                        summarize(&entry.instruction, 70)
                                    };
                                    ui.label(
                                        egui::RichText::new(preview).color(theme::text_soft()),
                                    );
                                })
                                .response
                                .interact(egui::Sense::click());
                            let tip = if drafting {
                                "按这条提示词起草（素材写在下面的临时提示词里）"
                            } else {
                                "按这条提示词优化当前稿件"
                            };
                            if response.on_hover_text(tip).clicked() {
                                self.config.last_ai_prompt = entry.id;
                                chosen =
                                    Some((entry.instruction.clone(), entry.name.clone()));
                            }
                            ui.add_space(4.0);
                        }
                    });

                ui.add_space(8.0);
                ui.separator();
                ui.label(
                    egui::RichText::new(if drafting {
                        "写作素材与要求（用完即弃，不进提示词库）"
                    } else {
                        "临时提示词（用完即弃，不进提示词库）"
                    })
                    .strong(),
                );
                ui.add(
                    egui::TextEdit::multiline(&mut picker.custom)
                        .hint_text(if drafting {
                            "例如：就 2026 年度教师培训经费事项向省财政厅去函，背景是……，请求是……"
                        } else {
                            "例如：把第三部分改写成三条并列举措，保留全部数据"
                        })
                        .desired_width(ui.available_width())
                        .desired_rows(if drafting { 8 } else { 4 }),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let has_custom = !picker.custom.trim().is_empty();
                    if theme::primary_icon_button_enabled(
                        ui,
                        has_custom,
                        theme::Icon::Sparkles,
                        if drafting { "按此素材起草" } else { "按此提示词优化" },
                    )
                    .clicked()
                    {
                        let label = if drafting { "临时素材" } else { "临时提示词" };
                        chosen = Some((picker.custom.trim().to_string(), label.into()));
                    }
                    if ui.button("取消").clicked() {
                        close = true;
                    }
                });
            });
        if let Some(w) = win {
            theme::window_enter_anim(ctx, egui::Id::new("ai_prompt_picker_anim"), &w.response);
        }

        if let Some((instruction, label)) = chosen {
            self.draft_page().start_optimize(instruction, label);
            return;
        }
        if !close {
            self.ai_prompt_picker = Some(picker);
        }
    }
}
