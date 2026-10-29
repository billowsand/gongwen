//! 主动展开找字：每行与候选条一样多格，可见四行，滚动浏览；筛选只分单字、词组。
use super::session::{Ime, Slot};

/// 打到几码才能展开找字、才补逐码提示。一码开头动辄几千条，翻不过来；
/// 一码时候选条只列编码正好是这一码的字词。
pub(super) const MIN_CODE: usize = 2;

/// 展开面板一屏露几行；翻页键一次翻这么多行。
pub(super) const VISIBLE_ROWS: usize = 4;

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    #[default]
    All,
    Character,
    Phrase,
}

#[derive(Default)]
pub(super) struct Browser {
    pub open: bool,
    pub kind: Kind,
    /// 筛选与去重之前的全部候选：切换筛选不必重查词表。
    pub all: Vec<Slot>,
    /// 当前滚到第几行在最上面；鼠标滚动后由界面回写。
    pub top_row: usize,
    /// 按键挪了高亮、要把网格滚到这一行在最上面；界面用掉就清空。
    pub scroll_to: Option<usize>,
}

impl Browser {
    pub fn matches(&self, slot: &Slot) -> bool {
        let single = slot.text.chars().count() == 1;
        match self.kind {
            Kind::All => true,
            Kind::Character => single,
            Kind::Phrase => !single,
        }
    }
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
        self.refresh(false);
        self.mark_browser_selection();
        self.forget_window();
    }

    pub(super) fn collapse_browser(&mut self) {
        self.refresh(true);
        self.forget_window();
    }

    /// 词表改动（置顶、调序、屏蔽）后重查。面板开着就留住筛选条件，高亮跟着原来那个词走，
    /// 词没了就停在原位附近：找到一个词顺手置顶，不该把滚过的位置全丢掉。
    pub(super) fn reload_candidates(&mut self) {
        if !self.browser.open {
            self.refresh(true);
            return;
        }
        let highlight = self.highlight;
        let top_row = self.browser.top_row;
        let text = self.layout.get(highlight).map(|slot| slot.text.clone());
        self.refresh(false);
        self.highlight = text
            .and_then(|text| self.layout.iter().position(|slot| slot.text == text))
            .unwrap_or_else(|| highlight.min(self.layout.len().saturating_sub(1)));
        self.browser.top_row = top_row;
        self.browser.scroll_to = Some(top_row);
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

    pub(super) fn filter_browser(&mut self) {
        self.highlight = 0;
        self.browser.top_row = 0;
        self.browser.scroll_to = Some(0);
        self.layout = self
            .browser
            .all
            .iter()
            .filter(|slot| self.browser.matches(slot))
            .cloned()
            .collect();
        self.exact = self
            .layout
            .iter()
            .take_while(|slot| slot.code == self.preedit.text)
            .count();
        let mut known = std::collections::HashSet::new();
        self.layout.retain(|slot| known.insert(slot.text.clone()));
        self.mark_browser_selection();
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
    fn filters_before_deduplication_and_respects_hidden_words() {
        let mut ime = Ime::bare(ImeSettings::default());
        ime.table.base = table::parse(
            "ab,1=甲\naba,1=甲\nabab,1=甲乙\nabb,1=乙\nabc,1=隐藏",
            false,
        )
        .entries;
        ime.table.rebuild();
        // 屏蔽走正式词表路径；此处避免写个人词表文件。
        ime.table.personal.hidden.push(table::Entry {
            code: "abc".into(),
            text: "隐藏".into(),
        });
        for c in "ab".chars() {
            ime.execute_guarded(Action::Push(c));
        }
        ime.expand_browser();
        assert_eq!(ime.layout.iter().filter(|s| s.text == "甲").count(), 1);
        assert!(!ime.layout.iter().any(|s| s.text == "隐藏"));
        ime.browser.kind = Kind::Character;
        ime.filter_browser();
        let texts: Vec<_> = ime.layout.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["甲", "乙"]);
        assert_eq!(ime.layout[0].code, "ab");
        ime.browser.kind = Kind::Phrase;
        ime.filter_browser();
        assert_eq!(ime.layout.len(), 1);
        assert_eq!(ime.layout[0].text, "甲乙");
    }

    #[test]
    fn word_table_changes_keep_the_panel_page_and_filters() {
        let dir = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(dir.path().to_path_buf()));
        let mut ime = Ime::bare(ImeSettings::default());
        let mut text = String::from("ab,1=简码\n");
        for i in 0..20u8 {
            text.push_str(&format!("abc{},1=词{i}\n", char::from(b'a' + i)));
        }
        text.push_str("abcz,1=甲乙\nabcz,2=乙丙\n");
        ime.table.base = table::parse(&text, false).entries;
        ime.table.rebuild();
        for c in "ab".chars() {
            ime.execute_guarded(Action::Push(c));
        }
        ime.execute_guarded(Action::Navigate(Navigation::Page(1)));
        ime.browser.kind = Kind::Phrase;
        ime.filter_browser();
        // 每行 5 格，翻页一次翻可见的 4 行。
        ime.execute_guarded(Action::Navigate(Navigation::Page(1)));
        assert_eq!(ime.highlight, 20);
        assert_eq!(ime.browser.top_row, 1);
        let target = ime.layout[ime.highlight].text.clone();
        ime.execute_guarded(Action::Block);
        assert!(ime.browser.open);
        assert!(ime.browser.kind == Kind::Phrase);
        assert!(!ime.layout.iter().any(|slot| slot.text == target));
        assert_eq!(ime.highlight, 20);
        assert_eq!(ime.browser.top_row, 1);
        // 置顶后高亮跟着那个词走。
        let position = |ime: &Ime, text: &str| ime.layout.iter().position(|s| s.text == text);
        ime.highlight = position(&ime, "乙丙").unwrap();
        ime.move_word("abcz", "乙丙", 0).unwrap();
        assert!(ime.browser.open);
        assert_eq!(ime.layout[ime.highlight].text, "乙丙");
        assert!(position(&ime, "乙丙") < position(&ime, "甲乙"));
        crate::storage::set_test_config_dir(None);
    }
}
