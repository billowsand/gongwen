//! 原文、别名和拼音分别匹配；原文精确命中优先，评分相同保留原有顺序。

use super::Target;
use nucleo_matcher::{
    Matcher, Utf32String,
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
};
use pinyin::ToPinyin;

pub(super) struct Entry {
    pub(super) title: String,
    pub(super) subtitle: String,
    pub(super) target: Target,
    pub(super) icon: crate::theme::Icon,
    pub(super) badge: Option<&'static str>,
    literal: String,
    keys: [Utf32String; 3],
}

impl Entry {
    pub(super) fn new(title: String, subtitle: String, aliases: &str, target: Target) -> Self {
        let literal = format!("{title} {aliases}").trim().to_lowercase();
        let (full, initials) = pinyin_keys(&literal);
        Self {
            keys: [literal.as_str().into(), full.into(), initials.into()],
            literal,
            title,
            subtitle,
            target,
            icon: match target {
                Target::Document(_) | Target::Manuscript(_) => crate::theme::Icon::FileTypeDoc,
                Target::Vocabulary(_) => crate::theme::Icon::Building,
                Target::Page(page) => page.icon(),
                Target::NewDocument => crate::theme::Icon::FilePlus,
            },
            badge: None,
        }
    }

    pub(super) fn with_icon(mut self, icon: crate::theme::Icon) -> Self {
        self.icon = icon;
        self
    }

    pub(super) fn with_badge(mut self, badge: &'static str) -> Self {
        self.badge = Some(badge);
        self
    }
}

fn pinyin_keys(text: &str) -> (String, String) {
    let mut full = String::new();
    let mut initials = String::new();
    for ch in text.chars() {
        if let Some(py) = ch.to_pinyin() {
            let syllable = py.plain().replace('ü', "v");
            full.push_str(&syllable);
            if let Some(first) = syllable.chars().next() {
                initials.push(first);
            }
        } else {
            full.extend(ch.to_lowercase());
            initials.extend(ch.to_lowercase());
        }
    }
    (full, initials)
}

pub(super) fn rank(entries: &[Entry], query: &str, matcher: &mut Matcher) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return (0..entries.len()).collect();
    }
    // 使用 new 而不是 parse：文号里的 !、^、$ 等字符都按字面值查找。
    let (full_query, initials_query) = pinyin_keys(&query);
    let patterns = [&query, &full_query, &initials_query].map(|query| {
        Pattern::new(
            query,
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Fuzzy,
        )
    });
    let mut scored = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let scores = entry.keys.iter().enumerate().filter_map(|(key, text)| {
            patterns[key]
                .score(text.slice(..), matcher)
                .map(|score| (key, score))
        });
        let Some((key, score)) = scores.max_by_key(|&(key, score)| (key == 0, score)) else {
            continue;
        };
        let priority = if entry.title.to_lowercase() == query {
            3
        } else if entry.literal.contains(&query) {
            2
        } else if key == 0 {
            1
        } else {
            0
        };
        scored.push((index, priority, score));
    }
    scored.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| b.2.cmp(&a.2))
            .then_with(|| a.0.cmp(&b.0))
    });
    scored.into_iter().map(|(index, ..)| index).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::NavPage;

    fn entry(title: &str, alias: &str) -> Entry {
        Entry::new(
            title.into(),
            String::new(),
            alias,
            Target::Page(NavPage::Manuscript),
        )
    }

    #[test]
    fn chinese_pinyin_initials_and_number_find_the_same_manuscript() {
        let entries = vec![
            entry("关于防汛值班的通知", "办〔2026〕12号"),
            entry("财务工作安排", ""),
        ];
        let mut matcher = Matcher::new(nucleo_matcher::Config::DEFAULT);
        for query in ["防汛通知", "fangxun", "FXZB", "2026 12", "防汛 tongzhi"] {
            assert_eq!(rank(&entries, query, &mut matcher), vec![0], "{query}");
        }
    }

    #[test]
    fn exact_title_beats_alias_and_ties_keep_original_order() {
        let entries = vec![
            entry("别的材料", "防汛"),
            entry("防汛", ""),
            entry("防汛", ""),
        ];
        let mut matcher = Matcher::new(nucleo_matcher::Config::DEFAULT);
        assert_eq!(rank(&entries, "防汛", &mut matcher), vec![1, 2, 0]);
        assert!(rank(&entries, "不存在的词", &mut matcher).is_empty());
        assert_eq!(rank(&entries, "  ", &mut matcher), vec![0, 1, 2]);
    }

    #[test]
    fn query_punctuation_is_literal_and_aliases_have_pinyin() {
        let entries = vec![entry("综合办公室", "办公室^通知$"), entry("财务处", "")];
        let mut matcher = Matcher::new(nucleo_matcher::Config::DEFAULT);
        assert_eq!(rank(&entries, "^通知$", &mut matcher), vec![0]);
        assert_eq!(rank(&entries, "bangongshi", &mut matcher), vec![0]);
    }
}
