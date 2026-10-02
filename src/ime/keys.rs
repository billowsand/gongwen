//! 词表输入法按键分流。编码只接受小写字母，快捷键仍交给编辑器。
use super::ImeSettings;
use eframe::egui;
#[derive(Debug, Clone, Copy)]
pub(crate) enum Key {
    Char(char),
    Backspace,
    Escape,
    Enter,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    /// Ctrl+Delete：组码时屏蔽高亮候选，不组码时交给编辑器删词。
    Block,
    Shortcut,
}
impl Key {
    pub fn from_egui(key: egui::Key, modifiers: egui::Modifiers) -> Option<Self> {
        use egui::Key::*;
        if matches!(
            key,
            ShiftLeft
                | ShiftRight
                | ControlLeft
                | ControlRight
                | AltLeft
                | AltRight
                | SuperLeft
                | SuperRight
        ) {
            return None;
        }
        if key == Delete && modifiers.command && !modifiers.shift && !modifiers.alt {
            return Some(Self::Block);
        }
        if modifiers.ctrl || modifiers.alt || modifiers.command || modifiers.mac_cmd {
            return Some(Self::Shortcut);
        }
        Some(match key {
            Backspace => Self::Backspace,
            Escape => Self::Escape,
            Enter => Self::Enter,
            ArrowUp => Self::Up,
            ArrowDown => Self::Down,
            ArrowLeft => Self::Left,
            ArrowRight => Self::Right,
            Home => Self::Home,
            End => Self::End,
            PageUp => Self::PageUp,
            PageDown => Self::PageDown,
            Delete => Self::Delete,
            Tab => Self::Shortcut,
            _ => return None,
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    Push(char),
    Backspace,
    Clear,
    CommitHighlighted,
    CommitRaw,
    FlushRaw,
    DeleteForward,
    CommitIndex(usize),
    Punctuate(char),
    Insert(char),
    Navigate(Navigation),
    Block,
    Passthrough,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Navigation {
    Highlight(isize),
    Page(isize),
    CursorLeft,
    CursorRight,
    CursorHome,
    CursorEnd,
}
pub(crate) struct Route {
    pub composing: bool,
    pub candidates: usize,
    pub english: bool,
    pub full_width: bool,
}
pub(crate) fn route(key: Key, state: Route, settings: &ImeSettings) -> Action {
    if state.english {
        return Action::Passthrough;
    }
    match key {
        Key::Char(c) if c.is_ascii_lowercase() => Action::Push(c),
        Key::Char(c) if c.is_ascii_uppercase() => Action::Insert(c),
        Key::Char(c) if state.composing => {
            if ('1'..='9').contains(&c) && state.candidates > 0 {
                return Action::CommitIndex(c as usize - '1' as usize);
            }
            if c == settings.page_keys.0 {
                return Action::Navigate(Navigation::Page(-1));
            }
            if c == settings.page_keys.1 {
                return Action::Navigate(Navigation::Page(1));
            }
            if c == ' ' {
                return Action::CommitHighlighted;
            }
            Action::Punctuate(c)
        }
        Key::Char(c) if state.full_width && c.is_ascii_punctuation() => Action::Punctuate(c),
        _ if !state.composing => Action::Passthrough,
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
        Key::Block => Action::Block,
        Key::Shortcut => Action::FlushRaw,
        Key::Char(c) => Action::Insert(c),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> Route {
        Route {
            composing: true,
            candidates: 2,
            english: false,
            full_width: true,
        }
    }
    #[test]
    fn codes_selection_and_shortcuts() {
        let settings = ImeSettings::default();
        assert_eq!(route(Key::Char('a'), state(), &settings), Action::Push('a'));
        assert_eq!(
            route(Key::Char('2'), state(), &settings),
            Action::CommitIndex(1)
        );
        assert_eq!(
            route(Key::Char('['), state(), &settings),
            Action::Navigate(Navigation::Page(-1))
        );
        assert_eq!(route(Key::Shortcut, state(), &settings), Action::FlushRaw);
        assert_eq!(route(Key::Block, state(), &settings), Action::Block);
        let mut idle = state();
        idle.composing = false;
        assert_eq!(route(Key::Block, idle, &settings), Action::Passthrough);
        assert!(matches!(
            Key::from_egui(egui::Key::Delete, egui::Modifiers::COMMAND),
            Some(Key::Block)
        ));
        let mut english = state();
        english.english = true;
        assert_eq!(
            route(Key::Char('a'), english, &settings),
            Action::Passthrough
        );
    }
}
