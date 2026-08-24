//! 大纲确认与逐节起草的窗口。
//!
//! 这一页的存在意义只有一句：**让结构错误在几十个字的时候被改掉**。所以它的
//! 重心不是好看，是「增删改序」四件事要顺手——大纲摆在眼前却不好改，等于没拆。
//!
//! 合稿按钮到最后才亮。这不是为了仪式感：全部小节都有正文才谈得上合稿，
//! 少一节合出来就是个带空标题的残稿，而残稿一旦落进正文，用户还得自己找哪儿缺。

use crate::app::GongwenApp;
use crate::outline::SectionState;
use crate::theme;
use eframe::egui;

/// 一帧里点出的动作，渲染完再执行——循环里借着 `&self.docs`，执行要 `&mut self`。
enum OutlineAction {
    Generate(usize),
    Remove(usize),
    MoveUp(usize),
    MoveDown(usize),
    Insert(usize),
    GenerateAll,
    Assemble,
    Close,
}

impl GongwenApp {
    pub(crate) fn outline_window(&mut self, ctx: &egui::Context) {
        if self.docs.is_empty() || !self.showing_doc() {
            return;
        }
        let index = self.active_doc;
        let Some(draft) = self.docs[index].outline.as_ref() else {
            theme::reset_window_anim(ctx, egui::Id::new("outline_anim"));
            return;
        };
        if !draft.open {
            return;
        }
        let busy = self.docs[index].busy;
        let mut keep = true;
        let mut action = None;

        let win = egui::Window::new("大纲起草")
            // 显式换 id，丢掉旧版本被撑到屏幕底部、又被 egui 持久化下来的窗口高度。
            .id(egui::Id::new("outline_window_v2"))
            .open(&mut keep)
            .collapsible(false)
            .resizable(true)
            .default_width(760.0)
            .default_height(620.0)
            .min_width(560.0)
            .min_height(420.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                let Some(draft) = self.docs[index].outline.as_mut() else {
                    return;
                };
                ui.horizontal_wrapped(|ui| {
                    ui.heading("确认大纲");
                    let total = draft.outline.sections.len();
                    let done = draft.outline.done_count();
                    theme::chip(
                        ui,
                        &format!("{done}/{total} 节已生成"),
                        theme::accent(),
                        theme::accent_soft(),
                    );
                });
                ui.weak(
                    "先把结构改对再生成正文：标题和要点都可以直接编辑，也可以增删和调序。\
                     某一节写坏了只重跑那一节，不必整篇重来。",
                );

                if let Some(error) = &draft.error {
                    ui.add_space(6.0);
                    ui.colored_label(theme::danger(), error);
                }

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [64.0, 20.0],
                        egui::Label::new(egui::RichText::new("公文标题").strong()),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut draft.outline.title)
                            .hint_text("公文主标题")
                            .desired_width(f32::INFINITY),
                    );
                });

                ui.add_space(8.0);
                // 底栏用 bottom_up 布局实际占位，正文再吃掉剩下的高度。写死的 56px 比底栏
                // 实际高度小（底栏还会换行），正文用 auto_shrink=false 撑满后整窗内容比窗口
                // 高出几像素，而 egui 的 Resize 每帧都做
                // `desired_size = desired_size.max(last_content_size)`，窗口就会自己一路
                // 长到屏幕底部。按真实布局占位后内容高度恒等于窗口高度。
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let ready = draft.outline.ready_to_assemble();
                        let pending = draft
                            .outline
                            .sections
                            .iter()
                            .filter(|item| !item.is_done())
                            .count();
                        if theme::primary_icon_button_enabled(
                            ui,
                            ready && !busy,
                            theme::Icon::SquareCheck,
                            "合稿写入审校稿",
                        )
                        .on_disabled_hover_text(if busy {
                            "有小节正在生成".to_string()
                        } else {
                            format!("还有 {pending} 节没有正文")
                        })
                        .clicked()
                        {
                            action = Some(OutlineAction::Assemble);
                        }
                        if ui
                            .add_enabled(
                                !busy && pending > 0,
                                theme::icon_text_button(
                                    theme::Icon::WandSparkles,
                                    &format!("逐节生成剩余 {pending} 节"),
                                ),
                            )
                            .on_hover_text("一次跑一节，中途可以停下来改大纲")
                            .clicked()
                        {
                            action = Some(OutlineAction::GenerateAll);
                        }
                        if ui
                            .add(theme::icon_text_button(theme::Icon::X, "关闭"))
                            .on_hover_text("大纲会保留，可以从功能区再打开")
                            .clicked()
                        {
                            action = Some(OutlineAction::Close);
                        }
                    });
                    ui.separator();
                    ui.add_space(1.0);
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("outline_sections")
                            .auto_shrink([false; 2])
                            .show(ui, |ui| {
                                let total = draft.outline.sections.len();
                                for (position, section) in
                                    draft.outline.sections.iter_mut().enumerate()
                                {
                                    if let Some(picked) =
                                        section_card(ui, position, total, section, busy)
                                    {
                                        action = Some(picked);
                                    }
                                    ui.add_space(6.0);
                                }
                                if ui
                                    .add(theme::icon_text_button(
                                        theme::Icon::FilePlus,
                                        "在末尾加一节",
                                    ))
                                    .clicked()
                                {
                                    action = Some(OutlineAction::Insert(total));
                                }
                            });
                    });
                });
            });
        if let Some(window) = win {
            theme::window_enter_anim(ctx, egui::Id::new("outline_anim"), &window.response);
        }
        if !keep {
            action = Some(OutlineAction::Close);
        }
        if let Some(action) = action {
            self.apply_outline_action(index, action);
        }
    }

    fn apply_outline_action(&mut self, index: usize, action: OutlineAction) {
        match action {
            OutlineAction::Generate(section) => {
                self.draft_page_at(index).start_section_draft(section);
            }
            OutlineAction::GenerateAll => {
                // 只发起第一节没写的：一次跑一节，中途可以停下来改大纲。
                // 一口气排队全跑，等于把「随时能改」这个好处又还回去了。
                let next = self.docs[index].outline.as_ref().and_then(|draft| {
                    draft
                        .outline
                        .sections
                        .iter()
                        .position(|item| !item.is_done())
                });
                if let Some(section) = next {
                    self.draft_page_at(index).start_section_draft(section);
                }
            }
            OutlineAction::Remove(section) => {
                if let Some(draft) = self.docs[index].outline.as_mut()
                    && section < draft.outline.sections.len()
                {
                    draft.outline.sections.remove(section);
                }
            }
            OutlineAction::Insert(at) => {
                if let Some(draft) = self.docs[index].outline.as_mut() {
                    let at = at.min(draft.outline.sections.len());
                    draft
                        .outline
                        .sections
                        .insert(at, crate::outline::OutlineSection::default());
                }
            }
            OutlineAction::MoveUp(section) => {
                if let Some(draft) = self.docs[index].outline.as_mut()
                    && section > 0
                    && section < draft.outline.sections.len()
                {
                    draft.outline.sections.swap(section - 1, section);
                }
            }
            OutlineAction::MoveDown(section) => {
                if let Some(draft) = self.docs[index].outline.as_mut()
                    && section + 1 < draft.outline.sections.len()
                {
                    draft.outline.sections.swap(section, section + 1);
                }
            }
            OutlineAction::Assemble => {
                let Some(draft) = self.docs[index].outline.as_ref() else {
                    return;
                };
                if !draft.outline.ready_to_assemble() {
                    return;
                }
                let markdown = draft.outline.assemble();
                // 与红线一致：这一步是**用户点的**，不是模型自己写进去的。
                // 已有正文时不覆盖——改走整篇提案那条路由用户对照确认。
                if self.docs[index].generated_markdown.trim().is_empty() {
                    self.docs[index].generated_markdown = markdown;
                    self.docs[index].outline = None;
                    self.draft_page_at(index).revalidate();
                    self.status = "大纲已合稿写入审校稿，请继续校对。".into();
                } else {
                    self.status =
                        "当前已有正文，合稿会覆盖它：请先清空审校稿，或把大纲内容自行取用。".into();
                }
            }
            OutlineAction::Close => {
                if let Some(draft) = self.docs[index].outline.as_mut() {
                    draft.open = false;
                }
            }
        }
    }
}

/// 一节的卡片：标题、要点、状态、正文预览与按钮。
fn section_card(
    ui: &mut egui::Ui,
    position: usize,
    total: usize,
    section: &mut crate::outline::OutlineSection,
    busy: bool,
) -> Option<OutlineAction> {
    let mut action = None;
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("第 {} 节", position + 1));
            match &section.state {
                SectionState::Pending => {
                    theme::chip(ui, "待生成", theme::text_muted(), theme::surface_sunk());
                }
                SectionState::Running => {
                    theme::chip(ui, "生成中", theme::accent(), theme::accent_soft());
                }
                SectionState::Done => {
                    theme::chip(ui, "已生成", theme::success(), theme::success_soft());
                }
                SectionState::Failed(error) => {
                    theme::chip(ui, "失败", theme::danger(), theme::danger_soft());
                    ui.colored_label(theme::danger(), error);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        position + 1 < total,
                        theme::icon_text_button(theme::Icon::ArrowDown, "下移"),
                    )
                    .clicked()
                {
                    action = Some(OutlineAction::MoveDown(position));
                }
                if ui
                    .add_enabled(
                        position > 0,
                        theme::icon_text_button(theme::Icon::ArrowUp, "上移"),
                    )
                    .clicked()
                {
                    action = Some(OutlineAction::MoveUp(position));
                }
            });
        });

        ui.add(
            egui::TextEdit::singleline(&mut section.heading)
                .hint_text("章节标题（不要写「一、」，编号由程序生成）")
                .desired_width(f32::INFINITY),
        );
        ui.add(
            egui::TextEdit::multiline(&mut section.intent)
                .hint_text("这一节要写什么：既决定要不要留它，也是给模型的约束")
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );

        if section.is_done() {
            ui.add_space(4.0);
            egui::CollapsingHeader::new(format!(
                "正文预览（{} 字）",
                section.markdown.chars().count()
            ))
            .id_salt(("outline_preview", position))
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(section.markdown.as_str()).color(theme::text_soft()),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
            });
        }

        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            let can_run = !busy && !section.heading.trim().is_empty();
            let label = if section.is_done() {
                "重新生成本节"
            } else {
                "生成本节"
            };
            if ui
                .add_enabled(
                    can_run,
                    theme::icon_text_button(theme::Icon::WandSparkles, label),
                )
                .on_disabled_hover_text(if busy {
                    "有任务正在运行"
                } else {
                    "请先填写章节标题"
                })
                .clicked()
            {
                action = Some(OutlineAction::Generate(position));
            }
            if ui
                .add(theme::icon_text_button(theme::Icon::FilePlus, "在此后插入"))
                .clicked()
            {
                action = Some(OutlineAction::Insert(position + 1));
            }
            if ui
                .add(theme::icon_text_button(theme::Icon::Trash, "删除本节"))
                .clicked()
            {
                action = Some(OutlineAction::Remove(position));
            }
        });
    });
    action
}
