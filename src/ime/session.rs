//! 组句状态与键盘接管：本帧的按键喂给引擎，上屏的文本塞回事件队列。
//!
//! 上屏走的是 egui 自己的**文本插入**通路（注入 `Event::Text`），不是 `ImeEvent::Preedit`：
//! egui 的预编辑文本会被真的写进文本框，而文本一进文本框，焦点一走就没人负责删掉它
//! （TextEdit 只在 `owns_ime_events` 为真时处理 Ime 事件）。所以拼音只在引擎里，
//! 画在我们自己的候选窗里（`candidates`），文本框里到上屏为止一个字都没有。
//!
//! 引擎里的 panic 不能把整篇文稿带走：按键路径整个套在 `catch_unwind` 里，拦下之后
//! 卸掉引擎（`RefCell` 可能停在借出状态，再调一定还会 panic），这次按键当没发生。

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use eframe::egui;
use qingjian_core::{CandidateLayout, Engine, FumaTable};

use super::ImeSettings;
use super::data;
use super::engine::{self, Assembly};
use super::keys::{self, Action, Key, Route};
use super::lexicon;
use crate::lexicon::LexiconTerm;

/// 学习数据落盘的间隔。被杀进程最多丢这么久的选择记录。
const FLUSH_INTERVAL: Duration = Duration::from_secs(60);

/// 要画在候选窗里的拼音串与光标位置（字符数）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Preedit {
    /// 拼音串（`ni'hao`），纠错生效时是纠正后的写法。
    pub(crate) text: String,

    /// 光标在 [`Self::text`] 里的字符位置：`Left` / `Right` 挪的就是它。
    pub(crate) caret: usize,
}

/// 一次按键的处理结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Outcome {
    /// 吃掉了，不再交给应用。
    pub(super) consumed: bool,

    /// 要插进文本框的文本。
    pub(super) commit: Option<String>,

    /// 缓冲变了：要重查候选并把高亮归零。
    pub(super) recompose: bool,
}

impl Outcome {
    /// 交给应用。
    const PASSTHROUGH: Self = Self {
        consumed: false,
        commit: None,
        recompose: false,
    };

    /// 吃掉了，缓冲变了。
    const CHANGED: Self = Self {
        consumed: true,
        commit: None,
        recompose: true,
    };

    /// 吃掉了，只挪了高亮 / 光标。
    const NAVIGATED: Self = Self {
        consumed: true,
        commit: None,
        recompose: false,
    };

    /// 吃掉了，上屏一段文本。
    fn commit(text: String) -> Self {
        Self {
            consumed: true,
            commit: Some(text),
            recompose: true,
        }
    }
}

/// 应用内输入法。
pub(crate) struct Ime {
    /// 装配好的引擎；数据缺失或已经崩过时为 `None`，这时键盘整个交给应用。
    assembled: Option<Assembly>,

    /// 设置。
    pub(super) settings: ImeSettings,

    /// 中英模式：默认中文，`Shift` 单击切换。
    pub(super) english: bool,

    /// 当前候选的分页布局。
    pub(super) layout: CandidateLayout,

    /// 高亮：跨页下标。
    pub(super) highlight: usize,

    /// 这一帧的拼音串。
    pub(super) preedit: Preedit,

    /// 鼠标点选的候选：下一帧开场就要插进文本框（点击发生在本帧文本框画完之后）。
    pub(super) pending_commit: Option<String>,

    /// 上一帧候选窗量到的尺寸，用来判断贴光标上方还是下方。
    window_size: Option<egui::Vec2>,

    /// 上一帧候选行量到的宽度，用来把页码顶到右边（见 `candidates::header`）。
    rows_width: Option<f32>,

    /// 辅码表加载了几个字；`None` 表示没装表（辅码关着）。
    fuma_words: Option<usize>,

    /// 上一帧有可编辑控件持有焦点（egui 的 `output.ime` 非空）。只有为真才接管键盘：
    /// 否则按键会被我们吃掉却没人在文本框里接收。
    editable_focus: bool,

    /// 上一帧的焦点控件。焦点换地方就丢掉这段拼音，别飘到新的输入框里。
    pub(super) focus_id: Option<egui::Id>,

    /// 光标在屏幕上的矩形，候选窗的锚点。
    pub(super) anchor: Option<egui::Rect>,

    /// Shift 单击判定：按下 Shift 之后还没有别的键插进来。
    shift_alone: bool,

    /// 已经显式让后端关过系统输入法了吗。
    system_ime_off: bool,

    /// 上次把学习数据写盘的时刻。
    last_flush: Instant,
}

impl Ime {
    /// 装配输入法。找不到数据（或缺词库）就返回一个不可用的输入法，键盘留给系统输入法。
    pub(crate) fn new(settings: ImeSettings) -> Self {
        let assembled = if settings.enabled { load() } else { None };
        let mut ime = Self {
            assembled,
            settings,
            english: false,
            layout: empty_layout(settings.page_size),
            highlight: 0,
            preedit: Preedit::default(),
            pending_commit: None,
            window_size: None,
            rows_width: None,
            fuma_words: None,
            editable_focus: false,
            focus_id: None,
            anchor: None,
            shift_alone: false,
            system_ime_off: false,
            last_flush: Instant::now(),
        };
        ime.apply_fuma();
        ime
    }

    /// 测试用：拿一份现成的引擎当输入法（不依赖随包的 `.qj` 数据）。
    #[cfg(test)]
    pub(super) fn for_test(assembly: Assembly, settings: ImeSettings) -> Self {
        let mut ime = Self::new(ImeSettings {
            enabled: false,
            ..settings
        });
        ime.settings = settings;
        ime.assembled = Some(assembly);
        ime
    }

    /// 候选窗量到的尺寸。
    pub(super) fn window_size(&self) -> Option<egui::Vec2> {
        self.window_size
    }

    /// 记下候选窗的尺寸。
    pub(super) fn remember_window(&mut self, size: egui::Vec2) {
        self.window_size = Some(size);
    }

    /// 上一帧候选行的宽度。
    pub(super) fn rows_width(&self) -> Option<f32> {
        self.rows_width
    }

    /// 记下候选行的宽度。
    pub(super) fn remember_rows_width(&mut self, width: f32) {
        self.rows_width = Some(width);
    }

    /// 候选窗不画了：量到的尺寸也没用了。
    pub(super) fn forget_window(&mut self) {
        self.window_size = None;
        self.rows_width = None;
    }

    /// 引擎在不在（数据齐、装配成功）。
    pub(crate) fn available(&self) -> bool {
        self.assembled.is_some()
    }

    /// 引擎在并且开着。
    pub(crate) fn active(&self) -> bool {
        self.settings.enabled && self.assembled.is_some()
    }

    /// 当前是不是英文模式。
    pub(crate) fn english(&self) -> bool {
        self.english
    }

    /// 词库 / 语言模型的来历，设置页显示一行。
    pub(crate) fn data_summary(&self) -> Option<String> {
        let assembly = self.assembled.as_ref()?;
        let mut summary = match &assembly.dictionary_name {
            Some(name) => format!("{name}（{} 条）", assembly.dictionary_entries),
            None => format!("词库 {} 条", assembly.dictionary_entries),
        };
        match assembly.bigrams {
            Some(count) => summary.push_str(&format!("，语言模型 {count} 组")),
            None => summary.push_str("，无语言模型"),
        }
        if let Some(license) = &assembly.dictionary_license {
            summary.push_str(&format!("，词库许可 {license}"));
        }
        summary.push_str(&format!("，加载 {} ms", assembly.load_ms));
        Some(summary)
    }

    /// 应用设置。设置没变就什么都不做，可以每帧调。
    pub(crate) fn apply_settings(&mut self, settings: ImeSettings) {
        if settings == self.settings {
            return;
        }
        let turned_on = settings.enabled && !self.settings.enabled;
        self.settings = settings;
        if turned_on && self.assembled.is_none() {
            // 关掉再打开时重新试一次：上次可能是数据还没准备好。
            self.assembled = load();
        }
        if !settings.enabled {
            self.drop_composition();
        }
        if let Some(assembly) = self.assembled.as_mut() {
            assembly.engine.set_shuangpin(settings.shuangpin);
            assembly
                .engine
                .set_full_width_punctuation(settings.full_width_punctuation);
        }
        self.apply_fuma();
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
        if focus_changed || !self.editable_focus {
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
        self.flush_if_due();
        if !self.system_ime_off {
            // 窗口默认是开着系统输入法的，而 winit 只在 `output.ime` 由 Some 变 None 时
            // 才会调 `set_ime_allowed`：第一帧之前得显式关一次，否则会漏进系统输入法。
            ctx.send_viewport_cmd(egui::ViewportCommand::IMEAllowed(false));
            self.system_ime_off = true;
        }
    }

    /// 把学习数据写盘（词频、用户词、个人 n-gram、敲错表）。
    pub(crate) fn flush(&mut self) {
        if let Some(assembly) = self.assembled.as_mut() {
            assembly.engine.flush_learning();
        }
    }

    /// 切中英。状态栏点一下与单击 `Shift` 是同一个开关。
    pub(crate) fn toggle_english(&mut self) {
        self.english = !self.english;
    }

    /// 辅码表加载了几个字；`None` 表示没装表。
    pub(crate) fn fuma_words(&self) -> Option<usize> {
        self.fuma_words
    }

    /// 把公文词表同步成输入法的附加词库，返回写进去的条数。
    ///
    /// 落在 `config_dir()/ime/dicts/`（与稿件库同一个用户目录），写完立即重新
    /// 装配附加词库，不用重启。
    pub(crate) fn sync_lexicon(&mut self, terms: &[LexiconTerm]) -> anyhow::Result<usize> {
        let dir = data::dicts_dir().context("无法确定输入法词库目录")?;
        let (tsv, written) = lexicon::build(terms);
        let path = dir.join(lexicon::FILE_NAME);
        std::fs::write(&path, tsv)
            .with_context(|| format!("写入输入法词库失败：{}", path.display()))?;
        self.reload_extra_dicts();
        Ok(written)
    }

    /// 导入辅码表：把选中的文件拷到用户目录再加载。返回表里的字数。
    ///
    /// 码表不随包（权利归方案作者），只能由使用者自己导入一份，格式与上游
    /// `assets/fuma/xiaohe.txt` 一致：每行 `字=两码`。
    pub(crate) fn import_fuma_table(&mut self, source: &std::path::Path) -> anyhow::Result<usize> {
        let scheme = self
            .settings
            .fuma
            .context("先在设置里选一个辅码方案，再导入码表")?;
        let target = data::fuma_path(scheme).context("无法确定辅码表目录")?;
        // 网上流传的码表不少是 GBK，这里统一转成 UTF-8 再落盘，加载端只认 UTF-8。
        let content = crate::text_file::read_to_string(source)
            .with_context(|| format!("读取辅码表失败：{}", source.display()))?;
        std::fs::write(&target, content)
            .with_context(|| format!("写入辅码表失败：{}", target.display()))?;
        self.apply_fuma();
        self.fuma_words.context("码表里没有认得出的条目")
    }

    /// 把构建内置的示例小鹤辅码表写进用户目录，并立刻加载。
    ///
    /// 仅在 `ime-builtin-xiaohe` feature 开启时**真的有内容可写**：发布构建里
    /// `data::builtin_xiaohe()` 返回 `None`，本方法直接报错；调用方应在调用
    /// 前用 [`Self::has_builtin_xiaohe`] 判断。这条路径**绝不**自动触发——
    /// 调用来自设置页里「使用内置示例小鹤辅码」按钮的二次确认结果。
    pub(crate) fn install_builtin_xiaohe(&mut self) -> anyhow::Result<usize> {
        let content = data::builtin_xiaohe().context(
            "当前构建没有内置示例小鹤辅码表。请检查是否使用 --features ime-builtin-xiaohe 构建，\
             或改用「导入码表…」按钮从本地 txt 导入。",
        )?;
        let scheme = qingjian_core::FumaScheme::Xiaohe;
        let target = data::fuma_path(scheme).context("无法确定辅码表目录")?;
        std::fs::write(&target, content)
            .with_context(|| format!("写入内置辅码表失败：{}", target.display()))?;
        // 没选方案也把设置改成小鹤，让这次写入立即生效；同样不静默——
        // 调用方应当在按钮按下时已经看到「码表方案：未选」的提示，并因此触发本方法。
        if self.settings.fuma.is_none() {
            self.settings.fuma = Some(scheme);
        }
        self.apply_fuma();
        self.fuma_words.context("内置码表里没有认得出的条目")
    }

    /// 当前构建是否打包了内置示例小鹤辅码表。
    pub(crate) fn has_builtin_xiaohe() -> bool {
        data::has_builtin_xiaohe()
    }

    /// 辅码表：方案变了就重新加载。表是使用者自己导入的（不随包，见
    /// `vendor/qingjian/README.md` 的许可说明），不在就当辅码关着。
    fn apply_fuma(&mut self) {
        let table = self.settings.fuma.and_then(|scheme| {
            let path = data::fuma_path(scheme)?;
            match FumaTable::from_path(&path) {
                Ok(table) => Some(table),
                Err(error) => {
                    eprintln!(
                        "[ime] 辅码表读取失败，辅码关着：{}（{error}）",
                        path.display()
                    );
                    None
                }
            }
        });
        self.fuma_words = table.as_ref().map(FumaTable::len);
        if let Some(engine) = self.engine_mut() {
            engine.set_fuma(table.map(Arc::new));
        }
    }

    /// 重新装配附加词库目录：公文词表导出的 TSV，加上使用者自己丢进去的领域词库。
    fn reload_extra_dicts(&mut self) {
        let Some(dir) = data::dicts_dir() else {
            return;
        };
        let dictionaries = engine::load_extra(&dir);
        if let Some(assembly) = self.assembled.as_mut() {
            assembly.engine.set_extra_dictionaries(dictionaries);
        }
    }

    /// 记下本帧的焦点与光标矩形。
    fn record_focus(&mut self, anchor: Option<egui::Rect>) {
        self.editable_focus = anchor.is_some();
        if anchor.is_some() {
            self.anchor = anchor;
        }
    }

    /// 到期就把学习数据落盘。
    fn flush_if_due(&mut self) {
        if self.last_flush.elapsed() < FLUSH_INTERVAL {
            return;
        }
        self.last_flush = Instant::now();
        self.flush();
    }

    /// 引擎（不可变）。查询与读组句状态都只要不可变借用。
    fn engine(&self) -> Option<&Engine> {
        self.assembled.as_ref().map(|assembly| &assembly.engine)
    }

    /// 正在组句（拼音缓冲非空）。
    fn composing(&self) -> bool {
        self.engine()
            .is_some_and(|engine| !engine.composition().is_empty())
    }

    /// 分流时要知道的当下状态。
    fn route(&self) -> Route {
        Route {
            composing: self.composing(),
            candidates: self.layout.len(),
            english: self.english,
            // 英文模式下的标点保持半角：公文里的小数点、括号都是半角。
            full_width: !self.english && self.settings.full_width_punctuation,
            // 辅码表真的装上了才算开着：配了方案但没导入表时，大写字母照旧是临时英文。
            fuma: self.engine().is_some_and(|engine| engine.fuma_enabled()),
        }
    }

    /// 丢掉这段组句。文本框里没有留过东西，所以只要清引擎与显示状态。
    fn drop_composition(&mut self) {
        if let Some(assembly) = self.assembled.as_mut()
            && !assembly.engine.composition().is_empty()
        {
            assembly.engine.clear();
            // 下一个上屏的词按句首记，别把跑题的上下文带到别处。
            assembly.engine.break_chain();
        }
        self.layout = empty_layout(self.settings.page_size);
        self.preedit = Preedit::default();
        self.pending_commit = None;
        self.forget_window();
        self.highlight = 0;
    }

    /// 重查候选：缓冲变了（`reset_highlight`）或只挪了光标。
    pub(super) fn refresh(&mut self, reset_highlight: bool) {
        let Some(engine) = self.engine() else {
            return;
        };
        let empty = empty_layout(self.settings.page_size);
        let (preedit, layout) = if engine.composition().is_empty() {
            (Preedit::default(), empty)
        } else {
            match engine.query() {
                Ok(query) => (
                    Preedit {
                        text: query.marked_text(),
                        caret: query.marked_cursor(),
                    },
                    CandidateLayout::new(query.candidates.items.clone(), self.settings.page_size),
                ),
                // 拼音切不动（`v`、`Ai` 的 `i` 这类不成音节的键）：拼音串照样要画出来，
                // 候选是空的。这里**不能**用 `CandidateLayout::default()`——它的每页格数
                // 是 0，候选窗拿它算页数会除零 panic，整个应用当场退出。
                Err(_) => {
                    let composition = engine.composition();
                    let text = composition.text().to_owned();
                    let caret = text[..composition.cursor()].chars().count();
                    (Preedit { text, caret }, empty)
                }
            }
        };
        self.preedit = preedit;
        self.layout = layout;
        if reset_highlight {
            self.highlight = 0;
        }
        let count = self.layout.len();
        if self.highlight >= count {
            self.highlight = count.saturating_sub(1);
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

    /// 一次功能键 / 修饰键事件：判 Shift 单击、放过组合键，然后分流。
    fn route_key(
        &mut self,
        key: egui::Key,
        pressed: bool,
        repeat: bool,
        modifiers: egui::Modifiers,
        kept: &mut Vec<egui::Event>,
    ) -> bool {
        if !pressed {
            // Shift 按下到抬起之间没有别的键插进来，就是一次单击：切中英。
            if is_shift(key) && self.shift_alone {
                self.shift_alone = false;
                self.english = !self.english;
            }
            return false;
        }
        if modifiers.ctrl || modifiers.alt || modifiers.command {
            self.shift_alone = false;
            return false;
        }
        if is_shift(key) {
            if !repeat {
                self.shift_alone = true;
            }
            return false;
        }
        self.shift_alone = false;
        match Key::from_egui(key) {
            Some(ime_key) => self.handle(ime_key, kept),
            None => false,
        }
    }

    /// 分流一次按键并执行。返回是否吃掉（不再交给应用）。
    fn handle(&mut self, key: Key, kept: &mut Vec<egui::Event>) -> bool {
        let action = keys::route(key, self.route(), &self.settings);
        if action == Action::Passthrough {
            return false;
        }
        let outcome = self.execute_guarded(action);
        if let Some(text) = outcome.commit {
            // 上屏走 egui 的文本插入通路：在光标处插入、替换选区、进撤销栈，
            // 与用户手打的字没有区别。
            kept.push(egui::Event::Text(text));
        }
        if outcome.recompose {
            self.refresh(true);
        }
        outcome.consumed
    }

    /// 拦 panic 地执行一次动作。
    pub(super) fn execute_guarded(&mut self, action: Action) -> Outcome {
        match catch_unwind(AssertUnwindSafe(|| self.execute(action))) {
            Ok(outcome) => outcome,
            Err(_) => {
                // 引擎里的 `RefCell` 可能停在借出状态，再调一定还会 panic：
                // 整个卸掉，退回系统输入法，直到设置里关掉再打开。
                eprintln!("[ime] 输入法处理按键时 panic，已卸载引擎并退回系统输入法");
                self.assembled = None;
                self.drop_composition();
                Outcome::PASSTHROUGH
            }
        }
    }

    /// 执行一次动作。
    ///
    /// 每个要用引擎的分支单独借一次：壳这边的字段（`layout` / `highlight`）
    /// 与引擎在同一时刻只借一边，不用绕借用检查器。
    fn execute(&mut self, action: Action) -> Outcome {
        match action {
            Action::Passthrough => Outcome::PASSTHROUGH,
            Action::Navigate(navigation) => self.navigate(navigation),
            Action::Push(c) => {
                let Some(engine) = self.engine_mut() else {
                    return Outcome::PASSTHROUGH;
                };
                engine.push(c);
                Outcome::CHANGED
            }
            Action::Backspace => {
                let Some(engine) = self.engine_mut() else {
                    return Outcome::PASSTHROUGH;
                };
                engine.backspace();
                Outcome::CHANGED
            }
            Action::Clear => {
                // 丢掉整段：谁都不要，包括应用（与 Esc 在系统输入法里的语义一致）。
                let Some(engine) = self.engine_mut() else {
                    return Outcome::PASSTHROUGH;
                };
                engine.clear();
                Outcome::CHANGED
            }
            Action::CommitHighlighted => {
                let candidate = self.layout.candidate(self.highlight).cloned();
                let Some(engine) = self.engine_mut() else {
                    return Outcome::PASSTHROUGH;
                };
                // 没有候选（缓冲还没切出音节）时把缓冲原样上屏。
                let text = match candidate {
                    Some(candidate) => engine.commit(&candidate),
                    None => engine.take_raw(),
                };
                Outcome::commit(text)
            }
            Action::CommitIndex(index) => {
                let page_size = self.settings.page_size.max(1);
                let page = self.highlight / page_size;
                let candidate = self.layout.candidate(page * page_size + index).cloned();
                let Some(engine) = self.engine_mut() else {
                    return Outcome::PASSTHROUGH;
                };
                let Some(candidate) = candidate else {
                    return Outcome::PASSTHROUGH;
                };
                Outcome::commit(engine.commit(&candidate))
            }
            Action::CommitRaw => {
                let Some(engine) = self.engine_mut() else {
                    return Outcome::PASSTHROUGH;
                };
                Outcome::commit(engine.take_raw())
            }
            Action::Punctuate(c) => {
                let Some(engine) = self.engine_mut() else {
                    return Outcome::PASSTHROUGH;
                };
                match engine.punctuate(c) {
                    Some(text) => Outcome::commit(text.to_owned()),
                    None => {
                        // 引擎转不了这个字符（英文、数字后的点、括号……）：交给应用，
                        // 但告诉引擎它上屏了，好让选词链断在这里。
                        engine.note_passthrough(c);
                        Outcome::PASSTHROUGH
                    }
                }
            }
            Action::Insert(c) => {
                let Some(engine) = self.engine_mut() else {
                    return Outcome::PASSTHROUGH;
                };
                // 中文模式里的临时英文：先把拼音原样上屏，再插这个字母。
                let prefix = (!engine.composition().is_empty()).then(|| engine.take_raw());
                engine.note_passthrough(c);
                Outcome::commit(match prefix {
                    Some(prefix) => format!("{prefix}{c}"),
                    None => c.to_string(),
                })
            }
        }
    }

    /// 挪高亮 / 翻页 / 挪拼音光标。光标没有上屏语义，只重算拼音串的位置。
    fn navigate(&mut self, navigation: keys::Navigation) -> Outcome {
        use keys::Navigation;
        let count = self.layout.len();
        let page_size = self.settings.page_size.max(1);
        match navigation {
            Navigation::Highlight(delta) => {
                self.highlight = moved_highlight(self.highlight, count, delta);
            }
            Navigation::Page(step) => {
                let (highlight, turned) = turned_page(self.highlight, count, page_size, step);
                self.highlight = highlight;
                if turned && let Some(engine) = self.engine_mut() {
                    engine.note_page_turn();
                }
            }
            Navigation::CursorLeft => {
                if let Some(engine) = self.engine_mut() {
                    engine.move_cursor_left();
                }
            }
            Navigation::CursorRight => {
                if let Some(engine) = self.engine_mut() {
                    engine.move_cursor_right();
                }
            }
            Navigation::CursorHome => {
                if let Some(engine) = self.engine_mut() {
                    engine.move_cursor_home();
                }
            }
            Navigation::CursorEnd => {
                if let Some(engine) = self.engine_mut() {
                    engine.move_cursor_end();
                }
            }
        }
        Outcome::NAVIGATED
    }

    /// 引擎（可变）。
    fn engine_mut(&mut self) -> Option<&mut Engine> {
        self.assembled.as_mut().map(|assembly| &mut assembly.engine)
    }
}

impl Drop for Ime {
    fn drop(&mut self) {
        // 退出前把学习数据落一次盘，别让最后几十次选词白学。
        self.flush();
    }
}

/// 没有候选时的布局。**不用 `CandidateLayout::default()`**：它的每页格数是 0，
/// `pages()` 会拿它作除数，候选窗一画就除零 panic（panic 从 winit 的窗口回调里
/// 穿出去，应用直接闪退）。每页格数与设置一致，空布局与有候选时同一套算法。
fn empty_layout(page_size: usize) -> CandidateLayout {
    CandidateLayout::new(Vec::new(), page_size.max(1))
}

/// 高亮挪 `delta`，夹在 `[0, count-1]` 里；没有候选就归零。
fn moved_highlight(highlight: usize, count: usize, delta: isize) -> usize {
    if count == 0 {
        return 0;
    }
    (highlight as isize + delta).clamp(0, count as isize - 1) as usize
}

/// 翻 `step` 页：返回（新的高亮，是否真的翻了页）。高亮落到目标页第一个候选。
fn turned_page(highlight: usize, count: usize, page_size: usize, step: isize) -> (usize, bool) {
    if count == 0 {
        return (0, false);
    }
    let pages = count.div_ceil(page_size);
    let current = highlight / page_size;
    let target = (current as isize + step).clamp(0, pages as isize - 1) as usize;
    ((target * page_size).min(count - 1), target != current)
}

/// Shift 的左右两键都算：egui 把修饰键也当普通键报，没有笼统的 `Key::Shift`。
fn is_shift(key: egui::Key) -> bool {
    matches!(key, egui::Key::ShiftLeft | egui::Key::ShiftRight)
}

/// 单个字符；多字符的 `Text`（粘贴、其他输入源）不碰。
fn single_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

/// 找数据并装配引擎。任何一环不通都返回 `None`：输入法不可用，键盘交给系统输入法。
fn load() -> Option<Assembly> {
    let data = data::find()?;
    let learning = data::learning_dir();
    let extra = learning
        .as_ref()
        .map(|dir| dir.join("dicts"))
        .unwrap_or_default();
    match engine::assemble(&data, learning.as_deref(), &extra) {
        Ok(assembly) => {
            eprintln!(
                "[ime] 输入法已就绪：词库 {}（{} 条）、语言模型 {:?}、{} ms",
                data.dict.display(),
                assembly.dictionary_entries,
                assembly.bigrams,
                assembly.load_ms,
            );
            Some(assembly)
        }
        Err(error) => {
            eprintln!("[ime] 输入法数据不可用，退回系统输入法：{error:#}");
            None
        }
    }
}

#[cfg(test)]
mod tests;
