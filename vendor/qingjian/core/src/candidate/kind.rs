use std::fmt;

use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

/// 候选的来源类型，平台层可据此区别显示。
///
/// 反序列化是手写的宽容版（见下方 impl）：Windows 的 IPC 协议直接传这个枚举，加新来源不能让
/// 还留在没重启的应用里的旧 DLL 整帧解析失败。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize)]
pub enum CandidateKind {
    /// 中文词库里的词。
    #[default]
    Chinese,

    /// 英文词表里的词（中英混输），上屏时吃掉整段输入。
    English,

    /// 快捷候选（日期 / 时间 / 星期），由输入直接算出，上屏时吃掉整段作用域、不记学习。
    Shortcut,

    /// 自定义短语的固定位置，从 1 开始。
    Custom(usize),

    /// 按候选词配的 emoji，音节与那个词相同；上屏按音节消耗拼音，不记学习。
    Emoji,

    /// 离线整句转换的结果（多个词拼成），带全部音节；上屏按音节消耗拼音，路径上的词逐条记入个人 n-gram，不记词频。
    Sentence,
}

/// 认不出的来源退到 [`CandidateKind::Chinese`]（照常当中文词画），不让整条消息解析失败。
/// serde 的外部标签形式下，无字段的变体是字符串 `"Emoji"`，带字段的是 `{"Custom":1}`，两种都要认。
impl<'de> Deserialize<'de> for CandidateKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(KindVisitor)
    }
}

struct KindVisitor;

impl<'de> Visitor<'de> for KindVisitor {
    type Value = CandidateKind;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a candidate kind")
    }

    fn visit_str<E>(self, name: &str) -> Result<Self::Value, E> {
        Ok(match name {
            "English" => CandidateKind::English,
            "Shortcut" => CandidateKind::Shortcut,
            "Emoji" => CandidateKind::Emoji,
            "Sentence" => CandidateKind::Sentence,
            _ => CandidateKind::Chinese,
        })
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let Some(name) = map.next_key::<String>()? else {
            return Ok(CandidateKind::Chinese);
        };
        if name == "Custom" {
            return Ok(CandidateKind::Custom(map.next_value()?));
        }
        map.next_value::<IgnoredAny>()?;
        Ok(CandidateKind::Chinese)
    }
}
