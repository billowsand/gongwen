mod docx;
mod latex;
mod table;
mod title;

use crate::models::{DraftInput, ExportSelection, FontConfig, TemplateKind, split_units};
use crate::units::UnitDisplay;
use anyhow::{Context, Result};
use regex::Regex;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

/// 主发文单位在联合单位列表中的位置；未指定或不在列表中时返回 `None`。
/// 代章直接跟在主发文单位后面，三处渲染器用同一个索引定位单位名。
pub(crate) fn joint_main_index(input: &DraftInput) -> Option<usize> {
    let units = split_units(&input.profile.joint_issuing_units);
    let main = input.profile.main_issuing_unit.trim();
    if units.is_empty() || main.is_empty() {
        return None;
    }
    units.iter().position(|unit| unit == main)
}

/// 当前稿件实际承载代章属性的单位：联合发文取主发文单位，其他公函取落款单位，
/// 落款单位为空时回落发文单位。是否标注由标准词库中的单位属性唯一决定。
/// 代章只对公函生效——其他文种不盖章，无需标注。
pub(crate) fn seals_on_behalf(input: &DraftInput, display: &UnitDisplay) -> bool {
    if input.kind != TemplateKind::OfficialLetter {
        return false;
    }
    let unit = if input.profile.joint_issuance_mode == crate::models::JointIssuanceMode::Mode1 {
        let units = split_units(&input.profile.joint_issuing_units);
        let main = input.profile.main_issuing_unit.trim();
        if units.iter().any(|unit| unit == main) {
            main.to_string()
        } else {
            units.first().cloned().unwrap_or_default()
        }
    } else if !input.profile.signing_unit.trim().is_empty() {
        input.profile.signing_unit.trim().to_string()
    } else {
        input.profile.issuing_unit.trim().to_string()
    };
    display.seals_on_behalf(&unit)
}

/// 联合发文中实际需要追加“（代章）”的单位位置（联合发文仅公函）。旧稿若未保存主发文单位，
/// 与红头规则一致回落到第一个联合发文单位。
pub(crate) fn joint_seal_index(input: &DraftInput, display: &UnitDisplay) -> Option<usize> {
    if !seals_on_behalf(input, display) {
        return None;
    }
    joint_main_index(input)
        .or_else(|| (!split_units(&input.profile.joint_issuing_units).is_empty()).then_some(0))
}

/// 联合发文落款：主发文单位所在列（0=左列，1=右列）。预览/LaTeX/DOCX 三处共用，
/// 把成文日期（含代章）压在主发文单位下方而不是整块居中。
/// 返回 `None` 表示主单位不在联合单位列表中、未指定，或是跨两列的最后一个单位，
/// 此时日期仍整体居中。
pub(crate) fn joint_main_column(input: &DraftInput) -> Option<usize> {
    let units = split_units(&input.profile.joint_issuing_units);
    let index = joint_main_index(input)?;
    // 超过两个且为奇数时最后一个单位跨两列居中（规格 §2.5），日期随之居中。
    let odd_last = units.len() > 2 && units.len() % 2 == 1;
    if odd_last && index + 1 == units.len() {
        return None;
    }
    Some(index % 2)
}

pub fn extract_title(markdown: &str, fallback: &str) -> String {
    markdown
        .lines()
        .find_map(|line| line.trim().strip_prefix("# "))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if fallback.trim().is_empty() {
                "未命名公文"
            } else {
                fallback.trim()
            }
        })
        .to_string()
}

/// Markdown 保留“标题 + 正文 + 可选附件”。密级、文号、主送、落款、成文日期、抄送和版记
/// 都由 DOCX/LaTeX 导出器按锁定元数据渲染，这里不再重复写入。
pub fn finalize_markdown(input: &DraftInput, generated: &str) -> String {
    let mut text = generated.trim().to_string();
    if !text.lines().any(|line| line.starts_with("# ")) {
        let title = if input.title_hint.trim().is_empty() {
            "【待核实：标题】"
        } else {
            input.title_hint.trim()
        };
        text = format!("# {title}\n\n{text}");
    }
    format!("{}\n", text.trim())
}

pub fn export_all(
    output_dir: &Path,
    input: &DraftInput,
    markdown: &str,
    selection: &ExportSelection,
    display: &UnitDisplay,
    fonts: &FontConfig,
) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("无法创建输出目录：{}", output_dir.display()))?;
    let title = extract_title(markdown, &input.title_hint);
    // 按文稿类型生成统一主干名，三格式及编译出的 PDF 共用，方便归档对应：
    // 会议议程“名称+会议时间”，白头件“白头+名称+时间戳”，公函“函号+名称+时间戳”，
    // 电话通知“电话通知+时间戳”，普通公文“普通公文+名称+时间戳”。
    let stem = document_stem(input, &title);
    // 每次导出以文件生成名为单元归档：同一批的 md/docx/tex/pdf
    // 以及 TeX 类文件都放进同名子目录。不覆盖时在目录名上统一编号，
    // 避免各扩展名分别寻找可用名称后落到不同版本。
    let export_stem = if selection.overwrite {
        stem
    } else {
        unique_directory_stem(output_dir, &stem)
    };
    let document_dir = output_dir.join(&export_stem);
    fs::create_dir_all(&document_dir)
        .with_context(|| format!("无法创建文稿导出目录：{}", document_dir.display()))?;
    let mut files = Vec::new();

    if selection.markdown {
        let path = document_dir.join(format!("{export_stem}.md"));
        fs::write(&path, markdown)?;
        // 图片复制到导出目录（保持 images/ 相对结构），导出的 md 目录自包含。
        crate::images::copy_refs(markdown, &document_dir)?;
        files.push(path);
    }
    if selection.docx {
        let path = document_dir.join(format!("{export_stem}.docx"));
        docx::write_docx(&path, input, markdown, display)?;
        files.push(path);
    }
    if selection.tex {
        let path = document_dir.join(format!("{export_stem}.tex"));
        latex::write_tex(&path, input, markdown, display, fonts)?;
        files.push(path);
    }
    Ok(files)
}

/// 导出文件名的固定前缀（不含分钟级时间戳）。导出目录里属于同一文稿的
/// 文件夹都以它为前缀，工具栏“打开最近导出”靠它识别当前文稿的文件夹：
/// - 会议议程：名称（或 名称 + 会议时间，两者都不带时间戳，前缀即完整主干）；
/// - 白头件：`白头` + 名称；
/// - 公函：函号 + 名称（草稿期未编序号时回落机关代字，均缺失时用“公函”）；
/// - 电话通知：`电话通知`；
/// - 普通公文：`普通公文` + 名称。
pub(crate) fn document_stem_prefix(input: &DraftInput, title: &str) -> String {
    let title = safe_filename(title);
    match input.kind {
        TemplateKind::MeetingAgenda => {
            if input.meeting_time.trim().is_empty() {
                title
            } else {
                format!("{title}-{}", safe_filename(&input.meeting_time))
            }
        }
        TemplateKind::WhitePaper => format!("白头-{title}"),
        TemplateKind::OfficialLetter => format!("{}-{title}", letter_prefix(input)),
        TemplateKind::PhoneNotice => "电话通知".to_string(),
        TemplateKind::PlainDocument => format!("普通公文-{title}"),
    }
}

/// 导出文件名的主干按文稿类型区分，`title` 已由 `extract_title` 取好：
/// 会议议程没有时间戳，其余类型在固定前缀后追加分钟级时间戳。
pub(crate) fn document_stem(input: &DraftInput, title: &str) -> String {
    // 时间戳为分钟级，同一分钟内反复导出保持同名，由 overwrite/同名编号决定是否覆盖。
    let timestamp = chrono::Local::now().format("%Y%m%d%H%M").to_string();
    let prefix = document_stem_prefix(input, title);
    match input.kind {
        TemplateKind::MeetingAgenda => prefix,
        _ => format!("{prefix}-{timestamp}"),
    }
}

/// 公函文件名的文号前缀：机关代字〔年〕序号号；草稿期未编序号时回落机关代字。
fn letter_prefix(input: &DraftInput) -> String {
    let code = input.profile.department_code.trim();
    let serial = input.profile.document_number.trim();
    if code.is_empty() && serial.is_empty() {
        return "公函".to_string();
    }
    if serial.is_empty() {
        return safe_filename(code);
    }
    let year = input.document_year();
    if code.is_empty() {
        return format!("{}号", safe_filename(serial));
    }
    let code = safe_filename(code);
    let serial = safe_filename(serial);
    if year.is_empty() {
        format!("{code}{serial}号")
    } else {
        format!("{code}〔{year}〕{serial}号")
    }
}

pub fn safe_filename(value: &str) -> String {
    let mut name = value
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect::<String>();
    name = name.trim().trim_end_matches('.').to_string();
    if name.chars().count() > 80 {
        name = name.chars().take(80).collect();
    }
    if name.is_empty() {
        "未命名公文".into()
    } else {
        name
    }
}

fn unique_directory_stem(dir: &Path, stem: &str) -> String {
    let candidate = dir.join(stem);
    if !candidate.exists() {
        return stem.to_string();
    }
    for index in 2..1000 {
        let versioned_stem = format!("{stem}-{index}");
        let candidate = dir.join(&versioned_stem);
        if !candidate.exists() {
            return versioned_stem;
        }
    }
    format!("{stem}-{}", chrono::Local::now().timestamp())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MarkdownBlock {
    Title(String),
    Heading(u8, String),
    Paragraph(String),
    ListItem(String),
    Table(Vec<Vec<String>>),
    Marker(MarkdownSection),
    Html(String),
    /// 独占一行的图片引用 `![alt](src)`，`src` 为相对路径（如 `images/xxx.png`）。
    Image {
        alt: String,
        src: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkdownSection {
    Body,
    Attachment,
}

/// 一个 Markdown 块，外加它在源码中的字节范围。界面预览据此在点击版式时
/// 回跳到对应的 Markdown 原文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocatedBlock {
    pub(crate) block: MarkdownBlock,
    pub(crate) range: std::ops::Range<usize>,
}

pub(crate) fn parse_markdown(markdown: &str) -> Vec<MarkdownBlock> {
    parse_markdown_located(markdown)
        .into_iter()
        .map(|located| located.block)
        .collect()
}

/// 逐行切分并记录每行的起始字节；行内容与 `str::lines` 一致（去掉行尾 \r\n）。
fn source_lines(markdown: &str) -> Vec<(usize, &str)> {
    let mut lines = Vec::new();
    let mut offset = 0usize;
    for piece in markdown.split_inclusive('\n') {
        let line = piece.strip_suffix('\n').unwrap_or(piece);
        let line = line.strip_suffix('\r').unwrap_or(line);
        lines.push((offset, line));
        offset += piece.len();
    }
    lines
}

/// 与 `parse_markdown` 同一套规则，另外记录每个块占用的源码字节范围：
/// 段落是合并的那几行，表格是表头到最后一行数据，其余就是它自己那一行。
pub(crate) fn parse_markdown_located(markdown: &str) -> Vec<LocatedBlock> {
    let mut blocks: Vec<LocatedBlock> = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();
    let mut paragraph_range = 0..0;
    let mut in_html_block = false;
    let flush = |paragraph: &mut Vec<String>,
                 range: &mut std::ops::Range<usize>,
                 blocks: &mut Vec<LocatedBlock>| {
        if !paragraph.is_empty() {
            blocks.push(LocatedBlock {
                block: MarkdownBlock::Paragraph(paragraph.join(" ")),
                range: range.clone(),
            });
            paragraph.clear();
        }
    };

    let lines = source_lines(markdown);
    let mut index = 0usize;
    while index < lines.len() {
        let (start, raw) = lines[index];
        let line = raw.trim();
        let span = start..start + raw.len();
        if in_html_block {
            blocks.push(LocatedBlock {
                block: MarkdownBlock::Html(line.to_string()),
                range: span,
            });
            if line.starts_with("</div") {
                in_html_block = false;
            }
            index += 1;
            continue;
        }
        if line.is_empty() {
            flush(&mut paragraph, &mut paragraph_range, &mut blocks);
        } else if let Some(section) = parse_section_marker(line) {
            flush(&mut paragraph, &mut paragraph_range, &mut blocks);
            blocks.push(LocatedBlock {
                block: MarkdownBlock::Marker(section),
                range: span,
            });
        } else if let Some((level, text)) = parse_heading(line) {
            flush(&mut paragraph, &mut paragraph_range, &mut blocks);
            let block = if level == 1 {
                MarkdownBlock::Title(text.to_string())
            } else {
                MarkdownBlock::Heading(level, clean_heading_number(text))
            };
            blocks.push(LocatedBlock { block, range: span });
        } else if let Some((alt, src)) = parse_image(line) {
            flush(&mut paragraph, &mut paragraph_range, &mut blocks);
            blocks.push(LocatedBlock {
                block: MarkdownBlock::Image { alt, src },
                range: span,
            });
        } else if line.starts_with("<div") {
            flush(&mut paragraph, &mut paragraph_range, &mut blocks);
            blocks.push(LocatedBlock {
                block: MarkdownBlock::Html(line.to_string()),
                range: span,
            });
            in_html_block = true;
        } else if index + 1 < lines.len()
            && is_table_row(line)
            && is_table_separator(lines[index + 1].1.trim())
        {
            flush(&mut paragraph, &mut paragraph_range, &mut blocks);
            let mut rows = vec![parse_table_row(line)];
            let mut end = lines[index + 1].0 + lines[index + 1].1.len();
            index += 2;
            while index < lines.len() && is_table_row(lines[index].1.trim()) {
                rows.push(parse_table_row(lines[index].1.trim()));
                end = lines[index].0 + lines[index].1.len();
                index += 1;
            }
            let column_count = rows.first().map_or(0, Vec::len);
            for row in &mut rows {
                row.resize(column_count, String::new());
                row.truncate(column_count);
            }
            blocks.push(LocatedBlock {
                block: MarkdownBlock::Table(rows),
                range: start..end,
            });
            continue;
        } else if let Some(text) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            flush(&mut paragraph, &mut paragraph_range, &mut blocks);
            blocks.push(LocatedBlock {
                block: MarkdownBlock::ListItem(format!("• {}", text.trim())),
                range: span,
            });
        } else {
            if paragraph.is_empty() {
                paragraph_range = span.clone();
            }
            paragraph_range.end = span.end;
            paragraph.push(line.to_string());
        }
        index += 1;
    }
    flush(&mut paragraph, &mut paragraph_range, &mut blocks);
    normalize_legacy_attachments(blocks)
}

/// 把旧版附件语法转换为统一的内部结构：
///
/// ```text
/// <!-- [附件] -->       <!-- [附件] -->
/// # 附件1          =>  # 附件正式标题
/// ## 附件正式标题      ## 附件内一级标题
/// ### 附件内一级标题
/// ```
///
/// 新版中每个附件标记就是一份附件的起点，附件标题和内容标题使用与正文相同的
/// Markdown 层级。这里只做读取期兼容，不改写用户原文；预览和导出因此只需维护
/// 一套规范结构。
fn normalize_legacy_attachments(blocks: Vec<LocatedBlock>) -> Vec<LocatedBlock> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum LegacyState {
        None,
        AwaitTitle,
        Content,
    }

    let mut out = Vec::with_capacity(blocks.len());
    let mut in_attachment = false;
    let mut at_attachment_start = false;
    let mut legacy = LegacyState::None;

    for mut located in blocks {
        match &located.block {
            MarkdownBlock::Marker(MarkdownSection::Attachment) => {
                in_attachment = true;
                at_attachment_start = true;
                legacy = LegacyState::None;
                out.push(located);
            }
            MarkdownBlock::Marker(MarkdownSection::Body) => {
                in_attachment = false;
                at_attachment_start = false;
                legacy = LegacyState::None;
                out.push(located);
            }
            MarkdownBlock::Title(text)
                if in_attachment && legacy_attachment_label(text).is_some() =>
            {
                // 旧格式只在第一份附件前放一个区段标记，后续 `# 附件N` 同时承担
                // 分隔作用；规范化时为后续附件补一个仅用于内部处理的标记。
                if !at_attachment_start {
                    out.push(LocatedBlock {
                        block: MarkdownBlock::Marker(MarkdownSection::Attachment),
                        range: located.range.start..located.range.start,
                    });
                }
                at_attachment_start = false;
                if let Some(name) = attachment_title_name(text) {
                    located.block = MarkdownBlock::Title(name);
                    out.push(located);
                    legacy = LegacyState::Content;
                } else {
                    legacy = LegacyState::AwaitTitle;
                }
            }
            MarkdownBlock::Heading(2, text)
                if in_attachment && legacy == LegacyState::AwaitTitle =>
            {
                located.block = MarkdownBlock::Title(text.clone());
                out.push(located);
                legacy = LegacyState::Content;
                at_attachment_start = false;
            }
            MarkdownBlock::Heading(level, text)
                if in_attachment && legacy == LegacyState::Content && *level > 2 =>
            {
                located.block = MarkdownBlock::Heading(*level - 1, text.clone());
                out.push(located);
                at_attachment_start = false;
            }
            _ => {
                if in_attachment && !matches!(located.block, MarkdownBlock::Html(_)) {
                    at_attachment_start = false;
                }
                out.push(located);
            }
        }
    }
    out
}

fn is_table_row(line: &str) -> bool {
    line.contains('|') && parse_table_row(line).len() >= 2
}

fn is_table_separator(line: &str) -> bool {
    let cells = parse_table_row(line);
    cells.len() >= 2
        && cells.iter().all(|cell| {
            let value = cell.trim().trim_matches(':');
            value.len() >= 3 && value.chars().all(|ch| ch == '-')
        })
}

fn parse_table_row(line: &str) -> Vec<String> {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn parse_heading(line: &str) -> Option<(u8, &str)> {
    let hashes = line.chars().take_while(|ch| *ch == '#').count();
    if !(1..=6).contains(&hashes)
        || !line
            .as_bytes()
            .get(hashes)
            .is_some_and(u8::is_ascii_whitespace)
    {
        return None;
    }
    Some((hashes as u8, line[hashes..].trim()))
}

/// 识别独占一行的图片引用 `![alt](src)`。`src` 不含空白与右括号（与
/// `images::image_refs` 的正则约束一致），返回 `(alt, src)`。
fn parse_image(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("![")?;
    let close = rest.find(']')?;
    let alt = rest[..close].to_string();
    let src = rest[close + 1..]
        .strip_prefix('(')?
        .strip_suffix(')')?
        .trim();
    if src.is_empty() || src.contains(char::is_whitespace) {
        return None;
    }
    Some((alt, src.to_string()))
}

/// 整行是否恰好是一张图片引用（与解析器的独占行识别一致）。
/// 实时排版编辑器据此把图片行高亮为占位提示。
pub(crate) fn is_image_line(line: &str) -> bool {
    parse_image(line.trim()).is_some()
}

/// 识别 `<!-- [正文] -->`、`<!-- [附件] -->`（含【】与英文变体）这类独占一行的区段标记。
/// 实时排版编辑器（draft_page / highlight）也复用它，以便在区段切换处重置公文标题计数器。
pub(crate) fn parse_section_marker(line: &str) -> Option<MarkdownSection> {
    let inner = line
        .trim()
        .strip_prefix("<!--")?
        .strip_suffix("-->")?
        .trim()
        .trim_start_matches(['[', '【'])
        .trim_end_matches([']', '】'])
        .trim();
    match inner.to_ascii_lowercase().as_str() {
        "正文" | "body" => Some(MarkdownSection::Body),
        "附件" | "附录" | "attachment" | "attachments" | "appendix" => {
            Some(MarkdownSection::Attachment)
        }
        _ => None,
    }
}

/// 正文区最大标题层级（# 号最多的那一级），紧缩风格（规格 §4.2）据此把该级标题与正文合并。
/// 文档标题（`#`）与附件区标题不计入；附件区标题使用独立排版与计数器。
pub(crate) fn body_heading_max_level(blocks: &[MarkdownBlock]) -> u8 {
    let mut section = MarkdownSection::Body;
    let mut max_level = 0u8;
    for block in blocks {
        match block {
            MarkdownBlock::Marker(next) => section = *next,
            MarkdownBlock::Heading(level, _) if section == MarkdownSection::Body => {
                max_level = max_level.max(*level);
            }
            _ => {}
        }
    }
    max_level
}

/// 与 mdx 公文转换保持一致：标题编号由导出器统一生成，先清掉模型或人工写入的旧编号。
fn clean_heading_number(text: &str) -> String {
    const PATTERNS: &[&str] = &[
        r"^附录\s*[A-Za-z0-9]+(?:[.\-][A-Za-z0-9]+)*\s*[、.．:：]?\s*",
        r"^(?i:appendix)\s*[A-Za-z0-9]+(?:[.\-][A-Za-z0-9]+)*\s*[、.．:：]?\s*",
        r"^第[一二三四五六七八九十百零\d]+[章节条部分]\s*[、.．]?\s*",
        r"^[（(][一二三四五六七八九十百零]+[）)]\s*[、.．]?\s*",
        r"^[一二三四五六七八九十百零]+[、,.．]\s*",
        r"^[（(]\d+[）)]\s*[、.．]?\s*",
        r"^\(\d+\)\s*[、.．]?\s*",
        r"^\d+(?:\.\d+)+[.．]?\s*",
        r"^\d+[.．、]\s+",
        r"^\d+\s+",
    ];
    static REGEXES: OnceLock<Vec<Regex>> = OnceLock::new();
    let mut cleaned = text.to_string();
    for regex in REGEXES.get_or_init(|| {
        PATTERNS
            .iter()
            .map(|pattern| Regex::new(pattern).expect("valid heading pattern"))
            .collect()
    }) {
        cleaned = regex.replace(&cleaned, "").to_string();
    }
    cleaned.trim().to_string()
}

/// 公文正文各级标题的编号：一、 →（一）→ 1. →（1）。DOCX 导出与界面预览共用，
/// 保证预览里看到的编号就是导出后的编号。
pub(crate) fn official_heading_text(
    level: u8,
    text: &str,
    counters: &mut [usize; 4],
) -> Option<String> {
    official_heading_prefix(level, counters).map(|prefix| format!("{prefix}{text}"))
}

/// 只生成公文标题编号前缀。实时排版编辑器不能把自动编号真正写进
/// Markdown，因此用这个共用函数在屏幕上叠加，导出时仍由同一套计数器生成。
pub(crate) fn official_heading_prefix(level: u8, counters: &mut [usize; 4]) -> Option<String> {
    match level {
        2 => {
            counters[0] += 1;
            counters[1..].fill(0);
            Some(format!("{}、", number_to_chinese(counters[0])))
        }
        3 => {
            counters[1] += 1;
            counters[2..].fill(0);
            Some(format!("（{}）", number_to_chinese(counters[1])))
        }
        4 => {
            counters[2] += 1;
            counters[3] = 0;
            Some(format!("{}.", counters[2]))
        }
        5 => {
            counters[3] += 1;
            Some(format!("({})", counters[3]))
        }
        _ => None,
    }
}

/// 逐行推进标题计数器并返回该行叠加的编号前缀，供实时排版编辑器使用。
/// 规则与导出器（docx/latex）和预览完全一致：
/// - 区段标记（正文/附件）处切换区段并重置计数器；
/// - 正文第一个 `#` 与每个附件标记后的 `#` 都是正式标题；
/// - 正文和附件的 `##` 及以下使用完全相同的编号层级；
/// - 非标题行返回 None 且不推进计数器。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HeadingCounters {
    levels: [usize; 4],
    expecting_title: bool,
    in_attachment: bool,
    legacy_attachment: bool,
    /// 最近一次 `next` 处理的行是否为正式标题（文档标题或附件正式标题），
    /// 需要按方正小标宋二号居中渲染，与预览/导出一致。
    centered_title: bool,
}

impl Default for HeadingCounters {
    fn default() -> Self {
        Self {
            levels: [0; 4],
            expecting_title: true,
            in_attachment: false,
            legacy_attachment: false,
            centered_title: false,
        }
    }
}

impl HeadingCounters {
    pub(crate) fn next(&mut self, line: &str) -> Option<String> {
        self.centered_title = false;
        if let Some(section) = parse_section_marker(line) {
            self.levels = [0; 4];
            self.expecting_title = true;
            self.in_attachment = section == MarkdownSection::Attachment;
            self.legacy_attachment = false;
            return None;
        }
        let trimmed = line.trim_start();
        let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
        let is_heading = (1..=6).contains(&hashes)
            && trimmed
                .as_bytes()
                .get(hashes)
                .is_some_and(u8::is_ascii_whitespace);
        if !is_heading {
            return None;
        }
        if hashes == 1 {
            let text = trimmed[hashes..].trim();
            if self.in_attachment && legacy_attachment_label(text).is_some() {
                self.levels = [0; 4];
                self.expecting_title = true;
                self.legacy_attachment = true;
                return None;
            }
            self.centered_title = self.expecting_title;
            self.expecting_title = false;
            self.legacy_attachment = false;
            return None;
        }
        if self.legacy_attachment && self.expecting_title && hashes == 2 {
            self.centered_title = true;
            self.expecting_title = false;
            return None;
        }
        let level = if self.legacy_attachment {
            hashes - 1
        } else {
            hashes
        };
        official_heading_prefix(level as u8, &mut self.levels)
    }

    /// 最近一次 `next` 处理的行是否为正式标题（方正小标宋二号居中渲染）。
    pub(crate) fn centered_title(&self) -> bool {
        self.centered_title
    }
}

/// 表格的一列在版心中所占的比例，以及是否居中。界面预览据此复用导出器的
/// 智能列宽，保证预览里的列宽与导出的 Word 表格一致。
pub(crate) struct TableColumn {
    /// 占版心宽度的比例，各列相加为 1。
    pub(crate) fraction: f32,
    pub(crate) centered: bool,
}

pub(crate) fn table_columns(rows: &[Vec<String>]) -> Vec<TableColumn> {
    let (grid, alignments) =
        table::to_docx_grid(rows, docx::TABLE_CONTENT_WIDTH_TWIPS, docx::TABLE_SIZE * 10);
    let total = grid.iter().sum::<usize>().max(1) as f32;
    grid.iter()
        .enumerate()
        .map(|(index, width)| TableColumn {
            fraction: *width as f32 / total,
            centered: alignments.get(index) == Some(&table::ColumnAlignment::Center),
        })
        .collect()
}

pub(crate) fn number_to_chinese(number: usize) -> String {
    const DIGITS: [&str; 11] = [
        "", "一", "二", "三", "四", "五", "六", "七", "八", "九", "十",
    ];
    match number {
        0..=10 => DIGITS[number].to_string(),
        11..=19 => format!("十{}", DIGITS[number - 10]),
        20..=99 if number.is_multiple_of(10) => format!("{}十", DIGITS[number / 10]),
        20..=99 => format!("{}十{}", DIGITS[number / 10], DIGITS[number % 10]),
        _ => number.to_string(),
    }
}

pub(crate) fn plain_text(text: &str) -> String {
    text.replace("**", "").replace("__", "").replace('`', "")
}

/// 把正文中的直引号、方向错误或风格混杂的引号统一为正确配对的中文引号。
fn normalize_chinese_quotes(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut double_open = true;
    let mut single_open = true;
    let chars = text.chars().collect::<Vec<_>>();
    for (index, ch) in chars.iter().copied().enumerate() {
        if matches!(ch, '"' | '＂' | '“' | '”' | '„' | '‟' | '「' | '」') {
            output.push(if double_open { '“' } else { '”' });
            double_open = !double_open;
        } else if matches!(ch, '\'' | '＇' | '‘' | '’' | '‚' | '‛' | '『' | '』') {
            let apostrophe = index > 0
                && index + 1 < chars.len()
                && chars[index - 1].is_ascii_alphanumeric()
                && chars[index + 1].is_ascii_alphanumeric();
            if apostrophe {
                output.push('’');
            } else {
                output.push(if single_open { '‘' } else { '’' });
                single_open = !single_open;
            }
        } else {
            output.push(ch);
        }
    }
    output
}

fn parenthesized_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut outer_start = None;
    for (index, ch) in text.char_indices() {
        let expected = match ch {
            '（' => Some('）'),
            '(' => Some(')'),
            '【' => Some('】'),
            _ => None,
        };
        if let Some(expected) = expected {
            if stack.is_empty() {
                outer_start = Some(index);
            }
            stack.push((expected, index));
            continue;
        }
        if matches!(ch, '）' | ')' | '】') {
            if stack.last().is_some_and(|(expected, _)| *expected == ch) {
                stack.pop();
                if stack.is_empty()
                    && let Some(start) = outer_start.take()
                {
                    ranges.push((start, index + ch.len_utf8()));
                }
            } else {
                // 错配或孤立的右括号不参与字体切换；正在解析的外层也作废。
                stack.clear();
                outer_start = None;
            }
        }
    }
    ranges
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InlineSegment {
    pub(crate) text: String,
    pub(crate) bold: bool,
    pub(crate) parenthesized: bool,
}

/// 解析正文行内 Markdown：保留 `**…**` / `__…__` 的加粗语义，同时叠加括号字体规则。
/// 加粗标记跨越括号边界时仍可正确切分为多个字体一致、粗细一致的片段。
pub(crate) fn inline_segments(text: &str) -> Vec<InlineSegment> {
    let text = normalize_chinese_quotes(&text.replace('`', ""));
    let paren_ranges = parenthesized_ranges(&text);

    // 只移除成对出现的 Markdown 加粗标记；孤立的 `**` / `__` 保留原文。
    let mut paired_markers: std::collections::HashMap<usize, &'static str> =
        std::collections::HashMap::new();
    for (marker, literal) in [("**", "**"), ("__", "__")] {
        let positions = text
            .match_indices(marker)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        for pair in positions.chunks_exact(2) {
            paired_markers.insert(pair[0], literal);
            paired_markers.insert(pair[1], literal);
        }
    }

    let mut segments: Vec<InlineSegment> = Vec::new();
    let mut buffer = String::new();
    let mut star_bold = false;
    let mut underscore_bold = false;
    let mut current_state: Option<(bool, bool)> = None;
    let mut index = 0usize;
    while index < text.len() {
        if let Some(marker) = paired_markers.get(&index) {
            if !buffer.is_empty()
                && let Some((bold, parenthesized)) = current_state
            {
                segments.push(InlineSegment {
                    text: std::mem::take(&mut buffer),
                    bold,
                    parenthesized,
                });
            }
            if *marker == "**" {
                star_bold = !star_bold;
            } else {
                underscore_bold = !underscore_bold;
            }
            current_state = None;
            index += marker.len();
            continue;
        }

        let ch = text[index..].chars().next().expect("valid char boundary");
        let parenthesized = paren_ranges
            .iter()
            .any(|(start, end)| *start <= index && index < *end);
        let state = (star_bold || underscore_bold, parenthesized);
        if current_state.is_some_and(|current| current != state) && !buffer.is_empty() {
            let (bold, parenthesized) = current_state.expect("state exists");
            segments.push(InlineSegment {
                text: std::mem::take(&mut buffer),
                bold,
                parenthesized,
            });
        }
        current_state = Some(state);
        buffer.push(ch);
        index += ch.len_utf8();
    }
    if !buffer.is_empty()
        && let Some((bold, parenthesized)) = current_state
    {
        segments.push(InlineSegment {
            text: buffer,
            bold,
            parenthesized,
        });
    }
    segments
}

/// 从附件标题提取内嵌名称：`附件1：统计表` → `统计表`；`附件1` → None。
fn attachment_title_name(label: &str) -> Option<String> {
    let rest = label.strip_prefix("附件")?;
    let after_number = rest
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .trim_start();
    let name = after_number
        .strip_prefix('：')
        .or_else(|| after_number.strip_prefix(':'))
        .or_else(|| after_number.strip_prefix('、'))
        .map(str::trim)
        .filter(|name| !name.is_empty())?;
    Some(plain_text(name))
}

pub(crate) fn legacy_attachment_label(label: &str) -> Option<()> {
    let rest = label.strip_prefix("附件")?;
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let tail = rest[digits..].trim();
    if tail.is_empty() || tail.starts_with('：') || tail.starts_with(':') || tail.starts_with('、')
    {
        Some(())
    } else {
        None
    }
}

/// 提取各附件正式标题。解析器已把旧格式规范化，因此每个附件标记之后的首个
/// `#` 标题就是名称；没有正式标题的附件不进入正文附件清单。
pub(crate) fn attachment_names(blocks: &[MarkdownBlock]) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_attachment = false;
    let mut pending_name = false;
    for block in blocks {
        match block {
            MarkdownBlock::Marker(section) => {
                in_attachment = matches!(section, MarkdownSection::Attachment);
                pending_name = in_attachment;
            }
            MarkdownBlock::Title(text) if in_attachment && pending_name => {
                names.push(plain_text(text));
                pending_name = false;
            }
            _ => {}
        }
    }
    names
}

/// 解析界面使用的“YYYY年M月D日”日期，供 Word 与 LaTeX 使用同一套预览占位规则。
pub(crate) fn chinese_date_parts(value: &str) -> Option<(&str, &str, &str)> {
    let (year, remainder) = value.trim().split_once('年')?;
    let (month, day) = remainder.split_once('月')?;
    let day = day.trim().trim_end_matches('日').trim();
    Some((year.trim(), month.trim(), day))
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    #[test]
    fn filename_is_windows_safe() {
        assert_eq!(safe_filename("关于A/B:测试?的函"), "关于A_B_测试_的函");
    }

    #[test]
    fn parses_chinese_document_date_parts() {
        assert_eq!(chinese_date_parts("2026年8月5日"), Some(("2026", "8", "5")));
        assert_eq!(chinese_date_parts("待定"), None);
    }

    /// 实时排版编辑器逐行叠加的标题编号：规则与导出器/预览一致——
    /// 区段标记与每个附件标题处都重置计数器，附件区整体上移一级编号。
    #[test]
    fn heading_prefixes_restart_after_section_markers() {
        let mut counters = HeadingCounters::default();
        let lines = [
            "# 测试函",
            "<!-- [正文] -->",
            "## 一、总体要求",
            "### （一）具体事项",
            "正文段落。",
            "<!-- [附件] -->",
            "# 统计表",
            "## 一、填报说明",
            "### 子项",
            "附件内容。",
            "<!-- [附件] -->",
            "# 说明材料",
            "## 二、其他说明",
        ];
        let prefixes = lines
            .iter()
            .map(|line| counters.next(line))
            .collect::<Vec<_>>();
        assert_eq!(prefixes[0], None, "文档标题不编号");
        assert_eq!(prefixes[1], None, "区段标记不编号");
        assert_eq!(prefixes[2].as_deref(), Some("一、"));
        assert_eq!(prefixes[3].as_deref(), Some("（一）"));
        assert_eq!(prefixes[4], None, "正文段落不编号");
        assert_eq!(prefixes[5], None, "附件区段标记不编号");
        assert_eq!(prefixes[6], None, "附件正式标题（#）不编号");
        assert_eq!(
            prefixes[7].as_deref(),
            Some("一、"),
            "附件内容标题与正文同级并从一、起"
        );
        assert_eq!(prefixes[8].as_deref(), Some("（一）"));
        assert_eq!(prefixes[9], None);
        assert_eq!(prefixes[10], None, "第二个附件标记");
        assert_eq!(prefixes[11], None, "第二个附件正式标题");
        assert_eq!(
            prefixes[12].as_deref(),
            Some("一、"),
            "每个附件之后的内容都重新编号"
        );

        // 旧格式仍可用于实时排版，附件内容层级会在显示期上移一级。
        let mut counters = HeadingCounters::default();
        counters.next("# 测试函");
        assert_eq!(counters.next("## 一"), Some("一、".to_string()));
        counters.next("<!-- [附件] -->");
        assert_eq!(counters.next("# 附件1"), None);
        assert_eq!(counters.next("## 统计表"), None);
        assert_eq!(counters.next("### 填报说明").as_deref(), Some("一、"));

        // 正文标记回到正文区，同样重新起算。
        let mut counters = HeadingCounters::default();
        counters.next("# 测试函");
        counters.next("## 一");
        counters.next("## 二");
        assert_eq!(counters.next("<!-- [正文] -->"), None);
        assert_eq!(counters.next("## 重新起算").as_deref(), Some("一、"));
    }

    /// 文档标题与附件正式标题需要按方正小标宋二号居中渲染（centered_title 标志），
    /// 附件标识与内容标题不居中。
    #[test]
    fn formal_titles_are_marked_for_centered_rendering() {
        let mut counters = HeadingCounters::default();
        assert!(!counters.centered_title());
        counters.next("# 测试函");
        assert!(counters.centered_title(), "文档标题需居中");
        counters.next("正文段落。");
        assert!(!counters.centered_title());
        counters.next("<!-- [附件] -->");
        assert!(!counters.centered_title());
        counters.next("# 附件1");
        assert!(!counters.centered_title(), "附件标识不居中");
        counters.next("## 统计表");
        assert!(counters.centered_title(), "附件正式标题需居中");
        counters.next("### 一、填报说明");
        assert!(!counters.centered_title(), "附件内容标题不居中");
    }

    /// 联合发文日期压在主发文单位所在列下方；主单位跨列时整体居中。
    #[test]
    fn joint_main_column_follows_the_main_unit() {
        let mut input = DraftInput::default();
        input.profile.joint_issuing_units = "甲单位、乙单位".into();
        input.profile.main_issuing_unit = "甲单位".into();
        assert_eq!(joint_main_column(&input), Some(0));
        input.profile.main_issuing_unit = "乙单位".into();
        assert_eq!(joint_main_column(&input), Some(1));

        // 超过两个且为奇数时，最后一个单位跨两列，日期随之整体居中。
        input.profile.joint_issuing_units = "甲单位、乙单位、丙单位".into();
        input.profile.main_issuing_unit = "甲单位".into();
        assert_eq!(joint_main_column(&input), Some(0));
        input.profile.main_issuing_unit = "丙单位".into();
        assert_eq!(joint_main_column(&input), None);

        // 主单位未指定或不在列表中时整体居中。
        input.profile.main_issuing_unit = "丁单位".into();
        assert_eq!(joint_main_column(&input), None);
        input.profile.main_issuing_unit.clear();
        assert_eq!(joint_main_column(&input), None);
    }

    #[test]
    fn joint_main_index_locates_the_main_unit() {
        let mut input = DraftInput::default();
        input.profile.joint_issuing_units = "甲单位、乙单位、丙单位".into();
        input.profile.main_issuing_unit = "乙单位".into();
        assert_eq!(joint_main_index(&input), Some(1));
        input.profile.main_issuing_unit = "丁单位".into();
        assert_eq!(joint_main_index(&input), None);
        input.profile.main_issuing_unit.clear();
        assert_eq!(joint_main_index(&input), None);
    }

    /// 每个块记下的字节范围必须正好切出它自己的源码：界面预览点击回跳全靠它。
    #[test]
    fn located_blocks_point_back_at_their_own_source() {
        let markdown = "# 标题\n\n<!-- [正文] -->\n\n第一段首行\n第一段次行\n\n- 列表项\n\n| 甲 | 乙 |\n|---|---|\n| 1 | 2 |\n";
        let located = parse_markdown_located(markdown);
        let slices = located
            .iter()
            .map(|block| &markdown[block.range.clone()])
            .collect::<Vec<_>>();
        assert_eq!(
            slices,
            [
                "# 标题",
                "<!-- [正文] -->",
                // 段落跨行时范围覆盖合并进来的每一行。
                "第一段首行\n第一段次行",
                "- 列表项",
                // 表格从表头一直到最后一行数据。
                "| 甲 | 乙 |\n|---|---|\n| 1 | 2 |",
            ]
        );
    }

    /// 带位置的解析必须与原来的解析给出完全相同的块序列。
    #[test]
    fn located_parsing_matches_plain_parsing() {
        let markdown = "# 标题\n\n正文（含括号）。\n\n## 小标题\n\n<div class=\"x\">\n内容\n</div>\n\n<!-- [附件] -->\n\n# 附件1\n";
        let located = parse_markdown_located(markdown)
            .into_iter()
            .map(|block| block.block)
            .collect::<Vec<_>>();
        assert_eq!(located, parse_markdown(markdown));
    }

    #[test]
    fn parses_basic_blocks() {
        let blocks = parse_markdown("# 标题\n\n## 一、事项\n正文\n\n- 要点");
        assert!(matches!(&blocks[0], MarkdownBlock::Title(t) if t == "标题"));
        assert!(matches!(&blocks[1], MarkdownBlock::Heading(2, t) if t == "事项"));
        assert!(matches!(&blocks[2], MarkdownBlock::Paragraph(_)));
    }

    #[test]
    fn parses_body_and_attachment_markers_and_all_heading_levels() {
        let blocks = parse_markdown(
            "# 函件标题\n<!-- [正文] -->\n## 一、正文事项\n### （一）具体要求\n<!-- [附件] -->\n# 表格\n## 一、说明",
        );
        assert!(matches!(
            blocks[1],
            MarkdownBlock::Marker(MarkdownSection::Body)
        ));
        assert!(matches!(&blocks[2], MarkdownBlock::Heading(2, text) if text == "正文事项"));
        assert!(matches!(&blocks[3], MarkdownBlock::Heading(3, text) if text == "具体要求"));
        assert!(matches!(
            blocks[4],
            MarkdownBlock::Marker(MarkdownSection::Attachment)
        ));
        assert!(matches!(&blocks[5], MarkdownBlock::Title(text) if text == "表格"));
        assert!(matches!(&blocks[6], MarkdownBlock::Heading(2, text) if text == "说明"));

        // 历史稿读取时规范化为相同结构，不要求用户手工迁移。
        let legacy = parse_markdown("# 函件标题\n<!-- [附件] -->\n# 附件1：表格\n### 一、说明");
        assert!(matches!(&legacy[2], MarkdownBlock::Title(text) if text == "表格"));
        assert!(matches!(&legacy[3], MarkdownBlock::Heading(2, text) if text == "说明"));
    }

    #[test]
    fn body_heading_max_level_ignores_title_and_attachment_headings() {
        // 正文区最深 4 级；附件区虽含 5 级标题，但不计入正文最大层级。
        let blocks = parse_markdown(
            "# 函件标题\n## 一、正文事项\n### （一）具体要求\n#### 1. 更细\n<!-- [附件] -->\n# 附件1：表格\n##### 1. 说明",
        );
        assert_eq!(body_heading_max_level(&blocks), 4);
        // 无正文标题时返回 0，紧缩合并自然不触发。
        let blocks = parse_markdown("# 函件标题\n正文。\n<!-- [附件] -->\n## 表一");
        assert_eq!(body_heading_max_level(&blocks), 0);
    }

    #[test]
    fn parses_gfm_table_as_a_single_block() {
        let blocks = parse_markdown(
            "# 附件测试\n| 序号 | 名称 | 说明 |\n| --- | --- | --- |\n| 1 | 甲 | 较长说明 |\n| 2 | 乙 | 另一说明 |",
        );
        assert!(
            matches!(&blocks[1], MarkdownBlock::Table(rows) if rows.len() == 3 && rows[0].len() == 3)
        );
    }

    #[test]
    fn parses_image_reference_as_own_block() {
        let blocks = parse_markdown(
            "# 标题\n正文段落。\n![扫描件](images/20260809_120000_扫描件.png)\n结尾段落。",
        );
        assert!(matches!(
            &blocks[2],
            MarkdownBlock::Image { alt, src }
                if alt.as_str() == "扫描件" && src.as_str() == "images/20260809_120000_扫描件.png"
        ));
        // 图片行独立成块，不并入相邻段落。
        assert!(matches!(&blocks[1], MarkdownBlock::Paragraph(_)));
        assert!(matches!(&blocks[3], MarkdownBlock::Paragraph(_)));
    }

    #[test]
    fn image_parsing_ignores_inline_and_malformed() {
        // 段内出现的 `![..]` 不被当作图片块。
        let blocks = parse_markdown("文中出现 ![图标](x.png) 引用");
        assert!(matches!(&blocks[0], MarkdownBlock::Paragraph(_)));
        // 残缺语法不匹配。
        let blocks = parse_markdown("![缺括号](images/a.png");
        assert!(matches!(&blocks[0], MarkdownBlock::Paragraph(_)));
        // src 含空白的引用不匹配（与 images::image_refs 约束一致）。
        let blocks = parse_markdown("![图](images/my file.png)");
        assert!(matches!(&blocks[0], MarkdownBlock::Paragraph(_)));
    }

    #[test]
    fn exports_all_three_formats() {
        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput::default();
        input.profile.issuing_unit = "某某单位".into();
        input.profile.recipient = "某某部门".into();
        input.profile.responsible_unit = "办公室".into();
        input.profile.contact_person = "张三".into();
        input.profile.contact_phone = "010-12345678".into();
        let markdown =
            "# 关于开展测试工作的函\n\n某某部门：\n\n现就有关事项函告如下。\n\n特此函告。";
        let files = export_all(
            temp.path(),
            &input,
            markdown,
            &ExportSelection::default(),
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        assert_eq!(files.len(), 3);
        assert!(files.iter().all(|path| path.metadata().unwrap().len() > 0));
        let document_dir = files[0].parent().unwrap();
        assert!(files.iter().all(|path| path.parent() == Some(document_dir)));
        assert!(
            files
                .iter()
                .all(|path| { path.file_stem() == document_dir.file_name() })
        );
        assert!(document_dir.join("gonghan-gwa.cls").exists());

        let docx_path = files
            .iter()
            .find(|p| p.extension().is_some_and(|e| e == "docx"))
            .unwrap();
        let file = std::fs::File::open(docx_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut xml = String::new();
        use std::io::Read;
        archive
            .by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        assert!(xml.contains("关于开展测试工作的函"));
    }

    #[test]
    fn markdown_export_survives_missing_image_refs() {
        // 图片引用指向不存在的文件时，md 导出照常成功（copy_refs 跳过缺失文件）。
        let temp = tempfile::tempdir().unwrap();
        let input = DraftInput::default();
        let selection = ExportSelection {
            markdown: true,
            docx: false,
            tex: false,
            overwrite: true,
        };
        let files = export_all(
            temp.path(),
            &input,
            "# 标题\n\n正文。\n\n![图](images/不存在的.png)",
            &selection,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].extension().is_some_and(|ext| ext == "md"));
        assert_eq!(
            std::fs::read_to_string(&files[0]).unwrap(),
            "# 标题\n\n正文。\n\n![图](images/不存在的.png)"
        );
    }

    #[test]
    fn overwrite_reuses_the_same_paths() {
        let temp = tempfile::tempdir().unwrap();
        let input = DraftInput::default();
        let markdown = "# 关于开展测试工作的函\n\n正文。\n";
        let selection = ExportSelection {
            markdown: true,
            docx: false,
            tex: false,
            overwrite: true,
        };
        let first = export_all(
            temp.path(),
            &input,
            markdown,
            &selection,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        let second = export_all(
            temp.path(),
            &input,
            markdown,
            &selection,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        assert_eq!(first, second);

        let versioned = ExportSelection {
            overwrite: false,
            ..selection
        };
        let third = export_all(
            temp.path(),
            &input,
            markdown,
            &versioned,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        assert_ne!(first, third);
        let versioned_dir = third[0].parent().unwrap();
        assert_eq!(third[0].file_stem(), versioned_dir.file_name());
        assert!(
            versioned_dir
                .file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with("-2")
        );
    }

    #[test]
    fn document_stem_is_per_kind() {
        let mut input = DraftInput::default();

        // 会议议程：名称 + 会议时间；时间里的半角冒号被替换为下划线，保证 Windows 安全。
        input.kind = TemplateKind::MeetingAgenda;
        input.meeting_time = "2026年8月5日（星期三）14:30".into();
        assert_eq!(
            document_stem(&input, "开展专题研讨会议"),
            "开展专题研讨会议-2026年8月5日（星期三）14_30"
        );
        // 会议时间留空时回落名称。
        input.meeting_time = "  ".into();
        assert_eq!(
            document_stem(&input, "开展专题研讨会议"),
            "开展专题研讨会议"
        );

        // 白头件：白头 + 名称 + 时间戳，时间戳为 12 位数字。
        input.kind = TemplateKind::WhitePaper;
        let stem = document_stem(&input, "关于解决XXX问题的请示");
        let (prefix, rest) = stem.split_once('-').unwrap();
        assert_eq!(prefix, "白头");
        assert!(rest.starts_with("关于解决XXX问题的请示-"));
        assert_timestamp(rest.rsplit('-').next().unwrap());

        // 电话通知：只有“电话通知” + 时间戳，不带名称。
        input.kind = TemplateKind::PhoneNotice;
        let stem = document_stem(&input, "关于召开会议的通知");
        assert!(stem.starts_with("电话通知-"));
        assert_timestamp(stem.strip_prefix("电话通知-").unwrap());

        // 普通公文：普通公文 + 名称 + 时间戳。
        input.kind = TemplateKind::PlainDocument;
        let stem = document_stem(&input, "工作安排");
        assert!(stem.starts_with("普通公文-工作安排-"));
        assert_timestamp(stem.rsplit('-').next().unwrap());
    }

    #[test]
    fn official_letter_stem_uses_full_document_number() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.department_code = "某政函".into();
        input.profile.document_number = "12".into();
        input.date = "2026年8月6日".into();
        let stem = document_stem(&input, "关于开展测试工作的函");
        let timestamp = stem.rsplit('-').next().unwrap();
        assert_eq!(
            stem.trim_end_matches(&format!("-{timestamp}")),
            "某政函〔2026〕12号-关于开展测试工作的函"
        );
        assert_timestamp(timestamp);
    }

    #[test]
    fn official_letter_without_serial_falls_back_to_code() {
        // 草稿期尚未编发文序号：只带机关代字，仍保留名称与时间戳。
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.department_code = "某政函".into();
        let stem = document_stem(&input, "关于开展测试工作的函");
        assert!(stem.starts_with("某政函-关于开展测试工作的函-"));

        // 代字与序号都缺失：回落“公函”。
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        let stem = document_stem(&input, "关于开展测试工作的函");
        assert!(stem.starts_with("公函-关于开展测试工作的函-"));
    }

    fn assert_timestamp(value: &str) {
        assert_eq!(value.len(), 12, "时间戳应为 12 位：{value}");
        assert!(value.chars().all(|character| character.is_ascii_digit()));
    }

    /// 把正文按"完整括号"切段：全角 `（…）`、半角 `(…)` 或方头括号 `【…】`
    /// 配对出现的整段归为"括号内"，其余归为"括号外"；括号不配对（缺左或缺右）时按普通文本处理。
    /// 仅供测试内对照：生产代码走的是 `inline_segments`（保留 Markdown `**…**` 加粗语义）。
    fn split_parenthesized(text: &str) -> Vec<(&str, bool)> {
        let mut segments = Vec::new();
        let mut normal_start = 0usize;
        for (start, end) in parenthesized_ranges(text) {
            if normal_start < start {
                segments.push((&text[normal_start..start], false));
            }
            segments.push((&text[start..end], true));
            normal_start = end;
        }
        if normal_start < text.len() {
            segments.push((&text[normal_start..], false));
        }
        segments
    }

    #[test]
    fn splits_complete_parentheses_only() {
        assert_eq!(
            split_parenthesized("现就（有关事项）函告如下。"),
            [
                ("现就", false),
                ("（有关事项）", true),
                ("函告如下。", false)
            ]
        );
        assert_eq!(
            split_parenthesized("使用(a)半角与（b）全角。"),
            [
                ("使用", false),
                ("(a)", true),
                ("半角与", false),
                ("（b）", true),
                ("全角。", false)
            ]
        );
        // 不配对的括号按普通文本处理。
        assert_eq!(
            split_parenthesized("缺少（右括号"),
            [("缺少（右括号", false)]
        );
        // 嵌套括号整体算一个括号段。
        assert_eq!(
            split_parenthesized("测试（含（内层）内容）完毕"),
            [
                ("测试", false),
                ("（含（内层）内容）", true),
                ("完毕", false)
            ]
        );
        assert_eq!(
            split_parenthesized("正文【特别说明】继续（办理要求）。"),
            [
                ("正文", false),
                ("【特别说明】", true),
                ("继续", false),
                ("（办理要求）", true),
                ("。", false)
            ]
        );
        assert_eq!(
            split_parenthesized("混合（外层【内层】内容）结束"),
            [
                ("混合", false),
                ("（外层【内层】内容）", true),
                ("结束", false)
            ]
        );
    }

    #[test]
    fn normalizes_quote_styles_and_directions() {
        assert_eq!(
            normalize_chinese_quotes(r#"他说："第一句"，又说：“第二句“。"#),
            "他说：“第一句”，又说：“第二句”。"
        );
        assert_eq!(
            normalize_chinese_quotes("外层「文字『内层』文字」"),
            "外层“文字‘内层’文字”"
        );
        // 英文单词内部的撇号使用中文右单引号，但不参与成对引号方向切换。
        assert_eq!(normalize_chinese_quotes("don't 'quote'"), "don’t ‘quote’");
    }

    #[test]
    fn inline_bold_and_parentheses_can_cross_each_other() {
        assert_eq!(
            inline_segments("正文（**重点**内容）和**【加粗说明】**。"),
            [
                InlineSegment {
                    text: "正文".into(),
                    bold: false,
                    parenthesized: false,
                },
                InlineSegment {
                    text: "（".into(),
                    bold: false,
                    parenthesized: true,
                },
                InlineSegment {
                    text: "重点".into(),
                    bold: true,
                    parenthesized: true,
                },
                InlineSegment {
                    text: "内容）".into(),
                    bold: false,
                    parenthesized: true,
                },
                InlineSegment {
                    text: "和".into(),
                    bold: false,
                    parenthesized: false,
                },
                InlineSegment {
                    text: "【加粗说明】".into(),
                    bold: true,
                    parenthesized: true,
                },
                InlineSegment {
                    text: "。".into(),
                    bold: false,
                    parenthesized: false,
                },
            ]
        );
    }

    #[test]
    fn collects_attachment_names_from_heading_and_inline() {
        // 规范写法：每个附件标记后的 `#` 是正式标题。
        let blocks = parse_markdown(
            "# 测试函\n<!-- [正文] -->\n正文。\n<!-- [附件] -->\n# 统计表\n内容。\n<!-- [附件] -->\n# 说明材料\n内容。",
        );
        assert_eq!(attachment_names(&blocks), ["统计表", "说明材料"]);
        // 历史格式在读取期规范化，附件清单结果保持不变。
        let blocks = parse_markdown("# 测试函\n<!-- [附件] -->\n# 附件1：汇总表\n内容。");
        assert_eq!(attachment_names(&blocks), ["汇总表"]);
        // 没有名称的附件不入列。
        let blocks = parse_markdown(
            "# 测试函\n<!-- [附件] -->\n内容。\n<!-- [附件] -->\n# 说明材料\n内容。",
        );
        assert_eq!(attachment_names(&blocks), ["说明材料"]);
    }
}
