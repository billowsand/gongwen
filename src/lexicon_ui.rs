//! 公文词表页：扫描语料、确认候选词、导出小鹤双拼用户码表。
//!
//! 一期只做主链路：累积 → 粗筛 → 导出。筛选、批量与读音工作台在二期铺开，
//! 所以这里的列表刻意只给「接受 / 拒绝」两个动作和一个读音编辑框——这三件事
//! 决定词表能不能长期用下去，其余都是效率问题。

use crate::app::GongwenApp;
use crate::lexicon::{TermOrigin, TermState, scan};
use crate::theme;
use eframe::egui;
use egui_extras::{Column, TableBuilder};

/// 词表页入口。
pub(crate) fn lexicon_ui(app: &mut GongwenApp, ui: &mut egui::Ui) {
    if app.lexicon_store.is_none() {
        ui.colored_label(
            theme::warn(),
            app.lexicon_error
                .clone()
                .unwrap_or_else(|| "公文词表不可用。".to_string()),
        );
        return;
    }
    if app.lexicon_dirty {
        app.refresh_lexicon();
    }
    toolbar(app, ui);
    ui.add_space(6.0);
    scan_panel(app, ui);
    ui.add_space(8.0);
    export_panel(app, ui);
    ui.add_space(8.0);
    if let Some(error) = app.lexicon_error.clone() {
        ui.horizontal(|ui| {
            ui.colored_label(theme::warn(), error);
            if ui.small_button("知道了").clicked() {
                app.lexicon_error = None;
            }
        });
        ui.add_space(4.0);
    }
    filter_bar(app, ui);
    ui.add_space(4.0);
    term_table(app, ui);
}

fn toolbar(app: &mut GongwenApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.heading("公文词表");
        ui.add_space(8.0);
        let stats = &app.lexicon_stats;
        ui.weak(format!(
            "{} 词 · 已接受 {} · 待确认 {} · 已拒绝 {} · 已扫 {} 篇",
            stats.total, stats.accepted, stats.candidate, stats.rejected, stats.scanned_sources
        ));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let busy = app.lexicon_busy;
            if ui
                .add_enabled(!busy, theme::icon_text_button(theme::Icon::Refresh, "扫描"))
                .on_hover_text(
                    "并入标准词库与校对词表，再扫正文变过的稿件；已扫过且没改动的整篇跳过",
                )
                .clicked()
            {
                app.start_lexicon_scan(false);
            }
            if ui
                .add_enabled(!busy, egui::Button::new("全部重扫"))
                .on_hover_text("忽略扫描记账，把每一篇都重新分词。换过扫描口径后用")
                .clicked()
            {
                app.start_lexicon_scan(true);
            }
        });
    });
}

fn scan_panel(app: &mut GongwenApp, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.weak("语料口径：");
        let mut options = app.lexicon_scan_options.clone();
        ui.checkbox(&mut options.include_drafts, "含草稿")
            .on_hover_text("草稿是半成品，错字与临时措辞最多，默认不计入语料");
        ui.checkbox(&mut options.include_knowledge, "含知识库外部文档")
            .on_hover_text(
                "只算从外部 Markdown 导入的；从稿件库入库的那些是副本，算两遍会让篇数翻倍",
            );
        if options != app.lexicon_scan_options {
            app.lexicon_scan_options = options;
        }
        ui.weak(scan_scope_hint(&app.lexicon_scan_options));
    });
    if let Some((done, total, title)) = &app.lexicon_scan_progress {
        ui.horizontal(|ui| {
            ui.add(
                egui::ProgressBar::new(if *total == 0 {
                    0.0
                } else {
                    *done as f32 / *total as f32
                })
                .desired_width(220.0)
                .text(format!("{done}/{total}")),
            );
            ui.weak(title);
        });
    }
    if let Some(result) = &app.lexicon_scan_result {
        ui.weak(result);
    }
}

fn export_panel(app: &mut GongwenApp, ui: &mut egui::Ui) {
    egui::CollapsingHeader::new("导出小鹤双拼用户码表")
        .default_open(true)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let options = &mut app.lexicon_export;
                ui.label("最少字数");
                ui.add(egui::DragValue::new(&mut options.min_chars).range(2..=8))
                    .on_hover_text(
                        "小鹤词组是四码定长：n 字词正常敲 2n 键，进码表一律 4 键。\
                         二字词省 0 键，收进去只会多一条重码",
                    );
                ui.add_space(8.0);
                ui.label("最少篇数");
                ui.add(egui::DragValue::new(&mut options.min_doc_count).range(1..=20))
                    .on_hover_text("只在一篇里出现过的词多半是一次性的，先别进码表");
                ui.add_space(8.0);
                ui.label("条数上限");
                ui.add(
                    egui::DragValue::new(&mut options.budget)
                        .range(50..=20000)
                        .speed(10.0),
                )
                .on_hover_text(
                    "用户码表和主码表合进同一个编码空间：多收一个词就是多一条重码、\
                         多一次翻页。超出上限的按「省键数 × 篇数」截断",
                );
            });
            ui.horizontal_wrapped(|ui| {
                let options = &mut app.lexicon_export;
                ui.checkbox(&mut options.include_candidates, "含待确认词");
                ui.checkbox(&mut options.exclude_common, "排除通用词")
                    .on_hover_text(
                        "排除 jieba 自带词典里就有的词。注意公文套语多半也在自带词典里，\
                         而那恰恰是最省键的一类，默认不排除",
                    );
                ui.checkbox(&mut options.with_header, "写入小鹤码表头")
                    .on_hover_text("导入「主码-用户码表」时需要这两行 ---config@ 头");
            });

            // 预览按口径缓存：条数、省键、重码都要在点导出之前就看得到，
            // 但给每个词出码不便宜，不能每帧重算。
            let preview = app.lexicon_preview();
            ui.horizontal_wrapped(|ui| {
                ui.label(&preview.summary);
                if preview.saved_keys > 0 {
                    ui.weak(format!("· 每轮各打一次可省 {} 键", preview.saved_keys));
                }
            });
            if !preview.origins.is_empty() {
                ui.weak(format!("词源构成：{}", preview.origins));
            }
            if preview.truncated > 0 {
                ui.colored_label(
                    theme::warn(),
                    format!(
                        "还有 {} 条够格但超出了条数上限，已按「省键数 × 篇数」截掉。\
                         要全收就调大上限，但重码会跟着变多。",
                        preview.truncated
                    ),
                );
            }
            if preview.conflict_codes > 0 {
                ui.colored_label(
                    theme::warn(),
                    format!(
                        "{} 个编码上有重码（共 {} 词）。重码越多翻页越频繁，\
                         可以调小条数上限或提高最少篇数。",
                        preview.conflict_codes, preview.conflict_terms
                    ),
                );
            }
            if !preview.failed.is_empty() {
                ui.colored_label(
                    theme::warn(),
                    format!(
                        "{} 条编码失败，多半是多音字标注与字数对不上：{}",
                        preview.failed.len(),
                        preview
                            .failed
                            .iter()
                            .take(3)
                            .map(String::as_str)
                            .collect::<Vec<_>>()
                            .join("、")
                    ),
                );
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        preview.written > 0,
                        theme::icon_text_button(theme::Icon::FileDown, "导出码表…"),
                    )
                    .clicked()
                {
                    app.export_flypy_table();
                }
                ui.weak("导出后在小鹤输入法的码表管理里导入「主码-用户码表」。");
            });
            // 码表是明文文件，导进输入法目录后就脱离本应用管控了。默认密级是
            // 「机密」，按密级过滤会把所有稿子都挡掉，所以这里只能靠明确提示。
            ui.colored_label(
                theme::warn(),
                "码表是明文文件，导入输入法后不再受本应用管控。涉密项目代号、\
                 人名与文号请先在下方列表里拒绝，再导出。",
            );
            if let Some(result) = &app.lexicon_export_result {
                ui.weak(result);
            }
        });
}

fn filter_bar(app: &mut GongwenApp, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.weak("状态：");
        let mut chosen = app.lexicon_filter.state;
        if ui.selectable_label(chosen.is_none(), "全部").clicked() {
            chosen = None;
        }
        for state in [
            TermState::Candidate,
            TermState::Accepted,
            TermState::Rejected,
        ] {
            if ui
                .selectable_label(chosen == Some(state), state.label())
                .clicked()
            {
                chosen = Some(state);
            }
        }
        ui.add_space(12.0);
        ui.weak("来源：");
        let mut origin = app.lexicon_filter.origin;
        if ui.selectable_label(origin.is_none(), "全部").clicked() {
            origin = None;
        }
        for value in [
            TermOrigin::Vocabulary,
            TermOrigin::Proofread,
            TermOrigin::Corpus,
            TermOrigin::Manual,
        ] {
            if ui
                .selectable_label(origin == Some(value), value.label())
                .clicked()
            {
                origin = Some(value);
            }
        }
        ui.add_space(12.0);
        let mut search = app.lexicon_filter.search.clone();
        let changed = ui
            .add(
                egui::TextEdit::singleline(&mut search)
                    .hint_text("搜词")
                    .desired_width(120.0),
            )
            .changed();
        if chosen != app.lexicon_filter.state || origin != app.lexicon_filter.origin || changed {
            app.lexicon_filter.state = chosen;
            app.lexicon_filter.origin = origin;
            app.lexicon_filter.search = search;
            app.lexicon_dirty = true;
        }
    });
    ui.horizontal(|ui| {
        let mut term = app.lexicon_new_term.clone();
        let submitted = ui
            .add(
                egui::TextEdit::singleline(&mut term)
                    .hint_text("手工加词")
                    .desired_width(140.0),
            )
            .lost_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter));
        app.lexicon_new_term = term;
        if (ui.button("加入").clicked() || submitted) && !app.lexicon_new_term.trim().is_empty() {
            app.add_lexicon_term();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if app.lexicon_clear_confirm {
                ui.colored_label(
                    theme::warn(),
                    "确定清空？词条、拒绝记录与扫描记账都会没有。",
                );
                if ui.button("确定清空").clicked() {
                    app.clear_lexicon();
                    app.lexicon_clear_confirm = false;
                }
                if ui.button("取消").clicked() {
                    app.lexicon_clear_confirm = false;
                }
            } else {
                if ui.button("清空词表").clicked() {
                    app.lexicon_clear_confirm = true;
                }
                if ui
                    .button("接受列出的全部")
                    .on_hover_text("把当前筛选结果里还没接受的词一次接受掉")
                    .clicked()
                {
                    app.accept_listed_lexicon_terms();
                }
            }
        });
    });
}

fn term_table(app: &mut GongwenApp, ui: &mut egui::Ui) {
    const ROW_HEIGHT: f32 = 26.0;
    if app.lexicon_terms.is_empty() {
        ui.weak("还没有词条。先点右上角「扫描」，从已发布与已归档的稿件里累积。");
        return;
    }
    // 逐行动作先收集，循环里直接改会和表格借用打架。
    let mut action: Option<(i64, TermState)> = None;
    let mut reading: Option<(i64, String, String)> = None;

    TableBuilder::new(ui)
        .id_salt("lexicon_table")
        .striped(true)
        .resizable(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::initial(160.0).at_least(90.0)) // 词
        .column(Column::initial(52.0).at_least(44.0)) // 篇数
        .column(Column::initial(52.0).at_least(44.0)) // 词频
        .column(Column::initial(52.0).at_least(44.0)) // 省键
        .column(Column::initial(72.0).at_least(56.0)) // 编码
        .column(Column::initial(72.0).at_least(56.0)) // 来源
        .column(Column::initial(60.0).at_least(52.0)) // 状态
        .column(Column::remainder().at_least(150.0).resizable(false)) // 操作
        .header(ROW_HEIGHT, |mut header| {
            for title in ["词", "篇数", "词频", "省键", "编码", "来源", "状态"] {
                header.col(|ui| {
                    ui.strong(title);
                });
            }
            header.col(|ui| {
                ui.strong("操作");
            });
        })
        .body(|body| {
            // 整份克隆会在每帧多出上千次分配；借走再放回既不分配也不和表格抢借用。
            let rows = std::mem::take(&mut app.lexicon_terms);
            body.rows(ROW_HEIGHT, rows.len(), |mut row| {
                let term = &rows[row.index()];
                row.col(|ui| {
                    ui.label(&term.term).on_hover_text(details(term));
                    if term.locked {
                        ui.weak("·锁")
                            .on_hover_text("读音或短码是人工设过的，重扫不会覆盖");
                    }
                    if term.in_base_dict {
                        ui.weak("·通")
                            .on_hover_text("jieba 自带词典里就有这个词，输入法多半也有，收益存疑");
                    }
                });
                row.col(|ui| {
                    ui.label(term.doc_count.to_string());
                });
                row.col(|ui| {
                    ui.label(term.freq_total.to_string());
                });
                row.col(|ui| {
                    let saved = term.saved_keys();
                    if saved == 0 {
                        ui.weak("0")
                            .on_hover_text("二字及以下在小鹤里本来就是四码，省 0 键");
                    } else {
                        ui.label(saved.to_string());
                    }
                });
                row.col(|ui| match term.code() {
                    Ok(code) => {
                        let text = if term.code_override.trim().is_empty() {
                            egui::RichText::new(code)
                        } else {
                            egui::RichText::new(code).color(theme::accent())
                        };
                        ui.label(text);
                    }
                    Err(error) => {
                        ui.colored_label(theme::warn(), "出错")
                            .on_hover_text(error.to_string());
                    }
                });
                row.col(|ui| {
                    ui.weak(term.origin.label());
                });
                row.col(|ui| {
                    let label = term.state.label();
                    match term.state {
                        TermState::Accepted => {
                            ui.colored_label(theme::accent(), label);
                        }
                        TermState::Rejected => {
                            ui.weak(label);
                        }
                        TermState::Candidate => {
                            ui.label(label);
                        }
                    };
                });
                row.col(|ui| {
                    if term.state != TermState::Accepted && ui.small_button("接受").clicked() {
                        action = Some((term.id, TermState::Accepted));
                    }
                    if term.state != TermState::Rejected
                        && ui
                            .small_button("拒绝")
                            .on_hover_text("永久记住：以后重扫不会再把它翻出来")
                            .clicked()
                    {
                        action = Some((term.id, TermState::Rejected));
                    }
                    let mut pinyin = term.pinyin.clone();
                    let mut code = term.code_override.clone();
                    let pinyin_changed = ui
                        .add(
                            egui::TextEdit::singleline(&mut pinyin)
                                .hint_text("标音")
                                .desired_width(78.0),
                        )
                        .on_hover_text("多音字的读音，如「chong qing shi」。改过就锁定，重扫不覆盖")
                        .lost_focus();
                    let code_changed = ui
                        .add(
                            egui::TextEdit::singleline(&mut code)
                                .hint_text("短码")
                                .desired_width(56.0),
                        )
                        .on_hover_text("给天天要打的长词配个自定义短码，留空则按四码规则")
                        .lost_focus();
                    if (pinyin_changed && pinyin != term.pinyin)
                        || (code_changed && code != term.code_override)
                    {
                        reading = Some((term.id, pinyin, code));
                    }
                });
            });
            app.lexicon_terms = rows;
        });

    if let Some((id, state)) = action {
        app.set_lexicon_state(id, state);
    }
    if let Some((id, pinyin, code)) = reading {
        app.set_lexicon_reading(id, &pinyin, &code);
    }
}

/// 鼠标停在词上时显示的细节：分组、备注与首末次出现的时间。
fn details(term: &crate::lexicon::LexiconTerm) -> String {
    let mut lines = Vec::new();
    if !term.group_name.trim().is_empty() {
        lines.push(format!("分组：{}", term.group_name));
    }
    if !term.note.trim().is_empty() {
        lines.push(format!("备注：{}", term.note));
    }
    if !term.first_seen.is_empty() {
        lines.push(format!("首次收录：{}", date_of(&term.first_seen)));
    }
    if !term.last_seen.is_empty() {
        lines.push(format!("最近出现：{}", date_of(&term.last_seen)));
    }
    lines.join("\n")
}

/// RFC3339 时间戳只取到日，词表这里不需要精确到秒。
fn date_of(timestamp: &str) -> &str {
    timestamp.split('T').next().unwrap_or(timestamp)
}

/// 口径说明。改口径后点一次普通「扫描」即可：移出口径的来源会在那一轮
/// 被清账，新进口径的来源没有记账、会照常扫到。
pub(crate) fn scan_scope_hint(options: &scan::ScanOptions) -> &'static str {
    if options.include_drafts {
        "当前口径包含草稿。改动口径后点一次「扫描」即可生效。"
    } else {
        "当前只统计已发布与已归档的稿件。改动口径后点一次「扫描」即可生效。"
    }
}
