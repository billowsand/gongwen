use qingjian_dictionary::SyllablePattern;

/// 一串模式扩展后的结果：每个位置若干写法，第一种是用户敲的（代价 0），其余是敲错变体
/// （按类别与个人敲错表定代价）。拥有字符串，借出 [`SyllablePattern`]。
#[derive(Debug, Clone, Default)]
pub struct Expanded {
    /// 每个位置的 (写法, 代价) 与「是否完整音节」（同一位置的写法完整性相同）。
    positions: Vec<(Vec<(String, f64)>, bool)>,
}

impl Expanded {
    pub fn new(positions: impl IntoIterator<Item = (Vec<(String, f64)>, bool)>) -> Self {
        Self {
            positions: positions.into_iter().collect(),
        }
    }

    /// 给 `index` 位置加一种写法；已有同样写法时只留代价低的那个。不完整的位置（简拼、前缀）不加。
    pub fn push_alternative(&mut self, index: usize, text: &str, cost: f64) {
        let Some((forms, complete)) = self.positions.get_mut(index) else {
            return;
        };
        if !*complete {
            return;
        }
        match forms.iter_mut().find(|(t, _)| t == text) {
            Some(existing) => existing.1 = existing.1.min(cost),
            None => forms.push((text.to_owned(), cost)),
        }
    }

    /// 词库多写法查询要的形状。
    pub fn positions(&self) -> Vec<Vec<SyllablePattern<'_>>> {
        self.positions
            .iter()
            .map(|(forms, complete)| {
                forms
                    .iter()
                    .map(|(text, _)| SyllablePattern {
                        text,
                        complete: *complete,
                    })
                    .collect()
            })
            .collect()
    }

    /// `index` 位置命中音节 `syllable` 的代价：敲的原样 0，敲错变体按写法记的代价；哪种写法都对不上（不该发生）算 0。
    pub fn cost(&self, index: usize, syllable: &str) -> f64 {
        let Some((forms, complete)) = self.positions.get(index) else {
            return 0.0;
        };
        forms
            .iter()
            .find(|(text, _)| {
                SyllablePattern {
                    text,
                    complete: *complete,
                }
                .accepts(syllable)
            })
            .map_or(0.0, |(_, cost)| *cost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn costs_follow_the_matched_form() {
        let mut expanded = Expanded::new([
            (vec![("mei".to_owned(), 0.0)], true),
            (
                vec![("gan".to_owned(), 0.0), ("gang".to_owned(), 0.7)],
                true,
            ),
            (vec![("x".to_owned(), 0.0)], false),
        ]);
        expanded.push_alternative(1, "guan", 4.5);
        expanded.push_alternative(1, "gang", 4.5); // 已有更便宜的写法，保留 0.7
        expanded.push_alternative(2, "xi", 4.0); // 不完整的位置不加
        assert_eq!(expanded.cost(1, "guan"), 4.5);
        assert_eq!(expanded.cost(1, "gang"), 0.7);
        assert_eq!(expanded.positions()[1].len(), 3);
        let plain = Expanded::new([(vec![("ni".to_owned(), 0.0)], true)]);
        assert_eq!(plain.cost(0, "ni"), 0.0);
    }
}
