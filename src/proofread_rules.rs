//! 文档级校对规则：标题规范、文种越界、数字用法、附件一致性、成文日期。
//!
//! 与 `proofread` 的词表分工：那边是「见到这个词就报」，规则由用户增删；这边
//! 是「这份稿子作为某个文种，结构上有没有问题」，依赖文档要素，改不得也删不掉。
//! 两边都产出带 `span` 的 [`ProofNote`]，在审校抽屉里混排。
//!
//! 每条规则的取舍原则一样：**宁可漏报，不可误报**。公文写法各单位有差异，
//! 一条天天误报的规则会让人把整个校对关掉，那比没有这条规则更糟。所以拿不准的
//! 一律给「疑似」，只有确定无疑的才给「必错」。

use crate::export;
use crate::models::{DraftInput, TemplateKind};
use crate::proofread::{Level, ProofNote};
use regex::Regex;
use std::sync::OnceLock;

mod catalog;
mod context;
#[cfg(test)]
mod regression_tests;
pub(crate) use catalog::{RULES, set_style, style_enabled};
pub(crate) use context::{body_text, declared_kind};

/// 跑一遍全部文档级规则。
pub fn check(input: &DraftInput, markdown: &str) -> Vec<ProofNote> {
    check_with_config(input, markdown, &crate::models::ProofreadConfig::default())
}

pub(crate) fn check_with_config(
    input: &DraftInput,
    markdown: &str,
    config: &crate::models::ProofreadConfig,
) -> Vec<ProofNote> {
    let mut notes = Vec::new();
    // 先在原文上查引用写法，再把引用屏蔽掉：引用的名称与文号是事实，其他规则不该去改。
    check_citations(markdown, &mut notes);
    let masked = crate::document_reference::masked(markdown);
    let original = masked.as_ref();
    let body_text = body_text(original);
    let markdown = body_text.as_str();
    // 主文掩码保护附件、引文与非正文块，并在 [正文] 标记后恢复检查。
    let body = 0..markdown.len();
    let title = find_title(original);
    let actual = title.as_ref().and_then(|title| claimed_kind(&title.text));
    if let Some(title) = &title {
        check_title(input.kind, title, &mut notes);
    }
    check_document_kind(input.kind, actual, markdown, &body, &mut notes);
    check_numbers(markdown, &body, &mut notes);
    check_heading_numbers(original, &mut notes);
    check_attachments(original, markdown, &mut notes);
    if !matches!(
        input.kind,
        TemplateKind::ResearchReport | TemplateKind::PhoneRecord | TemplateKind::MeetingAgenda
    ) {
        check_honorifics(markdown, &body, &mut notes);
    }
    if !matches!(
        input.kind,
        TemplateKind::ResearchReport | TemplateKind::PhoneRecord | TemplateKind::MeetingAgenda
    ) {
        check_sentence_endings(original, &mut notes);
    }
    check_language_style(input.kind, actual, config, markdown, &mut notes);
    notes.sort_by(|a, b| a.span.start.cmp(&b.span.start).then(a.level.cmp(&b.level)));
    notes
}

/// 直接引语（“……”）在给定切片里的字节范围。引文里出现请示结语、相对日期、
/// 汉字年份都是在照录原文，不该拿本稿的规则去改。
fn quoted_spans(text: &str) -> Vec<std::ops::Range<usize>> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"[“「][^”」]*[”」]").expect("valid regex"));
    re.find_iter(text).map(|m| m.range()).collect()
}

/// 区间是否完全落在某一处引语里。
fn in_any(spans: &[std::ops::Range<usize>], start: usize, end: usize) -> bool {
    spans
        .iter()
        .any(|span| span.start <= start && end <= span.end)
}

/// 正文里的文档主标题及其字节范围。
struct Title {
    text: String,
    span: std::ops::Range<usize>,
}

fn find_title(markdown: &str) -> Option<Title> {
    let (_, range) = context::main_title(markdown)?;
    let source = markdown.get(range.clone())?;
    let trimmed = source.trim_start();
    let rest = trimmed.strip_prefix("# ")?.trim_end();
    let start = range.start + source.len() - trimmed.len() + 2;
    Some(Title {
        text: rest.to_owned(),
        span: start..start + rest.len(),
    })
}

/// 标题末尾自称的法定文种。取最长的那个才准（「通报」「通告」都以「通」开头）。
fn claimed_kind(text: &str) -> Option<&'static str> {
    context::claimed_kind(text)
}

fn note(
    id: &str,
    group: &str,
    level: Level,
    message: String,
    span: std::ops::Range<usize>,
) -> ProofNote {
    ProofNote {
        entry_id: id.to_string(),
        group: group.to_string(),
        level,
        message,
        span,
        replacement: None,
    }
}

/// 带一键改法的规则提示。给得出确定改法时才用——给不出就老实只提示，
/// 猜一个改法比不给更糟。
fn note_with_fix(
    id: &str,
    group: &str,
    level: Level,
    message: String,
    span: std::ops::Range<usize>,
    replacement: String,
) -> ProofNote {
    ProofNote {
        replacement: Some(replacement),
        ..note(id, group, level, message, span)
    }
}

// ── 公文引用 ────────────────────────────────────────────────────────────────

/// 正文里《名称》（文号）的括号写法：年份用六角括号、外层用全角括号、序号不编虚位。
/// 规范写法是确定的，给一键改法。
fn check_citations(markdown: &str, notes: &mut Vec<ProofNote>) {
    for citation in crate::document_reference::detect_numbered(markdown) {
        if citation.standard(markdown) {
            continue;
        }
        let text = citation.text();
        notes.push(note_with_fix(
            "RULE-CITATION-NUMBER",
            "公文引用",
            Level::MustFix,
            format!("公文引用的文号写法不规范，应为「{text}」"),
            citation.range,
            text,
        ));
    }
}

// ── 标题规范 ────────────────────────────────────────────────────────────────

/// 公文标题里允许出现的标点：书名号和引号。除此之外标题不加标点，尤其不加句号。
const TITLE_ALLOWED_PUNCT: [char; 6] = ['《', '》', '“', '”', '‘', '’'];

/// 各文种标题允许的结尾文种词。空表示不比较。
///
/// 只有模板与法定文种一一对应时才填：公函就是函，电话通知就是通知。呈批件
/// 和白头件只是版式，实际文种由标题决定；普通公文更是没有文种信息。把模板名
/// 当法定文种来卡标题，正是审核报告里那批误报的根子。
fn definite_suffixes(kind: TemplateKind) -> &'static [&'static str] {
    match kind {
        TemplateKind::OfficialLetter => &["函"],
        TemplateKind::PhoneNotice => &["通知"],
        _ => &[],
    }
}

/// 正式公文标题才套标题规则。研究报告题名、电话记录单标题、会议名称是事务
/// 材料或表单的标题，另有写法，不能拿公文标题规范去判。
fn title_rules_apply(kind: TemplateKind) -> bool {
    matches!(
        kind,
        TemplateKind::OfficialLetter
            | TemplateKind::PhoneNotice
            | TemplateKind::WhitePaper
            | TemplateKind::RedHeadApproval
            | TemplateKind::PlainDocument
    )
}

/// 标题长度上限（字）。二号小标宋在 A4 版心一行约 22 字，超过两行就该考虑
/// 精简或手工回行——回行要词意完整、呈梯形，程序做不了，只能提示。
const TITLE_LONG_CHARS: usize = 36;

fn check_title(kind: TemplateKind, title: &Title, notes: &mut Vec<ProofNote>) {
    if !title_rules_apply(kind)
        || (matches!(
            kind,
            TemplateKind::PlainDocument | TemplateKind::WhitePaper | TemplateKind::RedHeadApproval
        ) && claimed_kind(&title.text).is_none())
    {
        return;
    }
    let text = title.text.trim();

    // 标点。逐字找，报第一个就够，免得一个标题刷出一串提示。
    if let Some((index, ch)) = first_stray_punctuation(text) {
        let start = title.span.start + index;
        notes.push(note(
            "RULE-TITLE-PUNCT",
            "标题规范",
            Level::MustFix,
            format!("公文标题内不用标点（书名号、引号除外），这里有一个「{ch}」"),
            start..start + ch.len_utf8(),
        ));
    }

    // 文种一致。标题自称的文种与模板绑定的法定文种对不上，是最丢人的那类错。
    // 没有绑定文种的模板（呈批件、普通公文）不比较——「没有明确文种时不判冲突」。
    let allowed = definite_suffixes(kind);
    if !allowed.is_empty()
        && let Some(claimed) = claimed_kind(text)
        && !allowed.contains(&claimed)
    {
        notes.push(note(
            "RULE-TITLE-KIND",
            "标题规范",
            Level::MustFix,
            format!(
                "当前文种是{}，标题却以「{claimed}」结尾；改标题或改文种，二者必须一致",
                kind.label()
            ),
            title.span.clone(),
        ));
    }

    if text.chars().count() > TITLE_LONG_CHARS {
        notes.push(note(
            "RULE-TITLE-LONG",
            "标题规范",
            Level::Hint,
            format!(
                "标题 {} 字，超过 {TITLE_LONG_CHARS} 字；如需回行，应在词意完整处断开并排成梯形",
                text.chars().count()
            ),
            title.span.clone(),
        ));
    }
}

/// 标题里第一个不该出现的标点。书名号、引号内部的标点是名称的一部分，
/// 不在此列（「关于印发《某某工作：试行办法》的通知」不报）。
fn first_stray_punctuation(text: &str) -> Option<(usize, char)> {
    let mut depth = 0usize;
    for (index, ch) in text.char_indices() {
        match ch {
            '《' | '“' | '‘' => depth += 1,
            '》' | '”' | '’' => depth = depth.saturating_sub(1),
            _ if depth == 0 && is_punctuation(ch) && !TITLE_ALLOWED_PUNCT.contains(&ch) => {
                return Some((index, ch));
            }
            _ => {}
        }
    }
    None
}

fn is_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '，' | '。'
            | '、'
            | '；'
            | '：'
            | '？'
            | '！'
            | '…'
            | '—'
            | ','
            | '.'
            | ';'
            | ':'
            | '?'
            | '!'
            | '《'
            | '》'
            | '“'
            | '”'
            | '‘'
            | '’'
    )
}

// ── 文种越界 / 一文一事 ─────────────────────────────────────────────────────

/// 请示结语。出现它就意味着这份材料在向上级要一个答复。
pub(crate) fn request_closing() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?:妥否|当否|可否|是否妥当)[，,]?\s*请(?:批示|指示|示下|审批)|请予(?:批复|批准)|特此请示")
            .expect("valid regex")
    })
}

/// 明确的非请示性文种。只有标题自称这几种，才把请示结语判为「夹带请示」。
/// 「函」不在其中：审批函依法可以请求批准（《条例》第八条），不能一概排除。
fn is_non_request_kind(kind: &str) -> bool {
    matches!(
        kind,
        "报告" | "通知" | "通报" | "公告" | "通告" | "纪要" | "决定"
    )
}

fn check_document_kind(
    kind: TemplateKind,
    actual: Option<&'static str>,
    markdown: &str,
    body: &std::ops::Range<usize>,
    notes: &mut Vec<ProofNote>,
) {
    if matches!(
        kind,
        TemplateKind::ResearchReport | TemplateKind::PhoneRecord | TemplateKind::MeetingAgenda
    ) {
        return;
    }
    let Some(slice) = markdown.get(body.clone()) else {
        return;
    };
    // 引文里的请示结语是照录原文，不算本稿的请求。
    let quotes = quoted_spans(slice);
    let hits: Vec<(std::ops::Range<usize>, &str)> = request_closing()
        .find_iter(slice)
        .filter(|hit| {
            !in_any(&quotes, hit.start(), hit.end())
                && closing_is_ours(slice, hit.start(), hit.end())
        })
        .map(|hit| {
            (
                body.start + hit.start()..body.start + hit.end(),
                hit.as_str(),
            )
        })
        .collect();
    if hits.is_empty() {
        return;
    }
    // 仅明确为请示时核对重复结语；次数不能证明一文多事，未知文种不猜。
    let request_doc = actual == Some("请示");
    if request_doc {
        if hits.len() > 1 {
            let (span, _) = &hits[1];
            notes.push(note(
                "RULE-ONE-MATTER",
                "文种",
                Level::Suspect,
                format!(
                    "本稿有 {} 处请求性结语，请核对是否重复；仅凭结语次数不能判定一文多事",
                    hits.len()
                ),
                span.clone(),
            ));
        }
        return;
    }
    // 只有正文自报为非请示性文种时才判「夹带请示」。函可以商洽审批请求，
    // 电话记录是转述，文种不明时保守不报。
    if actual.is_some_and(is_non_request_kind) {
        for (span, text) in &hits {
            notes.push(note(
                "RULE-KIND-REQUEST",
                "文种",
                Level::MustFix,
                format!(
                    "{}中不得夹带请示事项，这里出现了请示结语「{text}」；需要上级答复的事项应另行请示",
                    actual.unwrap_or(kind.label())
                ),
                span.clone(),
            ));
        }
    }
}

/// 只把独立结语行视为本稿请求；转述、举例和正文中的片段不作必错判断。
pub(crate) fn closing_is_ours(text: &str, start: usize, end: usize) -> bool {
    let line_start = text[..start]
        .rfind(['\n', '。', '！', '？'])
        .map_or(0, |pos| {
            pos + text[pos..].chars().next().expect("句界").len_utf8()
        });
    let line_end = text[end..].find('\n').map_or(text.len(), |pos| end + pos);
    let before = text[line_start..start].trim();
    let after = text[end..line_end]
        .trim()
        .trim_matches(['。', '！', '!', '.']);
    matches!(before, "" | "以上意见" | "以上请示" | "以上事项") && after.is_empty()
}

// ── 数字用法 ────────────────────────────────────────────────────────────────

/// 正文里的汉字数字年份。成文日期用汉字数字是旧式写法，正文里一律用阿拉伯数字。
fn chinese_year() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[〇零一二三四五六七八九]{4}年").expect("valid regex"))
}

fn check_numbers(markdown: &str, body: &std::ops::Range<usize>, notes: &mut Vec<ProofNote>) {
    let Some(slice) = markdown.get(body.clone()) else {
        return;
    };
    // 引文与附件里的汉字年份是原文事实，不报。
    let quotes = quoted_spans(slice);
    for hit in chinese_year().find_iter(slice) {
        if in_any(&quotes, hit.start(), hit.end()) {
            continue;
        }
        notes.push(note(
            "RULE-NUM-YEAR",
            "数字用法",
            Level::Suspect,
            format!("正文里的年份宜用阿拉伯数字，这里是「{}」", hit.as_str()),
            body.start + hit.start()..body.start + hit.end(),
        ));
    }
}

// ── 层级序号 ────────────────────────────────────────────────────────────────

/// 手写的层级序号：一、→（一）→ 1. →（1）。
///
/// 功能区插入的标题是自动编号的，碰不到这条；但正文里手敲序号是常态，
/// 跳号、重号、错序全靠人眼查，最费神也最容易漏。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeqLevel {
    /// 一、
    First,
    /// （一）
    Second,
    /// 1.
    Third,
    /// （1）
    Fourth,
}

impl SeqLevel {
    fn depth(self) -> usize {
        match self {
            Self::First => 0,
            Self::Second => 1,
            Self::Third => 2,
            Self::Fourth => 3,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::First => "一、",
            Self::Second => "（一）",
            Self::Third => "1.",
            Self::Fourth => "（1）",
        }
    }
}

/// 认出行首的手写序号，返回层级与序号值。
fn parse_sequence(line: &str) -> Option<(SeqLevel, usize, usize)> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    // 「（一）」
    if let Some(rest) = trimmed.strip_prefix('（')
        && let Some(end) = rest.find('）')
    {
        let inner = &rest[..end];
        let width = indent + '（'.len_utf8() + inner.len() + '）'.len_utf8();
        if let Some(value) = chinese_to_number(inner) {
            return Some((SeqLevel::Second, value, width));
        }
        if let Ok(value) = inner.parse::<usize>() {
            return Some((SeqLevel::Fourth, value, width));
        }
        return None;
    }
    // 「一、」
    if let Some(end) = trimmed.find('、') {
        let inner = &trimmed[..end];
        if let Some(value) = chinese_to_number(inner) {
            return Some((
                SeqLevel::First,
                value,
                indent + inner.len() + '、'.len_utf8(),
            ));
        }
    }
    // 「1.」
    let digits: String = trimmed.chars().take_while(char::is_ascii_digit).collect();
    if !digits.is_empty()
        && trimmed[digits.len()..].starts_with('.')
        // 「1.5亿元」是小数不是第三层序号：点号后紧跟数字的不算。
        && trimmed[digits.len() + 1..]
            .chars()
            .next()
            .is_none_or(|ch| !ch.is_ascii_digit())
        && let Ok(value) = digits.parse::<usize>()
    {
        return Some((SeqLevel::Third, value, indent + digits.len() + 1));
    }
    None
}

fn chinese_to_number(text: &str) -> Option<usize> {
    const DIGITS: [char; 10] = ['〇', '一', '二', '三', '四', '五', '六', '七', '八', '九'];
    let chars: Vec<char> = text.chars().collect();
    let value = |ch: char| DIGITS.iter().position(|d| *d == ch);
    match chars.as_slice() {
        ['十'] => Some(10),
        [single] => value(*single),
        ['十', ones] => value(*ones).map(|ones| 10 + ones),
        [tens, '十'] => value(*tens).map(|tens| tens * 10),
        [tens, '十', ones] => Some(value(*tens)? * 10 + value(*ones)?),
        _ => None,
    }
}

fn check_heading_numbers(markdown: &str, notes: &mut Vec<ProofNote>) {
    // 每层记「上一个序号」；进入更浅的一层时，比它深的层全部清零重来。
    let mut last = [0usize; 4];
    let mut offset = 0usize;
    for line in markdown.split('\n') {
        let line_start = offset;
        offset += line.len() + 1;
        let trimmed = line.trim_start();
        // Markdown 标题由导出器自动编号，手写序号才是这条规则的对象。新标题、
        // 新区段（附件标记）意味着另一段列表，编号重新起算，跨章节不延续。
        if trimmed.starts_with('#') || trimmed.starts_with("<!--") {
            last = [0usize; 4];
            continue;
        }
        let Some((level, value, width)) = parse_sequence(line) else {
            continue;
        };
        let depth = level.depth();
        let previous = last[depth];
        let expected = previous + 1;
        if value != expected {
            let message = if value == previous {
                format!("序号重复：又一个「{}」，上一个同级序号也是它", value)
            } else if previous == 0 {
                format!("{}这一层从 {value} 开始，通常应当从 1 起编", level.label())
            } else {
                format!("序号不连续：上一个是 {previous}，这里跳到 {value}")
            };
            notes.push(note(
                "RULE-SEQ",
                "层级序号",
                Level::Suspect,
                message,
                line_start..line_start + width,
            ));
        }
        last[depth] = value;
        // 更深的层级重新开始计数。
        for deeper in last.iter_mut().skip(depth + 1) {
            *deeper = 0;
        }
    }
}

// ── 附件一致性 ──────────────────────────────────────────────────────────────

/// 正文里声明的附件项目数，如「附件3项」「附件共三件」。
///
/// 只认「项／件」这类项目量词：「份」是印制份数（一个附件印三份），拿它和
/// 内嵌附件种类数比较必然误报（见 docs/proofread-rule-audit.md 第三节）。
fn declared_item_count() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"附件[^。；\n]{0,6}?([0-9]+|[一二三四五六七八九十]+)\s*(?:项|件)")
            .expect("valid regex")
    })
}

/// 正文里对某份附件的明确序号引用，如「见附件2」「详见附件三」。
///
/// 必须带引用动词：裸的「附件3」多半是清单标题或数量说明，且要能在金额、
/// 单位词前止步（「附件10万元预算表」不是引用附件10）。
fn attachment_reference() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?:详见|参见|见|附)\s*附件\s*([0-9]+|[一二三四五六七八九十]+)")
            .expect("valid regex")
    })
}

fn parse_count(text: &str) -> Option<usize> {
    text.parse::<usize>()
        .ok()
        .or_else(|| chinese_to_number(text))
}

/// 数量词／单位词：序号后面紧跟这些，说明它其实是数量而不是附件序号。
fn is_quantity_unit(ch: char) -> bool {
    matches!(
        ch,
        '份' | '万'
            | '亿'
            | '元'
            | '个'
            | '项'
            | '件'
            | '名'
            | '次'
            | '条'
            | '页'
            | '年'
            | '月'
            | '日'
            | '%'
            | '％'
            | '张'
            | '台'
            | '套'
            | '辆'
            | '人'
            | '家'
            | '户'
            | '本'
            | '册'
            | '部'
            | '支'
            | '种'
    )
}

/// 正文里完整可核验的附件项目清单：「附件：」后紧跟的连续编号项。
fn declared_list_count(slice: &str) -> Option<(usize, std::ops::Range<usize>)> {
    let mut count = 0usize;
    let mut list_span = None;
    let mut in_list = false;
    let mut offset = 0usize;
    for line in slice.split('\n') {
        let line_start = offset;
        offset += line.len() + 1;
        let trimmed = line.trim();
        if !in_list {
            let Some(rest) = trimmed
                .strip_prefix("附件：")
                .or_else(|| trimmed.strip_prefix("附件:"))
            else {
                continue;
            };
            in_list = true;
            list_span = Some(line_start..line_start + line.len());
            if let Some((SeqLevel::Third, 1, _)) = parse_sequence(rest.trim()) {
                count += 1;
            } else if !rest.trim().is_empty() {
                // 「附件：情况表」「附件：3份」不是编号清单，不宣称有零项。
                return None;
            }
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        if let Some((SeqLevel::Third, number, _)) = parse_sequence(trimmed) {
            if number != count + 1 {
                return None;
            }
            count += 1;
            if let Some(span) = &mut list_span {
                span.end = line_start + line.len();
            }
            continue;
        }
        break;
    }
    list_span.filter(|_| count > 0).map(|span| (count, span))
}

fn check_attachments(markdown: &str, slice: &str, notes: &mut Vec<ProofNote>) {
    let blocks = export::parse_markdown(markdown);
    let actual = export::attachment_names(&blocks).len();

    let declared = declared_list_count(slice).or_else(|| {
        declared_item_count().captures(slice).and_then(|hit| {
            let value = hit.get(1).and_then(|m| parse_count(m.as_str()))?;
            let whole = hit.get(0)?;
            Some((value, whole.start()..whole.end()))
        })
    });
    if let Some((declared, span)) = declared
        && declared != actual
    {
        notes.push(note(
            "RULE-ATTACH-COUNT",
            "附件",
            Level::Suspect,
            format!(
                "正文说明「{}」，编辑区内嵌附件为 {actual} 项；请核对是否另行随附，内嵌项目是否齐全",
                markdown[span.clone()].trim()
            ),
            span,
        ));
    }

    // 引用的序号不能超过实际份数。收文单位一清点就会打电话过来。
    for hit in attachment_reference().captures_iter(slice) {
        let Some(number) = hit.get(1) else { continue };
        let Some(value) = parse_count(number.as_str()) else {
            continue;
        };
        let whole = hit.get(0).expect("整体匹配");
        let after = slice[whole.end()..].trim_start().chars().next();
        // 「附件3份」是份数、「附件10万元」是金额，都不是序号引用。
        if after.is_some_and(is_quantity_unit) || after.is_some_and(|ch| ch.is_ascii_digit()) {
            continue;
        }
        if value > actual {
            let span = whole.range();
            notes.push(note(
                "RULE-ATTACH-REF",
                "附件",
                Level::Suspect,
                format!("正文引用了「附件{value}」，编辑区只内嵌 {actual} 项附件；请核对是否另行随附或缺少内容"),
                span,
            ));
        }
    }
}

// ── 敬称与自称 ──────────────────────────────────────────────────────────────

/// 机关间敬称的两类硬错。
///
/// 这一类原本交给模型检查器，三轮实测三种滥用形态，整类撤了回来（见
/// `revise_model::TASKS` 里称谓那一项的注释）。但撤回来之后要说清楚**能做什么、
/// 不能做什么**：
///
/// 「平行文用『贵』、下行文用『你』」这条判断做不了——它要的是发文单位与主送
/// 单位之间的**隶属关系**，而这份数据应用里没有。`correspondence_scope` 只有
/// 内部/外部两档，说的是名称用法，不是上下级。硬猜等于满屏误报。
///
/// 能做的是两条不依赖隶属关系、而且恰恰是模型做不好的：
///
/// 1. **「贵」后面直接跟完整单位名多半是误用。** 规范写法是「贵局」「贵委」
///    「贵单位」；「贵市财政局」不是词。但纯字符串分不清「贵阳市财政局」这种
///    地名，也容易跨短语误配，所以只降为**疑似**：词库能确认完整机构名之前，
///    不拿它当必错（见 docs/proofread-rule-audit.md P1）。
/// 2. **同一篇里不能既称「贵局」又称「你局」。** 这是全篇一致性问题：单看一句
///    两种都对，只有通读全文才发现前后不一。模型逐句看，永远发现不了。
fn check_honorifics(markdown: &str, body: &std::ops::Range<usize>, notes: &mut Vec<ProofNote>) {
    let Some(slice) = markdown.get(body.clone()) else {
        return;
    };
    // 附件表单与直接引文里的称谓不是本稿行文，不判。
    let quotes = quoted_spans(slice);
    static ATTACHED: OnceLock<Regex> = OnceLock::new();
    // 「贵」与机构后缀之间夹着少量汉字，多半是完整单位名。中间夹了称谓词或
    // 动词的按跨短语匹配排除（「贵单位负责联系市财政局」）。
    let attached = ATTACHED.get_or_init(|| {
        Regex::new(r"贵([\p{Han}]{1,5})(?:委员会|管理局|分局|局|委|办|厅|处|院|校|中心)")
            .expect("敬称正则必须有效")
    });
    for hit in attached.captures_iter(slice) {
        let whole = hit.get(0).expect("整体匹配");
        if in_any(&quotes, whole.start(), whole.end()) {
            continue;
        }
        let middle = hit.get(1).expect("中间分组").as_str();
        if ["贵阳", "贵港", "贵溪", "贵定"]
            .iter()
            .any(|name| whole.as_str().starts_with(name))
        {
            continue;
        }
        if [
            "单位", "公司", "本", "我", "你", "负责", "联系", "请", "要求", "关于", "和", "与",
            "及", "并", "的", "了", "等", "各", "驻",
        ]
        .iter()
        .any(|word| middle.contains(word))
        {
            continue;
        }
        notes.push(note(
            "RULE-HONOR-ATTACHED",
            "称谓规范",
            Level::Suspect,
            format!(
                "疑似把敬称加在了完整单位名前面：「{}」。「贵」只能代指对方机关，应写「贵局」「贵委」「贵单位」，或直接写单位名称",
                whole.as_str()
            ),
            body.start + whole.start()..body.start + whole.end(),
        ));
    }

    // 同一个机构后缀上「贵」「你」并用。取第二次出现的位置报，让用户看到冲突。
    static PAIRED: OnceLock<Regex> = OnceLock::new();
    let paired = paired_honorific(&PAIRED);
    let mut seen_polite: Vec<&str> = Vec::new();
    let mut seen_plain: Vec<(&str, std::ops::Range<usize>)> = Vec::new();
    for hit in paired.captures_iter(slice) {
        let whole = hit.get(0).expect("整体匹配");
        if in_any(&quotes, whole.start(), whole.end()) {
            continue;
        }
        let suffix = hit.get(2).expect("后缀分组").as_str();
        let honorific = hit.get(1).expect("敬称分组").as_str();
        if honorific == "贵" {
            seen_polite.push(suffix);
        } else {
            seen_plain.push((suffix, body.start + whole.start()..body.start + whole.end()));
        }
    }
    for (suffix, span) in seen_plain {
        if !seen_polite.contains(&suffix) {
            continue;
        }
        notes.push(note(
            "RULE-HONOR-MIXED",
            "称谓规范",
            Level::Suspect,
            format!(
                "正文同时出现「贵{suffix}」与「你{suffix}」，请核对是否指向同一机关；不同对象可以使用不同称谓"
            ),
            span,
        ));
    }
}

fn paired_honorific(cell: &'static OnceLock<Regex>) -> &'static Regex {
    cell.get_or_init(|| {
        Regex::new(r"(贵|你)(委员会|管理局|分局|局|委|办|厅|处|院|校|中心|单位|公司)")
            .expect("敬称配对正则必须有效")
    })
}

// ── 句末标点 ────────────────────────────────────────────────────────────────

/// 正文段落末尾可以合法收尾的字符。
///
/// 右引号、右括号、右书名号都收进来：「……遵照执行。」这类结尾里句号在内层，
/// 外面是配对符号，不算缺标点。
const SENTENCE_ENDERS: [char; 14] = [
    '。', '！', '？', '；', '：', '…', '—', '」', '』', '”', '’', '）', '》', '】',
];

/// 正文段落句末缺标点。
///
/// 这一条也是从模型检查器撤回来的：实测它基本不报（0/3、1/3），而且有道理——
/// 一行没有句末标点，在公文里更可能是标题、附件名或落款，模型只看到一句话，
/// 没有上下文分辨不了。而**哪些行是正文段落，程序比模型清楚**。
///
/// 所以这里只查 `Paragraph`，且：跳过附件区（附件名本来就不带句号）、
/// 跳过短段落（十字以内多半是落款、署名或单独一行的标记）。宁可漏报。
fn check_sentence_endings(markdown: &str, notes: &mut Vec<ProofNote>) {
    let mut in_attachment = false;
    for located in export::parse_markdown_located(markdown) {
        match &located.block {
            export::MarkdownBlock::Marker(export::MarkdownSection::Attachment) => {
                in_attachment = true;
            }
            export::MarkdownBlock::Marker(export::MarkdownSection::Body) => {
                in_attachment = false;
            }
            export::MarkdownBlock::Paragraph(text) if !in_attachment => {
                let trimmed = text.trim_end();
                // 研究公式、图题、表格与 HTML 注释不按句子写，末尾没有句号是对的。
                if trimmed.starts_with("$$")
                    || trimmed.starts_with('$')
                    || trimmed.starts_with('\\')
                    || trimmed.starts_with('|')
                    || trimmed.starts_with("<!--")
                {
                    continue;
                }
                if trimmed.chars().count() < 10 {
                    continue;
                }
                let Some(last) = trimmed.chars().next_back() else {
                    continue;
                };
                if SENTENCE_ENDERS.contains(&last) {
                    continue;
                }
                // 位置要锚在**源码**上：`text` 是解析后的内容，与源码不等长
                // （行内标记、缩进都会差），拿它的长度去加偏移会错位。
                let Some(span) = last_char_span(markdown, &located.range) else {
                    continue;
                };
                // span 必须罩住最后那个字，不能是插入点那样的空范围——空范围
                // 锚不住，`RevisionSet` 会把整条建议丢掉，而且不报错
                // （见 `revision::Anchor`）。所以连着最后一个字一起替换。
                let last_char = &markdown[span.clone()];
                notes.push(note_with_fix(
                    "RULE-SENTENCE-END",
                    "标点规范",
                    // 疑似而非必错：粘进来的整篇公文可能带落款、引文，
                    // 那些结尾不带句号是对的。给改法但不进「采纳全部必错」。
                    Level::Suspect,
                    format!("段落末尾缺句末标点：「…{}」", tail(trimmed)),
                    span,
                    format!("{last_char}。"),
                ));
            }
            _ => {}
        }
    }
}

/// 段落在源码里最后一个非空字符的字节范围。
fn last_char_span(
    markdown: &str,
    range: &std::ops::Range<usize>,
) -> Option<std::ops::Range<usize>> {
    let slice = markdown.get(range.clone())?;
    let trimmed = slice.trim_end();
    let last = trimmed.chars().next_back()?;
    let end = range.start + trimmed.len();
    Some(end - last.len_utf8()..end)
}

/// 提示里只回显段末几个字，够定位就行。
fn tail(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let start = chars.len().saturating_sub(8);
    chars[start..].iter().collect()
}

// ── 语言层：语感规则 ────────────────────────────────────────────────────────
//
// 下面几条来自对上百万字真实公文的全量统计（gongwen-skill 项目），挑的是
// 模型初稿最常见、又能用程序数出来的偏离：破折号当停顿、冒号当揭晓、强制词
// 堆砌、「进一步」刷屏、开篇空表态、力度词用错对象。起草提示词的
// 「行文规范」一节要求模型别这么写，这里是复查——提示词只约束意图，兜不住。
//
// 全部只给「提示」或「疑似」，没有一条是「必错」：这些是风格偏离，不是错误。
// 语料里最有质感的稿子往往多项落在常见区间外，把风格判成错会逼人关掉校对。

/// 正文段落（不含附件、标题、表格）的源码范围。语感规则只看这些：标题和
/// 表格单元本来就不按句子写，附件多是表单和清单，统计进去全是噪音。
fn body_paragraph_ranges(markdown: &str) -> Vec<std::ops::Range<usize>> {
    let mut in_attachment = false;
    let mut ranges = Vec::new();
    for located in export::parse_markdown_located(markdown) {
        match &located.block {
            export::MarkdownBlock::Marker(export::MarkdownSection::Attachment) => {
                in_attachment = true;
            }
            export::MarkdownBlock::Marker(export::MarkdownSection::Body) => {
                in_attachment = false;
            }
            export::MarkdownBlock::Paragraph(_) | export::MarkdownBlock::OrderedListItem { .. }
                if !in_attachment =>
            {
                ranges.push(located.range.clone());
            }
            _ => {}
        }
    }
    ranges
}

/// 在正文段落里找一个正则的全部命中，位置换算回整篇源码。
fn find_in_body<'a>(
    markdown: &'a str,
    ranges: &[std::ops::Range<usize>],
    re: &Regex,
) -> Vec<(std::ops::Range<usize>, &'a str)> {
    let mut hits = Vec::new();
    for range in ranges {
        let Some(slice) = markdown.get(range.clone()) else {
            continue;
        };
        for m in re.find_iter(slice) {
            hits.push((range.start + m.start()..range.start + m.end(), m.as_str()));
        }
    }
    hits
}

/// 破折号全篇上限。语料实测：经验材料 22 篇正文共 5 个，党建短材料平均 0.3 个/篇。
/// 模型初稿偏好用它做停顿（「十件里九件不是大事——一堵墙、一次装修」），
/// 公文里几乎不出现。副标题里的破折号是合法的，所以只数正文段落。
const DASH_LIMIT: usize = 1;

/// 「必须／严禁」全篇上限。语料密度 0.10–0.96‰，一篇 3000 字通常 0–4 个；
/// 给 AI 的执行清单定的是 ≤5。强制词是稀缺资源，用多了就失效。
///
/// 只数这两个，不数「应当」「不得」：那两个是条例、办法和报送要求里的中性
/// 法律语体（「应当一并核实」「不得自行推测」），本仓库的规范样例函件一篇就
/// 用了三十多处。语料统计的是事务性材料，那边的配额搬到法定公文上会天天误报。
/// 「必须」「严禁」不一样——它们是加重语气，写多了才真的失效。
const FORCE_WORD_LIMIT: usize = 5;

/// 「进一步」全篇上限。它可用，但 2.5 次/篇之外就是在凑字。
const JINYIBU_LIMIT: usize = 3;

/// 开篇判定范围（字）。语料 226 篇里前 45 字含「高度重视」的只有 4 篇，
/// 且全是历史陈述而非表态。
const OPENING_CHARS: usize = 45;

fn check_language_style(
    kind: TemplateKind,
    actual: Option<&str>,
    config: &crate::models::ProofreadConfig,
    markdown: &str,
    notes: &mut Vec<ProofNote>,
) {
    let ranges = body_paragraph_ranges(markdown);
    if ranges.is_empty() {
        return;
    }
    if style_enabled(config, kind, "RULE-PUNCT-DASH") {
        check_dashes(markdown, &ranges, notes);
    }
    if style_enabled(config, kind, "RULE-FORCE-QUOTA") {
        check_force_words(markdown, &ranges, notes);
    }
    if style_enabled(config, kind, "RULE-WORD-JINYIBU") {
        check_jinyibu(markdown, &ranges, notes);
    }
    if style_enabled(config, kind, "RULE-OPEN-CLICHE") {
        check_opening(markdown, &ranges, notes);
    }
    if !matches!(
        kind,
        TemplateKind::ResearchReport | TemplateKind::PhoneRecord | TemplateKind::MeetingAgenda
    ) {
        check_tone_direction(kind, actual, markdown, &ranges, notes);
    }
}

fn check_dashes(markdown: &str, ranges: &[std::ops::Range<usize>], notes: &mut Vec<ProofNote>) {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new("——").expect("valid regex"));
    let hits = find_in_body(markdown, ranges, re);
    if hits.len() <= DASH_LIMIT {
        return;
    }
    // 只在第一个超限处报一次，带上总数。逐处报会刷屏，而用户要做的是通读
    // 一遍把停顿改成逗号、把转折断成句号，报一处就够定位。
    let (span, _) = &hits[DASH_LIMIT];
    notes.push(note(
        "RULE-PUNCT-DASH",
        "标点规范",
        Level::Hint,
        format!(
            "正文用了 {} 处破折号，达到所选风格提示阈值；请按表达需要核对，不是公文标点数量上限",
            hits.len()
        ),
        span.clone(),
    ));
}

/// 强制词计数。引号内二十字以内的内容不计——那多是自造概念（需求分
/// 「必须改」「可以缓」两类），计入会顶破配额。这是语料统计里踩过的坑。
fn check_force_words(
    markdown: &str,
    ranges: &[std::ops::Range<usize>],
    notes: &mut Vec<ProofNote>,
) {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"必须|严禁").expect("valid regex"));
    static QUOTED: OnceLock<Regex> = OnceLock::new();
    let quoted = QUOTED.get_or_init(|| Regex::new(r"[“「][^”」]{0,20}[”」]").expect("valid regex"));

    let mut hits: Vec<std::ops::Range<usize>> = Vec::new();
    for range in ranges {
        let Some(slice) = markdown.get(range.clone()) else {
            continue;
        };
        let quotes: Vec<std::ops::Range<usize>> =
            quoted.find_iter(slice).map(|m| m.range()).collect();
        for m in re.find_iter(slice) {
            if quotes
                .iter()
                .any(|q| q.start <= m.start() && m.end() <= q.end)
            {
                continue;
            }
            hits.push(range.start + m.start()..range.start + m.end());
        }
    }
    if hits.len() <= FORCE_WORD_LIMIT {
        return;
    }
    notes.push(note(
        "RULE-FORCE-QUOTA",
        "力度用词",
        Level::Hint,
        format!(
            "正文「必须／严禁」共 {} 处，超过所选风格提示阈值 {FORCE_WORD_LIMIT} 处；请结合篇幅、职责和条款核对，不必替换必要的要求",
            hits.len()
        ),
        hits[FORCE_WORD_LIMIT].clone(),
    ));
}

fn check_jinyibu(markdown: &str, ranges: &[std::ops::Range<usize>], notes: &mut Vec<ProofNote>) {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new("进一步").expect("valid regex"));
    let hits = find_in_body(markdown, ranges, re);
    if hits.len() <= JINYIBU_LIMIT {
        return;
    }
    let (span, _) = &hits[JINYIBU_LIMIT];
    notes.push(note(
        "RULE-WORD-JINYIBU",
        "套话虚词",
        Level::Hint,
        format!(
            "正文「进一步」出现 {} 次，超过所选风格提示阈值 {JINYIBU_LIMIT} 次；请结合篇幅核对是否冗余",
            hits.len()
        ),
        span.clone(),
    ));
}

/// 开篇空表态。只看第一个正文段落的前 45 字。
fn check_opening(markdown: &str, ranges: &[std::ops::Range<usize>], notes: &mut Vec<ProofNote>) {
    let Some(first) = ranges.first() else {
        return;
    };
    let Some(slice) = markdown.get(first.clone()) else {
        return;
    };
    let head_len: usize = slice.chars().take(OPENING_CHARS).map(char::len_utf8).sum();
    let head = &slice[..head_len];
    let Some(pos) = head.find("高度重视") else {
        return;
    };
    // 只报本机关自己的空表态。「党中央高度重视」「市委市政府历来高度重视」
    // 是历史陈述，不是本稿态度，报了就是误报（见审核报告 P2）。
    const SELF: [&str; 7] = ["我局", "我单位", "我办", "我委", "我中心", "本单位", "我们"];
    if !SELF.iter().any(|word| head.contains(word)) {
        return;
    }
    let start = first.start + pos;
    notes.push(note(
        "RULE-OPEN-CLICHE",
        "套话虚词",
        Level::Hint,
        "所选风格提示：开篇本机关的「高度重视」是否有具体措施支撑，请结合上下文核对；事实陈述可保留".into(),
        start..start + "高度重视".len(),
    ));
}

/// 力度词与行文方向。用词由权力关系决定：函是平行文，对方不是下属，
/// 「要求贵局」就是越权；请示是上行文，「要求市政府」更是。
///
/// 只认「命令式动词 + 对方称谓」紧挨着的形态，中间隔了字就不认——
/// 「要求各县（市、区）……并抄送贵局」不是在要求贵局。
fn check_tone_direction(
    kind: TemplateKind,
    actual: Option<&str>,
    markdown: &str,
    ranges: &[std::ops::Range<usize>],
    notes: &mut Vec<ProofNote>,
) {
    match kind {
        TemplateKind::OfficialLetter => {
            static RE: OnceLock<Regex> = OnceLock::new();
            let re = RE.get_or_init(|| {
                Regex::new(
                    r"(?:要求|责成|责令|督促|务必)(?:贵|你)(?:委员会|管理局|分局|局|委|办|厅|处|院|校|中心|单位|公司|方)",
                )
                .expect("valid regex")
            });
            for (span, text) in find_in_body(markdown, ranges, re)
                .into_iter()
                .filter(|(span, _)| direct_request_start(markdown, span.start))
            {
                notes.push(note(
                    "RULE-TONE-PARALLEL",
                    "力度用词",
                    Level::Suspect,
                    format!(
                        "「{text}」为直接要求语气，请核对行文对象和职权依据；平行商洽可用「请贵单位」「请予支持」"
                    ),
                    span,
                ));
            }
        }
        _ if matches!(actual, Some("请示" | "报告")) => {
            static RE: OnceLock<Regex> = OnceLock::new();
            let re = RE.get_or_init(|| {
                Regex::new(r"(?:要求|责成|责令)(?:上级|领导|[省市县区州](?:委|政府))")
                    .expect("valid regex")
            });
            for (span, text) in find_in_body(markdown, ranges, re)
                .into_iter()
                .filter(|(span, _)| direct_request_start(markdown, span.start))
            {
                notes.push(note(
                    "RULE-TONE-UPWARD",
                    "力度用词",
                    Level::Suspect,
                    format!("「{text}」对上级用了命令式。上行文提事项用「建议」「请」「恳请」"),
                    span,
                ));
            }
        }
        _ => {}
    }
}

fn direct_request_start(markdown: &str, start: usize) -> bool {
    let prefix = &markdown[..start];
    let boundary = prefix
        .rfind(['\n', '。', '！', '？', '；'])
        .map_or(0, |pos| {
            pos + prefix[pos..].chars().next().expect("边界字符").len_utf8()
        });
    matches!(
        prefix[boundary..].trim(),
        "" | "现" | "现请" | "我局" | "我办" | "本单位"
    )
}

// ── 成文日期 ────────────────────────────────────────────────────────────────

/// 只在正式导出时核对自动成文日期；手定签发日期和历史稿重导不视为过期。
///
/// `today` 由调用方传入，便于测试。
pub fn check_doc_date(input: &DraftInput, today: chrono::NaiveDate) -> Option<String> {
    if !input.date_is_auto
        || input.profile.letter_version == crate::models::LetterVersion::Preview
        || !matches!(
            input.kind,
            TemplateKind::OfficialLetter | TemplateKind::WhitePaper | TemplateKind::RedHeadApproval
        )
    {
        return None;
    }
    // `chinese_date_parts` 给的是三个字符串片段，年月日都可能是汉字数字。
    let (year, month, day) = export::chinese_date_parts(&input.date)?;
    let year = parse_count(year)? as i32;
    let month = parse_count(month)? as u32;
    let day = parse_count(day)? as u32;
    let date = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
    let days = (today - date).num_days();
    // 只报「过期」，不报「未来」：提前定成文日期是常规操作。
    if days <= 0 {
        return None;
    }
    Some(format!(
        "自动成文日期是 {year} 年 {month} 月 {day} 日，距今 {days} 天，请核对实际通过或签发日期；历史稿重导可保留原日期。"
    ))
}

#[cfg(test)]
mod tests {
    // ── 公文引用 ────────────────────────────────────────────────────────

    #[test]
    fn citation_brackets_get_a_one_click_fix_and_mask_other_rules() {
        let text = "根据《关于二〇二六年工作的通知》(某办函[2026]012号)要求，\
                    结合《实施方案》（某办发〔2026〕3号）落实。";
        let markdown = format!(
            "# 关于测试有关事项的函

{text}
"
        );
        let notes = check_all_with(&markdown);
        let hits = notes
            .iter()
            .filter(|note| note.entry_id == "RULE-CITATION-NUMBER")
            .collect::<Vec<_>>();
        assert_eq!(hits.len(), 1, "规范写法不报");
        assert_eq!(
            hits[0].replacement.as_deref(),
            Some("《关于二〇二六年工作的通知》（某办函〔2026〕12号）")
        );
        assert_eq!(
            &markdown[hits[0].span.clone()],
            "《关于二〇二六年工作的通知》(某办函[2026]012号)"
        );
        // 引用名称里的汉字年份是原文事实，不报年份写法。
        assert!(notes.iter().all(|note| note.entry_id != "RULE-NUM-YEAR"));
    }

    // ── 敬称与句末标点 ──────────────────────────────────────────────────

    #[test]
    fn honorific_attached_to_a_full_unit_name_is_flagged() {
        let notes = check_all("请贵市财政局于本月底前反馈意见。");
        let hit = notes
            .iter()
            .find(|note| note.entry_id == "RULE-HONOR-ATTACHED")
            .expect("「贵市财政局」应当报出来");
        // 判据不足以确认完整机构名，只给疑似，不再以必错提示。
        assert_eq!(hit.level, Level::Suspect);
    }

    #[test]
    fn a_phrase_that_merely_starts_with_gui_is_not_a_full_unit_name() {
        // 「贵单位负责联系市财政局」是跨短语匹配，不是「贵＋完整单位名」。
        let notes = check_all("请贵单位负责联系市财政局，尽快反馈。");
        assert!(
            !notes
                .iter()
                .any(|note| note.entry_id == "RULE-HONOR-ATTACHED"),
            "跨短语匹配不该报敬称问题：{:?}",
            notes.iter().map(|n| &n.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn plain_honorifics_are_not_flagged() {
        // 「贵局」「贵委」「贵单位」都是规范写法，一个都不许报——
        // 这条规则是从模型手里接过来的，接过来就不能重犯同样的过度适用。
        for text in [
            "请贵局于本月底前反馈意见，我办将及时汇总。",
            "现将有关情况函告贵委，请予支持并及时反馈。",
            "感谢贵单位长期以来的大力支持与密切配合。",
            "请贵办公室协助落实本次会议的会务保障工作。",
        ] {
            let notes = check_all(text);
            assert!(
                !notes
                    .iter()
                    .any(|note| note.entry_id == "RULE-HONOR-ATTACHED"),
                "「{text}」不该被报出敬称问题：{:?}",
                notes.iter().map(|n| &n.message).collect::<Vec<_>>()
            );
        }
    }

    /// 全篇一致性是规则的主场：单看每一句「贵局」和「你局」都对，
    /// 只有通读全文才发现前后不一。逐句看的模型永远发现不了。
    #[test]
    fn mixing_polite_and_plain_forms_for_one_office_is_flagged() {
        let notes = check_all("请贵局尽快反馈。\n\n另请你局同步抄送我办。");
        assert!(
            notes.iter().any(|note| note.entry_id == "RULE-HONOR-MIXED"),
            "同篇「贵局」与「你局」并用应当报出来"
        );
    }

    #[test]
    fn honorifics_in_attachments_and_quotations_are_not_flagged() {
        // 附件表单里的称谓、引文里的称谓都不是本稿行文。
        let markdown = "# 关于测试的函\n\n请贵局尽快反馈。\n\n<!-- [附件] -->\n\n# 反馈表\n\n贵市财政局意见栏。";
        let notes = check_all_with(markdown);
        assert!(
            !notes
                .iter()
                .any(|note| note.entry_id == "RULE-HONOR-ATTACHED"),
            "附件里的称谓不该报：{:?}",
            ids(&notes)
        );

        let quoted = check_all("来函称“请贵市财政局核实”，我局已转办。");
        assert!(!ids(&quoted).contains(&"RULE-HONOR-ATTACHED"));
    }

    #[test]
    fn using_only_one_form_throughout_is_fine() {
        for text in [
            "请贵局尽快反馈。\n\n另请贵局同步抄送我办。",
            "请你局尽快反馈。\n\n另请你局同步抄送我办。",
        ] {
            let notes = check_all(text);
            assert!(
                !notes.iter().any(|note| note.entry_id == "RULE-HONOR-MIXED"),
                "「{text}」前后一致，不该报"
            );
        }
    }

    #[test]
    fn a_body_paragraph_without_final_punctuation_is_flagged() {
        let markdown = "# 标题\n\n请各有关单位认真组织落实并确保按期完成\n";
        let notes = check_all_with(markdown);
        let hit = notes
            .iter()
            .find(|note| note.entry_id == "RULE-SENTENCE-END")
            .expect("段落缺句号应当报出来");
        assert_eq!(hit.level, Level::Suspect, "疑似档，不进「采纳全部必错」");
        // 位置必须罩住最后一个字，不能是空范围——空范围锚不住，
        // 整条建议会被 `RevisionSet` 无声丢掉。
        assert!(!hit.span.is_empty(), "改动区间不能为空");
        assert_eq!(&markdown[hit.span.clone()], "成");
        let replacement = hit.replacement.as_deref().expect("应当给出改法");
        assert_eq!(replacement, "成。");
        // 套用改法后必须正好补上句号，不多不少。
        let mut applied = markdown.to_string();
        applied.replace_range(hit.span.clone(), replacement);
        assert!(applied.contains("确保按期完成。"));
    }

    #[test]
    fn paragraphs_that_already_end_properly_are_left_alone() {
        for text in [
            "请各有关单位认真组织落实并确保按期完成。",
            "现将有关事项通知如下：",
            "会议审议通过了《关于加强财务管理工作的实施意见》",
            "有关单位应当按照要求执行（详见附件）",
        ] {
            let markdown = format!("# 标题\n\n{text}\n");
            let notes = check_all_with(&markdown);
            assert!(
                !notes
                    .iter()
                    .any(|note| note.entry_id == "RULE-SENTENCE-END"),
                "「{text}」结尾合法，不该报"
            );
        }
    }

    #[test]
    fn short_trailing_lines_are_not_treated_as_paragraphs() {
        // 落款、署名这类短行本来就不带句号，报了只会满屏误报。
        let markdown = "# 标题\n\n某某市财政局\n";
        let notes = check_all_with(markdown);
        assert!(
            !notes
                .iter()
                .any(|note| note.entry_id == "RULE-SENTENCE-END"),
            "短行不该按正文段落处理"
        );
    }

    fn check_all(text: &str) -> Vec<ProofNote> {
        check_all_with(&format!("# 关于测试有关事项的函\n\n{text}\n"))
    }

    fn check_all_with(markdown: &str) -> Vec<ProofNote> {
        let input = DraftInput::default();
        let mut config = crate::models::ProofreadConfig::default();
        for rule in RULES.iter().filter(|rule| rule.optional) {
            set_style(&mut config, input.kind, rule.id, true);
        }
        check_with_config(&input, markdown, &config)
    }

    use super::*;
    use crate::models::TemplateProfile;

    fn draft(kind: TemplateKind) -> DraftInput {
        DraftInput {
            kind,
            profile: TemplateProfile::for_kind(kind),
            ..Default::default()
        }
    }

    fn ids(notes: &[ProofNote]) -> Vec<&str> {
        notes.iter().map(|note| note.entry_id.as_str()).collect()
    }

    #[test]
    fn a_well_formed_letter_raises_nothing() {
        let input = draft(TemplateKind::OfficialLetter);
        let markdown = "# 关于报送2026年度情况的函\n\n现将有关事项函告如下。\n\n一、报送内容\n\n（一）事项清单\n\n二、报送方式\n\n特此函达。";
        let notes = check(&input, markdown);
        assert!(notes.is_empty(), "规范稿件不该报错：{:?}", ids(&notes));
    }

    #[test]
    fn a_title_with_a_full_stop_is_flagged() {
        let input = draft(TemplateKind::OfficialLetter);
        let notes = check(&input, "# 关于报送情况的函。\n\n正文。");
        assert!(ids(&notes).contains(&"RULE-TITLE-PUNCT"));
    }

    #[test]
    fn book_title_marks_are_allowed_in_a_title() {
        let input = draft(TemplateKind::OfficialLetter);
        let notes = check(&input, "# 关于印发《工作方案》的函\n\n正文。");
        assert!(!ids(&notes).contains(&"RULE-TITLE-PUNCT"));
    }

    #[test]
    fn a_title_claiming_the_wrong_kind_is_flagged() {
        // 选了公函，标题却写成「的通知」——最丢人的那类错。
        let input = draft(TemplateKind::OfficialLetter);
        let notes = check(&input, "# 关于报送情况的通知\n\n正文。");
        assert!(ids(&notes).contains(&"RULE-TITLE-KIND"));
    }

    #[test]
    fn the_kind_check_is_silent_when_the_suffix_matches() {
        let input = draft(TemplateKind::PhoneNotice);
        let notes = check(&input, "# 关于召开会议的通知\n\n正文。");
        assert!(!ids(&notes).contains(&"RULE-TITLE-KIND"));
    }

    #[test]
    fn templates_without_a_definite_kind_do_not_check_the_title_suffix() {
        // 呈批件、普通公文只是版式，没有绑定的法定文种，不能拿标题来卡。
        for kind in [
            TemplateKind::WhitePaper,
            TemplateKind::RedHeadApproval,
            TemplateKind::PlainDocument,
        ] {
            let notes = check(&draft(kind), "# 关于某项工作的通知\n\n正文。");
            assert!(
                !ids(&notes).contains(&"RULE-TITLE-KIND"),
                "{} 不该因标题文种报错",
                kind.label()
            );
        }
    }

    #[test]
    fn research_and_record_titles_are_not_checked_as_documents() {
        // 研究报告题名里的冒号、电话记录单标题都不是公文标题规范的对象。
        let research = check(
            &draft(TemplateKind::ResearchReport),
            "# 产业发展：趋势与对策\n\n## 第一章\n\n正文。",
        );
        assert!(
            !ids(&research).contains(&"RULE-TITLE-PUNCT"),
            "研究报告题名不该报公文标点：{:?}",
            ids(&research)
        );
        let record = check(
            &draft(TemplateKind::PhoneRecord),
            "# 电话记录单（重要）\n\n来电内容。",
        );
        assert!(!ids(&record).contains(&"RULE-TITLE-PUNCT"));
    }

    #[test]
    fn punctuation_inside_book_title_marks_is_not_a_title_error() {
        let input = draft(TemplateKind::OfficialLetter);
        let notes = check(&input, "# 关于印发《某某工作：试行办法》的函\n\n正文。");
        assert!(!ids(&notes).contains(&"RULE-TITLE-PUNCT"));
    }

    #[test]
    fn a_report_carrying_a_request_closing_is_flagged() {
        // 报告不得夹带请示事项；文种由标题认出来。
        let input = draft(TemplateKind::PlainDocument);
        let notes = check(
            &input,
            "# 关于报送情况的报告\n\n有关情况如上。\n\n妥否，请批示。",
        );
        assert!(ids(&notes).contains(&"RULE-KIND-REQUEST"));
    }

    #[test]
    fn an_approval_letter_may_request_a_reply() {
        // 审批函依法可以请求批准，不能一概排除。
        let input = draft(TemplateKind::OfficialLetter);
        let notes = check(
            &input,
            "# 关于核准有关事项的函\n\n有关情况如上。\n\n请予批复。",
        );
        assert!(
            !ids(&notes).contains(&"RULE-KIND-REQUEST"),
            "函的审批请求不该报：{:?}",
            ids(&notes)
        );
    }

    #[test]
    fn a_request_closing_in_a_quotation_is_not_this_drafts_request() {
        // 报告里照录来函原文，不是本稿在请示。
        let input = draft(TemplateKind::PlainDocument);
        let notes = check(
            &input,
            "# 关于报送情况的报告\n\n来函称“妥否，请批示”，我局已按要求办理。",
        );
        assert!(!ids(&notes).contains(&"RULE-KIND-REQUEST"));
    }

    #[test]
    fn a_request_document_may_carry_one_request_closing() {
        let input = draft(TemplateKind::WhitePaper);
        let notes = check(
            &input,
            "# 关于经费的请示\n\n有关情况如上。\n\n妥否，请指示。",
        );
        assert!(!ids(&notes).contains(&"RULE-KIND-REQUEST"));
        assert!(!ids(&notes).contains(&"RULE-ONE-MATTER"));
    }

    #[test]
    fn two_request_closings_suggest_more_than_one_matter() {
        let input = draft(TemplateKind::WhitePaper);
        let notes = check(
            &input,
            "# 关于经费的请示\n\n第一件事。妥否，请指示。\n\n第二件事。当否，请批示。",
        );
        assert!(ids(&notes).contains(&"RULE-ONE-MATTER"));
    }

    #[test]
    fn a_chinese_year_in_the_body_is_flagged() {
        let input = draft(TemplateKind::PlainDocument);
        let notes = check(&input, "# 情况说明\n\n二〇二六年的工作已经完成。");
        assert!(ids(&notes).contains(&"RULE-NUM-YEAR"));
    }

    #[test]
    fn a_skipped_sequence_number_is_caught() {
        let input = draft(TemplateKind::PlainDocument);
        let notes = check(&input, "# 情况说明\n\n一、第一项\n\n三、第三项");
        assert!(
            ids(&notes).contains(&"RULE-SEQ"),
            "应报跳号：{:?}",
            ids(&notes)
        );
    }

    #[test]
    fn a_repeated_sequence_number_is_caught() {
        let input = draft(TemplateKind::PlainDocument);
        let notes = check(&input, "# 情况说明\n\n一、第一项\n\n一、又一项");
        assert!(ids(&notes).contains(&"RULE-SEQ"));
    }

    #[test]
    fn nested_levels_restart_independently() {
        // 二级序号在每个一级下面都从（一）重新开始，这是正常的，不能报。
        let input = draft(TemplateKind::PlainDocument);
        let markdown = "# 情况说明\n\n一、甲\n\n（一）甲一\n\n（二）甲二\n\n二、乙\n\n（一）乙一\n\n（二）乙二";
        let notes = check(&input, markdown);
        assert!(
            !ids(&notes).contains(&"RULE-SEQ"),
            "误报：{:?}",
            ids(&notes)
        );
    }

    #[test]
    fn a_decimal_is_not_a_third_level_number() {
        // 「1.5亿元」是小数，不是第三层序号。
        let input = draft(TemplateKind::PlainDocument);
        let notes = check(&input, "# 情况说明\n\n一、总体情况\n\n1.5亿元投资已到位。");
        assert!(!ids(&notes).contains(&"RULE-SEQ"));
    }

    #[test]
    fn attachment_numbering_restarts_after_the_marker() {
        // 正文「一、二、」后附件从「一、」重新编号，不算跳号。
        let input = draft(TemplateKind::OfficialLetter);
        let markdown = "# 关于报送情况的函\n\n一、第一项\n\n二、第二项\n\n<!-- [附件] -->\n\n# 情况表\n\n一、附件第一项\n\n二、附件第二项";
        let notes = check(&input, markdown);
        assert!(
            !ids(&notes).contains(&"RULE-SEQ"),
            "附件重新编号不该报：{:?}",
            ids(&notes)
        );
    }

    #[test]
    fn numbered_lists_restart_under_each_heading() {
        // 不同章节的清单各从 1 开始。
        let input = draft(TemplateKind::PlainDocument);
        let markdown = "# 情况说明\n\n## 甲\n\n1. 甲一\n\n2. 甲二\n\n## 乙\n\n1. 乙一\n\n2. 乙二";
        let notes = check(&input, markdown);
        assert!(!ids(&notes).contains(&"RULE-SEQ"));
    }

    #[test]
    fn a_declared_attachment_count_must_match_reality() {
        let input = draft(TemplateKind::OfficialLetter);
        let markdown = "# 关于报送情况的函\n\n随文报送附件3件。\n\n<!-- [附件] -->\n\n# 情况表";
        let notes = check(&input, markdown);
        assert!(ids(&notes).contains(&"RULE-ATTACH-COUNT"));
    }

    #[test]
    fn a_matching_attachment_count_is_silent() {
        let input = draft(TemplateKind::OfficialLetter);
        let markdown = "# 关于报送情况的函\n\n随文报送附件2件。\n\n<!-- [附件] -->\n\n# 情况表\n\n<!-- [附件] -->\n\n# 明细表";
        let notes = check(&input, markdown);
        assert!(!ids(&notes).contains(&"RULE-ATTACH-COUNT"));
    }

    #[test]
    fn printed_copies_are_not_compared_with_attachment_kinds() {
        // 一个附件印三份，正文「附件：3份」说的是份数，不是附件种类数。
        let input = draft(TemplateKind::OfficialLetter);
        let markdown = "# 关于报送情况的函\n\n随文报送附件3份。\n\n<!-- [附件] -->\n\n# 情况表";
        let notes = check(&input, markdown);
        assert!(
            !ids(&notes).contains(&"RULE-ATTACH-COUNT"),
            "印制份数不该当作附件种类数：{:?}",
            ids(&notes)
        );
    }

    #[test]
    fn an_itemized_attachment_list_is_counted() {
        // 完整编号清单才拿来比较：清单两项、实际只标一份。
        let input = draft(TemplateKind::OfficialLetter);
        let mismatched =
            "# 关于报送情况的函\n\n附件：1. 统计表\n2. 明细表\n\n<!-- [附件] -->\n\n# 情况表";
        let notes = check(&input, mismatched);
        assert!(
            ids(&notes).contains(&"RULE-ATTACH-COUNT"),
            "清单 2 项与实际 1 份不符：{:?}",
            ids(&notes)
        );
        let matched = "# 关于报送情况的函\n\n附件：1. 统计表\n\n<!-- [附件] -->\n\n# 情况表";
        let notes = check(&input, matched);
        assert!(!ids(&notes).contains(&"RULE-ATTACH-COUNT"));
    }

    #[test]
    fn an_amount_is_not_an_attachment_reference() {
        // 「附件10万元预算表」是金额，不是引用附件10。
        let input = draft(TemplateKind::OfficialLetter);
        let markdown =
            "# 关于报送情况的函\n\n详见附件10万元预算表。\n\n<!-- [附件] -->\n\n# 情况表";
        let notes = check(&input, markdown);
        assert!(!ids(&notes).contains(&"RULE-ATTACH-REF"));
    }

    #[test]
    fn referencing_a_nonexistent_attachment_is_flagged() {
        let input = draft(TemplateKind::OfficialLetter);
        let markdown = "# 关于报送情况的函\n\n详见附件3。\n\n<!-- [附件] -->\n\n# 情况表";
        let notes = check(&input, markdown);
        assert!(ids(&notes).contains(&"RULE-ATTACH-REF"));
    }

    #[test]
    fn a_stale_doc_date_is_reported_only_when_it_is_in_the_past() {
        let mut input = draft(TemplateKind::OfficialLetter);
        input.date = "2026年8月10日".into();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 8, 16).expect("日期");
        let message = check_doc_date(&input, today).expect("应当报出");
        assert!(message.contains("6 天"));

        // 同一天不报。
        let today = chrono::NaiveDate::from_ymd_opt(2026, 8, 10).expect("日期");
        assert!(check_doc_date(&input, today).is_none());
        // 提前定日期是常规操作，不报。
        let today = chrono::NaiveDate::from_ymd_opt(2026, 8, 1).expect("日期");
        assert!(check_doc_date(&input, today).is_none());
    }

    #[test]
    fn spans_point_at_the_offending_text() {
        // 提示能不能点着跳过去，全看 span 准不准。
        let input = draft(TemplateKind::PlainDocument);
        let markdown = "# 关于报送情况的报告\n\n正文。\n\n妥否，请批示。";
        let notes = check(&input, markdown);
        let hit = notes
            .iter()
            .find(|note| note.entry_id == "RULE-KIND-REQUEST")
            .expect("应当报出");
        assert_eq!(&markdown[hit.span.clone()], "妥否，请批示");
    }

    // ── 语言层规则 ──────────────────────────────────────────────────────

    #[test]
    fn a_single_dash_is_fine_but_two_are_reported_once() {
        let one = check_all("这项工作——也就是排查，已经完成。");
        assert!(!ids(&one).contains(&"RULE-PUNCT-DASH"), "一处破折号不该报");

        let markdown = "# 关于测试的函\n\n第一处——停顿。\n\n第二处——又停顿。\n\n第三处——再停。\n";
        let notes = check_all_with(markdown);
        let hits: Vec<_> = notes
            .iter()
            .filter(|n| n.entry_id == "RULE-PUNCT-DASH")
            .collect();
        assert_eq!(hits.len(), 1, "超限只报一次：{:?}", ids(&notes));
        assert!(hits[0].message.contains("3 处"));
        assert_eq!(&markdown[hits[0].span.clone()], "——");
        // 报在第二处，而不是第一处：第一处是允许的。
        assert!(markdown[..hits[0].span.start].contains("第二处"));
    }

    #[test]
    fn dashes_in_headings_and_attachments_do_not_count() {
        // 副标题里的破折号是合法的，附件多是表单，都不算。
        let markdown = "# 关于测试的函\n\n## 一、总体要求——统一思想\n\n正文。\n\n<!-- [附件] -->\n# 附件标题\n\n甲——乙——丙——丁。\n";
        let notes = check_all_with(markdown);
        assert!(
            !ids(&notes).contains(&"RULE-PUNCT-DASH"),
            "{:?}",
            ids(&notes)
        );
    }

    #[test]
    fn colon_enumeration_is_not_flagged() {
        // GB/T 15834 允许用冒号提示下文；「主要任务包括：调查、评估和整改。」
        // 是正常列举，不再当作必改（见审核报告 P2）。
        for text in [
            "主要做法是：一手抓排查，一手抓整改。",
            "主要任务包括：调查、评估和整改。",
            "现将有关事项通知如下：",
            "他说：“这个办法好。”",
        ] {
            let notes = check_all(text);
            assert!(
                !ids(&notes).contains(&"RULE-PUNCT-COLON"),
                "「{text}」不该报冒号问题"
            );
        }
    }

    #[test]
    fn force_words_are_reported_past_the_quota_excluding_quotes_and_legal_modals() {
        let five = "各单位必须落实。严禁推诿。严禁弄虚作假。必须到位。必须按期。";
        assert!(
            !ids(&check_all(five)).contains(&"RULE-FORCE-QUOTA"),
            "五处以内不报"
        );

        let six = format!("{five}必须抓紧。");
        let notes = check_all(&six);
        let hit = notes
            .iter()
            .find(|n| n.entry_id == "RULE-FORCE-QUOTA")
            .expect("第六处应当报出");
        assert!(hit.message.contains("6 处"), "{}", hit.message);

        // 引号内的自造概念不计；「应当」「不得」是法律语体的中性用词，也不计。
        let excluded =
            format!("{five}需求分“必须改”和“可以缓”两类。应当核实，不得推测，不得填报。");
        assert!(
            !ids(&check_all(&excluded)).contains(&"RULE-FORCE-QUOTA"),
            "排除项不该计入"
        );
    }

    #[test]
    fn jinyibu_is_reported_on_the_fourth_use() {
        let three = "进一步加强。进一步完善。进一步提升。";
        assert!(!ids(&check_all(three)).contains(&"RULE-WORD-JINYIBU"));
        let notes = check_all(&format!("{three}进一步落实。"));
        assert!(
            ids(&notes).contains(&"RULE-WORD-JINYIBU"),
            "{:?}",
            ids(&notes)
        );
    }

    #[test]
    fn opening_with_gaodu_zhongshi_is_reported_only_in_the_first_paragraph() {
        let notes = check_all("我局高度重视此项工作，现将情况报告如下。");
        assert!(
            ids(&notes).contains(&"RULE-OPEN-CLICHE"),
            "{:?}",
            ids(&notes)
        );

        let later = check_all(
            "现将情况报告如下。

各单位要高度重视，抓好落实。",
        );
        assert!(!ids(&later).contains(&"RULE-OPEN-CLICHE"), "只看开篇");
    }

    #[test]
    fn a_historical_statement_is_not_an_empty_opening_stance() {
        // 「党中央高度重视」是历史陈述，不是本稿空表态。
        let notes = check_all("党中央高度重视科技创新工作，作出了一系列重大部署。");
        assert!(
            !ids(&notes).contains(&"RULE-OPEN-CLICHE"),
            "历史陈述不该报：{:?}",
            ids(&notes)
        );
    }

    #[test]
    fn a_letter_must_not_order_its_peer_around() {
        let input = draft(TemplateKind::OfficialLetter);
        let markdown = "# 关于协助核查的函

要求贵局于本月底前反馈。
";
        let notes = check(&input, markdown);
        let hit = notes
            .iter()
            .find(|n| n.entry_id == "RULE-TONE-PARALLEL")
            .expect("应当报出");
        assert_eq!(&markdown[hit.span.clone()], "要求贵局");

        // 隔了字就不认：「要求各县……抄送贵局」不是在要求贵局。
        let indirect = check(
            &input,
            "# 关于协助核查的函

要求各县抓紧办理并抄送贵局。
",
        );
        assert!(!ids(&indirect).contains(&"RULE-TONE-PARALLEL"));

        // 通知是下行文，「要求你局」是正常语气。
        let notice = check(
            &draft(TemplateKind::PhoneNotice),
            "# 关于报送材料的通知

要求你局按期报送。
",
        );
        assert!(!ids(&notice).contains(&"RULE-TONE-PARALLEL"));
    }

    #[test]
    fn a_request_must_not_order_its_superior_around() {
        let input = draft(TemplateKind::RedHeadApproval);
        let markdown = "# 关于申请经费的请示

要求市政府尽快批复。

妥否，请批示。
";
        let notes = check(&input, markdown);
        let hit = notes
            .iter()
            .find(|n| n.entry_id == "RULE-TONE-UPWARD")
            .expect("应当报出");
        assert_eq!(&markdown[hit.span.clone()], "要求市政府");
    }

    #[test]
    fn style_rules_never_reach_must_fix() {
        // 风格偏离不是错误，进了「必错」就会被「采纳全部必错」批量处理。
        let markdown = "# 关于测试的函\n\n我局高度重视——办法是：必须、必须、严禁、必须、严禁、必须进一步、进一步、进一步、进一步。\n\n甲——乙。\n";
        for note in check_all_with(markdown) {
            if note.group == "标点规范" && note.entry_id == "RULE-SENTENCE-END" {
                continue;
            }
            if note.entry_id.starts_with("RULE-PUNCT-")
                || note.entry_id.starts_with("RULE-FORCE-")
                || note.entry_id.starts_with("RULE-WORD-")
                || note.entry_id.starts_with("RULE-OPEN-")
                || note.entry_id.starts_with("RULE-TONE-")
            {
                assert_ne!(note.level, Level::MustFix, "{} 不该是必错", note.entry_id);
            }
        }
    }
}

#[cfg(test)]
mod sample_tests {
    use super::*;
    use crate::models::TemplateProfile;

    /// 仓库里那份规范样例公文，一条规则都不该命中。
    /// 将来谁把某条规则放宽过头，这条会先炸。
    #[test]
    fn the_sample_letter_trips_no_rule() {
        let sample = include_str!("../examples/标准带附件函件测试样例.md");
        let input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            profile: TemplateProfile::for_kind(TemplateKind::OfficialLetter),
            ..Default::default()
        };
        let notes = check(&input, sample);
        assert!(
            notes.is_empty(),
            "规范公文被误报：{:?}",
            notes
                .iter()
                .map(|note| (&note.entry_id, &note.message))
                .collect::<Vec<_>>()
        );
    }
}
