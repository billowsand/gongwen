use serde::{Deserialize, Serialize};

use crate::candidate::CandidateKind;

/// 上屏的文字从哪来。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputSource {
    /// 词库里的词（含用户词）。
    Word,

    /// 本地整句转换。
    Sentence,

    /// 英文候选。
    English,

    /// 快捷候选（日期 / 时间 / 星期）。
    Shortcut,

    /// 用户配置的自定义短语。
    Custom,

    /// emoji。
    Emoji,

    /// 回车原样上屏敲的字母。
    Raw,
}

impl From<CandidateKind> for InputSource {
    fn from(kind: CandidateKind) -> Self {
        match kind {
            CandidateKind::Chinese => Self::Word,
            CandidateKind::Sentence => Self::Sentence,
            CandidateKind::English => Self::English,
            CandidateKind::Shortcut => Self::Shortcut,
            CandidateKind::Custom(_) => Self::Custom,
            CandidateKind::Emoji => Self::Emoji,
        }
    }
}
