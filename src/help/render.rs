//! 帮助正文的轻量 Markdown 渲染器。
//!
//! 只支持帮助文档实际用到的子集：标题、段落、粗体、行内代码、链接、
//! 两级列表、表格、图片、引用块、代码块与分隔线。不引入 Markdown 依赖——
//! 帮助正文是随程序打包的静态资源，语法可控，自己渲染反而能把链接做成
//! 真正可点的章内跳转。
//!
//! 行内标记在**所有**位置都解析：段落、列表项、表格单元、引用块。少解析
//! 一处，用户看到的就是一串裸着的 `**` 和反引号。
//!
//! 链接写法：
//! - `[文字](#锚点)` 跳本章小节，锚点是小节标题原文；
//! - `[文字](chapter:章id)` 跳别的章，id 见 `content::CHAPTERS`。

use super::content;
use crate::theme;
use eframe::egui::{self, RichText};
use theme::md;

/// 一次渲染里用户在正文上点出来的动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    /// 跳到本章内的小节标题。
    Section(String),
    /// 跳到别的章。
    Chapter(String),
    /// 放大看某张配图（值是 `images/xxx.png` 这样的 key）。
    Zoom(String),
}

/// 解析出的一段行内文本：普通、粗体、行内代码、链接。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Span {
    Text(String),
    Strong(String),
    Code(String),
    Link { text: String, target: String },
}

/// 引用块的语气，决定色条、淡底与左上角那枚标签的颜色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    /// 「提示」：补充说明，不看也不会错。
    Tip,
    /// 「注意」：不照做会出问题。
    Note,
    /// 「警告」「红线」：踩了就是事故。
    Alert,
    /// 没打标签的普通引用。
    Plain,
}

impl Tone {
    /// 按引用块首行的 `**标签**` 定调。标签是「红线一」这类带序号的写法时
    /// 也要认出来——只匹配「红线」二字会让三条红线掉回普通引用的灰底。
    fn of(label: Option<&str>) -> Self {
        match label {
            Some(l) if l.starts_with("红线") || l.starts_with("警告") => Tone::Alert,
            Some(l) if l.starts_with("注意") => Tone::Note,
            Some(l) if l.starts_with("提示") => Tone::Tip,
            _ => Tone::Plain,
        }
    }

    /// (色条与标签色, 淡底色)。
    fn colors(self) -> (egui::Color32, egui::Color32) {
        match self {
            Tone::Alert => (theme::danger(), theme::danger_soft()),
            Tone::Note => (theme::warn(), theme::warn_soft()),
            Tone::Tip => (theme::info(), theme::accent_soft()),
            Tone::Plain => (theme::border_strong(), theme::surface_sunk()),
        }
    }
}

/// 解析出的块级元素。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Block {
    Heading {
        level: usize,
        text: String,
    },
    Paragraph(Vec<Span>),
    List {
        ordered: bool,
        items: Vec<(usize, Vec<Span>)>,
    },
    Table {
        head: Vec<Vec<Span>>,
        rows: Vec<Vec<Vec<Span>>>,
    },
    Quote {
        tone: Tone,
        label: Option<String>,
        lines: Vec<Vec<Span>>,
    },
    Code {
        lines: Vec<String>,
    },
    Image {
        alt: String,
        src: String,
    },
    Rule,
}

/// 把 markdown 解析成块序列。行级扫描，够用且可测。
pub(crate) fn parse(source: &str) -> Vec<Block> {
    // 帮助正文是 include_str! 进来的 UTF-8；个别编辑器会留 BOM，吃掉免得它
    // 粘在第一行的 `#` 前面把标题吃成段落。
    let source = source.trim_start_matches('\u{feff}');
    let lines: Vec<&str> = source.lines().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            i += 1;
            continue;
        }
        // 代码块：``` 起到下一个 ``` 为止。
        if trimmed.starts_with("```") {
            let mut code = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim().starts_with("```") {
                code.push(lines[i].to_string());
                i += 1;
            }
            i += 1;
            out.push(Block::Code { lines: code });
            continue;
        }
        // 标题。
        if let Some(level) = heading_level(trimmed) {
            let text = trimmed.trim_start_matches('#').trim().to_string();
            out.push(Block::Heading { level, text });
            i += 1;
            continue;
        }
        // 分隔线。
        if is_rule(trimmed) {
            out.push(Block::Rule);
            i += 1;
            continue;
        }
        // 图片独占一行。
        if let Some((alt, src)) = parse_image(trimmed) {
            out.push(Block::Image { alt, src });
            i += 1;
            continue;
        }
        // 表格：以 | 开头且下一行为分隔行。
        if trimmed.starts_with('|') && i + 1 < lines.len() && is_table_rule(lines[i + 1].trim()) {
            let head = split_row(trimmed);
            let mut rows = Vec::new();
            i += 2;
            while i < lines.len() && lines[i].trim().starts_with('|') {
                rows.push(split_row(lines[i].trim()));
                i += 1;
            }
            // 短行补齐到表头列数，长行截断：单元格数对不上就会错列。
            let columns = head.len();
            for row in &mut rows {
                row.truncate(columns);
                while row.len() < columns {
                    row.push(Vec::new());
                }
            }
            out.push(Block::Table { head, rows });
            continue;
        }
        // 引用块。
        if trimmed.starts_with('>') {
            let mut quoted: Vec<String> = Vec::new();
            while i < lines.len() && lines[i].trim().starts_with('>') {
                let text = lines[i].trim().trim_start_matches('>').trim().to_string();
                quoted.push(text);
                i += 1;
            }
            // 首行的 `**注意**` 这类领头粗体拎出来当标签，正文里不再重复。
            let label = quoted.first().and_then(|first| leading_label(first));
            if let (Some(label), Some(first)) = (label.as_deref(), quoted.first_mut()) {
                *first = first[label.len() + 4..].trim_start().to_string();
            }
            let tone = Tone::of(label.as_deref());
            let lines = quoted
                .iter()
                .filter(|l| !l.is_empty())
                .map(|l| parse_spans(l))
                .collect();
            out.push(Block::Quote { tone, label, lines });
            continue;
        }
        // 列表：连续的 `- ` / `* ` / `1. ` 行，按缩进定层级。
        // 层级看原行的行首缩进，所以这里传 `line` 而不是 `trimmed`。
        if list_marker(line).is_some() {
            let ordered = list_marker(line).is_some_and(|(ord, _)| ord);
            let mut items = Vec::new();
            while i < lines.len() {
                let raw = lines[i];
                if raw.trim().is_empty() {
                    break;
                }
                match list_marker(raw) {
                    Some((ord, ind)) if ord == ordered => {
                        let text = raw[list_prefix_len(raw)..].trim().to_string();
                        items.push((ind.min(1), parse_spans(&text)));
                        i += 1;
                    }
                    _ => break,
                }
            }
            out.push(Block::List { ordered, items });
            continue;
        }
        // 段落：吃掉后续非空、非块首的行，软换行按 markdown 的规矩并成一段。
        let mut para: Vec<&str> = vec![trimmed];
        i += 1;
        while i < lines.len() {
            let t = lines[i].trim();
            if t.is_empty()
                || heading_level(t).is_some()
                || is_rule(t)
                || t.starts_with("```")
                || t.starts_with('>')
                || t.starts_with('|')
                || list_marker(t).is_some()
                || parse_image(t).is_some()
            {
                break;
            }
            para.push(t);
            i += 1;
        }
        out.push(Block::Paragraph(parse_spans(&join_soft_wrapped(&para))));
    }
    out
}

/// 软换行拼接：中文行之间不补空格，两头都是西文字符时才补一个。
fn join_soft_wrapped(lines: &[&str]) -> String {
    let mut out = String::new();
    for line in lines {
        let needs_space = out
            .chars()
            .last()
            .zip(line.chars().next())
            .is_some_and(|(a, b)| a.is_ascii_alphanumeric() && b.is_ascii_alphanumeric());
        if needs_space {
            out.push(' ');
        }
        out.push_str(line);
    }
    out
}

/// 取行首的 `**标签**`，不带星号。没有则 `None`。
fn leading_label(line: &str) -> Option<String> {
    let rest = line.strip_prefix("**")?;
    let end = rest.find("**")?;
    let label = &rest[..end];
    (!label.is_empty() && !label.contains(' ')).then(|| label.to_string())
}

fn heading_level(line: &str) -> Option<usize> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if (1..=4).contains(&hashes) && line[hashes..].starts_with(' ') {
        Some(hashes)
    } else {
        None
    }
}

fn is_rule(line: &str) -> bool {
    line.chars().all(|c| c == '-' || c == ' ') && line.matches('-').count() >= 3
}

fn is_table_rule(line: &str) -> bool {
    line.starts_with('|')
        && line.trim_matches('|').split('|').all(|cell| {
            !cell.trim().is_empty() && cell.trim().chars().all(|c| c == '-' || c == ':')
        })
}

/// 拆一行表格，顺手把每格的行内标记也解析掉。
fn split_row(line: &str) -> Vec<Vec<Span>> {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(|cell| parse_spans(cell.trim()))
        .collect()
}

fn parse_image(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("![")?;
    let close = rest.find("](")?;
    let alt = rest[..close].to_string();
    let src = rest[close + 2..].strip_suffix(')')?.trim().to_string();
    Some((alt, src))
}

/// 列表行的层级与是否有序；缩进两空格算一级。吃的是**原行**，不吃 trim 后的。
fn list_marker(line: &str) -> Option<(bool, usize)> {
    let indent = line.chars().take_while(|c| *c == ' ').count() / 2;
    let rest = line.trim_start();
    if rest.starts_with("- ") || rest.starts_with("* ") {
        Some((false, indent))
    } else if rest
        .split_once(". ")
        .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    {
        Some((true, indent))
    } else {
        None
    }
}

fn list_prefix_len(line: &str) -> usize {
    let rest = line.trim_start();
    if rest.starts_with("- ") || rest.starts_with("* ") {
        line.len() - rest.len() + 2
    } else if let Some((n, _)) = rest.split_once(". ") {
        line.len() - rest.len() + n.len() + 2
    } else {
        line.len()
    }
}

/// 把一行里的 `**粗体**`、`` `代码` ``、`[文字](链接)` 拆成 span。
fn parse_spans(line: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '*'
            && i + 1 < chars.len()
            && chars[i + 1] == '*'
            && let Some(end) = find_close(&chars, i + 2, "**")
        {
            if !buf.is_empty() {
                out.push(Span::Text(std::mem::take(&mut buf)));
            }
            out.push(Span::Strong(chars[i + 2..end].iter().collect()));
            i = end + 2;
            continue;
        }
        if chars[i] == '`'
            && let Some(end) = find_close(&chars, i + 1, "`")
        {
            if !buf.is_empty() {
                out.push(Span::Text(std::mem::take(&mut buf)));
            }
            out.push(Span::Code(chars[i + 1..end].iter().collect()));
            i = end + 1;
            continue;
        }
        if chars[i] == '['
            && let Some((text, target, next)) = parse_link(&chars, i)
        {
            if !buf.is_empty() {
                out.push(Span::Text(std::mem::take(&mut buf)));
            }
            out.push(Span::Link { text, target });
            i = next;
            continue;
        }
        buf.push(chars[i]);
        i += 1;
    }
    if !buf.is_empty() {
        out.push(Span::Text(buf));
    }
    out
}

fn find_close(chars: &[char], from: usize, marker: &str) -> Option<usize> {
    let m: Vec<char> = marker.chars().collect();
    let mut i = from;
    while i + m.len() <= chars.len() {
        if chars[i..i + m.len()] == m[..] {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// 从 `[` 起解析链接，返回文字、目标与结束下标。
fn parse_link(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    let mut i = start + 1;
    let text_start = i;
    while i < chars.len() && chars[i] != ']' {
        i += 1;
    }
    if i >= chars.len() || i + 1 >= chars.len() || chars[i + 1] != '(' {
        return None;
    }
    let text: String = chars[text_start..i].iter().collect();
    let target_start = i + 2;
    let mut j = target_start;
    while j < chars.len() && chars[j] != ')' {
        j += 1;
    }
    if j >= chars.len() {
        return None;
    }
    let target: String = chars[target_start..j].iter().collect();
    Some((text, target, j + 1))
}

// ── 渲染 ────────────────────────────────────────────────────────────────────

/// 正文最舒服的行宽。中文一行超过四十来字眼睛就得来回扫，窗口再宽也不铺满。
const MAX_TEXT_WIDTH: f32 = 780.0;

/// 把整章画进 `ui`，返回用户点到的跳转；没点返回 `None`。
///
/// `available` 是滚动区给出的可用宽度；行宽在此基础上封顶，多出来的宽度
/// 摊成左右留白，正文既不贴着分隔线，也不会在宽屏上拖成一条长横幅。
///
/// `scroll_to` 给出要滚到的小节标题；命中该标题时把它拉到滚动区顶部。
pub(crate) fn show(
    ui: &mut egui::Ui,
    source: &str,
    available: f32,
    scroll_to: Option<&str>,
) -> Option<Action> {
    let width = available.clamp(200.0, MAX_TEXT_WIDTH);
    let gutter = ((available - width) * 0.5).clamp(0.0, 56.0);
    let mut jump = None;
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: gutter as i8,
            right: 0,
            top: 0,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.set_width(width);
            for block in parse(source) {
                let hit = show_block(ui, &block, width, scroll_to);
                if jump.is_none() {
                    jump = hit;
                }
            }
        });
    jump
}

fn show_block(
    ui: &mut egui::Ui,
    block: &Block,
    width: f32,
    scroll_to: Option<&str>,
) -> Option<Action> {
    match block {
        Block::Heading { level, text } => {
            show_heading(ui, *level, text, scroll_to);
            None
        }
        Block::Paragraph(spans) => {
            let jump = show_spans(ui, spans, width);
            ui.add_space(9.0);
            jump
        }
        Block::List { ordered, items } => show_list(ui, *ordered, items, width),
        Block::Table { head, rows } => show_table(ui, head, rows, width),
        Block::Quote { tone, label, lines } => {
            show_quote(ui, *tone, label.as_deref(), lines, width)
        }
        Block::Code { lines } => {
            show_code(ui, lines, width);
            None
        }
        Block::Image { alt, src } => show_image(ui, alt, src, width),
        Block::Rule => {
            ui.add_space(10.0);
            theme::hairline(ui);
            ui.add_space(10.0);
            None
        }
    }
}

fn show_heading(ui: &mut egui::Ui, level: usize, text: &str, scroll_to: Option<&str>) {
    // 一级与二级是「换一个话题」，前面留白拉开；三级只是本话题下的一小段。
    ui.add_space(match level {
        1 | 2 => 18.0,
        _ => 12.0,
    });
    let size = match level {
        1 => theme::font_sizes::HEADING + 4.0,
        2 => theme::font_sizes::HEADING,
        3 => theme::font_sizes::BODY + 2.0,
        _ => theme::font_sizes::BODY,
    };
    let color = match level {
        1 | 2 => md::heading(),
        _ => theme::text(),
    };
    let response = ui.label(RichText::new(text).size(size).color(color).strong());
    // 目录点进来的小节，把它顶到滚动区最上面；scroll_to 只在跳转那一帧有值。
    if scroll_to.is_some_and(|target| target.trim() == text.trim()) {
        response.scroll_to_me(Some(egui::Align::TOP));
    }
    if level <= 2 {
        ui.add_space(5.0);
        theme::hairline(ui);
        ui.add_space(9.0);
    } else {
        ui.add_space(5.0);
    }
}

fn show_list(
    ui: &mut egui::Ui,
    ordered: bool,
    items: &[(usize, Vec<Span>)],
    width: f32,
) -> Option<Action> {
    let mut jump = None;
    // 每一层各自计数，嵌套的子项不会把外层的序号接着往下数。
    let mut counters = [1usize; 2];
    for (indent, spans) in items {
        let indent = (*indent).min(1);
        if indent == 0 {
            counters[1] = 1;
        }
        let marker = if ordered {
            let n = counters[indent];
            counters[indent] += 1;
            format!("{n}.")
        } else if indent == 0 {
            "•".to_string()
        } else {
            "◦".to_string()
        };
        let left = 6.0 + indent as f32 * 20.0;
        let marker_width = if ordered { 22.0 } else { 14.0 };
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.add_space(left);
            ui.allocate_ui_with_layout(
                egui::vec2(marker_width, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(marker_width);
                    ui.label(RichText::new(marker).color(md::bullet()));
                },
            );
            // 换行的续行缩进到与首行文字齐平，不会退回到项目符号底下。
            let text_width = (width - left - marker_width).max(120.0);
            ui.allocate_ui_with_layout(
                egui::vec2(text_width, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(text_width);
                    if let Some(hit) = show_spans(ui, spans, text_width)
                        && jump.is_none()
                    {
                        jump = Some(hit);
                    }
                },
            );
        });
        ui.add_space(3.0);
    }
    ui.add_space(6.0);
    jump
}

/// 画一行行内内容，返回用户点到的跳转。
fn show_spans(ui: &mut egui::Ui, spans: &[Span], width: f32) -> Option<Action> {
    if spans.is_empty() {
        return None;
    }
    let mut jump = None;
    ui.horizontal_wrapped(|ui| {
        ui.set_max_width(width);
        // 中文相邻两段之间不该有缝：粗体与正文之间多两像素就断开了。
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.spacing_mut().item_spacing.y = 3.0;
        draw_spans(ui, spans, &mut jump);
    });
    jump
}

/// 逐段画行内内容。抽出来是因为引用块要在同一行里先画一枚标签再接正文。
fn draw_spans(ui: &mut egui::Ui, spans: &[Span], jump: &mut Option<Action>) {
    for span in spans {
        match span {
            Span::Text(t) => {
                ui.label(RichText::new(t.as_str()).color(theme::text()));
            }
            Span::Strong(t) => {
                ui.label(RichText::new(t.as_str()).color(theme::text()).strong());
            }
            Span::Code(t) => {
                ui.label(
                    RichText::new(t.as_str())
                        .color(md::code())
                        .background_color(theme::surface_sunk())
                        .monospace()
                        .size(theme::font_sizes::MONO - 1.0),
                );
            }
            Span::Link { text, target } => {
                // 用带点击感知的 Label 而不是 Button：按钮自带上下内边距，混在
                // 一行文字里会把行高顶起来，一段话里有链接就一行高一行矮。
                let label = egui::Label::new(
                    RichText::new(text.as_str())
                        .color(theme::accent())
                        .underline(),
                )
                .sense(egui::Sense::click());
                let response = ui
                    .add(label)
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(hover_for(target, text));
                if response.clicked() && jump.is_none() {
                    *jump = parse_target(target);
                }
            }
        }
    }
}

fn hover_for(target: &str, text: &str) -> String {
    if let Some(id) = target.strip_prefix("chapter:") {
        match content::title_of(id) {
            Some(title) => format!("跳到「{title}」一章"),
            None => format!("跳到「{text}」"),
        }
    } else if target.starts_with('#') {
        format!("跳到本章「{text}」")
    } else {
        text.to_string()
    }
}

fn parse_target(target: &str) -> Option<Action> {
    if let Some(id) = target.strip_prefix("chapter:") {
        return Some(Action::Chapter(id.to_string()));
    }
    if let Some(section) = target.strip_prefix('#') {
        return Some(Action::Section(section.to_string()));
    }
    None
}

// ── 表格 ────────────────────────────────────────────────────────────────────

/// 单元格左右内边距，算列宽时要把它刨掉。
const CELL_PAD_X: f32 = 10.0;

fn show_table(
    ui: &mut egui::Ui,
    head: &[Vec<Span>],
    rows: &[Vec<Vec<Span>>],
    width: f32,
) -> Option<Action> {
    if head.is_empty() {
        return None;
    }
    let mut jump = None;
    let widths = column_widths(head, rows, width - 2.0);
    egui::Frame::new()
        .fill(theme::surface())
        .stroke(egui::Stroke::new(1.0, theme::border()))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(1))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            table_row(ui, &widths, head, theme::surface_sunk(), true, &mut jump);
            theme::hairline(ui);
            for (i, row) in rows.iter().enumerate() {
                let fill = if i % 2 == 1 {
                    theme::surface_sunk()
                } else {
                    egui::Color32::TRANSPARENT
                };
                table_row(ui, &widths, row, fill, false, &mut jump);
            }
        });
    ui.add_space(12.0);
    jump
}

fn table_row(
    ui: &mut egui::Ui,
    widths: &[f32],
    cells: &[Vec<Span>],
    fill: egui::Color32,
    strong: bool,
    jump: &mut Option<Action>,
) {
    egui::Frame::new().fill(fill).show(ui, |ui| {
        ui.set_width(widths.iter().sum::<f32>());
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (cell, w) in cells.iter().zip(widths) {
                // 定宽分配给每一格，格内自己换行；不定宽的话长单元格会把整行撑爆。
                ui.allocate_ui_with_layout(
                    egui::vec2(*w, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(*w);
                        egui::Frame::new()
                            .inner_margin(egui::Margin::symmetric(CELL_PAD_X as i8, 6))
                            .show(ui, |ui| {
                                let inner = (*w - CELL_PAD_X * 2.0).max(32.0);
                                ui.set_width(inner);
                                let spans: Vec<Span> = if strong {
                                    cell.iter().cloned().map(to_strong).collect()
                                } else {
                                    cell.clone()
                                };
                                if let Some(hit) = show_spans(ui, &spans, inner)
                                    && jump.is_none()
                                {
                                    *jump = Some(hit);
                                }
                            });
                    },
                );
            }
        });
    });
}

/// 表头整行加粗：普通文字升成粗体，代码与链接保持本来的样子。
fn to_strong(span: Span) -> Span {
    match span {
        Span::Text(t) => Span::Strong(t),
        other => other,
    }
}

/// 按各列内容的长短分宽度。等分会让「层」「可改」这种两字表头占掉半张表，
/// 挨着的长句却被挤成七八行。
fn column_widths(head: &[Vec<Span>], rows: &[Vec<Vec<Span>>], total: f32) -> Vec<f32> {
    let columns = head.len();
    let min_width = 58.0f32;
    let mut weights: Vec<f32> = head
        .iter()
        .map(|cell| plain_len(cell).max(3.0))
        .collect::<Vec<_>>();
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(columns) {
            // 用「较长的那几格」定权，而不是最长的一格：一条特别长的说明
            // 不该把整列拉宽到挤掉别的列。取长度的平方根压一压极端值。
            weights[i] = weights[i].max(plain_len(cell).sqrt() * 4.0);
        }
    }
    let sum: f32 = weights.iter().sum();
    if sum <= 0.0 || total <= min_width * columns as f32 {
        return vec![(total / columns as f32).max(min_width); columns];
    }
    let mut out: Vec<f32> = weights.iter().map(|w| total * w / sum).collect();
    // 提到下限的那几列多吃的宽度，从还有富余的列里按比例扣回来。
    let deficit: f32 = out.iter().map(|w| (min_width - w).max(0.0)).sum();
    if deficit > 0.0 {
        let surplus: f32 = out.iter().map(|w| (w - min_width).max(0.0)).sum();
        for w in &mut out {
            if *w < min_width {
                *w = min_width;
            } else if surplus > 0.0 {
                *w -= deficit * (*w - min_width) / surplus;
            }
        }
    }
    // 四舍五入的零头补给最后一列，免得整行比边框窄一两像素。
    let drift = total - out.iter().sum::<f32>();
    if let Some(last) = out.last_mut() {
        *last += drift;
    }
    out
}

/// 单元格的视觉长度：汉字按两个西文字符算。
fn plain_len(spans: &[Span]) -> f32 {
    spans
        .iter()
        .map(|span| match span {
            Span::Text(t) | Span::Strong(t) | Span::Code(t) => t.as_str(),
            Span::Link { text, .. } => text.as_str(),
        })
        .flat_map(|t| t.chars())
        .map(|c| if c.is_ascii() { 1.0 } else { 2.0 })
        .sum()
}

// ── 引用块、代码块与配图 ────────────────────────────────────────────────────

/// 引用块画成左侧竖条的提示框，首行的 `**注意**` 这类标签画成同色的抬头。
fn show_quote(
    ui: &mut egui::Ui,
    tone: Tone,
    label: Option<&str>,
    lines: &[Vec<Span>],
    width: f32,
) -> Option<Action> {
    let (bar, fill) = tone.colors();
    let mut jump = None;
    let response = egui::Frame::new()
        .fill(fill)
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin {
            left: 14,
            right: 12,
            top: 9,
            bottom: 9,
        })
        .show(ui, |ui| {
            let inner = (width - 28.0).max(120.0);
            ui.set_width(inner);
            for (i, line) in lines.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    ui.set_max_width(inner);
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.spacing_mut().item_spacing.y = 3.0;
                    // 标签跟正文同处一行：一条两行的提醒不该因为抬头单占一行变成三行。
                    if i == 0
                        && let Some(label) = label
                    {
                        ui.label(RichText::new(label).color(bar).strong());
                        ui.add_space(8.0);
                    }
                    draw_spans(ui, line, &mut jump);
                });
            }
        })
        .response;
    let bar_rect =
        egui::Rect::from_min_size(response.rect.min, egui::vec2(3.0, response.rect.height()));
    ui.painter().rect_filled(
        bar_rect,
        egui::CornerRadius {
            nw: 8,
            sw: 8,
            ne: 0,
            se: 0,
        },
        bar,
    );
    ui.add_space(11.0);
    jump
}

fn show_code(ui: &mut egui::Ui, lines: &[String], width: f32) {
    egui::Frame::new()
        .fill(theme::surface_sunk())
        .stroke(egui::Stroke::new(1.0, theme::border()))
        .corner_radius(egui::CornerRadius::same(7))
        .inner_margin(egui::Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.set_width((width - 26.0).max(120.0));
            ui.spacing_mut().item_spacing.y = 2.0;
            for line in lines {
                ui.label(
                    RichText::new(line.as_str())
                        .color(md::code())
                        .monospace()
                        .size(theme::font_sizes::MONO - 1.0),
                );
            }
        });
    ui.add_space(11.0);
}

/// 配图的缩放分档（点）。窗口一拖就重采样会卡，按档走只在跨档时重来一次。
const IMAGE_WIDTH_STEP: f32 = 24.0;

fn show_image(ui: &mut egui::Ui, alt: &str, src: &str, width: f32) -> Option<Action> {
    let Some(bytes) = content::image_bytes(src) else {
        missing(ui, format!("（缺图：{src}）"));
        return None;
    };
    let max_width = (width - 8.0).clamp(80.0, MAX_TEXT_WIDTH);
    let Some(texture) = cached_texture(ui.ctx(), src, bytes, max_width, "inline") else {
        missing(ui, format!("（图片无法解码：{src}）"));
        return None;
    };
    let size = texture.size_vec2();
    if size.x <= 0.0 || size.y <= 0.0 {
        return None;
    }
    // 示意图原图 1600 像素宽，挤进一栏正文里字会小到勉强能认。所以整张图
    // 可点：点开放大到整屏看细节，不用改内容就把可读性补回来。
    let shown = egui::vec2(max_width, max_width * size.y / size.x);
    let mut zoom = None;
    ui.add_space(6.0);
    ui.vertical_centered(|ui| {
        let frame = egui::Frame::new()
            .fill(theme::surface())
            .stroke(egui::Stroke::new(1.0, theme::border()))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(4))
            .show(ui, |ui| {
                ui.add(egui::Image::new(&texture).fit_to_exact_size(shown));
            });
        let response = ui
            .interact(
                frame.response.rect,
                egui::Id::new(("help_image_click", src)),
                egui::Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::ZoomIn)
            .on_hover_text("点开放大看");
        if response.clicked() {
            zoom = Some(Action::Zoom(src.to_string()));
        }
        if !alt.is_empty() {
            ui.add_space(5.0);
            ui.label(
                RichText::new(alt.to_string())
                    .color(theme::text_muted())
                    .size(theme::font_sizes::SMALL),
            );
        }
    });
    ui.add_space(14.0);
    zoom
}

fn missing(ui: &mut egui::Ui, text: String) {
    ui.label(
        RichText::new(text)
            .color(theme::text_muted())
            .size(theme::font_sizes::SMALL),
    );
}

/// 放大浮层：半透明幕布上铺一张尽量大的图，点任意处收起。返回是否该关掉。
pub(crate) fn zoom_overlay(ctx: &egui::Context, src: &str) -> bool {
    let Some(bytes) = content::image_bytes(src) else {
        return true;
    };
    let Some((origin_w, origin_h)) = png_size(bytes) else {
        return true;
    };
    let screen = ctx.content_rect();
    let margin = 40.0;
    let box_w = (screen.width() - margin * 2.0).max(160.0);
    let box_h = (screen.height() - margin * 2.0 - 30.0).max(160.0);
    // 先按原图比例算出能摆多宽，再照这个宽度去采样，纹理不多不少刚好够用。
    let aspect = origin_w as f32 / origin_h as f32;
    let shown_width = box_w.min(box_h * aspect).min(origin_w as f32);
    let Some(texture) = cached_texture(ctx, src, bytes, shown_width, "zoom") else {
        return true;
    };
    let shown = egui::vec2(shown_width, shown_width / aspect);
    let mut close = false;
    egui::Area::new(egui::Id::new("help_zoom_overlay"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            ui.set_clip_rect(screen);
            // 先把整幕占掉再画：Area 的大小是按内容算的，不先占位第一帧它
            // 只有巴掌大，命中区域跟着被裁没，点哪儿都收不起来。
            // 整幕可点也符合直觉——放大之后随便点一下就该关。
            let response = ui.allocate_response(screen.size(), egui::Sense::click());
            let painter = ui.painter();
            painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(200));
            let rect = egui::Rect::from_center_size(screen.center(), shown);
            painter.rect_stroke(
                rect,
                egui::CornerRadius::same(4),
                egui::Stroke::new(1.0, theme::border()),
                egui::StrokeKind::Outside,
            );
            painter.text(
                egui::pos2(screen.center().x, rect.max.y + 14.0),
                egui::Align2::CENTER_TOP,
                "点任意处或按 Esc 收起",
                egui::FontId::proportional(theme::font_sizes::SMALL),
                theme::text_muted(),
            );
            egui::Image::new(&texture).paint_at(ui, rect);
            if response.clicked() {
                close = true;
            }
        });
    close
}

/// 按目标宽度取（必要时重建）纹理。`slot` 区分正文里那张小的和放大那张。
fn cached_texture(
    ctx: &egui::Context,
    src: &str,
    bytes: &[u8],
    width: f32,
    slot: &'static str,
) -> Option<egui::TextureHandle> {
    let bucket = (width / IMAGE_WIDTH_STEP).round().max(1.0) as u32;
    let key = egui::Id::new(("help_image", slot, src));
    let cached = ctx.data_mut(|data| data.get_temp::<(u32, egui::TextureHandle)>(key));
    if let Some((cached_bucket, texture)) = cached
        && cached_bucket == bucket
    {
        return Some(texture);
    }
    let texture = load_scaled(ctx, &format!("{slot}:{src}"), bytes, width)?;
    ctx.data_mut(|data| data.insert_temp(key, (bucket, texture.clone())));
    Some(texture)
}

/// 从 PNG 头里直接读尺寸。只为算版面用，犯不上把整张图解码一遍。
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    // 8 字节签名 + 4 字节段长 + 4 字节 "IHDR"，之后紧跟宽、高各 4 字节大端。
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

/// 解码配图并按实际显示尺寸重采样后上传纹理。
///
/// 交给 GPU 的双线性过滤缩小两倍以上时，流程图里的细线和汉字会糊成一片；
/// 先在 CPU 上用 Lanczos3 采到目标像素数，画出来才是清楚的。
fn load_scaled(
    ctx: &egui::Context,
    src: &str,
    bytes: &[u8],
    max_width: f32,
) -> Option<egui::TextureHandle> {
    let decoded = image::load_from_memory(bytes).ok()?;
    let rgba = decoded.to_rgba8();
    let (origin_w, origin_h) = (rgba.width(), rgba.height());
    if origin_w == 0 || origin_h == 0 {
        return None;
    }
    // 不放大：原图比栏宽还窄就按原尺寸画，拉大只会更糊。
    let shown_width = max_width.min(origin_w as f32);
    let target_w = (shown_width * ctx.pixels_per_point()).round().max(1.0) as u32;
    let rgba = if target_w * 17 < origin_w * 16 {
        let target_h = ((origin_h as f32) * (target_w as f32 / origin_w as f32))
            .round()
            .max(1.0) as u32;
        image::imageops::resize(
            &rgba,
            target_w,
            target_h,
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        rgba
    };
    let size = [rgba.width() as usize, rgba.height() as usize];
    // PNG 存的是直通 alpha，不是预乘；用预乘那个构造器会把半透明像素压暗。
    let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    Some(ctx.load_texture(src.to_string(), color, egui::TextureOptions::LINEAR))
}

/// 抽出章内小节标题，供左栏目录与测试用。
pub(crate) fn headings(source: &str) -> Vec<(usize, String)> {
    parse(source)
        .into_iter()
        .filter_map(|block| match block {
            Block::Heading { level, text } => Some((level, text)),
            _ => None,
        })
        .collect()
}

/// 章节正文的纯文字，供左栏搜索匹配。
pub(crate) fn plain_text(source: &str) -> String {
    fn push(out: &mut String, spans: &[Span]) {
        for span in spans {
            match span {
                Span::Text(t) | Span::Strong(t) | Span::Code(t) => out.push_str(t),
                Span::Link { text, .. } => out.push_str(text),
            }
        }
        out.push('\n');
    }
    let mut out = String::new();
    for block in parse(source) {
        match block {
            Block::Heading { text, .. } => {
                out.push_str(&text);
                out.push('\n');
            }
            Block::Paragraph(spans) => push(&mut out, &spans),
            Block::List { items, .. } => {
                for (_, spans) in &items {
                    push(&mut out, spans);
                }
            }
            Block::Table { head, rows } => {
                for cell in &head {
                    push(&mut out, cell);
                }
                for row in &rows {
                    for cell in row {
                        push(&mut out, cell);
                    }
                }
            }
            Block::Quote { label, lines, .. } => {
                if let Some(label) = label {
                    out.push_str(&label);
                    out.push('\n');
                }
                for line in &lines {
                    push(&mut out, line);
                }
            }
            Block::Code { lines } => {
                for line in &lines {
                    out.push_str(line);
                    out.push('\n');
                }
            }
            Block::Image { alt, .. } => {
                out.push_str(&alt);
                out.push('\n');
            }
            Block::Rule => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 离屏画一遍正文，返回点到的跳转与这一帧画出来的每段文字及其位置。
    /// 版面上的毛病（画到栏外、行压行、链接点不动）只有真画一帧才看得出来。
    fn draw(
        ctx: &egui::Context,
        source: &str,
        events: Vec<egui::Event>,
    ) -> (Option<Action>, Vec<(String, egui::Rect)>) {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 1400.0),
            )),
            events,
            ..Default::default()
        };
        let mut jump = None;
        let output = ctx.run_ui(raw, |ui| {
            jump = show(ui, source, TEST_WIDTH, None);
        });
        let drawn = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(text) => Some((
                    text.galley.text().to_string(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                _ => None,
            })
            .collect();
        (jump, drawn)
    }

    const TEST_WIDTH: f32 = 700.0;

    fn test_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        ctx
    }

    fn rect_of(drawn: &[(String, egui::Rect)], needle: &str) -> Option<egui::Rect> {
        drawn
            .iter()
            .find(|(text, _)| text.contains(needle))
            .map(|(_, rect)| *rect)
    }

    /// 一段行内文字上一个稳稳落在它身上的点。
    ///
    /// 换行布局里一段文字的 galley 是从**整行行首**量起的：它的 rect 左边缘
    /// 是行首，不是这段文字自己的起点。所以取中心会点到前一段头上，只有
    /// 贴着右边缘往里收一点才一定在这段文字范围内。
    fn click_point(rect: egui::Rect) -> egui::Pos2 {
        egui::pos2(rect.max.x - 6.0, rect.center().y)
    }

    fn click_events(at: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    /// 段落之外的链接也要真能点。列表项与表格单元的点击结果曾被直接丢掉，
    /// 正文里「见 [某章]」看着是链接，点下去毫无反应。
    #[test]
    fn 段落列表表格引用块里的链接都点得动() {
        for source in [
            "路径见 [数据备份](chapter:21-data)。",
            "- 路径见 [数据备份](chapter:21-data)。",
            "| 项 | 出处 |\n|---|---|\n| 路径 | [数据备份](chapter:21-data) |",
            "> **提示** 路径见 [数据备份](chapter:21-data)。",
        ] {
            // 每条用新的 ctx：沿用同一个的话，上一轮留在链接上的指针会把悬停
            // 提示也画出来，提示里同样写着章名，量位置时会量到它头上。
            let ctx = test_ctx();
            let (_, drawn) = draw(&ctx, source, vec![]);
            let at = click_point(
                rect_of(&drawn, "数据备份").unwrap_or_else(|| panic!("链接文字没画出来：{source}")),
            );
            let (jump, _) = draw(&ctx, source, click_events(at));
            assert_eq!(
                jump,
                Some(Action::Chapter("21-data".into())),
                "点不动：{source}"
            );
        }
    }

    /// 表格必须按栏宽收住。定高单元格 + 等分列宽会让长单元格画到栏外。
    #[test]
    fn 表格画不出栏宽之外() {
        let ctx = test_ctx();
        let source = "| 级 | 含义 | 处置 | 颜色 |\n|---|---|---|---|\n\
             | **必错** | 确定性错误，签发前必须清零，审校抽屉里排在最前面 | 可一键替换 | 红 |";
        let (_, drawn) = draw(&ctx, source, vec![]);
        assert!(!drawn.is_empty(), "表格应当画出内容");
        for (text, rect) in &drawn {
            assert!(
                rect.max.x <= TEST_WIDTH + 2.0,
                "「{text}」画到了栏宽之外：{rect:?}"
            );
        }
    }

    /// 单元格换行撑高后，下一行要整体往下让位，不能压在上一行身上。
    #[test]
    fn 表格长单元格换行后不压住下一行() {
        let ctx = test_ctx();
        let source = "| 甲 | 乙 |\n|---|---|\n\
             | 上一行 | 这一格的说明特别长，长到必须折成好几行才放得下，绝不能盖住底下那行 |\n\
             | 下一行 | 短 |";
        let (_, drawn) = draw(&ctx, source, vec![]);
        let long_bottom = drawn
            .iter()
            .filter(|(text, _)| text.contains("上一行") || text.contains("这一格的说明"))
            .map(|(_, rect)| rect.max.y)
            .fold(f32::MIN, f32::max);
        let next = rect_of(&drawn, "下一行").expect("第二行应当画出来");
        assert!(
            next.min.y >= long_bottom - 1.0,
            "第二行画在了第一行里：第二行顶 {} < 第一行底 {long_bottom}",
            next.min.y
        );
    }

    /// 示意图原图 1600 像素宽，塞进一栏正文里字就小得勉强能认；点开放大是
    /// 补救办法，这条锁住「点得开、点得掉」。
    #[test]
    fn 配图点得开也点得掉() {
        let ctx = test_ctx();
        let source = "![界面五区结构](images/diag-layout.png)";
        let (_, drawn) = draw(&ctx, source, vec![]);
        let caption = rect_of(&drawn, "界面五区结构").expect("图注应当画出来");
        // 图在图注上方，往上挪一截就落在图上。
        let on_figure = egui::pos2(caption.center().x, caption.min.y - 60.0);
        let (action, _) = draw(&ctx, source, click_events(on_figure));
        assert_eq!(
            action,
            Some(Action::Zoom("images/diag-layout.png".into())),
            "点配图没要求放大"
        );

        // 放大层铺满整屏，点哪儿都该收起来。
        let ctx = test_ctx();
        let overlay = |events: Vec<egui::Event>| {
            ctx.begin_pass(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 900.0),
                )),
                events,
                ..Default::default()
            });
            let close = zoom_overlay(&ctx, "images/diag-layout.png");
            let _ = ctx.end_pass();
            close
        };
        assert!(!overlay(vec![]), "没点的时候放大层要留着");
        // 真人点之前指针已经在上面了；egui 也要先认到悬停才算这一下点击。
        let corner = egui::pos2(80.0, 80.0);
        assert!(!overlay(vec![egui::Event::PointerMoved(corner)]));
        assert!(overlay(click_events(corner)), "在放大层上点一下要收起来");
    }

    /// PNG 头里直接读尺寸，别为了算版面把整张图解码一遍。
    #[test]
    fn 从png头读得出尺寸() {
        let bytes = content::image_bytes("images/diag-layout.png").expect("配图应当打包进来");
        let decoded = image::load_from_memory(bytes).expect("配图应当能解码");
        assert_eq!(
            png_size(bytes),
            Some((
                image::GenericImageView::dimensions(&decoded).0,
                image::GenericImageView::dimensions(&decoded).1
            ))
        );
        assert_eq!(png_size(b"not a png"), None);
    }

    /// 正文行宽封顶：窗口拉得再宽，一行也不该铺成几百字的长横幅。
    #[test]
    fn 正文行宽封顶() {
        let ctx = test_ctx();
        let long = "本行特意写得很长，长到在一块很宽的画布上也会一直铺下去，用来验证\
             行宽确实被封住了，而不是跟着窗口一起长。"
            .repeat(6);
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(2400.0, 2000.0),
            )),
            ..Default::default()
        };
        let output = ctx.run_ui(raw, |ui| {
            show(ui, &long, 2000.0, None);
        });
        for clipped in &output.shapes {
            if let egui::epaint::Shape::Text(text) = &clipped.shape {
                assert!(
                    text.galley.size().x <= MAX_TEXT_WIDTH + 2.0,
                    "一行铺到了 {} 宽，封顶是 {MAX_TEXT_WIDTH}",
                    text.galley.size().x
                );
            }
        }
    }

    fn kinds(source: &str) -> Vec<String> {
        parse(source)
            .into_iter()
            .map(|block| match block {
                Block::Heading { level, .. } => format!("h{level}"),
                Block::Paragraph(_) => "p".into(),
                Block::List { ordered, .. } => {
                    if ordered {
                        "ol".into()
                    } else {
                        "ul".into()
                    }
                }
                Block::Table { .. } => "table".into(),
                Block::Quote { .. } => "quote".into(),
                Block::Code { .. } => "code".into(),
                Block::Image { .. } => "image".into(),
                Block::Rule => "rule".into(),
            })
            .collect()
    }

    #[test]
    fn 标题段落与分隔线分开解析() {
        let blocks = kinds("# 标题\n\n正文一段。\n\n---\n\n次段。");
        assert_eq!(blocks, ["h1", "p", "rule", "p"]);
    }

    #[test]
    fn 段落后紧跟分隔线不被吃进段落() {
        let blocks = kinds("正文一段。\n---\n次段。");
        assert_eq!(blocks, ["p", "rule", "p"]);
    }

    #[test]
    fn 标题必须有空格才认() {
        let blocks = kinds("#不是标题");
        assert_eq!(blocks, ["p"]);
    }

    #[test]
    fn 软换行的中文段落不插空格() {
        match parse("头一句话，\n第二句话。").remove(0) {
            Block::Paragraph(spans) => {
                assert_eq!(spans, vec![Span::Text("头一句话，第二句话。".into())]);
            }
            other => panic!("应为段落，得到 {other:?}"),
        }
    }

    #[test]
    fn 软换行的西文之间补空格() {
        match parse("LM Studio\nOllama").remove(0) {
            Block::Paragraph(spans) => {
                assert_eq!(spans, vec![Span::Text("LM Studio Ollama".into())]);
            }
            other => panic!("应为段落，得到 {other:?}"),
        }
    }

    #[test]
    fn 列表按缩进分级() {
        let blocks = parse("- 甲\n  - 乙\n1. 丙");
        assert_eq!(blocks.len(), 2);
        match &blocks[0] {
            Block::List { ordered, items } => {
                assert!(!*ordered);
                assert_eq!(items.len(), 2);
                assert_eq!(items[0].0, 0);
                assert_eq!(items[1].0, 1);
            }
            other => panic!("应为列表，得到 {other:?}"),
        }
        match &blocks[1] {
            Block::List { ordered, .. } => assert!(*ordered),
            other => panic!("应为有序列表，得到 {other:?}"),
        }
    }

    #[test]
    fn 表格单元里的行内标记会被解析() {
        let blocks = parse("| 级 | 含义 |\n|---|---|\n| **必错** | 见 `a` |");
        match &blocks[0] {
            Block::Table { head, rows } => {
                assert_eq!(head[0], vec![Span::Text("级".into())]);
                assert_eq!(rows[0][0], vec![Span::Strong("必错".into())]);
                assert_eq!(
                    rows[0][1],
                    vec![Span::Text("见 ".into()), Span::Code("a".into())]
                );
            }
            other => panic!("应为表格，得到 {other:?}"),
        }
    }

    #[test]
    fn 表格缺格补齐到表头列数() {
        let blocks = parse("| 甲 | 乙 | 丙 |\n|---|---|---|\n| 1 | 2 |");
        match &blocks[0] {
            Block::Table { rows, .. } => assert_eq!(rows[0].len(), 3),
            other => panic!("应为表格，得到 {other:?}"),
        }
    }

    #[test]
    fn 行内样式拆成片段() {
        let spans = parse_spans("见**粗**与`码`及[链](#锚)。");
        assert_eq!(
            spans,
            vec![
                Span::Text("见".into()),
                Span::Strong("粗".into()),
                Span::Text("与".into()),
                Span::Code("码".into()),
                Span::Text("及".into()),
                Span::Link {
                    text: "链".into(),
                    target: "#锚".into()
                },
                Span::Text("。".into()),
            ]
        );
    }

    #[test]
    fn 图片独占一行才认() {
        assert!(matches!(
            parse("![图](images/a.png)").remove(0),
            Block::Image { .. }
        ));
        assert!(matches!(
            parse("前缀 ![图](images/a.png)").remove(0),
            Block::Paragraph(_)
        ));
    }

    #[test]
    fn 链接目标解析() {
        assert_eq!(parse_target("#版头"), Some(Action::Section("版头".into())));
        assert_eq!(
            parse_target("chapter:11-export"),
            Some(Action::Chapter("11-export".into()))
        );
        assert_eq!(parse_target("https://example.com"), None);
    }

    #[test]
    fn 代码块吃掉围栏内部() {
        let blocks = parse("```text\n# 不是标题\n```\n\n正文");
        assert_eq!(blocks.len(), 2);
        match &blocks[0] {
            Block::Code { lines } => assert_eq!(lines, &["# 不是标题".to_string()]),
            other => panic!("应为代码块，得到 {other:?}"),
        }
    }

    #[test]
    fn 引用块首行的标签拎成抬头() {
        match parse("> **注意** 甲\n> 乙").remove(0) {
            Block::Quote { tone, label, lines } => {
                assert_eq!(tone, Tone::Note);
                assert_eq!(label.as_deref(), Some("注意"));
                assert_eq!(lines.len(), 2);
                // 标签不留在正文里重复一遍。
                assert_eq!(lines[0], vec![Span::Text("甲".into())]);
            }
            other => panic!("应为引用块，得到 {other:?}"),
        }
    }

    #[test]
    fn 带序号的红线也算警示() {
        for source in ["> **红线**  甲", "> **红线一** 甲", "> **红线三** 甲"] {
            match parse(source).remove(0) {
                Block::Quote { tone, .. } => assert_eq!(tone, Tone::Alert, "{source}"),
                other => panic!("应为引用块，得到 {other:?}"),
            }
        }
    }

    #[test]
    fn 不带标签的引用块保持普通语气() {
        match parse("> 只是一段引文").remove(0) {
            Block::Quote { tone, label, .. } => {
                assert_eq!(tone, Tone::Plain);
                assert!(label.is_none());
            }
            other => panic!("应为引用块，得到 {other:?}"),
        }
    }

    #[test]
    fn 列宽按内容长短分且不小于下限() {
        let head = vec![parse_spans("层"), parse_spans("来源"), parse_spans("可改")];
        let rows = vec![vec![
            parse_spans("内置"),
            parse_spans("程序自带 proofread-lexicon.tsv，一百五十五条，随程序更新"),
            parse_spans("只读"),
        ]];
        let widths = column_widths(&head, &rows, 600.0);
        assert_eq!(widths.len(), 3);
        assert!(widths.iter().all(|w| *w >= 57.0), "得到 {widths:?}");
        assert!((widths.iter().sum::<f32>() - 600.0).abs() < 0.5);
        assert!(widths[1] > widths[0], "长列应更宽：{widths:?}");
    }

    #[test]
    fn 窄表格退回等分不塌陷() {
        let head = vec![parse_spans("甲"), parse_spans("乙")];
        let widths = column_widths(&head, &[], 80.0);
        assert!(widths.iter().all(|w| *w >= 58.0), "得到 {widths:?}");
    }

    #[test]
    fn 每章正文都能解析出至少一个标题() {
        for chapter in content::CHAPTERS {
            let heads = headings(chapter.body);
            assert!(!heads.is_empty(), "章节 {} 解析不出标题", chapter.id);
        }
    }

    #[test]
    fn 正文里不该剩下没解析的行内标记() {
        for chapter in content::CHAPTERS {
            for block in parse(chapter.body) {
                for span in all_spans(&block) {
                    // 行内代码里的 `**` 是被引用的语法本身，不是漏解析。
                    let text = match span {
                        Span::Text(t) | Span::Strong(t) => t,
                        _ => continue,
                    };
                    assert!(
                        !text.contains("**"),
                        "章节 {} 渲染后仍留着裸粗体标记：{text}",
                        chapter.id
                    );
                }
            }
        }
    }

    #[test]
    fn 章内链接的目标都存在() {
        for chapter in content::CHAPTERS {
            for block in parse(chapter.body) {
                for target in link_targets(&block) {
                    if let Some(id) = target.strip_prefix("chapter:") {
                        assert!(
                            content::index_of(id).is_some(),
                            "章节 {} 指向了不存在的章 {id}",
                            chapter.id
                        );
                    } else if let Some(section) = target.strip_prefix('#') {
                        assert!(
                            headings(chapter.body)
                                .iter()
                                .any(|(_, text)| text == section),
                            "章节 {} 指向了不存在的小节「{section}」",
                            chapter.id
                        );
                    }
                }
            }
        }
    }

    fn link_targets(block: &Block) -> Vec<String> {
        all_spans(block)
            .into_iter()
            .filter_map(|span| match span {
                Span::Link { target, .. } => Some(target),
                _ => None,
            })
            .collect()
    }

    /// 一个块里所有行内片段，不管它藏在段落、列表、表格还是引用块里。
    fn all_spans(block: &Block) -> Vec<Span> {
        match block {
            Block::Paragraph(spans) => spans.clone(),
            Block::List { items, .. } => items.iter().flat_map(|(_, s)| s.clone()).collect(),
            Block::Table { head, rows } => head
                .iter()
                .chain(rows.iter().flatten())
                .flatten()
                .cloned()
                .collect(),
            Block::Quote { lines, .. } => lines.iter().flatten().cloned().collect(),
            _ => Vec::new(),
        }
    }
}
