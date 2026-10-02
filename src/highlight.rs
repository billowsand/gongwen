//! 审校区的 Markdown 语法高亮。
//!
//! 只做“看得清”这一件事：把结构性符号（`#`、`|`、`**`、区段标记）压成弱色，
//! 把真正要读的内容（标题、表格单元、待核实占位）提亮，让一屏文字有层次。
//! 另外给“锚点”——即在公文预览里点中的那一块——铺一层淡底，两栏对照时一眼能
//! 看出版式上的哪一段对应源码里的哪一段。
//! 高亮结果按（文本, 换行宽度, 锚点, 查找命中, 文类, 配色版本）缓存，正常编辑时每帧
//! 只需一次哈希；配色版本让切主题、换纸面之后的第一帧就重新上色，而不是等到下
//! 一次改字或挪光标。缓存里存的是 `LayoutJob` 而不是排好的 `Galley`，理由见
//! [`Cached`]。

use crate::models::{EditorFontScheme, EditorFontSlot};
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
    cache: Option<Cached>,
}

/// 一次排版的缓存。
///
/// 这里缓存的是把 Markdown 编译出来的 [`LayoutJob`]——费时的是解析与上色。排好
/// 的 `Galley` **必须每帧重新向 epaint 要一次**，不能自己攥着跨帧复用：galley 里
/// 每个字形记的是它在字形图集（font atlas）中的像素坐标，而 epaint 会整份重建
/// 图集——`Visuals::text_options` 变了，或者图集用满八成就重建，重建后旧坐标全部
/// 失效，画出来是缺字、糊成一片或串到别的字上。
///
/// 明暗主题互换恰好会改 `text_options`：egui 的深色 visuals 用另一条字形灰度曲线
/// （`FontColorTransferFunction::DARK_MODE_DEFAULT`），浅色用另一条。重建发生在
/// 切换的**下一帧**开头，所以在切换那一帧里做的任何作废（清缓存、给缓存键加配色
/// 版本号）都赶不上——下一帧缓存键原封不动地命中，返回的正是那份坐标已经失效的
/// galley。这就是「切明暗主题后 Markdown 编辑区不显示或显示混乱、鼠标点一下（改
/// 了光标行，缓存键跟着变）就恢复」的成因。
///
/// epaint 自己的 galley 缓存能正确识别重建，重新要一次只是一次哈希查表；拿回来的
/// `Arc` 指针没变就说明图集没动，后处理的成品可以接着用。
struct Cached {
    key: u64,
    width: u32,
    job: LayoutJob,
    /// epaint 上一次给出的 galley，仅用于指针判等。
    raw: Arc<egui::Galley>,
    /// 后处理（标题居中）之后真正交给 `TextEdit` 的成品。
    galley: Arc<egui::Galley>,
}

/// 缓存键命中就复用 `LayoutJob`，但 galley 每帧都向 epaint 重新要一次。
fn cached_galley(
    slot: &mut Option<Cached>,
    ui: &egui::Ui,
    key: u64,
    width: u32,
    build: impl FnOnce() -> LayoutJob,
    post: impl FnOnce(&mut Arc<egui::Galley>),
) -> Arc<egui::Galley> {
    if let Some(cached) = slot.as_mut()
        && cached.key == key
        && cached.width == width
    {
        let raw = ui
            .ctx()
            .fonts_mut(|fonts| fonts.layout_job(cached.job.clone()));
        if !Arc::ptr_eq(&raw, &cached.raw) {
            // epaint 重排过：字形坐标换了一套，后处理也要照着新的那份重做。
            let mut galley = raw.clone();
            post(&mut galley);
            cached.raw = raw;
            cached.galley = galley;
        }
        return cached.galley.clone();
    }
    let job = build();
    let raw = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job.clone()));
    let mut galley = raw.clone();
    post(&mut galley);
    *slot = Some(Cached {
        key,
        width,
        job,
        raw,
        galley: galley.clone(),
    });
    galley
}

impl MarkdownHighlighter {
    /// 供 `TextEdit::layouter` 调用：文本、宽度和锚点都没变时直接复用上一帧的排版。
    #[allow(clippy::too_many_arguments)]
    pub fn layout(
        &mut self,
        ui: &egui::Ui,
        text: &str,
        wrap_width: f32,
        base_size: f32,
        anchor: Option<&Range<usize>>,
        search_matches: &[Range<usize>],
        fonts: &EditorFontScheme,
        research: bool,
    ) -> Arc<egui::Galley> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        anchor.hash(&mut hasher);
        search_matches.hash(&mut hasher);
        base_size.to_bits().hash(&mut hasher);
        fonts.hash(&mut hasher);
        research.hash(&mut hasher);
        theme::revision().hash(&mut hasher);
        let key = hasher.finish();
        cached_galley(
            &mut self.cache,
            ui,
            key,
            wrap_width.to_bits(),
            || {
                highlight(
                    text,
                    wrap_width,
                    base_size,
                    anchor,
                    search_matches,
                    fonts,
                    research,
                )
            },
            |_| {},
        )
    }

    /// 缓存当前的键，供测试断言「配色一变，键就变」。
    #[cfg(test)]
    fn cache_key(&self) -> Option<u64> {
        self.cache.as_ref().map(|cached| cached.key)
    }
}

/// 编辑区默认行高相对字号的倍数：字号拉平后各行高度接近，行距略微放宽一点
/// 更透气，避免看着太挤。
const LINE_HEIGHT_RATIO: f32 = 1.35;

fn format(font: FontId, color: Color32) -> TextFormat {
    let line_height = Some(font.size * LINE_HEIGHT_RATIO);
    TextFormat {
        font_id: font,
        color,
        line_height,
        ..Default::default()
    }
}

fn filled(font: FontId, color: Color32, background: Color32) -> TextFormat {
    let line_height = Some(font.size * LINE_HEIGHT_RATIO);
    TextFormat {
        font_id: font,
        color,
        background,
        line_height,
        ..Default::default()
    }
}

/// 把整篇 Markdown 编译成带颜色的 `LayoutJob`；普通查找命中铺黄色底，当前命中
/// （`anchor`）再盖一层强调底色。`base_size` 是源码编辑器正文基准字号（px）。
/// `research` 为真时额外认研究报告的 mdx 扩展标记（`{#id}`、`{@id}`、`[@key]`、
/// `[^id]:(内容)`）。
pub fn highlight(
    text: &str,
    wrap_width: f32,
    base_size: f32,
    anchor: Option<&Range<usize>>,
    search_matches: &[Range<usize>],
    scheme: &EditorFontScheme,
    research: bool,
) -> LayoutJob {
    // 源码模式默认用独立的编辑器字体族：用户在设置里选了编辑器字体就生效，
    // 没选时族内整份回退到界面字体，行为与之前的 Proportional 一致。设置里把
    // 某一处换成公文字面时，那一处改用与预览、导出同源的那支字体。
    let fonts = EditorFonts {
        base_size,
        scheme: *scheme,
    };
    let body = fonts.font(EditorFontSlot::Body, base_size);

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
            job.append("\n", 0.0, format(body.clone(), theme::md::body()));
        }
        highlight_line(&mut job, line, &fonts, research);
    }
    for range in search_matches {
        paint_range(&mut job, range, theme::md::search_bg());
    }
    if let Some(anchor) = anchor {
        // 当前命中（或预览点击锚点）最后绘制，以更醒目的强调色盖过普通命中。
        paint_range(&mut job, anchor, theme::md::anchor_bg());
    }
    job
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

/// 一次高亮里各处元素该用哪支字面。字号仍按编辑器基准字号走，
/// 换的只是字面——源码编辑器首先得好编辑，不是第二个预览。
#[derive(Clone, Copy)]
struct EditorFonts {
    base_size: f32,
    scheme: EditorFontScheme,
}

impl EditorFonts {
    /// 某处元素在给定字号下的字体。
    fn font(&self, slot: EditorFontSlot, size: f32) -> FontId {
        FontId::new(size, theme::editor_face_family(self.scheme.face(slot)))
    }

    /// 结构标记的字体。标记跟着它所修饰的那段一起放大，否则标题行的 `#`
    /// 会比标题矮一截，看着像掉了行。
    fn mark(&self, size: f32) -> FontId {
        self.font(EditorFontSlot::Mark, size)
    }

    /// `#` 的个数对应的标题槽位；六级以内都有归属。
    fn heading_slot(hashes: usize) -> EditorFontSlot {
        match hashes {
            1 => EditorFontSlot::Title,
            2 => EditorFontSlot::Heading1,
            3 => EditorFontSlot::Heading2,
            _ => EditorFontSlot::Heading3,
        }
    }
}

fn highlight_line(job: &mut LayoutJob, line: &str, fonts: &EditorFonts, research: bool) {
    let base_size = fonts.base_size;
    let body = fonts.font(EditorFontSlot::Body, base_size);
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        job.append(line, 0.0, format(body, theme::md::body()));
        return;
    }
    let indent = &line[..line.len() - trimmed.len()];
    if !indent.is_empty() {
        job.append(indent, 0.0, format(body.clone(), theme::md::body()));
    }

    // 区段标记 `<!-- 附件 -->` 与裸 HTML：整行弱化为注释色。
    if trimmed.starts_with("<!--") || trimmed.starts_with('<') {
        job.append(
            trimmed,
            0.0,
            filled(
                fonts.mark(base_size),
                theme::md::comment(),
                theme::md::comment_bg(),
            ),
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
        // 层级主要靠字体（黑体/楷体/仿宋）和颜色区分，与正式预览
        // （`preview/layout.rs::heading_family`）一致；字号只留一点点提示，
        // 不再和正文拉开明显差距。
        let scale = match hashes {
            1 => 1.06,
            2 => 1.03,
            _ => 1.0,
        };
        let size = (base_size * scale).round();
        let font = fonts.font(EditorFonts::heading_slot(hashes), size);
        let color = if hashes == 1 {
            theme::md::title()
        } else {
            theme::md::heading()
        };
        let split = hashes + 1;
        job.append(
            &trimmed[..split],
            0.0,
            format(fonts.mark(size), theme::md::marker()),
        );
        append_inline(
            job,
            &trimmed[split..],
            &format(font, color),
            fonts,
            research,
        );
        return;
    }

    // 表格：竖线压成浅色，分隔行整行弱化，单元格内容用独立的蓝灰色。
    if trimmed.contains('|') && trimmed.matches('|').count() >= 2 {
        if is_separator_row(trimmed) {
            job.append(
                trimmed,
                0.0,
                format(fonts.mark(base_size), theme::md::table_rule()),
            );
            return;
        }
        let cell = format(body.clone(), theme::md::table_cell());
        let pipe = format(fonts.mark(base_size), theme::md::table_pipe());
        for piece in trimmed.split_inclusive('|') {
            let (content, bar) = match piece.strip_suffix('|') {
                Some(content) => (content, true),
                None => (piece, false),
            };
            append_inline(job, content, &cell, fonts, research);
            if bar {
                job.append("|", 0.0, pipe.clone());
            }
        }
        return;
    }

    // 列表：项目符号用强调色，内容照常走行内规则。
    if let Some(rest) = trimmed.strip_prefix("- ").or(trimmed.strip_prefix("* ")) {
        job.append(
            &trimmed[..2],
            0.0,
            format(fonts.mark(base_size), theme::md::bullet()),
        );
        append_inline(job, rest, &format(body, theme::md::body()), fonts, research);
        return;
    }

    if let Some((_, rest)) = export::parse_ordered_item(trimmed) {
        let split = rest.as_ptr() as usize - trimmed.as_ptr() as usize;
        job.append(
            &trimmed[..split],
            0.0,
            format(fonts.mark(base_size), theme::md::bullet()),
        );
        append_inline(job, rest, &format(body, theme::md::body()), fonts, research);
        return;
    }

    append_inline(
        job,
        trimmed,
        &format(body, theme::md::body()),
        fonts,
        research,
    );
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

/// 行内规则：`**加粗**`、`` `代码` ``、`【待核实：…】`、中文引号；`research` 为真时
/// 再认研究报告的扩展标记（`{#id}`、`{@id}`、`[@key]`、`[^id]:(内容)`）。
/// 未命中的部分按 `base` 输出，因此标题、表格单元都能复用这套扫描。
fn append_inline(
    job: &mut LayoutJob,
    text: &str,
    base: &TextFormat,
    fonts: &EditorFonts,
    research: bool,
) {
    let font = base.font_id.clone();
    let mark_font = fonts.mark(font.size);
    let mut plain_start = 0usize;
    let mut index = 0usize;
    while index < text.len() {
        let rest = &text[index..];
        let Some(Span {
            open_len,
            content_end,
            span_end,
            kind,
        }) = inline_span(rest, research)
        else {
            index += next_char_len(rest);
            continue;
        };
        if plain_start < index {
            job.append(&text[plain_start..index], 0.0, base.clone());
        }
        let (color, background, keep_marks) = match kind {
            Inline::Strong => (theme::md::strong(), theme::md::strong_bg(), false),
            Inline::Code => (theme::md::code(), theme::md::comment_bg(), false),
            Inline::Todo => (theme::md::todo(), theme::md::todo_bg(), true),
            Inline::Quoted => (theme::md::quoted(), Color32::TRANSPARENT, true),
            // 研究报告的扩展标记整体一个样式：行尾锚点是纯结构符号，压成标记弱色；
            // 交叉引用印出来是编号、性质接近链接，用强调色；文献引用与行内脚注各
            // 借一种现有的行内色，与前后正文区分开。
            Inline::Label => (theme::md::marker(), Color32::TRANSPARENT, true),
            Inline::Crossref => (theme::accent(), Color32::TRANSPARENT, true),
            Inline::Citation => (theme::md::quoted(), Color32::TRANSPARENT, true),
            Inline::Footnote => (theme::md::code(), Color32::TRANSPARENT, true),
        };
        // 行内代码是源码里才有的东西，跟着标记走；加粗、待核实、引号内都是
        // 成稿上的正文，继承所属元素的字面。
        let content_font = match kind {
            Inline::Code => mark_font.clone(),
            _ => font.clone(),
        };
        let content = filled(content_font, color, background);
        if keep_marks {
            job.append(&rest[..span_end], 0.0, content);
        } else {
            let marker = format(mark_font.clone(), theme::md::marker());
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
    /// 研究报告的行尾锚点 `{#id}`。
    Label,
    /// 研究报告的交叉引用 `{@id}`。
    Crossref,
    /// 研究报告的文献引用 `[@key]`、`[@a; @b]`。
    Citation,
    /// 研究报告的行内脚注 `[^id]:(内容)`。
    Footnote,
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
/// `research` 为真时额外认研究报告的 mdx 扩展标记（见 [`research_span`]）。
fn inline_span(rest: &str, research: bool) -> Option<Span> {
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
    if research {
        return research_span(rest);
    }
    None
}

/// 研究报告的 mdx 扩展标记：行尾锚点 `{#id}`、交叉引用 `{@id}`、文献引用
/// `[@key]` 与行内脚注 `[^id]:(内容)`。字符集与 `export::crossref` 的正则逐条
/// 对齐（那边靠 mdx 的契约测试兜底），这里手写扫描，不引入 regex。四类标记都
/// 整体一个样式，所以 `open_len` 置 0、`content_end` 指到段尾。
fn research_span(rest: &str) -> Option<Span> {
    let whole = |span_end, kind| {
        Some(Span {
            open_len: 0,
            content_end: span_end,
            span_end,
            kind,
        })
    };
    if rest.starts_with("{#") {
        return braced_id_end(rest, "{#", true).and_then(|end| whole(end, Inline::Label));
    }
    if rest.starts_with("{@") {
        return braced_id_end(rest, "{@", false).and_then(|end| whole(end, Inline::Crossref));
    }
    if rest.starts_with("[@") {
        return citation_end(rest).and_then(|end| whole(end, Inline::Citation));
    }
    if rest.starts_with("[^") {
        return footnote_end(rest).and_then(|end| whole(end, Inline::Footnote));
    }
    None
}

/// `{#id}` 与 `{@id}` 共用的骨架：`open` + id + `}`，id 是 `[A-Za-z][\w:.-]*`
/// （`\w` 按 Unicode 字母数字加下划线理解，与 regex 默认行为一致）。
/// `line_end` 为真时还要求 `}` 之后只剩空白——锚点只认行尾写法，行中间的
/// `{#…}` 在解析器眼里只是普通文字，不该上色。返回整段的字节结束位置。
fn braced_id_end(rest: &str, open: &str, line_end: bool) -> Option<usize> {
    let body = rest.strip_prefix(open)?;
    let mut chars = body.char_indices();
    let (_, first) = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    let mut id_len = first.len_utf8();
    for (index, ch) in chars {
        if ch.is_alphanumeric() || matches!(ch, '_' | ':' | '.' | '-') {
            id_len = index + ch.len_utf8();
        } else {
            break;
        }
    }
    let after = body[id_len..].strip_prefix('}')?;
    if line_end && !after.trim().is_empty() {
        return None;
    }
    Some(rest.len() - after.len())
}

/// 文献引用 `[@key]`、`[@a; @b]`：key 是 `[^\s@;,\[\]{}\\]+`，分号分隔多键，
/// 分号两侧允许空白。返回整段的字节结束位置。
fn citation_end(rest: &str) -> Option<usize> {
    let mut body = rest.strip_prefix('[')?;
    loop {
        body = body.strip_prefix('@')?;
        let key_len = body
            .find(|ch: char| {
                ch.is_whitespace() || matches!(ch, '@' | ';' | ',' | '[' | ']' | '{' | '}' | '\\')
            })
            .unwrap_or(body.len());
        if key_len == 0 {
            return None;
        }
        let trimmed = body[key_len..].trim_start();
        body = match trimmed.strip_prefix(';') {
            Some(next) => next.trim_start(),
            None => {
                let after = trimmed.strip_prefix(']')?;
                return Some(rest.len() - after.len());
            }
        };
    }
}

/// 行内脚注 `[^id]:(内容)`：id 是 `]` 以外的任意非空串，冒号兼容全角 `：`；
/// 括号要么一对半角、要么一对全角，内容里不能再出现同种闭括号（不支持嵌套，
/// 第一个闭括号收尾）。裸 `[^id]` 不带 `:(…)` 不匹配。返回整段的字节结束位置。
fn footnote_end(rest: &str) -> Option<usize> {
    let body = rest.strip_prefix("[^")?;
    let id_len = body.find(']')?;
    if id_len == 0 {
        return None;
    }
    let after_id = &body[id_len + ']'.len_utf8()..];
    let after_colon = after_id
        .strip_prefix(':')
        .or_else(|| after_id.strip_prefix('：'))?;
    let (open, close) = match after_colon.chars().next() {
        Some('(') => ('(', ')'),
        Some('（') => ('（', '）'),
        _ => return None,
    };
    let content = &after_colon[open.len_utf8()..];
    let content_len = content.find(close)?;
    // content 从开括号之后起算，所以差值里已含开括号，只补内容与闭括号。
    Some(rest.len() - content.len() + content_len + close.len_utf8())
}

fn next_char_len(rest: &str) -> usize {
    rest.chars().next().map_or(1, char::len_utf8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sections(text: &str) -> Vec<(String, Color32)> {
        sections_with(text, false)
    }

    /// 研究报告文类下的分段：扩展标记（`{#id}`、`{@id}` 等）只在此时上色。
    fn research_sections(text: &str) -> Vec<(String, Color32)> {
        sections_with(text, true)
    }

    fn sections_with(text: &str, research: bool) -> Vec<(String, Color32)> {
        let job = highlight(
            text,
            400.0,
            14.0,
            None,
            &[],
            &EditorFontScheme::default(),
            research,
        );
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

    /// 任何输入下 `LayoutJob` 的分段都必须首尾相接、完整覆盖原文，
    /// 否则 egui 会 panic 或错位。
    fn assert_covers(text: &str) {
        assert_covers_with(text, None);
    }

    fn assert_covers_with(text: &str, anchor: Option<&Range<usize>>) {
        let job = highlight(
            text,
            400.0,
            14.0,
            anchor,
            &[],
            &EditorFontScheme::default(),
            false,
        );
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
        let job = highlight(
            text,
            400.0,
            14.0,
            Some(&anchor),
            &[],
            &EditorFontScheme::default(),
            false,
        );

        for section in &job.sections {
            let (start, end) = (section.byte_range.start.0, section.byte_range.end.0);
            let inside = anchor.start <= start && end <= anchor.end;
            assert_eq!(
                section.format.background == theme::md::anchor_bg(),
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
            text,
            400.0,
            14.0,
            Some(&current),
            &matches,
            &EditorFontScheme::default(),
            false,
        );
        assert!(job.sections.iter().any(|section| {
            section.byte_range.start.0 == matches[0].start
                && section.format.background == theme::md::search_bg()
        }));
        assert!(job.sections.iter().any(|section| {
            section.byte_range.start.0 == current.start
                && section.format.background == theme::md::anchor_bg()
        }));
    }

    #[test]
    fn headings_dim_the_hashes_and_color_the_text() {
        let sections = sections("## 一级标题");
        assert_eq!(sections[0], ("## ".to_string(), theme::md::marker()));
        assert_eq!(sections[1], ("一级标题".to_string(), theme::md::heading()));
    }

    #[test]
    fn document_title_uses_the_accent_color() {
        let sections = sections("# 关于开展工作的函");
        assert_eq!(sections[1].1, theme::md::title());
    }

    #[test]
    fn markdown_source_uses_the_editor_font_family() {
        let job = highlight(
            "正文 **重点**",
            400.0,
            14.0,
            None,
            &[],
            &EditorFontScheme::default(),
            false,
        );
        // 源码模式走独立的编辑器字体族；族内回退链在 configure_fonts 里拼好。
        let expected = egui::FontFamily::Name(theme::EDITOR_FONT_FAMILY.into());
        assert!(
            job.sections
                .iter()
                .all(|section| section.format.font_id.family == expected)
        );
    }

    #[test]
    fn strong_marks_are_dimmed_and_content_is_highlighted() {
        let sections = sections("这是**重点**内容");
        let colors = sections.iter().map(|(_, color)| *color).collect::<Vec<_>>();
        assert!(colors.contains(&theme::md::strong()));
        assert_eq!(sections[1], ("**".to_string(), theme::md::marker()));
    }

    #[test]
    fn placeholders_and_quotes_keep_their_brackets() {
        let sections = sections("【待核实：文号】与“规范”");
        assert_eq!(
            sections[0],
            ("【待核实：文号】".to_string(), theme::md::todo())
        );
        assert!(
            sections
                .iter()
                .any(|(text, color)| text == "“规范”" && *color == theme::md::quoted())
        );
    }

    /// 研究报告的行尾锚点 `{#id}`：整段压成标记弱色；只认行尾写法，行中间的
    /// `{#…}` 与解析器一样按普通文字处理。
    #[test]
    fn research_label_dims_only_at_line_end() {
        let at_end = research_sections("## 研究背景 {#chap:bg}");
        assert_eq!(
            at_end.last().expect("锚点分段"),
            &("{#chap:bg}".to_string(), theme::md::marker())
        );

        let mid_line = research_sections("正文 {#chap:bg} 还有字");
        let middle = mid_line
            .iter()
            .find(|(text, _)| text.contains("{#chap:bg}"))
            .expect("行中锚点仍在分段里");
        assert_eq!(middle.1, theme::md::body(), "行中的 {{#…}} 不该上色");
    }

    /// 交叉引用 `{@id}` 整段用强调色（印出来是编号，性质接近链接）。
    #[test]
    fn research_crossref_uses_the_accent_color() {
        let sections = research_sections("见第{@chap:bg}章、表{@tbl:t}。");
        for mark in ["{@chap:bg}", "{@tbl:t}"] {
            assert!(
                sections
                    .iter()
                    .any(|(text, color)| text == mark && *color == theme::accent()),
                "{mark} 应整段强调色：{sections:?}"
            );
        }
    }

    /// 文献引用 `[@key]` 与分号多键 `[@a; @b]` 各整段一个样式；残缺的 `[@]`、
    /// `[@a` 不上色。
    #[test]
    fn research_citation_colors_single_and_multi_keys() {
        let colored = research_sections("综述[@wang2020]与[@wang2020; @li2021]。");
        for mark in ["[@wang2020]", "[@wang2020; @li2021]"] {
            assert!(
                colored
                    .iter()
                    .any(|(text, color)| text == mark && *color == theme::md::quoted()),
                "{mark} 应整段上色：{colored:?}"
            );
        }

        let broken = research_sections("残缺的 [@] 与 [@a 保持正文色");
        assert!(
            broken.iter().all(|(_, color)| *color == theme::md::body()),
            "残缺的引用键不该上色：{broken:?}"
        );
    }

    /// 行内脚注 `[^id]:(内容)`：整个标记（含内容）一个样式，冒号与括号兼容
    /// 全角；裸 `[^id]` 不带 `:(…)` 不匹配。
    #[test]
    fn research_footnote_accepts_fullwidth_colon_and_parens() {
        let colored = research_sections("脚注[^n1]:(详见附录)与[^n2]：（再注）收尾。");
        for mark in ["[^n1]:(详见附录)", "[^n2]：（再注）"] {
            assert!(
                colored
                    .iter()
                    .any(|(text, color)| text == mark && *color == theme::md::code()),
                "{mark} 应整段上色：{colored:?}"
            );
        }

        let bare = research_sections("裸脚注 [^n3] 保持正文色");
        assert!(
            bare.iter().all(|(_, color)| *color == theme::md::body()),
            "裸 [^id] 不该上色：{bare:?}"
        );
    }

    /// 非研究报告里这些扩展标记只是普通文字：四种标记一个都不上色。
    #[test]
    fn research_marks_stay_plain_outside_research_reports() {
        let plain = sections("见第{@chap:bg}章 [@wang2020]，脚注[^n]:(详见附录)。");
        assert!(
            plain.iter().all(|(_, color)| *color == theme::md::body()),
            "非研究报告不该给扩展标记上色：{plain:?}"
        );
        // 标题行里也一样：锚点不被认出，跟着标题文字一起走标题色。
        let heading = sections("## 研究背景 {#chap:bg}");
        assert_eq!(
            heading.last().expect("标题分段"),
            &("研究背景 {#chap:bg}".to_string(), theme::md::heading())
        );
    }

    /// 扩展标记切开分段后仍要完整覆盖原文，边界不能落在字符中间。
    #[test]
    fn research_marks_keep_section_coverage() {
        let text = "## 研究背景 {#chap:bg}\n\n见第{@chap:bg}章 [@a; @b]，脚注[^n]:(注)\
            与[^m]：（全角）。孤立的 {#x} 行中与 [@] 与 [^k] 不成形。";
        let job = highlight(
            text,
            400.0,
            14.0,
            None,
            &[],
            &EditorFontScheme::default(),
            true,
        );
        assert_eq!(job.text, text);
        let mut cursor = 0usize;
        for section in &job.sections {
            assert_eq!(section.byte_range.start.0, cursor);
            cursor = section.byte_range.end.0;
            assert!(job.text.is_char_boundary(cursor));
        }
        assert_eq!(cursor, job.text.len());
    }

    #[test]
    fn table_rows_split_pipes_from_cells() {
        let sections = sections("| 姓名 | 电话 |");
        assert!(
            sections
                .iter()
                .any(|(text, color)| text == "|" && *color == theme::md::table_pipe())
        );
        assert!(
            sections
                .iter()
                .any(|(_, color)| *color == theme::md::table_cell())
        );
    }

    #[test]
    fn separator_rows_are_dimmed_as_a_whole() {
        let sections = sections("|---|---|");
        assert_eq!(sections[0].1, theme::md::table_rule());
    }

    #[test]
    fn section_markers_render_as_comments() {
        let sections = sections("<!-- 附件 -->");
        assert_eq!(sections[0].1, theme::md::comment());
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

    /// 每段文字用的字体族，供字面方案的用例断言。
    fn families(text: &str, scheme: &EditorFontScheme) -> Vec<(String, egui::FontFamily)> {
        let job = highlight(text, 400.0, 14.0, None, &[], scheme, false);
        job.sections
            .iter()
            .map(|section| {
                (
                    job.text[section.byte_range.start.0..section.byte_range.end.0].to_string(),
                    section.format.font_id.family.clone(),
                )
            })
            .collect()
    }

    /// 某段文字用的字体族；`text` 必须在结果里唯一出现一次。
    fn family_of(sections: &[(String, egui::FontFamily)], needle: &str) -> egui::FontFamily {
        // 相邻同格式的分段会被 `LayoutJob` 合并，正文段前的换行因此常常粘在
        // 正文头上；比对时把它剥掉。
        let mut hit = sections
            .iter()
            .filter(|(piece, _)| piece.trim_start_matches('\n') == needle);
        let (_, family) = hit.next().unwrap_or_else(|| {
            panic!("没有找到分段 {needle:?}：{sections:?}");
        });
        assert!(hit.next().is_none(), "分段 {needle:?} 出现了不止一次");
        family.clone()
    }

    const SAMPLE: &str = "# 关于加强某项工作的通知\n## 一、总体要求\n### （一）指导思想\n#### 1. 基本原则\n各地各校要按期报送。";

    /// 默认方案就是加入这项设置之前的样子：六处全用编辑器字体。
    #[test]
    fn the_default_scheme_keeps_everything_on_the_editor_font() {
        let editor = theme::editor_face_family(crate::models::EditorFontFace::Editor);
        for (piece, family) in families(SAMPLE, &EditorFontScheme::default()) {
            assert_eq!(family, editor, "默认方案下 {piece:?} 不该换字面");
        }
    }

    /// 「与公文一致」：`#` 小标宋、`##` 黑体、`###` 楷体，其余仿宋，
    /// 而只在源码里出现的 `#` 标记仍留在编辑器字体上，方便一眼认出。
    #[test]
    fn the_official_preset_maps_each_level_to_its_document_face() {
        use crate::models::{EditorFontFace, EditorFontPreset};
        let scheme = EditorFontPreset::Official.scheme();
        let sections = families(SAMPLE, &scheme);
        for (needle, face) in [
            ("关于加强某项工作的通知", EditorFontFace::Biaosong),
            ("一、总体要求", EditorFontFace::Heiti),
            ("（一）指导思想", EditorFontFace::Kaiti),
            ("1. 基本原则", EditorFontFace::Fangsong),
            ("各地各校要按期报送。", EditorFontFace::Fangsong),
        ] {
            assert_eq!(
                family_of(&sections, needle),
                theme::editor_face_family(face),
                "{needle:?} 应当排成{}",
                face.label()
            );
        }
        let editor = theme::editor_face_family(EditorFontFace::Editor);
        for marker in ["# ", "## ", "### ", "#### "] {
            assert_eq!(
                family_of(&sections, marker),
                editor,
                "{marker:?} 是源码里才有的标记，应留在编辑器字体上"
            );
        }
    }

    /// 「标题随公文」只换标题：正文仍是编辑器字体，长段落照旧好编辑。
    #[test]
    fn the_heading_preset_leaves_the_body_on_the_editor_font() {
        use crate::models::{EditorFontFace, EditorFontPreset};
        let sections = families(SAMPLE, &EditorFontPreset::OfficialHeadings.scheme());
        assert_eq!(
            family_of(&sections, "关于加强某项工作的通知"),
            theme::editor_face_family(EditorFontFace::Biaosong)
        );
        assert_eq!(
            family_of(&sections, "各地各校要按期报送。"),
            theme::editor_face_family(EditorFontFace::Editor)
        );
    }

    /// 换字面也要让缓存失效，否则和切主题一样会停在上一套字体上。
    #[test]
    fn changing_the_font_scheme_changes_the_cache_key() {
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let mut highlighter = MarkdownHighlighter::default();
        let mut key_with = |scheme: &EditorFontScheme| {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                highlighter.layout(ui, SAMPLE, 400.0, 14.0, None, &[], scheme, false);
            });
            highlighter.cache_key()
        };
        let editor = key_with(&EditorFontScheme::default());
        let official = key_with(&crate::models::EditorFontPreset::Official.scheme());
        assert_ne!(editor, official, "换了字面方案就得重新排版");
    }
}
