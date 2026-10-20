//! 起草页导入时把 Word 等文档里内嵌的图片取出来入库。
//!
//! anydoc 解析时把内嵌图片的字节留在 `Document::assets`，但它的 Markdown 渲染只
//! 输出图片的 alt 文本（Word 里通常为空），图片就悄悄没了。这里在渲染前改写文档
//! 模型（渲染函数由 vendor/anydoc 的补丁公开）：
//!
//! - 应用本来就认的位图（PNG、JPEG，以及 GIF、BMP、WebP）原样写进图片资源库，引用
//!   改成相对路径；按字节判断格式，不信部件扩展名。同一份字节只存一次；
//! - EMF / WMF 矢量图转成 PNG 再入库（见 [`crate::metafile`]），同一份字节只转一次；
//! - 解析器只认独占一行的图片：段落里的图把段落拆开，图单独成段；标题、表格、列表、
//!   引用里的图挪到所在块之后；
//! - 收不了的（TIFF、SVG、转换失败的图元文件、外链图片、源文件里缺数据的）在原位留一行
//!   `<!-- [图片未导入：原因] -->`：注释不落到纸上，编辑器里看得到，提示里另有汇总；
//!   写成方括号标记，导入清洗才会像 `<!-- [附件] -->` 一样留着它；
//! - 脚注里的图片丢掉，只计入汇总；
//! - alt 一律留空：研究报告把非空 alt 当图题编号，Word 自动生成的「图片 3」之类替代
//!   文字印出来就是错的图题；原文的题注是图下单独一段，照常保留。

use anydoc::model::{Asset, Block, CellSlot, Document, ImageSource, Inline, inlines_are_empty};
use anyhow::Result;
use std::collections::HashMap;
use std::collections::btree_map::{BTreeMap, Entry};

/// 未入库图片的占位：渲染时先用这个前缀的伪链接占住一行，渲染完再换成注释。
/// 直接放注释文字会被渲染器转义。
const SKIPPED_SENTINEL: &str = "gongwen-skipped-image:";

/// 文件名里取原文件名的前多少个字，免得路径过长。
const STEM_CHARS: usize = 30;

/// 一次导入的图片结果。
#[derive(Debug, Default)]
pub(crate) struct ImageReport {
    /// 入库并在正文里引用的图片处数（同一张图出现两次算两处）。
    pub(crate) imported: usize,
    /// 没能导入的图片，每处一条原因。
    pub(crate) skipped: Vec<String>,
}

impl ImageReport {
    /// 接在导入状态栏后面的一句话；没有图片时为空串。
    pub(crate) fn notice(&self) -> String {
        let mut out = String::new();
        if self.imported > 0 {
            out.push_str(&format!(" 导入图片 {} 张。", self.imported));
        }
        if !self.skipped.is_empty() {
            // 按原因归并计数，保留第一次出现的顺序。
            let mut order: Vec<&str> = Vec::new();
            let mut counts: HashMap<&str, usize> = HashMap::new();
            for reason in &self.skipped {
                let count = counts.entry(reason.as_str()).or_insert(0);
                if *count == 0 {
                    order.push(reason);
                }
                *count += 1;
            }
            let reasons = order
                .iter()
                .map(|reason| format!("{reason} {} 张", counts[reason]))
                .collect::<Vec<_>>()
                .join("、");
            out.push_str(&format!(
                " {} 张图片未导入（{reasons}），正文原位置留有「图片未导入」注释，请手动补图。",
                self.skipped.len()
            ));
        }
        out
    }
}

/// 改写好图片引用的 Markdown 与图片结果。
#[derive(Debug)]
pub(crate) struct Rendered {
    pub(crate) markdown: String,
    pub(crate) images: ImageReport,
}

/// 把解析好的文档渲染成 Markdown，内嵌图片按模块说明入库。
///
/// `stem` 是源文件名（不含扩展名），用来给入库图片起名；`store` 把
/// `(建议文件名, 字节)` 写进资源库并按顺序返回相对路径。写入失败不中断导入：
/// 这些图片按「未导入」处理，正文照常返回。
pub(crate) fn render(
    mut doc: Document,
    stem: &str,
    store: impl FnOnce(&[(String, Vec<u8>)]) -> Result<Vec<String>>,
) -> Rendered {
    let stem: String = stem.chars().take(STEM_CHARS).collect();
    let mut sources = Vec::new();
    collect_assets(&doc.blocks, &mut sources);

    // 按出现顺序给能入库的图片编号；同一份字节（哪怕 Word 里是两个部件）只存一次，
    // 图元文件也只转换一次。
    let mut items: Vec<(String, Vec<u8>)> = Vec::new();
    let mut item_of_asset: BTreeMap<usize, Prepared> = BTreeMap::new();
    let mut item_of_bytes: HashMap<&[u8], Prepared> = HashMap::new();
    for id in sources {
        let Entry::Vacant(slot) = item_of_asset.entry(id) else {
            continue;
        };
        let Some(asset) = doc.assets.get(id) else {
            continue;
        };
        let prepared = item_of_bytes
            .entry(asset.bytes.as_slice())
            .or_insert_with(|| prepare(asset, &stem, &mut items))
            .clone();
        slot.insert(prepared);
    }
    let stored = if items.is_empty() {
        Ok(Vec::new())
    } else {
        store(&items).and_then(|paths| {
            anyhow::ensure!(paths.len() == items.len(), "图片入库数量不符");
            Ok(paths)
        })
    };
    let (paths, store_error) = match stored {
        Ok(paths) => (paths, None),
        Err(error) => (Vec::new(), Some(format!("写入图片目录失败（{error}）"))),
    };

    let blocks = std::mem::take(&mut doc.blocks);
    let mut notes = std::mem::take(&mut doc.notes);
    let mut rewriter = Rewriter {
        item_of_asset: &item_of_asset,
        paths: &paths,
        store_error: store_error.as_deref(),
        report: ImageReport::default(),
        placeholders: Vec::new(),
    };
    let blocks = rewriter.blocks(blocks);
    for note in &mut notes {
        if note.blocks.iter().any(block_has_image) {
            let mut dropped = Vec::new();
            rewriter.hoist(&mut note.blocks, &mut dropped, true);
        }
    }
    let Rewriter {
        report,
        placeholders,
        ..
    } = rewriter;
    doc.blocks = blocks;
    doc.notes = notes;

    let mut markdown = anydoc::document_to_markdown(&doc);
    if !placeholders.is_empty() {
        markdown = markdown
            .split('\n')
            .map(|line| {
                placeholder_index(line)
                    .and_then(|index| placeholders.get(index))
                    .map(|reason| format!("<!-- [图片未导入：{}] -->", marker_text(reason)))
                    .unwrap_or_else(|| line.to_string())
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    Rendered {
        markdown,
        images: report,
    }
}

/// 原因文字（可能带着错误信息）放进单行注释标记前的净化：换行、方括号与 `-->`
/// 都会把标记行截断。
fn marker_text(reason: &str) -> String {
    reason
        .replace("-->", "")
        .chars()
        .map(|ch| match ch {
            '[' | ']' => '_',
            ch if ch.is_control() => ' ',
            ch => ch,
        })
        .collect()
}

/// 一份内嵌图片的去向：入库清单里的序号，或不能入库的原因。
type Prepared = std::result::Result<usize, String>;

/// 决定一份内嵌图片怎么入库：位图原样收，图元文件转成 PNG，其余记下原因。
fn prepare(asset: &Asset, stem: &str, items: &mut Vec<(String, Vec<u8>)>) -> Prepared {
    let (extension, bytes) = if let Some(extension) = bitmap_extension(&asset.bytes) {
        (extension, asset.bytes.clone())
    } else if crate::metafile::is_metafile(&asset.bytes) {
        let kind = if crate::metafile::is_emf(&asset.bytes) {
            "EMF 矢量图"
        } else {
            "WMF 矢量图"
        };
        let png =
            crate::metafile::to_png(&asset.bytes).map_err(|error| format!("{kind}，{error}"))?;
        ("png", png)
    } else {
        return Err(unsupported_reason(asset));
    };
    items.push((format!("{stem}_图{}.{extension}", items.len() + 1), bytes));
    Ok(items.len() - 1)
}

/// 一行是不是占位伪链接，是则返回占位序号。
fn placeholder_index(line: &str) -> Option<usize> {
    line.trim()
        .strip_prefix("![](")?
        .strip_suffix(')')?
        .strip_prefix(SKIPPED_SENTINEL)?
        .parse()
        .ok()
}

/// 正文（不含脚注）里引用的内嵌图片，按出现顺序。
fn collect_assets(blocks: &[Block], out: &mut Vec<usize>) {
    for block in blocks {
        match block {
            Block::Paragraph(inlines)
            | Block::Heading {
                content: inlines, ..
            } => collect_inline_assets(inlines, out),
            Block::List(list) => {
                for item in &list.items {
                    collect_assets(&item.blocks, out);
                }
            }
            Block::Table(table) => {
                for slot in table.grid.iter().flatten() {
                    if let CellSlot::Origin(cell) = slot {
                        collect_assets(&cell.blocks, out);
                    }
                }
            }
            Block::BlockQuote(inner) => collect_assets(inner, out),
            Block::CodeBlock { .. } | Block::Rule | Block::Math(_) => {}
        }
    }
}

fn collect_inline_assets(inlines: &[Inline], out: &mut Vec<usize>) {
    for inline in inlines {
        match inline {
            Inline::Image {
                source: ImageSource::Asset(id),
                ..
            } => out.push(id.0),
            Inline::Link { content, .. } => collect_inline_assets(content, out),
            _ => {}
        }
    }
}

fn block_has_image(block: &Block) -> bool {
    match block {
        Block::Paragraph(inlines)
        | Block::Heading {
            content: inlines, ..
        } => inlines_have_image(inlines),
        Block::List(list) => list
            .items
            .iter()
            .any(|item| item.blocks.iter().any(block_has_image)),
        Block::Table(table) => table.grid.iter().flatten().any(|slot| match slot {
            CellSlot::Origin(cell) => cell.blocks.iter().any(block_has_image),
            CellSlot::Covered { .. } => false,
        }),
        Block::BlockQuote(inner) => inner.iter().any(block_has_image),
        Block::CodeBlock { .. } | Block::Rule | Block::Math(_) => false,
    }
}

fn inlines_have_image(inlines: &[Inline]) -> bool {
    inlines.iter().any(|inline| match inline {
        Inline::Image { .. } => true,
        Inline::Link { content, .. } => inlines_have_image(content),
        _ => false,
    })
}

/// 行内内容拆开后的一段：文字，或一张已换成独立段落的图。
enum Piece {
    Text(Vec<Inline>),
    Image(Block),
}

struct Rewriter<'a> {
    item_of_asset: &'a BTreeMap<usize, Prepared>,
    paths: &'a [String],
    store_error: Option<&'a str>,
    report: ImageReport,
    /// 占位序号 → 未导入原因。
    placeholders: Vec<String>,
}

impl Rewriter<'_> {
    /// 顶层块：段落里的图拆成独立段，其余块里的图挪到块后。不含图片的块原样保留，
    /// 保证没有图片的文档与 `anydoc::to_markdown` 输出逐字一致。
    fn blocks(&mut self, blocks: Vec<Block>) -> Vec<Block> {
        let mut out = Vec::with_capacity(blocks.len());
        for block in blocks {
            if !block_has_image(&block) {
                out.push(block);
                continue;
            }
            match block {
                Block::Paragraph(inlines) => {
                    let mut pieces = Vec::new();
                    self.pieces(inlines, &mut pieces, false);
                    for piece in pieces {
                        match piece {
                            Piece::Text(text) => {
                                let text = trim_breaks(text);
                                if !inlines_are_empty(&text) {
                                    out.push(Block::Paragraph(text));
                                }
                            }
                            Piece::Image(image) => out.push(image),
                        }
                    }
                }
                Block::Heading {
                    level,
                    anchor,
                    content,
                } => {
                    let (content, images) = self.split_out(content, false);
                    if !inlines_are_empty(&content) {
                        out.push(Block::Heading {
                            level,
                            anchor,
                            content,
                        });
                    }
                    out.extend(images);
                }
                mut container => {
                    let mut images = Vec::new();
                    self.hoist_block(&mut container, &mut images, false);
                    out.push(container);
                    out.extend(images);
                }
            }
        }
        out
    }

    /// 把一串块里（任意深度）的图片取出来放进 `images`，只剩图片的段落随之删掉。
    /// `in_note` 为真时图片只计入未导入，不生成占位。
    fn hoist(&mut self, blocks: &mut Vec<Block>, images: &mut Vec<Block>, in_note: bool) {
        blocks.retain_mut(|block| {
            if !block_has_image(block) {
                return true;
            }
            match block {
                Block::Paragraph(inlines)
                | Block::Heading {
                    content: inlines, ..
                } => {
                    let (text, found) = self.split_out(std::mem::take(inlines), in_note);
                    images.extend(found);
                    let text = trim_breaks(text);
                    let keep = !inlines_are_empty(&text);
                    *inlines = text;
                    keep
                }
                other => {
                    self.hoist_block(other, images, in_note);
                    true
                }
            }
        });
    }

    fn hoist_block(&mut self, block: &mut Block, images: &mut Vec<Block>, in_note: bool) {
        match block {
            Block::List(list) => {
                for item in &mut list.items {
                    self.hoist(&mut item.blocks, images, in_note);
                }
            }
            Block::Table(table) => {
                for slot in table.grid.iter_mut().flatten() {
                    if let CellSlot::Origin(cell) = slot {
                        self.hoist(&mut cell.blocks, images, in_note);
                    }
                }
            }
            Block::BlockQuote(inner) => self.hoist(inner, images, in_note),
            // 段落与标题要能整块删掉，由 `hoist` 直接处理，不会走到这里。
            Block::Paragraph(_)
            | Block::Heading { .. }
            | Block::CodeBlock { .. }
            | Block::Rule
            | Block::Math(_) => {}
        }
    }

    /// 把一串行内内容拆成去掉图片后的文字与取出来的图片块。
    fn split_out(&mut self, inlines: Vec<Inline>, in_note: bool) -> (Vec<Inline>, Vec<Block>) {
        let mut pieces = Vec::new();
        self.pieces(inlines, &mut pieces, in_note);
        let mut text = Vec::new();
        let mut images = Vec::new();
        for piece in pieces {
            match piece {
                Piece::Text(part) => text.extend(part),
                Piece::Image(image) => images.push(image),
            }
        }
        (text, images)
    }

    /// 按顺序把行内内容切成文字段与图片。链接里的图挪到链接之后，链接文字保留。
    fn pieces(&mut self, inlines: Vec<Inline>, out: &mut Vec<Piece>, in_note: bool) {
        for inline in inlines {
            match inline {
                Inline::Image { source, .. } => {
                    if let Some(image) = self.image_block(&source, in_note) {
                        out.push(Piece::Image(image));
                    }
                }
                Inline::Link { content, target } if inlines_have_image(&content) => {
                    let (content, images) = self.split_out(content, in_note);
                    if !inlines_are_empty(&content) {
                        push_text(out, Inline::Link { content, target });
                    }
                    out.extend(images.into_iter().map(Piece::Image));
                }
                other => push_text(out, other),
            }
        }
    }

    /// 一处图片换成的独立段落：入库的是图片引用，否则是占位（脚注里的不出块）。
    fn image_block(&mut self, source: &ImageSource, in_note: bool) -> Option<Block> {
        if in_note {
            self.report.skipped.push("脚注里的图片".to_string());
            return None;
        }
        let reason = match source {
            ImageSource::Asset(id) => match self.item_of_asset.get(&id.0) {
                Some(Ok(item)) => match self.paths.get(*item) {
                    Some(path) => {
                        self.report.imported += 1;
                        return Some(image_paragraph(path.clone()));
                    }
                    None => self.store_error.unwrap_or("写入图片目录失败").to_string(),
                },
                Some(Err(reason)) => reason.clone(),
                None => "源文件里缺少图片数据".to_string(),
            },
            ImageSource::External(_) => "链接到外部的图片".to_string(),
            ImageSource::Unavailable => "源文件里缺少图片数据".to_string(),
        };
        self.report.skipped.push(reason.clone());
        self.placeholders.push(reason);
        Some(image_paragraph(format!(
            "{SKIPPED_SENTINEL}{}",
            self.placeholders.len() - 1
        )))
    }
}

fn image_paragraph(target: String) -> Block {
    Block::Paragraph(vec![Inline::Image {
        alt: String::new(),
        source: ImageSource::External(target),
    }])
}

fn push_text(out: &mut Vec<Piece>, inline: Inline) {
    match out.last_mut() {
        Some(Piece::Text(text)) => text.push(inline),
        _ => out.push(Piece::Text(vec![inline])),
    }
}

/// 去掉段首段尾的换行：图片拆出去后，原来贴着图片的软回车会变成多余的行尾反斜杠。
fn trim_breaks(mut inlines: Vec<Inline>) -> Vec<Inline> {
    while matches!(inlines.last(), Some(Inline::LineBreak)) {
        inlines.pop();
    }
    let leading = inlines
        .iter()
        .take_while(|inline| matches!(inline, Inline::LineBreak))
        .count();
    inlines.drain(..leading);
    inlines
}

/// 应用能直接收的位图，按文件头判断，返回入库用的扩展名。
fn bitmap_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.len() > 14 && bytes.starts_with(b"BM") {
        Some("bmp")
    } else if bytes.len() > 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

/// 收不了的内嵌图片的原因，写进占位注释与提示。
/// EMF / WMF 不走这里，由 [`prepare`] 转成 PNG。
fn unsupported_reason(asset: &Asset) -> String {
    let bytes = asset.bytes.as_slice();
    let kind = if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        "TIFF 图片".to_string()
    } else if asset.media_type.contains("svg") {
        "SVG 矢量图".to_string()
    } else {
        match asset.media_type.rsplit('/').next() {
            Some(sub) if !sub.is_empty() => format!("{} 格式图片", sub.to_ascii_uppercase()),
            _ => "未知格式图片".to_string(),
        }
    };
    format!("{kind}，暂不支持")
}

#[cfg(test)]
mod tests {
    use super::*;
    use anydoc::model::{AssetId, Cell, Table, TableKind};

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake-png";
    const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3];

    fn emf() -> Vec<u8> {
        let mut bytes = vec![1, 0, 0, 0];
        bytes.resize(40, 0);
        bytes.extend_from_slice(b" EMF");
        bytes.extend_from_slice(&[0; 8]);
        bytes
    }

    fn asset(id: usize, media_type: &str, bytes: &[u8]) -> Asset {
        Asset {
            id: AssetId(id),
            media_type: media_type.to_string(),
            origin_part: format!("word/media/image{id}"),
            bytes: bytes.to_vec(),
        }
    }

    fn image(id: usize) -> Inline {
        Inline::Image {
            alt: "图片 3".to_string(),
            source: ImageSource::Asset(AssetId(id)),
        }
    }

    /// 要入库的一张图：建议文件名与字节。
    type Item = (String, Vec<u8>);

    /// 记下要入库的文件、按名字发路径，不碰磁盘。
    fn fake_store(seen: &mut Vec<Item>) -> impl FnOnce(&[Item]) -> Result<Vec<String>> + '_ {
        |items| {
            seen.extend_from_slice(items);
            Ok(items
                .iter()
                .map(|(name, _)| format!("images/{name}"))
                .collect())
        }
    }

    #[test]
    fn paragraph_images_split_onto_their_own_lines() {
        let doc = Document {
            blocks: vec![Block::Paragraph(vec![
                Inline::plain("如下图所示："),
                Inline::LineBreak,
                image(0),
                Inline::LineBreak,
                Inline::plain("图后文字。"),
            ])],
            notes: Vec::new(),
            assets: vec![asset(0, "image/png", PNG)],
        };
        let mut seen = Vec::new();
        let rendered = render(doc, "通知", fake_store(&mut seen));
        assert_eq!(
            rendered.markdown.trim_end(),
            "如下图所示：\n\n![](images/通知_图1.png)\n\n图后文字。"
        );
        assert_eq!(seen, vec![("通知_图1.png".to_string(), PNG.to_vec())]);
        assert_eq!(rendered.images.imported, 1);
        assert!(rendered.images.skipped.is_empty());
    }

    #[test]
    fn identical_bytes_are_stored_once_and_reused() {
        let doc = Document {
            blocks: vec![
                Block::Paragraph(vec![image(0)]),
                Block::Paragraph(vec![image(1)]),
                Block::Paragraph(vec![image(0)]),
                Block::Paragraph(vec![image(2)]),
            ],
            notes: Vec::new(),
            assets: vec![
                asset(0, "image/png", PNG),
                asset(1, "image/png", PNG),
                asset(2, "image/jpeg", JPEG),
            ],
        };
        let mut seen = Vec::new();
        let rendered = render(doc, "稿", fake_store(&mut seen));
        let names: Vec<_> = seen.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["稿_图1.png", "稿_图2.jpg"]);
        assert_eq!(
            rendered.markdown.trim_end(),
            "![](images/稿_图1.png)\n\n![](images/稿_图1.png)\n\n\
             ![](images/稿_图1.png)\n\n![](images/稿_图2.jpg)"
        );
        assert_eq!(rendered.images.imported, 4);
    }

    #[test]
    fn table_and_heading_images_move_after_the_block() {
        let table = Table::from_rows(
            vec![
                vec![
                    Cell::from_inlines(vec![Inline::plain("名称")]),
                    Cell::from_inlines(vec![Inline::plain("照片")]),
                ],
                vec![
                    Cell::from_inlines(vec![Inline::plain("闸门")]),
                    Cell::from_inlines(vec![image(0)]),
                ],
            ],
            1,
            TableKind::Data,
        );
        let doc = Document {
            blocks: vec![
                Block::heading(2, vec![Inline::plain("现场情况"), image(1)]),
                Block::Table(table),
            ],
            notes: Vec::new(),
            assets: vec![asset(0, "image/png", PNG), asset(1, "image/jpeg", JPEG)],
        };
        let mut seen = Vec::new();
        let markdown = render(doc, "报告", fake_store(&mut seen)).markdown;
        let markdown = markdown.trim_end();
        // 标题在前、图在后；表格里的图挪到表格之后，单元格留空。
        assert_eq!(
            markdown,
            "## 现场情况\n\n![](images/报告_图1.jpg)\n\n\
             | 名称 | 照片 |\n| --- | --- |\n| 闸门 |  |\n\n![](images/报告_图2.png)",
        );
    }

    #[test]
    fn unsupported_images_leave_a_comment_in_place() {
        let doc = Document {
            blocks: vec![
                Block::Paragraph(vec![Inline::plain("流程如下：")]),
                Block::Paragraph(vec![image(0)]),
                Block::Paragraph(vec![Inline::Image {
                    alt: String::new(),
                    source: ImageSource::External("https://e.com/a.png".into()),
                }]),
                Block::Paragraph(vec![Inline::plain("续。")]),
            ],
            notes: Vec::new(),
            assets: vec![asset(0, "image/tiff", b"II*\0rest")],
        };
        let mut seen = Vec::new();
        let rendered = render(doc, "稿", fake_store(&mut seen));
        assert!(seen.is_empty(), "TIFF 不入库");
        assert_eq!(
            rendered.markdown.trim_end(),
            "流程如下：\n\n<!-- [图片未导入：TIFF 图片，暂不支持] -->\n\n\
             <!-- [图片未导入：链接到外部的图片] -->\n\n续。"
        );
        assert_eq!(rendered.images.imported, 0);
        assert!(rendered.images.notice().contains("2 张图片未导入"));
        assert!(
            rendered
                .images
                .notice()
                .contains("TIFF 图片，暂不支持 1 张")
        );
    }

    /// EMF 转成 PNG 入库；同一份字节只转一次、存一份；解析不了的 EMF 原位留注释。
    #[test]
    fn metafiles_are_converted_to_png() {
        let flow = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/metafile/flow-dual.emf"),
        )
        .expect("读样本");
        let doc = Document {
            blocks: vec![
                Block::Paragraph(vec![image(0)]),
                Block::Paragraph(vec![image(1)]),
                Block::Paragraph(vec![image(2)]),
            ],
            notes: Vec::new(),
            assets: vec![
                asset(0, "image/x-emf", &flow),
                asset(1, "image/x-emf", &flow),
                asset(2, "image/x-emf", &emf()),
            ],
        };
        let mut seen = Vec::new();
        let rendered = render(doc, "稿", fake_store(&mut seen));
        assert_eq!(seen.len(), 1, "同一份 EMF 只存一份");
        assert_eq!(seen[0].0, "稿_图1.png");
        assert!(seen[0].1.starts_with(b"\x89PNG"), "存的是 PNG");
        assert_eq!(
            rendered.markdown.trim_end(),
            "![](images/稿_图1.png)\n\n![](images/稿_图1.png)\n\n\
             <!-- [图片未导入：EMF 矢量图，无法解析] -->"
        );
        assert_eq!(rendered.images.imported, 2);
        assert_eq!(rendered.images.skipped, ["EMF 矢量图，无法解析"]);
    }

    #[test]
    fn store_failure_keeps_the_text() {
        let doc = Document {
            blocks: vec![Block::Paragraph(vec![Inline::plain("正文"), image(0)])],
            notes: Vec::new(),
            assets: vec![asset(0, "image/png", PNG)],
        };
        let rendered = render(doc, "稿", |_| {
            anyhow::bail!("磁盘已满\n[os error 112] -->")
        });
        assert_eq!(
            rendered.markdown.trim_end(),
            "正文\n\n<!-- [图片未导入：写入图片目录失败（磁盘已满 _os error 112_ ）] -->"
        );
        assert_eq!(rendered.images.skipped.len(), 1);
    }

    #[test]
    fn documents_without_images_render_exactly_like_anydoc() {
        let doc = Document {
            blocks: vec![
                Block::heading(1, vec![Inline::plain("标题")]),
                Block::Paragraph(vec![Inline::plain("甲"), Inline::LineBreak]),
            ],
            notes: Vec::new(),
            assets: Vec::new(),
        };
        let expected = anydoc::document_to_markdown(&doc);
        let rendered = render(doc, "稿", |_| panic!("没有图片不该入库"));
        assert_eq!(rendered.markdown, expected);
        assert!(rendered.images.notice().is_empty());
    }

    #[test]
    fn bitmap_sniffing_ignores_declared_media_type() {
        assert_eq!(bitmap_extension(PNG), Some("png"));
        assert_eq!(bitmap_extension(JPEG), Some("jpg"));
        assert_eq!(bitmap_extension(b"GIF89a...."), Some("gif"));
        assert_eq!(bitmap_extension(&emf()), None);
        assert_eq!(
            unsupported_reason(&asset(0, "image/tiff", b"II*\0rest")),
            "TIFF 图片，暂不支持"
        );
    }
}
