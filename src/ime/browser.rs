//! 主动展开找字：只列单字，每行与候选条一样多格，可见四行，滚动浏览。
//!
//! 词组在候选条上照样能选；展开是为了在记不全编码时把字找出来，词不在这里找。
use super::session::Ime;

/// 打到几码才能展开找字、才补逐码提示。一码开头动辄几千条，翻不过来；
/// 一码时候选条只列编码正好是这一码的字词。
pub(super) const MIN_CODE: usize = 2;

/// 展开面板一屏露几行；翻页键一次翻这么多行。
pub(super) const VISIBLE_ROWS: usize = 4;

#[derive(Default)]
pub(super) struct Browser {
    pub open: bool,
    /// 当前滚到第几行在最上面；鼠标滚动后由界面回写。
    pub top_row: usize,
    /// 按键挪了高亮、要把网格滚到这一行在最上面；界面用掉就清空。
    pub scroll_to: Option<usize>,
}

impl Ime {
    /// 候选条每页几格；展开后是每行几格。
    pub(super) fn candidate_page_size(&self) -> usize {
        self.settings.page_size.max(1)
    }

    pub(super) fn can_browse(&self) -> bool {
        self.preedit.text.len() >= MIN_CODE
    }

    pub(super) fn expand_browser(&mut self) {
        if !self.can_browse() {
            return;
        }
        self.browser.open = true;
        self.highlight = 0;
        self.browser.top_row = 0;
        self.browser.scroll_to = Some(0);
        self.refresh(false);
        self.mark_browser_selection();
        self.forget_window();
    }

    pub(super) fn collapse_browser(&mut self) {
        self.refresh(true);
        self.forget_window();
    }

    /// 展开时布局只留单字。在同词去重之前做：先去重会让一个字被它的词组同码挤掉。
    pub(super) fn keep_characters(&mut self) {
        self.layout.retain(|slot| slot.text.chars().count() == 1);
        self.exact = self
            .layout
            .iter()
            .take_while(|slot| slot.code == self.preedit.text)
            .count();
    }

    /// 词表改动（置顶、调序、屏蔽）后重查。面板开着就不收起，高亮跟着原来那个字走，
    /// 字没了就停在原位附近：找到一个字顺手置顶，不该把滚过的位置全丢掉。
    pub(super) fn reload_candidates(&mut self) {
        if !self.browser.open {
            self.refresh(true);
            return;
        }
        let highlight = self.highlight;
        let text = self.layout.get(highlight).map(|slot| slot.text.clone());
        self.refresh(false);
        self.highlight = text
            .and_then(|text| self.layout.iter().position(|slot| slot.text == text))
            .unwrap_or_else(|| highlight.min(self.layout.len().saturating_sub(1)));
        self.browser.scroll_to = Some(self.browser.top_row);
        self.follow_highlight();
    }

    /// 高亮挪出可见的四行时，把网格滚到刚好露出它。
    pub(super) fn follow_highlight(&mut self) {
        let row = self.highlight / self.candidate_page_size();
        let top = self.browser.top_row;
        let top = if row < top {
            row
        } else if row >= top + VISIBLE_ROWS {
            row + 1 - VISIBLE_ROWS
        } else {
            return;
        };
        self.browser.top_row = top;
        self.browser.scroll_to = Some(top);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ime::{
        ImeSettings,
        keys::{Action, Navigation},
        table,
    };

    #[test]
    fn lists_characters_only_before_deduplication_and_respects_hidden_words() {
        let mut ime = Ime::bare(ImeSettings::default());
        ime.table.base = table::parse(
            "ab,1=甲乙\nab,2=甲\naba,1=甲\nabab,1=甲乙\nabb,1=乙\nabc,1=丙",
            false,
        )
        .entries;
        ime.table.rebuild();
        // 屏蔽走正式词表路径；此处避免写个人词表文件。
        ime.table.personal.hidden.push(table::Entry {
            code: "abc".into(),
            text: "丙".into(),
        });
        for c in "ab".chars() {
            ime.execute_guarded(Action::Push(c));
        }
        ime.expand_browser();
        let texts: Vec<_> = ime.layout.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["甲", "乙"]);
        // 「甲」的精确码是 ab：排在前面，算精确候选。
        assert_eq!(ime.layout[0].code, "ab");
        assert_eq!(ime.exact, 1);
        ime.collapse_browser();
        assert_eq!(ime.layout[0].text, "甲乙");
    }

    #[test]
    fn word_table_changes_keep_the_panel_position() {
        let dir = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(dir.path().to_path_buf()));
        let mut ime = Ime::bare(ImeSettings::default());
        let mut text = String::from("ab,1=戊\n");
        for i in 0..20u32 {
            let c = char::from_u32(0x4E00 + i).unwrap();
            text.push_str(&format!("abc{},1={c}\n", char::from(b'a' + i as u8)));
        }
        text.push_str("abcz,1=甲\nabcz,2=乙\n");
        ime.table.base = table::parse(&text, false).entries;
        ime.table.rebuild();
        for c in "ab".chars() {
            ime.execute_guarded(Action::Push(c));
        }
        // 每行 5 格，翻页一次翻可见的 4 行。
        ime.execute_guarded(Action::Navigate(Navigation::Page(1)));
        ime.execute_guarded(Action::Navigate(Navigation::Page(1)));
        assert_eq!(ime.highlight, 20);
        assert_eq!(ime.browser.top_row, 1);
        let target = ime.layout[ime.highlight].text.clone();
        ime.execute_guarded(Action::Block);
        assert!(ime.browser.open);
        assert!(!ime.layout.iter().any(|slot| slot.text == target));
        assert_eq!(ime.highlight, 20);
        assert_eq!(ime.browser.top_row, 1);
        // 置顶后高亮跟着那个字走。
        let position = |ime: &Ime, text: &str| ime.layout.iter().position(|s| s.text == text);
        ime.highlight = position(&ime, "乙").unwrap();
        ime.move_word("abcz", "乙", 0).unwrap();
        assert!(ime.browser.open);
        assert_eq!(ime.layout[ime.highlight].text, "乙");
        assert!(position(&ime, "乙") < position(&ime, "甲"));
        crate::storage::set_test_config_dir(None);
    }
}
