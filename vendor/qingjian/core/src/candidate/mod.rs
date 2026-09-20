//! 候选词数据模型。
//!
//! 翻译是候选词的 annotation：可选、单语言、最多 [`Translation::MAX_SENSES`] 条释义。
//! 不要把它扩展成多语言并列的结构，那会破坏「一次只学一种语言」的产品原则。

mod furigana;
mod kind;
mod language;
mod layout;
mod list;
mod part_of_speech;
mod sense;
mod translation;

use serde::{Deserialize, Serialize};

pub use furigana::{FuriganaSegment, furigana};
pub use kind::CandidateKind;
pub use language::{Language, UnknownLanguage};
pub use layout::{CandidateLayout, Cell};
pub use list::CandidateList;
pub use part_of_speech::{PartOfSpeech, UnknownPartOfSpeech};
pub use sense::Sense;
pub use translation::Translation;

/// 一个可上屏的候选。
///
/// Windows 的 IPC 协议直接传这个类型（`qingjian_platform::protocol::Frame` 里的候选列表），
/// 所以整个结构 `#[serde(default)]`：升级安装后老 DLL 还留在没重启的应用里，加字段、删字段、
/// 改名都不能让它整帧解析失败。改字段前先读 `protocol::PROTOCOL_VERSION` 的说明。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Candidate {
    /// 上屏文本。
    pub text: String,

    /// 来源类型。
    pub kind: CandidateKind,

    /// 该候选对应的拼音音节，供平台层高亮已匹配部分。
    pub syllables: Vec<String>,

    /// 读音（如日语假名），中文候选暂不使用。
    pub reading: Option<String>,

    /// 学习语言下的译文；查不到或尚未就绪时为 `None`。
    pub translation: Option<Translation>,

    /// 这个候选的辅码（`栏` → `ms`），只在敲了辅码时填：候选窗标出来，用户才知道下次该敲什么。
    /// 表里查不到首末字时为 `None`。
    pub fuma: Option<String>,
}
