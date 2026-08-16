//! 文字校对层：按词表逐条扫正文，报错别字、近义混淆、语病、套话和称谓问题。
//!
//! 与 `validator` 分工明确：`validator` 管**要素**（必填、互斥、结构），这里管
//! **文字**。分开的理由有三条——数据来源不同（这里依赖可编辑的词表）、触发时机
//! 不同（可以边打字边跑）、误报容忍度不同（词表规则天然会有误报，必须能按条
//! 忽略、按组关闭，而要素校验不该被关掉）。
//!
//! 所有提示都带 `span`，指向正文里出问题的那几个字，审校面板据此做成可点的。
//! 这是校对能不能真用起来的分界线：只说"发现非规范名称"而不给位置，人还得自己
//! 回正文里找。
//!
//! 内置词表是**种子**，不是运行时可改的数据。发行版里它编译在二进制中，机关
//! 内部用户碰不到文件，所以用户的停用、改写和自定义条目一律走界面，落在配置
//! 目录的覆盖层里，按 `id` 挂靠。这里只负责"给我一张词表，我来扫"。

use crate::models::{ProofreadConfig, ProofreadOverride, ProofreadRule, ReviewNote};
use regex::Regex;

/// 内置种子词表。只在首次初始化和「恢复默认」时读，运行时生效的是它与用户
/// 覆盖层合并后的结果。
const BUILTIN_LEXICON: &str = include_str!("../proofread-lexicon.tsv");

/// 提示级别。决定审校抽屉里的分档、颜色，以及能不能一键替换。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// 确定的错误，可一键替换。
    MustFix,
    /// 需要人判断的近义混淆，只提示，绝不自动改。
    Suspect,
    /// 弱提示（套话、冗余），整组可关。
    Hint,
}

impl Level {
    /// 审校抽屉的分档标题。
    pub fn label(self) -> &'static str {
        match self {
            Self::MustFix => "必错",
            Self::Suspect => "疑似",
            Self::Hint => "提示",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "必错" => Some(Self::MustFix),
            "疑似" => Some(Self::Suspect),
            "提示" => Some(Self::Hint),
            _ => None,
        }
    }
}

/// 命中条件。
///
/// 刻意没有把前后文交给正则：Rust 的 `regex` 不支持环视，`截止(?=目前)` 这种
/// 写法根本编译不过。改成由程序检查相邻字符，用户在界面上填「后接：目前|今天」
/// 即可，不必碰正则；确实需要复杂句式时才落到 `Regex`。
#[derive(Debug, Clone)]
pub enum Condition {
    /// 出现即命中。
    Always,
    /// 紧随其后是列表中任意一项才命中。
    FollowedBy(Vec<String>),
    /// 紧随其后不是列表中任意一项才命中。
    NotFollowedBy(Vec<String>),
    /// 紧邻其前是列表中任意一项才命中。
    PrecededBy(Vec<String>),
    /// 紧邻其前不是列表中任意一项才命中。
    NotPrecededBy(Vec<String>),
    /// 整句型问题（缺主语、倍数误用等）只能靠正则描述。
    Pattern(Regex),
}

impl Condition {
    /// 解析界面/词表里的命中条件写法。正则编译失败时返回 `Err`，由调用方决定
    /// 是整条丢弃（内置表）还是当场红字提示（用户编辑）。
    pub fn parse(value: &str) -> Result<Self, String> {
        let value = value.trim();
        if value == "总是" {
            return Ok(Self::Always);
        }
        let (head, rest) = value
            .split_once(':')
            .or_else(|| value.split_once('：'))
            .ok_or_else(|| format!("无法识别的命中条件「{value}」"))?;
        let list = || -> Vec<String> {
            rest.split('|')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect()
        };
        match head.trim() {
            "后接" => Ok(Self::FollowedBy(list())),
            "不后接" => Ok(Self::NotFollowedBy(list())),
            "前接" => Ok(Self::PrecededBy(list())),
            "不前接" => Ok(Self::NotPrecededBy(list())),
            "正则" => Regex::new(rest)
                .map(Self::Pattern)
                .map_err(|error| format!("正则有误：{error}")),
            other => Err(format!("无法识别的命中条件「{other}」")),
        }
    }
}

/// 词表里的一条规则。
#[derive(Debug, Clone)]
pub struct Entry {
    /// 稳定标识，用户覆盖层按它挂靠——不能用「错误写法」当键，那东西可改。
    pub id: String,
    pub wrong: String,
    pub suggestion: String,
    pub level: Level,
    pub condition: Condition,
    /// 命中条件的原始文本。编译成 `Condition` 后原文就取不回来了，而编辑器、
    /// 导出和「有没有被改过」的比对都要它，所以原样留一份。
    pub condition_text: String,
    pub group: String,
    pub note: String,
    pub enabled: bool,
}

impl Entry {
    /// 由配置里的纯字符串规则编译成可执行条目。级别或正则有问题时返回错误，
    /// 由调用方决定是丢弃该条还是提示用户。
    pub fn from_rule(rule: &ProofreadRule) -> Result<Self, String> {
        let level =
            Level::parse(&rule.level).ok_or_else(|| format!("级别「{}」无法识别", rule.level))?;
        let condition = Condition::parse(&rule.condition)?;
        Ok(Self {
            id: rule.id.trim().to_string(),
            wrong: rule.wrong.trim().to_string(),
            suggestion: rule.suggestion.trim().to_string(),
            level,
            condition,
            condition_text: rule.condition.trim().to_string(),
            group: rule.group.trim().to_string(),
            note: rule.note.trim().to_string(),
            enabled: rule.enabled,
        })
    }

    /// 反过来落回配置形态，供界面编辑与导出。
    pub fn to_rule(&self) -> ProofreadRule {
        ProofreadRule {
            id: self.id.clone(),
            wrong: self.wrong.clone(),
            suggestion: self.suggestion.clone(),
            level: self.level.label().to_string(),
            condition: self.condition_text.clone(),
            group: self.group.clone(),
            note: self.note.clone(),
            enabled: self.enabled,
        }
    }

    /// 内置条目的编号有固定前缀，自建的一律 `USR-`。界面据此决定能不能删。
    pub fn is_builtin(&self) -> bool {
        !self.id.starts_with("USR-")
    }

    /// 这条的建议写法能不能直接替换进正文。
    ///
    /// 只有「必错 + 非正则 + 建议写法是个纯词」才给替换按钮。语病类的建议写法
    /// 是「删去「通过」或「使」」这类操作说明，正则类命中的是一整句，两者拿去
    /// 替换都只会把正文改坏，所以宁可只提示。
    pub fn is_replaceable(&self) -> bool {
        self.level == Level::MustFix
            && !matches!(self.condition, Condition::Pattern(_))
            && !self.suggestion.trim().is_empty()
            && !self.suggestion.contains(['「', '」', '／', '（', '(', '…'])
    }
}

/// 一条校对提示。比 `ReviewNote` 多带级别、来源条目和可选的替换文本——审校
/// 面板要用它们分档、做「替换」按钮和「忽略本条」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofNote {
    /// 来源条目的 id，忽略记录按它存。
    pub entry_id: String,
    /// 来源分组。既让用户一眼看出是哪类规则命中的，也是将来分组折叠的键。
    pub group: String,
    pub level: Level,
    pub message: String,
    /// 正文中的字节范围。
    pub span: std::ops::Range<usize>,
    /// 可直接写回正文的替换文本；为 `None` 时界面不显示替换按钮。
    pub replacement: Option<String>,
}

impl From<ProofNote> for ReviewNote {
    /// 降级成通用审校提示。分档界面做出来之前，校对提示要和要素提示挤在同一个
    /// 抽屉里，所以把级别写进正文，人才分得清哪条必须改、哪条只是提请注意。
    fn from(note: ProofNote) -> Self {
        ReviewNote::located(
            format!("［{}·{}］{}", note.level.label(), note.group, note.message),
            note.span,
        )
    }
}

/// 进程内共享的内置种子表：解析、编译正则只做一次。
///
/// 这是**未经用户改动**的原始词表，只有两个用途：合并覆盖层的底子，以及界面上
/// 「恢复默认」时的参照。实际扫正文用的是 [`Lexicon::resolved`] 的结果。
pub fn builtin_lexicon() -> &'static Lexicon {
    static CACHE: std::sync::OnceLock<Lexicon> = std::sync::OnceLock::new();
    CACHE.get_or_init(Lexicon::builtin)
}

/// 内置种子表里某一条的原始定义，供界面显示「默认值」和判断有没有被改过。
pub fn builtin_entry(id: &str) -> Option<&'static Entry> {
    builtin_lexicon()
        .entries
        .iter()
        .find(|entry| entry.id == id)
}

/// 内置种子表里某一条的原始命中条件文本，供编辑器判断有没有被改过。
pub fn builtin_condition_text(id: &str) -> Option<String> {
    builtin_entry(id).map(|entry| entry.condition_text.clone())
}

/// 一张可用的词表。
#[derive(Debug, Clone, Default)]
pub struct Lexicon {
    pub entries: Vec<Entry>,
    /// 解析期问题（列数不对、级别不认识、正则编译失败）。内置表出问题属于
    /// 打包事故，用户导入出问题则要原样显示给用户看。
    pub load_warnings: Vec<String>,
}

impl Lexicon {
    /// 加载内置种子表。
    pub fn builtin() -> Self {
        Self::parse(BUILTIN_LEXICON)
    }

    /// 合成实际生效的词表：内置种子 → 套用用户覆盖 → 追加自建条目。
    ///
    /// 升级语义都落在这个函数里：
    /// - 新版种子表新增的条目会自然出现，因为底子每次都从种子重建；
    /// - 种子表修订过的措辞也能跟着更新，因为覆盖层只按字段生效；
    /// - 种子表删掉的条目，如果用户改过它，就降级成自建条目保住用户的编辑，
    ///   不能因为上游删了一行就把人家的改动一起吞掉。
    pub fn resolved(config: &ProofreadConfig) -> Self {
        let seed = builtin_lexicon();
        let mut entries = Vec::with_capacity(seed.entries.len() + config.custom.len());
        let mut load_warnings = seed.load_warnings.clone();

        for entry in &seed.entries {
            let mut entry = entry.clone();
            if let Some(patch) = config.override_for(&entry.id) {
                apply_override(&mut entry, patch, &mut load_warnings);
            }
            entries.push(entry);
        }

        // 上游已删、但用户改过的条目：转成自建条目留下。
        for patch in &config.overrides {
            if seed.entries.iter().any(|entry| entry.id == patch.id) {
                continue;
            }
            if patch.is_empty() {
                continue;
            }
            load_warnings.push(format!(
                "条目 {} 已不在内置词表中，你对它的改动被保留为自定义条目",
                patch.id
            ));
        }

        for rule in &config.custom {
            match Entry::from_rule(rule) {
                Ok(entry) => entries.push(entry),
                Err(error) => {
                    load_warnings.push(format!("自定义条目 {}：{error}", rule.id));
                }
            }
        }

        Self {
            entries,
            load_warnings,
        }
    }

    /// 解析 TSV。`#` 开头的行与空行跳过，首个非注释行是表头。
    pub fn parse(text: &str) -> Self {
        let mut entries = Vec::new();
        let mut load_warnings = Vec::new();
        let mut header_seen = false;
        for (index, raw) in text.lines().enumerate() {
            let line = raw.trim_end_matches('\r');
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let cols: Vec<&str> = line.split('\t').collect();
            if !header_seen {
                header_seen = true;
                // 表头本身不是数据；靠首列名字确认格式没跑偏。
                if cols.first().map(|c| c.trim()) == Some("条目编号") {
                    continue;
                }
                load_warnings.push(format!("第 {} 行不是预期的表头", index + 1));
                continue;
            }
            if cols.len() < 8 {
                load_warnings.push(format!(
                    "第 {} 行只有 {} 列，需要 8 列，已跳过",
                    index + 1,
                    cols.len()
                ));
                continue;
            }
            let Some(level) = Level::parse(cols[3]) else {
                load_warnings.push(format!("第 {} 行级别「{}」无法识别", index + 1, cols[3]));
                continue;
            };
            let condition = match Condition::parse(cols[4]) {
                Ok(condition) => condition,
                Err(error) => {
                    load_warnings.push(format!("第 {} 行{error}", index + 1));
                    continue;
                }
            };
            entries.push(Entry {
                id: cols[0].trim().to_string(),
                wrong: cols[1].trim().to_string(),
                suggestion: cols[2].trim().to_string(),
                level,
                condition,
                condition_text: cols[4].trim().to_string(),
                group: cols[5].trim().to_string(),
                note: cols[6].trim().to_string(),
                enabled: cols[7].trim() == "是",
            });
        }
        Self {
            entries,
            load_warnings,
        }
    }

    /// 扫一遍正文。只跑启用的条目，结果按出现位置排序。
    pub fn check(&self, text: &str) -> Vec<ProofNote> {
        let mut notes = Vec::new();
        for entry in self.entries.iter().filter(|entry| entry.enabled) {
            match &entry.condition {
                Condition::Pattern(pattern) => {
                    for found in pattern.find_iter(text) {
                        notes.push(ProofNote {
                            entry_id: entry.id.clone(),
                            group: entry.group.clone(),
                            level: entry.level,
                            message: format!(
                                "「{}」{}",
                                found.as_str(),
                                describe(&entry.suggestion, &entry.note)
                            ),
                            span: found.range(),
                            replacement: None,
                        });
                    }
                }
                condition => {
                    if entry.wrong.is_empty() {
                        continue;
                    }
                    for (start, matched) in text.match_indices(entry.wrong.as_str()) {
                        let end = start + matched.len();
                        if !context_allows(condition, text, start, end) {
                            continue;
                        }
                        notes.push(ProofNote {
                            entry_id: entry.id.clone(),
                            group: entry.group.clone(),
                            level: entry.level,
                            message: format!(
                                "「{}」{}",
                                entry.wrong,
                                describe(&entry.suggestion, &entry.note)
                            ),
                            span: start..end,
                            replacement: entry.is_replaceable().then(|| entry.suggestion.clone()),
                        });
                    }
                }
            }
        }
        // 先按位置、同位置再按级别，让"必错"排在"提示"前面。
        notes.sort_by(|a, b| {
            a.span
                .start
                .cmp(&b.span.start)
                .then(a.level.cmp(&b.level))
                .then(a.entry_id.cmp(&b.entry_id))
        });
        notes
    }
}

/// 把一条用户改动套到种子条目上。改坏的字段（级别写错、正则编译不过）不静默
/// 吞掉，而是保留原值并记一条告警——否则用户改错了规则却以为在生效。
fn apply_override(entry: &mut Entry, patch: &ProofreadOverride, warnings: &mut Vec<String>) {
    if let Some(enabled) = patch.enabled {
        entry.enabled = enabled;
    }
    if let Some(level) = &patch.level {
        match Level::parse(level) {
            Some(level) => entry.level = level,
            None => warnings.push(format!(
                "条目 {} 的级别「{level}」无法识别，已用默认值",
                entry.id
            )),
        }
    }
    if let Some(suggestion) = &patch.suggestion {
        entry.suggestion = suggestion.clone();
    }
    if let Some(note) = &patch.note {
        entry.note = note.clone();
    }
    if let Some(condition) = &patch.condition {
        match Condition::parse(condition) {
            Ok(parsed) => {
                entry.condition = parsed;
                entry.condition_text = condition.clone();
            }
            Err(error) => warnings.push(format!(
                "条目 {} 的命中条件有误（{error}），已用默认值",
                entry.id
            )),
        }
    }
}

/// 拼提示正文：建议写法 + 可选的说明。
fn describe(suggestion: &str, note: &str) -> String {
    let suggestion = suggestion.trim();
    let note = note.trim();
    let head = if suggestion.is_empty() {
        "用法有误".to_string()
    } else {
        format!("建议改为「{suggestion}」")
    };
    if note.is_empty() {
        head
    } else {
        format!("{head}：{note}")
    }
}

/// 判断命中处的前后文是否满足条件。`Pattern` 不走这里。
fn context_allows(condition: &Condition, text: &str, start: usize, end: usize) -> bool {
    let after = &text[end..];
    let before = &text[..start];
    match condition {
        Condition::Always => true,
        Condition::FollowedBy(list) => list.iter().any(|item| after.starts_with(item.as_str())),
        Condition::NotFollowedBy(list) => !list.iter().any(|item| after.starts_with(item.as_str())),
        Condition::PrecededBy(list) => list.iter().any(|item| before.ends_with(item.as_str())),
        Condition::NotPrecededBy(list) => !list.iter().any(|item| before.ends_with(item.as_str())),
        // 已在调用处分流，这里不该到达；返回 false 比 panic 安全。
        Condition::Pattern(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lexicon(rows: &str) -> Lexicon {
        let mut text =
            String::from("条目编号\t错误写法\t建议写法\t级别\t命中条件\t分组\t说明\t启用\n");
        text.push_str(rows);
        Lexicon::parse(&text)
    }

    #[test]
    fn builtin_lexicon_parses_without_warnings() {
        let lexicon = Lexicon::builtin();
        assert!(
            lexicon.load_warnings.is_empty(),
            "内置词表应当零解析告警：{:?}",
            lexicon.load_warnings
        );
        assert!(lexicon.entries.len() > 100, "内置词表条数异常");
    }

    #[test]
    fn builtin_ids_are_unique() {
        // 覆盖层按 id 挂靠，重复 id 会让用户的改动串到别的条目上。
        let lexicon = Lexicon::builtin();
        let mut ids: Vec<&str> = lexicon.entries.iter().map(|e| e.id.as_str()).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "内置词表存在重复条目编号");
    }

    #[test]
    fn cliche_group_ships_disabled() {
        // 套话档误报率天然高，默认开着会逼用户关掉整个校对。
        let lexicon = Lexicon::builtin();
        assert!(
            lexicon
                .entries
                .iter()
                .filter(|entry| entry.group == "套话虚词")
                .all(|entry| !entry.enabled),
            "套话虚词整组应默认关闭"
        );
    }

    #[test]
    fn always_condition_flags_every_occurrence_with_a_span() {
        let lexicon = lexicon("TYP-001\t布署\t部署\t必错\t总是\t错别字\t无此词\t是\n");
        let text = "已完成布署，下一步继续布署。";
        let notes = lexicon.check(text);
        assert_eq!(notes.len(), 2);
        assert_eq!(&text[notes[0].span.clone()], "布署");
        assert_eq!(&text[notes[1].span.clone()], "布署");
        assert_eq!(notes[0].replacement.as_deref(), Some("部署"));
        assert!(notes[0].message.contains("建议改为「部署」"));
    }

    #[test]
    fn followed_by_narrows_the_match() {
        // 「截止目前」要报，「报名截止」不能报——这是词表最典型的误报来源。
        let lexicon =
            lexicon("SYN-014\t截止\t截至\t疑似\t正则:截止(目前|今天|本月|[0-9])\t近义混淆\t\t是\n");
        assert_eq!(lexicon.check("截止目前已完成").len(), 1);
        assert_eq!(lexicon.check("截止8月底").len(), 1);
        assert!(lexicon.check("报名截止，不再受理。").is_empty());
    }

    #[test]
    fn not_followed_by_skips_the_legitimate_use() {
        let lexicon = lexicon("SYN-008\t检察\t检查\t疑似\t不后接:院|官|机关\t近义混淆\t\t是\n");
        assert!(lexicon.check("由检察院依法办理").is_empty());
        assert_eq!(lexicon.check("开展安全检察工作").len(), 1);
    }

    #[test]
    fn preceded_by_only_fires_after_the_listed_words() {
        let lexicon = lexicon("GRM-015\t莅临\t改用「到」\t疑似\t前接:我|我单位\t语病\t敬语\t是\n");
        assert_eq!(lexicon.check("欢迎我单位莅临指导").len(), 1);
        assert!(lexicon.check("欢迎上级领导莅临指导").is_empty());
    }

    #[test]
    fn pattern_reports_the_whole_matched_sentence() {
        let lexicon = lexicon(
            "GRM-001\t通过使\t删去「通过」或「使」\t必错\t正则:通过[^，。；]{2,40}，\\s*使\t语病\t缺主语\t是\n",
        );
        let text = "通过这次专项检查，使我们发现了问题。";
        let notes = lexicon.check(text);
        assert_eq!(notes.len(), 1);
        assert!(text[notes[0].span.clone()].starts_with("通过"));
        // 整句型问题不能给替换按钮，替换只会把正文改坏。
        assert!(notes[0].replacement.is_none());
    }

    #[test]
    fn disabled_entries_are_skipped() {
        let lexicon = lexicon("CLI-001\t高度重视\t写明谁重视\t提示\t总是\t套话虚词\t\t否\n");
        assert!(lexicon.check("各单位高度重视此项工作").is_empty());
    }

    #[test]
    fn instruction_style_suggestions_are_not_replaceable() {
        let plain = lexicon("GRM-003\t涉及到\t涉及\t必错\t总是\t语病\t\t是\n");
        assert_eq!(
            plain.check("涉及到多个部门")[0].replacement.as_deref(),
            Some("涉及")
        );
        let instruction = lexicon("GRM-012\t悬殊很大\t悬殊／差距很大\t必错\t总是\t语病\t\t是\n");
        assert!(
            instruction.check("两地差距悬殊很大")[0]
                .replacement
                .is_none()
        );
    }

    #[test]
    fn notes_come_back_in_document_order() {
        let lexicon = lexicon(
            "TYP-002\t撤消\t撤销\t必错\t总是\t错别字\t\t是\nTYP-001\t布署\t部署\t必错\t总是\t错别字\t\t是\n",
        );
        let notes = lexicon.check("先撤消原件，再重新布署。");
        assert_eq!(notes.len(), 2);
        assert!(notes[0].span.start < notes[1].span.start);
        assert_eq!(notes[0].entry_id, "TYP-002");
    }

    #[test]
    fn a_bad_regex_is_reported_and_the_row_dropped() {
        let lexicon = lexicon("BAD-001\tx\ty\t必错\t正则:(\t语病\t\t是\n");
        assert!(lexicon.entries.is_empty());
        assert_eq!(lexicon.load_warnings.len(), 1);
        assert!(lexicon.load_warnings[0].contains("正则有误"));
    }

    #[test]
    fn lookaround_is_rejected_by_the_regex_engine() {
        // 提醒后来者：环视写了也不生效，会在加载期就被挡下。
        let lexicon = lexicon("BAD-002\tx\ty\t必错\t正则:截止(?=目前)\t语病\t\t是\n");
        assert!(lexicon.entries.is_empty());
        assert!(!lexicon.load_warnings.is_empty());
    }

    #[test]
    fn short_rows_are_skipped_with_a_warning() {
        let lexicon = lexicon("TYP-009\t布署\t部署\n");
        assert!(lexicon.entries.is_empty());
        assert!(lexicon.load_warnings[0].contains("需要 8 列"));
    }
}

#[cfg(test)]
mod sample_tests {
    use super::*;

    /// 拿仓库里的示例公文当回归样本：这是一份写得规范的稿子，内置词表不该在
    /// 它身上报出「必错」。将来有人往词表里加了过宽的规则，这条会先炸。
    #[test]
    fn the_sample_letter_trips_no_must_fix_rule() {
        let sample = include_str!("../examples/标准带附件函件测试样例.md");
        let notes = builtin_lexicon().check(sample);
        let must_fix: Vec<&ProofNote> = notes
            .iter()
            .filter(|note| note.level == Level::MustFix)
            .collect();
        assert!(
            must_fix.is_empty(),
            "示例公文被判必错：{:?}",
            must_fix
                .iter()
                .map(|note| (&note.entry_id, &sample[note.span.clone()]))
                .collect::<Vec<_>>()
        );
    }
}

#[cfg(test)]
mod dirty_sample_tests {
    use super::*;

    /// 反向样本：一段刻意写坏的公文，几类规则都应当各自命中一次。
    /// 与「示例公文零必错」那条配对——一条防误报，一条防漏报。
    #[test]
    fn a_flawed_paragraph_is_caught_across_rule_families() {
        let text = "通过这次专项检查，使我们发现问题。各单位要按上级布署抓好落实，\
截止目前已完成整改，涉及到三个部门，办件量减少了3倍，其它事项另行通知。";
        let notes = builtin_lexicon().check(text);
        let hit = |id: &str| notes.iter().any(|note| note.entry_id == id);

        assert!(hit("GRM-001"), "应报「通过…使…」缺主语");
        assert!(hit("TYP-001"), "应报「布署」");
        assert!(hit("SYN-014"), "应报「截止目前」");
        assert!(hit("GRM-008"), "应报「涉及到」");
        assert!(hit("GRM-005"), "应报「减少了3倍」");
        assert!(hit("TYP-070"), "应报「其它」");

        // 错别字类给替换文本，句式类不给——替换整句只会把正文改坏。
        let typo = notes
            .iter()
            .find(|note| note.entry_id == "TYP-001")
            .unwrap();
        assert_eq!(typo.replacement.as_deref(), Some("部署"));
        let syntax = notes
            .iter()
            .find(|note| note.entry_id == "GRM-001")
            .unwrap();
        assert!(syntax.replacement.is_none());
    }
}

#[cfg(test)]
mod overlay_tests {
    use super::*;

    fn patched(id: &str, apply: impl FnOnce(&mut ProofreadOverride)) -> ProofreadConfig {
        let mut config = ProofreadConfig::default();
        apply(config.override_mut(id));
        config
    }

    #[test]
    fn without_any_override_the_resolved_table_equals_the_seed() {
        let resolved = Lexicon::resolved(&ProofreadConfig::default());
        assert_eq!(resolved.entries.len(), builtin_lexicon().entries.len());
        assert!(resolved.load_warnings.is_empty());
    }

    #[test]
    fn an_override_changes_only_the_field_it_names() {
        let seed = builtin_entry("TYP-001").expect("内置条目");
        let config = patched("TYP-001", |patch| patch.enabled = Some(false));
        let resolved = Lexicon::resolved(&config);
        let entry = resolved
            .entries
            .iter()
            .find(|entry| entry.id == "TYP-001")
            .expect("条目仍在");
        assert!(!entry.enabled);
        // 其余字段仍随种子走——这正是覆盖层相对整条快照的价值：以后种子改了
        // 措辞，用户也能跟着更新。
        assert_eq!(entry.suggestion, seed.suggestion);
        assert_eq!(entry.note, seed.note);
        assert_eq!(entry.level, seed.level);
    }

    #[test]
    fn a_broken_override_keeps_the_seed_value_and_warns() {
        // 用户把命中条件改坏了，不能静默按坏规则跑，也不能整条消失。
        let config = patched("TYP-001", |patch| patch.condition = Some("正则:(".into()));
        let resolved = Lexicon::resolved(&config);
        let entry = resolved
            .entries
            .iter()
            .find(|entry| entry.id == "TYP-001")
            .expect("条目仍在");
        assert_eq!(entry.condition_text, "总是", "应当退回种子的命中条件");
        assert!(
            resolved
                .load_warnings
                .iter()
                .any(|warning| warning.contains("TYP-001")),
            "必须报出来：{:?}",
            resolved.load_warnings
        );
    }

    #[test]
    fn custom_entries_are_appended_and_actually_run() {
        let mut config = ProofreadConfig::default();
        config.custom.push(ProofreadRule {
            id: "USR-001".into(),
            wrong: "我办".into(),
            suggestion: "本办".into(),
            level: "必错".into(),
            condition: "总是".into(),
            group: "自定义".into(),
            note: String::new(),
            enabled: true,
        });
        let resolved = Lexicon::resolved(&config);
        let notes = resolved.check("请将材料报我办。");
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].entry_id, "USR-001");
        assert_eq!(notes[0].replacement.as_deref(), Some("本办"));
    }

    #[test]
    fn an_override_for_a_removed_seed_entry_is_reported_not_swallowed() {
        // 模拟升级后种子里删掉了某条，而用户改过它。
        let config = patched("GONE-001", |patch| patch.enabled = Some(false));
        let resolved = Lexicon::resolved(&config);
        assert!(
            resolved
                .load_warnings
                .iter()
                .any(|warning| warning.contains("GONE-001")),
            "用户的改动不能悄无声息地消失"
        );
    }

    #[test]
    fn pruning_drops_overrides_that_no_longer_change_anything() {
        let mut config = ProofreadConfig::default();
        config.override_mut("TYP-001").enabled = Some(false);
        config.override_mut("TYP-002").enabled = None;
        config.prune();
        assert!(config.override_for("TYP-001").is_some());
        assert!(
            config.override_for("TYP-002").is_none(),
            "空覆盖不该留在配置里"
        );
    }

    #[test]
    fn custom_ids_keep_counting_up() {
        let mut config = ProofreadConfig::default();
        assert_eq!(config.next_custom_id(), "USR-001");
        config.custom.push(ProofreadRule {
            id: "USR-007".into(),
            ..Default::default()
        });
        assert_eq!(config.next_custom_id(), "USR-008");
    }
}
