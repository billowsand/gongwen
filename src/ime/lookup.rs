//! 查码：输入字词看它的全部编码、最短码与逐字码；输入编码看该码全部候选与后续编码。
//!
//! 浮窗（正文右键「查编码」）与词表页的「查码」标签共用 [`Ime::lookup_view`]。
use super::session::Ime;
use crate::theme;
use eframe::egui;

/// 编码查询时最多列几个以它开头的后续编码。
const MAX_EXTENDED: usize = 30;

/// 逐字查码最多查几个字。
const MAX_CHARS: usize = 12;

#[derive(Default)]
pub(super) struct Lookup {
    open: bool,
    pub query: String,
    focus: bool,
}

impl Ime {
    pub(crate) fn open_lookup(&mut self, text: &str) {
        self.lookup.open = true;
        self.lookup.query = text.trim().chars().take(40).collect();
        self.lookup.focus = true;
    }

    pub(super) fn lookup_window(&mut self, ctx: &egui::Context) {
        if !self.lookup.open {
            return;
        }
        let mut open = true;
        egui::Window::new("查编码")
            .id(egui::Id::new("ime-lookup-window"))
            .open(&mut open)
            .collapsible(false)
            .default_width(360.0)
            .show(ctx, |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.lookup.query)
                        .hint_text("字词；查编码先按 Shift 切英文")
                        .desired_width(f32::INFINITY),
                );
                if self.lookup.focus {
                    response.request_focus();
                    self.lookup.focus = false;
                }
                ui.separator();
                let query = self.lookup.query.clone();
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| self.lookup_view(ui, &query));
            });
        if !open {
            self.lookup.open = false;
        }
    }

    /// 查码结果。字母按编码查，其余按字词查。
    pub(crate) fn lookup_view(&mut self, ui: &mut egui::Ui, query: &str) {
        let query = query.trim();
        if query.is_empty() {
            ui.weak("输入字词查编码，输入小写字母查候选。");
            return;
        }
        if query.bytes().all(|b| b.is_ascii_lowercase()) {
            self.code_view(ui, query);
        } else {
            self.text_view(ui, query);
        }
    }

    fn code_view(&mut self, ui: &mut egui::Ui, code: &str) {
        let all = self.table.all(code).to_vec();
        if all.is_empty() {
            ui.label(format!("{code} 是空码。"));
        } else {
            ui.label(format!("{code} 的候选："));
            let mut position = 0;
            for candidate in &all {
                let hidden = self.table.hidden(code, &candidate.text);
                ui.horizontal(|ui| {
                    if hidden {
                        ui.weak("  ·");
                    } else {
                        position += 1;
                        ui.monospace(format!("{position:>3}"));
                    }
                    let text = egui::RichText::new(&candidate.text);
                    ui.label(if hidden {
                        text.strikethrough().weak()
                    } else {
                        text
                    });
                    ui.weak(candidate.sources.join("、"));
                    if hidden {
                        ui.weak("已屏蔽");
                    }
                });
            }
        }
        let extended: Vec<(String, String)> = self
            .table
            .extended(code, MAX_EXTENDED)
            .into_iter()
            .map(|(code, candidate)| (code, candidate.text))
            .collect();
        if !extended.is_empty() {
            ui.add_space(4.0);
            ui.weak("以它开头的编码：");
            ui.horizontal_wrapped(|ui| {
                for (full, text) in extended {
                    ui.label(
                        egui::RichText::new(format!("{text} {full}"))
                            .size(theme::font_sizes::SMALL),
                    );
                }
            });
        }
    }

    fn text_view(&mut self, ui: &mut egui::Ui, text: &str) {
        let codes = self.table.codes_of(text);
        if codes.is_empty() {
            ui.label(format!("「{text}」不在词表中。"));
            if let Ok(suggestions) = self.encoder.encode(text) {
                let list: Vec<&str> = suggestions
                    .iter()
                    .take(4)
                    .map(|s| s.code.as_str())
                    .collect();
                ui.weak(format!("按构词规则可编：{}", list.join("、")));
            }
        } else {
            let shortest = codes.iter().map(|(code, ..)| code.len()).min().unwrap_or(0);
            for (code, position, sources) in &codes {
                ui.horizontal(|ui| {
                    ui.monospace(format!("{code:<4}"));
                    ui.label(format!("第 {position} 位"));
                    ui.weak(sources.join("、"));
                    if code.len() == shortest && codes.len() > 1 {
                        ui.colored_label(theme::accent(), "最短");
                    }
                });
            }
            if let Some(false) = codes
                .iter()
                .find(|(code, ..)| code.len() == 4)
                .and_then(|(code, ..)| self.encoder.conforms(text, code))
            {
                ui.colored_label(theme::warn(), "四码与构词规则推得的码不符。");
            }
        }
        let chars: Vec<char> = text.chars().take(MAX_CHARS).collect();
        if chars.len() > 1 {
            ui.add_space(4.0);
            ui.weak("逐字：");
            for c in chars {
                let codes: Vec<String> = self
                    .table
                    .codes_of(&c.to_string())
                    .into_iter()
                    .map(|(code, position, _)| {
                        if position == 1 {
                            code
                        } else {
                            format!("{code}({position})")
                        }
                    })
                    .collect();
                ui.horizontal_wrapped(|ui| {
                    ui.label(c.to_string());
                    if codes.is_empty() {
                        ui.weak("无编码");
                    } else {
                        ui.monospace(codes.join("  "));
                    }
                });
            }
        }
        if !text.contains(['\t', '\r', '\n']) && ui.button("加入词表…").clicked() {
            self.open_add_word(text, None);
        }
    }
}
