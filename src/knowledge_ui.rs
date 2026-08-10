//! 知识库页：文档的分组列表、导入（外部 markdown / 库内稿件）、删除、重建索引、
//! 检索测试。从 app.rs 拆出以控制体积；通过 `GongwenApp` 上的 `pub(crate)` 字段
//! 与方法读写状态。

use crate::app::{GongwenApp, KnowledgeMode};
use crate::models::TemplateKind;
use crate::qa;
use crate::theme;
use eframe::egui;

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

    // 检索区固定显示在列表上方，导入多少数据都看得到、随时可用。
    search_panel(app, ui);
    ui.add_space(8.0);

    // 文档列表吃掉剩余高度：写死高度在窗口拉高时留大片空白、压矮时又够不着底部。
    ui.label(egui::RichText::new("库内文档").strong());
    ui.add_space(4.0);
    doc_list(app, ui);
    import_dialog(app, ui);
    delete_confirm(app, ui);
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
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let busy = app.knowledge_busy;
            if ui
                .add_enabled(
                    !busy,
                    theme::icon_text_button(theme::Icon::FileUp, "导入 Markdown"),
                )
                .on_hover_text("选择本机 .md 公文加入知识库")
                .clicked()
            {
                app.knowledge_pick_markdown();
            }
            if ui
                .add_enabled(
                    !busy,
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
            ui.add(egui::Spinner::new());
            let text = if title.is_empty() {
                format!("正在建立索引… {done}/{total}")
            } else {
                format!("正在建立索引… {done}/{total}　《{title}》")
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

fn doc_list(app: &mut GongwenApp, ui: &mut egui::Ui) {
    if app.knowledge_docs.is_empty() {
        theme::card().show(ui, |ui| {
            ui.weak("知识库还是空的。点右上角「导入 Markdown」选本机公文，或到「稿件管理」勾选稿件后用工具栏的「导入到知识库」。");
        });
        return;
    }
    // 吃掉剩余高度，不写死——写死的高度在窗口拉高时留白、压矮时够不着底部。
    egui::ScrollArea::vertical()
        .id_salt("knowledge_doc_list")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut delete_pending = None;
            // 借出 docs 拷贝，避免遍历与 delete 冲突。
            let docs = app.knowledge_docs.clone();
            for doc in &docs {
                theme::card().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        theme::chip(ui, doc.kind.label(), theme::accent(), theme::surface_sunk());
                        ui.label(egui::RichText::new(&doc.title).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add(theme::icon_text_button(theme::Icon::Trash, "删除"))
                                .clicked()
                            {
                                delete_pending = Some(doc.id);
                            }
                            ui.weak(format!("{} 块", doc.chunk_count));
                        });
                    });
                    ui.horizontal(|ui| {
                        let source_label = if doc.source == "manuscript" {
                            "稿件库"
                        } else {
                            "外部文件"
                        };
                        ui.weak(format!("来源：{source_label}"));
                        ui.separator();
                        ui.weak(format!(
                            "嵌入模型：{}",
                            if doc.embed_model.is_empty() {
                                "未嵌入"
                            } else {
                                &doc.embed_model
                            }
                        ));
                        ui.separator();
                        ui.weak(format!(
                            "更新：{}",
                            doc.updated_at.get(..10).unwrap_or(&doc.updated_at)
                        ));
                    });
                });
                ui.add_space(4.0);
            }
            if let Some(id) = delete_pending {
                app.knowledge_delete_confirm = Some(id);
            }
        });
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
            ui.add(egui::Spinner::new());
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
            ui.weak("未检索到相关片段。可先「导入 Markdown」或在稿件管理「导入到知识库」。");
        }
        return;
    }

    // 结果区限高滚动：它在页面中部，高度跟着窗口走会把下方的文档列表挤没，
    // 所以这里按可用高度的一半自适应，并留出底部列表的最小可视高度。
    let results = app.knowledge_test_results.clone();
    let max_height = (ui.available_height() - 180.0).clamp(140.0, 420.0);
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
            ui.add(egui::Spinner::new());
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
    // 与检索结果区同样的限高策略，把底部文档列表的可视空间留出来。
    let history = app.knowledge_qa_history.clone();
    let max_height = (ui.available_height() - 180.0).clamp(140.0, 420.0);
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
        theme::chip(
            ui,
            chunk.kind.label(),
            theme::accent(),
            theme::surface_sunk(),
        );
        ui.label(egui::RichText::new(&chunk.doc_title).strong());
        if !chunk.section.trim().is_empty() {
            ui.weak(format!("· {}", chunk.section));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::icon_button(ui, theme::Icon::Eye, "预览该文档").clicked() {
                app.knowledge_open_preview(chunk.doc_id);
            }
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
            // 排名徽章。
            let (bg, fg) = if index == 0 {
                (theme::accent(), egui::Color32::WHITE)
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
            ui.label(egui::RichText::new(&chunk.doc_title).strong());
            if !chunk.section.trim().is_empty() {
                ui.weak(format!("· {}", chunk.section));
            }
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
    if app.knowledge_import.is_none() {
        return;
    }
    let mut open = true;
    egui::Window::new("导入 Markdown 到知识库")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ui.ctx(), |ui| {
            let Some(draft) = app.knowledge_import.as_mut() else {
                return;
            };
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
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("开始导入").clicked() {
                    app.knowledge_confirm_import();
                }
                if ui.button("取消").clicked() {
                    app.knowledge_import = None;
                }
            });
        });
    if !open {
        app.knowledge_import = None;
    }
}

fn delete_confirm(app: &mut GongwenApp, ui: &mut egui::Ui) {
    let Some(id) = app.knowledge_delete_confirm else {
        return;
    };
    let mut open = true;
    let mut confirm = false;
    egui::Window::new("删除知识库文档")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ui.ctx(), |ui| {
            ui.label("确定把这篇文档从知识库删除吗？其切块与索引一并清除，不影响稿件库原件。");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("删除").clicked() {
                    confirm = true;
                }
                if ui.button("取消").clicked() {
                    app.knowledge_delete_confirm = None;
                }
            });
        });
    if confirm {
        app.knowledge_delete(id);
    } else if !open {
        app.knowledge_delete_confirm = None;
    }
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
