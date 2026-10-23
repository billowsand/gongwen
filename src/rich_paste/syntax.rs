//! 在 `anydoc` 的文档模型上按文种裁剪语法，再交给它的 Markdown 渲染器。
//!
//! 公文助手的 Markdown 只收「标题 / 段落 / 加粗 / 列表 / 表格 / 附件」这几样（研究报告
//! 多两样：引用块与公式）。从网页复制来的内容会带斜体、删除线、链接、图片、代码块、
//! 引用块、公式、整篇排版用的单格表格，这里在**模型**上一次性裁掉，而不是回头用正则
//! 改 Markdown 文本——渲染器已经处理过转义，模型上改才准。
//!
//! 规则见 `docs/rich-paste-design.md` 第四节：
//!
//! - 斜体去标记留文字；删除线（Word 修订的删除、网页划掉的旧内容）连文字丢；
//! - 链接、`ins`、`mark` 等行内包裹只留文字；图片丢不丢看调用方：块级粘贴保留
//!   （随后入库，见 images.rs），行内粘贴与 AI 侧栏材料入口丢掉；
//! - 引用块：研究报告保留，其余文种拆成普通段落；
//! - 代码块拆成普通段落；公式：研究报告保留，其余丢；
//! - 只有一格的表格（Word 文本框表格、网页排版表格）拆回段落；
//! - 有序列表改成 `- ` 列表：公文助手里两种列表编号效果相同，而 `1. ` 开头的行会被
//!   结构还原当成段首编号重新定级，嵌套的子项就压平了；
//! - 段内换行（`<br>`、Word 软回车）拆成两段：结构还原会把硬换行接回一行，
//!   「联系人：张某<br>联系电话：……」就成了「张某联系电话」。标题里的换行接成一行。

use anydoc::model::{
    Block, CellSlot, Document, Inline, List, MarkerKind, Style, inlines_are_empty,
};

use super::Stats;

/// 裁剪整篇文档，返回可以直接渲染的块。
///
/// `keep_images` 为真时在模型里留下图片（块级粘贴随后让 embedded 入库）；为假时
/// 图片直接丢掉，行内粘贴与 AI 侧栏材料都走这条路。
pub(crate) fn prune(
    document: Document,
    research: bool,
    keep_images: bool,
    stats: &mut Stats,
) -> Document {
    let Document { blocks, assets, .. } = document;
    let blocks = split_line_breaks(prune_blocks(blocks, research, keep_images, stats));
    // 图片字节挂在 Document::assets 上：块级粘贴要在这一步之后由 embedded 按
    // AssetId 取字节入库，必须原样带过去（脚注引用已被裁掉，notes 不必带）。
    Document {
        blocks,
        assets,
        ..Document::default()
    }
}

fn prune_blocks(
    blocks: Vec<Block>,
    research: bool,
    keep_images: bool,
    stats: &mut Stats,
) -> Vec<Block> {
    let mut out = Vec::with_capacity(blocks.len());
    for block in blocks {
        match block {
            Block::Heading {
                level,
                anchor: _,
                content,
            } => {
                // 标题折成两行（「关于做好冬季<br>森林防火工作的通知」）接成一行；
                // 汉字之间的空格由后面的清洗删掉，西文之间留着。
                let content = prune_inlines(content, research, keep_images)
                    .into_iter()
                    .map(|inline| match inline {
                        Inline::LineBreak => Inline::plain(" "),
                        inline => inline,
                    })
                    .collect::<Vec<_>>();
                if !inlines_are_empty(&content) {
                    // 标题层级原样保留：段首带编号的由 `structure` 按编号重定层级。
                    out.push(Block::heading(level, content));
                }
            }
            Block::Paragraph(inlines) => {
                let inlines = prune_inlines(inlines, research, keep_images);
                if !inlines_are_empty(&inlines) {
                    out.push(Block::Paragraph(inlines));
                }
            }
            Block::List(list) => {
                let mut items = Vec::with_capacity(list.items.len());
                for item in list.items {
                    let blocks = prune_blocks(item.blocks, research, keep_images, stats);
                    if blocks.is_empty() {
                        continue;
                    }
                    stats.list_items += 1;
                    items.push(anydoc::model::ListItem {
                        blocks,
                        marker_label: item.marker_label,
                    });
                }
                if !items.is_empty() {
                    out.push(Block::List(List {
                        marker: MarkerKind::Bullet,
                        start: list.start,
                        items,
                    }));
                }
            }
            Block::Table(mut table) => {
                if table.is_single_cell() {
                    // 排版用的单格表格（Word 文本框表格、网页版式表格）还原成正文，
                    // 不能留下一个只有一格的管道表。
                    if let Some(CellSlot::Origin(cell)) =
                        table.grid.first_mut().and_then(|row| row.first_mut())
                    {
                        out.extend(prune_blocks(
                            std::mem::take(&mut cell.blocks),
                            research,
                            keep_images,
                            stats,
                        ));
                    }
                } else {
                    for row in &mut table.grid {
                        for slot in row {
                            if let CellSlot::Origin(cell) = slot {
                                let blocks = std::mem::take(&mut cell.blocks);
                                cell.blocks = prune_blocks(blocks, research, keep_images, stats);
                            }
                        }
                    }
                    stats.tables += 1;
                    out.push(Block::Table(table));
                }
            }
            Block::BlockQuote(inner) => {
                let inner = prune_blocks(inner, research, keep_images, stats);
                if inner.is_empty() {
                    continue;
                }
                if research {
                    out.push(Block::BlockQuote(inner));
                } else {
                    // 公文不收引用块：拆成普通段落。
                    out.extend(inner);
                }
            }
            Block::CodeBlock { text, lang: _ } => {
                for line in text.lines() {
                    let line = line.trim_end();
                    if line.trim().is_empty() {
                        continue;
                    }
                    out.push(Block::Paragraph(vec![Inline::plain(line)]));
                }
            }
            Block::Math(tex) => {
                if research && !tex.trim().is_empty() {
                    out.push(Block::Math(tex));
                }
            }
            Block::Rule => {}
        }
    }
    out
}

/// 段落里的换行拆成几段。表格单元格不拆（管道表一格只能一行，由 `clean` 处理）；
/// 引用块与列表项里的段落一样拆。
fn split_line_breaks(blocks: Vec<Block>) -> Vec<Block> {
    let mut out = Vec::with_capacity(blocks.len());
    for block in blocks {
        match block {
            Block::Paragraph(inlines) => {
                let mut piece = Vec::new();
                for inline in inlines {
                    if matches!(inline, Inline::LineBreak) {
                        push_piece(&mut out, std::mem::take(&mut piece));
                    } else {
                        piece.push(inline);
                    }
                }
                push_piece(&mut out, piece);
            }
            Block::BlockQuote(inner) => out.push(Block::BlockQuote(split_line_breaks(inner))),
            Block::List(mut list) => {
                for item in &mut list.items {
                    item.blocks = split_line_breaks(std::mem::take(&mut item.blocks));
                }
                out.push(Block::List(list));
            }
            block => out.push(block),
        }
    }
    out
}

/// 拆出来的一段：去掉首尾空白（`<br>` 前后常带空格或缩进），空的不要。
fn push_piece(out: &mut Vec<Block>, mut piece: Vec<Inline>) {
    if let Some(Inline::Text { text, .. }) = piece.first_mut() {
        *text = text.trim_start().to_string();
    }
    if let Some(Inline::Text { text, .. }) = piece.last_mut() {
        *text = text.trim_end().to_string();
    }
    if !inlines_are_empty(&piece) {
        out.push(Block::Paragraph(piece));
    }
}

fn prune_inlines(inlines: Vec<Inline>, research: bool, keep_images: bool) -> Vec<Inline> {
    let mut out = Vec::with_capacity(inlines.len());
    for inline in inlines {
        match inline {
            Inline::Text { text, style } => {
                // 删除线是 Word 修订里删掉的内容、网页上划掉的旧内容，连文字一起丢。
                if style.strike {
                    continue;
                }
                // 斜体与等宽标记公文用不上，去掉记号留文字。
                let style = Style {
                    bold: style.bold,
                    ..Style::PLAIN
                };
                if !text.is_empty() {
                    out.push(Inline::Text { text, style });
                }
            }
            // 公文不收链接：留文字，丢目标。
            Inline::Link { content, .. } => {
                out.extend(prune_inlines(content, research, keep_images));
            }
            // 块级粘贴保留图片，由 embedded 入库；行内粘贴与材料入口丢掉。
            Inline::Image { .. } => {
                if keep_images {
                    out.push(inline);
                }
            }
            Inline::Math(tex) => {
                if research {
                    out.push(Inline::Math(tex));
                }
            }
            Inline::LineBreak => out.push(Inline::LineBreak),
            // 文档内部锚点、脚注引用、复选框在粘贴内容里没有意义。
            Inline::Anchor(_) | Inline::NoteRef(_) | Inline::Checkbox(_) => {}
        }
    }
    out
}

/// 文档里还有没有表格（单格排版表格已经拆成段落，不算）。
///
/// 粘在句子中间时用：表格没法塞进一行，有表格就不接管、原样粘贴。
pub(crate) fn has_table(document: &Document) -> bool {
    document.blocks.iter().any(block_has_table)
}

fn block_has_table(block: &Block) -> bool {
    match block {
        Block::Table(_) => true,
        Block::BlockQuote(inner) => inner.iter().any(block_has_table),
        Block::List(list) => list
            .items
            .iter()
            .any(|item| item.blocks.iter().any(block_has_table)),
        _ => false,
    }
}

/// 纯文本行里已经有 Markdown 的样子时不必再转换。
///
/// 判据与设计第六节一致：有 `#` 标题行、`<!-- [` 标记、管道表的分隔行，或者有
/// 代码围栏。用户从别处复制现成的 Markdown 时，原样粘贴才是他要的。
pub(crate) fn looks_like_markdown(text: &str) -> bool {
    for line in text.lines() {
        let trimmed = line.trim_start();
        if heading_line(trimmed) || trimmed.contains("<!-- [") || trimmed.starts_with("```") {
            return true;
        }
        if is_table_separator(trimmed) {
            return true;
        }
    }
    false
}

/// `# ` 到 `###### ` 开头的行。
fn heading_line(line: &str) -> bool {
    let hashes = line.chars().take_while(|ch| *ch == '#').count();
    (1..=6).contains(&hashes) && line[hashes..].starts_with(' ')
}

/// 管道表的分隔行：`| --- | :---: |`。
fn is_table_separator(line: &str) -> bool {
    let line = line.trim();
    if !line.contains('-') || !line.contains('|') {
        return false;
    }
    !line.is_empty()
        && line.chars().all(|ch| matches!(ch, '|' | '-' | ':' | ' '))
        && line.matches('-').count() >= 3
}

/// 数一段 Markdown 里有多少个标题、多少个列表项，用来写状态提示。
pub(crate) fn count_markdown(markdown: &str) -> (usize, usize) {
    let mut headings = 0;
    let mut list_items = 0;
    let mut fenced = false;
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        if heading_line(trimmed) {
            headings += 1;
        } else if trimmed.starts_with("- ") || ordered_item(trimmed) {
            list_items += 1;
        }
    }
    (headings, list_items)
}

/// `1. ` 这样的有序列表项。
fn ordered_item(line: &str) -> bool {
    let digits = line.chars().take_while(|ch| ch.is_ascii_digit()).count();
    digits > 0 && line[digits..].starts_with(". ")
}

/// 两段文字是否只差空白（用于「转换结果和原文没区别就不接管」）。
pub(crate) fn same_ignoring_whitespace(left: &str, right: &str) -> bool {
    squash(left) == squash(right)
}

fn squash(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_whitespace()).collect()
}
