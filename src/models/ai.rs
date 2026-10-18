//! AI 优化提示词与校对规则的配置结构。
//!
//! 由 src/models.rs 拆分而来：本文件是模块 `models::ai`，与其它子模块共享
//! `models` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::models::TemplateKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 一条可复用的 AI 优化提示词。它只描述“这次要模型做什么”，输出形态一律由
/// `prompt::output_contract` 内置约束，用户既不能改也不能关掉，
/// 因此自定义提示词再怎么写都不会破坏导出所依赖的 Markdown 结构。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AiPrompt {
    pub id: u32,
    pub name: String,
    /// 交给模型的任务描述。留空表示只按内置标准做格式规整。
    pub instruction: String,
    /// 适用文种；留空表示所有文种通用。
    pub kinds: Vec<TemplateKind>,
    /// 预置项的稳定标识。非空表示这是内置项：可改、可恢复默认，但不可删除。
    pub builtin_key: String,
}

impl AiPrompt {
    /// 该提示词是否适用于指定文种。未限定文种的条目对所有文种可见。
    pub(crate) fn applies_to(&self, kind: TemplateKind) -> bool {
        self.kinds.is_empty() || self.kinds.contains(&kind)
    }

    pub(crate) fn is_builtin(&self) -> bool {
        !self.builtin_key.is_empty()
    }

    /// 适用文种的中文摘要，用于列表和选择面板。
    pub(crate) fn kinds_label(&self) -> String {
        if self.kinds.is_empty() {
            "全部文种".to_string()
        } else {
            self.kinds
                .iter()
                .map(|kind| kind.label())
                .collect::<Vec<_>>()
                .join("、")
        }
    }
}

/// 预置提示词。`格式规整` 是保底项：指令留空，等价于旧版“只规整格式”的行为。
/// 其余三条允许改写措辞和篇幅，但都不得凭空造事实——这条底线同时写在
/// 各自指令和内置输出标准里。
pub(crate) fn builtin_ai_prompts() -> Vec<AiPrompt> {
    vec![
        AiPrompt {
            id: 0,
            name: "格式规整".into(),
            instruction: String::new(),
            kinds: vec![],
            builtin_key: "format".into(),
        },
        AiPrompt {
            id: 0,
            name: "公文化润色".into(),
            instruction: "在不改变任何事实、数据、结论和责任主体的前提下，把口语化、\
网络化、汇报体的表述改写为规范的公文语体：多用短句和动宾结构，去掉“我觉得”“大概”\
“差不多”“进一步加强”一类含糊或空转的说法，保持庄重、平实、准确。不得新增事实、\
数据、结论或政策依据。"
                .into(),
            kinds: vec![],
            builtin_key: "polish".into(),
        },
        AiPrompt {
            id: 0,
            name: "精简篇幅".into(),
            instruction: "在完整保留全部关键事实、数据、时限、责任主体和办理要求的前提下压缩篇幅：\
删去重复表述、套话铺垫和与主旨无关的枝节，合并同类内容，长句拆短。\
宁可少说，也不得为了通顺而改动或编造事实。"
                .into(),
            kinds: vec![],
            builtin_key: "condense".into(),
        },
        AiPrompt {
            id: 0,
            name: "充实论述".into(),
            instruction: "对论述单薄的段落做展开：补充逻辑衔接、层次划分和必要的说明性表述，\
使各部分详略均衡。只能在现有稿件已有的事实基础上展开；\
缺依据的地方原位写“【待核实：缺什么】”，绝不虚构数据、案例、文件名称、\
政策条款或领导讲话。"
                .into(),
            kinds: vec![
                TemplateKind::OfficialLetter,
                TemplateKind::PlainDocument,
                TemplateKind::WhitePaper,
                TemplateKind::RedHeadApproval,
            ],
            builtin_key: "expand".into(),
        },
    ]
}

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
/// 整条存快照的做法（如 `AiPrompt`）在这里不适用：词表要逐版打磨，快照会把
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
