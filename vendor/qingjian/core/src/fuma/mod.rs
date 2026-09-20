//! 辅码（双拼辅助码）：字级形码表与候选的严格过滤。
//!
//! 表只存单字两码，每行一条 `字=两码`（两码均为小写，见 `assets/fuma/xiaohe.txt`）；
//! 词组不存表，查询时按「单字取本字两码、多字取首字第 1 码 + 末字第 1 码」现算，
//! 语义与水杉引擎（MSIME-Engine 的 `HelpcodeUtils`）一致。
//! 汉字（含扩展区）之外的字符一律跳过，首末取的是汉字；
//! 首末字查不到表即视为不匹配（严格过滤，不出候选）。

mod table;

pub use table::FumaTable;

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// 辅码方案。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    /// 小鹤形码。
    Xiaohe,
}

impl Scheme {
    /// 全部方案，设置界面按这个顺序列出。
    pub const ALL: [Self; 1] = [Self::Xiaohe];

    /// 配置文件里的写法。
    pub fn key(self) -> &'static str {
        match self {
            Self::Xiaohe => "xiaohe",
        }
    }

    /// 界面上的名字。
    pub fn label(self) -> &'static str {
        match self {
            Self::Xiaohe => "小鹤辅码",
        }
    }

    /// 随包码表的相对路径（`assets/` 下）。
    pub fn asset(self) -> &'static str {
        match self {
            Self::Xiaohe => "fuma/xiaohe.txt",
        }
    }
}

impl FromStr for Scheme {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|scheme| scheme.key() == text.trim().to_ascii_lowercase())
            .ok_or_else(|| format!("unknown fuma scheme: {text:?}"))
    }
}

impl fmt::Display for Scheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_config_keys() {
        for scheme in Scheme::ALL {
            assert_eq!(scheme.key().parse::<Scheme>().unwrap(), scheme);
        }
        assert_eq!(" Xiaohe ".parse::<Scheme>().unwrap(), Scheme::Xiaohe);
        assert!("zrm".parse::<Scheme>().is_err());
        assert_eq!(Scheme::Xiaohe.to_string(), "xiaohe");
    }
}
