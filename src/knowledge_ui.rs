//! 知识库页：文档列表（筛选、排序、批量删除、改标题）、导入（外部文档 / 库内稿件）、
//! 建立与重建索引、检索与问答。导入只存正文，索引另点按钮建立，两步分开。从 app.rs 拆出以控制体积；通过 `GongwenApp` 上的 `pub(crate)` 字段
//! 与方法读写状态。

use crate::app::widgets::{centered_cell_text, centered_header, short_date};
use crate::app::{GongwenApp, KnowledgeMode};
use crate::knowledge::KnowledgeDocRow;
use crate::modal::{self, Dismiss};
use crate::models::TemplateKind;
use crate::qa;
use crate::theme;
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use std::collections::HashSet;

/// 知识库页入口。
pub(crate) fn knowledge_ui(app: &mut GongwenApp, ui: &mut egui::Ui) {
    // 库都打不开时只报错，不显示"点右上角导入"的空状态——那会引导用户去点
    // 一个必然失败的按钮。与稿件管理页的处置一致。
    if app.knowledge_store.is_none() {
        ui.colored_label(
            theme::warn(),
            app.knowledge_error
                .clone()
                .unwrap_or_else(|| "知识库不可用。".to_string()),
        );
        return;
    }
    if app.knowledge_dirty {
        app.refresh_knowledge();
    }
    toolbar(app, ui);
    ui.add_space(6.0);
    kind_filter_bar(app, ui);
    ui.add_space(6.0);
    index_status(app, ui);

    if let Some(error) = app.knowledge_error.clone() {
        ui.horizontal(|ui| {
            ui.colored_label(theme::warn(), error);
            if ui.small_button("知道了").clicked() {
                app.knowledge_error = None;
            }
        });
        ui.add_space(4.0);
    }
    // 换过 embedding 模型后旧文档会静默退出向量召回，必须显式提醒重建。
    if let Some(hint) = app.knowledge_embed_model_mismatch() {
        ui.colored_label(theme::warn(), hint);
        ui.add_space(4.0);
    }

    // 文档与检索问答分两个子页签，各占满整页高度：放在一页里时列表只分到
    // 下半截，文档一多就只能在一条窄缝里滚。
    page_tabs(app, ui);
    ui.add_space(6.0);
    match app.knowledge_list.tab {
        KnowledgeTab::Docs => doc_tab(app, ui),
        KnowledgeTab::Search => search_panel(app, ui),
    }
    import_dialog(app, ui);
    delete_confirm(app, ui);
    rename_dialog(app, ui);
}

/// 知识库页的子页签。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum KnowledgeTab {
    #[default]
    Docs,
    Search,
}

/// 文档列表的排序方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DocSort {
    #[default]
    UpdatedDesc,
    Title,
    ChunksDesc,
}

impl DocSort {
    const ALL: [Self; 3] = [Self::UpdatedDesc, Self::Title, Self::ChunksDesc];

    fn label(self) -> &'static str {
        match self {
            Self::UpdatedDesc => "最近更新",
            Self::Title => "标题",
            Self::ChunksDesc => "块数最多",
        }
    }
}

/// 知识库页的界面状态：子页签、文档列表的筛选 / 排序 / 勾选、改标题框。
#[derive(Debug, Default)]
pub(crate) struct KnowledgeListState {
    pub(crate) tab: KnowledgeTab,
    /// 标题 / 来源路径筛选词，只在内存里过滤列表，不走检索。
    pub(crate) filter: String,
    pub(crate) sort: DocSort,
    pub(crate) only_unindexed: bool,
    /// 勾选的文档 id。批量操作只作用于其中当前列出的那些。
    pub(crate) selected: HashSet<i64>,
    /// 正在改标题的文档：(id, 编辑中的标题)。
    pub(crate) rename: Option<(i64, String)>,
}

fn page_tabs(app: &mut GongwenApp, ui: &mut egui::Ui) {
    let count = app.knowledge_docs.len();
    ui.horizontal(|ui| {
        let tab = &mut app.knowledge_list.tab;
        ui.selectable_value(tab, KnowledgeTab::Docs, format!("文档（{count}）"));
        ui.selectable_value(tab, KnowledgeTab::Search, "检索 / 问答");
    });
}

fn toolbar(app: &mut GongwenApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.heading("知识库");
        ui.add_space(8.0);
        ui.weak(format!(
            "{} 篇 · {} 块",
            app.knowledge_docs.len(),
            app.knowledge_chunk_count
        ));
        if let Some(elapsed) = app.knowledge_retrieval_elapsed {
            ui.weak(format!("· 最近一次检索 {:.1} 秒", elapsed.as_secs_f64()));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let busy = app.knowledge_busy;
            if ui
                .add_enabled(
                    !busy,
                    theme::icon_text_button(theme::Icon::FileUp, "导入文档"),
                )
                .on_hover_text(
                    "选择本机公文加入知识库：Markdown、Word、WPS 另存的 docx、PDF（电子版）等；扫描版 PDF 无法导入",
                )
                .clicked()
            {
                app.knowledge_pick_documents();
            }
            // 右到左布局，先加的靠右：「建立索引」紧挨「导入文档」，顺着操作次序。
            let pending = app.knowledge_unindexed;
            let label = if pending > 0 {
                format!("建立索引（{pending}）")
            } else {
                "建立索引".to_string()
            };
            if ui
                .add_enabled(
                    !busy && pending > 0,
                    theme::icon_text_button(theme::Icon::Sparkles, &label),
                )
                .on_hover_text("为已导入、还没建立索引的文档切块、嵌入；建好索引才能被检索与问答用到")
                .clicked()
            {
                app.knowledge_build_index();
            }
            if ui
                .add_enabled(
                    !busy && !app.knowledge_docs.is_empty(),
                    theme::icon_text_button(theme::Icon::Refresh, "重建索引"),
                )
                .on_hover_text("对库内全部文档重新切块、嵌入（更换 embedding 模型后用）")
                .clicked()
            {
                app.knowledge_rebuild_all();
            }
        });
    });
}

fn kind_filter_bar(app: &mut GongwenApp, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.weak("文种：");
        let mut chosen: Option<Option<TemplateKind>> = None;
        let current = app.knowledge_filter_kind;
        if ui.selectable_label(current.is_none(), "全部").clicked() {
            chosen = Some(None);
        }
        for kind in TemplateKind::ALL {
            if ui
                .selectable_label(current == Some(kind), kind.label())
                .clicked()
            {
                chosen = Some(Some(kind));
            }
        }
        if let Some(kind) = chosen {
            app.knowledge_filter_kind = kind;
            app.knowledge_dirty = true;
        }
    });
}

fn index_status(app: &mut GongwenApp, ui: &mut egui::Ui) {
    if let Some((done, total, title)) = &app.knowledge_index_progress {
        ui.horizontal(|ui| {
            theme::spinner(ui, 14.0, theme::accent());
            let text = if title.is_empty() {
                format!("{}… {done}/{total}", app.knowledge_progress_verb)
            } else {
                format!(
                    "{}… {done}/{total}　《{title}》",
                    app.knowledge_progress_verb
                )
            };
            ui.label(text);
        });
        let fraction = if *total == 0 {
            0.0
        } else {
            *done as f32 / *total as f32
        };
        ui.add(egui::ProgressBar::new(fraction).desired_width(ui.available_width()));
    } else if let Some(result) = app.knowledge_index_result.clone() {
        // 结果摘要可关闭，否则会一直挂在页面上。
        ui.horizontal(|ui| {
            ui.weak(result);
            if ui.small_button("知道了").clicked() {
                app.knowledge_index_result = None;
            }
        });
    }
}

fn doc_tab(app: &mut GongwenApp, ui: &mut egui::Ui) {
    if app.knowledge_docs.is_empty() {
        theme::card().show(ui, |ui| {
            ui.weak("知识库还是空的。点右上角「导入文档」选本机公文（Word、PDF 电子版、Markdown 等），或到「稿件管理」勾选稿件后用工具栏的「导入到知识库」；导入后再点「建立索引」，就能检索与问答了。");
        });
        return;
    }
    let visible = visible_docs(&app.knowledge_docs, &app.knowledge_list);
    let visible_ids: Vec<i64> = visible.iter().map(|&i| app.knowledge_docs[i].id).collect();
    doc_list_bar(app, ui, &visible_ids);
    ui.add_space(4.0);
    if visible.is_empty() {
        ui.add_space(8.0);
        ui.weak("没有符合筛选条件的文档。");
        return;
    }
    match doc_table(app, ui, &visible, &visible_ids) {
        Some(DocAction::Preview(id)) => app.knowledge_open_preview(id),
        Some(DocAction::Rename(id, title)) => app.knowledge_list.rename = Some((id, title)),
        Some(DocAction::Delete(id)) => app.knowledge_delete_confirm = Some(vec![id]),
        None => {}
    }
}

/// 列表里一行触发的动作，表格画完再执行，免得边遍历边改。
enum DocAction {
    Preview(i64),
    Rename(i64, String),
    Delete(i64),
}

/// 按筛选条件与排序给出可见文档在 `docs` 里的下标。每帧现算：几千篇也只是
/// 一遍子串匹配加一次排序，不值得做缓存与失效。
fn visible_docs(docs: &[KnowledgeDocRow], list: &KnowledgeListState) -> Vec<usize> {
    let needle = list.filter.trim().to_lowercase();
    let mut visible: Vec<usize> = docs
        .iter()
        .enumerate()
        .filter(|(_, doc)| !list.only_unindexed || doc.embed_model.is_empty())
        .filter(|(_, doc)| {
            needle.is_empty()
                || doc.title.to_lowercase().contains(&needle)
                || doc.source_path.to_lowercase().contains(&needle)
        })
        .map(|(index, _)| index)
        .collect();
    match list.sort {
        // 库里取出来就是按更新时间倒序。
        DocSort::UpdatedDesc => {}
        DocSort::Title => visible.sort_by(|&a, &b| docs[a].title.cmp(&docs[b].title)),
        DocSort::ChunksDesc => {
            visible.sort_by(|&a, &b| docs[b].chunk_count.cmp(&docs[a].chunk_count))
        }
    }
    visible
}

/// 列表上方一行：筛选框、排序、只看未索引；勾选了文档时右侧出批量操作。
fn doc_list_bar(app: &mut GongwenApp, ui: &mut egui::Ui, visible_ids: &[i64]) {
    let total = app.knowledge_docs.len();
    let selected: Vec<i64> = visible_ids
        .iter()
        .copied()
        .filter(|id| app.knowledge_list.selected.contains(id))
        .collect();
    ui.horizontal(|ui| {
        let list = &mut app.knowledge_list;
        ui.add(
            egui::TextEdit::singleline(&mut list.filter)
                .hint_text("按标题或文件路径筛选")
                .desired_width(240.0),
        );
        if !list.filter.is_empty()
            && theme::icon_button(ui, theme::Icon::SearchClear, "清除筛选").clicked()
        {
            list.filter.clear();
        }
        egui::ComboBox::from_id_salt("knowledge_doc_sort")
            .selected_text(format!("排序：{}", list.sort.label()))
            .show_ui(ui, |ui| {
                for sort in DocSort::ALL {
                    ui.selectable_value(&mut list.sort, sort, sort.label());
                }
            });
        ui.checkbox(&mut list.only_unindexed, "只看未索引")
            .on_hover_text("只列出还没建索引、或改过标题待重建索引的文档");
        if visible_ids.len() != total {
            ui.weak(format!("{} / {total} 篇", visible_ids.len()));
        }
        if selected.is_empty() {
            return;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(theme::warning_icon_button(
                    theme::Icon::Trash,
                    &format!("删除所选（{}）", selected.len()),
                ))
                .clicked()
            {
                app.knowledge_delete_confirm = Some(selected.clone());
            }
            if ui.button("取消选择").clicked() {
                for id in &selected {
                    app.knowledge_list.selected.remove(id);
                }
            }
        });
    });
}

/// 文档表格：一篇一行，只画可视区里的行（`TableBuilder` 按固定行高虚拟滚动），
/// 上千篇也不卡。标题列吃剩余宽度并截断，右侧状态与操作列宽度固定，标题再长
/// 也挤不掉按钮——旧的卡片列表里超长标题会把「删除」挤出卡片，还把整个滚动区
/// 撑宽，连带后面所有卡片的按钮都跑到可视区外。
fn doc_table(
    app: &mut GongwenApp,
    ui: &mut egui::Ui,
    visible: &[usize],
    visible_ids: &[i64],
) -> Option<DocAction> {
    const ROW_HEIGHT: f32 = 28.0;
    let busy = app.knowledge_busy;
    let docs = &app.knowledge_docs;
    let selected = &mut app.knowledge_list.selected;
    let mut action = None;
    TableBuilder::new(ui)
        .id_salt("knowledge_doc_table")
        .striped(!theme::smartisan::active())
        .auto_shrink([false, false])
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::auto().at_least(28.0)) // 勾选
        .column(Column::exact(96.0)) // 文种
        .column(Column::remainder().at_least(160.0).clip(true)) // 标题
        .column(Column::exact(72.0)) // 块数 / 索引状态
        .column(Column::exact(96.0)) // 更新
        .column(Column::exact(72.0)) // 操作
        .header(ROW_HEIGHT, |mut header| {
            let mut all_selected =
                !visible_ids.is_empty() && visible_ids.iter().all(|id| selected.contains(id));
            header.col(|ui| {
                theme::smartisan::ledger_header(ui);
                if ui
                    .checkbox(&mut all_selected, "")
                    .on_hover_text(if all_selected {
                        "取消选择当前列出的全部文档"
                    } else {
                        "选择当前列出的全部文档"
                    })
                    .changed()
                {
                    for id in visible_ids {
                        if all_selected {
                            selected.insert(*id);
                        } else {
                            selected.remove(id);
                        }
                    }
                }
            });
            header.col(|ui| {
                theme::smartisan::ledger_header(ui);
                centered_header(ui, "文种");
            });
            header.col(|ui| {
                theme::smartisan::ledger_header(ui);
                ui.strong("标题");
            });
            header.col(|ui| {
                theme::smartisan::ledger_header(ui);
                centered_header(ui, "块数");
            });
            header.col(|ui| {
                theme::smartisan::ledger_header(ui);
                centered_header(ui, "更新");
            });
            header.col(|ui| {
                theme::smartisan::ledger_header(ui);
                centered_header(ui, "操作");
            });
        })
        .body(|mut body| {
            let ledger_painter = body.ui_mut().painter().clone();
            body.rows(ROW_HEIGHT, visible.len(), |mut row| {
                let doc = &docs[visible[row.index()]];
                let mut checked = selected.contains(&doc.id);
                row.set_selected(checked);
                row.col(|ui| {
                    if ui.checkbox(&mut checked, "").changed() {
                        if checked {
                            selected.insert(doc.id);
                        } else {
                            selected.remove(&doc.id);
                        }
                    }
                });
                row.col(|ui| {
                    centered_cell_text(ui, doc.kind.label());
                });
                row.col(|ui| {
                    // 标题左对齐、单行截断；点一下预览全文。
                    let title = egui::Label::new(egui::RichText::new(&doc.title).strong())
                        .truncate()
                        .sense(egui::Sense::click());
                    if ui
                        .add(title)
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_ui(|ui| doc_tooltip(ui, doc))
                        .clicked()
                    {
                        action = Some(DocAction::Preview(doc.id));
                    }
                });
                row.col(|ui| {
                    if !doc.embed_model.is_empty() {
                        centered_cell_text(ui, doc.chunk_count.to_string());
                    } else if doc.chunk_count > 0 {
                        centered_cell_text(ui, egui::RichText::new("待重建").color(theme::warn()))
                            .on_hover_text("标题改过，检索暂用旧索引；点「建立索引」更新");
                    } else {
                        centered_cell_text(ui, egui::RichText::new("未索引").color(theme::warn()))
                            .on_hover_text("还没建立索引，检索与问答用不到；点「建立索引」");
                    }
                });
                row.col(|ui| {
                    centered_cell_text(ui, short_date(&doc.updated_at));
                });
                row.col(|ui| {
                    // 索引任务会回写同一篇的 embed_model，与改名交错会把「待重建」冲掉。
                    if theme::icon_button_enabled(ui, !busy, theme::Icon::PencilLine, "改标题")
                        .clicked()
                    {
                        action = Some(DocAction::Rename(doc.id, doc.title.clone()));
                    }
                    if theme::danger_icon_button(ui, theme::Icon::Trash, "删除").clicked() {
                        action = Some(DocAction::Delete(doc.id));
                    }
                });
                theme::smartisan::index_row(&ledger_painter, row.response().rect, checked);
            });
        });
    action
}

/// 标题悬停：完整标题与出处、嵌入模型这些不常看的信息，省得每篇多占一行。
fn doc_tooltip(ui: &mut egui::Ui, doc: &KnowledgeDocRow) {
    ui.label(egui::RichText::new(&doc.title).strong());
    // 外部文件给完整路径，稿件给稿件编号。
    let origin = match (doc.source.as_str(), doc.source_manuscript_id) {
        ("manuscript", Some(id)) => format!("稿件库 · 稿件 #{id}"),
        ("manuscript", None) => "稿件库".to_string(),
        _ if doc.source_path.is_empty() => "外部文件".to_string(),
        _ => format!("外部文件 · {}", doc.source_path),
    };
    ui.weak(format!("来源：{origin}"));
    ui.weak(format!(
        "嵌入模型：{}",
        if doc.embed_model.is_empty() {
            "—"
        } else {
            &doc.embed_model
        }
    ));
    ui.weak("点击预览全文");
}

/// 输入区：模式切换（检索 / 问答）共用一个输入框，hint、按钮文字与回车行为
/// 随模式变化；发送后按模式分流到片段列表或问答对话区。两种模式的结果
/// 各自保留，来回切换互不清空。
fn search_panel(app: &mut GongwenApp, ui: &mut egui::Ui) {
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("检索 / 问答").strong())
                .on_hover_ui(|ui| match app.knowledge_mode {
                    KnowledgeMode::Search => {
                        ui.label(
                            "输入检索要求，查看会从知识库调出哪些片段（与起草时的检索一致）。",
                        );
                    }
                    KnowledgeMode::Qa => {
                        ui.label("向知识库提问，答案基于库内片段生成并标注出处，可连续追问。");
                    }
                });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                mode_switch(app, ui);
            });
        });
        ui.add_space(6.0);
        let (hint, button_label, icon) = match app.knowledge_mode {
            KnowledgeMode::Search => (
                "例如：关于开展安全生产检查的通知",
                "检索",
                theme::Icon::Reveal,
            ),
            KnowledgeMode::Qa => (
                "向知识库提问，例如：安全检查多久开展一次？",
                "提问",
                theme::Icon::Sparkles,
            ),
        };
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut app.knowledge_test_query)
                    .hint_text(hint)
                    .desired_width((ui.available_width() - 90.0).max(120.0)),
            );
            // 回车也能触发。
            let enter =
                response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            let submit = ui
                .add_enabled(
                    !app.knowledge_busy,
                    theme::icon_text_button(icon, button_label),
                )
                .clicked()
                || (enter && !app.knowledge_busy);
            if submit {
                match app.knowledge_mode {
                    KnowledgeMode::Search => app.knowledge_test_search(),
                    KnowledgeMode::Qa => app.knowledge_ask(),
                }
            }
        });
        // 文种筛选条同时在约束本次操作，不说明的话容易变成“明明导入了却没结果”。
        if let Some(kind) = app.knowledge_filter_kind {
            let verb = match app.knowledge_mode {
                KnowledgeMode::Search => "检索",
                KnowledgeMode::Qa => "问答",
            };
            ui.weak(format!(
                "当前只在「{}」范围内{verb}，改上方的文种筛选可放开。",
                kind.label()
            ));
        }
        ui.add_space(6.0);

        match app.knowledge_mode {
            KnowledgeMode::Search => search_results(app, ui),
            KnowledgeMode::Qa => qa_chat(app, ui),
        }
    });
}

/// 模式分段控件：检索 / 问答。切换只改输入区行为，不动对方的结果状态。
fn mode_switch(app: &mut GongwenApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.selectable_value(&mut app.knowledge_mode, KnowledgeMode::Search, "检索")
            .on_hover_text("按相关度列出命中的知识库片段");
        ui.selectable_value(&mut app.knowledge_mode, KnowledgeMode::Qa, "问答")
            .on_hover_text("基于知识库片段生成答案并标注出处，可连续追问");
    });
}

/// 检索模式的结果区：busy 提示、降级告警、相关片段列表。
fn search_results(app: &mut GongwenApp, ui: &mut egui::Ui) {
    // 索引任务也在占用 busy，此时不显示检索 spinner。
    if app.knowledge_busy && app.knowledge_index_progress.is_none() {
        ui.horizontal(|ui| {
            theme::spinner(ui, 14.0, theme::accent());
            ui.weak("正在检索…");
        });
        return;
    }
    // 降级告警：embedding 连不上、rerank 端点不对等，以前只写进服务端日志。
    for warning in &app.knowledge_search_warnings {
        ui.colored_label(theme::warn(), warning);
    }
    if app.knowledge_test_results.is_empty() {
        if !app.knowledge_test_query.trim().is_empty() {
            ui.weak("未检索到相关片段。可先「导入文档」或在稿件管理「导入到知识库」，再点「建立索引」。");
        }
        return;
    }

    // 检索问答独占一个子页签，结果区吃满剩余高度；减去的是外层卡片的下内边距。
    let results = app.knowledge_test_results.clone();
    let max_height = (ui.available_height() - 16.0).max(140.0);
    egui::ScrollArea::vertical()
        .id_salt("knowledge_search_results")
        .max_height(max_height)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (i, chunk) in results.iter().enumerate() {
                result_card(app, ui, i, chunk);
                ui.add_space(4.0);
            }
        });
}

/// 问答模式：多轮对话历史（问题 + 答案卡片），等待时展示待生成的问题。
fn qa_chat(app: &mut GongwenApp, ui: &mut egui::Ui) {
    let has_content = !app.knowledge_qa_history.is_empty() || app.knowledge_qa_pending.is_some();
    if has_content {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("对话").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(theme::icon_text_button(theme::Icon::RotateCcw, "清空对话"))
                    .on_hover_text("清除全部问答历史，不影响知识库文档")
                    .clicked()
                {
                    app.knowledge_qa_history.clear();
                    app.knowledge_qa_pending = None;
                }
            });
        });
        ui.add_space(4.0);
    }
    // 正在生成：先展示问题本身，答案回来后才入历史。
    if let Some(question) = app.knowledge_qa_pending.clone() {
        qa_question_row(ui, &question);
        ui.horizontal(|ui| {
            theme::spinner(ui, 14.0, theme::accent());
            ui.weak("正在生成答案…");
        });
        ui.add_space(6.0);
    }
    if app.knowledge_qa_history.is_empty() {
        if app.knowledge_qa_pending.is_none() {
            ui.weak("向知识库提问，答案会基于库内文档生成并标注出处。");
        }
        return;
    }
    // 与检索结果区同样吃满剩余高度。
    let history = app.knowledge_qa_history.clone();
    let max_height = (ui.available_height() - 16.0).max(140.0);
    egui::ScrollArea::vertical()
        .id_salt("knowledge_qa_history")
        .max_height(max_height)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (i, turn) in history.iter().enumerate() {
                qa_question_row(ui, &turn.question);
                qa_answer_card(app, ui, i, turn);
                ui.add_space(6.0);
            }
        });
}

/// 用户问题行：左侧「问」徽章 + 问题文本。
fn qa_question_row(ui: &mut egui::Ui, question: &str) {
    ui.horizontal(|ui| {
        theme::chip(ui, "问", theme::accent(), theme::surface_sunk());
        ui.label(egui::RichText::new(question).strong());
    });
    ui.add_space(2.0);
}

/// 单轮答案卡片：降级告警、答案正文、复制按钮、可折叠的参考片段。
fn qa_answer_card(app: &mut GongwenApp, ui: &mut egui::Ui, index: usize, turn: &qa::QaTurn) {
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        for warning in &turn.warnings {
            ui.colored_label(theme::warn(), warning);
        }
        ui.label(
            egui::RichText::new(if turn.answer.trim().is_empty() {
                "（模型未返回内容）"
            } else {
                turn.answer.as_str()
            })
            .color(theme::text_soft()),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if theme::icon_button(ui, theme::Icon::Copy, "复制答案").clicked() {
                ui.ctx().copy_text(turn.answer.clone());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if turn.has_references() {
                    egui::CollapsingHeader::new(format!("参考片段 {} 条", turn.references.len()))
                        .id_salt(("knowledge_qa_refs", index))
                        .show(ui, |ui| {
                            for chunk in &turn.references {
                                qa_ref_row(app, ui, chunk);
                                ui.add_space(3.0);
                            }
                        });
                }
            });
        });
    });
}

/// 折叠区里的一条引用：出处行 + 片段摘录，可预览整篇文档。
fn qa_ref_row(app: &mut GongwenApp, ui: &mut egui::Ui, chunk: &crate::rag::RetrievedChunk) {
    ui.horizontal(|ui| {
        // 与文档列表同理：先排右侧按钮，出处占剩余宽度并截断。
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::icon_button(ui, theme::Icon::Eye, "预览该文档").clicked() {
                app.knowledge_open_preview(chunk.doc_id);
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                theme::chip(
                    ui,
                    chunk.kind.label(),
                    theme::accent(),
                    theme::surface_sunk(),
                );
                chunk_origin(ui, chunk);
            });
        });
    });
    ui.label(egui::RichText::new(truncate_chars(&chunk.text, 120)).color(theme::text_soft()));
}

/// 单条检索结果：左侧排名徽章，标题行出处，正文截断，底部各档相似度。
fn result_card(
    app: &mut GongwenApp,
    ui: &mut egui::Ui,
    index: usize,
    chunk: &crate::rag::RetrievedChunk,
) {
    // 最终展示分：rerank 优先，其次向量余弦。两者都没有时说明本次是纯关键词
    // 召回——此时显示 RRF 融合分（0.0164 这种数量级）对用户毫无意义，
    // 直接标成「关键词命中」并给出 bm25 归一化分。
    let (label, score) = if let Some(rerank) = chunk.rerank_score {
        ("重排相关度", rerank)
    } else if chunk.vector_score > 0.0 {
        ("语义相似度", chunk.vector_score)
    } else {
        ("关键词匹配", chunk.bm25_score)
    };
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            // 先排右侧按钮与分数，出处占剩余宽度并截断（理由见 doc_list）。
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // 预览整篇文档（公文版式）。
                if theme::icon_button(ui, theme::Icon::Eye, "预览该文档").clicked() {
                    app.knowledge_open_preview(chunk.doc_id);
                }
                ui.label(
                    egui::RichText::new(format!("{label} {:.3}", score))
                        .strong()
                        .color(theme::accent()),
                );
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    // 排名徽章。
                    let (bg, fg) = if index == 0 {
                        (theme::accent(), theme::accent_text())
                    } else {
                        (theme::surface_sunk(), theme::text_soft())
                    };
                    theme::chip(ui, &format!("#{}", index + 1), fg, bg);
                    theme::chip(
                        ui,
                        chunk.kind.label(),
                        theme::accent(),
                        theme::surface_sunk(),
                    );
                    chunk_origin(ui, chunk);
                });
            });
        });
        ui.label(egui::RichText::new(truncate_chars(&chunk.text, 220)).color(theme::text_soft()));
        ui.weak(format!(
            "向量 {:.3} · 关键词 {:.3} · 融合 {:.4}{}",
            chunk.vector_score,
            chunk.bm25_score,
            chunk.fused_score,
            chunk
                .rerank_score
                .map(|s| format!(" · rerank {s:.3}"))
                .unwrap_or_default()
        ));
    });
}

fn import_dialog(app: &mut GongwenApp, ui: &mut egui::Ui) {
    let Some(draft) = app.knowledge_import.as_mut() else {
        return;
    };
    // 有下拉的表单：点遮罩常常只是想收起下拉，只认 Esc 与「取消」。
    let dialog = modal::dialog(
        ui.ctx(),
        egui::Id::new("knowledge_import"),
        "导入文档到知识库",
        420.0,
        Dismiss::EscOnly,
        |ui| {
            ui.label(format!("已选 {} 个文件：", draft.paths.len()));
            for path in draft.paths.iter().take(6) {
                ui.weak(format!("· {}", path.display()));
            }
            if draft.paths.len() > 6 {
                ui.weak(format!("… 等共 {} 个", draft.paths.len()));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("归入文种：");
                egui::ComboBox::from_id_salt("knowledge_import_kind")
                    .selected_text(draft.kind.label())
                    .show_ui(ui, |ui| {
                        for kind in TemplateKind::ALL {
                            ui.selectable_value(&mut draft.kind, kind, kind.label());
                        }
                    });
            });
            ui.add_space(10.0);
            let mut start = false;
            ui.horizontal(|ui| {
                if ui.button("开始导入").clicked() {
                    start = true;
                }
                if ui.button("取消").clicked() {
                    ui.close();
                }
            });
            start
        },
    );
    if dialog.inner {
        app.knowledge_confirm_import();
    } else if dialog.dismissed {
        app.knowledge_import = None;
    }
}

fn delete_confirm(app: &mut GongwenApp, ui: &mut egui::Ui) {
    let Some(ids) = app.knowledge_delete_confirm.clone() else {
        return;
    };
    let message = match ids.as_slice() {
        [id] => {
            let title = app
                .knowledge_docs
                .iter()
                .find(|doc| doc.id == *id)
                .map_or("这篇文档", |doc| doc.title.as_str());
            format!("确定把《{}》从知识库删除吗？", truncate_chars(title, 40))
        }
        _ => format!("确定把所选的 {} 篇文档从知识库删除吗？", ids.len()),
    };
    let dialog = modal::dialog(
        ui.ctx(),
        egui::Id::new("knowledge_delete_confirm"),
        "删除知识库文档",
        380.0,
        Dismiss::EscOrBackdrop,
        |ui| {
            ui.label(message);
            ui.weak("切块与索引一并清除，不影响稿件库原件与本机文件。");
            ui.add_space(10.0);
            let mut confirm = false;
            ui.horizontal(|ui| {
                if ui
                    .add(theme::warning_icon_button(theme::Icon::Trash, "删除"))
                    .clicked()
                {
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
        app.knowledge_delete(&ids);
    } else if dialog.dismissed {
        app.knowledge_delete_confirm = None;
    }
}

fn rename_dialog(app: &mut GongwenApp, ui: &mut egui::Ui) {
    let Some((id, title)) = app.knowledge_list.rename.as_mut() else {
        return;
    };
    let id = *id;
    let dialog = modal::dialog(
        ui.ctx(),
        egui::Id::new("knowledge_rename"),
        "修改文档标题",
        460.0,
        Dismiss::EscOrBackdrop,
        |ui| {
            let response = ui.add(
                egui::TextEdit::singleline(title)
                    .hint_text("文档标题")
                    .desired_width(ui.available_width()),
            );
            // 打开即可输入；失焦那一帧不抢回来，免得按回车后又被拉回输入框。
            if !response.has_focus() && !response.lost_focus() {
                response.request_focus();
            }
            let enter =
                response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            ui.add_space(4.0);
            ui.weak("标题参与检索：改完这篇转为「待重建」，点「建立索引」后检索才用上新标题。");
            ui.add_space(10.0);
            let valid = !title.trim().is_empty();
            let mut save = enter && valid;
            ui.horizontal(|ui| {
                if ui.add_enabled(valid, egui::Button::new("保存")).clicked() {
                    save = true;
                }
                if ui.button("取消").clicked() {
                    ui.close();
                }
            });
            save.then(|| title.clone())
        },
    );
    if let Some(title) = dialog.inner {
        app.knowledge_rename(id, &title);
    } else if dialog.dismissed {
        app.knowledge_list.rename = None;
    }
}

/// 片段出处「标题 · 小节」：合成一行截断，悬停看全文。
fn chunk_origin(ui: &mut egui::Ui, chunk: &crate::rag::RetrievedChunk) {
    let section = chunk.section.trim();
    let mut job = egui::text::LayoutJob::default();
    let style = ui.style();
    egui::RichText::new(&chunk.doc_title).strong().append_to(
        &mut job,
        style,
        egui::FontSelection::Default,
        egui::Align::Center,
    );
    if !section.is_empty() {
        egui::RichText::new(format!(" · {section}"))
            .color(ui.visuals().weak_text_color())
            .append_to(
                &mut job,
                style,
                egui::FontSelection::Default,
                egui::Align::Center,
            );
    }
    let full = if section.is_empty() {
        chunk.doc_title.clone()
    } else {
        format!("{} · {section}", chunk.doc_title)
    };
    ui.add(egui::Label::new(job).truncate()).on_hover_text(full);
}

fn truncate_chars(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let truncated: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}
