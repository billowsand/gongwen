//! 文档级校对规则：标题规范、文种越界、数字用法、附件一致性、成文日期。
//!
//! 与 `proofread` 的词表分工：那边是「见到这个词就报」，规则由用户增删；这边
//! 是「这份稿子作为某个文种，结构上有没有问题」，依赖公文要素，改不得也删不掉。
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

/// 跑一遍全部文档级规则。
pub fn check(input: &DraftInput, markdown: &str) -> Vec<ProofNote> {
    let mut notes = Vec::new();
    if let Some(title) = find_title(markdown) {
        check_title(input.kind, &title, &mut notes);
    }
    check_document_kind(input.kind, markdown, &mut notes);
    check_numbers(markdown, &mut notes);
    check_heading_numbers(markdown, &mut notes);
    check_attachments(markdown, &mut notes);
    check_honorifics(markdown, &mut notes);
    check_sentence_endings(markdown, &mut notes);
    notes.sort_by(|a, b| a.span.start.cmp(&b.span.start).then(a.level.cmp(&b.level)));
    notes
}

/// 正文里的文档主标题及其字节范围。
struct Title {
    text: String,
    span: std::ops::Range<usize>,
}

fn find_title(markdown: &str) -> Option<Title> {
    let mut offset = 0usize;
    for line in markdown.split('\n') {
        let line_start = offset;
        offset += line.len() + 1;
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("# ") else {
            continue;
        };
        if rest.trim().is_empty() {
            continue;
        }
        // 只取正文主标题；附件标题也是 `# `，但它在附件标记之后，这里取第一个即可。
        let indent = line.len() - trimmed.len();
        let start = line_start + indent + "# ".len();
        return Some(Title {
            text: rest.to_string(),
            span: start..start + rest.len(),
        });
    }
    None
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

// ── 标题规范 ────────────────────────────────────────────────────────────────

/// 公文标题里允许出现的标点：书名号和引号。除此之外标题不加标点，尤其不加句号。
const TITLE_ALLOWED_PUNCT: [char; 6] = ['《', '》', '“', '”', '‘', '’'];

/// 各文种标题允许的结尾文种词。空表示不限。
fn allowed_suffixes(kind: TemplateKind) -> &'static [&'static str] {
    match kind {
        TemplateKind::OfficialLetter => &["函"],
        TemplateKind::PhoneNotice => &["通知"],
        // 呈批件可以是请示、报告、意见、方案等，不好收窄。
        TemplateKind::WhitePaper | TemplateKind::RedHeadApproval => {
            &["请示", "报告", "意见", "方案", "建议", "说明"]
        }
        TemplateKind::PlainDocument | TemplateKind::MeetingAgenda => &[],
    }
}

/// 认得出来的文种词。标题以其中之一结尾，就能判断它自称是什么文种。
const KNOWN_SUFFIXES: [&str; 12] = [
    "函", "通知", "通报", "报告", "请示", "批复", "意见", "决定", "纪要", "公告", "通告", "方案",
];

/// 标题长度上限（字）。二号小标宋在 A4 版心一行约 22 字，超过两行就该考虑
/// 精简或手工回行——回行要词意完整、呈梯形，程序做不了，只能提示。
const TITLE_LONG_CHARS: usize = 36;

fn check_title(kind: TemplateKind, title: &Title, notes: &mut Vec<ProofNote>) {
    let text = title.text.trim();

    // 标点。逐字找，报第一个就够，免得一个标题刷出一串提示。
    if let Some((index, ch)) = text
        .char_indices()
        .find(|(_, ch)| is_punctuation(*ch) && !TITLE_ALLOWED_PUNCT.contains(ch))
    {
        let start = title.span.start + index;
        notes.push(note(
            "RULE-TITLE-PUNCT",
            "标题规范",
            Level::MustFix,
            format!("公文标题内不用标点（书名号、引号除外），这里有一个「{ch}」"),
            start..start + ch.len_utf8(),
        ));
    }

    // 「关于」。会议议程的标题是会议名称，不适用。
    if kind != TemplateKind::MeetingAgenda && !text.contains("关于") {
        notes.push(note(
            "RULE-TITLE-GUANYU",
            "标题规范",
            Level::Suspect,
            "公文标题的常规结构是「发文机关＋关于＋事由＋文种」，这里没有「关于」".into(),
            title.span.clone(),
        ));
    }

    // 文种一致。标题自称的文种与所选模板对不上，是最丢人的那类错。
    let allowed = allowed_suffixes(kind);
    if !allowed.is_empty()
        && let Some(claimed) = KNOWN_SUFFIXES
            .iter()
            .filter(|suffix| text.ends_with(*suffix))
            // 「通报」「通告」都以「通」开头，取最长的那个才准。
            .max_by_key(|suffix| suffix.len())
        && !allowed.contains(claimed)
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
fn request_closing() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(妥否|当否|可否|是否妥当)[，,]?\s*请(批示|指示|示下|审批)|请予批复")
            .expect("valid regex")
    })
}

fn check_document_kind(kind: TemplateKind, markdown: &str, notes: &mut Vec<ProofNote>) {
    let hits: Vec<_> = request_closing().find_iter(markdown).collect();
    if hits.is_empty() {
        return;
    }
    match kind {
        // 请示类：一份只该请示一件事。出现多处请示结语，多半是把几件事并成了一篇。
        TemplateKind::WhitePaper | TemplateKind::RedHeadApproval => {
            if hits.len() > 1 {
                notes.push(note(
                    "RULE-ONE-MATTER",
                    "文种",
                    Level::Suspect,
                    format!(
                        "全文有 {} 处请示结语，疑似一文多事；请示应当一文一事，另一件请另行行文",
                        hits.len()
                    ),
                    hits[1].range(),
                ));
            }
        }
        // 其余文种不该夹带请示事项。
        _ => {
            for hit in &hits {
                notes.push(note(
                    "RULE-KIND-REQUEST",
                    "文种",
                    Level::MustFix,
                    format!(
                        "{}中不得夹带请示事项，这里出现了请示结语「{}」；需要上级答复的事项应另行请示",
                        kind.label(),
                        hit.as_str()
                    ),
                    hit.range(),
                ));
            }
        }
    }
}

// ── 数字用法 ────────────────────────────────────────────────────────────────

/// 正文里的汉字数字年份。成文日期用汉字数字是旧式写法，正文里一律用阿拉伯数字。
fn chinese_year() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[〇零一二三四五六七八九]{4}年").expect("valid regex"))
}

fn check_numbers(markdown: &str, notes: &mut Vec<ProofNote>) {
    for hit in chinese_year().find_iter(markdown) {
        notes.push(note(
            "RULE-NUM-YEAR",
            "数字用法",
            Level::Suspect,
            format!("正文里的年份宜用阿拉伯数字，这里是「{}」", hit.as_str()),
            hit.range(),
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
        // Markdown 标题由导出器自动编号，手写序号才是这条规则的对象。
        if line.trim_start().starts_with('#') {
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

/// 正文里声明的附件份数，如「附件：3份」「附件共三份」。
fn declared_count() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"附件[^。；\n]{0,4}?([0-9]+|[一二三四五六七八九十]+)\s*份")
            .expect("valid regex")
    })
}

/// 正文里对某份附件的引用，如「见附件2」「详见附件三」。
fn attachment_reference() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"附件\s*([0-9]+|[一二三四五六七八九十]+)").expect("valid regex"))
}

fn parse_count(text: &str) -> Option<usize> {
    text.parse::<usize>()
        .ok()
        .or_else(|| chinese_to_number(text))
}

fn check_attachments(markdown: &str, notes: &mut Vec<ProofNote>) {
    let blocks = export::parse_markdown(markdown);
    let actual = export::attachment_names(&blocks).len();

    if let Some(hit) = declared_count().captures(markdown)
        && let Some(declared) = hit.get(1).and_then(|m| parse_count(m.as_str()))
        && declared != actual
    {
        let whole = hit.get(0).expect("整体匹配");
        notes.push(note(
            "RULE-ATTACH-COUNT",
            "附件",
            Level::MustFix,
            format!("正文说「{}」，但实际只标了 {actual} 份附件", whole.as_str()),
            whole.range(),
        ));
    }

    // 引用的序号不能超过实际份数。收文单位一清点就会打电话过来。
    for hit in attachment_reference().captures_iter(markdown) {
        let Some(number) = hit.get(1) else { continue };
        let Some(value) = parse_count(number.as_str()) else {
            continue;
        };
        // 「附件3份」这种是份数不是序号，前面那条规则已经管了。
        let whole = hit.get(0).expect("整体匹配");
        if markdown[whole.end()..].starts_with('份') {
            continue;
        }
        if value > actual {
            notes.push(note(
                "RULE-ATTACH-REF",
                "附件",
                Level::MustFix,
                format!("正文引用了「附件{value}」，但实际只有 {actual} 份附件"),
                whole.range(),
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
/// 1. **「贵」不能加在具体单位名前面。** 规范写法是「贵局」「贵委」「贵单位」，
///    「贵市财政局」不是词。纯字符串判定，零歧义——而这正是模型犯的第三种错。
/// 2. **同一篇里不能既称「贵局」又称「你局」。** 这是全篇一致性问题：单看一句
///    两种都对，只有通读全文才发现前后不一。模型逐句看，永远发现不了。
fn check_honorifics(markdown: &str, notes: &mut Vec<ProofNote>) {
    static ATTACHED: OnceLock<Regex> = OnceLock::new();
    // 「贵」与机构后缀之间还夹着两个以上汉字，就说明后面跟的是完整单位名。
    // 「贵局」「贵委」「贵办」中间没有字，不会命中；「贵单位」的「位」不是
    // 机构后缀，也不会命中。
    let attached = ATTACHED.get_or_init(|| {
        Regex::new(r"贵[\p{Han}]{2,10}(?:委员会|管理局|分局|局|委|办|厅|处|院|校|中心)")
            .expect("敬称正则必须有效")
    });
    for hit in attached.find_iter(markdown) {
        notes.push(note(
            "RULE-HONOR-ATTACHED",
            "称谓规范",
            Level::MustFix,
            format!(
                "「{}」把敬称加在了完整单位名前面。「贵」只能代指对方机关，应写「贵局」「贵委」「贵单位」，或直接写单位名称",
                hit.as_str()
            ),
            hit.range(),
        ));
    }

    // 同一个机构后缀上「贵」「你」并用。取第二次出现的位置报，让用户看到冲突。
    static PAIRED: OnceLock<Regex> = OnceLock::new();
    let paired = paired_honorific(&PAIRED);
    let mut seen_polite: Vec<&str> = Vec::new();
    let mut seen_plain: Vec<(&str, std::ops::Range<usize>)> = Vec::new();
    for hit in paired.captures_iter(markdown) {
        let whole = hit.get(0).expect("整体匹配");
        let suffix = hit.get(2).expect("后缀分组").as_str();
        let honorific = hit.get(1).expect("敬称分组").as_str();
        if honorific == "贵" {
            seen_polite.push(suffix);
        } else {
            seen_plain.push((suffix, whole.range()));
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
                "全篇对同一对象既称「贵{suffix}」又称「你{suffix}」，请统一（平行文用「贵」，下行文用「你」）"
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

// ── 成文日期 ────────────────────────────────────────────────────────────────

/// 成文日期是否已经过期。**只在导出时调用**——编辑期间日期本来就该是旧的，
/// 那时候弹提示纯属打扰。
///
/// `today` 由调用方传入，便于测试。
pub fn check_doc_date(input: &DraftInput, today: chrono::NaiveDate) -> Option<String> {
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
        "成文日期是 {year} 年 {month} 月 {day} 日，距今 {days} 天。签发日期与成文日期不一致时请先核对。"
    ))
}

#[cfg(test)]
mod tests {
    // ── 敬称与句末标点 ──────────────────────────────────────────────────

    #[test]
    fn honorific_attached_to_a_full_unit_name_is_flagged() {
        let notes = check_all("请贵市财政局于本月底前反馈意见。");
        let hit = notes
            .iter()
            .find(|note| note.entry_id == "RULE-HONOR-ATTACHED")
            .expect("「贵市财政局」应当报出来");
        assert_eq!(hit.level, Level::MustFix);
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
        check(&DraftInput::default(), markdown)
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
    fn a_notice_carrying_a_request_closing_is_flagged() {
        let input = draft(TemplateKind::OfficialLetter);
        let notes = check(
            &input,
            "# 关于报送情况的函\n\n有关情况如上。\n\n妥否，请批示。",
        );
        assert!(ids(&notes).contains(&"RULE-KIND-REQUEST"));
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
    fn a_declared_attachment_count_must_match_reality() {
        let input = draft(TemplateKind::OfficialLetter);
        let markdown = "# 关于报送情况的函\n\n随文报送附件3份。\n\n<!-- [附件] -->\n\n# 情况表";
        let notes = check(&input, markdown);
        assert!(ids(&notes).contains(&"RULE-ATTACH-COUNT"));
    }

    #[test]
    fn a_matching_attachment_count_is_silent() {
        let input = draft(TemplateKind::OfficialLetter);
        let markdown = "# 关于报送情况的函\n\n随文报送附件2份。\n\n<!-- [附件] -->\n\n# 情况表\n\n<!-- [附件] -->\n\n# 明细表";
        let notes = check(&input, markdown);
        assert!(!ids(&notes).contains(&"RULE-ATTACH-COUNT"));
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
        let input = draft(TemplateKind::OfficialLetter);
        let markdown = "# 关于报送情况的函\n\n正文。\n\n妥否，请批示。";
        let notes = check(&input, markdown);
        let hit = notes
            .iter()
            .find(|note| note.entry_id == "RULE-KIND-REQUEST")
            .expect("应当报出");
        assert_eq!(&markdown[hit.span.clone()], "妥否，请批示");
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
