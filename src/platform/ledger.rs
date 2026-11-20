//! 登记公共类型：登记不是一张大表。
//!
//! 发文台账、外部文件/制度各自建表，共享 `DocNumber` 和池键。池键是
//! “单位 + 方案 + 年度”，不是代字：同一个单位的纪要只有一条序列，
//! 带字/不带字只是显示不同，不能拆成两个池。

use super::doc::{DocNumber, NumberingScheme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Issued,
    Received,
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LedgerStatus {
    Used,
    Reserved,
    Void,
    Active,
    Abolished,
}

/// 台账池：单位 + 编号方案 + 年度。代字不进池键。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pool {
    pub unit: String,
    pub scheme: NumberingScheme,
    pub year: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LedgerEntry {
    pub number: DocNumber,
    pub pool: Pool,
    pub status: LedgerStatus,
    /// 手工补录的纸质件可以为空。
    pub manuscript_id: Option<i64>,
    pub note: String,
}

impl LedgerEntry {
    pub(crate) fn pool_key(&self) -> (&str, NumberingScheme, &str) {
        (
            self.pool.unit.trim(),
            self.pool.scheme,
            self.pool.year.trim(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(code: Option<&str>) -> LedgerEntry {
        LedgerEntry {
            number: DocNumber {
                scheme: NumberingScheme::Minutes,
                code: code.map(str::to_string),
                year: "2026".into(),
                serial: "3".into(),
            },
            pool: Pool {
                unit: "某局".into(),
                scheme: NumberingScheme::Minutes,
                year: "2026".into(),
            },
            status: LedgerStatus::Used,
            manuscript_id: None,
            note: String::new(),
        }
    }

    #[test]
    fn minutes_with_and_without_code_share_the_same_pool() {
        let with_code = entry(Some("某纪"));
        let without_code = entry(None);
        assert_eq!(with_code.number.display(), "某纪〔2026〕3号");
        assert_eq!(without_code.number.display(), "〔2026〕3号");
        assert_eq!(with_code.pool_key(), without_code.pool_key());
    }
}
