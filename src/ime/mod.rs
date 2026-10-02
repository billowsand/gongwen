//! 应用内词表输入法：编码精确查表，词表维护后立即生效。
mod add_word;
mod candidates;
mod cursor;
mod data;
mod encoder;
mod exempt;
mod keys;
mod lookup;
mod manage;
mod session;
mod table;
pub(crate) use cursor::follow_cursor;
pub(crate) use exempt::exempt;
pub(crate) use session::Ime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ImeSettings {
    pub enabled: bool,
    pub full_width_punctuation: bool,
    pub page_size: usize,
    pub page_keys: (char, char),
    pub vertical: bool,
    pub font_percent: u16,
    pub auto_commit: bool,
    pub fifth_commit: bool,
    pub code_hint: bool,
    pub phrase_hint: bool,
    pub prefix_hint: bool,
}
pub(crate) const MAX_PAGE_SIZE: usize = 9;
pub(crate) const PAGE_KEY_OPTIONS: [&str; 3] = ["[]", ",.", "-="];
pub(crate) const FONT_PERCENT_OPTIONS: [u16; 3] = [100, 125, 150];
impl Default for ImeSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            full_width_punctuation: true,
            page_size: 5,
            page_keys: ('[', ']'),
            vertical: false,
            font_percent: 100,
            auto_commit: false,
            fifth_commit: true,
            code_hint: true,
            phrase_hint: true,
            prefix_hint: false,
        }
    }
}
impl ImeSettings {
    pub fn from_config(config: &crate::models::ImeConfig) -> Self {
        let keys = if PAGE_KEY_OPTIONS.contains(&config.page_keys.as_str()) {
            &config.page_keys
        } else {
            "[]"
        };
        let mut chars = keys.chars();
        Self {
            enabled: config.enabled,
            full_width_punctuation: config.full_width_punctuation,
            page_size: config.page_size.clamp(1, MAX_PAGE_SIZE),
            page_keys: (chars.next().unwrap(), chars.next().unwrap()),
            vertical: config.candidate_vertical,
            font_percent: config.candidate_font_percent.clamp(100, 200),
            auto_commit: config.auto_commit,
            fifth_commit: config.fifth_commit,
            code_hint: config.code_hint,
            phrase_hint: config.phrase_hint,
            prefix_hint: config.prefix_hint,
        }
    }
}
