//! 审校区的 Markdown 语法高亮。
//!
//! 只做“看得清”这一件事：把结构性符号（`#`、`|`、`**`、区段标记）压成弱色，
//! 把真正要读的内容（标题、表格单元、待核实占位）提亮，让一屏文字有层次。
//! 另外给“锚点”——即在公文预览里点中的那一块——铺一层淡底，两栏对照时一眼能
//! 看出版式上的哪一段对应源码里的哪一段。
//! 高亮结果按（文本, 换行宽度, 锚点, 查找命中, 配色版本）缓存，正常编辑时每帧
//! 只需一次哈希；配色版本让切主题、换纸面之后的第一帧就重新上色，而不是等到下
//! 一次改字或挪光标。缓存里存的是 `LayoutJob` 而不是排好的 `Galley`，理由见
//! [`Cached`]。

use crate::models::{EditorFontScheme, EditorFontSlot, NumberingConfig};
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
    hybrid_cache: Option<Cached>,
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
    ) -> Arc<egui::Galley> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        anchor.hash(&mut hasher);
        search_matches.hash(&mut hasher);
        base_size.to_bits().hash(&mut hasher);
        fonts.hash(&mut hasher);
        theme::revision().hash(&mut hasher);
        let key = hasher.finish();
        cached_galley(
            &mut self.cache,
            ui,
            key,
            wrap_width.to_bits(),
            || highlight(text, wrap_width, base_size, anchor, search_matches, fonts),
            |_| {},
        )
    }

    /// “实时排版”编辑器的布局：光标所在行保留 Markdown 标记，
    /// 其余行折叠标记并使用公文字体、字号和固定行距。
    #[allow(clippy::too_many_arguments)]
    pub fn layout_hybrid(
        &mut self,
        ui: &egui::Ui,
        text: &str,
        wrap_width: f32,
        active_line: usize,
        anchor: Option<&Range<usize>>,
        search_matches: &[Range<usize>],
        numbering: &NumberingConfig,
    ) -> Arc<egui::Galley> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        anchor.hash(&mut hasher);
        search_matches.hash(&mut hasher);
        numbering.hash(&mut hasher);
        active_line.hash(&mut hasher);
        theme::revision().hash(&mut hasher);
        let key = hasher.finish();
        cached_galley(
            &mut self.hybrid_cache,
            ui,
            key,
            wrap_width.to_bits(),
            || {
                hybrid_highlight(
                    ui.style(),
                    text,
                    wrap_width,
                    active_line,
                    anchor,
                    search_matches,
                    numbering,
                )
            },
            |galley| center_document_title_rows(galley, text, wrap_width),
        )
    }

    /// 两份缓存当前的键，供测试断言「配色一变，键就变」。
    #[cfg(test)]
    fn cache_keys(&self) -> (Option<u64>, Option<u64>) {
        (
            self.cache.as_ref().map(|cached| cached.key),
            self.hybrid_cache.as_ref().map(|cached| cached.key),
        )
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

/// 把正式标题（文档标题与每个附件的正式标题）的所有视觉行居中。
/// 布局阶段先按普通段落排版，这里再逐个视觉行居中，长标题换行时第二行
/// 也不会回到版心左边。
fn center_document_title_rows(galley: &mut Arc<egui::Galley>, text: &str, width: f32) {
    // 复用实时排版计数器，找出需要居中的逻辑行号：文档标题与附件正式标题。
    let mut counters = export::HeadingCounters::default();
    let mut centered_lines = Vec::new();
    for (logical, line) in text.split('\n').enumerate() {
        counters.next(line);
        if counters.centered_title() {
            centered_lines.push(logical);
        }
    }
    let Some(last_centered) = centered_lines.last().copied() else {
        return;
    };
    let galley = Arc::make_mut(galley);
    let mut logical_line = 0usize;
    let mut centered_max_x = 0.0f32;
    for placed in &mut galley.rows {
        if centered_lines.contains(&logical_line) {
            placed.pos.x = ((width - placed.size.x) * 0.5).max(0.0);
            centered_max_x = centered_max_x.max(placed.pos.x + placed.size.x);
        }
        if placed.ends_with_newline {
            logical_line += 1;
            if logical_line > last_centered {
                break;
            }
        }
    }
    // 居中会把标题行移到内容包围盒之外，而 galley.rect 仍是布局时的旧值。
    // TextEdit（multiline 布局）按 galley.size()（=rect 内容包围盒）报告控件宽度，
    // 于是文档只有短标题时编辑区被压成标题行那么宽，居中后的标题落在裁剪区外
    // ——表现为“大标题显示不出来”，后续正文/二级标题够长把 rect 撑宽后才恢复。
    // 这里把包围盒扩展到编辑区宽度（left 保持 0、高度不变），让控件始终占满
    // wrap 宽度，标题完整居中显示；mesh_bounds 同步扩展，避免 culling 误判。
    galley.rect.max.x = width.max(galley.rect.max.x);
    galley.rect.max.x = galley.rect.max.x.max(centered_max_x);
    let bounds = egui::Rect::from_min_max(
        egui::pos2(0.0, galley.rect.min.y),
        egui::pos2(galley.rect.max.x, galley.rect.max.y),
    );
    galley.mesh_bounds = galley.mesh_bounds.union(bounds);
}

/// 把整篇 Markdown 编译成带颜色的 `LayoutJob`；普通查找命中铺黄色底，当前命中
/// （`anchor`）再盖一层强调底色。`base_size` 是源码编辑器正文基准字号（px）。
pub fn highlight(
    text: &str,
    wrap_width: f32,
    base_size: f32,
    anchor: Option<&Range<usize>>,
    search_matches: &[Range<usize>],
    scheme: &EditorFontScheme,
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
        highlight_line(&mut job, line, &fonts);
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

/// 把 Markdown 作为唯一数据源的所见即所得布局。这不生成第二份富文本：
/// 每个字符仍在 galley 中，只是非活动行的结构标记被折叠。
pub fn hybrid_highlight(
    _style: &egui::Style,
    text: &str,
    wrap_width: f32,
    active_line: usize,
    anchor: Option<&Range<usize>>,
    search_matches: &[Range<usize>],
    numbering: &NumberingConfig,
) -> LayoutJob {
    let body = official_format(
        theme::FONT_FANGSONG,
        OFFICIAL_BODY_PT,
        OFFICIAL_LINE_PT,
        theme::paper::ink(),
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
    let ordered_lines = ordered_list_lines(text);
    let mut counters = export::HeadingCounters::with_numbering(*numbering);
    let mut in_table = false;
    for (index, line) in source_lines.iter().copied().enumerate() {
        let prefix = counters.next(line);
        let centered = counters.centered_title();
        if index > 0 {
            let previous_is_blank = source_lines[index - 1].trim().is_empty();
            let inline_ordered =
                ordered_lines[index].is_some_and(|info| info.inline) && index != active_line;
            let newline = if inline_ordered || (previous_is_blank && index - 1 != active_line) {
                collapsed_marker()
            } else {
                body.clone()
            };
            job.append("\n", 0.0, newline);
        }
        let trimmed_table = line.trim();
        let table_line = trimmed_table.contains('|') && trimmed_table.matches('|').count() >= 2;
        let table_header = table_line && !in_table;
        in_table = table_line;
        hybrid_line(
            &mut job,
            line,
            index == active_line,
            prefix.as_deref(),
            centered,
            table_header,
            ordered_lines[index],
            numbering,
        );
    }
    for range in search_matches {
        paint_range(&mut job, range, theme::md::search_bg());
    }
    if let Some(anchor) = anchor {
        paint_range(&mut job, anchor, theme::md::anchor_bg());
    }
    job
}

#[allow(clippy::too_many_arguments)]
fn hybrid_line(
    job: &mut LayoutJob,
    line: &str,
    active: bool,
    heading_prefix: Option<&str>,
    centered_title: bool,
    table_header: bool,
    ordered: Option<OrderedLine>,
    numbering: &NumberingConfig,
) {
    let body = official_format(
        theme::FONT_FANGSONG,
        OFFICIAL_BODY_PT,
        OFFICIAL_LINE_PT,
        theme::paper::ink(),
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
            format.color = theme::md::comment();
            format.background = theme::md::comment_bg();
            format
        } else {
            hidden_marker(OFFICIAL_LINE_PT)
        };
        job.append(line, 0.0, marker);
        return;
    }

    // 独占一行的图片引用：实时排版是文本编辑器，无法内嵌图片，整行加淡底
    // 高亮为占位提示，指引到公文预览查看实际图片。
    if export::is_image_line(trimmed) {
        let mut format = body;
        format.color = theme::md::heading();
        format.background = theme::md::image_bg();
        job.append(line, 0.0, format);
        return;
    }

    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    if (1..=6).contains(&hashes)
        && trimmed
            .as_bytes()
            .get(hashes)
            .is_some_and(u8::is_ascii_whitespace)
    {
        // 文档标题与附件正式标题（每个附件标记后的 `#`）按方正小标宋二号居中渲染，
        // 与预览/导出一致；其余标题按公文字号表：正文一级黑体、二级楷体。
        let (family, size) = if hashes == 1 || centered_title {
            (theme::FONT_BIAOSONG, OFFICIAL_TITLE_PT)
        } else {
            match hashes {
                2 => (theme::FONT_HEITI, OFFICIAL_BODY_PT),
                3 => (theme::FONT_KAITI, OFFICIAL_BODY_PT),
                _ => (theme::FONT_FANGSONG, OFFICIAL_BODY_PT),
            }
        };
        let text_format = official_format(family, size, OFFICIAL_LINE_PT, theme::paper::ink());
        let marker = if active {
            let mut format = text_format.clone();
            format.color = theme::md::marker();
            format
        } else {
            hidden_marker(OFFICIAL_LINE_PT)
        };
        let split = hashes + 1;
        let mut leading = if hashes == 1 || centered_title {
            // 正式标题：先按正常段落排版，galley 生成后再逐个视觉行居中，
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
            theme::paper::ink(),
        );
        if is_separator_row(trimmed) && !active {
            job.append(line, 0.0, collapsed_marker());
            return;
        }
        let marker = if active {
            let mut format = table.clone();
            format.color = theme::md::table_pipe();
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
            format.color = theme::md::bullet();
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

    if let Some(info) = ordered
        && let Some((_, rest)) = export::parse_ordered_item(trimmed)
    {
        let split = rest.as_ptr() as usize - trimmed.as_ptr() as usize;
        let marker = if active {
            let mut format = body.clone();
            format.color = theme::md::bullet();
            format
        } else {
            collapsed_marker()
        };
        let display = if info.inline {
            export::render_list_number(numbering.list1, info.number)
        } else {
            export::render_list_number(numbering.list2, info.number)
        };
        let display_units = display
            .chars()
            .map(|ch| if ch.is_ascii() { 0.55 } else { 1.0 })
            .sum::<f32>();
        let mut leading = if info.inline {
            0.0
        } else {
            OFFICIAL_BODY_PT * PT * 2.0
        };
        if !active {
            leading += display_units * OFFICIAL_BODY_PT * PT;
        }
        append_with_leading(job, indent, &mut leading, body.clone());
        append_with_leading(job, &trimmed[..split], &mut leading, marker);
        append_hybrid_inline(job, rest, &body, active, &mut leading);
        return;
    }

    // 普通段落按成文规则首行缩进 2 字。leading_space 只是布局信息，
    // 不会向 Markdown 里真正插入全角空格。
    let mut leading = OFFICIAL_BODY_PT * PT * 2.0;
    append_with_leading(job, indent, &mut leading, body.clone());
    append_hybrid_inline(job, trimmed, &body, active, &mut leading);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OrderedLine {
    pub(crate) number: usize,
    pub(crate) inline: bool,
}

/// 每个源码行对应的有序列表显示信息。是否为段内列表直接复用解析器的块范围，
/// 避免实时编辑器另写一套“有没有空行”的判断后逐渐漂移。
pub(crate) fn ordered_list_lines(text: &str) -> Vec<Option<OrderedLine>> {
    let source_lines = text.split('\n').collect::<Vec<_>>();
    let mut starts = Vec::with_capacity(source_lines.len());
    let mut start = 0usize;
    for line in &source_lines {
        starts.push(start);
        start += line.len() + 1;
    }
    let located = export::parse_markdown_located(text);
    let mut result = vec![None; source_lines.len()];
    let mut next_number = None;
    let mut group_inline = false;
    for (index, line) in source_lines.iter().enumerate() {
        if let Some((source_number, _)) = export::parse_ordered_item(line.trim_end()) {
            if next_number.is_none() {
                group_inline = located.iter().any(|block| {
                    matches!(&block.block, export::MarkdownBlock::Paragraph(_))
                        && block.range.start <= starts[index]
                        && starts[index] <= block.range.end
                });
            }
            let number = next_number.unwrap_or(source_number);
            result[index] = Some(OrderedLine {
                number,
                inline: group_inline,
            });
            next_number = Some(number + 1);
        } else {
            next_number = None;
            group_inline = false;
        }
    }
    result
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
                    format.color = theme::md::marker();
                    format
                } else {
                    hidden_marker(OFFICIAL_LINE_PT)
                };
                let strong = official_format(
                    theme::FONT_HEITI,
                    OFFICIAL_BODY_PT,
                    OFFICIAL_LINE_PT,
                    theme::paper::ink(),
                );
                append_with_leading(job, &rest[..open_len], leading, marker.clone());
                append_with_leading(job, &rest[open_len..content_end], leading, strong);
                append_with_leading(job, &rest[content_end..span_end], leading, marker);
            }
            Inline::Code => {
                let marker = if active {
                    let mut format = base.clone();
                    format.color = theme::md::marker();
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

fn highlight_line(job: &mut LayoutJob, line: &str, fonts: &EditorFonts) {
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
        let scale = match hashes {
            1 => 1.30,
            2 => 1.16,
            3 => 1.07,
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
        append_inline(job, &trimmed[split..], &format(font, color), fonts);
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
            append_inline(job, content, &cell, fonts);
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
        append_inline(job, rest, &format(body, theme::md::body()), fonts);
        return;
    }

    if let Some((_, rest)) = export::parse_ordered_item(trimmed) {
        let split = rest.as_ptr() as usize - trimmed.as_ptr() as usize;
        job.append(
            &trimmed[..split],
            0.0,
            format(fonts.mark(base_size), theme::md::bullet()),
        );
        append_inline(job, rest, &format(body, theme::md::body()), fonts);
        return;
    }

    append_inline(job, trimmed, &format(body, theme::md::body()), fonts);
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
fn append_inline(job: &mut LayoutJob, text: &str, base: &TextFormat, fonts: &EditorFonts) {
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
        }) = inline_span(rest)
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
        let job = highlight(text, 400.0, 14.0, None, &[], &EditorFontScheme::default());
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
        let job = hybrid_highlight(
            &egui::Style::default(),
            text,
            600.0,
            active_line,
            None,
            &[],
            &crate::models::NumberingConfig::default(),
        );
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
        let job = highlight(text, 400.0, 14.0, anchor, &[], &EditorFontScheme::default());
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
    fn hybrid_marks_image_lines_with_placeholder_background() {
        // 独占一行的图片引用在实时排版里以淡底高亮为占位提示（TextEdit 无法内嵌图片）。
        let text = "# 标题\n\n正文。\n\n![示意图](images/20260809_120000_示意图.png)\n\n结尾。";
        let sections = hybrid_sections(text, 0);
        let image_line = sections
            .iter()
            .find(|(t, _)| t.contains("![示意图]"))
            .expect("图片行应有分段");
        assert_eq!(
            image_line.1.background,
            theme::md::image_bg(),
            "图片行应铺图片占位底色"
        );
        // 普通正文行不铺底。
        let body_line = sections
            .iter()
            .find(|(t, _)| t.contains("正文。"))
            .expect("正文行应有分段");
        assert_eq!(body_line.1.background, Color32::TRANSPARENT);
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
        let job = hybrid_highlight(
            &egui::Style::default(),
            text,
            600.0,
            usize::MAX,
            None,
            &[],
            &crate::models::NumberingConfig::default(),
        );
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
            let job = hybrid_highlight(
                &egui::Style::default(),
                text,
                600.0,
                usize::MAX,
                None,
                &[],
                &crate::models::NumberingConfig::default(),
            );
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

    /// 附件正式标题（附件区 `##`）与文档标题一样按方正小标宋二号居中渲染。
    #[test]
    fn attachment_formal_title_uses_biaosong_and_is_centered() {
        let text = "# 测试函\n<!-- [正文] -->\n## 一、总体要求\n正文。\n<!-- [附件] -->\n# 统计表\n## 一、填报说明\n附件内容。";
        let sections = hybrid_sections(text, usize::MAX);
        let title = sections
            .iter()
            .find(|(t, _)| t == "统计表")
            .expect("附件正式标题分段");
        assert_eq!(
            title.1.font_id.family,
            theme::official_family(theme::FONT_BIAOSONG)
        );
        assert_eq!(title.1.font_id.size, OFFICIAL_TITLE_PT * PT);

        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let mut centered = None;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let job = hybrid_highlight(
                &egui::Style::default(),
                text,
                600.0,
                usize::MAX,
                None,
                &[],
                &crate::models::NumberingConfig::default(),
            );
            let mut galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
            center_document_title_rows(&mut galley, text, 600.0);
            centered = Some(galley);
        });
        let galley = centered.expect("已排版");
        let mut found = false;
        let mut logical = 0usize;
        for row in &galley.rows {
            if logical == 5 {
                // 第 5 个逻辑行是附件正式标题 "# 统计表"。
                assert!(
                    (row.pos.x - (600.0 - row.size.x) * 0.5).abs() < 1.0,
                    "附件正式标题应居中"
                );
                found = true;
                break;
            }
            if row.ends_with_newline {
                logical += 1;
            }
        }
        assert!(found, "应找到附件正式标题行");
    }

    #[test]
    fn hybrid_collapses_markdown_only_blank_lines() {
        let text = "第一段\n\n第二段";
        let job = hybrid_highlight(
            &egui::Style::default(),
            text,
            600.0,
            usize::MAX,
            None,
            &[],
            &crate::models::NumberingConfig::default(),
        );
        assert!(job.sections.iter().any(|section| {
            &job.text[section.byte_range.start.0..section.byte_range.end.0] == "\n"
                && section.format.line_height == Some(1.0)
        }));
    }

    #[test]
    fn hybrid_table_reserves_real_cell_widths_and_hides_pipes() {
        let text = "| 部门 | 负责人 | 时限 |";
        let job = hybrid_highlight(
            &egui::Style::default(),
            text,
            600.0,
            usize::MAX,
            None,
            &[],
            &crate::models::NumberingConfig::default(),
        );
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
        let job = hybrid_highlight(
            &egui::Style::default(),
            text,
            600.0,
            2,
            None,
            &[],
            &crate::models::NumberingConfig::default(),
        );
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

    /// 历史 bug：文档只有一行短标题（`# 标题`）时，居中把标题行移到内容包围盒
    /// 之外，而 TextEdit 按 galley.rect 报告控件宽度，编辑区被压成标题行那么宽，
    /// 居中后的标题落在裁剪区外显示不出来；后续内容够长把 rect 撑宽后才恢复。
    /// 修复后 rect/mesh_bounds 必须扩展到 wrap 宽度，标题行始终落在包围盒内。
    #[test]
    fn centered_short_title_expands_galley_rect_to_wrap_width() {
        let ctx = egui::Context::default();
        // headless 测试：先用默认字体绑定公文字体族（不加载真实字体文件），
        // 再跑一帧让字体可用；布局几何不依赖具体字体。
        {
            let mut fonts = egui::FontDefinitions::default();
            let fallback = fonts.families[&egui::FontFamily::Proportional].clone();
            for family in [
                theme::FONT_FANGSONG,
                theme::FONT_HEITI,
                theme::FONT_KAITI,
                theme::FONT_BIAOSONG,
            ] {
                fonts
                    .families
                    .insert(egui::FontFamily::Name(family.into()), fallback.clone());
            }
            ctx.set_fonts(fonts);
        }
        let _ = ctx.run_ui(egui::RawInput::default(), |_| {});
        for text in ["# 大标题\n", "# 大标题", "# 大标题\n\n正文。\n"] {
            let job = hybrid_highlight(
                &egui::Style::default(),
                text,
                600.0,
                usize::MAX,
                None,
                &[],
                &crate::models::NumberingConfig::default(),
            );
            let mut galley = ctx.fonts_mut(|fonts| fonts.layout_job(job));
            center_document_title_rows(&mut galley, text, 600.0);
            assert!(
                galley.rect.max.x >= 600.0,
                "仅短标题时 rect 应扩展到 wrap 宽度：{text:?} rect={:?}",
                galley.rect,
            );
            assert!(
                galley.rect.min.x == 0.0 && galley.rect.max.x <= 600.0 + 0.5,
                "rect 不应向左偏移或超出 wrap 宽度：{text:?} rect={:?}",
                galley.rect,
            );
            let title_row = &galley.rows[0];
            assert!(
                title_row.pos.x + title_row.size.x <= galley.rect.max.x + 0.5
                    && title_row.pos.x >= galley.rect.min.x - 0.5,
                "标题行居中后应落在扩展后的包围盒内：{text:?} row_pos={:?} rect={:?}",
                title_row.pos,
                galley.rect,
            );
        }
    }

    #[test]
    fn ordered_list_visuals_follow_blank_line_classification_and_auto_numbering() {
        let lines =
            ordered_list_lines("正文：\n1. 第一项；\n1. 第二项。\n\n3. 独立甲。\n1. 独立乙。");
        assert_eq!(
            lines[1],
            Some(OrderedLine {
                number: 1,
                inline: true,
            })
        );
        assert_eq!(
            lines[2],
            Some(OrderedLine {
                number: 2,
                inline: true,
            })
        );
        assert_eq!(
            lines[4],
            Some(OrderedLine {
                number: 3,
                inline: false,
            })
        );
        assert_eq!(
            lines[5],
            Some(OrderedLine {
                number: 4,
                inline: false,
            })
        );
    }

    /// 每段文字用的字体族，供字面方案的用例断言。
    fn families(text: &str, scheme: &EditorFontScheme) -> Vec<(String, egui::FontFamily)> {
        let job = highlight(text, 400.0, 14.0, None, &[], scheme);
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
                highlighter.layout(ui, SAMPLE, 400.0, 14.0, None, &[], scheme);
            });
            highlighter.cache_keys().0
        };
        let editor = key_with(&EditorFontScheme::default());
        let official = key_with(&crate::models::EditorFontPreset::Official.scheme());
        assert_ne!(editor, official, "换了字面方案就得重新排版");
    }

    /// 明暗互换会改 `Visuals::text_options`（深浅底的字形灰度曲线不同），epaint
    /// 因此在下一帧开头整份重建字形图集，之前排好的 galley 里记的图集坐标全部作
    /// 废——继续画就是缺字或乱码。缓存键这时一个字节都没变（文本、宽度、配色版本
    /// 号都不受 egui 内部 visuals 影响），所以只有「每帧重新向 epaint 要一次」才
    /// 能接住这次重建。
    #[test]
    fn swapping_light_and_dark_relayouts_against_the_rebuilt_font_atlas() {
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let mut highlighter = MarkdownHighlighter::default();
        let layout_once = |highlighter: &mut MarkdownHighlighter| {
            let mut galleys = None;
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                galleys = Some((
                    highlighter.layout(
                        ui,
                        SAMPLE,
                        400.0,
                        14.0,
                        None,
                        &[],
                        &EditorFontScheme::default(),
                    ),
                    highlighter.layout_hybrid(
                        ui,
                        SAMPLE,
                        600.0,
                        0,
                        None,
                        &[],
                        &crate::models::NumberingConfig::default(),
                    ),
                ));
            });
            galleys.expect("run_ui 一定跑过闭包")
        };

        ctx.set_theme(egui::ThemePreference::Light);
        let light = layout_once(&mut highlighter);
        assert!(
            Arc::ptr_eq(&light.0, &layout_once(&mut highlighter).0),
            "配色与字形图集都没动就该复用同一份 galley"
        );
        let keys = highlighter.cache_keys();

        ctx.set_theme(egui::ThemePreference::Dark);
        let dark = layout_once(&mut highlighter);

        assert_eq!(keys, highlighter.cache_keys(), "明暗互换不会改变缓存键");
        assert!(
            !Arc::ptr_eq(&light.0, &dark.0),
            "源码模式必须换成按新图集排的 galley"
        );
        assert!(
            !Arc::ptr_eq(&light.1, &dark.1),
            "实时排版必须换成按新图集排的 galley"
        );
    }

    /// 切主题只改全局配色、不改一个字：缓存键要是只看文本，编辑区就会继续画着
    /// 上一套颜色，直到用户挪一下光标或敲一个字才刷新——这正是「切完主题显示不
    /// 对、点一下才恢复」的成因。这里特意重新选中当前那套主题：配色版本照样 +1，
    /// 而全局调色板原地不动，不会干扰并行跑的其它测试。
    #[test]
    fn changing_the_theme_changes_both_cache_keys() {
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let text = "# 关于开展专项检查的通知\n\n各有关单位：";
        let mut highlighter = MarkdownHighlighter::default();
        let layout_once = |highlighter: &mut MarkdownHighlighter| {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                highlighter.layout(
                    ui,
                    text,
                    400.0,
                    14.0,
                    None,
                    &[],
                    &EditorFontScheme::default(),
                );
                highlighter.layout_hybrid(
                    ui,
                    text,
                    600.0,
                    0,
                    None,
                    &[],
                    &crate::models::NumberingConfig::default(),
                );
            });
            highlighter.cache_keys()
        };

        let before = layout_once(&mut highlighter);
        assert_eq!(
            before,
            layout_once(&mut highlighter),
            "配色没变就该命中缓存"
        );

        theme::set_current(crate::models::ThemeName::Claude);
        let after = layout_once(&mut highlighter);
        assert_ne!(before.0, after.0, "切主题后源码模式必须重新上色");
        assert_ne!(before.1, after.1, "切主题后实时排版也必须重新上色");

        theme::set_current_paper(crate::models::PaperMode::Follow);
        let repapered = layout_once(&mut highlighter);
        assert_ne!(after.1, repapered.1, "换纸面明暗后实时排版必须重新上色");
    }
}
