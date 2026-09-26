//! 候选右上角要不要标辅码：关 / 敲了辅码时标还要敲的码 / 始终标（学码）。

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// 候选显示辅码的档位。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Hint {
    /// 不标。
    Off,

    /// 只敲了首码时，给首码对得上的候选标出第二码（`栏ˢ`）；两码敲满或没敲都不标。
    #[default]
    Typed,

    /// 在 [`Hint::Typed`] 之外，没敲辅码时也给每个候选标出完整两码（`栏ᵐˢ`），给还没记住码的人。
    Always,
}

impl Hint {
    /// 全部档位，设置界面按这个顺序列出。
    pub const ALL: [Self; 3] = [Self::Typed, Self::Always, Self::Off];

    /// 配置文件里的写法。
    pub fn key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Typed => "typed",
            Self::Always => "always",
        }
    }

    /// 界面上的名字。
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "不显示",
            Self::Typed => "敲了辅码时",
            Self::Always => "始终（学码）",
        }
    }
}

impl FromStr for Hint {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|hint| hint.key() == text.trim().to_ascii_lowercase())
            .ok_or_else(|| format!("unknown fuma hint: {text:?}"))
    }
}

impl fmt::Display for Hint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_config_keys() {
        for hint in Hint::ALL {
            assert_eq!(hint.key().parse::<Hint>().unwrap(), hint);
        }
        assert_eq!(" Always ".parse::<Hint>().unwrap(), Hint::Always);
        assert!("sometimes".parse::<Hint>().is_err());
        assert_eq!(Hint::default(), Hint::Typed);
    }
}
