//! 按键分流：把一次按键变成交给引擎的动作。
//!
//! 规则与上游 Windows 壳的 `dispatch/key/input.rs` 对齐（那一份又和 macOS 壳对齐）：
//!
//! * 中文模式小写字母进拼音缓冲；大写字母是**临时打英文**——先把拼音原样上屏，再插字母。
//! * 英文模式（`Shift` 单击切过来的持久状态）字母直接进文本框，不进缓冲、不出候选。
//! * 组句中的 `1`–`9` 选当前页第几个、空格上屏高亮、回车原样上屏、`[` `]` 翻页、
//!   上下挪高亮、左右 / Home / End 挪拼音光标、退格删一个字母、Esc 丢掉整段。
//! * 组句中的 `Delete` 删光标后的一个字母，`Ctrl+退格` / `Ctrl+←` / `Ctrl+→` 按音节删、按音节挪，
//!   `Ctrl+数字` 删掉当前页第几个候选的学习记录（学错的词、误造的用户词）。
//! * 没在组句时只有标点问一句引擎（要不要转全角），其余一律交给应用；退格顺带告诉引擎
//!   删的是刚上屏的字（选错了删掉重打，那次学习就退回去），回车、方向键、快捷键这类
//!   会让光标离开刚才上屏位置的键顺带断开上文（下一个词按句首记）。
//! * 其余带 Ctrl / Alt / Command 的组合键（以及 `Tab`）交给应用；组句中先把拼音原样上屏，
//!   免得快捷键作用在候选窗背后的正文上、拼音却还悬着（`Ctrl+V` 粘到了拼音前面这类）。
//!
//! 分流是纯函数：只吃 [`Route`] 里的状态，不碰引擎，所以不用真实词库就能测。

use eframe::egui;

use super::ImeSettings;

/// 与平台无关的一次按键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    /// 可打印字符。
    Char(char),

    /// 退格。
    Backspace,

    /// Esc。
    Escape,

    /// 回车。
    Enter,

    /// 上下：挪高亮。
    Up,
    Down,

    /// 左右：挪拼音光标。
    Left,
    Right,

    /// Home / End：拼音光标到首 / 尾。
    Home,
    End,

    /// PageUp / PageDown：翻页。
    PageUp,
    PageDown,

    /// Delete：删光标后的一个字母。
    Delete,

    /// Tab。
    Tab,

    /// Ctrl / Alt + 退格：删光标前的一个音节。
    SyllableBackspace,

    /// Ctrl / Alt + 左右：拼音光标按音节挪。
    SyllableLeft,
    SyllableRight,

    /// Ctrl（macOS 上是 Command）+ 1–9：删当前页第几个候选的学习记录（0 起）。
    Forget(usize),

    /// 其余带 Ctrl / Alt / Command 的组合键：快捷键，归应用。
    Shortcut,
}

impl Key {
    /// 认 egui 的主键。返回 `None` 表示这个键与输入法无关（字母靠字符事件走）。
    ///
    /// 修饰键本身（单按一下 Ctrl）也是 `None`：它还不是快捷键，不该把拼音冲上屏。
    pub(crate) fn from_egui(key: egui::Key, modifiers: egui::Modifiers) -> Option<Self> {
        if is_modifier(key) {
            return None;
        }
        if (modifiers.ctrl || modifiers.command)
            && !modifiers.alt
            && !modifiers.shift
            && let Some(index) = digit_index(key)
        {
            return Some(Self::Forget(index));
        }
        if modifiers.ctrl || modifiers.alt || modifiers.command || modifiers.mac_cmd {
            // Windows / Linux 的 Ctrl、macOS 的 Option 是按词挪的那一档；macOS 的 Command
            // 是整行，输入法里没有对应，当普通快捷键。
            let word = (modifiers.ctrl || modifiers.alt) && !modifiers.mac_cmd;
            return Some(match key {
                egui::Key::Backspace if word => Self::SyllableBackspace,
                egui::Key::ArrowLeft if word => Self::SyllableLeft,
                egui::Key::ArrowRight if word => Self::SyllableRight,
                _ => Self::Shortcut,
            });
        }
        Some(match key {
            egui::Key::Backspace => Self::Backspace,
            egui::Key::Escape => Self::Escape,
            egui::Key::Enter => Self::Enter,
            egui::Key::ArrowUp => Self::Up,
            egui::Key::ArrowDown => Self::Down,
            egui::Key::ArrowLeft => Self::Left,
            egui::Key::ArrowRight => Self::Right,
            egui::Key::Home => Self::Home,
            egui::Key::End => Self::End,
            egui::Key::PageUp => Self::PageUp,
            egui::Key::PageDown => Self::PageDown,
            egui::Key::Delete => Self::Delete,
            egui::Key::Tab => Self::Tab,
            _ => return None,
        })
    }
}

/// 数字键 1–9 → 0–8。
fn digit_index(key: egui::Key) -> Option<usize> {
    use egui::Key::*;
    [Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9]
        .iter()
        .position(|&digit| digit == key)
}

/// 修饰键本身。egui 把它们也当普通键报。
fn is_modifier(key: egui::Key) -> bool {
    use egui::Key::*;
    matches!(
        key,
        ShiftLeft
            | ShiftRight
            | ControlLeft
            | ControlRight
            | AltLeft
            | AltRight
            | SuperLeft
            | SuperRight
    )
}

/// 这次按键要做什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    /// 进拼音缓冲区。
    Push(char),

    /// 退格：删一个字母。
    Backspace,

    /// 丢掉整段组句。
    Clear,

    /// 上屏当前高亮；没有候选就把缓冲原样上屏。
    CommitHighlighted,

    /// 缓冲原样上屏（回车）。
    CommitRaw,

    /// 缓冲原样上屏，但这次按键**照样交给应用**（快捷键、Tab、粘贴）。
    FlushRaw,

    /// 删光标后的一个字母。
    DeleteForward,

    /// 删光标前的一个音节。
    DeleteSyllable,

    /// 删当前页第几个候选的学习记录（0 起）。
    Forget(usize),

    /// 选当前页第几个候选（0 起）。
    CommitIndex(usize),

    /// 交引擎转全角标点；转不了就原样交给应用。
    Punctuate(char),

    /// 直接插进文本框；进之前先把拼音缓冲原样上屏。
    Insert(char),

    /// 没在组句时的退格：交给应用删正文，同时告诉引擎删的是刚上屏的字。
    NoteBackspace,

    /// 光标要离开刚才上屏的位置（换行、方向键、快捷键）：交给应用，同时断开上文。
    BreakChain,

    /// 组句内的导航。
    Navigate(Navigation),

    /// 交给应用。
    Passthrough,
}

/// 组句内的导航动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Navigation {
    /// 高亮挪 `±1`。
    Highlight(isize),

    /// 翻 `±1` 页。
    Page(isize),

    /// 拼音光标左移一个字母。
    CursorLeft,

    /// 拼音光标右移一个字母。
    CursorRight,

    /// 拼音光标到开头。
    CursorHome,

    /// 拼音光标到末尾。
    CursorEnd,

    /// 拼音光标左移一个音节。
    SyllableLeft,

    /// 拼音光标右移一个音节。
    SyllableRight,
}

/// 分流时要知道的壳状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Route {
    /// 正在组句（拼音缓冲非空）。
    pub(crate) composing: bool,

    /// 当前一页有几个候选。
    pub(crate) candidates: usize,

    /// 英文模式：字母直接进文本框。
    pub(crate) english: bool,

    /// 当前模式下的标点要不要转全角。
    pub(crate) full_width: bool,

    /// 开着双拼辅码：组句中的大写字母是辅码键，进缓冲区而不是当临时英文。
    pub(crate) fuma: bool,
}

/// 一次按键的分流结果。
pub(crate) fn route(key: Key, route: Route, settings: &ImeSettings) -> Action {
    if route.english {
        return route_key_english(key, route);
    }
    match key {
        Key::Char(c) => route_char(c, route, settings),
        Key::Backspace if !route.composing => Action::NoteBackspace,
        Key::Escape if !route.composing => Action::Passthrough,
        _ if !route.composing => Action::BreakChain,
        Key::Backspace => Action::Backspace,
        Key::Escape => Action::Clear,
        Key::Enter => Action::CommitRaw,
        Key::Up => Action::Navigate(Navigation::Highlight(-1)),
        Key::Down => Action::Navigate(Navigation::Highlight(1)),
        Key::PageUp => Action::Navigate(Navigation::Page(-1)),
        Key::PageDown => Action::Navigate(Navigation::Page(1)),
        Key::Left => Action::Navigate(Navigation::CursorLeft),
        Key::Right => Action::Navigate(Navigation::CursorRight),
        Key::Home => Action::Navigate(Navigation::CursorHome),
        Key::End => Action::Navigate(Navigation::CursorEnd),
        Key::Delete => Action::DeleteForward,
        Key::SyllableBackspace => Action::DeleteSyllable,
        Key::SyllableLeft => Action::Navigate(Navigation::SyllableLeft),
        Key::SyllableRight => Action::Navigate(Navigation::SyllableRight),
        Key::Forget(index) if route.candidates > 0 => Action::Forget(index),
        Key::Tab | Key::Forget(_) | Key::Shortcut => Action::FlushRaw,
    }
}

/// 英文模式：只有字母和标点需要过一眼，功能键全交给应用。
fn route_key_english(key: Key, route: Route) -> Action {
    match key {
        Key::Char(c) if c.is_ascii_alphabetic() => Action::Insert(c),
        Key::Char(c) => route_punctuation(c, route),
        Key::Backspace => Action::NoteBackspace,
        Key::Escape => Action::Passthrough,
        _ => Action::BreakChain,
    }
}

/// 中文模式下的可打印字符。
fn route_char(c: char, route: Route, settings: &ImeSettings) -> Action {
    if c.is_ascii_lowercase() {
        return Action::Push(c);
    }
    if c.is_ascii_uppercase() {
        // 辅码开着时，组句中的大写字母是辅码键（`栏` 打 `lanM`），进缓冲区参与过滤；
        // 其余情况是临时英文。
        if route.composing && route.fuma {
            return Action::Push(c);
        }
        return Action::Insert(c);
    }
    if !route.composing {
        return route_punctuation(c, route);
    }
    if ('1'..='9').contains(&c) && route.candidates > 0 {
        return Action::CommitIndex(c as usize - '1' as usize);
    }
    if let Some(step) = page_step(c, settings.page_keys) {
        return Action::Navigate(Navigation::Page(step));
    }
    if c == ' ' {
        return Action::CommitHighlighted;
    }
    // 组句中的其余字符进缓冲：`'` 是音节分隔符，`no-way` 这类英文直输段也靠它。
    Action::Push(c)
}

/// 标点：开着全角就问引擎要全角那一份，否则原样交给应用。
fn route_punctuation(c: char, route: Route) -> Action {
    if route.full_width {
        Action::Punctuate(c)
    } else {
        Action::Passthrough
    }
}

/// 翻页键：`-1` 上一页，`+1` 下一页。
fn page_step(c: char, page_keys: (char, char)) -> Option<isize> {
    if c == page_keys.0 {
        Some(-1)
    } else if c == page_keys.1 {
        Some(1)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> ImeSettings {
        ImeSettings::default()
    }

    /// 组句中的状态。
    fn composing(candidates: usize) -> Route {
        Route {
            composing: true,
            candidates,
            english: false,
            full_width: true,
            fuma: false,
        }
    }

    /// 没在组句的状态。
    fn idle() -> Route {
        Route {
            composing: false,
            candidates: 0,
            english: false,
            full_width: true,
            fuma: false,
        }
    }

    fn char_route(c: char, state: Route) -> Action {
        route(Key::Char(c), state, &settings())
    }

    #[test]
    fn lowercase_letters_go_into_the_buffer() {
        assert_eq!(char_route('k', idle()), Action::Push('k'));
        assert_eq!(char_route('k', composing(5)), Action::Push('k'));
    }

    /// 大写字母是临时英文：先把拼音原样上屏，再插这个字母。
    #[test]
    fn uppercase_letters_are_temporary_english() {
        assert_eq!(char_route('K', composing(5)), Action::Insert('K'));
        assert_eq!(char_route('K', idle()), Action::Insert('K'));
    }

    #[test]
    fn space_commits_the_highlighted_candidate_while_composing() {
        assert_eq!(char_route(' ', composing(5)), Action::CommitHighlighted);
    }

    /// 没在组句时空格就是个普通空格：问一句引擎（转不了就交给应用）。
    #[test]
    fn space_is_punctuation_when_idle() {
        assert_eq!(char_route(' ', idle()), Action::Punctuate(' '));
    }

    /// 只有当前页有候选时数字才选词，否则是普通字符。
    #[test]
    fn digits_select_candidates_only_when_there_are_any() {
        assert_eq!(char_route('3', composing(5)), Action::CommitIndex(2));
        assert_eq!(char_route('3', composing(0)), Action::Push('3'));
        assert_eq!(char_route('3', idle()), Action::Punctuate('3'));
    }

    /// 数字键 `0` 不选词。
    #[test]
    fn zero_is_not_a_candidate_shortcut() {
        assert_eq!(char_route('0', composing(5)), Action::Push('0'));
    }

    #[test]
    fn page_keys_turn_pages_only_while_composing() {
        assert_eq!(
            char_route('[', composing(5)),
            Action::Navigate(Navigation::Page(-1))
        );
        assert_eq!(
            char_route(']', composing(5)),
            Action::Navigate(Navigation::Page(1))
        );
        assert_eq!(char_route('[', idle()), Action::Punctuate('['));
    }

    /// 组句里的标点是英文直输段的一部分（`no-way`），进缓冲。
    #[test]
    fn punctuation_inside_a_composition_goes_into_the_buffer() {
        assert_eq!(char_route('-', composing(5)), Action::Push('-'));
        assert_eq!(char_route('\'', composing(5)), Action::Push('\''));
    }

    /// 中文标点在没组句时问引擎转不转全角；关掉全角就直接交给应用。
    #[test]
    fn punctuation_asks_the_engine_only_when_full_width_is_on() {
        assert_eq!(char_route('，', idle()), Action::Punctuate('，'));
        let half = Route {
            full_width: false,
            ..idle()
        };
        assert_eq!(char_route('，', half), Action::Passthrough);
    }

    /// 没在组句时的功能键交给应用，并断开上文：光标离开了刚才上屏的位置。
    #[test]
    fn function_keys_break_the_chain_when_idle() {
        for key in [
            Key::Enter,
            Key::Up,
            Key::Down,
            Key::PageUp,
            Key::PageDown,
            Key::Left,
            Key::Right,
            Key::Home,
            Key::End,
            Key::Delete,
            Key::Tab,
            Key::SyllableBackspace,
            Key::SyllableLeft,
            Key::SyllableRight,
            Key::Forget(0),
            Key::Shortcut,
        ] {
            assert_eq!(
                route(key, idle(), &settings()),
                Action::BreakChain,
                "{key:?} 没在组句时应当交给应用并断开上文"
            );
        }
    }

    #[test]
    fn function_keys_drive_the_composition() {
        assert_eq!(
            route(Key::Backspace, composing(5), &settings()),
            Action::Backspace
        );
        assert_eq!(route(Key::Escape, composing(5), &settings()), Action::Clear);
        assert_eq!(
            route(Key::Enter, composing(5), &settings()),
            Action::CommitRaw
        );
        assert_eq!(
            route(Key::Up, composing(5), &settings()),
            Action::Navigate(Navigation::Highlight(-1))
        );
        assert_eq!(
            route(Key::Down, composing(5), &settings()),
            Action::Navigate(Navigation::Highlight(1))
        );
        assert_eq!(
            route(Key::Left, composing(5), &settings()),
            Action::Navigate(Navigation::CursorLeft)
        );
        assert_eq!(
            route(Key::End, composing(5), &settings()),
            Action::Navigate(Navigation::CursorEnd)
        );
    }

    /// Esc 不挪光标，原样交给应用。
    #[test]
    fn idle_escape_is_passed_through() {
        assert_eq!(route(Key::Escape, idle(), &settings()), Action::Passthrough);
    }

    /// 没在组句时的退格删的是正文：交给应用，并告诉引擎（英文模式也一样）。
    #[test]
    fn idle_backspace_is_noted() {
        assert_eq!(
            route(Key::Backspace, idle(), &settings()),
            Action::NoteBackspace
        );
        let english = Route {
            english: true,
            ..idle()
        };
        assert_eq!(
            route(Key::Backspace, english, &settings()),
            Action::NoteBackspace
        );
    }

    /// 组句中：Delete 删后一个字母，Ctrl+退格 / 左右按音节，其余快捷键与 Tab 先上屏拼音再交给应用。
    #[test]
    fn editing_keys_while_composing_stay_in_the_composition() {
        let at = |key| route(key, composing(5), &settings());
        assert_eq!(at(Key::Delete), Action::DeleteForward);
        assert_eq!(at(Key::SyllableBackspace), Action::DeleteSyllable);
        assert_eq!(
            at(Key::SyllableLeft),
            Action::Navigate(Navigation::SyllableLeft)
        );
        assert_eq!(
            at(Key::SyllableRight),
            Action::Navigate(Navigation::SyllableRight)
        );
        assert_eq!(at(Key::Forget(1)), Action::Forget(1));
        assert_eq!(
            route(Key::Forget(1), composing(0), &settings()),
            Action::FlushRaw,
            "没有候选可删时当普通快捷键"
        );
        assert_eq!(at(Key::Shortcut), Action::FlushRaw);
        assert_eq!(at(Key::Tab), Action::FlushRaw);
    }

    /// 修饰键组合认成哪个键：Ctrl / Alt 的退格与左右按音节，其余是快捷键，单按修饰键不算。
    #[test]
    fn modifier_combinations_are_recognized() {
        let ctrl = egui::Modifiers::CTRL;
        let alt = egui::Modifiers::ALT;
        assert_eq!(
            Key::from_egui(egui::Key::Backspace, ctrl),
            Some(Key::SyllableBackspace)
        );
        assert_eq!(
            Key::from_egui(egui::Key::ArrowLeft, alt),
            Some(Key::SyllableLeft)
        );
        assert_eq!(
            Key::from_egui(egui::Key::ArrowRight, ctrl),
            Some(Key::SyllableRight)
        );
        assert_eq!(Key::from_egui(egui::Key::V, ctrl), Some(Key::Shortcut));
        assert_eq!(Key::from_egui(egui::Key::Num3, ctrl), Some(Key::Forget(2)));
        assert_eq!(
            Key::from_egui(egui::Key::Num3, ctrl | egui::Modifiers::SHIFT),
            Some(Key::Shortcut)
        );
        assert_eq!(Key::from_egui(egui::Key::Num0, ctrl), Some(Key::Shortcut));
        assert_eq!(Key::from_egui(egui::Key::Z, ctrl), Some(Key::Shortcut));
        assert_eq!(
            Key::from_egui(egui::Key::ArrowLeft, egui::Modifiers::MAC_CMD),
            Some(Key::Shortcut)
        );
        assert_eq!(Key::from_egui(egui::Key::ControlLeft, ctrl), None);
        assert_eq!(Key::from_egui(egui::Key::A, egui::Modifiers::NONE), None);
        assert_eq!(
            Key::from_egui(egui::Key::Delete, egui::Modifiers::NONE),
            Some(Key::Delete)
        );
    }

    /// 英文模式：字母直接进文本框，标点按当前模式的全角设置走，功能键交给应用。
    #[test]
    fn english_mode_inserts_letters_directly() {
        let english = Route {
            english: true,
            ..idle()
        };
        assert_eq!(char_route('a', english), Action::Insert('a'));
        assert_eq!(char_route('A', english), Action::Insert('A'));
        assert_eq!(char_route('，', english), Action::Punctuate('，'));
        assert_eq!(route(Key::Enter, english, &settings()), Action::BreakChain);
        assert_eq!(
            route(Key::Escape, english, &settings()),
            Action::Passthrough
        );
        // 英文模式下敲的数字不选词，也不会进拼音缓冲。
        assert_eq!(char_route('3', english), Action::Punctuate('3'));
    }

    /// 开着辅码时，组句中的大写字母是辅码键：进缓冲区，不当临时英文。
    #[test]
    fn uppercase_letters_are_fuma_keys_when_fuma_is_on() {
        let fuma = Route {
            fuma: true,
            ..composing(5)
        };
        assert_eq!(char_route('M', fuma), Action::Push('M'));
        // 没在组句时的大写字母仍然是临时英文（辅码只管组句内的过滤）。
        let idle_fuma = Route {
            fuma: true,
            ..idle()
        };
        assert_eq!(char_route('M', idle_fuma), Action::Insert('M'));
        // 英文模式下辅码不参与。
        let english_fuma = Route {
            english: true,
            fuma: true,
            ..idle()
        };
        assert_eq!(char_route('M', english_fuma), Action::Insert('M'));
    }

    /// 翻页键可以配置。
    #[test]
    fn page_keys_follow_the_settings() {
        let settings = ImeSettings {
            page_keys: (',', '.'),
            ..ImeSettings::default()
        };
        assert_eq!(
            route(Key::Char(','), composing(5), &settings),
            Action::Navigate(Navigation::Page(-1))
        );
        assert_eq!(
            route(Key::Char('.'), composing(5), &settings),
            Action::Navigate(Navigation::Page(1))
        );
        // 配成 `,.` 之后 `[` 不再是翻页键，回到普通字符。
        assert_eq!(
            route(Key::Char('['), composing(5), &settings),
            Action::Push('[')
        );
    }
}
