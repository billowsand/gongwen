//! AI 管理页的「润色预设」与「输出标准」两个分区（侧栏技能的 `{preset}`）。
//! 页面布局见 `app::ai_manage`。
//!
//! 由 src/app.rs 拆分而来：本文件是模块 `app::ai_prompts`，与其它子模块共享
//! `app` 根模块的私有可见性（`GongwenApp` 结构体与根模块常量仍在 app.rs 中）。

use crate::app::{GongwenApp, summarize, warn};
use crate::models::{AiPrompt, TemplateKind, builtin_ai_prompts};
use crate::prompt;
use crate::theme;
use eframe::egui;

/// AI 管理页右侧的编辑区。先应用到提示词库，保存更改时写回配置。
pub(crate) struct AiPromptDraft {
    /// 新建时为 None，保存时才分配 id。
    id: Option<u32>,
    name: String,
    instruction: String,
    kinds: Vec<TemplateKind>,
    error: Option<String>,
    focus_name: bool,
}

impl AiPromptDraft {
    pub(crate) fn from_entry(entry: &AiPrompt) -> Self {
        Self {
            id: Some(entry.id),
            name: entry.name.clone(),
            instruction: entry.instruction.clone(),
            kinds: entry.kinds.clone(),
            error: None,
            focus_name: false,
        }
    }

    pub(crate) fn blank() -> Self {
        Self {
            id: None,
            name: String::new(),
            instruction: String::new(),
            kinds: vec![],
            error: None,
            focus_name: true,
        }
    }

    fn has_pending_changes(&self, entry: Option<&AiPrompt>) -> bool {
        match entry {
            Some(entry) => {
                self.name != entry.name
                    || self.instruction != entry.instruction
                    || self.kinds != entry.kinds
            }
            None => {
                !self.name.trim().is_empty()
                    || !self.instruction.trim().is_empty()
                    || !self.kinds.is_empty()
            }
        }
    }
}

impl GongwenApp {
    fn can_switch_ai_prompt(&mut self) -> bool {
        let pending = self.ai_prompt_editor.as_ref().is_some_and(|draft| {
            draft.has_pending_changes(draft.id.and_then(|id| self.config.ai_prompt(id)))
        });
        if pending && let Some(draft) = self.ai_prompt_editor.as_mut() {
            draft.error = Some("请先应用或取消当前编辑，再切换提示词。".into());
        }
        !pending
    }

    /// 润色预设分区：顶上一行计数与「新建预设」，下面列表与编辑区——够宽时并排，
    /// 窄了上下排。
    pub(crate) fn ai_presets_section_ui(&mut self, ui: &mut egui::Ui) {
        if self.ai_prompt_editor.is_none() {
            let entry = self
                .ai_prompt_selected
                .and_then(|id| self.config.ai_prompt(id))
                .or_else(|| self.config.ai_prompts.first());
            if let Some(entry) = entry {
                self.ai_prompt_selected = Some(entry.id);
                self.ai_prompt_editor = Some(AiPromptDraft::from_entry(entry));
            }
        }

        ui.horizontal(|ui| {
            let builtin = self
                .config
                .ai_prompts
                .iter()
                .filter(|entry| entry.is_builtin())
                .count();
            ui.weak(format!(
                "{} 条预设（内置 {builtin} 条）· 仅保存在本机",
                self.config.ai_prompts.len()
            ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(theme::icon_text_button(theme::Icon::FilePlus, "新建预设"))
                    .on_hover_text("新建条目保存后排在列表最上方")
                    .clicked()
                    && self.can_switch_ai_prompt()
                {
                    self.ai_prompt_selected = None;
                    self.ai_prompt_editor = Some(AiPromptDraft::blank());
                }
            });
        });
        ui.add_space(6.0);

        let content_width = ui.available_width();
        let item_gap = ui.spacing().item_spacing.x;
        if content_width < 760.0 {
            self.ai_prompt_list_ui(ui);
            ui.add_space(8.0);
            self.ai_prompt_editor_ui(ui);
        } else {
            let list_width = (content_width * 0.38).clamp(280.0, 440.0);
            let editor_width = content_width - list_width - 12.0 - 2.0 * item_gap;
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(list_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(list_width);
                        self.ai_prompt_list_ui(ui);
                    },
                );
                ui.add_space(12.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(editor_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(editor_width);
                        self.ai_prompt_editor_ui(ui);
                    },
                );
            });
        }
    }

    /// 页底「保存」前先把编辑区应用到预设库；名称不合格时把错误挂在编辑区上，返回 false。
    pub(crate) fn apply_pending_ai_prompt(&mut self) -> bool {
        let Some(mut draft) = self.ai_prompt_editor.take() else {
            return true;
        };
        let entry = draft.id.and_then(|id| self.config.ai_prompt(id));
        if !draft.has_pending_changes(entry) {
            self.ai_prompt_editor = Some(draft);
            return true;
        }
        match self.apply_ai_prompt_draft(&draft) {
            Ok(id) => {
                self.ai_prompt_selected = Some(id);
                self.ai_prompt_editor = self.config.ai_prompt(id).map(AiPromptDraft::from_entry);
                true
            }
            Err(message) => {
                draft.error = Some(message);
                self.ai_prompt_editor = Some(draft);
                false
            }
        }
    }

    pub(crate) fn ai_prompt_list_ui(&mut self, ui: &mut egui::Ui) {
        let mut edit = None;
        theme::card().show(ui, |ui| {
            let row_width = (ui.available_width() - 20.0).max(220.0);
            ui.set_width(row_width);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("提示词库").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak("按顺序显示");
                });
            });
            for (index, entry) in self.config.ai_prompts.iter().enumerate() {
                let selected = self.ai_prompt_selected == Some(entry.id);
                let frame = if selected {
                    theme::card()
                        .fill(theme::accent_soft())
                        .inner_margin(egui::Margin::symmetric(10, 5))
                } else {
                    theme::card().inner_margin(egui::Margin::symmetric(10, 5))
                };
                // 整张卡片可点：手型指针 + 悬停/按下底色过渡，见 `theme::clickable_card`。
                let row =
                    theme::clickable_card(ui, ("ai_prompt_row", entry.id), frame, selected, |ui| {
                        ui.set_width((row_width - 20.0).max(180.0));
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [24.0, 22.0],
                                egui::Label::new(
                                    egui::RichText::new(format!("{:02}", index + 1))
                                        .color(theme::text_muted()),
                                ),
                            );
                            ui.add_space(3.0);
                            // 标题占满左侧剩余空间，状态标签固定在行尾；长标题截断。
                            let badge_width = if entry.is_builtin() { 64.0 } else { 0.0 };
                            let title_width =
                                (ui.available_width() - badge_width - ui.spacing().item_spacing.x)
                                    .max(80.0);
                            // 标题左对齐，紧跟序号，和下面的摘要行起笔一致。
                            ui.allocate_ui_with_layout(
                                egui::vec2(title_width, 22.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.set_min_width(title_width);
                                    ui.add(
                                        egui::Label::new(egui::RichText::new(&entry.name).strong())
                                            .truncate(),
                                    );
                                },
                            );
                            if entry.is_builtin() {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        theme::chip(
                                            ui,
                                            "内置",
                                            theme::info(),
                                            theme::surface_sunk(),
                                        );
                                    },
                                );
                            }
                        });
                        let preview = if entry.instruction.trim().is_empty() {
                            "只按内置标准做格式规整".to_string()
                        } else {
                            summarize(&entry.instruction, 36)
                        };
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("{} · {preview}", entry.kinds_label()))
                                    .color(theme::text_soft()),
                            )
                            .truncate(),
                        )
                        .on_hover_text(&entry.instruction);
                    });
                if row.response.clicked() {
                    edit = Some(entry.id);
                }
            }

            if self.config.ai_prompts.is_empty() {
                ui.weak("提示词库为空。点上方“新建预设”添加一条。");
            }
        });

        if let Some(id) = edit
            && self.ai_prompt_selected != Some(id)
            && self.can_switch_ai_prompt()
            && let Some(entry) = self.config.ai_prompt(id)
        {
            self.ai_prompt_selected = Some(id);
            self.ai_prompt_editor = Some(AiPromptDraft::from_entry(entry));
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
            ui.set_width((ui.available_width() - 20.0).max(220.0));
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
                    self.status = format!("已删除提示词“{name}”。记得点页底“保存”。");
                }
                if ui.button("取消").clicked() {
                    self.ai_prompt_delete_confirm = None;
                }
            });
        });
    }

    pub(crate) fn ai_prompt_editor_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut draft) = self.ai_prompt_editor.take() else {
            ui.weak("在左侧选一条提示词编辑，或新建一条。");
            return;
        };
        let mut close = false;
        let mut submit = false;
        let mut move_up = false;
        let mut move_down = false;
        let mut duplicate = false;
        let mut restore = false;
        let mut delete = false;

        theme::card().show(ui, |ui| {
            ui.set_width((ui.available_width() - 20.0).max(240.0));
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(if draft.id.is_some() {
                        "编辑提示词"
                    } else {
                        "新建提示词"
                    })
                    .strong(),
                );
                if let Some(id) = draft.id
                    && let Some(entry) = self.config.ai_prompt(id)
                {
                    if ui.available_width() > 620.0 {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("· {}", entry.name))
                                    .color(theme::text_soft()),
                            )
                            .truncate(),
                        );
                    }
                    if entry.is_builtin() {
                        ui.add_space(2.0);
                        theme::chip(ui, "内置", theme::info(), theme::surface_sunk());
                    }
                }
                if let Some(id) = draft.id
                    && let Some(index) = self
                        .config
                        .ai_prompts
                        .iter()
                        .position(|entry| entry.id == id)
                {
                    let last = self.config.ai_prompts.len().saturating_sub(1);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.config.ai_prompts[index].is_builtin() {
                            restore = theme::icon_button(ui, theme::Icon::RotateCcw, "恢复默认")
                                .on_hover_text("把这条内置提示词还原为出厂内容")
                                .clicked();
                        } else {
                            delete =
                                theme::danger_icon_button(ui, theme::Icon::Trash, "删除").clicked();
                        }
                        duplicate = theme::icon_button(ui, theme::Icon::Copy, "复制一份")
                            .on_hover_text("以这条为底稿新建可修改的提示词")
                            .clicked();
                        move_down = theme::icon_button_enabled(
                            ui,
                            index < last,
                            theme::Icon::ArrowDown,
                            "下移",
                        )
                        .on_hover_text("顺序与 AI 优化选择面板一致")
                        .clicked();
                        move_up =
                            theme::icon_button_enabled(ui, index > 0, theme::Icon::ArrowUp, "上移")
                                .on_hover_text("顺序与 AI 优化选择面板一致")
                                .clicked();
                    });
                }
            });
            ui.separator();

            ui.label("名称");
            let name_response = ui.add(
                egui::TextEdit::singleline(&mut draft.name)
                    .hint_text("例如：精简篇幅")
                    .desired_width(ui.available_width()),
            );
            if draft.focus_name {
                name_response.request_focus();
                draft.focus_name = false;
            }
            ui.label("适用文种")
                .on_hover_text("一个都不勾表示所有文种通用；勾选后只在对应文种的选择面板里出现。");
            ui.horizontal_wrapped(|ui| {
                for (index, kind) in TemplateKind::ALL.into_iter().enumerate() {
                    if index > 0 {
                        ui.add_space(3.0);
                    }
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
            ui.label("优化指令").on_hover_ui(|ui| {
                ui.label("只写“这次要模型做什么”。");
                ui.label(
                    "输出的 Markdown 结构、表格写法、不得输出版记落款等\
要求由内置标准强制，无需也无法在这里改。",
                );
            });
            crate::app::widgets::bounded_text_edit(
                ui,
                "ai_prompt_instruction",
                8,
                egui::TextEdit::multiline(&mut draft.instruction)
                    .hint_text("留空表示只按内置标准做格式规整")
                    .desired_width(ui.available_width())
                    .desired_rows(6),
            );

            if let Some(error) = &draft.error {
                ui.add_space(4.0);
                ui.colored_label(warn(), error);
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if theme::primary_icon_button(ui, theme::Icon::Save, "应用到提示词库")
                    .on_hover_text("写回提示词库；仍需点页底“保存”落盘")
                    .clicked()
                {
                    submit = true;
                }
                if ui.button("取消").clicked() {
                    close = true;
                }
            });
        });

        if let Some(id) = draft.id {
            if (move_up || move_down)
                && let Some(index) = self
                    .config
                    .ai_prompts
                    .iter()
                    .position(|entry| entry.id == id)
            {
                let other = if move_up { index - 1 } else { index + 1 };
                self.config.ai_prompts.swap(index, other);
            }
            if duplicate {
                self.duplicate_ai_prompt(id);
                return;
            }
            if restore {
                self.restore_builtin_ai_prompt(id);
                return;
            }
            if delete {
                self.ai_prompt_delete_confirm = Some(id);
            }
        }

        if submit {
            match self.apply_ai_prompt_draft(&draft) {
                Ok(id) => {
                    self.ai_prompt_selected = Some(id);
                    self.ai_prompt_editor =
                        self.config.ai_prompt(id).map(AiPromptDraft::from_entry);
                    self.status = "提示词已更新。点页底“保存”写入本机配置。".into();
                    return;
                }
                Err(message) => draft.error = Some(message),
            }
        }
        if close {
            let entry = draft
                .id
                .and_then(|id| self.config.ai_prompt(id))
                .or_else(|| self.config.ai_prompts.first());
            self.ai_prompt_selected = entry.map(|entry| entry.id);
            self.ai_prompt_editor = entry.map(AiPromptDraft::from_entry);
        } else {
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
                self.config.ai_prompts.insert(
                    0,
                    AiPrompt {
                        id,
                        name: name.to_string(),
                        instruction: draft.instruction.trim().to_string(),
                        kinds,
                        builtin_key: String::new(),
                    },
                );
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
        self.config.ai_prompts.insert(
            0,
            AiPrompt {
                id: new_id,
                name,
                instruction: source.instruction,
                kinds: source.kinds,
                builtin_key: String::new(),
            },
        );
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
    /// 怀疑自定义预设到底还受不受约束。
    pub(crate) fn output_contract_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("文种");
            egui::ComboBox::from_id_salt("contract_kind")
                .selected_text(self.ai_contract_preview_kind.label())
                .show_ui(ui, |ui| {
                    for kind in TemplateKind::ALL {
                        ui.selectable_value(&mut self.ai_contract_preview_kind, kind, kind.label());
                    }
                });
        });
        ui.add_space(6.0);
        let mut contract = prompt::output_contract(self.ai_contract_preview_kind);
        crate::app::widgets::bounded_text_edit(
            ui,
            "ai_output_contract",
            24,
            egui::TextEdit::multiline(&mut contract)
                .desired_width(ui.available_width())
                .desired_rows(24)
                .interactive(false),
        );
    }
}
