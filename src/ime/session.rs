//! 词表输入法的焦点、按键与上屏处理。预编辑仅画在候选窗中。
use super::{
    ImeSettings, data,
    keys::{self, Action, Key, Route},
    table::{self, Candidate, Entry, Personal, Table},
};
use eframe::egui;
use std::collections::HashSet;
use std::time::{Duration, Instant};
const SHIFT_TAP_MAX: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Preedit {
    pub text: String,
    pub caret: usize,
}
pub(super) struct Outcome {
    pub consumed: bool,
    pub commit: Option<String>,
    pub recompose: bool,
}
impl Outcome {
    const PASSTHROUGH: Self = Self {
        consumed: false,
        commit: None,
        recompose: false,
    };
    const CHANGED: Self = Self {
        consumed: true,
        commit: None,
        recompose: true,
    };
    const NAVIGATED: Self = Self {
        consumed: true,
        commit: None,
        recompose: false,
    };
    fn commit(text: String) -> Self {
        Self {
            consumed: true,
            commit: Some(text),
            recompose: true,
        }
    }
}

pub(crate) struct Ime {
    pub(super) settings: ImeSettings,
    pub(super) table: Table,
    pub(super) manager: super::manage::Manager,
    pub(super) storage_ok: bool,
    pub(super) load_error: Option<String>,
    pub(super) layout: Vec<Candidate>,
    pub(super) highlight: usize,
    pub(super) preedit: Preedit,
    pub(super) english: bool,
    pub(super) pending_commit: Option<String>,
    window_size: Option<egui::Vec2>,
    rows_width: Option<f32>,
    editable_focus: bool,
    pub(super) focus_id: Option<egui::Id>,
    exempt: HashSet<egui::Id>,
    pub(super) anchor: Option<egui::Rect>,
    shift_alone: bool,
    shift_pressed_at: Instant,
    system_ime_off: bool,
    notice: Option<String>,
    quote_open: bool,
    single_quote_open: bool,
}

impl Ime {
    pub(crate) fn new(settings: ImeSettings) -> Self {
        let mut ime = Self::bare(settings);
        match data::load_base() {
            Ok(entries) => ime.table.base = entries,
            Err(error) => ime.load_error = Some(error.to_string()),
        }
        match data::directory().map(|dir| dir.join("tables.json")) {
            Ok(path) if path.exists() => match std::fs::read(&path)
                .map_err(anyhow::Error::from)
                .and_then(|bytes| Ok(serde_json::from_slice::<Personal>(&bytes)?))
            {
                Ok(personal) => ime.table.personal = personal,
                Err(error) => {
                    ime.storage_ok = false;
                    ime.load_error = Some(format!("个人词表读取失败，保留原文件：{error}"));
                }
            },
            Err(error) => {
                ime.storage_ok = false;
                ime.load_error = Some(error.to_string());
            }
            _ => {}
        }
        ime.table.rebuild();
        ime
    }
    fn bare(settings: ImeSettings) -> Self {
        Self {
            settings,
            table: Table::default(),
            manager: Default::default(),
            storage_ok: true,
            load_error: None,
            layout: Vec::new(),
            highlight: 0,
            preedit: Default::default(),
            english: false,
            pending_commit: None,
            window_size: None,
            rows_width: None,
            editable_focus: false,
            focus_id: None,
            exempt: HashSet::new(),
            anchor: None,
            shift_alone: false,
            shift_pressed_at: Instant::now(),
            system_ime_off: false,
            notice: None,
            quote_open: false,
            single_quote_open: false,
        }
    }
    pub(crate) fn available(&self) -> bool {
        !self.table.base.is_empty()
    }
    pub(crate) fn active(&self) -> bool {
        self.settings.enabled && self.available()
    }
    pub(crate) fn english(&self) -> bool {
        self.english
    }
    pub(crate) fn toggle_english(&mut self) {
        self.drop_composition();
        self.english = !self.english;
    }
    pub(crate) fn take_notice(&mut self) -> Option<String> {
        self.notice.take()
    }
    pub(crate) fn data_brief(&self) -> Option<String> {
        self.available().then(|| {
            format!(
                "基础表 {} 条 · 公文表 {} 条",
                self.table.base.len(),
                self.table.document.len()
            )
        })
    }
    pub(crate) fn apply_settings(&mut self, settings: ImeSettings) {
        if self.settings == settings {
            return;
        }
        self.settings = settings;
        self.drop_composition();
    }
    pub(super) fn window_size(&self) -> Option<egui::Vec2> {
        self.window_size
    }
    pub(super) fn remember_window(&mut self, size: egui::Vec2) {
        self.window_size = Some(size);
    }
    pub(super) fn rows_width(&self) -> Option<f32> {
        self.rows_width
    }
    pub(super) fn remember_rows_width(&mut self, width: f32) {
        self.rows_width = Some(width);
    }
    pub(super) fn forget_window(&mut self) {
        self.window_size = None;
        self.rows_width = None;
    }
    pub(crate) fn focus_exempt(&self) -> bool {
        self.focus_id.is_some_and(|id| self.exempt.contains(&id))
    }
    fn record_focus(&mut self, anchor: Option<egui::Rect>) {
        self.editable_focus = anchor.is_some();
        self.anchor = anchor;
    }
    fn composing(&self) -> bool {
        !self.preedit.text.is_empty()
    }
    fn drop_composition(&mut self) {
        self.preedit = Default::default();
        self.layout.clear();
        self.highlight = 0;
        self.pending_commit = None;
        self.forget_window();
    }
    pub(super) fn refresh(&mut self, reset: bool) {
        self.layout = self.table.lookup(&self.preedit.text);
        if reset || self.highlight >= self.layout.len() {
            self.highlight = 0;
        }
    }
    pub(crate) fn sync_lexicon(
        &mut self,
        terms: &[crate::lexicon::LexiconTerm],
    ) -> anyhow::Result<usize> {
        self.table.document = terms
            .iter()
            .filter(|term| term.state == crate::lexicon::TermState::Accepted)
            .filter_map(|term| {
                let code = term.code().ok()?;
                table::valid_code(&code, true).then(|| Entry {
                    code,
                    text: term.term.clone(),
                })
            })
            .collect();
        self.table.rebuild();
        self.manager.dirty = true;
        self.refresh(true);
        Ok(self.table.document.len())
    }
    /// 兼容旧短语，只迁移一次；超长编码保留在旧配置中，不加入四码输入。
    pub(crate) fn apply_phrases(&mut self, phrases: &[crate::models::ImePhrase]) {
        if self.table.personal.migrated_phrases || !self.storage_ok {
            return;
        }
        let mut personal = self.table.personal.clone();
        for phrase in phrases.iter().filter(|p| {
            p.enabled
                && table::valid_code(&p.code, false)
                && !p.text.is_empty()
                && !p.text.contains(['\r', '\n', '\t'])
        }) {
            let entry = Entry {
                code: phrase.code.clone(),
                text: phrase.text.clone(),
            };
            if !personal.entries.contains(&entry) {
                personal.entries.push(entry);
            }
        }
        personal.migrated_phrases = true;
        if let Err(error) = self.save_personal(personal) {
            self.notice = Some(format!("旧短语迁移失败：{error}"));
        }
    }
    pub(super) fn save_personal(&mut self, personal: Personal) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.storage_ok,
            "个人词表文件读取失败，不能覆盖；请先恢复原文件"
        );
        data::save_json(&data::directory()?.join("tables.json"), &personal)?;
        self.table.personal = personal;
        self.manager.dirty = true;
        self.table.rebuild();
        self.refresh(true);
        Ok(())
    }

    /// 帧首：接管键盘。要在任何控件跑之前调用。
    pub(crate) fn begin_frame(&mut self, ctx: &egui::Context) {
        if !self.active() {
            return;
        }
        let focused = ctx.memory(|memory| memory.focused());
        let focus_changed = focused != self.focus_id;
        if focus_changed {
            self.focus_id = focused;
            self.drop_composition();
        }
        // 焦点刚换过地方的那一帧不接键盘：这时 `editable_focus` 还是上一帧的，
        // 而按键已经该归新控件了；不然回车、空格这类键会被白白吃掉。
        // 声明过不走输入法的字段（密码、接口地址）同样不接：按键原样交给文本框。
        if focus_changed || !self.editable_focus || self.focus_exempt() {
            self.drop_composition();
            return;
        }
        ctx.input_mut(|input| {
            let events = std::mem::take(&mut input.events);
            let mut routed = self.route_events(events);
            // 鼠标点选的候选：这一帧的文本框已经画完了，只能排到队首，
            // 下一帧最早落笔。注在分流之后，免得又被当成按键喂回引擎。
            if let Some(text) = self.pending_commit.take() {
                routed.insert(0, egui::Event::Text(text));
            }
            input.events = routed;
        });
    }

    /// 帧尾：记下光标矩形当候选窗锚点，并关掉系统输入法。要在所有控件跑完之后调用。
    pub(crate) fn end_frame(&mut self, ctx: &egui::Context) {
        // 本帧控件的声明留给下一帧帧首用；不管接不接管都要取走，别越攒越多。
        self.exempt = super::exempt::take(ctx);
        if !self.active() {
            // 不接管键盘：光标矩形照旧报给系统输入法。
            let anchor = ctx.output(|output| output.ime.map(|ime| ime.cursor_rect));
            self.record_focus(anchor);
            super::follow_cursor(ctx);
            return;
        }
        // `take` 掉就是关掉系统输入法：egui-winit 见 `output.ime` 为 `None` 会调
        // `Window::set_ime_allowed(false)`，Windows 落到 `ImmAssociateContextEx`，
        // Linux / macOS 落到各自的 text-input 协议。只影响本窗口。
        let anchor = ctx.output_mut(|output| output.ime.take().map(|ime| ime.cursor_rect));
        self.record_focus(anchor);

        if !self.system_ime_off {
            // 窗口默认是开着系统输入法的，而 winit 只在 `output.ime` 由 Some 变 None 时
            // 才会调 `set_ime_allowed`：第一帧之前得显式关一次，否则会漏进系统输入法。
            ctx.send_viewport_cmd(egui::ViewportCommand::IMEAllowed(false));
            self.system_ime_off = true;
        }
    }

    /// 走一遍本帧的事件：该吃的吃掉，该上屏的塞回去。
    fn route_events(&mut self, events: Vec<egui::Event>) -> Vec<egui::Event> {
        let mut kept = Vec::with_capacity(events.len() + 2);
        for event in events {
            let consumed = match &event {
                egui::Event::Key {
                    key,
                    pressed,
                    modifiers,
                    repeat,
                    ..
                } => self.route_key(*key, *pressed, *repeat, *modifiers, &mut kept),
                // 字符走 `Text`：`Key` 事件对纯字母没有插入语义，两边都处理会插两次。
                // 按着 Shift 点鼠标、滚滚轮是扩选，不是单击 Shift
                egui::Event::PointerButton { pressed: true, .. } => {
                    self.shift_alone = false;
                    self.pointer_pressed();
                    false
                }
                egui::Event::MouseWheel { .. } => {
                    self.shift_alone = false;
                    false
                }
                // 剪切、粘贴不走 `Key` 事件（egui-winit 直接换成这两个事件），也改正文：
                // 与快捷键同样处理，组码中先把编码原样上屏。复制不动正文，不管。
                egui::Event::Cut | egui::Event::Paste(_) => {
                    self.shift_alone = false;
                    self.handle(Key::Shortcut, &mut kept)
                }
                egui::Event::Text(text) => match single_char(text) {
                    Some(c) => {
                        // 字符也要按掉「Shift 单击」：Shift、字母、Shift 这个序列里
                        // 字母走的是 `Text` 而不是 `Key`，漏了它会把打字当成切换。
                        self.shift_alone = false;
                        self.handle(Key::Char(c), &mut kept)
                    }
                    None => false,
                },
                _ => false,
            };
            if !consumed {
                kept.push(event);
            }
        }
        kept
    }

    /// 一次功能键 / 修饰键事件：判 Shift 单击，然后分流（组合键怎么分见 `Key::from_egui`）。
    fn route_key(
        &mut self,
        key: egui::Key,
        pressed: bool,
        repeat: bool,
        modifiers: egui::Modifiers,
        kept: &mut Vec<egui::Event>,
    ) -> bool {
        if !pressed {
            // Shift 按下到抬起之间没有别的键插进来、也没按太久，就是一次单击：切中英。
            if is_shift(key) && self.shift_alone {
                self.shift_alone = false;
                if self.shift_pressed_at.elapsed() <= SHIFT_TAP_MAX {
                    self.shift_tapped(kept);
                }
            }
            return false;
        }
        if is_shift(key) {
            // 按着 Ctrl / Alt 再按 Shift 是组合键的一部分，不算单击
            if !repeat && !(modifiers.ctrl || modifiers.alt || modifiers.command) {
                self.shift_alone = true;
                self.shift_pressed_at = Instant::now();
            }
            return false;
        }
        self.shift_alone = false;
        match Key::from_egui(key, modifiers) {
            Some(ime_key) => self.handle(ime_key, kept),
            None => false,
        }
    }

    fn pointer_pressed(&mut self) {}
    /// 单击 Shift：切中英。组码中切到英文时，已经敲的字母原样上屏——
    /// 打了 `hello` 才发现该是英文，单击 Shift 就是 hello，不用删了重打。
    fn shift_tapped(&mut self, kept: &mut Vec<egui::Event>) {
        if !self.english && self.composing() {
            let outcome = self.execute_guarded(Action::CommitRaw);
            if let Some(text) = outcome.commit {
                kept.push(egui::Event::Text(text));
            }
            self.refresh(true);
        }
        self.english = !self.english;
    }

    fn route(&self) -> Route {
        Route {
            composing: self.composing(),
            candidates: self.layout.len(),
            english: self.english,
            full_width: !self.english && self.settings.full_width_punctuation,
        }
    }
    fn handle(&mut self, key: Key, kept: &mut Vec<egui::Event>) -> bool {
        let action = keys::route(key, self.route(), &self.settings);
        let outcome = self.execute_guarded(action);
        if let Some(text) = outcome.commit.filter(|t| !t.is_empty()) {
            kept.push(egui::Event::Text(text));
        }
        if outcome.recompose {
            self.refresh(true);
        }
        outcome.consumed
    }
    pub(super) fn execute_guarded(&mut self, action: Action) -> Outcome {
        let pushed = matches!(action, Action::Push(_));
        let mut outcome = self.execute(action);
        if outcome.recompose {
            self.refresh(true);
        }
        if pushed
            && self.preedit.caret == 4
            && self.settings.auto_commit
            && self.preedit.text.len() == 4
            && self.layout.len() == 1
        {
            let text = self.commit_at(0).unwrap_or_default();
            outcome.commit = Some(format!("{}{text}", outcome.commit.unwrap_or_default()));
            self.refresh(true);
        }
        outcome
    }
    fn commit_at(&mut self, index: usize) -> Option<String> {
        let text = self.layout.get(index)?.text.clone();
        self.preedit = Default::default();
        Some(text)
    }
    fn take_raw(&mut self) -> String {
        let text = std::mem::take(&mut self.preedit.text);
        self.preedit.caret = 0;
        text
    }
    fn execute(&mut self, action: Action) -> Outcome {
        match action {
            Action::Passthrough => Outcome::PASSTHROUGH,
            Action::Push(c) => {
                let mut committed = None;
                if self.preedit.text.len() == 4 {
                    if !self.settings.fifth_commit || self.preedit.caret != 4 {
                        return Outcome::NAVIGATED;
                    }
                    committed = self.commit_at(self.highlight);
                    if committed.is_none() {
                        self.notice = Some("当前四码没有候选，请退格修改或按 Esc 清码。".into());
                        return Outcome::NAVIGATED;
                    }
                }
                self.preedit.text.insert(self.preedit.caret, c);
                self.preedit.caret += 1;
                Outcome {
                    consumed: true,
                    commit: committed,
                    recompose: true,
                }
            }
            Action::Backspace => {
                if self.preedit.caret > 0 {
                    self.preedit.caret -= 1;
                    self.preedit.text.remove(self.preedit.caret);
                }
                Outcome::CHANGED
            }
            Action::DeleteForward => {
                if self.preedit.caret < self.preedit.text.len() {
                    self.preedit.text.remove(self.preedit.caret);
                }
                Outcome::CHANGED
            }
            Action::Clear => {
                self.preedit = Default::default();
                Outcome::CHANGED
            }
            Action::CommitRaw => Outcome::commit(self.take_raw()),
            Action::FlushRaw => Outcome {
                consumed: false,
                commit: Some(self.take_raw()),
                recompose: true,
            },
            Action::CommitHighlighted => match self.commit_at(self.highlight) {
                Some(text) => Outcome::commit(text),
                None => Outcome::NAVIGATED,
            },
            Action::CommitIndex(index) => {
                let absolute =
                    self.highlight / self.settings.page_size * self.settings.page_size + index;
                match self.commit_at(absolute) {
                    Some(text) => Outcome::commit(text),
                    None => Outcome::NAVIGATED,
                }
            }
            Action::Insert(c) => {
                let mut text = self.take_raw();
                text.push(c);
                Outcome::commit(text)
            }
            Action::Punctuate(c) => {
                let prefix = if self.composing() {
                    self.commit_at(self.highlight)
                        .unwrap_or_else(|| self.take_raw())
                } else {
                    String::new()
                };
                let punctuation = self.punctuation(c);
                Outcome::commit(format!("{prefix}{punctuation}"))
            }
            Action::Navigate(nav) => {
                use keys::Navigation::*;
                match nav {
                    Highlight(delta) => {
                        self.highlight = (self.highlight as isize + delta)
                            .clamp(0, self.layout.len().saturating_sub(1) as isize)
                            as usize
                    }
                    Page(delta) => {
                        let size = self.settings.page_size;
                        let pages = self.layout.len().div_ceil(size).max(1);
                        let page = (self.highlight as isize / size as isize + delta)
                            .clamp(0, pages as isize - 1)
                            as usize;
                        self.highlight = page * size;
                    }
                    CursorLeft => self.preedit.caret = self.preedit.caret.saturating_sub(1),
                    CursorRight => {
                        self.preedit.caret = (self.preedit.caret + 1).min(self.preedit.text.len())
                    }
                    CursorHome => self.preedit.caret = 0,
                    CursorEnd => self.preedit.caret = self.preedit.text.len(),
                };
                Outcome::NAVIGATED
            }
        }
    }
    fn punctuation(&mut self, c: char) -> String {
        if self.english || !self.settings.full_width_punctuation {
            return c.to_string();
        }
        match c {
            ',' => "，",
            '.' => "。",
            ';' => "；",
            ':' => "：",
            '?' => "？",
            '!' => "！",
            '(' => "（",
            ')' => "）",
            '[' => "【",
            ']' => "】",
            '<' => "《",
            '>' => "》",
            '"' => {
                self.quote_open = !self.quote_open;
                if self.quote_open { "“" } else { "”" }
            }
            '\'' => {
                self.single_quote_open = !self.single_quote_open;
                if self.single_quote_open { "‘" } else { "’" }
            }
            _ => return c.to_string(),
        }
        .into()
    }
}
fn is_shift(key: egui::Key) -> bool {
    matches!(key, egui::Key::ShiftLeft | egui::Key::ShiftRight)
}
fn single_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let c = chars.next()?;
    chars.next().is_none().then_some(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ime() -> Ime {
        let mut ime = Ime::bare(ImeSettings::default());
        ime.table.base = table::parse(
            "a,1=啊\nabcd,1=公文\nefgh,1=词表\nzzzz,1=首选\nzzzz,2=次选",
            false,
        )
        .entries;
        ime.table.rebuild();
        ime
    }
    fn type_code(ime: &mut Ime, code: &str) -> String {
        let mut out = String::new();
        for c in code.chars() {
            if let Some(s) = ime.execute_guarded(Action::Push(c)).commit {
                out.push_str(&s);
            }
        }
        out
    }
    #[test]
    fn exact_lookup_fifth_key_and_invalid_code() {
        let mut ime = ime();
        assert!(type_code(&mut ime, "abcd").is_empty());
        assert_eq!(ime.layout[0].text, "公文");
        assert_eq!(type_code(&mut ime, "e"), "公文");
        assert_eq!(ime.preedit.text, "e");
        type_code(&mut ime, "fgh");
        assert_eq!(
            ime.execute_guarded(Action::CommitHighlighted)
                .commit
                .as_deref(),
            Some("词表")
        );
        type_code(&mut ime, "xxxxa");
        assert_eq!(ime.preedit.text, "xxxx");
        assert!(ime.layout.is_empty());
        assert!(
            ime.execute_guarded(Action::CommitHighlighted)
                .commit
                .is_none()
        );
        assert_eq!(
            ime.execute_guarded(Action::CommitRaw).commit.as_deref(),
            Some("xxxx")
        );
    }
    #[test]
    fn auto_commit_single_char_and_conflicts() {
        let mut ime = ime();
        ime.settings.auto_commit = true;
        assert_eq!(type_code(&mut ime, "abcd"), "公文");
        type_code(&mut ime, "zzzz");
        assert_eq!(ime.layout.len(), 2);
        assert_eq!(
            ime.execute_guarded(Action::CommitIndex(1))
                .commit
                .as_deref(),
            Some("次选")
        );
    }
    #[test]
    fn literal_codes_and_cursor_editing() {
        let mut ime = ime();
        type_code(&mut ime, "a");
        assert_eq!(ime.layout[0].text, "啊");
        ime.execute_guarded(Action::Clear);
        type_code(&mut ime, "ab");
        assert!(ime.layout.is_empty());
        ime.execute_guarded(Action::Navigate(keys::Navigation::CursorLeft));
        ime.execute_guarded(Action::DeleteForward);
        assert_eq!(ime.preedit.text, "a");
    }
    #[test]
    fn document_table_only_loads_accepted_four_codes() {
        use crate::lexicon::{LexiconTerm, TermOrigin, TermState};
        let term = LexiconTerm {
            id: 1,
            term: "本单位".into(),
            pinyin: String::new(),
            code_override: "abcd".into(),
            freq_total: 1,
            doc_count: 1,
            origin: TermOrigin::Manual,
            state: TermState::Accepted,
            locked: true,
            in_base_dict: false,
            group_name: String::new(),
            note: String::new(),
            first_seen: String::new(),
            last_seen: String::new(),
        };
        let mut candidate = term.clone();
        candidate.term = "待确认".into();
        candidate.state = TermState::Candidate;
        let mut short = term.clone();
        short.term = "短码".into();
        short.code_override = "ab".into();
        let mut ime = ime();
        assert_eq!(ime.sync_lexicon(&[term, candidate, short]).unwrap(), 1);
        assert_eq!(
            ime.table
                .lookup("abcd")
                .iter()
                .map(|c| c.text.as_str())
                .collect::<Vec<_>>(),
            ["公文", "本单位"]
        );
        assert!(ime.table.lookup("ab").is_empty());
    }
    #[test]
    fn punctuation_commits_candidate_and_shortcuts_preserve_raw() {
        let mut ime = ime();
        type_code(&mut ime, "abcd");
        assert_eq!(
            ime.execute_guarded(Action::Punctuate(','))
                .commit
                .as_deref(),
            Some("公文，")
        );
        type_code(&mut ime, "ab");
        let out = ime.execute_guarded(Action::FlushRaw);
        assert!(!out.consumed);
        assert_eq!(out.commit.as_deref(), Some("ab"));
    }
    #[test]
    fn focus_change_clears_code_and_mouse_commit_keeps_target() {
        let mut ime = ime();
        let ctx = egui::Context::default();
        let id = egui::Id::new("正文");
        ctx.memory_mut(|m| m.request_focus(id));
        ime.focus_id = Some(id);
        ime.editable_focus = true;
        type_code(&mut ime, "abcd");
        ime.pending_commit = Some("公文".into());
        ime.begin_frame(&ctx);
        assert!(ctx.input(|i| {
            i.events
                .iter()
                .any(|e| matches!(e,egui::Event::Text(s) if s=="公文"))
        }));
        ctx.memory_mut(|m| m.request_focus(egui::Id::new("新框")));
        ime.begin_frame(&ctx);
        assert!(ime.preedit.text.is_empty());
    }
}
