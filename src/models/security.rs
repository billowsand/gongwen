//! 密级、保密期限规则与期限单位解析。
//!
//! 由 src/models.rs 拆分而来：本文件是模块 `models::security`，与其它子模块共享
//! `models` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use serde::{Deserialize, Serialize};

/// 公文中可以选择的密级。密级是所有文稿类型的必要填报项目，
/// 新建稿件默认“机密”、保密期限 20 年；`Unmarked` 仅用于兼容
/// 旧配置与旧稿件里没有密级的快照，新稿一律不允许留空。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SecurityLevel {
    #[default]
    Unmarked,
    Internal,
    Secret,
    Confidential,
    TopSecret,
}

impl SecurityLevel {
    pub const ALL: [Self; 5] = [
        Self::Unmarked,
        Self::Internal,
        Self::Secret,
        Self::Confidential,
        Self::TopSecret,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Unmarked => "不标注密级",
            Self::Internal => "内部",
            Self::Secret => "秘密",
            Self::Confidential => "机密",
            Self::TopSecret => "绝密",
        }
    }

    /// 写入版心的密级文字；“不标注”为空串。
    pub(crate) fn marking(self) -> &'static str {
        match self {
            Self::Unmarked => "",
            Self::Internal => "内部",
            Self::Secret => "秘密",
            Self::Confidential => "机密",
            Self::TopSecret => "绝密",
        }
    }

    pub(crate) fn from_marking(value: &str) -> Self {
        match value.trim().trim_end_matches('级') {
            "内部" | "内部资料" | "工作秘密" => Self::Internal,
            "秘密" => Self::Secret,
            "机密" => Self::Confidential,
            "绝密" => Self::TopSecret,
            _ => Self::Unmarked,
        }
    }
}

/// 密级与保密期限的约束。默认值取自《中华人民共和国保守国家秘密法》第十五条：
/// 绝密级不超过三十年、机密级不超过二十年、秘密级不超过十年。
/// 本单位另有更严格或不同口径时，可在“设置”页直接改这三个上限。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityRules {
    pub secret_max_years: u32,
    pub confidential_max_years: u32,
    pub top_secret_max_years: u32,
    /// 期限不能确定时是否允许写“长期”。
    pub allow_long_term: bool,
}

impl Default for SecurityRules {
    fn default() -> Self {
        Self {
            secret_max_years: 10,
            confidential_max_years: 20,
            top_secret_max_years: 30,
            allow_long_term: true,
        }
    }
}

/// 可供选择的保密期限阶梯，按年数升序；实际下拉项会按密级上限裁剪。
const PERIOD_LADDER_YEARS: [u32; 7] = [1, 3, 5, 10, 20, 30, 50];

pub const LONG_TERM: &str = "长期";

impl SecurityRules {
    pub(crate) fn max_years(&self, level: SecurityLevel) -> Option<u32> {
        match level {
            SecurityLevel::Secret => Some(self.secret_max_years),
            SecurityLevel::Confidential => Some(self.confidential_max_years),
            SecurityLevel::TopSecret => Some(self.top_secret_max_years),
            _ => None,
        }
    }

    pub(crate) fn period_options(&self, level: SecurityLevel) -> Vec<String> {
        match level {
            // “内部”不是国家秘密，没有法定保密期限，版心只印“内部”二字。
            SecurityLevel::Unmarked | SecurityLevel::Internal => Vec::new(),
            _ => {
                let max = self.max_years(level).unwrap_or(0);
                let mut options = vec!["6个月".to_string()];
                options.extend(
                    PERIOD_LADDER_YEARS
                        .iter()
                        .filter(|years| **years <= max)
                        .map(|years| format!("{years}年")),
                );
                if !options.iter().any(|value| value == &format!("{max}年")) && max > 0 {
                    options.push(format!("{max}年"));
                }
                if self.allow_long_term {
                    options.push(LONG_TERM.to_string());
                }
                options
            }
        }
    }

    /// 切换密级时给出的默认期限：取该密级的上限；“内部”无期限。
    pub(crate) fn default_period(&self, level: SecurityLevel) -> String {
        match level {
            SecurityLevel::Unmarked | SecurityLevel::Internal => String::new(),
            _ => self
                .max_years(level)
                .map(|years| format!("{years}年"))
                .unwrap_or_default(),
        }
    }

    /// 返回 `Some(提示)` 表示密级与保密期限不匹配。
    pub(crate) fn check(&self, level: SecurityLevel, period: &str) -> Option<String> {
        let period = period.trim();
        if level == SecurityLevel::Unmarked {
            return (!period.is_empty()).then(|| "未选择密级时不应填写保密期限".to_string());
        }
        // “内部”没有保密期限：旧稿可能还带着“长期”，提示清空即可。
        if level == SecurityLevel::Internal {
            return (!period.is_empty()).then(|| "“内部”件不标注保密期限，请清空".to_string());
        }
        if period.is_empty() {
            return Some(format!("{}级公文必须标注保密期限", level.label()));
        }
        if period == LONG_TERM {
            return if self.allow_long_term {
                None
            } else {
                Some("当前设置不允许使用“长期”作为保密期限".to_string())
            };
        }
        let Some(months) = parse_period_months(period) else {
            return Some(format!(
                "保密期限“{period}”无法识别，应写为“6个月”“10年”或“长期”"
            ));
        };
        let max = self.max_years(level)?;
        if months > max * 12 {
            return Some(format!(
                "{}级公文的保密期限不得超过{}年，当前为“{}”",
                level.label(),
                max,
                period
            ));
        }
        None
    }
}

/// 把“6个月”“10年”解析为月数；“长期”和无法识别的写法返回 `None`。
pub(crate) fn parse_period_months(value: &str) -> Option<u32> {
    let value = value.trim();
    if let Some(years) = value.strip_suffix('年') {
        return years.trim().parse::<u32>().ok().map(|years| years * 12);
    }
    if let Some(months) = value.strip_suffix("个月") {
        return months.trim().parse::<u32>().ok();
    }
    None
}

/// 把保密期限拆成“前导数字 + 其余部分”：`“10年” → (“10”, “年”)`、`“6个月” → (“6”, “个月”)`。
/// 非数字期限（如“长期”）返回 `("", 原串)`。数字部分用于按等宽字体（LaTeX `\ttfamily`、
/// Word 等宽西文字体）排版，其余按行内基准字体。
pub(crate) fn split_period_digits(value: &str) -> (&str, &str) {
    let value = value.trim();
    let digits = value.chars().take_while(|ch| ch.is_ascii_digit()).count();
    // 前导 ASCII 数字每个占 1 字节，切点正好是字符边界。
    (&value[..digits], &value[digits..])
}

/// 主送、抄送等多值字段在配置中以顿号分隔存储，界面上按列表编辑。
pub(crate) fn split_units(value: &str) -> Vec<String> {
    value
        .split(['、', '，', ',', '；', ';', '\n'])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

pub(crate) fn join_units(units: &[String]) -> String {
    units.join("、")
}
