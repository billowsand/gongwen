//! 审校区的 Markdown 语法高亮。
//!
//! 只做“看得清”这一件事：把结构性符号（`#`、`|`、`**`、区段标记）压成弱色，
//! 把真正要读的内容（标题、表格单元、待核实占位）提亮，让一屏文字有层次。
//! 另外给“锚点”——即在公文预览里点中的那一块——铺一层淡底，两栏对照时一眼能
//! 看出版式上的哪一段对应源码里的哪一段。
//! 高亮结果按（文本, 换行宽度, 锚点, 查找命中）缓存，正常编辑时每帧只需一次哈希。

use crate::{export, theme};
use eframe::egui::{
    self, Color32, FontId,
    text::{LayoutJob, TextFormat},
};
use std::{
    hash::{Hash, Hasher},
    ops::Range,
    sync::Arc,
};

/// 挂在 `GongwenApp` 上的高亮缓存。
#[derive(Default)]
pub struct MarkdownHighlighter {
    cache: Option<(u64, u32, Arc<egui::Galley>)>,
    hybrid_cache: Option<(u64, u32, usize, Arc<egui::Galley>)>,
}

impl MarkdownHighlighter {
    /// 供 `TextEdit::layouter` 调用：文本、宽度和锚点都没变时直接复用上一帧的 galley。
    pub fn layout(
        &mut self,
        ui: &egui::Ui,
        text: &str,
        wrap_width: f32,
        anchor: Option<&Range<usize>>,
        search_matches: &[Range<usize>],
    ) -> Arc<egui::Galley> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        anchor.hash(&mut hasher);
        search_matches.hash(&mut hasher);
        let key = hasher.finish();
        let width = wrap_width.to_bits();
        if let Some((cached_key, cached_width, galley)) = &self.cache
            && *cached_key == key
            && *cached_width == width
        {
            return galley.clone();
        }
        let job = highlight(ui.style(), text, wrap_width, anchor, search_matches);
        let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
        self.cache = Some((key, width, galley.clone()));
        galley
    }

    /// “实时排版”编辑器的布局：光标所在行保留 Markdown 标记，
    /// 其余行折叠标记并使用公文字体、字号和固定行距。
    pub fn layout_hybrid(
        &mut self,
        ui: &egui::Ui,
        text: &str,
        wrap_width: f32,
        active_line: usize,
        anchor: Option<&Range<usize>>,
        search_matches: &[Range<usize>],
    ) -> Arc<egui::Galley> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        anchor.hash(&mut hasher);
        search_matches.hash(&mut hasher);
        let key = hasher.finish();
        let width = wrap_width.to_bits();
        if let Some((cached_key, cached_width, cached_line, galley)) = &self.hybrid_cache
            && *cached_key == key
            && *cached_width == width
            && *cached_line == active_line
        {
            return galley.clone();
        }
        let job = hybrid_highlight(
            ui.style(),
            text,
            wrap_width,
            active_line,
            anchor,
            search_matches,
        );
        let mut galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
        center_document_title_rows(&mut galley, text, wrap_width);
        self.hybrid_cache = Some((key, width, active_line, galley.clone()));
        galley
    }
}

fn format(font: FontId, color: Color32) -> TextFormat {
    TextFormat {
        font_id: font,
        color,
        ..Default::default()
    }
}

fn filled(font: FontId, color: Color32, background: Color32) -> TextFormat {
    TextFormat {
        font_id: font,
        color,
        background,
        ..Default::default()
    }
}

const PT: f32 = 96.0 / 72.0;
const OFFICIAL_BODY_PT: f32 = 16.0;
const OFFICIAL_TITLE_PT: f32 = 22.0;
const OFFICIAL_TABLE_PT: f32 = 14.0;
const OFFICIAL_LINE_PT: f32 = 28.0;
const OFFICIAL_TABLE_LINE_PT: f32 = 21.0;
const OFFICIAL_LIST_INDENT_PT: f32 = 21.0;

fn official_format(family: &str, size_pt: f32, line_pt: f32, color: Color32) -> TextFormat {
    TextFormat {
        font_id: FontId::new(size_pt * PT, theme::official_family(family)),
        color,
        line_height: Some(line_pt * PT),
        ..Default::default()
    }
}

fn hidden_marker(line_pt: f32) -> TextFormat {
    TextFormat {
        // TextEdit 要求布局文本与源文逐字对应，因此不能真删标记。
        // 把它压到近乎零宽且透明，既保留光标映射，也不留明显缺口。
        font_id: FontId::new(0.1, egui::FontFamily::Proportional),
        color: Color32::TRANSPARENT,
        line_height: Some(line_pt * PT),
        ..Default::default()
    }
}

fn collapsed_marker() -> TextFormat {
    TextFormat {
        font_id: FontId::new(0.1, egui::FontFamily::Proportional),
        color: Color32::TRANSPARENT,
        line_height: Some(1.0),
        ..Default::default()
    }
}

fn append_with_leading(job: &mut LayoutJob, text: &str, leading: &mut f32, format: TextFormat) {
    if text.is_empty() {
        return;
    }
    job.append(text, *leading, format);
    *leading = 0.0;
}

fn center_document_title_rows(galley: &mut Arc<egui::Galley>, text: &str, width: f32) {
    let title_line = text
        .split('\n')
        .position(|line| !line.trim().is_empty() && line.trim_start().starts_with("# "));
    let Some(title_line) = title_line else {
        return;
    };
    let galley = Arc::make_mut(galley);
    let mut logical_line = 0usize;
    for placed in &mut galley.rows {
        if logical_line == title_line {
            placed.pos.x = ((width - placed.size.x) * 0.5).max(0.0);
        }
        if placed.ends_with_newline {
            logical_line += 1;
            if logical_line > title_line {
                break;
            }
        }
    }
}

/// 把整篇 Markdown 编译成带颜色的 `LayoutJob`；普通查找命中铺黄色底，当前命中
/// （`anchor`）再盖一层强调底色。
pub fn highlight(
    style: &egui::Style,
    text: &str,
    wrap_width: f32,
    anchor: Option<&Range<usize>>,
    search_matches: &[Range<usize>],
) -> LayoutJob {
    let base_size = style
        .text_styles
        .get(&egui::TextStyle::Body)
        .map_or(14.0, |font| font.size);
    let family = egui::FontFamily::Proportional;
    let body = FontId::new(base_size, family.clone());

    let mut job = LayoutJob {
        wrap: egui::text::TextWrapping {
            max_width: wrap_width,
            ..Default::default()
        },
        // 编辑器里空格要能看见，否则光标位置与所见不符。
        keep_trailing_whitespace: true,
        ..Default::default()
    };

    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            job.append("\n", 0.0, format(body.clone(), theme::md::BODY));
        }
        highlight_line(&mut job, line, base_size, &family);
    }
    for range in search_matches {
        paint_range(&mut job, range, theme::md::SEARCH_BG);
    }
    if let Some(anchor) = anchor {
        // 当前命中（或预览点击锚点）最后绘制，以更醒目的强调色盖过普通命中。
        paint_range(&mut job, anchor, theme::md::ANCHOR_BG);
    }
    job
}

/// 把 Markdown 作为唯一数据源的所见即所得布局。这不生成第二份富文本：
/// 每个字符仍在 galley 中，只是非活动行的结构标记被折叠。
pub fn hybrid_highlight(
    _style: &egui::Style,
    text: &str,
    wrap_width: f32,
    active_line: usize,
    anchor: Option<&Range<usize>>,
    search_matches: &[Range<usize>],
) -> LayoutJob {
    let body = official_format(
        theme::FONT_FANGSONG,
        OFFICIAL_BODY_PT,
        OFFICIAL_LINE_PT,
        Color32::BLACK,
    );
    let mut job = LayoutJob {
        wrap: egui::text::TextWrapping {
            max_width: wrap_width,
            ..Default::default()
        },
        keep_trailing_whitespace: true,
        ..Default::default()
    };

    let source_lines = text.split('\n').collect::<Vec<_>>();
    let mut counters = [0usize; 4];
    let mut in_table = false;
    for (index, line) in source_lines.iter().copied().enumerate() {
        if index > 0 {
            let previous_is_blank = source_lines[index - 1].trim().is_empty();
            let newline = if previous_is_blank && index - 1 != active_line {
                collapsed_marker()
            } else {
                body.clone()
            };
            job.append("\n", 0.0, newline);
        }
        let trimmed = line.trim_start();
        let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
        let level = if (1..=6).contains(&hashes)
            && trimmed
                .as_bytes()
                .get(hashes)
                .is_some_and(u8::is_ascii_whitespace)
        {
            hashes as u8
        } else {
            0
        };
        let prefix = export::official_heading_prefix(level, &mut counters);
        let trimmed_table = line.trim();
        let table_line = trimmed_table.contains('|') && trimmed_table.matches('|').count() >= 2;
        let table_header = table_line && !in_table;
        in_table = table_line;
        hybrid_line(
            &mut job,
            line,
            index == active_line,
            prefix.as_deref(),
            table_header,
        );
    }
    for range in search_matches {
        paint_range(&mut job, range, theme::md::SEARCH_BG);
    }
    if let Some(anchor) = anchor {
        paint_range(&mut job, anchor, theme::md::ANCHOR_BG);
    }
    job
}

fn hybrid_line(
    job: &mut LayoutJob,
    line: &str,
    active: bool,
    heading_prefix: Option<&str>,
    table_header: bool,
) {
    let body = official_format(
        theme::FONT_FANGSONG,
        OFFICIAL_BODY_PT,
        OFFICIAL_LINE_PT,
        Color32::BLACK,
    );
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        job.append(line, 0.0, body);
        return;
    }
    let indent = &line[..line.len() - trimmed.len()];

    // 附件分隔注释不是成文内容；非活动行整行折叠，聚焦后恢复可编辑源码。
    if trimmed.starts_with("<!--") || trimmed.starts_with('<') {
        let marker = if active {
            let mut format = body;
            format.color = theme::md::COMMENT;
            format.background = theme::md::COMMENT_BG;
            format
        } else {
            hidden_marker(OFFICIAL_LINE_PT)
        };
        job.append(line, 0.0, marker);
        return;
    }

    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    if (1..=6).contains(&hashes)
        && trimmed
            .as_bytes()
            .get(hashes)
            .is_some_and(u8::is_ascii_whitespace)
    {
        let (family, size) = match hashes {
            1 => (theme::FONT_BIAOSONG, OFFICIAL_TITLE_PT),
            2 => (theme::FONT_HEITI, OFFICIAL_BODY_PT),
            3 => (theme::FONT_KAITI, OFFICIAL_BODY_PT),
            _ => (theme::FONT_FANGSONG, OFFICIAL_BODY_PT),
        };
        let text_format = official_format(family, size, OFFICIAL_LINE_PT, Color32::BLACK);
        let marker = if active {
            let mut format = text_format.clone();
            format.color = theme::md::MARKER;
            format
        } else {
            hidden_marker(OFFICIAL_LINE_PT)
        };
        let split = hashes + 1;
        let mut leading = if hashes == 1 {
            // 先按正常段落排版，galley 生成后再逐个视觉行居中。
            // 这样长标题换成两行时，第二行也不会回到版心左边。
            0.0
        } else if active {
            OFFICIAL_BODY_PT * PT * 2.0
        } else {
            OFFICIAL_BODY_PT * PT * 2.0
                + heading_prefix.map_or(0.0, |prefix| {
                    prefix
                        .chars()
                        .map(|ch| if ch.is_ascii() { 0.55 } else { 1.0 })
                        .sum::<f32>()
                        * OFFICIAL_BODY_PT
                        * PT
                })
        };
        append_with_leading(job, indent, &mut leading, text_format.clone());
        append_with_leading(job, &trimmed[..split], &mut leading, marker);
        append_hybrid_inline(job, &trimmed[split..], &text_format, active, &mut leading);
        return;
    }

    // 表格在单个 TextEdit 中无法真正跨行合并列，但字号和行距与成文一致；
    // 离开本行后折叠竖线和分隔线，继续编辑时则显示完整表格源码。
    if trimmed.contains('|') && trimmed.matches('|').count() >= 2 {
        let table = official_format(
            if table_header {
                theme::FONT_HEITI
            } else {
                theme::FONT_FANGSONG
            },
            OFFICIAL_TABLE_PT,
            OFFICIAL_TABLE_LINE_PT,
            Color32::BLACK,
        );
        if is_separator_row(trimmed) && !active {
            job.append(line, 0.0, collapsed_marker());
            return;
        }
        let marker = if active {
            let mut format = table.clone();
            format.color = theme::md::TABLE_PIPE;
            format
        } else {
            hidden_marker(OFFICIAL_TABLE_LINE_PT)
        };
        let mut leading = 0.0;
        append_with_leading(job, indent, &mut leading, table.clone());
        if active {
            for piece in trimmed.split_inclusive('|') {
                let (content, bar) = match piece.strip_suffix('|') {
                    Some(content) => (content, true),
                    None => (piece, false),
                };
                append_hybrid_inline(job, content, &table, true, &mut leading);
                if bar {
                    append_with_leading(job, "|", &mut leading, marker.clone());
                }
            }
        } else {
            let leading_pipe = trimmed.starts_with('|');
            let trailing_pipe = trimmed.ends_with('|');
            let cells_text = trimmed
                .strip_prefix('|')
                .unwrap_or(trimmed)
                .strip_suffix('|')
                .unwrap_or_else(|| trimmed.strip_prefix('|').unwrap_or(trimmed));
            let cells = cells_text.split('|').collect::<Vec<_>>();
            let cell_width = job.wrap.max_width / cells.len().max(1) as f32;
            if leading_pipe {
                job.append("|", 0.0, marker.clone());
            }
            for (index, content) in cells.iter().enumerate() {
                append_hybrid_inline(job, content, &table, false, &mut leading);
                let estimated = content
                    .chars()
                    .map(|ch| if ch.is_ascii() { 0.55 } else { 1.0 })
                    .sum::<f32>()
                    * OFFICIAL_TABLE_PT
                    * PT;
                if index + 1 < cells.len() || trailing_pipe {
                    job.append("|", 0.0, marker.clone());
                }
                leading = (cell_width - estimated).max(4.0);
            }
        }
        return;
    }

    if let Some(rest) = trimmed.strip_prefix("- ").or(trimmed.strip_prefix("* ")) {
        let marker = if active {
            let mut format = body.clone();
            format.color = theme::md::BULLET;
            format
        } else {
            hidden_marker(OFFICIAL_LINE_PT)
        };
        let mut leading = OFFICIAL_LIST_INDENT_PT * PT;
        append_with_leading(job, indent, &mut leading, body.clone());
        append_with_leading(job, &trimmed[..2], &mut leading, marker);
        append_hybrid_inline(job, rest, &body, active, &mut leading);
        return;
    }

    // 普通段落按成文规则首行缩进 2 字。leading_space 只是布局信息，
    // 不会向 Markdown 里真正插入全角空格。
    let mut leading = OFFICIAL_BODY_PT * PT * 2.0;
    append_with_leading(job, indent, &mut leading, body.clone());
    append_hybrid_inline(job, trimmed, &body, active, &mut leading);
}

fn append_hybrid_inline(
    job: &mut LayoutJob,
    text: &str,
    base: &TextFormat,
    active: bool,
    leading: &mut f32,
) {
    let mut plain_start = 0usize;
    let mut index = 0usize;
    while index < text.len() {
        let rest = &text[index..];
        let Some(Span {
            open_len,
            content_end,
            span_end,
            kind,
        }) = inline_span(rest)
        else {
            index += next_char_len(rest);
            continue;
        };
        if plain_start < index {
            append_with_leading(job, &text[plain_start..index], leading, base.clone());
        }
        match kind {
            Inline::Strong => {
                let marker = if active {
                    let mut format = base.clone();
                    format.color = theme::md::MARKER;
                    format
                } else {
                    hidden_marker(OFFICIAL_LINE_PT)
                };
                let strong = official_format(
                    theme::FONT_HEITI,
                    OFFICIAL_BODY_PT,
                    OFFICIAL_LINE_PT,
                    Color32::BLACK,
                );
                append_with_leading(job, &rest[..open_len], leading, marker.clone());
                append_with_leading(job, &rest[open_len..content_end], leading, strong);
                append_with_leading(job, &rest[content_end..span_end], leading, marker);
            }
            Inline::Code => {
                let marker = if active {
                    let mut format = base.clone();
                    format.color = theme::md::MARKER;
                    format
                } else {
                    hidden_marker(OFFICIAL_LINE_PT)
                };
                append_with_leading(job, &rest[..open_len], leading, marker.clone());
                append_with_leading(job, &rest[open_len..content_end], leading, base.clone());
                append_with_leading(job, &rest[content_end..span_end], leading, marker);
            }
            // 待核实占位和中文引号是成文内容，不属于 Markdown 结构标记。
            Inline::Todo | Inline::Quoted => {
                append_with_leading(job, &rest[..span_end], leading, base.clone());
            }
        }
        index += span_end;
        plain_start = index;
    }
    if plain_start < text.len() {
        append_with_leading(job, &text[plain_start..], leading, base.clone());
    }
}

/// 给 `anchor` 覆盖的字节铺底色。分段本来就首尾相接，这里只在边界处把跨界的
/// 那一段切开，保证 `LayoutJob` 的分段仍然完整覆盖原文。
///
/// 锚点是按点击当时的正文算出来的字节范围，正文一改就可能过期。越界或落在
/// 字符中间的锚点一律丢掉——不然分段会从半个汉字中间切开，epaint 排版时 panic。
fn paint_range(job: &mut LayoutJob, anchor: &Range<usize>, background: Color32) {
    if anchor.is_empty()
        || anchor.end > job.text.len()
        || !job.text.is_char_boundary(anchor.start)
        || !job.text.is_char_boundary(anchor.end)
    {
        return;
    }
    let mut sections = Vec::with_capacity(job.sections.len() + 2);
    for section in std::mem::take(&mut job.sections) {
        let (start, end) = (section.byte_range.start.0, section.byte_range.end.0);
        // 与锚点无交集的分段原样保留。
        if end <= anchor.start || anchor.end <= start {
            sections.push(section);
            continue;
        }
        for (piece_start, piece_end, inside) in [
            (start, end.min(anchor.start), false),
            (start.max(anchor.start), end.min(anchor.end), true),
            (start.max(anchor.end), end, false),
        ] {
            if piece_start >= piece_end {
                continue;
            }
            let mut piece = section.clone();
            piece.byte_range = egui::text::ByteIndex(piece_start)..egui::text::ByteIndex(piece_end);
            if inside {
                piece.format.background = background;
            }
            sections.push(piece);
        }
    }
    job.sections = sections;
}

fn highlight_line(job: &mut LayoutJob, line: &str, base_size: f32, family: &egui::FontFamily) {
    let body = FontId::new(base_size, family.clone());
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        job.append(line, 0.0, format(body, theme::md::BODY));
        return;
    }
    let indent = &line[..line.len() - trimmed.len()];
    if !indent.is_empty() {
        job.append(indent, 0.0, format(body.clone(), theme::md::BODY));
    }

    // 区段标记 `<!-- 附件 -->` 与裸 HTML：整行弱化为注释色。
    if trimmed.starts_with("<!--") || trimmed.starts_with('<') {
        job.append(
            trimmed,
            0.0,
            filled(body, theme::md::COMMENT, theme::md::COMMENT_BG),
        );
        return;
    }

    // 标题：`#` 标记弱化，标题文字按层级放大。
    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    if (1..=6).contains(&hashes)
        && trimmed
            .as_bytes()
            .get(hashes)
            .is_some_and(u8::is_ascii_whitespace)
    {
        let scale = match hashes {
            1 => 1.30,
            2 => 1.16,
            3 => 1.07,
            _ => 1.0,
        };
        let font = FontId::new((base_size * scale).round(), family.clone());
        let color = if hashes == 1 {
            theme::md::TITLE
        } else {
            theme::md::HEADING
        };
        let split = hashes + 1;
        job.append(
            &trimmed[..split],
            0.0,
            format(font.clone(), theme::md::MARKER),
        );
        append_inline(job, &trimmed[split..], &format(font, color));
        return;
    }

    // 表格：竖线压成浅色，分隔行整行弱化，单元格内容用独立的蓝灰色。
    if trimmed.contains('|') && trimmed.matches('|').count() >= 2 {
        if is_separator_row(trimmed) {
            job.append(trimmed, 0.0, format(body, theme::md::TABLE_RULE));
            return;
        }
        let cell = format(body.clone(), theme::md::TABLE_CELL);
        let pipe = format(body, theme::md::TABLE_PIPE);
        for piece in trimmed.split_inclusive('|') {
            let (content, bar) = match piece.strip_suffix('|') {
                Some(content) => (content, true),
                None => (piece, false),
            };
            append_inline(job, content, &cell);
            if bar {
                job.append("|", 0.0, pipe.clone());
            }
        }
        return;
    }

    // 列表：项目符号用强调色，内容照常走行内规则。
    if let Some(rest) = trimmed.strip_prefix("- ").or(trimmed.strip_prefix("* ")) {
        job.append(&trimmed[..2], 0.0, format(body.clone(), theme::md::BULLET));
        append_inline(job, rest, &format(body, theme::md::BODY));
        return;
    }

    append_inline(job, trimmed, &format(body, theme::md::BODY));
}

fn is_separator_row(line: &str) -> bool {
    let cells = line
        .trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .collect::<Vec<_>>();
    cells.len() >= 2
        && cells.iter().all(|cell| {
            let value = cell.trim().trim_matches(':');
            value.len() >= 3 && value.chars().all(|ch| ch == '-')
        })
}

/// 行内规则：`**加粗**`、`` `代码` ``、`【待核实：…】`、中文引号。
/// 未命中的部分按 `base` 输出，因此标题、表格单元都能复用这套扫描。
fn append_inline(job: &mut LayoutJob, text: &str, base: &TextFormat) {
    let font = base.font_id.clone();
    let mut plain_start = 0usize;
    let mut index = 0usize;
    while index < text.len() {
        let rest = &text[index..];
        let Some(Span {
            open_len,
            content_end,
            span_end,
            kind,
        }) = inline_span(rest)
        else {
            index += next_char_len(rest);
            continue;
        };
        if plain_start < index {
            job.append(&text[plain_start..index], 0.0, base.clone());
        }
        let (color, background, keep_marks) = match kind {
            Inline::Strong => (theme::md::STRONG, theme::md::STRONG_BG, false),
            Inline::Code => (theme::md::CODE, theme::md::COMMENT_BG, false),
            Inline::Todo => (theme::md::TODO, theme::md::TODO_BG, true),
            Inline::Quoted => (theme::md::QUOTED, Color32::TRANSPARENT, true),
        };
        let content = filled(font.clone(), color, background);
        if keep_marks {
            job.append(&rest[..span_end], 0.0, content);
        } else {
            let marker = format(font.clone(), theme::md::MARKER);
            job.append(&rest[..open_len], 0.0, marker.clone());
            job.append(&rest[open_len..content_end], 0.0, content);
            job.append(&rest[content_end..span_end], 0.0, marker);
        }
        index += span_end;
        plain_start = index;
    }
    if plain_start < text.len() {
        job.append(&text[plain_start..], 0.0, base.clone());
    }
}

#[derive(Clone, Copy)]
enum Inline {
    Strong,
    Code,
    Todo,
    Quoted,
}

/// `rest` 开头那段行内标记在 `rest` 中的位置。
struct Span {
    /// 起始标记的字节长度。
    open_len: usize,
    /// 内容结束（即结束标记开始）的字节位置。
    content_end: usize,
    /// 整段（含结束标记）的字节结束位置。
    span_end: usize,
    kind: Inline,
}

/// 判断 `rest` 是否以一段成对的行内标记开头；标记必须闭合且内容非空。
fn inline_span(rest: &str) -> Option<Span> {
    const PAIRS: [(&str, &str, Inline); 5] = [
        ("**", "**", Inline::Strong),
        ("__", "__", Inline::Strong),
        ("`", "`", Inline::Code),
        ("【", "】", Inline::Todo),
        ("“", "”", Inline::Quoted),
    ];
    for (open, close, kind) in PAIRS {
        if !rest.starts_with(open) {
            continue;
        }
        let body = &rest[open.len()..];
        if let Some(offset) = body.find(close)
            && offset > 0
        {
            return Some(Span {
                open_len: open.len(),
                content_end: open.len() + offset,
                span_end: open.len() + offset + close.len(),
                kind,
            });
        }
    }
    None
}

fn next_char_len(rest: &str) -> usize {
    rest.chars().next().map_or(1, char::len_utf8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sections(text: &str) -> Vec<(String, Color32)> {
        let job = highlight(&egui::Style::default(), text, 400.0, None, &[]);
        job.sections
            .iter()
            .map(|section| {
                (
                    job.text[section.byte_range.start.0..section.byte_range.end.0].to_string(),
                    section.format.color,
                )
            })
            .collect()
    }

    fn hybrid_sections(text: &str, active_line: usize) -> Vec<(String, TextFormat)> {
        let job = hybrid_highlight(&egui::Style::default(), text, 600.0, active_line, None, &[]);
        job.sections
            .iter()
            .map(|section| {
                (
                    job.text[section.byte_range.start.0..section.byte_range.end.0].to_string(),
                    section.format.clone(),
                )
            })
            .collect()
    }

    /// 任何输入下 `LayoutJob` 的分段都必须首尾相接、完整覆盖原文，
    /// 否则 egui 会 panic 或错位。
    fn assert_covers(text: &str) {
        assert_covers_with(text, None);
    }

    fn assert_covers_with(text: &str, anchor: Option<&Range<usize>>) {
        let job = highlight(&egui::Style::default(), text, 400.0, anchor, &[]);
        assert_eq!(job.text, text);
        let mut cursor = 0usize;
        for section in &job.sections {
            assert_eq!(section.byte_range.start.0, cursor);
            cursor = section.byte_range.end.0;
            // 分段从半个汉字中间切开会让 epaint 在排版时 panic。
            assert!(
                job.text.is_char_boundary(cursor),
                "分段边界 {cursor} 落在了字符中间"
            );
        }
        assert_eq!(cursor, job.text.len());
    }

    /// 锚点覆盖到的字节铺底色，其余不动；分段切开后仍要完整覆盖原文。
    #[test]
    fn anchor_tints_exactly_the_requested_bytes() {
        let text = "# 标题\n\n第一段。\n\n第二段。\n";
        let anchor = text.find("第一段。").expect("样例里有第一段")..;
        let anchor = anchor.start..anchor.start + "第一段。".len();
        let job = highlight(&egui::Style::default(), text, 400.0, Some(&anchor), &[]);

        for section in &job.sections {
            let (start, end) = (section.byte_range.start.0, section.byte_range.end.0);
            let inside = anchor.start <= start && end <= anchor.end;
            assert_eq!(
                section.format.background == theme::md::ANCHOR_BG,
                inside,
                "分段 {:?} 的底色与是否落在锚点内不符",
                &text[start..end]
            );
        }
    }

    /// 锚点落在任意字节位置都不能破坏分段：改完稿的旧锚点可能越界，也可能正好
    /// 指到某个汉字的中间，这两种都得安静地丢掉而不是把排版搞崩。
    #[test]
    fn anchor_never_breaks_section_coverage() {
        let text = "# 标题\n\n正文**加粗**（括号）。\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        for start in 0..text.len() + 4 {
            for end in [start, start + 1, start + 7, text.len(), text.len() + 8] {
                assert_covers_with(text, Some(&(start..end)));
            }
        }
    }

    #[test]
    fn search_matches_are_tinted_and_current_match_wins() {
        let text = "重点与重点";
        let matches = vec![0.."重点".len(), "重点与".len()..text.len()];
        let current = matches[1].clone();
        let job = highlight(
            &egui::Style::default(),
            text,
            400.0,
            Some(&current),
            &matches,
        );
        assert!(job.sections.iter().any(|section| {
            section.byte_range.start.0 == matches[0].start
                && section.format.background == theme::md::SEARCH_BG
        }));
        assert!(job.sections.iter().any(|section| {
            section.byte_range.start.0 == current.start
                && section.format.background == theme::md::ANCHOR_BG
        }));
    }

    #[test]
    fn headings_dim_the_hashes_and_color_the_text() {
        let sections = sections("## 一级标题");
        assert_eq!(sections[0], ("## ".to_string(), theme::md::MARKER));
        assert_eq!(sections[1], ("一级标题".to_string(), theme::md::HEADING));
    }

    #[test]
    fn hybrid_only_reveals_markers_on_the_active_line() {
        let sections = hybrid_sections("# 标题\n正文 **重点**\n## 小标题", 1);
        let marker_sections = sections
            .iter()
            .filter(|(text, _)| matches!(text.as_str(), "# " | "## " | "**"))
            .collect::<Vec<_>>();
        assert_eq!(marker_sections.len(), 4);
        assert_eq!(marker_sections[0].1.color, Color32::TRANSPARENT);
        assert_ne!(marker_sections[1].1.color, Color32::TRANSPARENT);
        assert_ne!(marker_sections[2].1.color, Color32::TRANSPARENT);
        assert_eq!(marker_sections[3].1.color, Color32::TRANSPARENT);
    }

    #[test]
    fn hybrid_uses_official_fonts_sizes_and_line_height() {
        let sections = hybrid_sections("# 公文标题\n正文", usize::MAX);
        let title = sections
            .iter()
            .find(|(text, _)| text == "公文标题")
            .expect("标题分段");
        let body = sections
            .iter()
            .find(|(text, _)| text == "正文")
            .expect("正文分段");
        assert_eq!(
            title.1.font_id.family,
            theme::official_family(theme::FONT_BIAOSONG)
        );
        assert_eq!(title.1.font_id.size, OFFICIAL_TITLE_PT * PT);
        assert_eq!(
            body.1.font_id.family,
            theme::official_family(theme::FONT_FANGSONG)
        );
        assert_eq!(body.1.font_id.size, OFFICIAL_BODY_PT * PT);
        assert_eq!(body.1.line_height, Some(OFFICIAL_LINE_PT * PT));
    }

    #[test]
    fn hybrid_reserves_two_char_indent_and_heading_number_space() {
        let text = "# 公文标题\n## 总体要求\n### 具体安排";
        let job = hybrid_highlight(&egui::Style::default(), text, 600.0, usize::MAX, None, &[]);
        let section = |needle: &str| {
            job.sections
                .iter()
                .find(|section| {
                    &job.text[section.byte_range.start.0..section.byte_range.end.0] == needle
                })
                .expect("样例分段存在")
        };
        assert_eq!(section("# ").leading_space, 0.0);
        assert!(section("## ").leading_space >= OFFICIAL_BODY_PT * PT * 2.0);
        assert!(section("### ").leading_space > section("## ").leading_space);
    }

    #[test]
    fn hybrid_centers_every_visual_row_of_a_wrapped_document_title() {
        let text = "# 关于报送2026年度政务服务事项标准化建设情况的函\n正文";
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let mut centered = None;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let job = hybrid_highlight(&egui::Style::default(), text, 600.0, usize::MAX, None, &[]);
            let mut galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
            center_document_title_rows(&mut galley, text, 600.0);
            centered = Some(galley);
        });
        let galley = centered.expect("已排版");
        let mut title_rows = Vec::new();
        for row in &galley.rows {
            title_rows.push(row);
            if row.ends_with_newline {
                break;
            }
        }
        assert!(title_rows.len() >= 2);
        for row in title_rows {
            assert!((row.pos.x - (600.0 - row.size.x) * 0.5).abs() < 1.0);
        }
    }

    #[test]
    fn hybrid_collapses_markdown_only_blank_lines() {
        let text = "第一段\n\n第二段";
        let job = hybrid_highlight(&egui::Style::default(), text, 600.0, usize::MAX, None, &[]);
        assert!(job.sections.iter().any(|section| {
            &job.text[section.byte_range.start.0..section.byte_range.end.0] == "\n"
                && section.format.line_height == Some(1.0)
        }));
    }

    #[test]
    fn hybrid_table_reserves_real_cell_widths_and_hides_pipes() {
        let text = "| 部门 | 负责人 | 时限 |";
        let job = hybrid_highlight(&egui::Style::default(), text, 600.0, usize::MAX, None, &[]);
        let pipes = job
            .sections
            .iter()
            .filter(|section| {
                &job.text[section.byte_range.start.0..section.byte_range.end.0] == "|"
            })
            .collect::<Vec<_>>();
        assert!(!pipes.is_empty());
        assert!(
            pipes
                .iter()
                .all(|section| section.format.color == Color32::TRANSPARENT)
        );
        assert!(
            job.sections
                .iter()
                .any(|section| section.leading_space > 80.0)
        );
    }

    #[test]
    fn hybrid_sections_still_cover_the_exact_markdown_source() {
        let text = "# 标题\n\n正文 **重点**\n| 姓名 | 电话 |\n|---|---|\n<!-- [附件] -->";
        let job = hybrid_highlight(&egui::Style::default(), text, 600.0, 2, None, &[]);
        assert_eq!(job.text, text);
        let mut cursor = 0usize;
        for section in &job.sections {
            assert_eq!(section.byte_range.start.0, cursor);
            cursor = section.byte_range.end.0;
            assert!(job.text.is_char_boundary(cursor));
        }
        assert_eq!(cursor, text.len());
    }

    #[test]
    fn document_title_uses_the_accent_color() {
        let sections = sections("# 关于开展工作的函");
        assert_eq!(sections[1].1, theme::md::TITLE);
    }

    #[test]
    fn markdown_source_uses_the_ui_font_family() {
        let job = highlight(&egui::Style::default(), "正文 **重点**", 400.0, None, &[]);
        assert!(
            job.sections
                .iter()
                .all(|section| section.format.font_id.family == egui::FontFamily::Proportional)
        );
    }

    #[test]
    fn strong_marks_are_dimmed_and_content_is_highlighted() {
        let sections = sections("这是**重点**内容");
        let colors = sections.iter().map(|(_, color)| *color).collect::<Vec<_>>();
        assert!(colors.contains(&theme::md::STRONG));
        assert_eq!(sections[1], ("**".to_string(), theme::md::MARKER));
    }

    #[test]
    fn placeholders_and_quotes_keep_their_brackets() {
        let sections = sections("【待核实：文号】与“规范”");
        assert_eq!(
            sections[0],
            ("【待核实：文号】".to_string(), theme::md::TODO)
        );
        assert!(
            sections
                .iter()
                .any(|(text, color)| text == "“规范”" && *color == theme::md::QUOTED)
        );
    }

    #[test]
    fn table_rows_split_pipes_from_cells() {
        let sections = sections("| 姓名 | 电话 |");
        assert!(
            sections
                .iter()
                .any(|(text, color)| text == "|" && *color == theme::md::TABLE_PIPE)
        );
        assert!(
            sections
                .iter()
                .any(|(_, color)| *color == theme::md::TABLE_CELL)
        );
    }

    #[test]
    fn separator_rows_are_dimmed_as_a_whole() {
        let sections = sections("|---|---|");
        assert_eq!(sections[0].1, theme::md::TABLE_RULE);
    }

    #[test]
    fn section_markers_render_as_comments() {
        let sections = sections("<!-- 附件 -->");
        assert_eq!(sections[0].1, theme::md::COMMENT);
    }

    #[test]
    fn sections_always_cover_the_whole_text() {
        assert_covers("");
        assert_covers("\n\n");
        assert_covers(
            "# 标题\n\n正文**加粗**（括号）。\n\n- 列表\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
        );
        assert_covers("孤立的 ** 与 【 与 “ 不成对");
        assert_covers("中文**加粗**混排 ASCII `code` 与【待核实：日期】");
    }
}
