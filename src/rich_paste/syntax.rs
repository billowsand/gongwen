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
//! - 链接、`ins`、`mark` 等行内包裹只留文字；图片丢掉并计数；
//! - 引用块：研究报告保留，其余文种拆成普通段落；
//! - 代码块拆成普通段落；公式：研究报告保留，其余丢；
//! - 只有一格的表格（Word 文本框表格、网页排版表格）拆回段落。

use anydoc::model::{Block, CellSlot, Document, Inline, List, Style, inlines_are_empty};

use super::Stats;

/// 裁剪整篇文档，返回可以直接渲染的块。
pub(crate) fn prune(document: Document, research: bool, stats: &mut Stats) -> Document {
    let Document { blocks, .. } = document;
    let blocks = prune_blocks(blocks, research, stats);
    Document {
        blocks,
        ..Document::default()
    }
}

fn prune_blocks(blocks: Vec<Block>, research: bool, stats: &mut Stats) -> Vec<Block> {
    let mut out = Vec::with_capacity(blocks.len());
    for block in blocks {
        match block {
            Block::Heading {
                level,
                anchor: _,
                content,
            } => {
                let content = prune_inlines(content, research);
                if !inlines_are_empty(&content) {
                    // 标题层级原样保留：段首带编号的由 `structure` 按编号重定层级。
                    out.push(Block::heading(level, content));
                }
            }
            Block::Paragraph(inlines) => {
                let inlines = prune_inlines(inlines, research);
                if !inlines_are_empty(&inlines) {
                    out.push(Block::Paragraph(inlines));
                }
            }
            Block::List(list) => {
                let mut items = Vec::with_capacity(list.items.len());
                for item in list.items {
                    let blocks = prune_blocks(item.blocks, research, stats);
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
                        marker: list.marker,
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
                            stats,
                        ));
                    }
                } else {
                    for row in &mut table.grid {
                        for slot in row {
                            if let CellSlot::Origin(cell) = slot {
                                let blocks = std::mem::take(&mut cell.blocks);
                                cell.blocks = prune_blocks(blocks, research, stats);
                            }
                        }
                    }
                    stats.tables += 1;
                    out.push(Block::Table(table));
                }
            }
            Block::BlockQuote(inner) => {
                let inner = prune_blocks(inner, research, stats);
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

fn prune_inlines(inlines: Vec<Inline>, research: bool) -> Vec<Inline> {
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
            Inline::Link { content, .. } => out.extend(prune_inlines(content, research)),
            // 图片由 `html.rs` 数张数，第一、二期不随粘贴带入。
            Inline::Image { .. } => {}
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
