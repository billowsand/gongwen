//! 词表合并、精确查询与个人调整；不解析拼音，不生成候选。

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub code: String,
    pub text: String,
}

pub(crate) fn valid_code(code: &str, four: bool) -> bool {
    (if four {
        code.len() == 4
    } else {
        (1..=4).contains(&code.len())
    }) && code.bytes().all(|b| b.is_ascii_lowercase())
}

/// 词条可以入表：编码合法，文字非空且不含换行、制表符。
pub(crate) fn valid_entry(entry: &Entry, four: bool) -> bool {
    valid_code(&entry.code, four)
        && !entry.text.is_empty()
        && !entry.text.contains(['\t', '\r', '\n'])
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Parsed {
    pub entries: Vec<Entry>,
    pub invalid: usize,
    pub duplicates: usize,
}

/// 支持基础表的「编码,位置=文字」以及公文表的「文字 TAB 四码」。
pub(crate) fn parse(text: &str, four: bool) -> Parsed {
    let mut result = Parsed::default();
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for (line_no, line) in text.lines().enumerate() {
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("---config@") {
            continue;
        }
        let parts = if let Some((left, word)) = line.split_once('=') {
            let (code, position) = left.split_once(',').unwrap_or((left, "1"));
            position
                .trim()
                .parse::<usize>()
                .ok()
                .filter(|p| *p > 0)
                .map(|position| (code.trim(), word.trim(), position))
        } else if let Some((left, right)) = line.split_once('\t') {
            let (left, right) = (left.trim(), right.trim());
            if valid_code(right, four) {
                Some((right, left, 1))
            } else {
                Some((left, right, 1))
            }
        } else {
            None
        };
        let Some((code, word, position)) = parts else {
            result.invalid += 1;
            continue;
        };
        if !valid_code(code, four) || word.is_empty() || word.contains(['\t', '\r', '\n']) {
            result.invalid += 1;
            continue;
        }
        let entry = Entry {
            code: code.into(),
            text: word.into(),
        };
        if !seen.insert((entry.code.clone(), entry.text.clone())) {
            result.duplicates += 1;
            continue;
        }
        rows.push((position, line_no, entry));
    }
    rows.sort_by_key(|(position, line_no, _)| (*position, *line_no));
    result.entries = rows.into_iter().map(|(_, _, entry)| entry).collect();
    result
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Batch {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Personal {
    pub batches: Vec<Batch>,
    pub entries: Vec<Entry>,
    pub hidden: Vec<Entry>,
    /// 每个编码的显式候选顺序，未指定的仍按基础表、词表顺序排。
    pub order: BTreeMap<String, Vec<String>>,
    pub migrated_phrases: bool,
    /// 手动指定的二字、三字、四字及以上构词规则；不填按基础表推算。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rules: Option<[String; 3]>,
}

/// 个人词条的来源名。
pub(crate) const PERSONAL: &str = "个人词条";

/// 个人层体检：多余的个人词条、失效的屏蔽与排序。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct OverlayReport {
    /// 别的来源已经有同码同词。
    pub redundant: Vec<Entry>,
    /// 屏蔽的词条已不在任何来源里。
    pub stale_hidden: Vec<Entry>,
    /// 显式排序里有已不存在的词的编码。
    pub stale_order: Vec<String>,
}

impl OverlayReport {
    pub fn is_empty(&self) -> bool {
        self.redundant.is_empty() && self.stale_hidden.is_empty() && self.stale_order.is_empty()
    }

    pub fn describe(&self) -> String {
        format!(
            "{} 条个人词条已被其他来源收录，{} 条屏蔽记录已失效，{} 个编码的排序含已不存在的词",
            self.redundant.len(),
            self.stale_hidden.len(),
            self.stale_order.len()
        )
    }
}

/// 两版基础表的差异。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BaseDiff {
    pub added: usize,
    pub removed: usize,
    /// 同码同词但在该码里的次序变了。
    pub moved: usize,
}

/// 每条（编码, 文字）在该码中的次序。
fn ranks(entries: &[Entry]) -> HashMap<(&str, &str), usize> {
    let mut seen = HashMap::<&str, usize>::new();
    entries
        .iter()
        .map(|entry| {
            let position = seen.entry(entry.code.as_str()).or_default();
            *position += 1;
            ((entry.code.as_str(), entry.text.as_str()), *position)
        })
        .collect()
}

pub(crate) fn base_diff(old: &[Entry], new: &[Entry]) -> BaseDiff {
    let (old, new) = (ranks(old), ranks(new));
    let mut diff = BaseDiff::default();
    for (key, position) in &new {
        match old.get(key) {
            None => diff.added += 1,
            Some(before) if before != position => diff.moved += 1,
            Some(_) => {}
        }
    }
    diff.removed = old.keys().filter(|key| !new.contains_key(*key)).count();
    diff
}

#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub text: String,
    pub sources: Vec<String>,
}

#[derive(Debug, Default)]
pub(crate) struct Table {
    pub base: Vec<Entry>,
    pub document: Vec<Entry>,
    pub personal: Personal,
    index: BTreeMap<String, Vec<Candidate>>,
    /// 反查：文字 → 编码，短码在前。含已屏蔽的，查询时再过滤。
    by_text: HashMap<String, Vec<String>>,
}

impl Table {
    /// 拿给定的三层另建一张表，用来预览升级之类的改动。
    pub fn with(base: Vec<Entry>, document: Vec<Entry>, personal: Personal) -> Self {
        let mut table = Self {
            base,
            document,
            personal,
            ..Default::default()
        };
        table.rebuild();
        table
    }

    pub fn rebuild(&mut self) {
        let mut index: BTreeMap<String, Vec<Candidate>> = BTreeMap::new();
        let mut add = |entries: &[Entry], source: &str| {
            for entry in entries {
                let candidates = index.entry(entry.code.clone()).or_default();
                if let Some(found) = candidates.iter_mut().find(|c| c.text == entry.text) {
                    if !found.sources.iter().any(|s| s == source) {
                        found.sources.push(source.into());
                    }
                } else {
                    candidates.push(Candidate {
                        text: entry.text.clone(),
                        sources: vec![source.into()],
                    });
                }
            }
        };
        add(&self.base, "基础表");
        add(&self.document, "公文词表");
        for batch in &self.personal.batches {
            if batch.enabled {
                add(&batch.entries, &batch.name);
            }
        }
        add(&self.personal.entries, PERSONAL);
        for (code, candidates) in &mut index {
            if let Some(order) = self.personal.order.get(code) {
                candidates.sort_by_key(|candidate| {
                    order
                        .iter()
                        .position(|w| w == &candidate.text)
                        .unwrap_or(usize::MAX)
                });
            }
        }
        let mut by_text: HashMap<String, Vec<String>> = HashMap::new();
        for (code, candidates) in &index {
            for candidate in candidates {
                by_text
                    .entry(candidate.text.clone())
                    .or_default()
                    .push(code.clone());
            }
        }
        for codes in by_text.values_mut() {
            codes.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
        }
        self.index = index;
        self.by_text = by_text;
    }

    /// 一个词的全部有效编码：编码、在该码候选中的位置（从 1 起）与来源，短码在前。
    pub fn codes_of(&self, text: &str) -> Vec<(String, usize, Vec<String>)> {
        self.by_text
            .get(text)
            .into_iter()
            .flatten()
            .filter_map(|code| {
                let visible = self.lookup(code);
                let index = visible.iter().position(|c| c.text == text)?;
                Some((code.clone(), index + 1, visible[index].sources.clone()))
            })
            .collect()
    }

    pub fn all(&self, code: &str) -> &[Candidate] {
        self.index.get(code).map(Vec::as_slice).unwrap_or_default()
    }

    pub fn hidden(&self, code: &str, text: &str) -> bool {
        self.personal
            .hidden
            .iter()
            .any(|e| e.code == code && e.text == text)
    }

    pub fn lookup(&self, code: &str) -> Vec<Candidate> {
        self.all(code)
            .iter()
            .filter(|c| !self.hidden(code, &c.text))
            .cloned()
            .collect()
    }

    /// 以 `prefix` 开头且更长的编码的有效候选：短码在前，同码按候选顺序。
    pub fn extended(&self, prefix: &str, limit: usize) -> Vec<(String, Candidate)> {
        let mut codes: Vec<&String> = self
            .index
            .range::<str, _>((
                std::ops::Bound::Excluded(prefix),
                std::ops::Bound::Unbounded,
            ))
            .map(|(code, _)| code)
            .take_while(|code| code.starts_with(prefix))
            .collect();
        codes.sort_by_key(|code| code.len());
        codes
            .into_iter()
            .flat_map(|code| {
                self.lookup(code)
                    .into_iter()
                    .map(move |candidate| (code.clone(), candidate))
            })
            .take(limit)
            .collect()
    }

    /// 以 `prefix` 开头的全部编码及候选（含已屏蔽），按编码排序；空前缀为全部。
    pub fn codes_from<'a>(
        &'a self,
        prefix: &'a str,
    ) -> impl Iterator<Item = (&'a String, &'a Vec<Candidate>)> + 'a {
        self.index
            .range::<str, _>((
                std::ops::Bound::Included(prefix),
                std::ops::Bound::Unbounded,
            ))
            .take_while(move |(code, _)| code.starts_with(prefix))
    }

    /// 与最终词表比较，而不是仅在导入文件内部检查重码。
    pub fn preview(&self, parsed: &Parsed, replacing_base: bool) -> (usize, usize, usize) {
        let mut combined = BTreeMap::<String, BTreeSet<String>>::new();
        let mut existing = 0;
        for entry in &parsed.entries {
            if self.all(&entry.code).iter().any(|c| c.text == entry.text) {
                existing += 1;
            }
            let words = combined.entry(entry.code.clone()).or_insert_with(|| {
                self.lookup(&entry.code)
                    .into_iter()
                    .filter(|candidate| {
                        !replacing_base || candidate.sources.iter().any(|source| source != "基础表")
                    })
                    .map(|c| c.text)
                    .collect()
            });
            words.insert(entry.text.clone());
        }
        (
            parsed.entries.len() - existing,
            existing,
            combined.values().filter(|words| words.len() > 1).count(),
        )
    }

    /// 个人层里多余或失效的记录：基础表升级、导入表撤销之后常会留下。
    pub fn overlay_report(&self) -> OverlayReport {
        let present = |entry: &Entry| self.all(&entry.code).iter().any(|c| c.text == entry.text);
        OverlayReport {
            redundant: self
                .personal
                .entries
                .iter()
                .filter(|entry| {
                    self.all(&entry.code)
                        .iter()
                        .any(|c| c.text == entry.text && c.sources.iter().any(|s| s != PERSONAL))
                })
                .cloned()
                .collect(),
            stale_hidden: self
                .personal
                .hidden
                .iter()
                .filter(|entry| !present(entry))
                .cloned()
                .collect(),
            stale_order: self
                .personal
                .order
                .iter()
                .filter(|(code, words)| {
                    let all = self.all(code);
                    words.iter().any(|w| !all.iter().any(|c| &c.text == w))
                })
                .map(|(code, _)| code.clone())
                .collect(),
        }
    }

    /// 按报告清理后的个人层：删多余个人词条与失效屏蔽，排序里去掉已不存在的词。
    pub fn cleaned(&self, report: &OverlayReport) -> Personal {
        let mut personal = self.personal.clone();
        personal.entries.retain(|e| !report.redundant.contains(e));
        personal.hidden.retain(|e| !report.stale_hidden.contains(e));
        for code in &report.stale_order {
            let all = self.all(code);
            if let Some(words) = personal.order.get_mut(code) {
                words.retain(|w| all.iter().any(|c| &c.text == w));
                if words.len() <= 1 {
                    personal.order.remove(code);
                }
            }
        }
        personal
    }

    pub fn export(&self) -> String {
        let mut text = String::from("\u{feff}# 词表输入法：编码,候选位置=文字\n");
        for code in self.index.keys() {
            for (position, candidate) in self.lookup(code).iter().enumerate() {
                text.push_str(&format!("{code},{}={}\n", position + 1, candidate.text));
            }
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "本机基础词表不入库"]
    fn real_base_table_is_complete_and_exact() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fuma/quan.txt");
        let parsed = parse(&crate::text_file::read_to_string(&path).unwrap(), false);
        assert!(parsed.entries.len() >= 60_000);
        let mut table = Table {
            base: parsed.entries,
            ..Default::default()
        };
        table.rebuild();
        assert!(
            table
                .lookup("vidc")
                .iter()
                .any(|candidate| candidate.text == "指导")
        );
        assert!(!table.lookup("d").is_empty());
        let exported = parse(&table.export(), false);
        assert_eq!(exported.entries.len(), table.base.len());
    }
    #[test]
    fn formats_order_duplicates_and_validation() {
        let p = parse(
            "\u{feff}#注释\na,2=啊\na,1=阿\na,1=阿\n公文\tgswf\nBAD,1=错\nabcde,1=错\n---config@标题",
            false,
        );
        assert_eq!(p.invalid, 2);
        assert_eq!(p.duplicates, 1);
        let mut table = Table {
            base: p.entries,
            ..Default::default()
        };
        table.rebuild();
        assert_eq!(
            table
                .lookup("a")
                .iter()
                .map(|c| c.text.as_str())
                .collect::<Vec<_>>(),
            ["阿", "啊"]
        );
        assert_eq!(table.lookup("gswf")[0].text, "公文");
        assert!(table.lookup("gs").is_empty());
        assert_eq!(parse("a,1=啊\n公文\tgswf", true).invalid, 1);
    }
    #[test]
    fn overlays_conflicts_disable_and_roundtrip() {
        let mut t = Table {
            base: parse("abcd,1=基础\na,1=简码", false).entries,
            ..Default::default()
        };
        let p = parse("新增\tabcd\n基础\tabcd", true);
        t.rebuild();
        assert_eq!(t.preview(&p, false), (1, 1, 1));
        t.personal.batches.push(Batch {
            id: "1".into(),
            name: "导入表".into(),
            enabled: true,
            entries: p.entries,
        });
        t.personal.order.insert("abcd".into(), vec!["新增".into()]);
        t.rebuild();
        assert_eq!(t.lookup("abcd")[0].text, "新增");
        assert_eq!(t.lookup("abcd")[1].sources.len(), 2);
        t.personal.hidden.push(Entry {
            code: "abcd".into(),
            text: "基础".into(),
        });
        let exported = parse(&t.export(), false);
        assert!(exported.entries.iter().any(|e| e.code == "a"));
        assert!(!exported.entries.iter().any(|e| e.text == "基础"));
        t.personal.batches[0].enabled = false;
        t.rebuild();
        assert!(t.lookup("abcd").is_empty());
        t.personal.hidden.clear();
        assert_eq!(t.lookup("abcd")[0].text, "基础");
    }
    #[test]
    fn overlay_report_and_base_diff() {
        let mut table = Table {
            base: parse("abcd,1=甲\nabcd,2=乙\nefgh,1=丙", false).entries,
            ..Default::default()
        };
        table.personal.entries = parse("abcd,1=甲\nijkl,1=丁", false).entries;
        table.personal.hidden = parse("efgh,1=丙\nmnop,1=没了", false).entries;
        table
            .personal
            .order
            .insert("abcd".into(), vec!["乙".into(), "旧词".into(), "甲".into()]);
        table.rebuild();
        let report = table.overlay_report();
        assert_eq!(report.redundant, parse("abcd,1=甲", false).entries);
        assert_eq!(report.stale_hidden, parse("mnop,1=没了", false).entries);
        assert_eq!(report.stale_order, ["abcd"]);
        let cleaned = table.cleaned(&report);
        assert_eq!(cleaned.entries, parse("ijkl,1=丁", false).entries);
        assert_eq!(cleaned.order["abcd"], ["乙", "甲"]);
        let next = parse("abcd,1=乙\nabcd,2=甲\nqrst,1=新", false).entries;
        assert_eq!(
            base_diff(&table.base, &next),
            BaseDiff {
                added: 1,
                removed: 1,
                moved: 2
            }
        );
    }
    #[test]
    fn replacing_base_preview_does_not_count_retired_words() {
        let mut table = Table {
            base: parse("abcd,1=旧词", false).entries,
            ..Default::default()
        };
        table.rebuild();
        let next = parse("abcd,1=新词", false);
        assert_eq!(table.preview(&next, true), (1, 0, 0));
        table.document = parse("abcd,1=公文词", true).entries;
        table.rebuild();
        assert_eq!(table.preview(&next, true), (1, 0, 1));
    }
}
