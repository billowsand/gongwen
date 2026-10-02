//! 词组自动出码：从基础表反推每个字的编码与构词规则，不预设任何输入方案。
//!
//! 构词规则沿用 Rime 的写法：每两个字母一组，大写选字、小写选码。
//! `A`–`T` 是从前数第 1–20 个字，`U`–`Z` 从后数（`Z` 是末字）；小写同理，
//! `z` 是该字编码的最后一码。小鹤音形与小鹤双拼词组是
//! 二字 `AaAbBaBb`、三字 `AaBaCaCb`、四字及以上 `AaBaCaZa`，五笔也是这三条。
//!
//! 每个字的「字码」取基础表里该字长度够用的全部编码的前缀（规则只用到前两码时
//! 就取前两码），去重。多音字会有多个字码，出码时按「这个字码在已有词组里
//! 被用了多少次」排序：基础表本身就是最好的读音统计。

use super::table::Entry;
use std::collections::{BTreeMap, HashMap};

/// 规则中的一个选择：从前数第几个，或从后数第几个（0 是最后一个）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pick {
    Front(usize),
    Back(usize),
}

impl Pick {
    fn parse(c: char, base: char) -> Option<Self> {
        let offset = c as u32 - base as u32;
        match offset {
            0..=19 => Some(Self::Front(offset as usize)),
            20..=25 => Some(Self::Back(25 - offset as usize)),
            _ => None,
        }
    }

    fn resolve(self, len: usize) -> Option<usize> {
        match self {
            Self::Front(i) => (i < len).then_some(i),
            Self::Back(i) => len.checked_sub(i + 1),
        }
    }
}

/// 一条构词规则，如 `AaAbBaBb`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Formula {
    text: String,
    picks: Vec<(Pick, Pick)>,
}

impl Formula {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let chars: Vec<char> = text.chars().collect();
        if chars.is_empty() || !chars.len().is_multiple_of(2) {
            return None;
        }
        let picks = chars
            .chunks(2)
            .map(|pair| {
                if !pair[0].is_ascii_uppercase() || !pair[1].is_ascii_lowercase() {
                    return None;
                }
                Some((Pick::parse(pair[0], 'A')?, Pick::parse(pair[1], 'a')?))
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            text: text.into(),
            picks,
        })
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// 规则要用到字码的前几码；有从后数的码就要整个字码。
    fn prefix_len(&self) -> Option<usize> {
        self.picks
            .iter()
            .try_fold(0, |max, (_, letter)| match letter {
                Pick::Front(i) => Some(max.max(i + 1)),
                Pick::Back(_) => None,
            })
    }

    /// 人话说明：「首字前两码＋次字前两码」。
    pub fn describe(&self) -> String {
        let mut groups: Vec<(Pick, Vec<Pick>)> = Vec::new();
        for (word, letter) in &self.picks {
            match groups.last_mut() {
                Some((last, letters)) if last == word => letters.push(*letter),
                _ => groups.push((*word, vec![*letter])),
            }
        }
        groups
            .iter()
            .map(|(word, letters)| {
                let name = match word {
                    Pick::Front(0) => "首字".to_string(),
                    Pick::Front(1) => "次字".to_string(),
                    Pick::Front(i) => format!("第{}字", i + 1),
                    Pick::Back(0) => "末字".to_string(),
                    Pick::Back(i) => format!("倒数第{}字", i + 1),
                };
                let contiguous = letters
                    .iter()
                    .enumerate()
                    .all(|(index, letter)| *letter == Pick::Front(index));
                let letters = if contiguous && letters.len() == 1 {
                    "首码".to_string()
                } else if contiguous {
                    format!("前{}码", chinese_number(letters.len()))
                } else {
                    letters
                        .iter()
                        .map(|letter| match letter {
                            Pick::Front(i) => format!("第{}码", i + 1),
                            Pick::Back(0) => "末码".into(),
                            Pick::Back(i) => format!("倒数第{}码", i + 1),
                        })
                        .collect::<Vec<_>>()
                        .join("、")
                };
                format!("{name}{letters}")
            })
            .collect::<Vec<_>>()
            .join("＋")
    }
}

fn chinese_number(n: usize) -> String {
    ["零", "一", "两", "三", "四", "五", "六", "七", "八", "九"]
        .get(n)
        .map(|s| s.to_string())
        .unwrap_or_else(|| n.to_string())
}

/// 二字、三字、四字及以上三条规则。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rules(pub [Formula; 3]);

pub(crate) const RULE_LABELS: [&str; 3] = ["二字", "三字", "四字及以上"];

/// 推算时比较的候选规则：常见音形、双拼、五笔、郑码的写法都在里面。
const CANDIDATES: [&[&str]; 3] = [
    &["AaAbBaBb", "AaBaAbBb", "AaAbBa", "AaBa"],
    &["AaBaCaCb", "AaAbBaCa", "AaBaCa"],
    &["AaBaCaZa", "AaBaCaDa", "AaBaCa"],
];

impl Default for Rules {
    fn default() -> Self {
        Self(CANDIDATES.map(|options| Formula::parse(options[0]).expect("内置规则合法")))
    }
}

impl Rules {
    pub fn parse(texts: &[String; 3]) -> Option<Self> {
        Some(Self([
            Formula::parse(&texts[0])?,
            Formula::parse(&texts[1])?,
            Formula::parse(&texts[2])?,
        ]))
    }

    pub fn texts(&self) -> [String; 3] {
        self.0.clone().map(|formula| formula.text)
    }

    fn for_len(&self, len: usize) -> Option<&Formula> {
        match len {
            0 | 1 => None,
            2 => Some(&self.0[0]),
            3 => Some(&self.0[1]),
            _ => Some(&self.0[2]),
        }
    }

    fn prefix_len(&self) -> Option<usize> {
        self.0
            .iter()
            .try_fold(1, |max, formula| Some(max.max(formula.prefix_len()?)))
    }
}

/// 一条规则在基础表词组上的命中情况。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RuleStat {
    pub checked: usize,
    pub matched: usize,
}

/// 一个建议码与它的来由。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Suggestion {
    pub code: String,
    pub score: f64,
    /// 每个参与取码的字及选用的字码。
    pub parts: Vec<(char, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EncodeError {
    /// 单字没有构词规则，要自己填码。
    Single,
    /// 这些字在基础表里没有足够长的编码。
    Missing(Vec<char>),
    Empty,
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Single => f.write_str("单字没有构词规则，请手填编码"),
            Self::Missing(chars) => write!(
                f,
                "「{}」在基础表里没有字码，请手填编码",
                chars.iter().collect::<String>()
            ),
            Self::Empty => f.write_str("文字为空"),
        }
    }
}

/// 字码与读音统计，随基础表与规则一起重建。
#[derive(Debug, Default)]
pub(crate) struct Encoder {
    pub inferred: Rules,
    pub rules: Rules,
    pub overridden: bool,
    /// 当前规则在基础表词组上的命中，按二字、三字、四字及以上。
    pub stats: [RuleStat; 3],
    /// 每个字的字码（按基础表首次出现的顺序）与它在词组里被用到的次数。
    chars: HashMap<char, Vec<(String, f64)>>,
}

/// 组合数上限：多音字连在一起时不至于爆炸。
const MAX_COMBINATIONS: usize = 4096;

impl Encoder {
    pub fn build(base: &[Entry], custom: Option<&Rules>) -> Self {
        // 只拿全码词组推算与统计：表里的三简、二简词组（如三字词只取三码）不按构词规则编。
        let full = base.iter().map(|entry| entry.code.len()).max().unwrap_or(0);
        let words: Vec<(&str, Vec<char>)> = base
            .iter()
            .filter(|entry| entry.code.len() == full)
            .filter_map(|entry| {
                let chars: Vec<char> = entry.text.chars().collect();
                (chars.len() >= 2).then_some((entry.code.as_str(), chars))
            })
            .collect();
        // 候选规则都只用到前两码，按前两码先推算规则。
        let two = char_codes(base, Some(2));
        let inferred = Rules(std::array::from_fn(|class| {
            let best = CANDIDATES[class]
                .iter()
                .map(|text| Formula::parse(text).expect("内置规则合法"))
                .map(|formula| {
                    let stat = rule_stat(&formula, class, &words, &two);
                    (stat.matched, formula)
                })
                .fold(
                    None::<(usize, Formula)>,
                    |best, (matched, formula)| match best {
                        Some((top, _)) if top >= matched => best,
                        _ => Some((matched, formula)),
                    },
                );
            best.expect("候选规则非空").1
        }));
        let rules = custom.cloned().unwrap_or_else(|| inferred.clone());
        let prefix = rules.prefix_len();
        let codes = if prefix == Some(2) {
            two
        } else {
            char_codes(base, prefix)
        };
        let stats = std::array::from_fn(|class| rule_stat(&rules.0[class], class, &words, &codes));
        let mut chars: HashMap<char, Vec<(String, f64)>> = codes
            .into_iter()
            .map(|(c, codes)| (c, codes.into_iter().map(|code| (code, 0.0)).collect()))
            .collect();
        // 读音统计：只在一个字贡献了它字码的全部码位时才算一票，
        // 只取一码的位置分不清是哪个读音。
        if let Some(prefix) = prefix {
            for (code, word) in &words {
                let Some(formula) = rules.for_len(word.len()) else {
                    continue;
                };
                if formula.picks.len() != code.len() {
                    continue;
                }
                let mut taken: BTreeMap<usize, Vec<Option<u8>>> = BTreeMap::new();
                for (position, (word_pick, letter_pick)) in formula.picks.iter().enumerate() {
                    let (Some(index), Pick::Front(letter)) =
                        (word_pick.resolve(word.len()), letter_pick)
                    else {
                        continue;
                    };
                    let slots = taken.entry(index).or_insert_with(|| vec![None; prefix]);
                    slots[*letter] = Some(code.as_bytes()[position]);
                }
                for (index, slots) in taken {
                    let Some(bytes) = slots.into_iter().collect::<Option<Vec<u8>>>() else {
                        continue;
                    };
                    let used = String::from_utf8(bytes).unwrap_or_default();
                    if let Some(options) = chars.get_mut(&word[index])
                        && let Some(option) = options.iter_mut().find(|(code, _)| *code == used)
                    {
                        option.1 += 1.0;
                    }
                }
            }
        }
        Self {
            inferred,
            overridden: custom.is_some(),
            rules,
            stats,
            chars,
        }
    }

    /// 一个字的全部字码，按在词组里用到的次数从多到少。
    pub fn char_codes(&self, c: char) -> Vec<String> {
        let mut options = self.chars.get(&c).cloned().unwrap_or_default();
        options.sort_by(|a, b| b.1.total_cmp(&a.1));
        options.into_iter().map(|(code, _)| code).collect()
    }

    pub fn formula_for(&self, text: &str) -> Option<&Formula> {
        self.rules.for_len(text.chars().count())
    }

    /// 按规则给词组出码，最可能的排在前面。
    pub fn encode(&self, text: &str) -> Result<Vec<Suggestion>, EncodeError> {
        let word: Vec<char> = text.chars().collect();
        if word.is_empty() {
            return Err(EncodeError::Empty);
        }
        let formula = self.rules.for_len(word.len()).ok_or(EncodeError::Single)?;
        let mut used: Vec<usize> = formula
            .picks
            .iter()
            .filter_map(|(pick, _)| pick.resolve(word.len()))
            .collect();
        used.sort_unstable();
        used.dedup();
        let missing: Vec<char> = used
            .iter()
            .map(|index| word[*index])
            .filter(|c| self.chars.get(c).is_none_or(Vec::is_empty))
            .collect();
        if !missing.is_empty() {
            let mut unique = Vec::new();
            for c in missing {
                if !unique.contains(&c) {
                    unique.push(c);
                }
            }
            return Err(EncodeError::Missing(unique));
        }
        let options: Vec<&Vec<(String, f64)>> = used
            .iter()
            .map(|index| &self.chars[&word[*index]])
            .collect();
        let mut merged: Vec<Suggestion> = Vec::new();
        let mut choice = vec![0usize; used.len()];
        for _ in 0..MAX_COMBINATIONS {
            let mut code = String::new();
            for (word_pick, letter_pick) in &formula.picks {
                let index = word_pick.resolve(word.len()).expect("上面已解析");
                let slot = used.binary_search(&index).expect("上面已收集");
                let char_code = &options[slot][choice[slot]].0;
                if let Some(letter) = letter_pick
                    .resolve(char_code.len())
                    .and_then(|i| char_code.as_bytes().get(i))
                {
                    code.push(*letter as char);
                }
            }
            if code.len() == formula.picks.len() {
                let score: f64 = choice
                    .iter()
                    .enumerate()
                    .map(|(slot, choice)| options[slot][*choice].1 + 0.5)
                    .product();
                match merged.iter_mut().find(|s| s.code == code) {
                    Some(found) => found.score += score,
                    None => merged.push(Suggestion {
                        code,
                        score,
                        parts: used
                            .iter()
                            .enumerate()
                            .map(|(slot, index)| {
                                (word[*index], options[slot][choice[slot]].0.clone())
                            })
                            .collect(),
                    }),
                }
            }
            // 逐位进位，走完全部组合。
            let mut slot = 0;
            loop {
                if slot == choice.len() {
                    merged.sort_by(|a, b| b.score.total_cmp(&a.score));
                    return Ok(merged);
                }
                choice[slot] += 1;
                if choice[slot] < options[slot].len() {
                    break;
                }
                choice[slot] = 0;
                slot += 1;
            }
        }
        merged.sort_by(|a, b| b.score.total_cmp(&a.score));
        Ok(merged)
    }

    /// 编码是否符合规则推得的某个可能码；单字或推不出码时返回 None。
    pub fn conforms(&self, text: &str, code: &str) -> Option<bool> {
        let suggestions = self.encode(text).ok()?;
        Some(suggestions.iter().any(|s| s.code == code))
    }
}

/// 从基础表单字条目收集字码：取前 `prefix` 码，编码不够长的不算；None 取整码。
fn char_codes(base: &[Entry], prefix: Option<usize>) -> HashMap<char, Vec<String>> {
    let mut codes: HashMap<char, Vec<String>> = HashMap::new();
    for entry in base {
        let mut chars = entry.text.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            continue;
        };
        let code = match prefix {
            Some(len) if entry.code.len() >= len => &entry.code[..len],
            Some(_) => continue,
            None => entry.code.as_str(),
        };
        let list = codes.entry(c).or_default();
        if !list.iter().any(|known| known == code) {
            list.push(code.into());
        }
    }
    codes
}

/// 规则在某一词长类上的命中数。每个字的字码选择相互独立，逐字检查即可，不必枚举组合。
fn rule_stat(
    formula: &Formula,
    class: usize,
    words: &[(&str, Vec<char>)],
    codes: &HashMap<char, Vec<String>>,
) -> RuleStat {
    let mut stat = RuleStat::default();
    for (code, word) in words {
        let class_of = (word.len().min(4)) - 2;
        if class_of != class {
            continue;
        }
        stat.checked += 1;
        if code.len() != formula.picks.len() {
            continue;
        }
        let mut needs: BTreeMap<usize, Vec<(Pick, u8)>> = BTreeMap::new();
        let mut resolvable = true;
        for (position, (word_pick, letter_pick)) in formula.picks.iter().enumerate() {
            match word_pick.resolve(word.len()) {
                Some(index) => needs
                    .entry(index)
                    .or_default()
                    .push((*letter_pick, code.as_bytes()[position])),
                None => resolvable = false,
            }
        }
        let matched = resolvable
            && needs.iter().all(|(index, needs)| {
                codes.get(&word[*index]).is_some_and(|options| {
                    options.iter().any(|option| {
                        needs.iter().all(|(pick, byte)| {
                            pick.resolve(option.len())
                                .and_then(|i| option.as_bytes().get(i))
                                == Some(byte)
                        })
                    })
                })
            });
        if matched {
            stat.matched += 1;
        }
    }
    stat
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ime::table::parse;

    fn base() -> Vec<Entry> {
        parse(
            "vi,1=只\nvifo,1=指\ndcs,1=导\ndcsc,1=导\nzs,1=中\nvs,1=中\nhgii,1=行\nxkii,1=行\n\
             vs,1=中\nhr,1=华\nrf,1=人\nmb,1=民\ngs,1=共\nhe,1=和\ngoky,1=国\nyb,1=银\n\
             vidc,1=指导\nvshr,1=中华\nxkvs,1=行中\nhgyb,1=行银\nhgrf,1=行人\nhgmb,1=行民\n\
             vhrg,1=中华人民共和国\nvhrf,1=中华人\nzzzz,1=错码词",
            false,
        )
        .entries
    }

    #[test]
    fn parses_and_describes_rime_formulas() {
        let formula = Formula::parse("AaAbBaBb").unwrap();
        assert_eq!(formula.describe(), "首字前两码＋次字前两码");
        assert_eq!(
            Formula::parse("AaBaCaZa").unwrap().describe(),
            "首字首码＋次字首码＋第3字首码＋末字首码"
        );
        assert_eq!(
            Formula::parse("AaZz").unwrap().describe(),
            "首字首码＋末字末码"
        );
        assert!(Formula::parse("Aa1b").is_none());
        assert!(Formula::parse("AaA").is_none());
    }

    #[test]
    fn infers_rules_and_encodes_by_usage() {
        let encoder = Encoder::build(&base(), None);
        assert_eq!(
            encoder.inferred.texts(),
            ["AaAbBaBb", "AaBaCaCb", "AaBaCaZa"]
        );
        assert_eq!(encoder.stats[0].checked, 6);
        assert_eq!(encoder.stats[0].matched, 6);
        assert_eq!(encoder.encode("指导").unwrap()[0].code, "vidc");
        // 「行」在已有词组里 hg 用了三次、xk 一次，首选 hg。
        let codes: Vec<String> = encoder
            .encode("行华")
            .unwrap()
            .into_iter()
            .map(|s| s.code)
            .collect();
        assert_eq!(codes, ["hghr", "xkhr"]);
        // 「中」有 zs、vs 两个字码，vs 在词组里用过。
        assert_eq!(encoder.encode("中银").unwrap()[0].code, "vsyb");
        assert_eq!(encoder.encode("中华人民").unwrap()[0].code, "vhrm");
        assert_eq!(encoder.encode("银").unwrap_err(), EncodeError::Single);
        assert_eq!(
            encoder.encode("指鹤鹤").unwrap_err(),
            EncodeError::Missing(vec!['鹤'])
        );
        assert_eq!(encoder.conforms("指导", "vidc"), Some(true));
        assert_eq!(encoder.conforms("指导", "vidd"), Some(false));
        assert_eq!(encoder.char_codes('行'), ["hg", "xk"]);
    }

    #[test]
    fn custom_rules_override_inference() {
        let custom = Rules::parse(&["AaBa".into(), "AaBaCa".into(), "AaBaCa".into()]).unwrap();
        let encoder = Encoder::build(&base(), Some(&custom));
        assert!(encoder.overridden);
        assert_eq!(encoder.encode("指导").unwrap()[0].code, "vd");
        assert_eq!(encoder.stats[0].matched, 0);
    }

    #[test]
    #[ignore = "本机基础词表不入库"]
    fn real_base_table_top_choice_accuracy() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fuma/quan.txt");
        let base = parse(&crate::text_file::read_to_string(&path).unwrap(), false).entries;
        let encoder = Encoder::build(&base, None);
        assert_eq!(
            encoder.inferred.texts(),
            ["AaAbBaBb", "AaBaCaCb", "AaBaCaZa"]
        );
        let (mut total, mut top) = (0, 0);
        for entry in base
            .iter()
            .filter(|e| e.code.len() == 4 && e.text.chars().count() >= 2)
        {
            if let Ok(suggestions) = encoder.encode(&entry.text) {
                total += 1;
                top += usize::from(suggestions[0].code == entry.code);
            }
        }
        assert!(top * 100 >= total * 95, "首选命中 {top}/{total}");
    }
}
