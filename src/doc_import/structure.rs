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
//!   两行折开的标题先接起来。
//! - 单独一行的「附件」「附件2」后接一行短标题，改成 `<!-- [附件] -->` + `# 标题`。
//!
//! 剥掉的编号由程序按层级重新生成，输出里不留原编号；认不准的段落原样保留，
//! 宁可少认也不错认。

use regex::Regex;
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
}

/// 按公文版式还原标题、列表、文档标题与附件标记。输入是归一化过的 markdown。
pub(crate) fn restructure(markdown: &str, options: &Options) -> String {
    let mut blocks = parse_blocks(markdown, options.lines_are_paragraphs);
    resolve_ambiguous_levels(&mut blocks);
    if options.detect_title {
        promote_title(&mut blocks);
    }
    mark_attachments(&mut blocks);
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
    /// 1 = `一、`，2 = `（一）`，3 = `1.`，4 = `（1）`，5 = `①`。
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
    let title = plain[prefix.len..].trim().to_string();
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
    let mut rest = vec![plain_first[prefix.len..].trim().to_string()];
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
    static PATTERNS: OnceLock<[Regex; 5]> = OnceLock::new();
    let [chinese, chinese_paren, arabic, arabic_paren, circled] = PATTERNS.get_or_init(|| {
        let numeral = "[一二三四五六七八九十百零〇]{1,4}";
        [
            Regex::new(&format!(r"^({numeral})[、，,．.]\s*")),
            Regex::new(&format!(r"^[（(]({numeral})[）)]\s*")),
            Regex::new(r"^(\d{1,3})([.．、])\s*"),
            Regex::new(r"^[（(](\d{1,3})[）)]\s*"),
            Regex::new(r"^([①-⑳])\s*"),
        ]
        .map(|re| re.expect("valid numbering pattern"))
    });
    let prefix = if let Some(caps) = chinese.captures(text) {
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
        if text[caps[0].len()..].starts_with(|ch: char| ch.is_ascii_digit())
            && caps[0].len() == caps[1].len() + caps[2].len()
        {
            return None;
        }
        Prefix {
            level: if from_list && &caps[2] == "、" { 1 } else { 3 },
            number: caps[1].parse().ok()?,
            len: caps[0].len(),
            ambiguous: false,
        }
    } else if let Some(caps) = arabic_paren.captures(text) {
        Prefix {
            level: 4,
            number: caps[1].parse().ok()?,
            len: caps[0].len(),
            ambiguous: from_list,
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

/// 「一、二季度」「2、3月份」：顿号连着的是并列的数，不是编号。
fn is_enumeration(rest: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(?:[一二三四五六七八九十]+|\d+)[、，,季月年日号]")
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

/// 开头像公文标题的段落升成 `# `。只看第一个编号段之前的几段。
fn promote_title(blocks: &mut Vec<Block>) {
    if blocks
        .iter()
        .any(|block| matches!(block, Block::Heading(1, _)))
    {
        return;
    }
    let mut index = 0;
    while index < blocks.len().min(8) {
        let text = match &blocks[index] {
            Block::Para(lines) => para_text(lines),
            Block::Raw(_) => {
                index += 1;
                continue;
            }
            _ => return,
        };
        // 两行折开的标题：「关于做好××工作」「的通知」。
        if let Some(Block::Para(next)) = blocks.get(index + 1) {
            let next = para_text(next);
            let joined = format!("{text}{next}");
            if chars(&next) <= 20 && !ends_with_kind(&text) && is_doc_title(&joined) {
                blocks[index] = Block::Heading(1, joined);
                blocks.remove(index + 1);
                return;
            }
        }
        if is_doc_title(&text) {
            blocks[index] = Block::Heading(1, text);
            return;
        }
        index += 1;
    }
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

/// 单独一行的「附件」「附件2」后接一行短标题 → 附件起点。
fn mark_attachments(blocks: &mut Vec<Block>) {
    let mut index = 0;
    while index + 1 < blocks.len() {
        let label = match &blocks[index] {
            Block::Para(lines) if lines.len() == 1 => is_attachment_label(&strip_bold(&lines[0])),
            _ => false,
        };
        let title = match &blocks[index + 1] {
            Block::Para(lines) if label => Some(para_text(lines)),
            _ => None,
        };
        if let Some(title) = title.filter(|title| {
            chars(title) <= DOC_TITLE_MAX && !title.contains('。') && !ends_with_punctuation(title)
        }) {
            blocks[index] = Block::Attachment(title);
            blocks.remove(index + 1);
        }
        index += 1;
    }
}

fn is_attachment_label(text: &str) -> bool {
    let text = text.trim().trim_end_matches(['：', ':']);
    text.strip_prefix("附件")
        .is_some_and(|number| number.trim().chars().all(|ch| ch.is_ascii_digit()))
}

// ---------------------------------------------------------------- 定性

/// 给每个编号段定性：标题、列表还是原样保留。
fn classify(blocks: &[Block]) -> Vec<Role> {
    let mut roles = vec![Role::Literal; blocks.len()];
    let mut groups: Vec<Vec<usize>> = Vec::new();
    // 下标是层级：记着该级正在进行的那一组与上一个序号。
    let mut open: [Option<(usize, u32)>; 6] = [None; 6];
    for (index, block) in blocks.iter().enumerate() {
        match block {
            Block::Item(item) => {
                let level = item.level as usize;
                for slot in open.iter_mut().skip(level + 1) {
                    *slot = None;
                }
                match item.level {
                    1 | 2 if item.split.is_some() => roles[index] = Role::Heading,
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
            Block::Heading(..) | Block::Attachment(_) => open = [None; 6],
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
    // 每个单元之间空一行；同一组列表项挤在一个单元里，中间不能空行。
    let mut units: Vec<String> = Vec::new();
    let mut previous_list: Option<u8> = None;
    for (block, &role) in blocks.iter().zip(roles) {
        let mut list_level = None;
        let unit = match block {
            Block::Para(lines) | Block::Raw(lines) => lines.join("\n"),
            Block::Heading(level, text) => format!("{} {text}", "#".repeat(*level)),
            Block::Attachment(title) => format!("<!-- [附件] -->\n\n# {title}"),
            Block::Item(item) => match (role, &item.split) {
                (Role::Heading, Some(split)) => {
                    let hashes = "#".repeat(item.level.min(4) as usize + 1);
                    let mut lines = vec![format!("{hashes} {}", split.title)];
                    lines.extend(split.body.iter().cloned());
                    lines.join("\n")
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
        restructure(
            markdown,
            &Options {
                detect_title: true,
                lines_are_paragraphs: false,
            },
        )
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
                lines_are_paragraphs: false,
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
        let cleaned = restructure(
            raw,
            &Options {
                detect_title: true,
                lines_are_paragraphs: true,
            },
        );
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
}
