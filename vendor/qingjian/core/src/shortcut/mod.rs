//! 快捷候选：不查词库、由输入本身直接算出来的候选。
//!
//! - `rq` / `sj` / `xq`：今天的日期、现在的时间、星期几，插在本地候选第二位起。

mod calendar;

use jiff::Zoned;

use crate::candidate::{Candidate, CandidateKind};

pub use calendar::{date_forms, time_forms, weekday_forms};

/// 按输入算快捷候选；不是快捷输入时为空。`now` 由调用方给，测试可固定时间。
pub fn candidates(input: &str, now: &Zoned) -> Vec<Candidate> {
    let texts: Vec<String> = match input {
        "rq" => date_forms(now),
        "sj" => time_forms(now),
        "xq" => weekday_forms(now),
        _ => Vec::new(),
    };
    texts
        .into_iter()
        .map(|text| Candidate {
            text,
            kind: CandidateKind::Shortcut,
            syllables: Vec::new(),
            reading: None,
            translation: None,
            fuma: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;
    use jiff::tz::TimeZone;

    use super::*;

    fn now() -> Zoned {
        date(2026, 9, 3)
            .at(19, 6, 23, 0)
            .to_zoned(TimeZone::UTC)
            .unwrap()
    }

    fn texts(input: &str) -> Vec<String> {
        candidates(input, &now())
            .into_iter()
            .map(|c| c.text)
            .collect()
    }

    #[test]
    fn calendar_shortcuts() {
        assert_eq!(texts("rq"), ["2026年9月3日", "2026-09-03", "2026/09/03"]);
        assert_eq!(texts("sj"), ["19:06", "19:06:23", "19点06分"]);
        assert_eq!(texts("xq"), ["星期四", "周四"]);
        assert!(texts("rqi").is_empty());
        assert!(texts("nihao").is_empty());
    }
}
