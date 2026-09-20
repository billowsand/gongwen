use std::collections::HashMap;
use std::path::Path;

/// 字 → 两码。第一码是字的第一个部件、第二码是第二个部件（各按其读音的声母取键）。
/// 词组的期望辅码不存表：单字取本字两码，多字取首字第 1 码 + 末字第 1 码，查询时现算。
#[derive(Debug, Default, Clone)]
pub struct FumaTable {
    /// 字 → [第一码, 第二码]。
    entries: HashMap<char, [char; 2]>,
}

impl FumaTable {
    /// 解析每行 `字=两码` 的表：空行与 `#` 开头跳过；同一字重复出现时后一条覆盖前一条
    /// （与上游水杉加载器一致）；格式不对的行返回行号。
    pub fn parse(source: &str) -> Result<Self, usize> {
        let mut entries: HashMap<char, [char; 2]> = HashMap::new();
        for (index, raw) in source.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fail = || index + 1;
            let (word, code) = line.split_once('=').ok_or_else(fail)?;
            let mut word = word.chars();
            let (Some(word), None) = (word.next(), word.next()) else {
                return Err(fail());
            };
            let mut code = code.chars();
            let (Some(first), Some(second), None) = (code.next(), code.next(), code.next()) else {
                return Err(fail());
            };
            if !first.is_ascii_lowercase() || !second.is_ascii_lowercase() {
                return Err(fail());
            }
            entries.insert(word, [first, second]);
        }
        Ok(Self { entries })
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        let source = std::fs::read_to_string(path)?;
        Self::parse(&source).map_err(|line| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("fuma table line {line}: malformed entry"),
            )
        })
    }

    /// 字的两码；不在表里为 `None`。
    pub fn codes(&self, word: char) -> Option<[char; 2]> {
        self.entries.get(&word).copied()
    }

    /// 一个候选词的期望辅码：只有一个汉字时取它完整两码，多个汉字取首字第 1 码 + 末字第 1 码
    /// （词长无所谓，二字词与整句同规则，同水杉一致）；词里没有汉字、或首末字不在表里为 `None`。
    pub fn expected_codes(&self, word: &str) -> Option<[char; 2]> {
        let mut han = word.chars().filter(|c| is_han(*c));
        let first = han.next()?;
        let mut last = first;
        let mut count = 1;
        for c in han {
            last = c;
            count += 1;
        }
        if count == 1 {
            return self.entries.get(&first).copied();
        }
        Some([self.entries.get(&first)?[0], self.entries.get(&last)?[0]])
    }

    /// 候选是否匹配敲出的两码（严格过滤，`typed` 是解析键时已小写化、处理过反转顺序的两码）。
    pub fn matches(&self, word: &str, typed: [char; 2]) -> bool {
        self.expected_codes(word) == Some(typed)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// 汉字范围：〇、扩展 A 区、基本区、兼容区、扩展 B–F 区（与水杉的 `is_han_code_point` 一致）。
fn is_han(c: char) -> bool {
    let code = c as u32;
    code == 0x3007
        || (0x3400..=0x4DBF).contains(&code)
        || (0x4E00..=0x9FFF).contains(&code)
        || (0xF900..=0xFAFF).contains(&code)
        || (0x20000..=0x2FA1F).contains(&code)
        || (0x30000..=0x323AF).contains(&code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lines_and_later_entry_wins() {
        let table = FumaTable::parse("# 注释\n阿=ek\n啊=kk\n阿=xz\n\n").unwrap();
        assert_eq!(table.codes('阿'), Some(['x', 'z']));
        assert_eq!(table.codes('啊'), Some(['k', 'k']));
        assert_eq!(table.codes('好'), None);
        assert_eq!(table.len(), 2);
    }

    #[test]
    fn rejects_malformed_lines() {
        assert_eq!(FumaTable::parse("阿ek\n").unwrap_err(), 1);
        assert_eq!(FumaTable::parse("阿=EK\n").unwrap_err(), 1);
        assert_eq!(FumaTable::parse("阿=e\n").unwrap_err(), 1);
        assert_eq!(FumaTable::parse("阿=ekk\n").unwrap_err(), 1);
        assert_eq!(FumaTable::parse("阿姨=ek\n").unwrap_err(), 1);
    }

    #[test]
    fn expected_codes_follow_word_length() {
        let table = FumaTable::parse("阿=ek\n姨=yn\n好=nz\n盘=xm\n人=rr\n").unwrap();
        // 单字：本字两码
        assert_eq!(table.expected_codes("阿"), Some(['e', 'k']));
        assert_eq!(table.expected_codes("好"), Some(['n', 'z']));
        // 多字词：首字第 1 码 + 末字第 1 码，与词长无关
        assert_eq!(table.expected_codes("阿姨"), Some(['e', 'y']));
        assert_eq!(table.expected_codes("阿人姨"), Some(['e', 'y']));
        // 同字重复的词也算多字，不落到单字分支
        assert_eq!(table.expected_codes("人人"), Some(['r', 'r']));
        // 汉字以外的字符跳过；只剩一个汉字时取它完整两码
        assert_eq!(table.expected_codes("C盘"), Some(['x', 'm']));
        assert_eq!(table.expected_codes("😀阿"), Some(['e', 'k']));
        // 没有汉字、或首末字不在表里
        assert_eq!(table.expected_codes("rust"), None);
        assert_eq!(table.expected_codes("阿妩"), None);
    }

    #[test]
    fn matching_is_strict() {
        let table = FumaTable::parse("阿=ek\n姨=yn\n").unwrap();
        assert!(table.matches("阿", ['e', 'k']));
        assert!(table.matches("阿姨", ['e', 'y']));
        // 两码对不上、字不在表里都算不匹配
        assert!(!table.matches("阿", ['e', 'x']));
        assert!(!table.matches("阿姨", ['e', 'k']));
        assert!(!table.matches("妩", ['e', 'k']));
    }

    #[test]
    fn recognizes_han_ranges() {
        assert!(is_han('〇'));
        assert!(is_han('中'));
        assert!(is_han('\u{3400}'));
        assert!(is_han('\u{20000}'));
        assert!(!is_han('a'));
        assert!(!is_han('，'));
        assert!(!is_han('😀'));
    }
}
