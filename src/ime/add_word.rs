//! 加词浮窗：按构词规则自动给码，列出其他可能码、该码现有候选与新词的位置。
//!
//! 入口三处：正文右键「加入词表」、造词快捷键（有选区取选区，没有取最近上屏的字）、
//! 查码结果里的「加入词表」。都只写个人词条，不改正文，也不改基础表。
use super::{
    encoder::{EncodeError, Suggestion},
    session::Ime,
    table::{self, Entry},
};
use crate::theme;
use eframe::egui;

/// 其他可能码最多列几个。
const MAX_ALTERNATIVES: usize = 6;

/// 造词时最多往回取几个字。
pub(super) const MAX_RECENT_CHARS: usize = 8;

pub(super) struct AddWord {
    open: bool,
    text: String,
    code: String,
    /// 编码跟着文字自动出；手改过或点过其他可能码后不再覆盖。
    auto: bool,
    /// 上次出码用的文字，文字一变就重算。
    computed_for: Option<String>,
    suggestions: Result<Vec<Suggestion>, EncodeError>,
    /// 造词时最近上屏的那一段汉字，可挑尾部几个字。
    recent: Option<String>,
    first: bool,
    message: String,
    focus_code: bool,
    /// 打开前焦点所在的编辑框，保存或关闭后还回去。
    return_focus: Option<egui::Id>,
}

impl Default for AddWord {
    fn default() -> Self {
        Self {
            open: false,
            text: String::new(),
            code: String::new(),
            auto: true,
            computed_for: None,
            suggestions: Err(EncodeError::Empty),
            recent: None,
            first: false,
            message: String::new(),
            focus_code: false,
            return_focus: None,
        }
    }
}

impl Ime {
    /// 打开加词窗。`recent` 为造词时最近上屏的整段汉字。
    pub(crate) fn open_add_word(&mut self, text: &str, recent: Option<String>) {
        let state = &mut self.add_word;
        state.open = true;
        state.text = text.trim().to_string();
        state.code.clear();
        state.auto = true;
        state.computed_for = None;
        state.first = false;
        state.message.clear();
        state.recent = recent.filter(|r| r.chars().count() >= 2);
        state.focus_code = true;
        state.return_focus = self.focus_id;
    }

    /// 造词：取最近上屏的一段汉字，默认用整段（最多八个字）。
    pub(crate) fn open_make_word(&mut self) -> bool {
        let recent = self.recent_han();
        if recent.chars().count() < 2 {
            return false;
        }
        self.open_add_word(&recent.clone(), Some(recent));
        true
    }

    /// 浮窗：加词与查码。正文右键菜单经临时存储发来请求。
    pub(crate) fn windows_ui(&mut self, ctx: &egui::Context) {
        if let Some(text) =
            ctx.data_mut(|data| data.remove_temp::<String>(egui::Id::new("ime-add-word")))
        {
            self.open_add_word(&text, None);
        }
        if let Some(text) =
            ctx.data_mut(|data| data.remove_temp::<String>(egui::Id::new("ime-lookup")))
        {
            self.open_lookup(&text);
        }
        self.add_word_window(ctx);
        self.lookup_window(ctx);
    }

    fn add_word_window(&mut self, ctx: &egui::Context) {
        if !self.add_word.open {
            return;
        }
        if self.add_word.computed_for.as_deref() != Some(self.add_word.text.as_str()) {
            self.recompute_code();
        }
        let mut open = true;
        let mut close = false;
        egui::Window::new("加入词表")
            .id(egui::Id::new("ime-add-word-window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .show(ctx, |ui| close = self.add_word_body(ui));
        if !open || close {
            self.add_word.open = false;
            if let Some(id) = self.add_word.return_focus.take() {
                ctx.memory_mut(|memory| memory.request_focus(id));
            }
        }
    }

    /// 窗体内容。返回 true 表示保存后关窗。
    fn add_word_body(&mut self, ui: &mut egui::Ui) -> bool {
        let mut close = false;
        egui::Grid::new("ime-add-word-grid")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("文字");
                ui.vertical(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.add_word.text).desired_width(220.0),
                    );
                    if let Some(recent) = self.add_word.recent.clone() {
                        self.recent_choices(ui, &recent);
                    }
                });
                ui.end_row();

                ui.label("编码");
                ui.horizontal(|ui| {
                    let response = super::exempt(
                        ui.add(
                            egui::TextEdit::singleline(&mut self.add_word.code)
                                .hint_text("1–4 个小写字母")
                                .desired_width(80.0)
                                .font(egui::TextStyle::Monospace),
                        ),
                    );
                    if self.add_word.focus_code {
                        response.request_focus();
                        self.add_word.focus_code = false;
                    }
                    if response.changed() {
                        self.add_word.code = self.add_word.code.trim().to_ascii_lowercase();
                        self.add_word.auto = false;
                    }
                    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        close = self.save_add_word(false);
                    }
                    if self.add_word.auto {
                        ui.weak("自动");
                    } else if self.add_word.suggestions.is_ok()
                        && ui.small_button("按规则重新出码").clicked()
                    {
                        self.add_word.auto = true;
                        self.add_word.computed_for = None;
                    }
                });
                ui.end_row();
            });
        self.rule_hints(ui);
        ui.separator();
        self.code_occupants(ui);
        ui.add_space(4.0);
        ui.checkbox(&mut self.add_word.first, "设为该码首选");
        ui.horizontal(|ui| {
            if ui.button("保存").clicked() {
                close = self.save_add_word(false);
            }
            if ui
                .button("保存并继续")
                .on_hover_text("保存后清空，接着加下一个词")
                .clicked()
            {
                self.save_add_word(true);
            }
        });
        if !self.add_word.message.is_empty() {
            ui.label(&self.add_word.message);
        }
        close
    }

    /// 造词时挑最近上屏的尾部几个字。
    fn recent_choices(&mut self, ui: &mut egui::Ui, recent: &str) {
        let chars: Vec<char> = recent.chars().collect();
        ui.horizontal_wrapped(|ui| {
            ui.weak("最近上屏：");
            for len in 2..=chars.len() {
                let tail: String = chars[chars.len() - len..].iter().collect();
                if ui
                    .selectable_label(self.add_word.text == tail, format!("{len}字"))
                    .on_hover_text(&tail)
                    .clicked()
                {
                    self.add_word.text = tail;
                }
            }
        });
    }

    fn recompute_code(&mut self) {
        let text = self.add_word.text.trim().to_string();
        self.add_word.suggestions = self.encoder.encode(&text);
        if self.add_word.auto {
            self.add_word.code = match &self.add_word.suggestions {
                Ok(suggestions) => suggestions
                    .first()
                    .map(|s| s.code.clone())
                    .unwrap_or_default(),
                Err(_) => String::new(),
            };
        }
        self.add_word.computed_for = Some(self.add_word.text.clone());
    }

    /// 规则说明、其他可能码、逐字码与不符规则的提醒。
    fn rule_hints(&mut self, ui: &mut egui::Ui) {
        let text = self.add_word.text.trim().to_string();
        if text.is_empty() {
            return;
        }
        if let Some(formula) = self.encoder.formula_for(&text) {
            let class = super::encoder::RULE_LABELS[(text.chars().count().min(4)) - 2];
            ui.weak(format!(
                "规则：{class} {} · {}",
                formula.text(),
                formula.describe()
            ));
        }
        match self.add_word.suggestions.clone() {
            Ok(suggestions) => {
                if suggestions.len() > 1 {
                    ui.horizontal_wrapped(|ui| {
                        ui.weak("其他可能：");
                        for suggestion in suggestions.iter().take(MAX_ALTERNATIVES) {
                            let selected = suggestion.code == self.add_word.code;
                            if ui
                                .selectable_label(
                                    selected,
                                    egui::RichText::new(&suggestion.code).monospace(),
                                )
                                .on_hover_text(parts_text(&suggestion.parts))
                                .clicked()
                            {
                                self.add_word.code = suggestion.code.clone();
                                self.add_word.auto = false;
                            }
                        }
                    });
                }
                let chosen = suggestions
                    .iter()
                    .find(|s| s.code == self.add_word.code)
                    .or(suggestions.first());
                if let Some(chosen) = chosen {
                    ui.weak(format!("逐字：{}", parts_text(&chosen.parts)));
                }
                let code = self.add_word.code.as_str();
                if !code.is_empty() && !suggestions.iter().any(|s| s.code == code) {
                    ui.colored_label(theme::warn(), "编码与规则推得的码不符，确认无误再保存。");
                }
            }
            Err(EncodeError::Single) => {
                let codes = self
                    .encoder
                    .char_codes(text.chars().next().unwrap_or_default());
                if codes.is_empty() {
                    ui.weak("单字没有构词规则，请手填编码。");
                } else {
                    ui.weak(format!("单字请手填编码；该字字码：{}", codes.join("、")));
                }
            }
            Err(error) => {
                ui.colored_label(theme::warn(), error.to_string());
            }
        }
        let existing = self.table.codes_of(&text);
        if !existing.is_empty() {
            let list: Vec<String> = existing
                .iter()
                .map(|(code, position, sources)| {
                    format!("{code}（{}，第{position}位）", sources.join("、"))
                })
                .collect();
            ui.label(format!("此词已有编码：{}", list.join("；")));
        }
    }

    /// 该码现有候选，以及新词保存后排第几位。
    fn code_occupants(&mut self, ui: &mut egui::Ui) {
        let code = self.add_word.code.trim();
        if !table::valid_code(code, false) {
            ui.weak("编码为 1–4 个小写字母。");
            return;
        }
        let text = self.add_word.text.trim();
        let current = self.table.lookup(code);
        if current.is_empty() {
            ui.weak(format!("{code} 目前是空码，新词将独占此码。"));
            return;
        }
        let list: Vec<String> = current
            .iter()
            .take(9)
            .enumerate()
            .map(|(i, c)| format!("{} {}", i + 1, c.text))
            .collect();
        let more = if current.len() > 9 {
            format!(" 等 {} 个", current.len())
        } else {
            String::new()
        };
        ui.label(format!("{code} 现有：{}{more}", list.join("  ")));
        if let Some(index) = current.iter().position(|c| c.text == text) {
            ui.weak(format!("此词已在该码第 {} 位。", index + 1));
        } else if self.add_word.first {
            ui.weak("保存后新词排第 1 位。");
        } else {
            ui.weak(format!("保存后新词排第 {} 位。", current.len() + 1));
        }
    }

    /// 保存为个人词条。`next` 为「保存并继续」：清空文字，窗口留着。返回是否关窗。
    fn save_add_word(&mut self, next: bool) -> bool {
        let entry = Entry {
            code: self.add_word.code.trim().into(),
            text: self.add_word.text.trim().into(),
        };
        if !table::valid_entry(&entry, false) {
            self.add_word.message = "编码为 1–4 个小写字母，文字不能为空或含换行、制表符。".into();
            return false;
        }
        let mut personal = self.table.personal.clone();
        personal.hidden.retain(|e| e != &entry);
        let known = self
            .table
            .all(&entry.code)
            .iter()
            .any(|c| c.text == entry.text);
        if !known && !personal.entries.contains(&entry) {
            personal.entries.push(entry.clone());
        }
        if self.add_word.first {
            let mut order: Vec<String> = self
                .table
                .all(&entry.code)
                .iter()
                .map(|c| c.text.clone())
                .filter(|t| t != &entry.text)
                .collect();
            order.insert(0, entry.text.clone());
            personal.order.insert(entry.code.clone(), order);
        }
        match self.save_personal(personal) {
            Ok(()) => {
                self.notice = Some(format!("已加入词表：{} {}", entry.code, entry.text));
                if next {
                    self.add_word.text.clear();
                    self.add_word.code.clear();
                    self.add_word.auto = true;
                    self.add_word.computed_for = None;
                    self.add_word.recent = None;
                    self.add_word.message =
                        format!("已保存 {} {}，可以接着加。", entry.code, entry.text);
                    false
                } else {
                    true
                }
            }
            Err(error) => {
                self.add_word.message = format!("保存失败：{error}");
                false
            }
        }
    }
}

/// 「网 wh · 格 ge」。
pub(super) fn parts_text(parts: &[(char, String)]) -> String {
    parts
        .iter()
        .map(|(c, code)| format!("{c} {code}"))
        .collect::<Vec<_>>()
        .join(" · ")
}
