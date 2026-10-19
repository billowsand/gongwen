//! 导入正文的结构清洗：把 Word / PDF / 纯文本里手打编号的段落还原成公文助手的
//! Markdown 结构。
//!
//! 按国标排的公文很少套 Word 标题样式，「一、」「（一）」「1.」多是手打（或自动
//! 编号）在段首，anydoc 转出来只是普通段落：预览里编号照印、层级全无，标题字体
//! 也排不出来。这里只用确定性规则判断，不调用模型：
//!
//! - `一、` → `##`，`（一）` → `###`：取编号后第一句（遇 `。` 截止）或冒号前的短语
//!   作标题，其余留作正文，紧贴标题下一行写——选了紧缩风格时程序会再把两者接排
//!   回「（一）加强领导。各单位……」的样子，没选就各自成段。
//! - `1.` / `（1）`：同级连续的一组编号按版式定性。有项后面还挂着正文段或下级编号，
//!   或者每项都是「短标题。长正文」的接排写法，就是标题（`####` / `#####`）；
//!   否则是干条，转成列表 `1. `。`①` 一律当列表。
//! - 原本就是 Markdown 标题、但文字带编号的（Word 标题样式），按编号重排层级。
//! - 开头没有 `# ` 时，把像公文标题的段落（「关于……」或以文种结尾）升成 `# `，
//!   两行、三行折开的标题先接起来，后面的「（征求意见稿）」括注并进标题。
//! - 单独一行的「附件」「附件2」后接短标题，或「附件1：清单」写成一行，都改成
//!   `<!-- [附件] -->` + `# 标题`；正文末尾程序会重新生成的「附件：1.××」说明行删掉。
//! - `第X章/节/部分` 比「一、」还浅一层；全角数字、`1)`、字母编号也认。
//! - 文首版头（份号、密级、紧急程度、发文机关标志、文号、签发人）与文末版记
//!   （落款单位、成文日期、抄送、印发机关）按整行高置信度规则剥离。
//! - 段内「一是……；二是……」拆成一项一行，引子（到第一个逗号 / 句号）加粗；
//!   没有编号的短标题、条文里的「第X条」也加粗——这些版面线索不够，加粗比猜
//!   标题层级安全。
//!
//! 只按实际用到的编号层级压紧输出，避免 `##` 下面直接出现 `####`。剥掉的编号由
//! 程序按层级重新生成，输出里不留原编号；认不准的段落原样保留，宁可少认也不错认。

use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

/// 句号前的标题最多这么多字；再长就是段首带编号的正文段。
const TITLE_MAX: usize = 40;
/// 冒号前的短语最多这么多字，且不能带逗号——「会议时间：」是标题，
/// 「根据工作安排，现将有关事项通知如下：」不是。
const LABEL_MAX: usize = 20;
/// 接排标题（「1.加强领导。各单位……」）的标题最多这么多字。
const RUN_IN_TITLE_MAX: usize = 20;
/// 接排标题后面的正文至少这么多字，才不算干条。
const RUN_IN_BODY_MIN: usize = 30;
/// 文档标题最多这么多字。
const DOC_TITLE_MAX: usize = 60;
/// 没编号的短标题最多这么多字；再长就是正文段。
const UNNUMBERED_TITLE_MAX: usize = 14;
/// 版头 / 版记最多往前 / 往后看这么多块。
const LAYOUT_SCAN: usize = 10;

/// 公文标题常见的结尾文种，用来认开头那行是不是文档标题。
const DOC_KINDS: &[&str] = &[
    "通知",
    "通报",
    "通告",
    "公告",
    "函",
    "请示",
    "报告",
    "意见",
    "方案",
    "决定",
    "决议",
    "纪要",
    "批复",
    "议程",
    "办法",
    "规定",
    "细则",
    "要点",
    "计划",
    "规划",
    "总结",
    "汇报",
    "提案",
    "建议",
    "说明",
    "讲话",
    "呈批件",
];

/// 清洗选项。
pub(crate) struct Options {
    /// 开头没有 `# ` 时，是否把像公文标题的段落升成 `# `。往已经有标题的稿子
    /// 里插入时要关掉，否则正文区会多出第二个标题。
    pub(crate) detect_title: bool,
    /// 每个非空行就是一段。Word 另存的纯文本段与段之间不空行，按 Markdown 读会
    /// 全部接成一段。
    pub(crate) lines_are_paragraphs: bool,
    /// 剥掉文首版头（密级、紧急程度、文号、签发人、份号）与文末版记（落款单位、
    /// 成文日期、抄送、印发机关与日期）。导入已有公文时用；对正在写的稿子做
    /// 结构整理时关掉，免得把作者刚写的落款当成版记删掉。
    pub(crate) strip_layout: bool,
    /// 把没有编号、单独成行的短标题加粗（`**总体要求**`），而不是升成 Markdown
    /// 标题——版面上没有字号线索，加粗比猜层级安全。
    pub(crate) bold_unnumbered: bool,
    /// 把段内的「一是……；二是……」拆成一项一行，并把「一是」到第一个逗号
    /// （或句号）之间的内容加粗。公文里这是标准的段内枚举写法，Markdown 没有
    /// 对应语法，用加粗凸显。
    pub(crate) inline_enumeration: bool,
    /// 修复 PDF 抽文字时并成一段的多个自然段：PDF 的段落间距小，转换器常把
    /// 主送、正文、加粗标题、附件说明、落款并进同一个段落。只有 PDF 走这条。
    pub(crate) repair_merged_paragraphs: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            detect_title: true,
            lines_are_paragraphs: false,
            strip_layout: true,
            bold_unnumbered: true,
            inline_enumeration: true,
            repair_merged_paragraphs: false,
        }
    }
}

impl Options {
    /// 导入一份现成公文时的选项。
    pub(crate) fn import(detect_title: bool, lines_are_paragraphs: bool) -> Self {
        Self {
            detect_title,
            lines_are_paragraphs,
            ..Self::default()
        }
    }

    /// 对正在写的稿子做「公文编号 → Markdown 编号」整理：稿子里已经有标题、附件
    /// 与要素，不认标题、不剥版头版记，只把段首编号转成结构。
    pub(crate) fn restructure_only() -> Self {
        Self {
            detect_title: false,
            lines_are_paragraphs: false,
            strip_layout: false,
            bold_unnumbered: false,
            inline_enumeration: false,
            repair_merged_paragraphs: false,
        }
    }
}

/// 按公文版式还原标题、列表、文档标题与附件标记。输入是归一化过的 markdown。
pub(crate) fn restructure(markdown: &str, options: &Options) -> String {
    let repaired;
    let markdown = if options.repair_merged_paragraphs {
        repaired = repair_merged_paragraphs(markdown);
        repaired.as_str()
    } else {
        markdown
    };
    let mut blocks = parse_blocks(markdown, options.lines_are_paragraphs);
    if options.strip_layout {
        strip_layout(&mut blocks);
    }
    resolve_ambiguous_levels(&mut blocks);
    if options.detect_title {
        promote_title(&mut blocks);
    }
    mark_attachments(&mut blocks);
    if options.inline_enumeration {
        split_inline_enumerations(&mut blocks);
    }
    if options.bold_unnumbered {
        bold_unnumbered_titles(&mut blocks);
    }
    bold_article_labels(&mut blocks);
    let roles = classify(&blocks);
    render(&blocks, &roles)
}

/// 切出来的一块。
#[derive(Debug)]
enum Block {
    /// 普通段落：各行已去掉行首空白与行尾的硬换行符。
    Para(Vec<String>),
    /// 段首带编号的段落。
    Item(Item),
    /// 原有的、不带编号的 Markdown 标题，原样保留。
    Heading(usize, String),
    /// 认出来的附件起点，带附件标题。
    Attachment(String),
    /// 表格、代码、公式、图片、注释、普通列表等，原样输出。
    Raw(Vec<String>),
}

#[derive(Debug)]
struct Item {
    /// 编号样式从浅到深：0 = `第X章/节/部分`，1 = `一、`，2 = `（一）`，
    /// 3 = `1.`，4 = `（1）`，5 = `①`。定级先按样式，输出时再按全篇实际
    /// 用到的层级压紧，避免跳级。
    level: u8,
    /// 阿拉伯数字与圈号的序号，用来判断是不是同一组；汉字序号不需要，记 0。
    number: u32,
    /// 原文各行，既不当标题也不当列表时原样输出。
    source: Vec<String>,
    /// 去掉编号与加粗记号后的文字，按原行分开。
    rest: Vec<String>,
    /// 拆出来的标题与正文；拆不出来为 `None`。
    split: Option<Split>,
    /// 原本就是 Markdown 标题（Word 标题样式），只按编号重排层级。
    styled: bool,
    /// Word 自动编号的「（N）」：汉字计数也被转成阿拉伯数字，分不清是 `（一）`
    /// 还是 `（1）`，要看上下文。
    ambiguous: bool,
}

#[derive(Debug)]
struct Split {
    title: String,
    body: Vec<String>,
    /// 标题后面还接着正文（「标题。正文」的接排写法）。
    run_in: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// 不是编号段，或编号段原样保留。
    Literal,
    Heading,
    List,
}

// ---------------------------------------------------------------- 切块

fn parse_blocks(markdown: &str, lines_are_paragraphs: bool) -> Vec<Block> {
    let mut parser = Parser::default();
    for line in markdown.lines() {
        parser.line(line, lines_are_paragraphs);
    }
    parser.flush_para();
    parser.flush_raw();
    parser.blocks
}

#[derive(Default)]
struct Parser {
    blocks: Vec<Block>,
    para: Vec<String>,
    /// 当前段落来自 Word 自动编号列表（`- 1、 内容`）。
    para_from_list: bool,
    raw: Vec<String>,
    /// 正在代码块或公式块里，记着收尾记号。
    fence: Option<&'static str>,
}

impl Parser {
    fn line(&mut self, line: &str, lines_are_paragraphs: bool) {
        let trimmed = line.trim();
        if let Some(close) = self.fence {
            self.raw.push(line.to_string());
            if trimmed.starts_with(close) {
                self.fence = None;
                self.flush_raw();
            }
            return;
        }
        if trimmed.is_empty() {
            self.flush_para();
            self.flush_raw();
            return;
        }
        if let Some(close) = fence_open(trimmed) {
            self.flush_para();
            self.flush_raw();
            self.raw.push(line.to_string());
            self.fence = Some(close);
            return;
        }
        if let Some((level, text)) = markdown_heading(trimmed) {
            self.flush_para();
            self.flush_raw();
            self.blocks.push(heading_block(level, text, trimmed));
            return;
        }
        let text = unescape_number(trimmed);
        if let Some(content) = strip_list_marker(&text) {
            self.flush_para();
            if parse_prefix(&strip_bold(content), true).is_some() {
                // Word 自动编号转出来的 `- 1、 内容`：编号是版式，不是列表。
                self.flush_raw();
                self.para.push(strip_hard_break(content).to_string());
                self.para_from_list = true;
                self.flush_para();
            } else {
                self.raw.push(line.to_string());
            }
            return;
        }
        if is_raw_line(trimmed) {
            self.flush_para();
            self.raw.push(line.to_string());
            return;
        }
        self.flush_raw();
        let piece = strip_hard_break(&text).to_string();
        // 段中间冒出一行编号，多半是 PDF 没切开的新段。
        if !self.para.is_empty() && parse_prefix(&strip_bold(&piece), false).is_some() {
            self.flush_para();
        }
        self.para.push(piece);
        if lines_are_paragraphs {
            self.flush_para();
        }
    }

    fn flush_para(&mut self) {
        if self.para.is_empty() {
            return;
        }
        let lines = std::mem::take(&mut self.para);
        let from_list = std::mem::take(&mut self.para_from_list);
        self.blocks.push(paragraph_block(lines, from_list));
    }

    fn flush_raw(&mut self) {
        if !self.raw.is_empty() {
            self.blocks.push(Block::Raw(std::mem::take(&mut self.raw)));
        }
    }
}

/// 代码块与独占一行的公式块的开头，返回收尾记号。
fn fence_open(trimmed: &str) -> Option<&'static str> {
    if trimmed.starts_with("```") {
        Some("```")
    } else if trimmed.starts_with("~~~") {
        Some("~~~")
    } else if trimmed == "$$" {
        Some("$$")
    } else {
        None
    }
}

/// `## 标题` → `(2, "标题")`。
fn markdown_heading(trimmed: &str) -> Option<(usize, &str)> {
    let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &trimmed[hashes..];
    if !rest.starts_with([' ', '\t']) || rest.trim().is_empty() {
        return None;
    }
    Some((hashes, rest.trim()))
}

/// 原有的 Markdown 标题：文字带编号就按编号重排层级，否则原样保留。
fn heading_block(level: usize, text: &str, source: &str) -> Block {
    let plain = strip_bold(text);
    let Some(prefix) = parse_prefix(&plain, false) else {
        return Block::Heading(level, text.to_string());
    };
    let title = strip_leading_punct(&plain[prefix.len..]).to_string();
    Block::Item(Item {
        level: prefix.level,
        number: prefix.number,
        source: vec![source.to_string()],
        rest: vec![title.clone()],
        split: Some(Split {
            title,
            body: Vec::new(),
            run_in: false,
        }),
        styled: true,
        ambiguous: prefix.ambiguous,
    })
}

/// anydoc 会把段首的「1. 」转义成「1\. 」，免得被当成列表；这里还原。
fn unescape_number(trimmed: &str) -> String {
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=3).contains(&digits) && trimmed[digits..].starts_with("\\.") {
        format!("{}{}", &trimmed[..digits], &trimmed[digits + 1..])
    } else {
        trimmed.to_string()
    }
}

/// `- 内容` / `* 内容` / `+ 内容` 去掉列表记号。
fn strip_list_marker(text: &str) -> Option<&str> {
    ["- ", "* ", "+ "]
        .iter()
        .find_map(|marker| text.strip_prefix(marker))
        .map(str::trim)
        .filter(|content| !content.is_empty())
}

/// 表格、图片、注释、引用、HTML、分隔线、单行公式：结构清洗不碰。
fn is_raw_line(trimmed: &str) -> bool {
    if trimmed.starts_with(['|', '<', '>']) || trimmed.starts_with("![") {
        return true;
    }
    if trimmed.len() > 4 && trimmed.starts_with("$$") && trimmed.ends_with("$$") {
        return true;
    }
    let marks: String = trimmed
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '\\')
        .collect();
    marks.chars().count() >= 3
        && (marks.chars().all(|ch| ch == '-')
            || marks.chars().all(|ch| ch == '*')
            || marks.chars().all(|ch| ch == '_'))
}

/// Word 的软回车（Shift+Enter）转出来是行尾反斜杠；公文正文没有行内换行，去掉。
fn strip_hard_break(text: &str) -> &str {
    match text.strip_suffix('\\') {
        Some(rest) if !rest.ends_with('\\') => rest.trim_end(),
        _ => text,
    }
}

/// 去掉成对的 `**`。标题与判断编号都不需要加粗记号。
fn strip_bold(text: &str) -> String {
    if text.matches("**").count() >= 2 {
        text.replace("**", "")
    } else {
        text.to_string()
    }
}

/// 段落收尾：段首认得出编号就成编号段，否则是普通段落。
fn paragraph_block(lines: Vec<String>, from_list: bool) -> Block {
    let first = &lines[0];
    // 段首加粗的一截（`**一、总体要求。**各单位……`）就是作者标出来的标题。
    let (bold, after_bold) = leading_bold(first);
    let plain_first = match bold {
        Some(inner) => format!("{inner}{after_bold}"),
        None => strip_bold(first),
    };
    let Some(prefix) = parse_prefix(&plain_first, from_list) else {
        return Block::Para(lines);
    };
    // 「（一）、加强领导」这种编号后多打一个顿号的，顿号不算标题内容。
    let mut rest = vec![strip_leading_punct(&plain_first[prefix.len..]).to_string()];
    rest.extend(lines[1..].iter().map(|line| strip_bold(line)));
    let bold_title = bold
        .filter(|inner| inner.len() > prefix.len)
        .map(|inner| {
            let title = inner[prefix.len..]
                .trim()
                .trim_end_matches(['。', '：', ':', '．', '.'])
                .trim();
            (title.to_string(), after_bold.trim().to_string())
        })
        .filter(|(title, _)| !title.is_empty());
    let split = split_title(bold_title, &rest);
    Block::Item(Item {
        level: prefix.level,
        number: prefix.number,
        source: lines,
        rest,
        split,
        styled: false,
        ambiguous: prefix.ambiguous,
    })
}

/// 编号后误带的顿号 / 逗号 / 句点（「（一）、加强领导」）不算标题内容。
fn strip_leading_punct(text: &str) -> &str {
    text.trim_start_matches(['、', '，', ',', '．', '.', '。', '）', ')', '；', ';'])
        .trim_start()
}

/// 行首 `**…**` 一截：返回加粗里的文字与其后的文字。
fn leading_bold(line: &str) -> (Option<&str>, &str) {
    let Some(inner) = line.strip_prefix("**") else {
        return (None, line);
    };
    match inner.find("**") {
        Some(end) if end > 0 => (Some(&inner[..end]), &inner[end + 2..]),
        _ => (None, line),
    }
}

// ---------------------------------------------------------------- 编号

struct Prefix {
    level: u8,
    number: u32,
    /// 编号（连同其后空白）占的字节数。
    len: usize,
    ambiguous: bool,
}

/// 认段首编号。`from_list` 表示编号来自 Word 自动编号：汉字计数的「一、」会被
/// 转成「1、」，所以这时 `N、` 算一级标题。
fn parse_prefix(text: &str, from_list: bool) -> Option<Prefix> {
    static PATTERNS: OnceLock<[Regex; 7]> = OnceLock::new();
    let [
        chapter,
        chinese,
        chinese_paren,
        arabic,
        arabic_paren,
        arabic_stop,
        circled,
    ] = PATTERNS.get_or_init(|| {
        let numeral = "[一二三四五六七八九十百零〇]{1,4}";
        let digits = "[0-9０-９]{1,3}";
        [
            // 第X章 / 第X节 / 第X部分 / 第X编：比「一、」还浅的一层。
            Regex::new(
                r"^第([一二三四五六七八九十百零〇0-9０-９]{1,4})(章|节|部分|编)\s*[、.．:：]?\s*",
            ),
            Regex::new(&format!(r"^({numeral})[、，,．.]\s*")),
            Regex::new(&format!(r"^[（(]({numeral})[）)]\s*")),
            Regex::new(&format!(r"^({digits})([.．、])\s*")),
            Regex::new(&format!(r"^[（(]({digits})[）)]\s*")),
            // 「1)」：公文常见，Word 自动编号也常转成这个形状。
            Regex::new(&format!(r"^({digits})[)）]\s*")),
            Regex::new(r"^([①-⑳])\s*"),
        ]
        .map(|re| re.expect("valid numbering pattern"))
    });
    let prefix = if let Some(caps) = chapter.captures(text) {
        Prefix {
            level: 0,
            number: 0,
            len: caps[0].len(),
            ambiguous: false,
        }
    } else if let Some(caps) = chinese.captures(text) {
        Prefix {
            level: 1,
            number: 0,
            len: caps[0].len(),
            ambiguous: false,
        }
    } else if let Some(caps) = chinese_paren.captures(text) {
        Prefix {
            level: 2,
            number: 0,
            len: caps[0].len(),
            ambiguous: false,
        }
    } else if let Some(caps) = arabic.captures(text) {
        // 「1.5亿元」「3.2.1」不是编号。
        if text[caps[0].len()..]
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_digit() || ('０'..='９').contains(&ch))
            && caps[0].len() == caps[1].len() + caps[2].len()
        {
            return None;
        }
        Prefix {
            level: if from_list && &caps[2] == "、" { 1 } else { 3 },
            number: parse_number(&caps[1])?,
            len: caps[0].len(),
            ambiguous: false,
        }
    } else if let Some(caps) = arabic_paren.captures(text) {
        Prefix {
            level: 4,
            number: parse_number(&caps[1])?,
            len: caps[0].len(),
            ambiguous: from_list,
        }
    } else if let Some(caps) = arabic_stop.captures(text) {
        Prefix {
            level: 4,
            number: parse_number(&caps[1])?,
            len: caps[0].len(),
            ambiguous: false,
        }
    } else {
        let caps = circled.captures(text)?;
        let ch = caps[1].chars().next()?;
        Prefix {
            level: 5,
            number: ch as u32 - '①' as u32 + 1,
            len: caps[0].len(),
            ambiguous: false,
        }
    };
    let rest = &text[prefix.len..];
    if rest.trim().is_empty() || is_enumeration(rest) {
        return None;
    }
    Some(prefix)
}

/// 把编号里的数字读成 `u32`：全角数字（`１`）与半角数字都认。
fn parse_number(text: &str) -> Option<u32> {
    let mut value = 0u32;
    for ch in text.chars() {
        let digit = match ch {
            '0'..='9' => ch as u32 - '0' as u32,
            '０'..='９' => ch as u32 - '０' as u32,
            _ => return None,
        };
        value = value.checked_mul(10)?.checked_add(digit)?;
    }
    Some(value)
}

/// 「一、二季度」「2、3月份」：顿号连着的是并列的数，不是编号。
fn is_enumeration(rest: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(?:[一二三四五六七八九十]+|[0-9０-９]+)[、，,季月年日号]")
            .expect("valid enumeration pattern")
    })
    .is_match(rest)
}

// ---------------------------------------------------------------- 拆标题

/// 从编号后的文字里拆出标题与正文。`bold` 是段首加粗的那截（已去编号与句末标点）
/// 及其后的文字。
fn split_title(bold: Option<(String, String)>, rest: &[String]) -> Option<Split> {
    if let Some((title, after)) = bold
        && chars(&title) <= TITLE_MAX
        && !title.contains('。')
    {
        return Some(make_split(title, &after, &rest[1..]));
    }
    // 标题与正文之间是软回车。
    if rest.len() > 1 && is_title_line(&rest[0]) {
        return Some(make_split(rest[0].clone(), "", &rest[1..]));
    }
    let first = &rest[0];
    if let Some((index, mark)) = first_top_level_stop(first) {
        let title = first[..index].trim();
        let after = &first[index + mark.len_utf8()..];
        let ok = if matches!(mark, '：' | ':') {
            chars(title) <= LABEL_MAX && !title.contains(['，', ',', '；', ';'])
        } else {
            chars(title) <= TITLE_MAX
        };
        return (ok && !title.is_empty()).then(|| make_split(title.to_string(), after, &rest[1..]));
    }
    (rest.len() == 1 && is_title_line(first)).then(|| make_split(first.clone(), "", &[]))
}

fn make_split(title: String, after: &str, others: &[String]) -> Split {
    let mut body = Vec::new();
    let after = after.trim();
    if !after.is_empty() {
        body.push(after.to_string());
    }
    body.extend(
        others
            .iter()
            .filter(|line| !line.trim().is_empty())
            .cloned(),
    );
    Split {
        title: title.replace("**", "").trim().to_string(),
        run_in: !body.is_empty(),
        body,
    }
}

/// 括号、书名号、引号以外的第一个句号 / 问号 / 叹号 / 冒号。
fn first_top_level_stop(text: &str) -> Option<(usize, char)> {
    let mut depth = 0i32;
    for (index, ch) in text.char_indices() {
        match ch {
            '（' | '(' | '《' | '“' | '‘' | '【' | '「' | '〔' => depth += 1,
            '）' | ')' | '》' | '”' | '’' | '】' | '」' | '〕' => {
                depth = (depth - 1).max(0)
            }
            '。' | '！' | '？' | '：' | ':' if depth == 0 => return Some((index, ch)),
            _ => {}
        }
    }
    None
}

/// 能单独作标题的一行：不长、不以标点收尾、不含句号。
fn is_title_line(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty()
        && chars(text) <= TITLE_MAX
        && !text.contains('。')
        && !ends_with_punctuation(text)
}

fn ends_with_punctuation(text: &str) -> bool {
    text.ends_with([
        '。', '；', ';', '，', ',', '：', ':', '！', '？', '!', '?', '、',
    ])
}

fn chars(text: &str) -> usize {
    text.chars().count()
}

// ---------------------------------------------------------------- 上下文

/// Word 自动编号的「（N）」：前面最近的编号是 `1.` 一级或更深就是 `（1）`，
/// 否则是 `（一）`。
fn resolve_ambiguous_levels(blocks: &mut [Block]) {
    let mut context = 0u8;
    for block in blocks.iter_mut() {
        match block {
            Block::Item(item) => {
                if item.ambiguous {
                    item.level = if context >= 3 { 4 } else { 2 };
                }
                context = item.level;
            }
            Block::Heading(..) | Block::Attachment(_) => context = 0,
            _ => {}
        }
    }
}

/// 开头像公文标题的段落升成 `# `。只看第一个编号段、主送机关之前的几段。
fn promote_title(blocks: &mut Vec<Block>) {
    if blocks
        .iter()
        .any(|block| matches!(block, Block::Heading(1, _)))
    {
        return;
    }
    let mut index = 0;
    while index < blocks.len().min(LAYOUT_SCAN) {
        let text = match &blocks[index] {
            Block::Para(lines) => para_text(lines),
            Block::Raw(_) => {
                index += 1;
                continue;
            }
            Block::Item(_) | Block::Attachment(_) | Block::Heading(..) => return,
        };
        if text.trim().is_empty() {
            index += 1;
            continue;
        }
        // 主送机关（「各区县人民政府：」）之后不会再有文档标题。
        if is_recipient_line(&text) {
            return;
        }
        // 两行、三行折开的标题：「关于做好××工作」「的通知」。只有以文种结尾才算
        // 标题完整；「关于……」开头的半截标题要继续接下一行。
        let mut joined = text.clone();
        let mut consumed = 1usize;
        while consumed <= 3 && !title_complete(&joined) {
            if joined.contains('。') || ends_with_punctuation(&joined) {
                break;
            }
            let Some(Block::Para(next)) = blocks.get(index + consumed) else {
                break;
            };
            let next = para_text(next);
            if chars(&next) > 20
                || next.trim().is_empty()
                || ends_with_kind(&joined)
                || ends_with_punctuation(&next)
                || is_recipient_line(&next)
            {
                break;
            }
            // 只接能让标题收在文种上的那半截，或「的通知」这种虚词开头；否则多半
            // 是下一段的短标题，接上去会把标题写坏。
            let candidate = format!("{joined}{next}");
            if !ends_with_kind(&candidate) && !next.starts_with(['的', '之']) {
                break;
            }
            joined = candidate;
            consumed += 1;
        }
        if is_doc_title(&joined) {
            for _ in 1..consumed {
                blocks.remove(index + 1);
            }
            blocks[index] = Block::Heading(1, joined);
            // 标题后紧跟的「（征求意见稿）」这类括注并进标题。
            if let Some(Block::Para(lines)) = blocks.get(index + 1) {
                let suffix = para_text(lines);
                if is_title_suffix(&suffix) {
                    if let Block::Heading(1, title) = &mut blocks[index] {
                        title.push_str(&suffix);
                    }
                    blocks.remove(index + 1);
                }
            }
            return;
        }
        index += 1;
    }
}

/// 主送机关行：「各区县人民政府：」「办公室：」。
fn is_recipient_line(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty()
        && chars(text) <= 30
        && text.ends_with(['：', ':'])
        && !text.contains('。')
        && !text.starts_with('（')
}

/// 标题后的括注：「（征求意见稿）」「（代拟稿）」。
fn is_title_suffix(text: &str) -> bool {
    let text = text.trim();
    chars(text) <= 16
        && text.starts_with(['（', '('])
        && text.ends_with(['）', ')'])
        && (text.contains('稿') || text.contains("草案") || text.contains("试行"))
}

/// 标题已经写到文种，可以停手了；「关于××工作」这种半截还要接下一行。
fn title_complete(text: &str) -> bool {
    is_doc_title(text) && ends_with_kind(text)
}

fn is_doc_title(text: &str) -> bool {
    let count = chars(text);
    (4..=DOC_TITLE_MAX).contains(&count)
        && !text.contains(['。', '：', ':'])
        && !ends_with_punctuation(text)
        && (text.starts_with("关于") || ends_with_kind(text))
}

fn ends_with_kind(text: &str) -> bool {
    DOC_KINDS.iter().any(|kind| text.ends_with(kind))
}

fn para_text(lines: &[String]) -> String {
    join_pieces(
        &lines
            .iter()
            .map(|line| strip_bold(line))
            .collect::<Vec<_>>(),
    )
}

/// 段落块的文字；编号段、标题、原始块返回 `None`。
fn single_para_text(block: &Block) -> Option<String> {
    match block {
        Block::Para(lines) if !lines.is_empty() => Some(para_text(lines)),
        _ => None,
    }
}

/// 附件起点：「附件」「附件2」单独一行后接短标题；「附件1：物资清单」写成一行也认。
fn mark_attachments(blocks: &mut Vec<Block>) {
    let mut index = 0;
    while index < blocks.len() {
        let current = match &blocks[index] {
            Block::Para(lines) if lines.len() == 1 => strip_bold(&lines[0]),
            _ => {
                index += 1;
                continue;
            }
        };
        // 「附件：1.×× 2.××」是附件说明，不是附件标题；文件里有附件内容时最后会删掉。
        if is_attachment_list_line(&current) {
            index += 1;
            continue;
        }
        // 同行写法：标号与标题都在一行。
        if let Some(title) = attachment_inline_title(&current)
            && let Some(title) = clean_attachment_title(&title)
        {
            blocks[index] = Block::Attachment(title);
            index += 1;
            continue;
        }
        if !is_attachment_label(&current) {
            index += 1;
            continue;
        }
        // 单独一行的标号：后接一行或两行短标题（折行）。
        let mut title = String::new();
        let mut consumed = 0usize;
        while consumed < 2 {
            let Some(Block::Para(lines)) = blocks.get(index + 1 + consumed) else {
                break;
            };
            let piece = para_text(lines);
            if chars(&piece) > DOC_TITLE_MAX
                || piece.contains('。')
                || ends_with_punctuation(&piece)
            {
                break;
            }
            title.push_str(&piece);
            consumed += 1;
        }
        if let Some(title) = clean_attachment_title(&title) {
            for _ in 0..consumed {
                blocks.remove(index + 1);
            }
            blocks[index] = Block::Attachment(title);
        }
        index += 1;
    }

    // 正文末尾的「附件：1.×× 2.××」说明行由程序重新生成；文件里已有附件内容时删掉。
    if blocks
        .iter()
        .any(|block| matches!(block, Block::Attachment(_)))
    {
        blocks.retain(|block| {
            !single_para_text(block).is_some_and(|text| is_attachment_list_line(&text))
        });
    }
}

/// 「附件1：物资清单」这种把标号与标题写在一行的，取冒号后的标题。
fn attachment_inline_title(text: &str) -> Option<String> {
    let rest = text.trim().strip_prefix("附件")?;
    let mut rest = rest.trim_start();
    let digits = rest.chars().take_while(|ch| ch.is_ascii_digit()).count();
    rest = &rest[digits..];
    let rest = rest.trim_start().strip_prefix(['：', ':'])?.trim();
    (!rest.is_empty()).then(|| rest.to_string())
}

fn clean_attachment_title(title: &str) -> Option<String> {
    let title = title.trim();
    (!title.is_empty()
        && chars(title) <= DOC_TITLE_MAX
        && !title.contains('。')
        && !ends_with_punctuation(title))
    .then(|| title.to_string())
}

fn is_attachment_label(text: &str) -> bool {
    let text = text.trim().trim_end_matches(['：', ':']);
    text.strip_prefix("附件")
        .is_some_and(|number| number.trim().chars().all(|ch| ch.is_ascii_digit()))
}

/// 「附件：1.×× 2.××」：附件说明行，程序会按附件标题重新生成。
fn is_attachment_list_line(text: &str) -> bool {
    let Some(rest) = text.trim().strip_prefix("附件") else {
        return false;
    };
    let rest = rest.trim_start();
    let digits = rest.chars().take_while(|ch| ch.is_ascii_digit()).count();
    let rest = rest[digits..].trim_start();
    let Some(rest) = rest.strip_prefix(['：', ':']) else {
        return false;
    };
    let rest = rest.trim();
    let numbered = rest.starts_with("1.")
        || rest.starts_with("1、")
        || rest.starts_with("（1）")
        || rest.starts_with("(1)");
    numbered
        && (rest.contains("2.")
            || rest.contains("2、")
            || rest.contains("（2）")
            || rest.contains("(2)"))
}

// ---------------------------------------------------------------- PDF 并段修复

/// PDF 转换器按行间距判段落，公文行距紧时常把多段并成一段。这里只切「明显该断」
/// 的地方：段内加粗编号标题、主送机关冒号、附件说明、抄送、落款单位 + 成文日期，
/// 以及列表项短句后接的正文；只对 PDF 打开，Word / 纯文本段落本来就是分开的。
fn repair_merged_paragraphs(markdown: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut previous_nonblank: Option<String> = None;
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !out
                .last()
                .is_some_and(|last: &String| last.trim().is_empty())
            {
                out.push(String::new());
            }
            previous_nonblank = None;
            continue;
        }
        // 表格、代码围栏、公式、图片、已有标题、注释：保持原样。
        if is_raw_line(trimmed) || markdown_heading(trimmed).is_some() {
            out.push(line.to_string());
            previous_nonblank = Some(trimmed.to_string());
            continue;
        }
        let mut pieces = split_inline_headings(line);
        pieces = pieces
            .into_iter()
            .flat_map(|piece| split_after_recipient(&piece))
            .collect();
        pieces = pieces
            .into_iter()
            .flat_map(|piece| split_before_layout_markers(&piece))
            .collect();
        pieces = pieces
            .into_iter()
            .flat_map(|piece| split_signature_and_date(&piece))
            .collect();
        // 只有上一非空行也是列表项时才拆「3.短句。后文」，免得把带正文的
        // 「1.排查隐患。对河道……」这种标题当列表项。
        let in_list = previous_nonblank
            .as_deref()
            .is_some_and(starts_with_list_marker);
        if in_list {
            pieces = pieces
                .into_iter()
                .flat_map(|piece| split_list_item_body(&piece))
                .collect();
        }
        if pieces.len() > 1 {
            if starts_new_paragraph(trimmed)
                && !out
                    .last()
                    .is_some_and(|last: &String| last.trim().is_empty())
            {
                out.push(String::new());
            }
            for (index, piece) in pieces.iter().enumerate() {
                if index > 0 {
                    out.push(String::new());
                }
                out.push(piece.trim().to_string());
            }
        } else {
            if starts_new_paragraph(trimmed)
                && !out
                    .last()
                    .is_some_and(|last: &String| last.trim().is_empty())
            {
                out.push(String::new());
            }
            out.push(line.to_string());
        }
        previous_nonblank = Some(trimmed.to_string());
    }
    let mut repaired: Vec<String> = Vec::new();
    for line in out {
        if line.trim().is_empty() {
            if !repaired
                .last()
                .is_some_and(|last: &String| last.trim().is_empty())
            {
                repaired.push(String::new());
            }
        } else {
            repaired.push(line);
        }
    }
    while repaired
        .first()
        .is_some_and(|line: &String| line.trim().is_empty())
    {
        repaired.remove(0);
    }
    while repaired
        .last()
        .is_some_and(|line: &String| line.trim().is_empty())
    {
        repaired.pop();
    }
    repaired.join("\n")
}

/// 段内的加粗编号标题（如两星号包住的「一、总体要求」）：PDF 会把它们和前后正文并成一行。
fn split_inline_headings(line: &str) -> Vec<String> {
    let mut cuts = Vec::new();
    let mut search = 0;
    while let Some(relative) = line[search..].find("**") {
        let open = search + relative;
        let Some(close_relative) = line[open + 2..].find("**") else {
            break;
        };
        let close = open + 2 + close_relative;
        if is_inline_heading(&line[open + 2..close]) {
            cuts.push(open);
        }
        search = close + 2;
    }
    split_at_cuts(line, cuts)
}

fn is_inline_heading(inner: &str) -> bool {
    let inner = inner.trim();
    !inner.is_empty() && chars(inner) <= TITLE_MAX && parse_prefix(inner, false).is_some()
}

/// 主送机关与正文并在一行：「各区县人民政府：为切实做好……」。
fn split_after_recipient(line: &str) -> Vec<String> {
    let trimmed = line.trim_start();
    let Some((colon, ch)) = trimmed
        .char_indices()
        .find(|(_, ch)| matches!(ch, '：' | ':'))
    else {
        return vec![line.to_string()];
    };
    let prefix = &trimmed[..colon];
    let rest = trimmed[colon + ch.len_utf8()..].trim_start();
    if rest.is_empty() || !looks_like_recipient(prefix) {
        return vec![line.to_string()];
    }
    vec![
        trimmed[..colon + ch.len_utf8()].trim_end().to_string(),
        rest.to_string(),
    ]
}

fn looks_like_recipient(prefix: &str) -> bool {
    let prefix = prefix.trim();
    (2..=30).contains(&chars(prefix))
        && !prefix.contains(['。', '；', '！', '？'])
        && (prefix.contains('各')
            || prefix.ends_with("单位")
            || prefix.ends_with("部门")
            || prefix.ends_with("政府")
            || prefix.ends_with("委员会")
            || prefix.ends_with("办公室")
            || prefix.ends_with("同志"))
}

/// 附件说明、抄送与前面的正文并在一行。
fn split_before_layout_markers(line: &str) -> Vec<String> {
    let mut cuts = Vec::new();
    for marker in ["附件：", "附件:", "抄送：", "抄送:"] {
        if let Some(position) = line.find(marker)
            && position > 0
            && line[..position].chars().next_back().is_some_and(|ch| {
                ch.is_whitespace() || matches!(ch, '。' | '；' | '：' | '！' | '？' | '日')
            })
        {
            cuts.push(position);
        }
    }
    split_at_cuts(line, cuts)
}

/// 落款单位与成文日期并在一行。
fn split_signature_and_date(line: &str) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"((?:\d{4}|[〇零一二三四五六七八九]{4})年(?:\d{1,2}|[一二三四五六七八九十]{1,3})月(?:\d{1,2}|[一二三四五六七八九十]{1,3})日)$",
        )
        .expect("valid date suffix pattern")
    });
    let trimmed = line.trim();
    let Some(caps) = re.captures(trimmed) else {
        return vec![line.to_string()];
    };
    let start = caps.get(1).map_or(trimmed.len(), |caps| caps.start());
    let unit = trimmed[..start].trim_end();
    if !looks_like_signature_unit(unit) {
        return vec![line.to_string()];
    }
    vec![unit.to_string(), trimmed[start..].to_string()]
}

fn looks_like_signature_unit(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty()
        && !text.contains(['。', '；', '：', '，', '！', '？'])
        && [
            "办公室",
            "局",
            "厅",
            "委员会",
            "政府",
            "公司",
            "中心",
            "院",
            "部",
            "处",
            "科",
            "室",
            "所",
            "站",
            "队",
            "组",
        ]
        .iter()
        .any(|suffix| text.ends_with(suffix))
}

/// 上一行是不是 1. / - 这种列表项。
fn starts_with_list_marker(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        return true;
    }
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    (1..=3).contains(&digits) && trimmed[digits..].starts_with(['.', '．', '、'])
}

/// 列表项短句后接正文：「3.严格信息报送。各单位要于……前报送。」
fn split_list_item_body(line: &str) -> Vec<String> {
    if !starts_with_list_marker(line) {
        return vec![line.to_string()];
    }
    let Some(stop) = line.find('。') else {
        return vec![line.to_string()];
    };
    let end = stop + '。'.len_utf8();
    let rest = line[end..].trim_start();
    if rest.is_empty() {
        vec![line.to_string()]
    } else {
        vec![line[..end].trim_end().to_string(), rest.to_string()]
    }
}

/// 这一行应该新起一段：附件说明、抄送、成文日期。
fn starts_new_paragraph(line: &str) -> bool {
    let trimmed = line.trim_start();
    ["附件：", "附件:", "抄送：", "抄送:"]
        .iter()
        .any(|marker| trimmed.starts_with(marker))
        || is_date_line(trimmed)
}

/// 按给定的字节位置切行，返回各段（空段丢掉）。
fn split_at_cuts(line: &str, mut cuts: Vec<usize>) -> Vec<String> {
    cuts.sort_unstable();
    cuts.dedup();
    if cuts.is_empty() {
        return vec![line.to_string()];
    }
    let mut pieces = Vec::new();
    let mut start = 0;
    for cut in cuts {
        if cut > start {
            let piece = line[start..cut].trim();
            if !piece.is_empty() {
                pieces.push(piece.to_string());
            }
        }
        start = cut;
    }
    let tail = line[start..].trim();
    if !tail.is_empty() {
        pieces.push(tail.to_string());
    }
    pieces
}

// ---------------------------------------------------------------- 版头版记

/// 剥掉文首版头与文末版记。只认高置信度的整行标记；拿不准的保留，宁可少删。
fn strip_layout(blocks: &mut Vec<Block>) {
    // 文首：份号、密级、紧急程度、发文机关标志、文号、签发人。
    let index = 0;
    while index < blocks.len() && index < LAYOUT_SCAN {
        let Some(text) = single_para_text(&blocks[index]) else {
            break;
        };
        if !is_layout_head_line(&text) {
            break;
        }
        blocks.remove(index);
    }
    // 文末：抄送、印发机关与印发日期。
    while let Some(text) = blocks.last().and_then(single_para_text) {
        if !is_layout_tail_line(&text) {
            break;
        }
        blocks.pop();
    }
    // 落款单位 + 成文日期：日期在最后，单位在它上面一行。
    if let Some(text) = blocks.last().and_then(single_para_text)
        && is_date_line(&text)
    {
        blocks.pop();
        if let Some(previous) = blocks.last().and_then(single_para_text)
            && is_signature_unit(&previous)
        {
            blocks.pop();
        }
    }
}

fn is_layout_head_line(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    static RE: OnceLock<[Regex; 3]> = OnceLock::new();
    let [security, document_number, serial] = RE.get_or_init(|| {
        [
            // 「机密」「机密★1年」「秘密★长期」这类密级与保密期限。
            Regex::new(r"^(?:绝密|机密|秘密|内部)(?:★.*)?$").expect("valid security pattern"),
            // 「××发〔2026〕12号」「××函〔2026〕3号」。
            Regex::new(r"[〔\[［]\s*[12]\d{3}\s*[〕\]］].*号").expect("valid number pattern"),
            // 份号：六位数字。
            Regex::new(r"^\d{6}$").expect("valid serial pattern"),
        ]
    });
    security.is_match(text)
        || document_number.is_match(text)
        || serial.is_match(text)
        || matches!(text, "特急" | "加急" | "平急" | "特提")
        || text.starts_with("签发人")
        || is_org_logo_line(text)
}

/// 发文机关标志：「××市人民政府文件」。
fn is_org_logo_line(text: &str) -> bool {
    let text = text.trim();
    chars(text) <= 24
        && text.ends_with("文件")
        && !text.contains(['。', '，', '：', ':'])
        && !text.contains("关于")
}

fn is_layout_tail_line(text: &str) -> bool {
    let text = text.trim();
    text.starts_with("抄送")
        || text.starts_with("印发")
        || (text.ends_with("印发") && chars(text) <= 40)
}

/// 成文日期：阿拉伯数字（2026年5月10日）或中文数字（二〇二六年五月十日）。
fn is_date_line(text: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^(?:[0-9０-９]{4}年[0-9０-９]{1,2}月[0-9０-９]{1,2}日|[〇零一二三四五六七八九]{4}年[一二三四五六七八九十]{1,3}月[一二三四五六七八九十]{1,3}日)$",
        )
        .expect("valid date pattern")
    })
    .is_match(text.trim())
}

/// 落款单位行：短、不带句读，且不是「特此通知」这类结语。
fn is_signature_unit(text: &str) -> bool {
    let text = text.trim();
    (2..=25).contains(&chars(text))
        && !text.contains(['。', '，', '；', '：', ':', '！', '？'])
        && !text.starts_with("附件")
        && !text.starts_with(['（', '('])
        && ![
            "特此", "妥否", "以上", "请", "拟", "此致", "敬礼", "注", "说明",
        ]
        .iter()
        .any(|prefix| text.starts_with(prefix))
}

// ---------------------------------------------------------------- 非标准写法

/// 段内「一是……；二是……」拆成一项一行，并把引子（到第一个逗号 / 句号）加粗。
fn split_inline_enumerations(blocks: &mut Vec<Block>) {
    let mut output = Vec::with_capacity(blocks.len());
    for block in blocks.drain(..) {
        match block {
            Block::Para(lines) => {
                let text = para_text(&lines);
                let parts = split_enumeration(&text);
                if parts.len() > 1 {
                    output.extend(parts.into_iter().map(|part| Block::Para(vec![part])));
                } else {
                    output.push(Block::Para(lines));
                }
            }
            other => output.push(other),
        }
    }
    *blocks = output;
}

fn split_enumeration(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut starts = Vec::new();
    for index in 0..chars.len() {
        if !matches!(
            chars[index],
            '一' | '二' | '三' | '四' | '五' | '六' | '七' | '八' | '九' | '十'
        ) {
            continue;
        }
        if chars.get(index + 1) != Some(&'是') {
            continue;
        }
        let at_sentence_start = index == 0
            || matches!(
                chars[index - 1],
                '。' | '；' | '！' | '？' | '：' | '，' | ',' | '\n' | ' '
            );
        if at_sentence_start {
            starts.push(index);
        }
    }
    if starts.len() < 2 {
        return vec![text.to_string()];
    }
    let mut parts = Vec::new();
    if starts[0] > 0 {
        let prefix: String = chars[..starts[0]].iter().collect();
        let prefix = prefix.trim().to_string();
        if !prefix.is_empty() {
            parts.push(prefix);
        }
    }
    for (position, &start) in starts.iter().enumerate() {
        let end = starts.get(position + 1).copied().unwrap_or(chars.len());
        let segment: String = chars[start..end].iter().collect();
        parts.push(bold_enumeration_lead(&segment));
    }
    parts
}

/// 「一是加强组织领导，……」→「**一是加强组织领导**，……」。
fn bold_enumeration_lead(segment: &str) -> String {
    const MARKER: usize = 2; // 「一是」「二是」这类引子都是两个汉字。
    let segment = segment.trim();
    let marker_bytes: usize = segment
        .chars()
        .take(MARKER)
        .map(char::len_utf8)
        .sum::<usize>();
    if marker_bytes == 0 {
        return segment.to_string();
    }
    let body = &segment[marker_bytes.min(segment.len())..];
    let stop = body
        .char_indices()
        .find(|(_, ch)| matches!(ch, '，' | ',' | '。' | '；' | ';' | '：' | ':' | '、'))
        .map(|(offset, _)| offset);
    match stop {
        Some(stop) => {
            let lead = segment[..marker_bytes + stop].trim_end();
            format!("**{lead}**{}", &segment[marker_bytes + stop..])
        }
        None => segment.to_string(),
    }
}

/// 没有编号的短标题加粗：只在已经有编号标题的公文里认，避免讲话稿的短行被误加粗。
fn bold_unnumbered_titles(blocks: &mut [Block]) {
    let has_numbered_heading = blocks
        .iter()
        .any(|block| matches!(block, Block::Item(item) if item.split.is_some()));
    if !has_numbered_heading {
        return;
    }
    for index in 0..blocks.len() {
        let Some(text) = single_para_text(&blocks[index]) else {
            continue;
        };
        if !is_unnumbered_title(&text) {
            continue;
        }
        let next_len = blocks
            .get(index + 1)
            .and_then(single_para_text)
            .map(|text| chars(&text))
            .unwrap_or(0);
        if next_len < RUN_IN_BODY_MIN {
            continue;
        }
        blocks[index] = Block::Para(vec![format!("**{}**", text.trim())]);
    }
}

fn is_unnumbered_title(text: &str) -> bool {
    let text = text.trim();
    let count = chars(text);
    (2..=UNNUMBERED_TITLE_MAX).contains(&count)
        && !text.contains([
            '。', '，', '；', '：', '！', '？', '、', '的', '了', '着', '过', '是', '在', '就',
            '也', '都', '将', '被', '把', '对', '从', '向', '于', '而', '且', '或', '与', '和',
            '并', '及', '请', '拟', '各',
        ])
        && !ends_with_punctuation(text)
        && !text.starts_with(['（', '('])
        && !text.starts_with("第")
        && !text.starts_with("附件")
        && ![
            "特此", "妥否", "以上", "现将", "根据", "按照", "为了", "此致", "敬礼", "注", "要",
            "应", "需", "须", "可", "必", "凡", "当前", "目前", "近期", "今天", "下面",
        ]
        .iter()
        .any(|prefix| text.starts_with(prefix))
}

/// 条文里的「第X条」加粗成引子，避免整条降成标题后被程序重新编号。
fn bold_article_labels(blocks: &mut [Block]) {
    for block in blocks.iter_mut() {
        let Block::Para(lines) = block else {
            continue;
        };
        let Some(first) = lines.first() else {
            continue;
        };
        let text = first.trim();
        if text.starts_with("**") {
            continue;
        }
        let Some(label_len) = article_label_len(text) else {
            continue;
        };
        let label = &text[..label_len];
        let rest = &text[label_len..];
        lines[0] = format!("**{label}**{rest}");
    }
}

fn article_label_len(text: &str) -> Option<usize> {
    let mut chars = text.char_indices();
    let (_, first) = chars.next()?;
    if first != '第' {
        return None;
    }
    let mut end = '第'.len_utf8();
    let mut count = 0usize;
    for (offset, ch) in chars {
        if is_numeral_char(ch) {
            end = offset + ch.len_utf8();
            count += 1;
        } else {
            break;
        }
    }
    if count == 0 || !text[end..].starts_with('条') {
        return None;
    }
    Some(end + '条'.len_utf8())
}

fn is_numeral_char(ch: char) -> bool {
    matches!(
        ch,
        '一' | '二'
            | '三'
            | '四'
            | '五'
            | '六'
            | '七'
            | '八'
            | '九'
            | '十'
            | '百'
            | '千'
            | '零'
            | '〇'
            | '0'..='9'
            | '０'..='９'
    )
}

// ---------------------------------------------------------------- 定性

/// 给每个编号段定性：标题、列表还是原样保留。
fn classify(blocks: &[Block]) -> Vec<Role> {
    let mut roles = vec![Role::Literal; blocks.len()];
    let mut groups: Vec<Vec<usize>> = Vec::new();
    // 下标是层级：记着该级正在进行的那一组与上一个序号。
    let mut open: [Option<(usize, u32)>; 7] = [None; 7];
    for (index, block) in blocks.iter().enumerate() {
        match block {
            Block::Item(item) => {
                let level = item.level as usize;
                for slot in open.iter_mut().skip(level + 1) {
                    *slot = None;
                }
                match item.level {
                    0..=2 if item.split.is_some() => roles[index] = Role::Heading,
                    3 | 4 => match open[level] {
                        Some((group, last)) if item.number == last + 1 => {
                            groups[group].push(index);
                            open[level] = Some((group, item.number));
                        }
                        _ => {
                            groups.push(vec![index]);
                            open[level] = Some((groups.len() - 1, item.number));
                        }
                    },
                    5 => roles[index] = Role::List,
                    _ => {}
                }
            }
            Block::Heading(..) | Block::Attachment(_) => open = [None; 7],
            _ => {}
        }
    }
    for group in groups {
        let role = if group_is_heading(blocks, &group) {
            Role::Heading
        } else {
            Role::List
        };
        for index in group {
            roles[index] = role;
        }
    }
    roles
}

/// 一组同级的 `1.` / `（1）`：是标题还是干条。
fn group_is_heading(blocks: &[Block], group: &[usize]) -> bool {
    let items: Vec<&Item> = group
        .iter()
        .filter_map(|&index| match &blocks[index] {
            Block::Item(item) => Some(item),
            _ => None,
        })
        .collect();
    if items.iter().any(|item| item.styled) {
        return true;
    }
    if items.iter().any(|item| item.split.is_none()) {
        return false;
    }
    // 除最后一项外，有项下面还挂着正文或下级编号：这是分节，不是干条。最后一项
    // 后面的正文可能是上一级的收尾语（「以上材料请于……前报送。」），不算数。
    let (last, others) = group.split_last().expect("group is never empty");
    if others.iter().any(|&index| has_content_after(blocks, index)) {
        return true;
    }
    if others.is_empty() {
        let item = items[0];
        if has_content_after(blocks, *last) && !item.split.as_ref().is_some_and(|s| s.run_in) {
            return true;
        }
    }
    // 每项都是「短标题。长正文」：接排的三级标题。
    items.iter().all(|item| {
        item.split.as_ref().is_some_and(|split| {
            split.run_in
                && chars(&split.title) <= RUN_IN_TITLE_MAX
                && split.body.iter().map(|line| chars(line)).sum::<usize>() >= RUN_IN_BODY_MIN
        })
    })
}

/// 编号段后面紧跟的是不是它自己的内容（正文段、表格、下级编号）。
fn has_content_after(blocks: &[Block], index: usize) -> bool {
    let Block::Item(item) = &blocks[index] else {
        return false;
    };
    match blocks.get(index + 1) {
        Some(Block::Item(next)) => next.level > item.level,
        Some(Block::Para(_) | Block::Raw(_)) => true,
        _ => false,
    }
}

// ---------------------------------------------------------------- 输出

fn render(blocks: &[Block], roles: &[Role]) -> String {
    let heading_levels = heading_level_map(blocks, roles);
    // 每个单元之间空一行；同一组列表项挤在一个单元里，中间不能空行。
    let mut units: Vec<String> = Vec::new();
    let mut previous_list: Option<u8> = None;
    for (block, &role) in blocks.iter().zip(roles) {
        let mut list_level = None;
        let unit = match block {
            // 折行的段落接回一行：中文之间不补空格，西文词间补一个空格。
            Block::Para(lines) => join_pieces(lines),
            Block::Raw(lines) => lines.join("\n"),
            Block::Heading(level, text) => format!("{} {text}", "#".repeat(*level)),
            Block::Attachment(title) => format!("<!-- [附件] -->\n\n# {title}"),
            Block::Item(item) => match (role, &item.split) {
                (Role::Heading, Some(split)) => {
                    let level = heading_levels.get(&item.level).copied().unwrap_or(2);
                    let hashes = "#".repeat(level as usize);
                    let body = join_pieces(&split.body);
                    if body.is_empty() {
                        format!("{hashes} {}", split.title)
                    } else {
                        format!("{hashes} {}\n{body}", split.title)
                    }
                }
                (Role::List, _) => {
                    list_level = Some(item.level);
                    format!("{}. {}", item.number.max(1), join_pieces(&item.rest))
                }
                _ => item.source.join("\n"),
            },
        };
        match units.last_mut() {
            Some(last) if list_level.is_some() && list_level == previous_list => {
                last.push('\n');
                last.push_str(&unit);
            }
            _ => units.push(unit),
        }
        previous_list = list_level;
    }
    units.join("\n\n")
}

/// 把全篇实际用到的编号层级压紧成连续的 `##`…`#####`：样式按从浅到深排，
/// 第一层保持它原本的固定层级，后面每层紧跟上一层加一，避免「## 下面直接出现
/// ####」这种跳级。只出现 `1.` 的文档仍从 `####` 开始（作者只用了这一级）。
fn heading_level_map(blocks: &[Block], roles: &[Role]) -> BTreeMap<u8, u8> {
    let mut ranks: BTreeSet<u8> = BTreeSet::new();
    for (block, role) in blocks.iter().zip(roles) {
        if let (Block::Item(item), Role::Heading) = (block, role) {
            ranks.insert(item.level);
        }
    }
    let mut map = BTreeMap::new();
    let mut previous: Option<u8> = None;
    for rank in ranks {
        let level = match previous {
            None => match rank {
                0 | 1 => 2,
                2 => 3,
                3 => 4,
                _ => 5,
            },
            Some(previous) => (previous + 1).min(5),
        };
        map.insert(rank, level);
        previous = Some(level);
    }
    map
}

/// 把几行接成一行：中文之间直接相连，西文 / 数字之间补一个空格。
fn join_pieces(pieces: &[String]) -> String {
    let mut out = String::new();
    for piece in pieces.iter().map(|piece| piece.trim()) {
        if piece.is_empty() {
            continue;
        }
        let needs_space = out
            .chars()
            .last()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
            && piece.starts_with(|ch: char| ch.is_ascii_alphanumeric());
        if needs_space {
            out.push(' ');
        }
        out.push_str(piece);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(markdown: &str) -> String {
        restructure(markdown, &Options::import(true, false))
    }

    #[test]
    fn chinese_numbered_paragraphs_become_headings_with_run_in_body() {
        let raw = "一、总体要求\n\n坚持稳中求进。\n\n\
                   （一）加强组织领导。各单位要高度重视，明确责任分工。\n\n\
                   （二）工作目标：到2027年基本建成。\n\n\
                   **二、主要任务**";
        assert_eq!(
            clean(raw),
            "## 总体要求\n\n坚持稳中求进。\n\n\
             ### 加强组织领导\n各单位要高度重视，明确责任分工。\n\n\
             ### 工作目标\n到2027年基本建成。\n\n\
             ## 主要任务"
        );
    }

    #[test]
    fn long_first_sentence_stays_a_paragraph() {
        let raw = "一、根据市委市政府关于进一步加强和改进新形势下安全生产工作的部署要求和全年工作安排，现将有关事项通知如下。";
        assert_eq!(clean(raw), raw);
        // 冒号前带逗号的是正文引语，不是标题。
        let raw = "一、根据工作安排，现将有关事项通知如下：";
        assert_eq!(clean(raw), raw);
    }

    #[test]
    fn short_items_become_a_list() {
        let raw = "请报送以下材料：\n\n1.工作总结；\n\n2.统计报表；\n\n3.典型案例。\n\n以上材料请于5月10日前报送。";
        assert_eq!(
            clean(raw),
            "请报送以下材料：\n\n1. 工作总结；\n2. 统计报表；\n3. 典型案例。\n\n以上材料请于5月10日前报送。"
        );
    }

    #[test]
    fn items_with_paragraphs_under_them_become_level_three_headings() {
        let raw = "（一）重点任务\n\n1.排查隐患\n\n各单位要全面排查。\n\n\
                   2.整改落实\n\n对排查出的问题限期整改。";
        assert_eq!(
            clean(raw),
            "### 重点任务\n\n#### 排查隐患\n\n各单位要全面排查。\n\n\
             #### 整改落实\n\n对排查出的问题限期整改。"
        );
    }

    #[test]
    fn run_in_items_with_long_bodies_become_headings() {
        let raw = "1.加强组织领导。各单位要高度重视，成立工作专班，明确责任分工，确保各项任务落到实处。\n\n\
                   2.强化经费保障。财政部门要统筹安排专项资金，加强资金监管，提高资金使用效益和管理水平。";
        assert_eq!(
            clean(raw),
            "#### 加强组织领导\n各单位要高度重视，成立工作专班，明确责任分工，确保各项任务落到实处。\n\n\
             #### 强化经费保障\n财政部门要统筹安排专项资金，加强资金监管，提高资金使用效益和管理水平。"
        );
    }

    #[test]
    fn colon_items_with_short_bodies_stay_a_list() {
        let raw = "1.会议时间：5月10日上午9时。\n\n2.会议地点：市政府3楼会议室。";
        assert_eq!(
            clean(raw),
            "1. 会议时间：5月10日上午9时。\n2. 会议地点：市政府3楼会议室。"
        );
    }

    #[test]
    fn sub_items_make_their_parent_a_heading() {
        let raw = "1.检查内容\n\n（1）制度建设情况；\n\n（2）台账管理情况。\n\n2.检查方式\n\n采取实地查看方式。";
        assert_eq!(
            clean(raw),
            "#### 检查内容\n\n1. 制度建设情况；\n2. 台账管理情况。\n\n#### 检查方式\n\n采取实地查看方式。"
        );
    }

    #[test]
    fn word_auto_numbering_labels_map_to_levels() {
        let raw = "- 1、 总体要求\n\n正文。\n\n- （1） 指导思想\n\n正文。\n\n\
                   1\\. 排查隐患\n\n- （1） 制度建设\n- （2） 台账管理";
        assert_eq!(
            clean(raw),
            "## 总体要求\n\n正文。\n\n### 指导思想\n\n正文。\n\n\
             #### 排查隐患\n\n1. 制度建设\n2. 台账管理"
        );
    }

    #[test]
    fn styled_headings_are_relevelled_by_their_numbering() {
        let raw =
            "# 关于做好防汛工作的通知\n\n# 一、总体要求\n\n正文。\n\n## （一）指导思想\n\n正文。";
        assert_eq!(
            clean(raw),
            "# 关于做好防汛工作的通知\n\n## 总体要求\n\n正文。\n\n### 指导思想\n\n正文。"
        );
    }

    #[test]
    fn document_title_is_detected_and_joined() {
        let raw = "关于做好2026年防汛工作\n\n的通知\n\n各区县人民政府：\n\n一、总体要求";
        assert_eq!(
            clean(raw),
            "# 关于做好2026年防汛工作的通知\n\n各区县人民政府：\n\n## 总体要求"
        );
        let off = restructure(
            "关于做好防汛工作的通知\n\n正文。",
            &Options {
                detect_title: false,
                ..Default::default()
            },
        );
        assert_eq!(off, "关于做好防汛工作的通知\n\n正文。");
    }

    #[test]
    fn attachment_label_becomes_marker() {
        let raw = "正文。\n\n附件1\n\n2026年防汛物资清单\n\n一、物资种类\n\n正文。";
        assert_eq!(
            clean(raw),
            "正文。\n\n<!-- [附件] -->\n\n# 2026年防汛物资清单\n\n## 物资种类\n\n正文。"
        );
    }

    #[test]
    fn enumerations_and_decimals_are_not_numbering() {
        for raw in [
            "一、二季度全市经济运行平稳。",
            "2、3月份降水偏多。",
            "1.5亿元资金已下达。",
            "3.2.1 技术路线",
        ] {
            assert_eq!(clean(raw), raw, "{raw}");
        }
    }

    #[test]
    fn bold_lead_run_is_the_title() {
        let raw = "**（一）坚持问题导向，突出工作重点。**各地要聚焦薄弱环节。";
        assert_eq!(
            clean(raw),
            "### 坚持问题导向，突出工作重点\n各地要聚焦薄弱环节。"
        );
    }

    #[test]
    fn soft_return_separates_title_from_body() {
        let raw = "一、总体要求\\\n坚持稳中求进工作总基调。";
        assert_eq!(clean(raw), "## 总体要求\n坚持稳中求进工作总基调。");
    }

    #[test]
    fn tables_code_and_markers_are_untouched() {
        let raw = "| 1. 序号 | 名称 |\n| --- | --- |\n| 1 | 甲 |\n\n```\n一、不是标题\n```\n\n<!-- [附件] -->";
        assert_eq!(clean(raw), raw);
    }

    #[test]
    fn plain_text_lines_are_paragraphs() {
        let raw = "关于开展安全检查的通知\n各单位：\n一、检查范围\n全市所有生产经营单位。";
        let cleaned = restructure(raw, &Options::import(true, true));
        assert_eq!(
            cleaned,
            "# 关于开展安全检查的通知\n\n各单位：\n\n## 检查范围\n\n全市所有生产经营单位。"
        );
    }

    #[test]
    fn circled_items_are_always_a_list() {
        let raw = "检查内容包括：\n\n①制度建设；\n\n②台账管理。";
        assert_eq!(clean(raw), "检查内容包括：\n\n1. 制度建设；\n2. 台账管理。");
    }

    /// 清洗的产物要能被起草页解析器读成预期的块：标题、列表、段落。
    #[test]
    fn output_parses_as_expected_blocks() {
        use crate::export::{MarkdownBlock, parse_markdown};
        let cleaned = clean(
            "一、总体要求\n\n（一）加强组织领导。各单位要高度重视。\n\n\
             请报送以下材料：\n\n1.工作总结；\n\n2.统计报表。",
        );
        let blocks = parse_markdown(&cleaned);
        let headings = blocks
            .iter()
            .filter(|block| matches!(block, MarkdownBlock::Heading(..)))
            .count();
        let items = blocks
            .iter()
            .filter(|block| matches!(block, MarkdownBlock::OrderedListItem { .. }))
            .count();
        assert_eq!((headings, items), (2, 2), "{cleaned}\n{blocks:#?}");
    }

    #[test]
    fn inline_enumeration_splits_and_bolds_each_lead() {
        let raw = "会议强调，一是加强组织领导，明确责任分工；二是强化督导检查，确保任务落实；\
                   三是严格考核问责，倒逼工作落地。";
        assert_eq!(
            clean(raw),
            "会议强调，\n\n**一是加强组织领导**，明确责任分工；\n\n\
             **二是强化督导检查**，确保任务落实；\n\n\
             **三是严格考核问责**，倒逼工作落地。"
        );
    }

    #[test]
    fn unnumbered_short_heading_is_bolded() {
        let raw = "一、总体要求\n\n各单位要高度重视，明确责任分工，确保各项任务落到实处。\n\n\
                   工作目标\n\n到2027年基本建成，重点领域和关键环节取得实质性进展，形成一批可复制可推广的经验做法。";
        assert_eq!(
            clean(raw),
            "## 总体要求\n\n各单位要高度重视，明确责任分工，确保各项任务落到实处。\n\n\
             **工作目标**\n\n到2027年基本建成，重点领域和关键环节取得实质性进展，形成一批可复制可推广的经验做法。"
        );
    }

    #[test]
    fn article_labels_are_bolded_not_promoted() {
        let raw = "第一条　为规范管理，制定本办法。\n\n第二条　本办法适用于本单位。";
        assert_eq!(
            clean(raw),
            "**第一条**　为规范管理，制定本办法。\n\n**第二条**　本办法适用于本单位。"
        );
    }

    #[test]
    fn layout_head_and_tail_are_stripped() {
        let raw = "000123\n\n秘密★1年\n\n××市人民政府文件\n\n\
                   ×政发〔2026〕12号\n\n签发人：张三\n\n\
                   关于做好防汛工作的通知\n\n各区县人民政府：\n\n正文。\n\n\
                   ××市人民政府办公室\n\n2026年5月10日\n\n\
                   抄送：市委办公室。\n\n××市人民政府办公室　2026年5月10日印发";
        assert_eq!(
            clean(raw),
            "# 关于做好防汛工作的通知\n\n各区县人民政府：\n\n正文。"
        );
    }

    #[test]
    fn chapter_headings_are_recognized() {
        let raw = "第一章 总则\n\n正文。\n\n一、总体要求\n\n正文。";
        assert_eq!(clean(raw), "## 总则\n\n正文。\n\n### 总体要求\n\n正文。");
    }

    #[test]
    fn full_width_digits_and_half_paren_numbering_are_recognized() {
        assert_eq!(
            parse_prefix("１．总体要求", false).map(|prefix| prefix.level),
            Some(3)
        );
        assert_eq!(
            parse_prefix("1)具体事项", false).map(|prefix| prefix.level),
            Some(4)
        );
        assert_eq!(
            parse_prefix("第一章　总则", false).map(|prefix| prefix.level),
            Some(0)
        );
        assert!(parse_prefix("第一条　为规范管理", false).is_none());
    }

    #[test]
    fn skipped_third_level_is_compressed_under_its_parent() {
        let raw = "一、总体要求\n\n正文。\n\n1.具体任务\n\n正文。";
        assert_eq!(
            clean(raw),
            "## 总体要求\n\n正文。\n\n### 具体任务\n\n正文。"
        );
    }

    #[test]
    fn same_line_attachment_and_title_suffix_are_handled() {
        let raw = "关于做好防汛工作的通知\n\n（征求意见稿）\n\n正文。\n\n\
                   附件1：2026年防汛物资清单\n\n一、物资种类\n\n正文。";
        assert_eq!(
            clean(raw),
            "# 关于做好防汛工作的通知（征求意见稿）\n\n正文。\n\n\
             <!-- [附件] -->\n\n# 2026年防汛物资清单\n\n## 物资种类\n\n正文。"
        );
    }

    #[test]
    fn attachment_description_line_is_dropped_when_content_follows() {
        let raw = "正文。\n\n附件：1.物资清单 2.责任分工\n\n附件1\n\n物资清单\n\n内容。";
        assert_eq!(
            clean(raw),
            "正文。\n\n<!-- [附件] -->\n\n# 物资清单\n\n内容。"
        );
    }

    #[test]
    fn ribbon_restructure_keeps_title_and_layout() {
        // 对已有稿子做结构整理：不认标题、不剥版头版记、不拆段内枚举。
        let raw = "关于做好防汛工作的通知\n\n一、总体要求\n\n一是加强领导；二是强化督导。";
        assert_eq!(
            restructure(raw, &Options::restructure_only()),
            "关于做好防汛工作的通知\n\n## 总体要求\n\n一是加强领导；二是强化督导。"
        );
    }
}
