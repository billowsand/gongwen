//! AI 侧栏的输入框（第 ⑥ 期，`docs/ai-agent-workbench.md` 16.13）。
//!
//! 参照 Claude、Codex 的一体化输入框：圆角大框，文字区在上，底栏在框内——左边「+」与
//! Pi 技能 / 选区 / 引用文章这些标签，右边圆形发送按钮（运行中变停止）。
//!
//! 敲 `/` 在光标处弹出技能列表，敲 `@` 弹出稿件库与知识库文档；继续打字就过滤（名称、id、
//! 拼音首字母），↑↓ 选、回车或 Tab 确认、Esc 关。确认技能后 `/xxx` 从文字里去掉、技能标签
//! 换成它；确认文章后文字里留 `@《标题》` 记号，底栏同时出一个文章标签，发送时解析成引用。
//! 识别与过滤的纯逻辑在 `mention.rs`。

use super::AiPanel;
use super::mention::{self, CatalogItem, PopupKey, Trigger, TriggerKind};
use crate::ai_services::references::{RefSource, Reference};
use crate::draft_page::DraftPage;
use crate::pi_agent::MaterialRole;
use crate::theme;
use eframe::egui;
use egui::text::{CCursor, CCursorRange};

/// 弹出层一屏最多显示几行，多了在层内滚动。
const POPUP_ROWS: usize = 8;
/// `@` 列表每组最多列几篇；再多就请继续打字过滤。
const GROUP_LIMIT: usize = 30;
/// 底栏上可去掉的标签（选区、引用的文章）：与技能标签同一字号，右端一个 ×。
fn removable_pill(label: &str) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(label.to_owned()).small())
        .right_text(theme::Icon::X.image_sized(10.0))
        .image_tint_follows_text_color(true)
        .frame_when_inactive(true)
        .truncate()
        .corner_radius(egui::CornerRadius::same(255))
}

/// 圆形发送按钮的直径。
const SEND_SIZE: f32 = 30.0;

/// 弹出列表里选中一项之后做什么。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pick {
    Skill(String),
    Article(Reference),
}

/// 弹出列表里的一行。
struct Row {
    title: String,
    detail: String,
    /// 分组名：这一行是该组的第一行时在上面画组名。
    group: Option<&'static str>,
    /// 技能现在用不了：照样能选，只是淡一些（发送时会说明原因）。
    dim: bool,
    pick: Pick,
}

/// 底栏与 `+` 菜单里点出来的动作，画完统一执行。
#[derive(Default)]
struct BarActions {
    send: bool,
    stop: bool,
    pick_selection: bool,
    /// 在光标处插入触发符（`+` 菜单里的「引用文章」「选技能」）。
    insert: Option<char>,
    /// 去掉一个引用：按「来源 + 编号」定位，只删这一篇。
    unlink: Option<(RefSource, i64)>,
    remove_material: Option<usize>,
    /// 改某篇粘贴材料 / `@` 引用的用途。
    material_role: Option<(usize, MaterialRole)>,
    ref_role: Option<((String, i64), MaterialRole)>,
}

impl DraftPage<'_> {
    pub(super) fn ai_composer_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        self.commit_prompt_ui(ui);
        self.doc.ai_panel.ensure_pi_skills();
        let ctx = ui.ctx().clone();
        let running = self.doc.ai_panel.running();
        let selection_now = crate::draft_page::editor_selection(&ctx, &self.doc.generated_markdown)
            .filter(|range| !range.is_empty());
        let input_id = egui::Id::new(("ai_panel_input", self.doc.key));
        let popup_id = input_id.with("popup");
        let had_focus_id = input_id.with("had_focus");
        let had_focus = ctx.data(|data| data.get_temp::<bool>(had_focus_id).unwrap_or(false));
        let mut focused = ctx.memory(|memory| memory.has_focus(input_id));
        // 只接管本输入框的 Paste；文件写入失败时把事件留给 TextEdit，原文不会丢失。
        if focused {
            let composer = &mut self.doc.ai_panel.composer;
            ui.input_mut(|input| {
                input.events.retain(|event| {
                    if let egui::Event::Paste(text) = event
                        && super::paste::is_large(text)
                    {
                        // 材料要的是原文：剪贴板里有 HTML 就用它转出来的 Markdown
                        // （网页、Word 的表格不会散成制表符），没有就照旧用纯文本。
                        let material = crate::rich_paste::material_text(text);
                        composer
                            .materials
                            .push(super::paste::PastedFile::new(&material));
                        // 逐篇用途跟着材料一起进，旧会话没有这项时回退到默认值。
                        composer.pi_material_roles.push(composer.pi_material_role);
                        composer.error = None;
                        return false;
                    }
                    true
                });
            });
        }
        // 点弹出层的那一下文本框会失焦；指针在弹出层上时照样当它开着。
        let over_popup = ctx
            .memory(|memory| memory.area_rect(popup_id))
            .is_some_and(|rect| {
                ctx.pointer_hover_pos()
                    .is_some_and(|pos| rect.contains(pos))
            });

        // 先按上一帧的光标看有没有正在输入的 `/`、`@`，把弹出层要的按键在文本框之前吃掉。
        let caret = caret_of(&ctx, input_id, &self.doc.ai_panel.composer.text);
        let popup_open = self.sync_popup(caret);
        if let Some(trigger) = popup_open.clone()
            && (focused || had_focus)
        {
            let rows = self.popup_rows(&trigger);
            let mut picked = None;
            for (key, action) in [
                (egui::Key::ArrowUp, PopupKey::Up),
                (egui::Key::ArrowDown, PopupKey::Down),
                (egui::Key::Enter, PopupKey::Pick),
                (egui::Key::Tab, PopupKey::Pick),
                (egui::Key::Escape, PopupKey::Close),
            ] {
                if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, key))
                    && let Some(index) = self.doc.ai_panel.composer.popup.key(action, rows.len())
                {
                    picked = rows.get(index).map(|row| row.pick.clone());
                }
            }
            if let Some(pick) = picked {
                self.apply_pick(&ctx, input_id, &trigger, caret, pick);
            }
            // egui 在帧首见到 Esc 就让文本框失焦（文本框的按键过滤里没法声明要 Esc）；
            // 这一下是弹出层的，焦点还回去。
            if !focused && self.doc.ai_panel.composer.popup.open.is_none() {
                ctx.memory_mut(|memory| memory.request_focus(input_id));
                focused = true;
            }
        }

        // Ctrl+Enter 发送：同样要在文本框之前吃掉，否则会被当成换行。Enter 留给换行与输入法上屏。
        let mut actions = BarActions {
            send: focused
                && ui.input_mut(|input| {
                    input.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter)
                }),
            ..BarActions::default()
        };
        // 引用记号被手动删掉的，标签跟着去掉。
        {
            let composer = &mut self.doc.ai_panel.composer;
            composer.refs = mention::live_refs(&composer.text, &composer.refs);
        }

        let border = if focused {
            theme::accent()
        } else if theme::smartisan::active() {
            theme::border()
        } else {
            theme::border().gamma_multiply(0.6)
        };
        let framed = egui::Frame::new()
            .fill(super::appearance::composer_fill())
            .stroke(egui::Stroke::new(1.0, border))
            .corner_radius(egui::CornerRadius::same(if theme::smartisan::active() {
                10
            } else {
                14
            }))
            .shadow(egui::epaint::Shadow {
                offset: [0, 4],
                blur: if theme::smartisan::active() { 10 } else { 20 },
                spread: 0,
                color: egui::Color32::from_black_alpha(if theme::smartisan::active() {
                    12
                } else {
                    24
                }),
            })
            .inner_margin(egui::Margin {
                left: 12,
                right: 8,
                top: 8,
                bottom: 6,
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                // TextEdit 的 desired_rows 只是最小高度，长文必须在独立滚动区中绘制。
                let text_height = (ctx.content_rect().height() * 0.25).clamp(60.0, 180.0);
                let output = egui::ScrollArea::vertical()
                    .id_salt(input_id.with("text_scroll"))
                    .max_height(text_height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        egui::TextEdit::multiline(&mut self.doc.ai_panel.composer.text)
                            .id(input_id)
                            .frame(egui::Frame::NONE)
                            .margin(egui::Margin::ZERO)
                            .desired_rows(3)
                            .desired_width(f32::INFINITY)
                            .hint_text("告诉 Pi 要起草、修改或提炼什么…")
                            // 弹出层开着时 Tab 是「确认」，不能让 egui 拿去切焦点。
                            .lock_focus(popup_open.is_some())
                            .show(ui)
                    })
                    .inner;
                ui.add_space(4.0);
                self.composer_bar(ui, running, selection_now.is_some(), &mut actions);
                output
            });
        let input_rect = framed.response.rect;
        let output = framed.inner;

        // 弹出层跟着这一帧的光标：刚打进去的字也算在过滤里。
        let caret = output
            .cursor_range
            .map(|range| range.primary.index.0)
            .unwrap_or_else(|| caret_of(&ctx, input_id, &self.doc.ai_panel.composer.text));
        if let Some(trigger) = self.sync_popup(caret)
            && (focused || over_popup || output.response.has_focus())
        {
            let rows = self.popup_rows(&trigger);
            let highlight = self.doc.ai_panel.composer.popup.highlight;
            if let Some(pick) = popup_ui(&ctx, popup_id, input_rect, &trigger, &rows, highlight) {
                self.apply_pick(&ctx, input_id, &trigger, caret, pick);
            }
        } else {
            ctx.data_mut(|data| data.remove::<PopupSelection>(popup_id.with("selection")));
        }

        let has_focus = output.response.has_focus();
        ctx.data_mut(|data| data.insert_temp(had_focus_id, has_focus));

        let error = self.doc.ai_panel.composer.error.clone();
        if let Some(error) = &error {
            ui.add_space(4.0);
            ui.colored_label(theme::danger(), error);
        }
        // 给阴影留出空间，输入框与侧栏底边之间保持呼吸感。
        ui.add_space(10.0);

        let had_selection = selection_now.is_some();
        if actions.pick_selection
            && let Some(range) = selection_now
        {
            let text = self.doc.generated_markdown[range.clone()].to_string();
            self.doc.ai_panel.composer.selection = Some((range, text));
        }
        if let Some(key) = actions.unlink {
            // 按「来源 + 编号」删：同名不同来源的两篇各算各的，不会因标题相同把两篇一起删掉。
            let composer = &mut self.doc.ai_panel.composer;
            if let Some(index) = composer
                .refs
                .iter()
                .position(|reference| (reference.source, reference.id) == key)
            {
                let reference = composer.refs.remove(index);
                composer.text = mention::unlink(&composer.text, &reference);
                composer.pi_ref_roles.retain(|(saved, _)| {
                    saved != &(reference.source.label().to_string(), reference.id)
                });
            }
        }
        if let Some(index) = actions.remove_material {
            self.doc.ai_panel.composer.materials.remove(index);
            if index < self.doc.ai_panel.composer.pi_material_roles.len() {
                self.doc.ai_panel.composer.pi_material_roles.remove(index);
            }
        }
        if let Some((index, role)) = actions.material_role {
            let composer = &mut self.doc.ai_panel.composer;
            while composer.pi_material_roles.len() <= index {
                composer.pi_material_roles.push(composer.pi_material_role);
            }
            composer.pi_material_roles[index] = role;
        }
        if let Some((key, role)) = actions.ref_role {
            let composer = &mut self.doc.ai_panel.composer;
            if let Some((_, saved)) = composer
                .pi_ref_roles
                .iter_mut()
                .find(|(saved, _)| *saved == key)
            {
                *saved = role;
            } else {
                composer.pi_ref_roles.push((key, role));
            }
        }
        if let Some(symbol) = actions.insert {
            insert_trigger(&ctx, input_id, &mut self.doc.ai_panel.composer.text, symbol);
        }
        if actions.stop {
            self.stop_ai_task();
            actions.send = false;
        }
        if actions.send && !self.doc.ai_panel.running() {
            self.send_ai_panel(had_selection);
        }
    }

    /// 输入框底栏：左边「+」与各个标签，右边发送 / 停止。
    fn composer_bar(
        &mut self,
        ui: &mut egui::Ui,
        running: bool,
        can_pick_selection: bool,
        actions: &mut BarActions,
    ) {
        let has_proposal = self.doc.ai_proposal.is_some();
        let catalog = self.doc.ai_panel.pi_skills.clone().unwrap_or_default();
        let AiPanel { composer, .. } = &mut self.doc.ai_panel;
        ui.horizontal_top(|ui| {
            let spacing = ui.spacing().item_spacing.x;
            let left_width = (ui.available_width() - SEND_SIZE - spacing).max(80.0);
            ui.allocate_ui_with_layout(
                egui::vec2(left_width, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt(("ai_composer_chips", self.doc.key))
                        .max_height(if theme::smartisan::active() { 104.0 } else { 76.0 })
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            let plus = ui
                                .add(
                                    egui::Button::image(theme::Icon::Plus.image_sized(14.0))
                                        .image_tint_follows_text_color(true)
                                        .corner_radius(egui::CornerRadius::same(255))
                                        .frame_when_inactive(false),
                                )
                                .on_hover_text("引用文章、选技能、用当前选区");
                            egui::Popup::menu(&plus).show(|ui| {
                                if ui
                                    .add(theme::menu_item(theme::Icon::Quote, "引用文章（@）"))
                                    .clicked()
                                {
                                    actions.insert = Some('@');
                                }
                                if ui
                                    .add(theme::menu_item(theme::Icon::WandSparkles, "选技能（/）"))
                                    .clicked()
                                {
                                    actions.insert = Some('/');
                                }
                                if ui
                                    .add_enabled(
                                        can_pick_selection,
                                        theme::menu_item(theme::Icon::Edit, "用当前选区"),
                                    )
                                    .on_disabled_hover_text("先在编辑器里选中一段")
                                    .clicked()
                                {
                                    actions.pick_selection = true;
                                }
                                ui.separator();
                                ui.label("新粘贴材料用途");
                                for role in MaterialRole::ALL {
                                    if ui.add(theme::menu_selectable_item(composer.pi_material_role == role, role.label())).clicked() {
                                        composer.pi_material_role = role;
                                    }
                                }
                                ui.separator();
                                ui.label("新 @ 引用用途");
                                for role in MaterialRole::ALL {
                                    if ui.add(theme::menu_selectable_item(composer.pi_ref_role == role, role.label())).clicked() {
                                        composer.pi_ref_role = role;
                                    }
                                }
                                ui.separator();
                                ui.label("写作策略");
                                for preference in crate::pi_agent::PiStrategyPreference::ALL {
                                    if ui.add(theme::menu_selectable_item(composer.pi_strategy == preference, preference.label())).clicked() {
                                        composer.pi_strategy = preference;
                                    }
                                }
                                ui.weak("自动按实际窗口与任务规模选常规或分阶段；倾向分阶段只对长任务生效。");
                                ui.weak("事实材料可作为依据；样稿与范文只参考写法。知识库由 Pi 按需检索。");
                            });

                            let label = composer.pi_skill.as_deref().map_or_else(
                                || "Pi · 自动".to_string(),
                                |name| {
                                    let suffix = catalog.get(name).map_or(
                                        "（不存在）",
                                        |entry| {
                                            if entry.loadable() {
                                                ""
                                            } else {
                                                "（不可用）"
                                            }
                                        },
                                    );
                                    format!("Pi · {}{suffix}", catalog.label_for(name))
                                },
                            );
                            let skill = ui.add(pill(&label, composer.pi_skill.is_some()))
                                .on_hover_text("发送只使用 Pi；自动按任务选择技能，知识库由 Pi 按需检索");
                            egui::Popup::menu(&skill).show(|ui| {
                                if ui.add(theme::menu_selectable_item(composer.pi_skill.is_none(), "自动")).clicked() {
                                    composer.pi_skill = None;
                                }
                                for entry in catalog.writing_enabled() {
                                    if ui.add(theme::menu_selectable_item(composer.pi_skill.as_deref() == Some(entry.name.as_str()), &entry.label)).on_hover_text(&entry.description).clicked() {
                                        composer.pi_skill = Some(entry.name.clone());
                                    }
                                }
                                let enabled = catalog.writing_enabled();
                                if enabled.is_empty() {
                                    ui.weak("没有启用的 Pi 技能，先到「AI 管理 → 技能」里启用。");
                                }
                            });

                            if has_proposal && composer.selection.is_none() {
                                // 「改提案」（16.15 B.7）：开着时「再短一点」改的是刚交的提案。
                                let toggle = ui
                                    .add(
                                        egui::Button::image_and_text(
                                            theme::Icon::Edit.image_sized(12.0),
                                            egui::RichText::new("改提案").small(),
                                        )
                                        .image_tint_follows_text_color(true)
                                        .selected(!composer.skip_proposal)
                                        .corner_radius(egui::CornerRadius::same(255)),
                                    )
                                    .on_hover_text(if composer.skip_proposal {
                                        "现在改的是正文；点一下改为接着改待确认的提案"
                                    } else {
                                        "接着改待确认的提案（如「再短一点」「第二条展开说」），新提案取代旧的；点一下改为改正文"
                                    });
                                if toggle.clicked() {
                                    composer.skip_proposal = !composer.skip_proposal;
                                }
                            }

                            if let Some((_, text)) = &composer.selection {
                                if ui
                                    .add(removable_pill(&format!("选区 {} 字", text.chars().count())))
                                    .on_hover_text("只改这段，其余逐字保持不变；点 × 改为全文")
                                    .clicked()
                                {
                                    composer.selection = None;
                                }
                            } else {
                                if can_pick_selection {
                                    if ui
                                        .small_button("用当前选区")
                                        .on_hover_text("只改编辑器里选中的那段")
                                        .clicked()
                                    {
                                        actions.pick_selection = true;
                                    }
                                } else {
                                    ui.label(
                                        egui::RichText::new("全文")
                                            .small()
                                            .color(theme::text_muted()),
                                    );
                                }
                            }

                            for reference in &composer.refs {
                                if ui
                                    .add(removable_pill(&format!("《{}》", reference.title)))
                                    .on_hover_text(format!(
                                        "引用{}的这篇；点 × 取消引用",
                                        reference.source.label()
                                    ))
                                    .clicked()
                                {
                                    actions.unlink = Some((reference.source, reference.id));
                                }
                                let role = composer.ref_role(reference.source, reference.id);
                                let key = (reference.source.label().to_string(), reference.id);
                                egui::Popup::menu(&ui.add(pill(role.label(), role != crate::pi_agent::MaterialRole::Fact)))
                                    .show(|ui| {
                                        for role in MaterialRole::ALL {
                                            if ui.add(theme::menu_selectable_item(role == composer.ref_role(reference.source, reference.id), role.label())).clicked() {
                                                actions.ref_role = Some((key.clone(), role));
                                            }
                                        }
                                        ui.weak("按来源与编号单独记，同名不同来源不会串用。");
                                    });
                            }
                            for (index, material) in composer.materials.iter().enumerate() {
                                if ui
                                    .add(removable_pill(&format!(
                                        "粘贴材料 · {} 字",
                                        material.chars
                                    )))
                                    .on_hover_text(format!(
                                        "{}\n全文作为材料发送；点 × 取消引用",
                                        material.material.name
                                    ))
                                    .clicked()
                                {
                                    actions.remove_material = Some(index);
                                }
                                let role = composer.material_role(index);
                                egui::Popup::menu(&ui.add(pill(role.label(), role != crate::pi_agent::MaterialRole::Fact)))
                                    .show(|ui| {
                                        for candidate in MaterialRole::ALL {
                                            if ui.add(theme::menu_selectable_item(candidate == composer.material_role(index), candidate.label())).clicked() {
                                                actions.material_role = Some((index, candidate));
                                            }
                                        }
                                    });
                            }
                        });
                        });
                },
            );
            if send_button(ui, running).clicked() {
                if running {
                    actions.stop = true;
                } else {
                    actions.send = true;
                }
            }
        });
    }

    /// 按光标位置更新弹出层状态，返回此刻开着的触发。`@` 刚弹出时读一次文章列表。
    fn sync_popup(&mut self, caret: usize) -> Option<Trigger> {
        let composer = &mut self.doc.ai_panel.composer;
        let trigger = mention::detect(&composer.text, caret);
        let open = composer.popup.sync(trigger).cloned()?;
        if open.kind == TriggerKind::Article && composer.popup.catalog.is_none() {
            let catalog = self.load_catalog();
            self.doc.ai_panel.composer.popup.catalog = Some(catalog);
        }
        Some(open)
    }

    /// 弹出列表：技能按名称、id、拼音首字母过滤；文章按标题过滤，稿件库在前、知识库在后。
    fn popup_rows(&self, trigger: &Trigger) -> Vec<Row> {
        let composer = &self.doc.ai_panel.composer;
        match trigger.kind {
            TriggerKind::Skill => self
                .doc
                .ai_panel
                .pi_skills
                .as_ref()
                .map(|catalog| pi_skill_rows(&trigger.query, catalog))
                .unwrap_or_default(),
            TriggerKind::Article => {
                let catalog = composer.popup.catalog.as_deref().unwrap_or_default();
                let order =
                    mention::rank(&trigger.query, catalog, |item| (&item.reference.title, ""));
                let mut rows = Vec::new();
                for source in [RefSource::Manuscript, RefSource::Knowledge] {
                    let mut first = true;
                    for item in order
                        .iter()
                        .map(|index| &catalog[*index])
                        .filter(|item| item.reference.source == source)
                        .take(GROUP_LIMIT)
                    {
                        rows.push(Row {
                            title: item.reference.title.clone(),
                            detail: [item.kind.label(), item.date.as_str()]
                                .into_iter()
                                .filter(|part| !part.is_empty())
                                .collect::<Vec<_>>()
                                .join(" · "),
                            group: first.then_some(source.label()),
                            dim: false,
                            pick: Pick::Article(item.reference.clone()),
                        });
                        first = false;
                    }
                }
                rows
            }
        }
    }

    /// 稿件库（不含当前这篇）与知识库的文档列表。读不出来就当没有，不打断输入。
    fn load_catalog(&mut self) -> Vec<CatalogItem> {
        let current = self.doc.manuscript_id;
        let filter = crate::manuscript::ManuscriptFilter::default();
        let manuscripts = match self.store.as_deref_mut() {
            Some(store) => store.list(&filter).ok(),
            None => crate::storage::manuscript_db_path()
                .ok()
                .and_then(|path| crate::manuscript::ManuscriptStore::open(&path).ok())
                .and_then(|mut store| store.list(&filter).ok()),
        }
        .unwrap_or_default();
        let mut items: Vec<CatalogItem> = manuscripts
            .into_iter()
            .filter(|row| Some(row.id) != current && !row.title.trim().is_empty())
            .map(|row| CatalogItem {
                date: if row.doc_date.trim().is_empty() {
                    day_of(&row.updated_at)
                } else {
                    row.doc_date
                },
                kind: row.kind,
                reference: Reference {
                    source: RefSource::Manuscript,
                    id: row.id,
                    title: row.title,
                },
            })
            .collect();
        let knowledge = crate::storage::manuscript_db_path()
            .ok()
            .and_then(|path| crate::knowledge::KnowledgeStore::open(&path).ok())
            .and_then(|mut store| store.list_docs(None).ok())
            .unwrap_or_default();
        items.extend(knowledge.into_iter().map(|row| CatalogItem {
            date: day_of(&row.updated_at),
            kind: row.kind,
            reference: Reference {
                source: RefSource::Knowledge,
                id: row.id,
                title: row.title,
            },
        }));
        items
    }

    /// 选中弹出列表里的一项：换掉 `/xxx` 或 `@xxx`，光标放到插入内容之后。
    fn apply_pick(
        &mut self,
        ctx: &egui::Context,
        input_id: egui::Id,
        trigger: &Trigger,
        caret: usize,
        pick: Pick,
    ) {
        let composer = &mut self.doc.ai_panel.composer;
        let replacement = match pick {
            Pick::Skill(id) => {
                composer.pi_skill = Some(id);
                String::new()
            }
            Pick::Article(mut reference) => {
                // 同名不同来源：显示名必须唯一，否则记号分不清是哪一篇；同名同源再选一次
                // 沿用已有的显示名（不发散成第二个名字）。
                reference.title = match composer
                    .refs
                    .iter()
                    .find(|r| r.source == reference.source && r.id == reference.id)
                {
                    Some(existing) => existing.title.clone(),
                    None => mention::unique_title(&reference, &composer.refs),
                };
                let mark = mention::mark(&reference.title);
                if !composer.refs.contains(&reference) {
                    composer.refs.push(reference);
                }
                mark
            }
        };
        let (text, caret) = mention::replace_trigger(&composer.text, trigger, caret, &replacement);
        composer.text = text;
        composer.error = None;
        set_caret(ctx, input_id, caret);
    }

    /// 按输入区的内容发起 Pi 任务。`/compact` 提示由 Pi 管理上下文；「把工作稿提交到
    /// 正文」这类话（`commit_intent`）也不发给模型——正文只能由用户合并（红线 1），改为弹出
    /// 「写入正文？」确认卡，点「接受」与结果卡「采用」走同一个 `accept_ai_proposal`。
    pub(super) fn send_ai_panel(&mut self, editor_has_selection: bool) {
        if self.doc.ai_panel.composer.text.trim() == "/compact" {
            self.doc.ai_panel.composer.text.clear();
            self.doc.ai_panel.composer.error =
                Some("Pi 由运行时管理上下文，不使用旧会话压缩。".into());
            return;
        }
        if super::commit_intent::is_commit_request(&self.doc.ai_panel.composer.text) {
            let reason = self.commit_unavailable();
            let composer = &mut self.doc.ai_panel.composer;
            composer.text.clear();
            composer.error = reason;
            composer.commit_prompt = composer.error.is_none();
            return;
        }
        match self.start_pi_job(editor_has_selection) {
            Ok(()) => {
                let composer = &mut self.doc.ai_panel.composer;
                composer.text.clear();
                composer.error = None;
                composer.refs.clear();
                composer.materials.clear();
                composer.pi_material_roles.clear();
                composer.pi_ref_roles.clear();
                composer.selection = None;
            }
            Err(message) => self.doc.ai_panel.composer.error = Some(message),
        }
    }

    /// 现在为什么没有可写入正文的提案；有就是 None。
    fn commit_unavailable(&self) -> Option<String> {
        if self.doc.ai_proposal.is_some() {
            return None;
        }
        let panel = &self.doc.ai_panel;
        Some(if panel.running() {
            "AI 还在写，跑完会交成提案，到时再写入正文。".into()
        } else {
            match panel.turns.iter().rev().map(|turn| &turn.state).find(|state| {
                matches!(
                    state,
                    super::TurnState::Accepted | super::TurnState::Expired | super::TurnState::Discarded
                )
            }) {
                Some(super::TurnState::Accepted) => "最近一份提案已经写入正文了。".into(),
                Some(super::TurnState::Expired) => {
                    "提案交出之后正文改过了，不能再写入（会盖掉你的改动）；可以让 AI 按当前正文重做。".into()
                }
                _ => "现在没有待写入的 AI 提案：AI 工作稿要等任务跑完交成提案，才能写入正文。".into(),
            }
        })
    }

    /// 「写入正文？」确认卡（`commit_intent` 认出来之后）：提案摘要 + 接受 / 打开审阅 / 算了。
    /// 有没核对的关键事实变化时不给「接受」，只给「打开审阅核对」——核对只在审阅里做。
    fn commit_prompt_ui(&mut self, ui: &mut egui::Ui) {
        if !self.doc.ai_panel.composer.commit_prompt {
            return;
        }
        let Some(proposal) = self.doc.ai_proposal.as_ref() else {
            // 提案在这期间被接受、放弃或被新任务取代了。
            self.doc.ai_panel.composer.commit_prompt = false;
            return;
        };
        let changes =
            crate::diff::body_diff(&proposal.before, &proposal.result.markdown).changed_count;
        let facts = proposal.fact_changes.len();
        let needs_review = facts > 0 && !proposal.fact_changes_confirmed;
        let mut summary = format!(
            "「{}」提案：{} 字 · 正文变化 {changes} 处 · 关键事实变化 {facts} 项",
            proposal.label,
            proposal.result.markdown.chars().count()
        );
        if !proposal.excluded.is_empty() {
            summary.push_str(&format!(
                "（已排除 {} 处，只写入其余改动）",
                proposal.excluded.len()
            ));
        }
        #[derive(PartialEq)]
        enum Choice {
            Accept,
            Review,
            Cancel,
        }
        let mut choice = None;
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.strong("写入正文？");
            ui.label(egui::RichText::new(summary).small());
            if needs_review {
                ui.label(
                    egui::RichText::new("有关键事实变化还没核对，请在审阅里逐项核对后再接受。")
                        .small()
                        .color(theme::danger()),
                );
            }
            ui.horizontal_wrapped(|ui| {
                if needs_review {
                    if theme::primary_icon_button(ui, theme::Icon::Compare, "打开审阅核对")
                        .clicked()
                    {
                        choice = Some(Choice::Review);
                    }
                } else {
                    if theme::primary_icon_button(ui, theme::Icon::SquareCheck, "接受提案")
                        .clicked()
                    {
                        choice = Some(Choice::Accept);
                    }
                    if ui
                        .add(theme::secondary_icon_button(
                            theme::Icon::Compare,
                            "打开审阅",
                        ))
                        .clicked()
                    {
                        choice = Some(Choice::Review);
                    }
                }
                if ui.button("算了").clicked() {
                    choice = Some(Choice::Cancel);
                }
            });
        });
        ui.add_space(4.0);
        let Some(choice) = choice else {
            return;
        };
        self.doc.ai_panel.composer.commit_prompt = false;
        match choice {
            // 与结果卡「采用」同一个入口：事实核对、合并后重过闸门都在里面（红线 1、3）。
            Choice::Accept => {
                crate::app::GongwenApp::accept_ai_proposal(self.doc, self.config, self.status);
            }
            Choice::Review => {
                if let Some(proposal) = self.doc.ai_proposal.as_mut() {
                    proposal.open = true;
                }
            }
            Choice::Cancel => {}
        }
    }

    /// 旧流程（小模型分阶段）的任务入口：**仅供已有自动测试**，生产代码不再调用。
    ///
    /// 统一写作第五阶段开始，侧栏新任务一律走 Pi（`start_pi_job`），旧卡片的「重新生成 /
    /// 重试 / 从头重来」也改成了「复制为新 Pi 任务」（`copy_turn_to_pi`）。这里留着是因为
    /// 旧模型联机用例仍以旧后端为基准；旧入口整体退役时随 `ai_panel/skill_job.rs` 一并删。
    #[cfg(test)]
    pub(super) fn start_panel_request(&mut self, request: super::TurnRequest) -> bool {
        match self.start_skill(request, None) {
            Ok(()) => true,
            Err(message) => {
                self.doc.ai_panel.composer.error = Some(message);
                false
            }
        }
    }
}

/// 「2025-11-02 10:20:00」→「2025-11-02」。
fn day_of(stamp: &str) -> String {
    stamp.trim().chars().take(10).collect()
}

/// 上一帧存下的光标位置（字符）；文本框还没画过时当作在末尾。
fn caret_of(ctx: &egui::Context, id: egui::Id, text: &str) -> usize {
    egui::TextEdit::load_state(ctx, id)
        .and_then(|state| state.cursor.char_range())
        .map(|range| range.primary.index.0)
        .unwrap_or_else(|| text.chars().count())
}

fn set_caret(ctx: &egui::Context, id: egui::Id, caret: usize) {
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(CCursorRange::one(CCursor::new(caret))));
    state.store(ctx, id);
    ctx.memory_mut(|memory| memory.request_focus(id));
}

/// 在光标处插入触发符：`/` 前面不是空白时先补一个空格（`/` 只认行首或空白之后）。
fn insert_trigger(ctx: &egui::Context, id: egui::Id, text: &mut String, symbol: char) {
    let caret = caret_of(ctx, id, text);
    let mut chars: Vec<char> = text.chars().collect();
    let caret = caret.min(chars.len());
    let mut insert = String::new();
    let before = caret.checked_sub(1).map(|i| chars[i]);
    let needs_space = match symbol {
        '/' => before.is_some_and(|c| !c.is_whitespace()),
        _ => before.is_some_and(|c| c.is_ascii_alphanumeric()),
    };
    if needs_space {
        insert.push(' ');
    }
    insert.push(symbol);
    let count = insert.chars().count();
    chars.splice(caret..caret, insert.chars());
    *text = chars.into_iter().collect();
    set_caret(ctx, id, caret + count);
}

/// 底栏上的圆角标签按钮（带下拉箭头）。`active` 表示用户指定过，常显淡底。
fn pill(label: &str, active: bool) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(label.to_owned()).small())
        .right_text(theme::Icon::ChevronDown.image_sized(10.0))
        .image_tint_follows_text_color(true)
        .selected(active)
        .frame_when_inactive(true)
        .corner_radius(egui::CornerRadius::same(255))
}

/// 圆形发送按钮：主色底自适应箭头；运行中换成停止方块。
fn send_button(ui: &mut egui::Ui, running: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(SEND_SIZE, SEND_SIZE), egui::Sense::click());
    let fill = if running {
        theme::text_soft()
    } else if response.is_pointer_button_down_on() {
        theme::accent_active()
    } else if response.hovered() {
        theme::accent_hover()
    } else {
        theme::accent()
    };
    ui.painter()
        .circle_filled(rect.center(), SEND_SIZE / 2.0, fill);
    let icon = if running {
        theme::Icon::Square
    } else {
        theme::Icon::ArrowUp
    };
    icon.image_sized(16.0).tint(theme::accent_text()).paint_at(
        ui,
        egui::Rect::from_center_size(rect.center(), egui::vec2(16.0, 16.0)),
    );
    let tip = if running {
        "停止".to_string()
    } else {
        format!("发送给 Pi（{}）", theme::primary_shortcut("Enter"))
    };
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tip)
}

/// 只在筛选或键盘选中项改变时跟随滚动，不能每帧把鼠标滚动拉回选中行。
type PopupSelection = (Trigger, Option<Pick>, usize);

/// 输入框上沿的弹出列表；空间不足时收缩滚动区，下面更宽裕时改为向下弹。
/// 点了某一行时返回它。
fn popup_ui(
    ctx: &egui::Context,
    id: egui::Id,
    input_rect: egui::Rect,
    trigger: &Trigger,
    rows: &[Row],
    highlight: usize,
) -> Option<Pick> {
    let screen = ctx.content_rect().shrink(8.0);
    // 宽度含边框与内边距，随侧栏收缩，不再跟着 @ 在文字里的横坐标移动。
    let width = input_rect.width().min(screen.width()).max(1.0);
    let x = input_rect
        .left()
        .min(screen.right() - width)
        .max(screen.left());
    let style = ctx.global_style();
    let (body_height, small_height) = ctx.fonts_mut(|fonts| {
        (
            fonts.row_height(&egui::TextStyle::Body.resolve(&style)),
            fonts.row_height(&egui::TextStyle::Small.resolve(&style)),
        )
    });
    let narrow = width < 360.0;
    let header_height = 22.0
        + body_height
        + 12.0
        + if narrow {
            small_height + style.spacing.item_spacing.y
        } else {
            0.0
        };
    let row_height = body_height + small_height + style.spacing.item_spacing.y + 12.0;
    let wanted_height = header_height + rows.len().clamp(1, POPUP_ROWS) as f32 * row_height;
    let above = (input_rect.top() - 8.0 - screen.top()).max(0.0);
    let below = (screen.bottom() - input_rect.bottom() - 8.0).max(0.0);
    let up = above >= wanted_height || above >= below;
    let available_height = if up { above } else { below };
    let (pos, pivot) = if up {
        (
            egui::pos2(x, input_rect.top() - 8.0),
            egui::Align2::LEFT_BOTTOM,
        )
    } else {
        (
            egui::pos2(x, input_rect.bottom() + 8.0),
            egui::Align2::LEFT_TOP,
        )
    };
    let selection = (
        trigger.clone(),
        rows.get(highlight).map(|row| row.pick.clone()),
        rows.len(),
    );
    let follow_selection = ctx.data_mut(|data| {
        let key = id.with("selection");
        let changed = data.get_temp::<PopupSelection>(key).as_ref() != Some(&selection);
        data.insert_temp(key, selection);
        changed
    });
    let mut clicked = None;
    egui::Area::new(id)
        .order(egui::Order::Foreground)
        .movable(false)
        .constrain_to(screen)
        .default_size(egui::vec2(width, available_height.min(wanted_height)))
        .fixed_pos(pos)
        .pivot(pivot)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::same(10))
                .corner_radius(egui::CornerRadius::same(10))
                .show(ui, |ui| {
                    ui.set_width((width - 22.0).max(1.0));
                    let header_top = ui.cursor().top();
                    let title = |ui: &mut egui::Ui| {
                        ui.label(
                            egui::RichText::new(match trigger.kind {
                                TriggerKind::Skill => "技能",
                                TriggerKind::Article => "引用文章",
                            })
                            .strong()
                            .color(theme::text_soft()),
                        );
                    };
                    let hint = |ui: &mut egui::Ui| {
                        ui.label(
                            egui::RichText::new("↑↓ 选择 · 回车确认 · Esc 关闭")
                                .small()
                                .color(theme::text_muted()),
                        );
                    };
                    if narrow {
                        title(ui);
                        hint(ui);
                    } else {
                        ui.horizontal(|ui| {
                            title(ui);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    hint(ui);
                                },
                            );
                        });
                    }
                    ui.separator();
                    // 标题实际高度包含横排的最小行高、分隔线与间距，不能只估算字号。
                    let actual_header_height = ui.cursor().top() - header_top + 22.0;
                    let scroll_height = (available_height - actual_header_height)
                        .max(1.0)
                        .min(POPUP_ROWS as f32 * row_height);
                    if rows.is_empty() {
                        ui.weak(match trigger.kind {
                            TriggerKind::Skill => "没有匹配的技能",
                            TriggerKind::Article => "稿件库与知识库里没有匹配的文章",
                        });
                        return;
                    }
                    theme::popup_scroll(scroll_height)
                        .id_salt("candidates")
                        .show(ui, |ui| {
                            for (index, row) in rows.iter().enumerate() {
                                if let Some(group) = row.group {
                                    ui.add_space(2.0);
                                    ui.label(
                                        egui::RichText::new(group)
                                            .small()
                                            .color(theme::text_muted()),
                                    );
                                }
                                // 标题可能重名，也可能筛选后换位置；按来源与真实 ID 隔离整行的控件。
                                let row_id = match &row.pick {
                                    Pick::Skill(id) => ui.id().with(("skill", id)),
                                    Pick::Article(reference) => ui.id().with((
                                        "article",
                                        reference.source.label(),
                                        reference.id,
                                    )),
                                };
                                let response = ui
                                    .push_id(row_id, |ui| popup_row(ui, row, index == highlight))
                                    .inner;
                                if index == highlight && follow_selection {
                                    response.scroll_to_me(None);
                                }
                                if response.clicked() {
                                    clicked = Some(row.pick.clone());
                                }
                            }
                        });
                });
        });
    clicked
}

/// 弹出列表的一行：标题在上，说明（技能的一句话、文章的文种与日期）在下，整行可点。
fn popup_row(ui: &mut egui::Ui, row: &Row, highlighted: bool) -> egui::Response {
    let width = ui.available_width();
    let title_color = if row.dim {
        theme::text_muted()
    } else {
        theme::text()
    };
    let mut frame = egui::Frame::new()
        .fill(if highlighted {
            theme::accent_soft()
        } else {
            egui::Color32::TRANSPARENT
        })
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(8, 6))
        .begin(ui);
    {
        let ui = &mut frame.content_ui;
        ui.set_width((width - 16.0).max(1.0));
        ui.add(
            egui::Label::new(egui::RichText::new(&row.title).color(title_color))
                .truncate()
                .selectable(false),
        );
        if !row.detail.is_empty() {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(&row.detail)
                        .small()
                        .color(theme::text_muted()),
                )
                .truncate()
                .selectable(false),
            );
        }
    }
    let rect = frame.allocate_space(ui).rect;
    let response = ui
        .interact(rect, ui.id().with("select"), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.hovered() && !highlighted {
        frame.frame.fill = theme::surface_hover();
    }
    // Frame::begin 预留的底色位置在文字之前；交互后补画也不会盖住文字。
    frame.paint(ui);
    response
}

/// 只列出 Pi 标准技能，不读取旧流程技能目录。
fn pi_skill_rows(query: &str, catalog: &crate::pi_agent::pi_skills::Catalog) -> Vec<Row> {
    let skills = catalog.writing_enabled();
    mention::rank(query, &skills, |entry| {
        (entry.label.as_str(), entry.name.as_str())
    })
    .into_iter()
    .map(|index| {
        let entry = skills[index];
        Row {
            title: entry.label.clone(),
            detail: entry.description.clone(),
            group: None,
            dim: false,
            pick: Pick::Skill(entry.name.clone()),
        }
    })
    .collect()
}
