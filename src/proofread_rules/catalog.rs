//! 文档规则目录与可选风格提示；管理界面和执行器共享同一份定义。

use crate::models::{ProofreadConfig, TemplateKind};

pub(crate) struct RuleInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub scope: &'static str,
    pub condition: &'static str,
    pub basis: &'static str,
    pub optional: bool,
}

impl RuleInfo {
    pub(crate) fn level_label(&self) -> &'static str {
        match self.id {
            "RULE-CITATION-NUMBER"
            | "RULE-TITLE-PUNCT"
            | "RULE-TITLE-KIND"
            | "RULE-KIND-REQUEST" => "必错",
            "RULE-TITLE-LONG" | "EXPORT-DOC-DATE" => "提示",
            _ if self.optional => "提示",
            _ => "疑似",
        }
    }
}

macro_rules! rule {
    ($id:literal, $name:literal, $scope:literal, $condition:literal, $basis:literal, $optional:literal) => {
        RuleInfo {
            id: $id,
            name: $name,
            scope: $scope,
            condition: $condition,
            basis: $basis,
            optional: $optional,
        }
    };
}

pub(crate) const RULES: &[RuleInfo] = &[
    rule!(
        "RULE-CITATION-NUMBER",
        "公文引用文号",
        "各文类中的可识别公文引用",
        "文号括号或序号写法不规范；不改引用名称",
        "公文格式及本程序引用格式",
        false
    ),
    rule!(
        "RULE-TITLE-PUNCT",
        "主标题标点",
        "公函、电话通知；有明确文种的普通公文及呈批件",
        "标题有额外标点；书名号、引号内豁免。研究、记录、会议名称不查",
        "正式公文标题写法",
        false
    ),
    rule!(
        "RULE-TITLE-KIND",
        "标题文种一致",
        "公函、电话通知",
        "标题自称的文种与模板明确绑定的文种冲突",
        "所选模板的文种约束",
        false
    ),
    rule!(
        "RULE-TITLE-LONG",
        "标题长度提醒",
        "同主标题标点范围",
        "超过36字仅提示考虑断行；不是法定字数上限",
        "本程序版式经验值",
        false
    ),
    rule!(
        "RULE-ONE-MATTER",
        "请求性结语重复",
        "标题明确为请示的正文",
        "出现多处独立请求结语；不据此断言一文多事",
        "结语重复检查；一文一事需另行审核",
        false
    ),
    rule!(
        "RULE-KIND-REQUEST",
        "非请示稿请求结语",
        "标题明确为报告、通知、通报、公告、通告、纪要、决定的正文",
        "独立结语行出现审批请求；函、记录、研究、附件及直接引文排除",
        "公文处理条例第十五条",
        false
    ),
    rule!(
        "RULE-NUM-YEAR",
        "年份写法",
        "自写正文",
        "四位汉字年份；引用文件名称、直接引文、附件不查",
        "数字用法的核对提示",
        false
    ),
    rule!(
        "RULE-SEQ",
        "手写序号连续性",
        "各文类的手写序号",
        "同级跳号或重号；新标题、区段重新计数，小数不算序号",
        "本程序序号一致性检查",
        false
    ),
    rule!(
        "RULE-ATTACH-COUNT",
        "内嵌附件数量核对",
        "正文声明附件项目数或完整编号清单",
        "与内嵌项目数不一致时给疑似；印制份数不比，另行随附需人工核对",
        "附件说明与编辑区内容一致性",
        false
    ),
    rule!(
        "RULE-ATTACH-REF",
        "附件引用核对",
        "正文明确序号引用",
        "引用序号超过内嵌项目数时给疑似；金额和数量不认序号",
        "内嵌附件引用一致性",
        false
    ),
    rule!(
        "RULE-HONOR-ATTACHED",
        "敬称与机构名",
        "机关间自写正文",
        "疑似敬称接完整机构名；贵阳、贵港、贵溪、贵定地名和跨短语排除",
        "称谓用法核对提示",
        false
    ),
    rule!(
        "RULE-HONOR-MIXED",
        "贵／你称谓核对",
        "自写正文",
        "相同机关后缀出现两种称谓；只提示核对对象，不断言指代同一机关",
        "本稿称谓一致性",
        false
    ),
    rule!(
        "RULE-SENTENCE-END",
        "段末标点",
        "公文正文自然段",
        "长段落结尾缺标点时给疑似；附件、引文、研究、记录、议程排除",
        "段末标点核对提示",
        false
    ),
    rule!(
        "RULE-PUNCT-DASH",
        "破折号使用偏好",
        "按当前模板选用的自写正文",
        "超过1处提示通读；不宣称违反标点规范",
        "可选写作偏好，无通用数量上限",
        true
    ),
    rule!(
        "RULE-FORCE-QUOTA",
        "强制词使用偏好",
        "按当前模板选用的自写正文",
        "必须／严禁超过5处提示核对；不是制度、通知的强制上限",
        "可选写作偏好",
        true
    ),
    rule!(
        "RULE-WORD-JINYIBU",
        "进一步使用偏好",
        "按当前模板选用的自写正文",
        "超过3处提示核对冗余；需考虑篇幅",
        "可选写作偏好",
        true
    ),
    rule!(
        "RULE-OPEN-CLICHE",
        "开篇表态偏好",
        "按当前模板选用的自写正文",
        "开篇本机关高度重视时提示是否有具体措施；不判事实陈述错误",
        "可选写作偏好",
        true
    ),
    rule!(
        "RULE-TONE-PARALLEL",
        "函的直接要求语气",
        "公函正文中的直接要求",
        "独立句起以命令词指向贵／你单位时给疑似；转述排除，职权需人工判断",
        "行文关系与职权范围核对",
        false
    ),
    rule!(
        "RULE-TONE-UPWARD",
        "上行稿直接要求语气",
        "标题明确为请示、报告的正文",
        "独立句起以命令词指向上级时给疑似；第三方叙述排除",
        "上行文语气核对",
        false
    ),
    rule!(
        "EXPORT-DOC-DATE",
        "自动成文日期核对",
        "正式公函、白头件、红头呈批件导出时",
        "仅自动日期早于今天时提醒；手定日期、预览版不报，不称过期",
        "成文日期应与通过／签发日期相符",
        false
    ),
];

pub(crate) fn style_enabled(config: &ProofreadConfig, kind: TemplateKind, id: &str) -> bool {
    RULES.iter().any(|rule| rule.id == id && rule.optional)
        && config
            .document_styles
            .get(&format!("{kind:?}"))
            .is_some_and(|ids| ids.iter().any(|item| item == id))
}

pub(crate) fn set_style(config: &mut ProofreadConfig, kind: TemplateKind, id: &str, enabled: bool) {
    if !RULES.iter().any(|rule| rule.id == id && rule.optional) {
        return;
    }
    let key = format!("{kind:?}");
    let ids = config.document_styles.entry(key.clone()).or_default();
    ids.retain(|item| item != id);
    if enabled {
        ids.push(id.to_owned());
    }
    if ids.is_empty() {
        config.document_styles.remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_configuration_does_not_enable_styles_and_mandatory_rules_cannot_be_disabled() {
        let mut config: ProofreadConfig = serde_json::from_str("{}").unwrap();
        assert!(!style_enabled(
            &config,
            TemplateKind::OfficialLetter,
            "RULE-PUNCT-DASH"
        ));
        set_style(
            &mut config,
            TemplateKind::OfficialLetter,
            "RULE-PUNCT-DASH",
            true,
        );
        assert!(style_enabled(
            &config,
            TemplateKind::OfficialLetter,
            "RULE-PUNCT-DASH"
        ));
        assert!(!style_enabled(
            &config,
            TemplateKind::ResearchReport,
            "RULE-PUNCT-DASH"
        ));
        set_style(
            &mut config,
            TemplateKind::OfficialLetter,
            "RULE-TITLE-KIND",
            true,
        );
        assert!(!style_enabled(
            &config,
            TemplateKind::OfficialLetter,
            "RULE-TITLE-KIND"
        ));
        let roundtrip: ProofreadConfig =
            serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
        assert_eq!(config, roundtrip);
        set_style(
            &mut config,
            TemplateKind::OfficialLetter,
            "RULE-PUNCT-DASH",
            false,
        );
        assert!(config.document_styles.is_empty());
    }

    #[test]
    fn catalog_covers_all_emitted_rule_ids() {
        let source = include_str!("../proofread_rules.rs").replace("\r\n", "\n");
        let source = source.split("\n#[cfg(test)]\nmod tests").next().unwrap();
        let mut emitted = Vec::new();
        for id in source.split('"').filter(|part| part.starts_with("RULE-")) {
            assert!(RULES.iter().any(|rule| rule.id == id), "目录漏了 {id}");
            emitted.push(id);
        }
        emitted.sort_unstable();
        emitted.dedup();
        assert_eq!(
            emitted.len(),
            RULES.len() - 1,
            "每条正文规则都应在目录中，导出日期单列"
        );
        let mut ids: Vec<_> = RULES.iter().map(|rule| rule.id).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count);
    }
}
