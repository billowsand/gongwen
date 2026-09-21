use crate::export::{self, MarkdownBlock, MarkdownSection};
use crate::models::{
    DraftInput, JointIssuanceMode, ReviewNote, SecurityLevel, SecurityRules, TemplateKind,
    VocabularyCategory, VocabularyEntry, join_units, split_units,
};
use chrono::{Datelike, NaiveDate, Weekday};
use regex::Regex;

/// 排版层面的粗估提示（段末挂单字），与 `validate` 分开。
///
/// 它只在**没有实测结果**时使用：编译过一次之后，`orphan_probe` 会给出同一批
/// 段落的精确行数与末行字数，两者一起显示只会互相打架。调用方（`revalidate`）
/// 负责二选一。带上段落的字节范围，审校面板据此做成可点击定位。
pub fn estimate_layout_notes(markdown: &str) -> Vec<ReviewNote> {
    crate::last_char_orphan::find_orphans(markdown)
        .iter()
        .map(|orphan| {
            let message = crate::last_char_orphan::format_warning(orphan);
            match export::block_span_for_line(markdown, orphan.start_line) {
                Some(span) => ReviewNote::located(message, span),
                None => ReviewNote::from(message),
            }
        })
        .collect()
}

/// 正文为空的提示语。导出闸门要按它认人，所以拎成常量，别让两处文案各写各的。
const EMPTY_BODY: &str = "模型未返回正文";

/// 报告题名（正文区段的 `#`）的三条检查。
///
/// 题名印在封面上，正文纸上不排第二遍——所以它既不编号，也没有 `\ref` 引得到
/// 的号：锚点挂上去，PDF 里是 undefined，预览里是 `??`。题名的正主是文档要素
/// 的「文件名称」（导出时写进 frontmatter，封面按它印），正文里那个只是没填
/// 文件名称时的兜底，两处对不上就得说一声。
///
/// 只数正文区：附录里的 `#` 是合法的附录章标题，摘要、版本变更记录、参考文献
/// 区段里的标题另有归属，都不算。
fn research_title_warnings(input: &DraftInput, text: &str, warnings: &mut Vec<String>) {
    let titles = export::research_report_titles(text);
    if titles.len() >= 2 {
        warnings.push("研究报告正文里报告题名（#）整篇只应有一个".into());
    }
    let Some(first) = titles.first() else {
        return;
    };
    let (title, anchor) = export::crossref::split_label(first);
    if let Some(id) = anchor {
        warnings.push(format!(
            "报告题名不编号，锚点 {{#{id}}} 挂在它上面引不出编号，请改挂到章、表或图上"
        ));
    }
    let hint = input.title_hint.trim();
    let title = title.trim();
    if !hint.is_empty() && !title.is_empty() && title != hint {
        warnings.push(format!(
            "正文的报告题名“{title}”与文档要素的文件名称“{hint}”不一致，封面按文件名称印"
        ));
    }
}

/// 研究报告行内标记的核对：悬空交叉引用在 PDF 里会印成 `??`，重复锚点的编号
/// 以先到的为准，缺键的文献引用印出来是 `[?]`——都提前在这里说出来。
fn research_mark_warnings(input: &DraftInput, text: &str, warnings: &mut Vec<String>) {
    // 挂在报告题名上的锚点不生效（见 [`research_title_warnings`]），不能算数：
    // 引到它的 `{@id}` 照样要报悬空，否则预览印 `??`、校验却说没事。
    let on_title: Vec<&str> = export::research_report_titles(text)
        .into_iter()
        .filter_map(|line| export::crossref::split_label(line).1)
        .collect();
    // 一趟走完：交叉引用要对照的去重清单，和重复定义的计数，都出自这一张表。
    let mut labels: Vec<(&str, usize)> = Vec::new();
    for line in text.lines() {
        let Some(id) = export::crossref::split_label(line).1 else {
            continue;
        };
        match labels.iter_mut().find(|(known, _)| *known == id) {
            Some((_, count)) => *count += 1,
            None => labels.push((id, 1)),
        }
    }
    for id in export::crossref::crossref_ids(text) {
        let defined = labels
            .iter()
            .find(|(known, _)| *known == id)
            .map_or(0, |(_, count)| *count);
        let dead = on_title.iter().filter(|known| **known == id).count();
        if defined == dead {
            warnings.push(format!("交叉引用 {{@{id}}} 没有对应的锚点 {{#{id}}}"));
        }
    }
    for (id, count) in labels {
        if count > 1 {
            warnings.push(format!("锚点 {{#{id}}} 定义了多次，编号以先到的为准"));
        }
    }
    let bib = input.research.bibliography_content.trim();
    if bib.is_empty() {
        return;
    }
    let known_keys = export::crossref::bibtex_keys(bib);
    let mut reported: Vec<&str> = Vec::new();
    for key in export::crossref::citation_keys(text) {
        if known_keys.iter().any(|known| known == key) || reported.contains(&key) {
            continue;
        }
        reported.push(key);
        warnings.push(format!("文献引用 [@{key}] 不在已导入的参考文献里"));
    }
}

/// 研究报告的区段结构：附件标识与附录一一对应，不编号的区段要自带标题。
///
/// 附件的写法与公文统一——每加一份附件就写一个附件标识。mdx 那边两种写法都编
/// 得出来（一个标识后面跟几个章标题，就是几份附录），但两种混着写，附录的字母
/// 编号从哪儿断开就只有编译完才看得出来。
///
/// 摘要之外的不编号区段（版本变更记录、参考文献），标题由区段里的首个标题充
/// 当：没有标题，编译出来的 PDF 里这一节就是无题的。
fn research_section_warnings(text: &str, warnings: &mut Vec<String>) {
    use export::ResearchSection as Section;

    // 每个区段，以及区段里数出来的章标题条数。
    let mut spans = vec![(Section::Body, 0usize)];
    // 附录段里写过 `#` 就整体下移一级，`##` 不再是章——与 mdx 的判定一致。
    let mut saw_h1 = false;
    for line in text.lines() {
        if let Some(next) = export::parse_research_marker(line) {
            spans.push((next, 0));
            saw_h1 = false;
            continue;
        }
        let level = line.len() - line.trim_start_matches('#').len();
        if level == 0 || !line[level..].starts_with(' ') {
            continue;
        }
        let (section, chapters) = spans.last_mut().expect("至少有正文一段");
        match section {
            Section::Appendix => {
                saw_h1 |= level == 1;
                if level == 1 || (level == 2 && !saw_h1) {
                    *chapters += 1;
                }
            }
            // 摘要的标题由 `\begin{abstract}` 自己排，一个不写也不缺题。
            Section::Abstract => {}
            _ if level <= 2 => *chapters += 1,
            _ => {}
        }
    }

    let appendices = spans
        .iter()
        .filter(|(section, _)| *section == Section::Appendix);
    if appendices.clone().any(|(_, chapters)| *chapters > 1) {
        warnings.push(
            "一个附件标识下有多份附录；与公文一致，每份附录前各写一个“<!-- [附录] -->”".into(),
        );
    }
    if appendices.clone().any(|(_, chapters)| *chapters == 0) {
        warnings.push("附件标识之后应写“## 附录标题”".into());
    }
    for (section, _) in spans.iter().filter(|(section, chapters)| {
        matches!(section, Section::ChangeLog | Section::References) && *chapters == 0
    }) {
        let label = section.label();
        warnings.push(format!(
            "“{label}”区段应以“## {label}”开头，否则编译出的 PDF 里这一节没有标题"
        ));
    }
}

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
        warnings.push(EMPTY_BODY.into());
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
    if input.kind.is_research() {
        research_title_warnings(input, text, &mut warnings);
        if !text.lines().any(|line| line.starts_with("## ")) {
            warnings.push("研究报告正文缺少“## 章标题”".into());
        }
        if text.starts_with("---") {
            warnings.push("研究报告 frontmatter 应填写到左侧“文档要素”，不要放在正文中".into());
        }
        if text.contains("[@") && input.research.bibliography_content.trim().is_empty() {
            warnings.push("正文包含 BibTeX 引用，但尚未导入 .bib 参考文献文件".into());
        }
        research_mark_warnings(input, text, &mut warnings);
        research_section_warnings(text, &mut warnings);
    } else {
        if body_h1_count == 0 {
            warnings.push("缺少一级标题，请在导出前补充“# 标题”".into());
        }
        if body_h1_count > 1 {
            warnings.push(
                "正文区检测到多个一级标题；正式标题只保留一个，附件前请加入“<!-- [附件] -->”"
                    .into(),
            );
        }
        if empty_attachment {
            warnings.push("检测到没有内容的附件标记".into());
        }
        if attachment_missing_title {
            warnings.push("每个附件标记之后应使用“# 附件正式标题”".into());
        }
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
            let responsible = crate::models::joint_responsible_entries(&input.profile);
            if responsible.is_empty() {
                warnings.push("红头呈批件至少需要一条承办单位、联系人和电话".into());
            }
            if responsible.len() > crate::models::RED_APPROVAL_MAX_RESPONSIBLE {
                warnings.push(format!(
                    "红头呈批件承办信息最多 {} 条，当前 {} 条，首页承办区会挤占批示框和正文",
                    crate::models::RED_APPROVAL_MAX_RESPONSIBLE,
                    responsible.len()
                ));
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
        TemplateKind::ResearchReport => {}
    }

    warnings.sort();
    warnings.dedup();
    warnings
}

/// 签发前必须处理的问题：事实缺失、元数据不完整、结构错误和日期冲突。
///
/// **它不是导出闸门。** 这些问题决定的是这份稿子能不能签发，不决定 PDF 能不能
/// 排出来——缺主送单位的稿子照样编译得出一份完整的 PDF，而用户往往正是要拿这份
/// PDF 去看版式、去请人过目。挡着不让编译，等于用「稿子还没定稿」这件人人都知道
/// 的事，去换掉一个真正有用的功能。所以这里只报数、只进审校抽屉，导出照做；
/// 真正做不出成品的情况由 [`compile_blocking_issues`] 单独判。
pub fn mustfix_issues(
    input: &DraftInput,
    markdown: &str,
    vocabulary: &[VocabularyEntry],
    rules: &SecurityRules,
) -> Vec<String> {
    validate(input, markdown, vocabulary, rules)
        .into_iter()
        .filter(|message| !is_advisory(message))
        .collect()
}

/// 真正让导出做不出成品的问题——唯一的导出闸门。
///
/// 判据只有一条：**没有它就连 .tex 都写不出来，更谈不上编译。** 目前只有「正文
/// 为空」符合。其余校验结论（要素缺失、名称不规范、日期冲突、结构瑕疵）全都不
/// 影响 Tectonic 能不能编译成功，因此一律不挡；它们照常出现在审校抽屉里。
///
/// 往这里加条目要非常克制：先问「这条真的会让 PDF 编不出来吗」，答案不是斩钉截铁
/// 的「是」就不该进来。排版挤占、超长、越界之类的问题编译得出 PDF，只是难看，
/// 那是审校提示要说的事，不是闸门要挡的事。
pub fn compile_blocking_issues(
    input: &DraftInput,
    markdown: &str,
    vocabulary: &[VocabularyEntry],
    rules: &SecurityRules,
) -> Vec<String> {
    mustfix_issues(input, markdown, vocabulary, rules)
        .into_iter()
        .filter(|message| blocks_compilation(message))
        .collect()
}

/// 这条校验结论会不会让导出根本做不出成品。
fn blocks_compilation(message: &str) -> bool {
    message == EMPTY_BODY
}

fn is_advisory(message: &str) -> bool {
    if message.starts_with("发现非规范名称") {
        return false;
    }
    ["疑似", "建议", "不宜", "请确认", "未单独填写"]
        .iter()
        .any(|marker| message.contains(marker))
}

/// 校验表单元数据本身：密级与保密期限的约束、联系人与电话的绑定关系、
/// 以及各单位名称是否取自标准词库。这部分与模型输出无关，填表阶段就能查。
fn validate_metadata(
    input: &DraftInput,
    vocabulary: &[VocabularyEntry],
    rules: &SecurityRules,
    warnings: &mut Vec<String>,
) {
    if input.kind.is_research() {
        let metadata = &input.research;
        for (label, value) in [
            ("文件名称", input.title_hint.as_str()),
            ("密级", metadata.security.as_str()),
            ("文件类型", metadata.file_type.as_str()),
            ("撰写单位", metadata.institution.as_str()),
            ("撰写时间", metadata.date.as_str()),
        ] {
            if value.trim().is_empty() {
                warnings.push(format!("研究报告缺少{label}"));
            }
        }
        if !metadata.bibliography_name.trim().is_empty()
            && metadata.bibliography_content.trim().is_empty()
        {
            warnings.push("研究报告参考文献文件为空，请重新导入 BibTeX".into());
        }
        return;
    }
    let profile = &input.profile;
    let unit_display = crate::units::UnitDisplay::new(vocabulary);
    let marking = profile.security_level.trim();
    let level = SecurityLevel::from_marking(marking);
    if marking.is_empty() {
        warnings.push("缺少密级：密级是所有文稿类型的必要填报项目，默认机密、保密期限20年".into());
    } else if level == SecurityLevel::Unmarked {
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

    // 份号只有公函的版头有位置放。切换文种后旧配置可能仍带着这个开关，
    // 那样勾着却不生效，必须说出来，别让人以为印出来是编了号的。
    if profile.number_copies && !input.kind.has_copy_numbering() {
        warnings.push(format!(
            "已勾选“逐份编号”，但{}的版头没有份号位，导出仍只有一份且不编号",
            input.kind.label()
        ));
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
        TemplateKind::ResearchReport => {}
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

    /// 孤行提示必须带着段落范围回来，审校面板才点得动。
    #[test]
    fn layout_notes_carry_the_paragraph_span() {
        // 首行缩进 2 字后排 26 字，其后每行 28 字；凑到 26+28+1 个字位末行只剩一个。
        let filler: String = "工作要点部署要求经研究决定开展检查评估现将有关事项通知如下请各单位遵照执行并及时反馈情况我们将根据反馈意见完善制度"
            .chars()
            .take(54)
            .collect();
        assert_eq!(filler.chars().count(), 54, "构造的段落长度要精确");
        let markdown = format!("# 标题\n\n短段落。\n\n{filler}。\n");
        let notes = estimate_layout_notes(&markdown);
        assert_eq!(notes.len(), 1, "只有末行挂字的那段该报：{notes:?}");
        let span = notes[0].span.clone().expect("提示要带上定位范围");
        assert_eq!(&markdown[span], format!("{filler}。"));
        assert!(notes[0].message.contains("第 5 行"));
    }
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

        // 承办条目有硬上限：界面已封顶，旧稿件超限时要提示（首页承办区放不下）。
        input.profile.joint_contacts = (0..crate::models::RED_APPROVAL_MAX_RESPONSIBLE + 1)
            .map(|index| crate::models::JointContact {
                unit: format!("承办{index}处"),
                name: "李四".into(),
                phone: "010-1".into(),
            })
            .collect();
        let pairs = input
            .profile
            .joint_contacts
            .iter()
            .map(|contact| crate::models::JointResponsibleEntry {
                unit: contact.unit.clone(),
                name: contact.name.clone(),
                phone: contact.phone.clone(),
            })
            .collect::<Vec<_>>();
        crate::models::sync_joint_responsible(&mut input.profile, &pairs);
        let over = validate(&input, "# 呈批标题\n\n妥否，请指示。", &[], &rules());
        assert!(
            over.iter().any(|warning| warning.contains("承办信息最多")),
            "超过上限应提示：{over:?}"
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
    fn warns_when_copy_numbering_is_on_for_a_kind_without_a_serial_slot() {
        // 切换文种后旧配置可能仍带着这个开关，勾着却不生效，必须说出来。
        let mut input = DraftInput {
            kind: TemplateKind::WhitePaper,
            profile: crate::models::TemplateProfile::for_kind(TemplateKind::WhitePaper),
            ..Default::default()
        };
        input.profile.number_copies = true;
        let warnings = validate(&input, "# 标题\n\n正文。", &[], &rules());
        assert!(
            warnings.iter().any(|w| w.contains("份号位")),
            "应当提示份号不生效：{warnings:?}"
        );
    }

    #[test]
    fn copy_numbering_on_a_letter_is_silent() {
        let mut input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            profile: crate::models::TemplateProfile::for_kind(TemplateKind::OfficialLetter),
            ..Default::default()
        };
        input.profile.number_copies = true;
        let warnings = validate(&input, "# 标题\n\n正文。", &[], &rules());
        assert!(!warnings.iter().any(|w| w.contains("份号位")));
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

    /// 研究报告的附件写法与公文统一：每份附录各写一个附件标识。
    #[test]
    fn research_appendices_want_one_marker_each() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::ResearchReport;
        input.profile.kind = TemplateKind::ResearchReport;
        let crowded = validate(
            &input,
            "## 研究背景\n\n正文。\n\n<!-- [附录] -->\n\n## 调查问卷\n\n问卷。\n\n## 原始数据\n\n数据。\n",
            &[],
            &rules(),
        );
        assert!(
            crowded
                .iter()
                .any(|warning| warning.contains("每份附录前各写一个")),
            "{crowded:?}"
        );

        let separated = validate(
            &input,
            "## 研究背景\n\n正文。\n\n<!-- [附录] -->\n\n## 调查问卷\n\n问卷。\n\n<!-- [附录] -->\n\n## 原始数据\n\n数据。\n",
            &[],
            &rules(),
        );
        assert!(
            !separated.iter().any(|warning| warning.contains("附录")),
            "每份附录各带标识就不该再提示：{separated:?}"
        );
    }

    /// 版本变更记录、参考文献的标题由区段里的首个标题充当，缺了 PDF 里就是无题的。
    #[test]
    fn an_unnumbered_research_section_without_a_heading_is_flagged() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::ResearchReport;
        input.profile.kind = TemplateKind::ResearchReport;
        let warnings = validate(
            &input,
            "## 研究背景\n\n正文。\n\n<!-- [版本变更记录] -->\n\n| 版本 | 日期 |\n| --- | --- |\n| V1.0 | 2026年1月 |\n",
            &[],
            &rules(),
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("“版本变更记录”区段应以")),
            "{warnings:?}"
        );
    }

    /// 报告标题（#）是合法写法，但整篇只应有一个；附录里的 `#` 是附录自己的
    /// 章标题，不计入。
    #[test]
    fn research_body_allows_one_report_title_but_not_two() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::ResearchReport;
        input.profile.kind = TemplateKind::ResearchReport;

        let single = validate(
            &input,
            "# 某某问题研究报告\n\n## 研究背景\n\n正文。\n",
            &[],
            &rules(),
        );
        assert!(
            !single.iter().any(|warning| warning.contains("报告题名")),
            "单个报告题名不该提示：{single:?}"
        );

        let doubled = validate(
            &input,
            "# 某某问题研究报告\n\n## 研究背景\n\n正文。\n\n# 又一个标题\n",
            &[],
            &rules(),
        );
        assert!(
            doubled
                .iter()
                .any(|warning| warning.contains("报告题名（#）整篇只应有一个")),
            "{doubled:?}"
        );

        let in_appendix = validate(
            &input,
            "# 某某问题研究报告\n\n## 研究背景\n\n正文。\n\n<!-- [附录] -->\n\n# 调查问卷\n\n问卷。\n",
            &[],
            &rules(),
        );
        assert!(
            !in_appendix
                .iter()
                .any(|warning| warning.contains("报告题名")),
            "附录里的 # 是附录章标题，不该计入：{in_appendix:?}"
        );
    }

    /// 封面按文档要素的「文件名称」印，正文 `#` 只是兜底：两处都写了又对不上，
    /// 作者改的那一份不会出现在纸上，得先说一声。
    #[test]
    fn a_report_title_that_differs_from_the_file_name_is_flagged() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::ResearchReport;
        input.profile.kind = TemplateKind::ResearchReport;
        input.title_hint = "某某问题研究报告".into();

        let matched = validate(&input, "# 某某问题研究报告\n\n## 研究背景\n", &[], &rules());
        assert!(
            !matched.iter().any(|warning| warning.contains("不一致")),
            "题名与文件名称一致就不该提示：{matched:?}"
        );

        let mismatched = validate(&input, "# 另一个题名\n\n## 研究背景\n", &[], &rules());
        assert!(
            mismatched
                .iter()
                .any(|warning| warning.contains("与文档要素的文件名称")),
            "{mismatched:?}"
        );

        // 正文没写题名是常态（封面自己有），不提示。
        let no_title = validate(&input, "## 研究背景\n\n正文。\n", &[], &rules());
        assert!(
            !no_title.iter().any(|warning| warning.contains("不一致")),
            "{no_title:?}"
        );
    }

    /// 报告题名不落版面也就没有编号，锚点挂上去引不出号：既要提示改挂，
    /// 引到它的 `{@id}` 也得按悬空报——否则预览印 `??`、校验却说没事。
    #[test]
    fn an_anchor_on_the_report_title_is_flagged_and_does_not_resolve() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::ResearchReport;
        input.profile.kind = TemplateKind::ResearchReport;

        let warnings = validate(
            &input,
            "# 某某问题研究报告 {#chap:t}\n\n## 研究背景\n\n见{@chap:t}。\n",
            &[],
            &rules(),
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("锚点 {#chap:t} 挂在它上面引不出编号")),
            "{warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("交叉引用 {@chap:t} 没有对应的锚点")),
            "题名上的锚点不算数，引它仍是悬空：{warnings:?}"
        );

        // 同一个 id 在章上另有一处定义时，引用解析得了，只剩重复定义的提示。
        let also_on_chapter = validate(
            &input,
            "# 报告 {#chap:t}\n\n## 研究背景 {#chap:t}\n\n见{@chap:t}。\n",
            &[],
            &rules(),
        );
        assert!(
            !also_on_chapter
                .iter()
                .any(|warning| warning.contains("没有对应的锚点")),
            "{also_on_chapter:?}"
        );
        assert!(
            also_on_chapter
                .iter()
                .any(|warning| warning.contains("定义了多次")),
            "{also_on_chapter:?}"
        );
    }

    /// 悬空交叉引用在 PDF 里会印成 `??`，校验要抢在编译前指出来；同一个 id
    /// 引用多次只报一次。
    #[test]
    fn dangling_crossrefs_are_flagged_once_each() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::ResearchReport;
        input.profile.kind = TemplateKind::ResearchReport;

        let dangling = validate(
            &input,
            "# 报告\n\n## 研究背景 {#chap:bg}\n\n见{@chap:bg}章与{@chap:nope}节，再引{@chap:nope}。\n",
            &[],
            &rules(),
        );
        let nope: Vec<_> = dangling
            .iter()
            .filter(|warning| warning.contains("{@chap:nope} 没有对应的锚点 {#chap:nope}"))
            .collect();
        assert_eq!(nope.len(), 1, "同一悬空引用只报一次：{dangling:?}");
        assert!(
            !dangling
                .iter()
                .any(|warning| warning.contains("chap:bg} 没有")),
            "有锚点的引用不该报：{dangling:?}"
        );

        let resolved = validate(
            &input,
            "# 报告\n\n## 研究背景 {#chap:bg}\n\n见{@chap:bg}章。\n",
            &[],
            &rules(),
        );
        assert!(
            !resolved
                .iter()
                .any(|warning| warning.contains("没有对应的锚点")),
            "引用都有锚点就不该提示：{resolved:?}"
        );
    }

    /// 同一锚点定义多次时编号以先到的为准，要提示用户删掉多余的。
    #[test]
    fn duplicate_anchors_are_flagged() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::ResearchReport;
        input.profile.kind = TemplateKind::ResearchReport;

        let duplicated = validate(
            &input,
            "# 报告\n\n## 研究背景 {#chap:bg}\n\n正文。\n\n## 研究方法 {#chap:bg}\n\n正文。\n",
            &[],
            &rules(),
        );
        assert!(
            duplicated
                .iter()
                .any(|warning| warning.contains("锚点 {#chap:bg} 定义了多次")),
            "{duplicated:?}"
        );

        let distinct = validate(
            &input,
            "# 报告\n\n## 研究背景 {#chap:bg}\n\n正文。\n\n## 研究方法 {#chap:m}\n\n正文。\n",
            &[],
            &rules(),
        );
        assert!(
            !distinct
                .iter()
                .any(|warning| warning.contains("定义了多次")),
            "锚点各不相同就不该提示：{distinct:?}"
        );
    }

    /// 已导入 .bib 时逐键核对：正文引用的键不在文献库里要指出来，同一缺键
    /// 只报一次；没导入 .bib 时走原有的粗粒度提示，不逐键报。
    #[test]
    fn citation_keys_are_checked_against_the_imported_bib() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::ResearchReport;
        input.profile.kind = TemplateKind::ResearchReport;
        input.research.bibliography_content =
            "@article{wang2020, title={A}}\n@book{li2021, title={B}}\n".into();

        let missing = validate(
            &input,
            "# 报告\n\n## 研究背景\n\n综述[@wang2020; @nope]，再引[@nope]。\n",
            &[],
            &rules(),
        );
        let nope: Vec<_> = missing
            .iter()
            .filter(|warning| warning.contains("文献引用 [@nope] 不在已导入的参考文献里"))
            .collect();
        assert_eq!(nope.len(), 1, "同一缺键只报一次：{missing:?}");
        assert!(
            !missing
                .iter()
                .any(|warning| warning.contains("[@wang2020] 不在")),
            "文献库里有的键不该报：{missing:?}"
        );

        let all_known = validate(
            &input,
            "# 报告\n\n## 研究背景\n\n综述[@wang2020; @li2021]。\n",
            &[],
            &rules(),
        );
        assert!(
            !all_known
                .iter()
                .any(|warning| warning.contains("不在已导入的参考文献里")),
            "键都在文献库里就不该提示：{all_known:?}"
        );

        input.research.bibliography_content.clear();
        let no_bib = validate(
            &input,
            "# 报告\n\n## 研究背景\n\n综述[@nope]。\n",
            &[],
            &rules(),
        );
        assert!(
            !no_bib
                .iter()
                .any(|warning| warning.contains("不在已导入的参考文献里")),
            "没导入 .bib 时不逐键报：{no_bib:?}"
        );
        assert!(
            no_bib.iter().any(|warning| warning.contains("尚未导入")),
            "没导入 .bib 的粗粒度提示应保留：{no_bib:?}"
        );
    }

    /// 密级是所有文稿类型的必要填报项目：任何文种留空都应提示补填。
    #[test]
    fn every_kind_requires_security_level() {
        for kind in TemplateKind::ALL {
            let mut input = DraftInput::default();
            input.kind = kind;
            input.profile.kind = kind;
            if kind.is_research() {
                input.research.security.clear();
                input.research.security_years.clear();
            } else {
                input.profile.security_level.clear();
                input.profile.security_period.clear();
            }
            let warnings = validate(&input, "# 标题\n\n正文。", &[], &rules());
            assert!(
                warnings.iter().any(|warning| warning.contains("缺少密级")),
                "{kind:?} 留空密级应提示补填，实际为 {warnings:?}"
            );
        }
    }

    /// 默认稿（未显式清空密级）应自带“机密、20年”，不产生缺少密级提示。
    #[test]
    fn default_draft_carries_confidential_20_years() {
        let input = DraftInput::default();
        let warnings = validate(&input, "# 标题\n\n正文。", &[], &rules());
        assert!(
            !warnings.iter().any(|warning| warning.contains("缺少密级")),
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

    #[test]
    fn mustfix_issues_keep_missing_facts_but_drop_style_advice() {
        let mut input = DraftInput::default();
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();
        input.profile.security_level = "机密".into();
        input.profile.security_period = "10年".into();
        let blockers = mustfix_issues(
            &input,
            "# 关于测试的函\n\n请于【待核实：具体日期】报送材料。",
            &[],
            &rules(),
        );
        assert!(blockers.iter().any(|message| message.contains("待核实")));

        let vocabulary = vec![VocabularyEntry {
            category: VocabularyCategory::Unit,
            canonical: "某某省教育厅".into(),
            aliases: vec!["省教育厅".into()],
            ..Default::default()
        }];
        let blockers = mustfix_issues(
            &input,
            "# 关于测试的函\n\n省教育厅负责办理。",
            &vocabulary,
            &rules(),
        );
        assert!(
            blockers
                .iter()
                .any(|message| message.contains("非规范名称"))
        );

        let blockers = mustfix_issues(
            &input,
            "# 关于测试的函\n\n正文使用项目符号也只是风格建议。",
            &[],
            &rules(),
        );
        assert!(blockers.iter().all(|message| !message.contains("建议")));
    }

    /// 要素缺一大片、正文还留着待核实，PDF 照样排得出来——这种稿子必须能导出，
    /// 因为看版式、请人过目靠的就是这份 PDF。闸门只认「连 .tex 都写不出」。
    #[test]
    fn compile_gate_only_trips_on_an_empty_body() {
        let input = DraftInput::default();
        let blockers = compile_blocking_issues(
            &input,
            "# 关于测试的函\n\n请于【待核实：具体日期】报送材料。",
            &[],
            &rules(),
        );
        assert!(
            blockers.is_empty(),
            "缺要素不该挡编译，却挡下了：{blockers:?}"
        );

        // 反过来，正文为空连 .tex 都写不出，这一条必须拦住。
        let blockers = compile_blocking_issues(&input, "   \n", &[], &rules());
        assert_eq!(blockers, vec![EMPTY_BODY.to_string()]);
    }
}
