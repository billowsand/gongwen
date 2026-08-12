use crate::export::{self, MarkdownBlock, MarkdownSection};
use crate::last_char_orphan;
use crate::models::{
    DraftInput, JointIssuanceMode, SecurityLevel, SecurityRules, TemplateKind, VocabularyCategory,
    VocabularyEntry, join_units, split_units,
};
use chrono::{Datelike, NaiveDate, Weekday};
use regex::Regex;

pub fn validate(
    input: &DraftInput,
    markdown: &str,
    vocabulary: &[VocabularyEntry],
    rules: &SecurityRules,
) -> Vec<String> {
    let mut warnings = Vec::new();
    let text = markdown.trim();

    validate_metadata(input, vocabulary, rules, &mut warnings);

    if text.is_empty() {
        warnings.push("模型未返回正文".into());
        return warnings;
    }
    let blocks = export::parse_markdown(text);
    let mut section = MarkdownSection::Body;
    let mut body_h1_count = 0usize;
    let mut attachment_count = 0usize;
    let mut attachment_needs_title = false;
    let mut attachment_has_content = false;
    let mut attachment_missing_title = false;
    let mut empty_attachment = false;
    for block in &blocks {
        match block {
            MarkdownBlock::Marker(MarkdownSection::Attachment) => {
                if attachment_count > 0 {
                    attachment_missing_title |= attachment_needs_title;
                    empty_attachment |= !attachment_has_content;
                }
                section = MarkdownSection::Attachment;
                attachment_count += 1;
                attachment_needs_title = true;
                attachment_has_content = false;
            }
            MarkdownBlock::Marker(MarkdownSection::Body) => {
                if section == MarkdownSection::Attachment {
                    attachment_missing_title |= attachment_needs_title;
                    empty_attachment |= !attachment_has_content;
                }
                section = MarkdownSection::Body;
                attachment_needs_title = false;
            }
            MarkdownBlock::Title(_) if section == MarkdownSection::Body => body_h1_count += 1,
            MarkdownBlock::Title(_) if attachment_needs_title => {
                attachment_needs_title = false;
                attachment_has_content = true;
            }
            MarkdownBlock::Html(_) => {}
            _ if section == MarkdownSection::Attachment => attachment_has_content = true,
            _ => {}
        }
    }
    if section == MarkdownSection::Attachment {
        attachment_missing_title |= attachment_needs_title;
        empty_attachment |= !attachment_has_content;
    }
    if body_h1_count == 0 {
        warnings.push("缺少一级标题，请在导出前补充“# 标题”".into());
    }
    if body_h1_count > 1 {
        warnings.push(
            "正文区检测到多个一级标题；正式标题只保留一个，附件前请加入“<!-- [附件] -->”".into(),
        );
    }
    if empty_attachment {
        warnings.push("检测到没有内容的附件标记".into());
    }
    if attachment_missing_title {
        warnings.push("每个附件标记之后应使用“# 附件正式标题”".into());
    }
    if text.contains("【待核实") {
        warnings.push("正文含“待核实”字段，签发前必须补齐".into());
    }

    for entry in vocabulary {
        for alias in &entry.aliases {
            if !alias.trim().is_empty() && text.contains(alias) && alias != &entry.canonical {
                warnings.push(format!(
                    "发现非规范名称“{}”，建议替换为“{}”",
                    alias, entry.canonical
                ));
            }
        }
    }

    let square_year = Regex::new(r"\[[12]\d{3}\]").expect("valid regex");
    if square_year.is_match(text) {
        warnings.push("疑似使用方括号文号年份，应核对并改为六角括号〔〕".into());
    }

    let relative_date =
        Regex::new(r"今天|明天|后天|大后天|(?:本周|下周)[一二三四五六日天]").expect("valid regex");
    if let Some(found) = relative_date.find(text) {
        warnings.push(format!(
            "正文仍含相对日期“{}”，应按生成时的本机时间换算为具体年月日和星期",
            found.as_str()
        ));
    }

    validate_date_weekdays(text, &mut warnings);

    for orphan in last_char_orphan::find_orphans(text) {
        warnings.push(last_char_orphan::format_warning(&orphan));
    }

    let mixed_time = Regex::new(r"(?:上午|下午|晚上)\s*(?:1[3-9]|2[0-3])(?:(?::|：)\d{2}|时)")
        .expect("valid regex");
    if let Some(found) = mixed_time.find(text) {
        warnings.push(format!(
            "时间“{}”混用了时段词和24小时制，建议直接写24小时制时间",
            found.as_str()
        ));
    }

    match input.kind {
        TemplateKind::OfficialLetter => {
            if input.profile.recipient.trim().is_empty() {
                warnings.push("公函缺少主送单位".into());
            }
            let issuing_value = if input.profile.joint_issuance_mode == JointIssuanceMode::Mode1 {
                &input.profile.joint_issuing_units
            } else {
                &input.profile.issuing_unit
            };
            if issuing_value.trim().is_empty() {
                warnings.push("公函缺少发文单位".into());
            }
            if input.profile.joint_issuance_mode == JointIssuanceMode::Mode1 {
                let issuing_units = split_units(&input.profile.joint_issuing_units);
                if issuing_units.len() < 2 {
                    warnings.push("联合发文模式1至少需要选择两个发文单位".into());
                }
                if !issuing_units
                    .iter()
                    .any(|unit| unit == input.profile.main_issuing_unit.trim())
                {
                    warnings.push("联合发文模式1必须从发文单位中明确一个主发文单位".into());
                }
            }
        }
        TemplateKind::PhoneNotice => {
            if input.profile.recipient.trim().is_empty() {
                warnings.push("电话通知缺少主送单位".into());
            }
            if input.profile.issuing_unit.trim().is_empty() {
                warnings.push("电话通知缺少发文单位".into());
            }
        }
        TemplateKind::PlainDocument => {}
        TemplateKind::MeetingAgenda => {
            validate_meeting_agenda_format(text, &mut warnings);
            if input.meeting_time.trim().is_empty() {
                warnings.push("会议时间未单独填写：请确认生成结果确已从素材中正确提取".into());
            }
            if input.profile.meeting_location.trim().is_empty() {
                warnings.push("会议地点未单独填写：请确认生成结果确已从素材中正确提取".into());
            }
            if input.attendees.trim().is_empty() {
                warnings.push("参加人员未单独填写：请确认生成结果确已从素材中正确归纳".into());
            }
            if input.profile.security_level.trim().is_empty() {
                warnings.push("会议议程缺少密级，Word 首行将标记为待核实".into());
            }
        }
        TemplateKind::WhitePaper => {
            if input.profile.reporting_leaders.trim().is_empty() {
                warnings.push("白头件缺少呈报领导".into());
            }
            if !text.contains("妥否，请指示") {
                warnings.push("白头件缺少规范请示结语“妥否，请指示。”".into());
            }
            if text.lines().any(|line| {
                let s = line.trim_start();
                s.starts_with("- ") || s.starts_with("* ")
            }) {
                warnings.push("白头件正文不宜使用项目符号，应改为“一是、二是……”".into());
            }
        }
        TemplateKind::RedHeadApproval => {
            if input.profile.issuing_unit.trim().is_empty() {
                warnings.push("红头呈批件缺少发文单位".into());
            }
            if input.profile.reporting_leaders.trim().is_empty() {
                warnings.push("红头呈批件缺少呈报领导".into());
            }
            if input.profile.signing_unit.trim().is_empty() {
                warnings.push("红头呈批件缺少落款单位".into());
            }
            if crate::models::joint_responsible_entries(&input.profile).is_empty() {
                warnings.push("红头呈批件至少需要一条承办单位、联系人和电话".into());
            }
            if !text.contains("妥否，请指示") {
                warnings.push("红头呈批件缺少规范请示结语“妥否，请指示。”".into());
            }
            if text.lines().any(|line| {
                let s = line.trim_start();
                s.starts_with("- ") || s.starts_with("* ")
            }) {
                warnings.push("红头呈批件正文不宜使用项目符号，应改为“一是、二是……”".into());
            }
        }
    }

    warnings.sort();
    warnings.dedup();
    warnings
}

/// 校验表单元数据本身：密级与保密期限的约束、联系人与电话的绑定关系、
/// 以及各单位名称是否取自标准词库。这部分与模型输出无关，填表阶段就能查。
fn validate_metadata(
    input: &DraftInput,
    vocabulary: &[VocabularyEntry],
    rules: &SecurityRules,
    warnings: &mut Vec<String>,
) {
    let profile = &input.profile;
    let unit_display = crate::units::UnitDisplay::new(vocabulary);
    let marking = profile.security_level.trim();
    let level = SecurityLevel::from_marking(marking);
    if !marking.is_empty() && level == SecurityLevel::Unmarked {
        warnings.push(format!(
            "密级“{marking}”不是规范写法，应为内部、秘密、机密或绝密"
        ));
    }
    if let Some(message) = rules.check(level, &profile.security_period) {
        warnings.push(message);
    }
    if profile.special_handling && marking.is_empty() {
        warnings
            .push("已勾选“指人专办”，但未选择密级；密级为空时指人专办不会出现在成稿中".to_string());
    }

    if input.kind.has_document_number() {
        let year = input.document_year();
        if year.len() != 4 || !year.chars().all(|ch| ch.is_ascii_digit()) {
            warnings.push("文号发文年份应填写为四位数字，例如“2026”".to_string());
        }
        let serial = profile.document_number.trim();
        if profile.letter_version == crate::models::LetterVersion::Formal
            && !serial.is_empty()
            && !serial.chars().all(|ch| ch.is_ascii_digit())
        {
            warnings.push("发文序号只能填写数字，不要包含“号”或完整函号".to_string());
        }
    }

    if (input.kind == TemplateKind::OfficialLetter
        && input.profile.joint_issuance_mode == JointIssuanceMode::Mode1)
        || input.kind == TemplateKind::RedHeadApproval
    {
        // 承办单位与联系人成对录入（一一对应），旧稿件按索引回落配对。
        let entries = crate::models::joint_responsible_entries(profile);
        for (index, entry) in entries.iter().enumerate() {
            if entry.unit.trim().is_empty() {
                warnings.push(format!("第 {} 行承办单位未填写", index + 1));
            }
            if entry.phone.trim().is_empty() {
                warnings.push(format!("联系人“{}”缺少联系电话", entry.name.trim()));
            }
            if let Some(vocab) = vocabulary.iter().find(|vocab| {
                vocab.category == VocabularyCategory::Person
                    && vocab.canonical.trim() == entry.name.trim()
            }) && !vocab.phone.trim().is_empty()
                && vocab.phone.trim() != entry.phone.trim()
            {
                warnings.push(format!(
                    "联系人“{}”在标准词库中的电话为“{}”，与当前填写的“{}”不一致",
                    entry.name.trim(),
                    vocab.phone.trim(),
                    entry.phone.trim()
                ));
            }
            // 规格 §2.3/2.5：承办单位、联系人、电话一一对应，联系人应属于同行承办单位。
            if let Some(vocab) = vocabulary.iter().find(|vocab| {
                vocab.category == VocabularyCategory::Person
                    && vocab.canonical.trim() == entry.name.trim()
            }) {
                let bound = vocab.unit.trim();
                let responsible = entry.unit.trim();
                if !bound.is_empty()
                    && !responsible.is_empty()
                    && !unit_display.same_unit(bound, responsible)
                {
                    warnings.push(format!(
                        "联系人“{}”属于“{}”，与第 {} 行承办单位“{}”不一致",
                        entry.name.trim(),
                        unit_display.full_name(bound),
                        index + 1,
                        unit_display.full_name(responsible)
                    ));
                }
            }
        }
    } else if input.kind == TemplateKind::OfficialLetter {
        let contact = profile.contact_person.trim();
        let phone = profile.contact_phone.trim();
        match (contact.is_empty(), phone.is_empty()) {
            (false, true) => {
                warnings.push(format!("联系人“{contact}”缺少联系电话，二者必须成对填写"))
            }
            (true, false) => warnings.push(format!(
                "填写了联系电话“{phone}”但没有联系人，二者必须成对填写"
            )),
            _ => {}
        }
        if !contact.is_empty()
            && let Some(entry) = vocabulary.iter().find(|entry| {
                entry.category == VocabularyCategory::Person && entry.canonical.trim() == contact
            })
            && !entry.phone.trim().is_empty()
            && entry.phone.trim() != phone
        {
            warnings.push(format!(
                "联系人“{contact}”在标准词库中的电话为“{}”，与当前填写的“{phone}”不一致",
                entry.phone.trim()
            ));
        }
        // 规格 §2.3“人随事走”：联系人应属于承办单位。
        if !contact.is_empty()
            && let Some(entry) = vocabulary.iter().find(|entry| {
                entry.category == VocabularyCategory::Person && entry.canonical.trim() == contact
            })
        {
            let bound = entry.unit.trim();
            let responsible = profile.responsible_unit.trim();
            if !bound.is_empty()
                && !responsible.is_empty()
                && !unit_display.same_unit(bound, responsible)
            {
                warnings.push(format!(
                    "联系人“{contact}”属于“{}”，与承办单位“{}”不一致",
                    unit_display.full_name(bound),
                    unit_display.full_name(responsible)
                ));
            }
        }
    }

    let joint_contact_names = join_units(
        &profile
            .joint_contacts
            .iter()
            .map(|contact| contact.name.clone())
            .collect::<Vec<_>>(),
    );

    match input.kind {
        TemplateKind::OfficialLetter => {
            check_units(
                if profile.joint_issuance_mode == JointIssuanceMode::Mode1 {
                    &profile.joint_issuing_units
                } else {
                    &profile.issuing_unit
                },
                &[VocabularyCategory::Unit],
                "发文单位",
                vocabulary,
                warnings,
            );
            check_department_code(&profile.department_code, vocabulary, warnings);
            check_units(
                &profile.recipient,
                &[VocabularyCategory::Unit],
                "主送单位",
                vocabulary,
                warnings,
            );
            check_units(
                &profile.copies_to,
                &[VocabularyCategory::Unit],
                "抄送单位",
                vocabulary,
                warnings,
            );
            check_units(
                if profile.joint_issuance_mode == JointIssuanceMode::Mode1 {
                    &profile.joint_responsible_units
                } else {
                    &profile.responsible_unit
                },
                &[VocabularyCategory::Unit],
                "承办单位",
                vocabulary,
                warnings,
            );
            check_units(
                if profile.joint_issuance_mode == JointIssuanceMode::Mode1 {
                    &joint_contact_names
                } else {
                    &profile.contact_person
                },
                &[VocabularyCategory::Person],
                "联系人",
                vocabulary,
                warnings,
            );
            if input.uses_external_unit_names() {
                let mut external_units = if profile.joint_issuance_mode == JointIssuanceMode::Mode1
                {
                    split_units(&profile.joint_issuing_units)
                } else {
                    split_units(&profile.issuing_unit)
                };
                external_units.extend(split_units(&profile.recipient));
                external_units.extend(split_units(&profile.copies_to));
                for unit in unit_display.missing_external_names(&external_units) {
                    warnings.push(format!(
                        "外部函使用的单位“{unit}”未设置外部名称，当前将以正常名称代替"
                    ));
                }
            }
            // 规格 §2.4：同一单位只能占发文、主送、抄送三者之一。
            let recipients = split_units(&profile.recipient);
            for unit in split_units(&profile.copies_to) {
                if recipients.contains(&unit) {
                    warnings.push(format!(
                        "“{unit}”同时出现在主送单位和抄送单位中，只能保留其一"
                    ));
                }
            }
            let issuing = if profile.joint_issuance_mode == JointIssuanceMode::Mode1 {
                split_units(&profile.joint_issuing_units)
            } else {
                split_units(&profile.issuing_unit)
            };
            for unit in &issuing {
                if recipients.contains(unit) {
                    warnings.push(format!("“{unit}”既是发文单位又是主送单位，不能给自己发文"));
                }
                if split_units(&profile.copies_to).contains(unit) {
                    warnings.push(format!("“{unit}”既是发文单位又是抄送单位，只能保留其一"));
                }
            }
        }
        TemplateKind::PhoneNotice => {
            check_units(
                &profile.issuing_unit,
                &[VocabularyCategory::Unit],
                "发文单位",
                vocabulary,
                warnings,
            );
            check_units(
                &profile.recipient,
                &[VocabularyCategory::Unit],
                "主送单位",
                vocabulary,
                warnings,
            );
        }
        TemplateKind::PlainDocument => {}
        // 会议地点直接填写，不入词库，因此无需校验。
        TemplateKind::MeetingAgenda => {}
        TemplateKind::WhitePaper => {
            check_units(
                &profile.reporting_leaders,
                &[VocabularyCategory::Person],
                "呈报领导",
                vocabulary,
                warnings,
            );
            check_units(
                &profile.signing_unit,
                &[VocabularyCategory::Unit],
                "落款单位",
                vocabulary,
                warnings,
            );
        }
        TemplateKind::RedHeadApproval => {
            check_units(
                &profile.issuing_unit,
                &[VocabularyCategory::Unit],
                "发文单位",
                vocabulary,
                warnings,
            );
            check_department_code(&profile.department_code, vocabulary, warnings);
            check_units(
                &profile.reporting_leaders,
                &[VocabularyCategory::Person],
                "呈报领导",
                vocabulary,
                warnings,
            );
            check_units(
                &profile.joint_responsible_units,
                &[VocabularyCategory::Unit],
                "承办单位",
                vocabulary,
                warnings,
            );
            check_units(
                &joint_contact_names,
                &[VocabularyCategory::Person],
                "联系人",
                vocabulary,
                warnings,
            );
            check_units(
                &profile.signing_unit,
                &[VocabularyCategory::Unit],
                "落款单位",
                vocabulary,
                warnings,
            );
        }
    }
}

/// 词库里已经维护了该类词条时，才提示表单中出现的词库外名称；
/// 词库为空说明用户尚未建库，不打扰。
fn check_units(
    value: &str,
    categories: &[VocabularyCategory],
    field: &str,
    vocabulary: &[VocabularyEntry],
    warnings: &mut Vec<String>,
) {
    let known = vocabulary
        .iter()
        .filter(|entry| categories.contains(&entry.category))
        .map(|entry| entry.canonical.trim())
        .filter(|canonical| !canonical.is_empty())
        .collect::<Vec<_>>();
    if known.is_empty() {
        return;
    }
    for unit in split_units(value) {
        if !known.iter().any(|canonical| *canonical == unit) {
            warnings.push(format!(
                "{field}“{unit}”不在标准词库中，请核对全称或先补入词库"
            ));
        }
    }
}

/// 机关代字绑定在单位上。词库里已经有单位绑定了代字时，才提示表单中出现的陌生代字。
fn check_department_code(value: &str, vocabulary: &[VocabularyEntry], warnings: &mut Vec<String>) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    let known = crate::units::department_codes(vocabulary);
    if known.is_empty() || known.iter().any(|code| code == value) {
        return;
    }
    warnings.push(format!(
        "机关代字“{value}”没有绑定到任何单位，请核对或在标准词库中补入"
    ));
}

fn validate_meeting_agenda_format(text: &str, warnings: &mut Vec<String>) {
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.first().is_none_or(|line| !line.starts_with("# ")) {
        warnings.push("会议议程第一行必须为“# 会议标题”".into());
    }

    let required = ["一、时间地点：", "二、参加人员：", "三、研讨内容："];
    let positions = required
        .iter()
        .map(|prefix| lines.iter().position(|line| line.starts_with(prefix)))
        .collect::<Vec<_>>();
    for (prefix, position) in required.iter().zip(&positions) {
        if position.is_none() {
            warnings.push(format!("会议议程缺少固定字段“{prefix}”"));
        }
    }

    if let [Some(time), Some(attendees), Some(content)] = positions.as_slice() {
        if !(*time == 1 && *attendees == 2 && *content == 3) {
            warnings.push(
                "会议议程字段必须按“标题—时间地点—参加人员—研讨内容”顺序排列，且不得插入其他段落"
                    .into(),
            );
        }

        let item_pattern = Regex::new(r"^(?P<number>\d+)\. (?P<body>.+)$").expect("valid regex");
        let item_lines = lines.iter().skip(*content + 1).copied().collect::<Vec<_>>();
        if item_lines.is_empty() {
            warnings.push("会议议程“研讨内容”下至少需要一项议程".into());
        } else {
            let mut numbers = Vec::new();
            let mut all_items_valid = true;
            for line in &item_lines {
                let Some(captures) = item_pattern.captures(line) else {
                    all_items_valid = false;
                    continue;
                };
                if let Ok(number) = captures["number"].parse::<usize>() {
                    numbers.push(number);
                }
            }
            if !all_items_valid {
                warnings.push(
                    "研讨内容只能使用“1. 事项”格式，每项单独一行，不得使用项目符号、中文序号或多级标题"
                        .into(),
                );
            }
            if numbers
                .iter()
                .enumerate()
                .any(|(index, number)| *number != index + 1)
            {
                warnings.push("会议议程事项必须从“1.”开始连续编号".into());
            }
            for (index, line) in item_lines.iter().enumerate() {
                let expected = if index + 1 == item_lines.len() {
                    '。'
                } else {
                    '；'
                };
                if !line.ends_with(expected) {
                    warnings
                        .push("会议议程除最后一项使用句号外，其余议程事项均应以分号结尾".into());
                    break;
                }
            }
        }
    }

    if lines.iter().skip(1).any(|line| {
        line.starts_with('#')
            || line.starts_with("- ")
            || line.starts_with("* ")
            || line.starts_with('|')
    }) {
        warnings.push("会议议程不得使用二级标题、Markdown 项目符号或表格".into());
    }

    let exact_skeleton = Regex::new(
        r"(?s)^# [^\n]+\n\n一、时间地点：[^\n]+\n\n二、参加人员：[^\n]+\n\n三、研讨内容：\n\n(?:\d+\. [^\n]+(?:\n|$))+\z",
    )
    .expect("valid regex");
    if !exact_skeleton.is_match(text.trim()) {
        warnings
            .push("会议议程未完全符合固定 Markdown 骨架，请重新生成或按模板调整空行和字段".into());
    }
}

fn validate_date_weekdays(text: &str, warnings: &mut Vec<String>) {
    let pattern = Regex::new(
        r"(?P<year>20\d{2})年(?P<month>\d{1,2})月(?P<day>\d{1,2})日\s*[（(]星期(?P<weekday>[一二三四五六日天])[）)]",
    )
    .expect("valid regex");
    for captures in pattern.captures_iter(text) {
        let year = captures["year"].parse::<i32>().ok();
        let month = captures["month"].parse::<u32>().ok();
        let day = captures["day"].parse::<u32>().ok();
        let full = captures.get(0).map(|value| value.as_str()).unwrap_or("");
        let Some(date) = year
            .zip(month)
            .zip(day)
            .and_then(|((year, month), day)| NaiveDate::from_ymd_opt(year, month, day))
        else {
            warnings.push(format!("日期“{full}”不是有效的公历日期"));
            continue;
        };
        let expected = weekday_cn(date.weekday());
        let supplied = &captures["weekday"];
        let supplied = if supplied == "天" { "日" } else { supplied };
        if supplied != expected {
            warnings.push(format!(
                "日期与星期不一致：“{full}”实际为星期{expected}，请核实原始素材"
            ));
        }
    }
}

fn weekday_cn(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Mon => "一",
        Weekday::Tue => "二",
        Weekday::Wed => "三",
        Weekday::Thu => "四",
        Weekday::Fri => "五",
        Weekday::Sat => "六",
        Weekday::Sun => "日",
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use crate::models::{CorrespondenceScope, VocabularyCategory, VocabularyEntry};

    fn rules() -> SecurityRules {
        SecurityRules::default()
    }

    #[test]
    fn flags_alias_and_missing_letter_metadata() {
        let input = DraftInput::default();
        let vocabulary = vec![VocabularyEntry {
            category: VocabularyCategory::Unit,
            canonical: "某某省教育厅".into(),
            aliases: vec!["省教育厅".into()],
            note: String::new(),
            phone: String::new(),
            ..Default::default()
        }];
        let warnings = validate(
            &input,
            "# 标题\n\n省教育厅拟开展有关工作。",
            &vocabulary,
            &rules(),
        );
        assert!(warnings.iter().any(|w| w.contains("某某省教育厅")));
        assert!(warnings.iter().any(|w| w.contains("主送单位")));
    }

    #[test]
    fn phone_notice_requires_only_issuing_and_recipient_metadata() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::PhoneNotice;
        let missing = validate(&input, "# 电话通知\n\n正文。", &[], &rules());
        assert!(
            missing
                .iter()
                .any(|warning| warning.contains("缺少发文单位"))
        );
        assert!(
            missing
                .iter()
                .any(|warning| warning.contains("缺少主送单位"))
        );

        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();
        input.profile.contact_person = "不应校验的隐藏联系人".into();
        let warnings = validate(&input, "# 电话通知\n\n正文。", &[], &rules());
        assert!(!warnings.iter().any(|warning| warning.contains("联系电话")));
        assert!(
            !warnings
                .iter()
                .any(|warning| warning.contains("缺少发文单位"))
        );
        assert!(
            !warnings
                .iter()
                .any(|warning| warning.contains("缺少主送单位"))
        );
    }

    #[test]
    fn red_head_approval_requires_independent_parties_and_paired_contacts() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::RedHeadApproval;
        input.profile.kind = TemplateKind::RedHeadApproval;
        input.profile.document_year = "2026".into();
        input.profile.document_number = "12".into();
        let missing = validate(&input, "# 呈批标题\n\n正文。", &[], &rules());
        for required in [
            "缺少发文单位",
            "缺少呈报领导",
            "缺少落款单位",
            "至少需要一条承办",
        ] {
            assert!(
                missing.iter().any(|warning| warning.contains(required)),
                "缺少校验：{required}，实际为 {missing:?}"
            );
        }

        input.profile.issuing_unit = "发文单位".into();
        input.profile.reporting_leaders = "张三".into();
        input.profile.signing_unit = "落款单位".into();
        input.profile.joint_responsible_units = "承办甲、承办乙".into();
        input.profile.joint_contacts = vec![
            crate::models::JointContact {
                unit: "承办甲".into(),
                name: "李四".into(),
                phone: "010-1".into(),
            },
            crate::models::JointContact {
                unit: "承办乙".into(),
                name: "王五".into(),
                phone: "010-2".into(),
            },
        ];
        let valid = validate(
            &input,
            "# 呈批标题\n\n现将有关情况呈报如下。妥否，请指示。",
            &[],
            &rules(),
        );
        assert!(
            !valid
                .iter()
                .any(|warning| warning.contains("红头呈批件缺少")
                    || warning.contains("至少需要一条承办")
                    || warning.contains("缺少联系电话")),
            "{valid:?}"
        );
    }

    #[test]
    fn joint_mode_one_requires_two_units_main_unit_and_contact_phones() {
        let mut input = DraftInput::default();
        input.profile.joint_issuance_mode = JointIssuanceMode::Mode1;
        input.profile.joint_issuing_units = "甲单位".into();
        input.profile.main_issuing_unit = "乙单位".into();
        input.profile.recipient = "收文单位".into();
        input.profile.joint_contacts = vec![crate::models::JointContact {
            unit: "甲单位".into(),
            name: "张三".into(),
            phone: String::new(),
        }];
        let warnings = validate(&input, "# 联合发文函\n\n正文。", &[], &rules());
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("至少需要选择两个"))
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("明确一个主发文单位"))
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("缺少联系电话"))
        );

        input.profile.joint_issuing_units = "甲单位、乙单位".into();
        input.profile.main_issuing_unit = "乙单位".into();
        input.profile.joint_contacts[0].phone = "010-12345678".into();
        let valid = validate(&input, "# 联合发文函\n\n正文。", &[], &rules());
        assert!(
            !valid
                .iter()
                .any(|warning| warning.contains("至少需要选择两个"))
        );
        assert!(
            !valid
                .iter()
                .any(|warning| warning.contains("明确一个主发文单位"))
        );
        assert!(!valid.iter().any(|warning| warning.contains("缺少联系电话")));
    }

    #[test]
    fn flags_period_exceeding_security_level() {
        let mut input = DraftInput::default();
        input.profile.security_level = "秘密".into();
        input.profile.security_period = "30年".into();
        let warnings = validate(&input, "# 标题\n\n正文。", &[], &rules());
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("不得超过10年")),
            "{warnings:?}"
        );
    }

    #[test]
    fn flags_contact_phone_mismatch_and_orphan_phone() {
        let vocabulary = vec![VocabularyEntry {
            category: VocabularyCategory::Person,
            canonical: "张三处长".into(),
            aliases: vec![],
            note: String::new(),
            phone: "010-12345678".into(),
            ..Default::default()
        }];
        let mut input = DraftInput::default();
        input.profile.contact_person = "张三处长".into();
        input.profile.contact_phone = "010-00000000".into();
        let warnings = validate(&input, "# 标题\n\n正文。", &vocabulary, &rules());
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("电话不一致") || warning.contains("与当前填写的")),
            "{warnings:?}"
        );

        let mut orphan = DraftInput::default();
        orphan.profile.contact_phone = "010-12345678".into();
        let warnings = validate(&orphan, "# 标题\n\n正文。", &[], &rules());
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("没有联系人"))
        );
    }

    #[test]
    fn flags_a_unit_listed_as_both_recipient_and_copy() {
        let mut input = DraftInput::default();
        input.profile.recipient = "某某市教育局、某某市财政局".into();
        input.profile.copies_to = "某某市财政局".into();
        let warnings = validate(&input, "# 标题\n\n正文。", &[], &rules());
        assert!(
            warnings.iter().any(|warning| warning.contains("某某市财政局")
                && warning.contains("只能保留其一")),
            "{warnings:?}"
        );

        let mut clean = DraftInput::default();
        clean.profile.recipient = "某某市教育局".into();
        clean.profile.copies_to = "某某市财政局".into();
        let warnings = validate(&clean, "# 标题\n\n正文。", &[], &rules());
        assert!(
            !warnings
                .iter()
                .any(|warning| warning.contains("只能保留其一"))
        );
    }

    #[test]
    fn flags_an_issuing_unit_that_is_also_a_recipient_or_copy() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.issuing_unit = "甲单位".into();
        input.profile.recipient = "甲单位、乙单位".into();
        input.profile.copies_to = "甲单位".into();
        let warnings = validate(&input, "# 测试函\n\n正文。", &[], &rules());
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("既是发文单位又是主送单位")),
            "{warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("既是发文单位又是抄送单位")),
            "{warnings:?}"
        );

        // 三者互不重叠时不应报警。
        input.profile.recipient = "乙单位".into();
        input.profile.copies_to = "丙单位".into();
        let warnings = validate(&input, "# 测试函\n\n正文。", &[], &rules());
        assert!(
            !warnings
                .iter()
                .any(|warning| warning.contains("既是发文单位")),
            "{warnings:?}"
        );
    }

    #[test]
    fn flags_units_outside_the_vocabulary() {
        let vocabulary = vec![VocabularyEntry {
            category: VocabularyCategory::Unit,
            canonical: "某某市教育局".into(),
            aliases: vec![],
            note: String::new(),
            phone: String::new(),
            ..Default::default()
        }];
        let mut input = DraftInput::default();
        input.profile.recipient = "某某市教育局、某某市财政局".into();
        let warnings = validate(&input, "# 标题\n\n正文。", &vocabulary, &rules());
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("某某市财政局") && warning.contains("不在标准词库")),
            "{warnings:?}"
        );
        assert!(
            !warnings
                .iter()
                .any(|warning| warning.contains("某某市教育局") && warning.contains("不在标准词库"))
        );
    }

    #[test]
    fn external_letter_warns_and_falls_back_only_for_missing_external_names() {
        let vocabulary = vec![
            VocabularyEntry {
                category: VocabularyCategory::Unit,
                canonical: "甲单位".into(),
                external_name: "甲单位对外名称".into(),
                ..Default::default()
            },
            VocabularyEntry {
                category: VocabularyCategory::Unit,
                canonical: "乙单位".into(),
                ..Default::default()
            },
        ];
        let mut input = DraftInput::default();
        input.profile.issuing_unit = "甲单位".into();
        input.profile.recipient = "乙单位".into();
        input.profile.correspondence_scope = CorrespondenceScope::External;
        let warnings = validate(&input, "# 测试函\n\n正文。", &vocabulary, &rules());
        assert!(warnings.iter().any(|warning| {
            warning.contains("乙单位")
                && warning.contains("未设置外部名称")
                && warning.contains("正常名称代替")
        }));
        assert!(!warnings.iter().any(|warning| {
            warning.contains("甲单位") && warning.contains("未设置外部名称")
        }));

        input.profile.correspondence_scope = CorrespondenceScope::Internal;
        let warnings = validate(&input, "# 测试函\n\n正文。", &vocabulary, &rules());
        assert!(
            !warnings
                .iter()
                .any(|warning| warning.contains("未设置外部名称"))
        );
    }

    #[test]
    fn flags_relative_and_inconsistent_dates() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::MeetingAgenda;
        input.meeting_time = "明天下午2点".into();
        input.profile.meeting_location = "第一会议室".into();
        let warnings = validate(
            &input,
            "# 会议议程\n\n会议定于明天召开，即2026年8月5日（星期四）下午14:30。",
            &[],
            &rules(),
        );
        assert!(warnings.iter().any(|warning| warning.contains("相对日期")));
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("实际为星期三"))
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("混用了时段词"))
        );
    }

    #[test]
    fn accepts_fixed_meeting_agenda_markdown() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::MeetingAgenda;
        input.meeting_time = "2026年8月5日（星期三）14:30".into();
        input.profile.meeting_location = "3C会议室".into();
        input.attendees = "张三同志、项目组成员".into();
        input.profile.security_level = "机密".into();
        input.profile.security_period = "10年".into();
        let markdown = "# 专题研商会议议程\n\n一、时间地点：2026年8月5日（星期三）14:30，3C会议室。\n\n二、参加人员：张三同志、项目组成员。\n\n三、研讨内容：\n\n1. 汇报总体思路；\n2. 研究下一步工作。";

        let warnings = validate(&input, markdown, &[], &rules());
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn attachment_title_does_not_count_as_a_second_document_title() {
        let mut input = DraftInput::default();
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();
        let markdown =
            "# 测试函\n<!-- [正文] -->\n正文。\n<!-- [附件] -->\n# 附件1\n## 统计表\n附件内容。";
        let warnings = validate(&input, markdown, &[], &rules());
        assert!(
            !warnings
                .iter()
                .any(|warning| warning.contains("多个一级标题")),
            "{warnings:?}"
        );
    }

    #[test]
    fn warns_when_attachment_marker_has_no_content() {
        let mut input = DraftInput::default();
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();
        let warnings = validate(&input, "# 测试函\n正文。\n<!-- [附件] -->", &[], &rules());
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("没有内容的附件标记")),
            "{warnings:?}"
        );
    }

    #[test]
    fn validates_attachment_label_and_formal_title_structure() {
        let mut input = DraftInput::default();
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();
        let valid = validate(
            &input,
            "# 测试函\n正文。\n<!-- [附件] -->\n# 情况统计表\n附件内容。",
            &[],
            &rules(),
        );
        assert!(
            !valid.iter().any(|warning| warning.contains("附件正式标题")),
            "{valid:?}"
        );

        let invalid = validate(
            &input,
            "# 测试函\n正文。\n<!-- [附件] -->\n## 情况统计表\n附件内容。",
            &[],
            &rules(),
        );
        assert!(
            invalid
                .iter()
                .any(|warning| warning.contains("附件正式标题")),
            "{invalid:?}"
        );
    }

    #[test]
    fn flags_nonstandard_meeting_agenda_markdown() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::MeetingAgenda;
        input.meeting_time = "2026年8月5日（星期三）14:30".into();
        input.profile.meeting_location = "3C会议室".into();
        input.attendees = "张三同志、项目组成员".into();
        input.profile.security_level = "机密".into();
        input.profile.security_period = "10年".into();
        let markdown =
            "# 专题会\n\n## 会议时间\n2026年8月5日（星期三）14:30\n\n## 议程\n- 汇报情况";

        let warnings = validate(&input, markdown, &[], &rules());
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("缺少固定字段"))
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("不得使用二级标题"))
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("固定 Markdown 骨架"))
        );
    }
}
