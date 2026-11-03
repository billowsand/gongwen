//! AI 优化提示词与校对规则的配置结构。
//!
//! 由 src/models.rs 拆分而来：本文件是模块 `models::ai`，与其它子模块共享
//! `models` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 校对词表里的一条完整规则。字段与内置种子表 `proofread-lexicon.tsv` 的列
/// 一一对应；这里一律存字符串，正则的编译交给 `proofread` 模块。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProofreadRule {
    /// 条目编号。内置条目形如 `TYP-001`，用户自建的用 `USR-` 前缀。
    pub id: String,
    pub wrong: String,
    pub suggestion: String,
    /// 必错 / 疑似 / 提示。
    pub level: String,
    /// 总是 / 后接:… / 不后接:… / 前接:… / 不前接:… / 正则:…
    pub condition: String,
    pub group: String,
    pub note: String,
    pub enabled: bool,
}

/// 对某条内置规则的改动。
///
/// 只存用户真正改过的字段，没改的留 `None`——这样内置词表在后续版本里修订了
/// 措辞、收紧了命中条件，老用户也能跟着更新，只有他自己动过的地方才保持不变。
/// 整条存快照的做法在这里不适用：词表要逐版打磨，快照会把
/// 用户永远钉死在安装当时的那一版上。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProofreadOverride {
    /// 挂靠用的条目编号，对应内置表的第一列。
    pub id: String,
    pub enabled: Option<bool>,
    pub level: Option<String>,
    pub suggestion: Option<String>,
    pub condition: Option<String>,
    pub note: Option<String>,
}

impl ProofreadOverride {
    /// 没有任何改动的覆盖记录没有保存价值，回落默认时直接删掉。
    pub(crate) fn is_empty(&self) -> bool {
        self.enabled.is_none()
            && self.level.is_none()
            && self.suggestion.is_none()
            && self.condition.is_none()
            && self.note.is_none()
    }
}

/// 校对词表的用户层。内置种子表编译在二进制里，用户的一切改动落在这里。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProofreadConfig {
    /// 对内置条目的改动，按条目编号挂靠。
    pub overrides: Vec<ProofreadOverride>,
    /// 用户自建条目，完整保存——它们没有种子可回落。
    pub custom: Vec<ProofreadRule>,
    /// 按模板明确选用的文档风格提示。旧配置默认不启用，不影响强制规则。
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub document_styles: BTreeMap<String, Vec<String>>,
    /// 永久忽略的修订建议，键为「来源编号 + \u{1} + 原文」。
    ///
    /// 没有忽略机制，误报一次用户就会把整个校对关掉——那比没有这条规则更糟
    /// （`proofread_rules` 开头的取舍原则同此）。分两档：本篇忽略随稿件走，
    /// 存在 `RevisionSet` 里；这里是「以后都别再提」的那一档。
    pub ignored: Vec<String>,
}

impl ProofreadConfig {
    pub(crate) fn override_for(&self, id: &str) -> Option<&ProofreadOverride> {
        self.overrides.iter().find(|item| item.id == id)
    }

    /// 取出（必要时新建）某条内置规则的覆盖记录，供界面就地改写。
    pub(crate) fn override_mut(&mut self, id: &str) -> &mut ProofreadOverride {
        if let Some(index) = self.overrides.iter().position(|item| item.id == id) {
            return &mut self.overrides[index];
        }
        self.overrides.push(ProofreadOverride {
            id: id.to_string(),
            ..Default::default()
        });
        self.overrides.last_mut().expect("刚推入的覆盖记录必然存在")
    }

    /// 把某条内置规则恢复默认：删掉它的覆盖记录。
    pub(crate) fn clear_override(&mut self, id: &str) {
        self.overrides.retain(|item| item.id != id);
    }

    /// 丢掉不再产生任何改动的空覆盖记录，避免配置文件越攒越大。
    pub(crate) fn prune(&mut self) {
        self.overrides.retain(|item| !item.is_empty());
    }

    /// 生成下一个可用的自建条目编号。
    pub(crate) fn next_custom_id(&self) -> String {
        let max = self
            .custom
            .iter()
            .filter_map(|rule| rule.id.strip_prefix("USR-"))
            .filter_map(|number| number.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        format!("USR-{:03}", max + 1)
    }
}
