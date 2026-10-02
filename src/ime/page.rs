//! 「输入法词表」页：查询与维护、查码、导入与来源、体检、修改记录。
//!
//! 查询用两个框：编码框只收字母（不走输入法，支持前缀与 `?` 通配），文字框走输入法。
//! 结果表虚拟滚动，不设条数上限；点一行在右侧看详情、改词、调序、屏蔽。
use super::{
    session::Ime,
    table::{Entry, PERSONAL},
};
use crate::theme;
use eframe::egui;
use egui_extras::{Column, TableBuilder};

/// 体检里每类最多列几条。
const MAX_HEALTH_ROWS: usize = 200;

/// 重码多于这么多个才算「高重码组」。
const CROWDED: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Tab {
    #[default]
    Browse,
    Lookup,
    Sources,
    Health,
    History,
}

impl Tab {
    const ALL: [Tab; 5] = [
        Tab::Browse,
        Tab::Lookup,
        Tab::Sources,
        Tab::Health,
        Tab::History,
    ];

    fn label(self) -> &'static str {
        match self {
            Tab::Browse => "查询与维护",
            Tab::Lookup => "查码",
            Tab::Sources => "导入与来源",
            Tab::Health => "体检",
            Tab::History => "修改记录",
        }
    }
}

/// 来源筛选。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum Source {
    #[default]
    All,
    Named(String),
    Hidden,
    Ordered,
}

impl Source {
    fn label(&self) -> String {
        match self {
            Source::All => "全部来源".into(),
            Source::Named(name) => name.clone(),
            Source::Hidden => "已屏蔽".into(),
            Source::Ordered => "调过顺序".into(),
        }
    }
}

/// 结果表的一行。
#[derive(Debug, Clone)]
struct Row {
    code: String,
    text: String,
    sources: Vec<String>,
    /// 在该码有效候选中的位置；屏蔽的为 None。
    position: Option<usize>,
}

/// 体检结果。
#[derive(Debug, Clone, Default)]
struct Health {
    /// 非基础表词组的编码与规则不符：词条与建议码。
    nonconforming: Vec<(Entry, String, Vec<String>)>,
    /// 同一个词有多个四码，且至少一个不是基础表的。
    multi_code: Vec<(String, Vec<String>)>,
    /// 候选多于 `CROWDED` 个的编码。
    crowded: Vec<(String, usize)>,
    /// 非基础表词组里没有字码的字。
    uncoded: Vec<char>,
}

#[derive(Default)]
pub(super) struct Page {
    pub tab: Tab,
    code: String,
    text: String,
    source: Source,
    code_len: usize,
    word_len: usize,
    conflicts: bool,
    nonconforming: bool,
    rows: Vec<Row>,
    stale: bool,
    /// 结果表算过至少一次。
    ready: bool,
    selected: Option<Entry>,
    edit_code: String,
    edit_text: String,
    lookup_code: String,
    lookup_text: String,
    health: Option<Health>,
}

/// 页上要交给应用层做的事。
pub(crate) enum PageAction {
    SyncLexicon,
}

impl Ime {
    /// 输入法词表页。
    pub(crate) fn page_ui(&mut self, ui: &mut egui::Ui) -> Option<PageAction> {
        self.refresh_manager();
        if self.manager.dirty {
            self.page.stale = true;
            self.page.health = None;
        }
        let mut action = None;
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.heading("输入法词表");
            ui.add_space(6.0);
            let brief = self.data_brief().unwrap_or_else(|| "未导入基础表".into());
            theme::chip(ui, &brief, theme::text_muted(), theme::surface_sunk());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(theme::icon_text_button(
                        theme::Icon::Refresh,
                        "同步公文词表",
                    ))
                    .on_hover_text("把公文词表里已接受且为四码的词交给输入法")
                    .clicked()
                {
                    action = Some(PageAction::SyncLexicon);
                }
                if ui
                    .add(theme::icon_text_button(theme::Icon::FilePlus, "加词"))
                    .on_hover_text("按构词规则自动出码，也可用 Ctrl+Shift+A 从正文造词")
                    .clicked()
                {
                    self.open_add_word("", None);
                }
            });
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for tab in Tab::ALL {
                if theme::ribbon_tab_button(ui, self.page.tab == tab, tab.label()).clicked() {
                    self.page.tab = tab;
                }
            }
        });
        ui.separator();
        self.message_ui(ui);
        match self.page.tab {
            Tab::Browse => self.browse_tab(ui),
            Tab::Lookup => self.lookup_tab(ui),
            Tab::Sources => {
                egui::ScrollArea::vertical()
                    .id_salt("ime_sources")
                    .show(ui, |ui| {
                        self.import_section(ui);
                        self.batches_section(ui);
                        ui.weak("公文词表中已接受且编码为四码的词自动加载，也可点右上角立即同步。");
                        self.rules_summary(ui);
                    });
            }
            Tab::Health => self.health_tab(ui),
            Tab::History => self.history_section(ui),
        }
        // 管理分区与本页都看过了，词表变动的标记到这里清掉。
        self.manager.dirty = false;
        action
    }

    /// 打开词表页并切到查码，预填文字。
    pub(crate) fn show_lookup_tab(&mut self, text: &str) {
        self.page.tab = Tab::Lookup;
        self.page.lookup_text = text.to_string();
        self.page.lookup_code.clear();
    }

    fn browse_tab(&mut self, ui: &mut egui::Ui) {
        self.filter_bar(ui);
        if self.page.stale || !self.page.ready {
            self.page.rows = self.filtered_rows();
            self.page.stale = false;
            self.page.ready = true;
        }
        ui.add_space(4.0);
        let detail_width = 330.0;
        let table_width = (ui.available_width() - detail_width - 12.0).max(320.0);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(table_width);
                self.results_table(ui);
            });
            ui.separator();
            ui.vertical(|ui| {
                ui.set_width(detail_width);
                egui::ScrollArea::vertical()
                    .id_salt("ime_detail")
                    .show(ui, |ui| self.detail_panel(ui));
            });
        });
    }

    fn filter_bar(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            ui.label("编码");
            changed |= super::exempt(
                ui.add(
                    egui::TextEdit::singleline(&mut self.page.code)
                        .hint_text("前缀，? 通配")
                        .desired_width(90.0)
                        .font(egui::TextStyle::Monospace),
                ),
            )
            .changed();
            ui.label("文字");
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut self.page.text)
                        .hint_text("包含")
                        .desired_width(120.0),
                )
                .changed();
            let mut sources = vec![
                Source::All,
                Source::Named("基础表".into()),
                Source::Named("公文词表".into()),
            ];
            sources.extend(
                self.table
                    .personal
                    .batches
                    .iter()
                    .map(|b| Source::Named(b.name.clone())),
            );
            sources.extend([
                Source::Named(PERSONAL.into()),
                Source::Hidden,
                Source::Ordered,
            ]);
            egui::ComboBox::from_id_salt("ime_source_filter")
                .selected_text(self.page.source.label())
                .show_ui(ui, |ui| {
                    for source in sources {
                        let label = source.label();
                        changed |= ui
                            .selectable_value(&mut self.page.source, source, label)
                            .changed();
                    }
                });
            egui::ComboBox::from_id_salt("ime_code_len")
                .selected_text(match self.page.code_len {
                    0 => "码长不限".to_string(),
                    n => format!("{n} 码"),
                })
                .show_ui(ui, |ui| {
                    for n in 0..=4 {
                        let label = if n == 0 {
                            "码长不限".to_string()
                        } else {
                            format!("{n} 码")
                        };
                        changed |= ui
                            .selectable_value(&mut self.page.code_len, n, label)
                            .changed();
                    }
                });
            egui::ComboBox::from_id_salt("ime_word_len")
                .selected_text(word_len_label(self.page.word_len))
                .show_ui(ui, |ui| {
                    for n in 0..=4 {
                        changed |= ui
                            .selectable_value(&mut self.page.word_len, n, word_len_label(n))
                            .changed();
                    }
                });
            changed |= ui.checkbox(&mut self.page.conflicts, "只看重码").changed();
            changed |= ui
                .checkbox(&mut self.page.nonconforming, "只看不符规则")
                .on_hover_text("公文词表、导入表与个人词条里编码与构词规则推得的码不符的词组")
                .changed();
        });
        if changed {
            self.page.stale = true;
        }
    }

    /// 按筛选条件扫一遍词表。只在条件或词表变化时调用。
    fn filtered_rows(&self) -> Vec<Row> {
        let page = &self.page;
        let pattern = page.code.trim().to_ascii_lowercase();
        let wildcard = pattern.contains('?');
        let prefix = if wildcard {
            pattern.split('?').next().unwrap_or_default().to_string()
        } else {
            pattern.clone()
        };
        let text = page.text.trim();
        let mut rows = Vec::new();
        for (code, candidates) in self.table.codes_from(&prefix) {
            if wildcard
                && (code.len() != pattern.len()
                    || !code
                        .bytes()
                        .zip(pattern.bytes())
                        .all(|(c, p)| p == b'?' || c == p))
            {
                continue;
            }
            if page.code_len != 0 && code.len() != page.code_len {
                continue;
            }
            if page.source == Source::Ordered && !self.table.personal.order.contains_key(code) {
                continue;
            }
            let visible = candidates
                .iter()
                .filter(|c| !self.table.hidden(code, &c.text))
                .count();
            if page.conflicts && visible < 2 {
                continue;
            }
            let mut position = 0;
            for candidate in candidates {
                let hidden = self.table.hidden(code, &candidate.text);
                if !hidden {
                    position += 1;
                }
                if !text.is_empty() && !candidate.text.contains(text) {
                    continue;
                }
                let chars = candidate.text.chars().count();
                if page.word_len != 0 && chars.min(4) != page.word_len {
                    continue;
                }
                let source_ok = match &page.source {
                    Source::All | Source::Ordered => true,
                    Source::Named(name) => candidate.sources.iter().any(|s| s == name),
                    Source::Hidden => hidden,
                };
                if !source_ok {
                    continue;
                }
                if page.nonconforming
                    && !(chars >= 2
                        && candidate.sources.iter().any(|s| s != "基础表")
                        && self.encoder.conforms(&candidate.text, code) == Some(false))
                {
                    continue;
                }
                rows.push(Row {
                    code: code.clone(),
                    text: candidate.text.clone(),
                    sources: candidate.sources.clone(),
                    position: (!hidden).then_some(position),
                });
            }
        }
        rows
    }

    fn results_table(&mut self, ui: &mut egui::Ui) {
        const ROW_HEIGHT: f32 = 24.0;
        ui.weak(format!("共 {} 条", self.page.rows.len()));
        let rows = std::mem::take(&mut self.page.rows);
        let mut clicked = None;
        TableBuilder::new(ui)
            .id_salt("ime_table_rows")
            .striped(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(60.0).at_least(48.0))
            .column(Column::initial(160.0).at_least(80.0))
            .column(Column::initial(48.0).at_least(40.0))
            .column(Column::remainder().at_least(100.0))
            .header(ROW_HEIGHT, |mut header| {
                for title in ["编码", "文字", "位次", "来源"] {
                    header.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(ROW_HEIGHT, rows.len(), |mut row| {
                    let data = &rows[row.index()];
                    let selected = self
                        .page
                        .selected
                        .as_ref()
                        .is_some_and(|e| e.code == data.code && e.text == data.text);
                    row.set_selected(selected);
                    row.col(|ui| {
                        ui.monospace(&data.code);
                    });
                    row.col(|ui| {
                        let text = egui::RichText::new(&data.text);
                        ui.add(
                            egui::Label::new(if data.position.is_none() {
                                text.strikethrough().weak()
                            } else {
                                text
                            })
                            .truncate()
                            .selectable(false),
                        );
                    });
                    row.col(|ui| match data.position {
                        Some(position) => {
                            ui.label(position.to_string());
                        }
                        None => {
                            ui.weak("屏蔽");
                        }
                    });
                    row.col(|ui| {
                        ui.add(
                            egui::Label::new(egui::RichText::new(data.sources.join("、")).weak())
                                .truncate()
                                .selectable(false),
                        );
                    });
                    if row.response().clicked() {
                        clicked = Some(Entry {
                            code: data.code.clone(),
                            text: data.text.clone(),
                        });
                    }
                });
            });
        self.page.rows = rows;
        if let Some(entry) = clicked {
            self.page.edit_code = entry.code.clone();
            self.page.edit_text = entry.text.clone();
            self.page.selected = Some(entry);
        }
    }

    /// 右侧详情：该码全部候选、该词全部编码、规则码与修改。
    fn detail_panel(&mut self, ui: &mut egui::Ui) {
        let Some(entry) = self.page.selected.clone() else {
            ui.weak("点左侧一行查看详情。");
            return;
        };
        ui.horizontal(|ui| {
            ui.heading(&entry.text);
            ui.monospace(&entry.code);
        });
        let all = self.table.all(&entry.code).to_vec();
        let Some(current) = all.iter().find(|c| c.text == entry.text).cloned() else {
            ui.weak("这条词已不在词表中。");
            return;
        };
        let hidden = self.table.hidden(&entry.code, &entry.text);
        ui.weak(format!("来源：{}", current.sources.join("、")));
        let mut result: Option<(anyhow::Result<()>, &str)> = None;

        ui.add_space(6.0);
        ui.strong(format!("{} 的全部候选", entry.code));
        let ordered = self.table.personal.order.contains_key(&entry.code);
        let mut position = 0;
        for candidate in &all {
            let is_hidden = self.table.hidden(&entry.code, &candidate.text);
            ui.horizontal(|ui| {
                if is_hidden {
                    ui.weak("  ·");
                } else {
                    position += 1;
                    ui.monospace(format!("{position:>2}"));
                }
                let text = egui::RichText::new(&candidate.text);
                let text = if candidate.text == entry.text {
                    text.strong()
                } else {
                    text
                };
                ui.label(if is_hidden {
                    text.strikethrough().weak()
                } else {
                    text
                });
                if ui.small_button("置顶").clicked() {
                    result = Some((
                        self.move_word(&entry.code, &candidate.text, 0),
                        "顺序已保存。",
                    ));
                }
                if ui.small_button("↑").clicked() {
                    result = Some((
                        self.move_word(&entry.code, &candidate.text, -1),
                        "顺序已保存。",
                    ));
                }
                if ui.small_button("↓").clicked() {
                    result = Some((
                        self.move_word(&entry.code, &candidate.text, 1),
                        "顺序已保存。",
                    ));
                }
            });
        }
        if ordered && ui.small_button("恢复默认顺序").clicked() {
            result = Some((self.reset_order(&entry.code), "已恢复默认顺序。"));
        }

        ui.add_space(6.0);
        ui.strong("这个词的全部编码");
        for (code, position, sources) in self.table.codes_of(&entry.text) {
            ui.horizontal(|ui| {
                ui.monospace(format!("{code:<4}"));
                ui.label(format!("第 {position} 位"));
                ui.weak(sources.join("、"));
            });
        }
        if let Ok(suggestions) = self.encoder.encode(&entry.text) {
            let list: Vec<&str> = suggestions
                .iter()
                .take(3)
                .map(|s| s.code.as_str())
                .collect();
            let conforms = suggestions.iter().any(|s| s.code == entry.code);
            ui.horizontal_wrapped(|ui| {
                ui.weak(format!("按规则：{}", list.join("、")));
                if !conforms && entry.code.len() == 4 {
                    ui.colored_label(theme::warn(), "当前编码不符规则");
                }
            });
        }

        ui.add_space(6.0);
        ui.strong("修改");
        ui.horizontal(|ui| {
            super::exempt(
                ui.add(
                    egui::TextEdit::singleline(&mut self.page.edit_code)
                        .desired_width(60.0)
                        .font(egui::TextStyle::Monospace),
                ),
            );
            ui.add(egui::TextEdit::singleline(&mut self.page.edit_text).desired_width(150.0));
            if ui.button("保存").clicked() {
                let next = Entry {
                    code: self.page.edit_code.trim().to_string(),
                    text: self.page.edit_text.trim().to_string(),
                };
                let saved = self.edit_entry(&entry, &next);
                if saved.is_ok() {
                    self.page.selected = Some(next);
                }
                result = Some((saved, "词条已修改。"));
            }
        });
        if current.sources.iter().any(|s| s != PERSONAL) {
            ui.weak("这条词来自其他来源：修改会屏蔽原词、另存一条个人词条。");
        }
        ui.horizontal_wrapped(|ui| {
            if ui.button(if hidden { "恢复" } else { "屏蔽" }).clicked() {
                result = Some((
                    self.set_hidden(&entry, !hidden),
                    if hidden {
                        "已恢复。"
                    } else {
                        "已屏蔽。"
                    },
                ));
            }
            if current.sources.iter().any(|s| s == PERSONAL) && ui.button("删除个人词条").clicked()
            {
                result = Some((self.remove_personal(&entry), "个人词条已删除。"));
            }
            if ui.button("查码").clicked() {
                self.show_lookup_tab(&entry.text);
            }
        });
        if let Some((outcome, success)) = result {
            self.report(outcome, success);
        }
    }

    fn lookup_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("字词");
            ui.add(
                egui::TextEdit::singleline(&mut self.page.lookup_text)
                    .hint_text("查它的编码")
                    .desired_width(200.0),
            );
            ui.label("编码");
            super::exempt(
                ui.add(
                    egui::TextEdit::singleline(&mut self.page.lookup_code)
                        .hint_text("查它的候选")
                        .desired_width(80.0)
                        .font(egui::TextStyle::Monospace),
                ),
            );
        });
        ui.separator();
        let text = self.page.lookup_text.clone();
        let code = self.page.lookup_code.trim().to_ascii_lowercase();
        egui::ScrollArea::vertical()
            .id_salt("ime_lookup_tab")
            .show(ui, |ui| {
                if !text.trim().is_empty() {
                    self.lookup_view(ui, &text);
                }
                if !code.is_empty() {
                    ui.add_space(8.0);
                    self.lookup_view(ui, &code);
                }
                if text.trim().is_empty() && code.is_empty() {
                    ui.weak(
                        "输入字词看它的全部编码与逐字码；输入编码看该码全部候选与以它开头的编码。",
                    );
                }
            });
    }

    fn health_tab(&mut self, ui: &mut egui::Ui) {
        if self.page.health.is_none() {
            self.page.health = Some(self.compute_health());
        }
        let health = self.page.health.clone().unwrap_or_default();
        egui::ScrollArea::vertical()
            .id_salt("ime_health")
            .show(ui, |ui| {
                self.overlay_section(ui);
                ui.add_space(8.0);
                let mut fix = None;
                egui::CollapsingHeader::new(format!(
                    "编码与构词规则不符（{}）",
                    health.nonconforming.len()
                ))
                .id_salt("health_nonconforming")
                .default_open(!health.nonconforming.is_empty())
                .show(ui, |ui| {
                    ui.weak("公文词表、导入表与个人词条里的词组。可能是手填错了，也可能是多音字。");
                    for (entry, suggestion, sources) in
                        health.nonconforming.iter().take(MAX_HEALTH_ROWS)
                    {
                        ui.horizontal(|ui| {
                            ui.monospace(&entry.code);
                            ui.label(&entry.text);
                            ui.weak(sources.join("、"));
                            ui.label(format!("→ {suggestion}"));
                            if ui.small_button("改为建议码").clicked() {
                                fix = Some((entry.clone(), suggestion.clone()));
                            }
                        });
                    }
                });
                egui::CollapsingHeader::new(format!("同词多个四码（{}）", health.multi_code.len()))
                    .id_salt("health_multi")
                    .show(ui, |ui| {
                        ui.weak("至少一个编码不是基础表的；多半是旧码残留，留下常用的一个。");
                        for (text, codes) in health.multi_code.iter().take(MAX_HEALTH_ROWS) {
                            ui.horizontal(|ui| {
                                ui.label(text);
                                ui.monospace(codes.join("  "));
                                if ui.small_button("查看").clicked() {
                                    self.show_lookup_tab(text);
                                }
                            });
                        }
                    });
                egui::CollapsingHeader::new(format!(
                    "高重码编码（{} 个，候选多于 {CROWDED} 个）",
                    health.crowded.len()
                ))
                .id_salt("health_crowded")
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for (code, count) in health.crowded.iter().take(MAX_HEALTH_ROWS) {
                            if ui
                                .small_button(format!("{code} ×{count}"))
                                .on_hover_text("在查询与维护里看这个码")
                                .clicked()
                            {
                                self.page.tab = Tab::Browse;
                                self.page.code = code.clone();
                                self.page.text.clear();
                                self.page.stale = true;
                            }
                        }
                    });
                });
                egui::CollapsingHeader::new(format!("没有字码的汉字（{}）", health.uncoded.len()))
                    .id_salt("health_uncoded")
                    .show(ui, |ui| {
                        ui.weak(
                            "这些字在基础表里没有两码以上的编码，含它们的词加词时无法自动出码。",
                        );
                        ui.label(health.uncoded.iter().collect::<String>());
                    });
                if let Some((entry, code)) = fix {
                    let next = Entry {
                        code,
                        text: entry.text.clone(),
                    };
                    let result = self.edit_entry(&entry, &next);
                    self.report(result, "已改为建议码。");
                }
            });
    }

    fn compute_health(&self) -> Health {
        let mut health = Health::default();
        let mut uncoded = std::collections::BTreeSet::new();
        let mut words: std::collections::HashMap<String, (Vec<String>, bool)> =
            std::collections::HashMap::new();
        for (code, candidates) in self.table.codes_from("") {
            let visible: Vec<_> = candidates
                .iter()
                .filter(|c| !self.table.hidden(code, &c.text))
                .collect();
            if code.len() == 4 && visible.len() > CROWDED {
                health.crowded.push((code.clone(), visible.len()));
            }
            for candidate in visible {
                let foreign = candidate.sources.iter().any(|s| s != "基础表");
                if code.len() == 4 {
                    let slot = words.entry(candidate.text.clone()).or_default();
                    slot.0.push(code.clone());
                    slot.1 |= foreign;
                }
                if !foreign || candidate.text.chars().count() < 2 {
                    continue;
                }
                match self.encoder.encode(&candidate.text) {
                    Ok(suggestions) => {
                        if !suggestions.iter().any(|s| &s.code == code)
                            && let Some(first) = suggestions.first()
                        {
                            health.nonconforming.push((
                                Entry {
                                    code: code.clone(),
                                    text: candidate.text.clone(),
                                },
                                first.code.clone(),
                                candidate.sources.clone(),
                            ));
                        }
                    }
                    Err(super::encoder::EncodeError::Missing(chars)) => uncoded.extend(chars),
                    Err(_) => {}
                }
            }
        }
        health.multi_code = words
            .into_iter()
            .filter(|(_, (codes, foreign))| codes.len() > 1 && *foreign)
            .map(|(text, (codes, _))| (text, codes))
            .collect();
        health.multi_code.sort();
        health
            .crowded
            .sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        health.uncoded = uncoded.into_iter().collect();
        health
    }
}

fn word_len_label(n: usize) -> &'static str {
    match n {
        0 => "字数不限",
        1 => "单字",
        2 => "二字",
        3 => "三字",
        _ => "四字及以上",
    }
}

#[cfg(test)]
mod tests {
    use super::super::table;
    use super::*;

    #[test]
    fn filters_by_wildcard_source_and_health() {
        let mut ime = Ime::bare(super::super::ImeSettings::default());
        ime.table.base = table::parse(
            "vi,1=只\nvifo,1=指\ndcs,1=导\ndcsc,1=导\nvidc,1=指导\nab,1=甲\nab,2=乙",
            false,
        )
        .entries;
        ime.table.personal.entries = table::parse("vidd,1=指导\nxxyy,1=鹤鹤", false).entries;
        ime.table.personal.hidden = table::parse("ab,2=乙", false).entries;
        ime.table.rebuild();
        ime.rebuild_encoder();
        ime.page.code = "vi?c".into();
        let rows = ime.filtered_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text, "指导");
        ime.page.code.clear();
        ime.page.source = Source::Hidden;
        let rows = ime.filtered_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].position, None);
        ime.page.source = Source::All;
        ime.page.nonconforming = true;
        let rows = ime.filtered_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].code, "vidd");
        let health = ime.compute_health();
        assert_eq!(health.nonconforming.len(), 1);
        assert_eq!(health.nonconforming[0].1, "vidc");
        assert_eq!(
            health.multi_code,
            [(
                "指导".to_string(),
                vec!["vidc".to_string(), "vidd".to_string()]
            )]
        );
        assert_eq!(health.uncoded, ['鹤']);
    }

    #[test]
    fn page_selects_rows_and_switches_tabs() {
        use egui_kittest::{Harness, kittest::Queryable};
        let mut ime = Ime::bare(super::super::ImeSettings::default());
        ime.table.base = table::parse(
            "ab,1=甲
ab,2=乙
vidc,1=指导",
            false,
        )
        .entries;
        ime.table.rebuild();
        ime.rebuild_encoder();
        let mut harness = Harness::new_ui_state(
            |ui, ime: &mut Ime| {
                ime.page_ui(ui);
            },
            ime,
        );
        harness.set_size(egui::vec2(1000.0, 700.0));
        harness.run();
        assert_eq!(harness.state().page.rows.len(), 3);
        harness.get_by_label("乙").click();
        harness.run();
        assert_eq!(
            harness.state().page.selected,
            Some(Entry {
                code: "ab".into(),
                text: "乙".into()
            })
        );
        for tab in ["查码", "导入与来源", "体检", "修改记录", "查询与维护"] {
            // 详情面板里也有一个「查码」按钮，标签页按钮画在前面。
            harness.get_all_by_label(tab).next().unwrap().click();
            harness.run();
        }
        assert_eq!(harness.state().page.tab, Tab::Browse);
    }

    #[test]
    #[ignore = "本机基础词表不入库"]
    fn real_base_table_filters_quickly() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fuma/quan.txt");
        let mut ime = Ime::bare(super::super::ImeSettings::default());
        ime.table.base =
            table::parse(&crate::text_file::read_to_string(&path).unwrap(), false).entries;
        ime.table.rebuild();
        ime.rebuild_encoder();
        let started = std::time::Instant::now();
        let rows = ime.filtered_rows();
        let health = ime.compute_health();
        let elapsed = started.elapsed();
        assert!(rows.len() >= 60_000);
        assert!(health.nonconforming.is_empty());
        println!("全表 {} 行与体检用时 {elapsed:?}", rows.len());
    }
}
