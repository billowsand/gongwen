//! 输入法词表页：整页表格快速增删改，检查结果直接处理，误操作可撤销。
mod checks;
mod quick;
use super::{
    session::Ime,
    table::{Entry, PERSONAL},
};
use crate::theme;
use eframe::egui;

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
    fn label(self) -> &'static str {
        match self {
            Tab::Browse => "词条",
            Tab::Lookup => "查码",
            Tab::Sources => "导入与来源",
            Tab::Health => "检查",
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
    search: String,
    filters_open: bool,
    checked: std::collections::BTreeSet<(String, String)>,
    editing: Option<Entry>,
    edit_focus: bool,
    edit_focus_code: bool,
    details_open: bool,
    search_focus: bool,
    health_checked: std::collections::BTreeSet<(String, String)>,
    health_ignored: std::collections::BTreeSet<(String, String, String)>,
    health_filter: usize,
    health_editing: Option<Entry>,
    health_edit_code: String,
    health_edit_focus: bool,
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
    /// 页上登记的、交给应用层去办的事（开文件框一类）。帧末取走。
    pub request: Option<PageAction>,
}

/// 页上要交给应用层做的事。
pub(crate) enum PageAction {
    SyncLexicon,
    /// 导入词表：应用层开文件框，选好后交给输入法。
    ImportWordTable {
        four: bool,
    },
    /// 恢复个人设置备份：应用层开文件框，选好后交给输入法。
    RestoreWordTableBackup,
    /// 导出当前生效词表或筛选结果。
    ExportWordTable {
        filtered: bool,
    },
    /// 导出个人设置（词条、屏蔽、排序与构词规则）。
    ExportPersonalSettings,
    /// 导出最终生效词表。
    ExportFinalWordTable,
    /// 导出个人层备份。
    ExportPersonalBackup,
}

impl Ime {
    /// 输入法词表页。
    pub(crate) fn page_ui(&mut self, ui: &mut egui::Ui) -> Option<PageAction> {
        self.refresh_manager();
        if self.manager.dirty {
            self.page.stale = true;
            self.page.health = None;
        }
        // 先消费旧标记：本帧的保存/撤销不能在帧尾被清掉。
        self.manager.dirty = false;
        let margin = if theme::smartisan::active() {
            0
        } else if ui.available_width() < 900.0 {
            20
        } else {
            28
        };
        let mut action = None;
        egui::Frame::new()
            .inner_margin(egui::Margin::same(margin))
            .show(ui, |ui| {
                action = self.quick_header(ui);
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    for tab in [Tab::Browse, Tab::Health] {
                        if theme::ribbon_tab_button(ui, self.page.tab == tab, tab.label(), false)
                            .clicked()
                        {
                            self.page.tab = tab;
                        }
                    }
                    ui.menu_button("更多", |ui| {
                        for tab in [Tab::Lookup, Tab::Sources, Tab::History] {
                            if ui.button(tab.label()).clicked() {
                                self.page.tab = tab;
                                ui.close();
                            }
                        }
                        ui.separator();
                        if ui.button("同步公文词表").clicked() {
                            action = Some(PageAction::SyncLexicon);
                            ui.close();
                        }
                    });
                });
                ui.separator();
                self.quick_feedback(ui);
                match self.page.tab {
                    Tab::Browse => self.browse_tab(ui),
                    Tab::Lookup => self.lookup_tab(ui),
                    Tab::Sources => {
                        egui::ScrollArea::vertical()
                            .id_salt("ime_sources")
                            .show(ui, |ui| {
                                self.import_section(ui);
                                self.batches_section(ui);
                                self.rules_summary(ui);
                            });
                    }
                    Tab::Health => self.health_tab(ui),
                    Tab::History => self.history_section(ui),
                }
            });
        self.quick_details(ui.ctx());
        // 页上登记的请求（开文件框一类）交给应用层去办。
        if let Some(request) = self.page.request.take() {
            action = Some(request);
        }
        action
    }

    /// 打开词表页并切到查码，预填文字。
    pub(crate) fn show_lookup_tab(&mut self, text: &str) {
        self.page.tab = Tab::Lookup;
        self.page.lookup_text = text.to_string();
        self.page.lookup_code.clear();
    }

    fn browse_tab(&mut self, ui: &mut egui::Ui) {
        if self.page.filters_open {
            self.filter_bar(ui);
        }
        if self.page.stale || !self.page.ready {
            self.page.rows = self.filtered_rows();
            self.page.stale = false;
            self.page.ready = true;
            let visible: std::collections::BTreeSet<_> = self
                .page
                .rows
                .iter()
                .map(|r| (r.code.clone(), r.text.clone()))
                .collect();
            self.page.checked.retain(|key| visible.contains(key));
            if self
                .page
                .selected
                .as_ref()
                .is_some_and(|entry| !visible.contains(&(entry.code.clone(), entry.text.clone())))
            {
                self.page.selected = None;
            }
        }
        self.quick_table(ui);
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
        let search = page.search.trim();
        let code_search =
            !search.is_empty() && search.bytes().all(|b| b.is_ascii_alphabetic() || b == b'?');
        let pattern = if code_search {
            search
        } else {
            page.code.trim()
        }
        .to_ascii_lowercase();
        let wildcard = pattern.contains('?');
        let prefix = if wildcard {
            pattern.split('?').next().unwrap_or_default().to_string()
        } else {
            pattern.clone()
        };
        let text = if !search.is_empty() && !code_search {
            search
        } else {
            page.text.trim()
        };
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
                if hidden && page.source != Source::Hidden {
                    continue;
                }
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
    fn quick_search_hides_removed_words_and_handles_code_prefixes() {
        let mut ime = Ime::bare(super::super::ImeSettings::default());
        ime.table.base =
            table::parse("ab,1=甲\nabcd,1=工作\nabce,1=工作报告\nxy,1=乙", false).entries;
        ime.table.personal.hidden = vec![Entry {
            code: "abce".into(),
            text: "工作报告".into(),
        }];
        ime.table.rebuild();
        ime.page.search = "工作".into();
        assert_eq!(ime.filtered_rows().len(), 1);
        ime.page.search = "AB?D".into();
        assert_eq!(ime.filtered_rows()[0].text, "工作");
        ime.page.search = "ab".into();
        assert_eq!(ime.filtered_rows().len(), 2);
        ime.page.source = Source::Hidden;
        assert_eq!(ime.filtered_rows()[0].text, "工作报告");
    }

    #[test]
    fn delete_shortcut_never_removes_words_while_search_has_focus() {
        use egui_kittest::Harness;
        let mut ime = Ime::bare(super::super::ImeSettings::default());
        ime.table.base = table::parse("ab,1=甲", false).entries;
        ime.table.rebuild();
        ime.page.checked.insert(("ab".into(), "甲".into()));
        let mut harness = Harness::new_ui_state(
            |ui, ime: &mut Ime| {
                ime.page_ui(ui);
            },
            ime,
        );
        harness.run();
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::F);
        harness.run();
        harness.key_press(egui::Key::Delete);
        harness.run();
        assert!(harness.state().table.personal.hidden.is_empty());
        assert_eq!(harness.state().page.checked.len(), 1);
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
        for tab in ["检查", "词条"] {
            // 详情面板里也有一个「查码」按钮，标签页按钮画在前面。
            harness.get_all_by_label(tab).next().unwrap().click();
            harness.run();
        }
        assert_eq!(harness.state().page.tab, Tab::Browse);
    }

    #[test]
    #[ignore = "本机基础词表不入库"]
    fn real_base_table_filters_quickly() {
        let path = crate::ime::data::development_base_path().expect("本机基础词表不存在");
        let mut ime = Ime::bare(super::super::ImeSettings::default());
        ime.table.base =
            table::parse(&crate::text_file::read_to_string(&path).unwrap(), false).entries;
        ime.table.rebuild();
        ime.rebuild_encoder();
        let started = std::time::Instant::now();
        let rows = ime.filtered_rows();
        let health = ime.compute_health();
        let elapsed = started.elapsed();
        // 2026-10-24 第二轮清理后全表 51,269 条（原 57,874 条），下限随实际规模下调。
        assert!(rows.len() >= 50_000);
        assert!(health.nonconforming.is_empty());
        println!("全表 {} 行与体检用时 {elapsed:?}", rows.len());
    }
}
