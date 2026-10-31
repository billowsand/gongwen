//! 图表题注：表题配对（公文与研究报告预览共用）与公文的全篇流水编号。
//!
//! 表题识别规则与 mdx 的 `parser::parse_table_caption_marker` 对齐：表前优先、
//! 表后兜底，允许表题与表格之间夹序号表标记行，手写旧编号一律剥掉。
//! 公文与研究报告的图、表都各自全篇连续编号（表 1、表 2…图 1、图 2…），
//! 不按章清零，也不拼章号 / 附录字母；没有题注的表 / 图不编号、不占号。

use crate::export;
use crate::export::crossref;
use crate::export::{LocatedBlock, MarkdownBlock};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// 块序列的元素：`MarkdownBlock` 本身或带源码位置的 `LocatedBlock`，
/// 让配对与编号逻辑对两种切片都能直接用，不必为调用方再包一层。
pub(crate) trait BlockRef {
    fn block(&self) -> &MarkdownBlock;
}

impl BlockRef for MarkdownBlock {
    fn block(&self) -> &MarkdownBlock {
        self
    }
}

impl BlockRef for LocatedBlock {
    fn block(&self) -> &MarkdownBlock {
        &self.block
    }
}

/// 「表格块下标 → 表题块下标」。表题写在表前或表后都认，表前优先——与 mdx 的
/// `take_leading_table_caption().or(parse_trailing_table_caption())` 一致。
pub(crate) fn table_captions<B: BlockRef>(blocks: &[B]) -> HashMap<usize, usize> {
    let mut pairs = HashMap::new();
    let mut used = HashSet::new();
    for (index, located) in blocks.iter().enumerate() {
        if !matches!(located.block(), MarkdownBlock::Table { .. }) {
            continue;
        }
        // 表题与表格之间可以夹一行序号表标记：它不落版面，mdx 也不把它当块。
        let mut before = index.checked_sub(1);
        while let Some(marker) = before.filter(|marker| {
            matches!(blocks[*marker].block(), MarkdownBlock::Html(line)
                if export::parse_numbered_table_marker(line))
        }) {
            before = marker.checked_sub(1);
        }
        let leading = before
            .filter(|before| !used.contains(before))
            .filter(|before| caption_text(&blocks[*before]).is_some());
        let trailing = (index + 1 < blocks.len())
            .then_some(index + 1)
            .filter(|after| !used.contains(after))
            .filter(|after| caption_text(&blocks[*after]).is_some());
        if let Some(caption) = leading.or(trailing) {
            used.insert(caption);
            pairs.insert(index, caption);
        }
    }
    pairs
}

/// 这一块是不是一条表题；是就给出题名。
pub(crate) fn caption_text<B: BlockRef>(located: &B) -> Option<String> {
    let MarkdownBlock::Paragraph(text) = located.block() else {
        return None;
    };
    // 第一遍解析拿到的文字还带着行尾锚点，剥掉再认。
    parse_table_caption(crossref::split_label(text).0)
}

/// 表题行：`表：题名`、`表 1.2 题名`、`Table: 题名`、`：题名`。
/// 编号由程序生成，源码里写的旧编号一律剥掉，免得印两遍。
/// 写法与正则都照 mdx 的 `parser::parse_table_caption_marker`。
pub(crate) fn parse_table_caption(line: &str) -> Option<String> {
    static NUMBERED: OnceLock<Regex> = OnceLock::new();
    let numbered = NUMBERED.get_or_init(|| {
        Regex::new(
            r"^(?i:表|table)\s*(?:[A-Za-z]?\d+|[A-Za-z][.\-]\d+)(?:[.\-][A-Za-z0-9]+)*\s*(?:[:：]\s*|\s+)(.+)$",
        )
        .expect("带编号表题正则")
    });
    let trimmed = line.trim();
    if let Some(caps) = numbered.captures(trimmed) {
        let caption = strip_caption_number(caps[1].trim());
        if !caption.is_empty() {
            return Some(caption);
        }
    }
    let caption = trimmed
        .strip_prefix("Table:")
        .or_else(|| trimmed.strip_prefix("table:"))
        .or_else(|| trimmed.strip_prefix("TABLE:"))
        .or_else(|| trimmed.strip_prefix("表:"))
        .or_else(|| trimmed.strip_prefix("表："))
        .or_else(|| trimmed.strip_prefix(':'))?;
    let caption = strip_caption_number(caption.trim());
    (!caption.is_empty()).then_some(caption)
}

/// 剥掉表题开头残留的旧编号（`4.6 题名`、`E.1 题名`、`附录E 题名`）。
pub(crate) fn strip_caption_number(caption: &str) -> String {
    static APPENDIX: OnceLock<Regex> = OnceLock::new();
    let appendix = APPENDIX.get_or_init(|| {
        Regex::new(r"^附录\s*[A-Za-z0-9]+(?:[.\-][A-Za-z0-9]+)*\s*[、.．:：]?\s*")
            .expect("附录表题正则")
    });
    static LEADING_NUM: OnceLock<Regex> = OnceLock::new();
    let leading = LEADING_NUM.get_or_init(|| {
        Regex::new(r"^(?:\d+(?:\.\d+)*|[A-Za-z][.\-]\d+(?:[.\-]\d+)*)[.．、]?\s+([^\d\s])")
            .expect("表题编号正则")
    });
    let caption = appendix.replace(caption, "");
    leading.replace(&caption, "$1").trim().to_string()
}

/// 公文（非研究报告）全文的图表题注与编号，预览、PDF、Word 三端共用这一份，
/// 保证同一张表、同一幅图在三端印的号一致。
#[derive(Default, Clone)]
pub(crate) struct OfficialCaptions {
    /// 表格块下标 →（表号，题名）。表题排在表格上方。
    pub(crate) tables: HashMap<usize, (usize, String)>,
    /// `Image` / `Diagram` 块下标 →（图号，题名）。图题排在图下方，
    /// 普通插图与 Mermaid 图共享图号。
    pub(crate) figures: HashMap<usize, (usize, String)>,
    /// 被吸收为表题的段落下标：三端渲染时跳过，不再按普通段落排。
    pub(crate) caption_paragraphs: HashSet<usize>,
}

/// 走一遍公文块序列，配对表题、分配全文流水号。公文不做正文交叉引用，
/// 题注行尾的 `{#tbl:…}` / `{#fig:…}` 锚点只剥掉、不登记。
pub(crate) fn official_captions<B: BlockRef>(blocks: &[B]) -> OfficialCaptions {
    let pairs = table_captions(blocks);
    let mut captions = OfficialCaptions::default();
    let mut figure = 0usize;
    for (index, located) in blocks.iter().enumerate() {
        match located.block() {
            MarkdownBlock::Table { .. } => {
                if let Some(&caption_index) = pairs.get(&index)
                    && let Some(text) = caption_text(&blocks[caption_index])
                {
                    let number = captions.tables.len() + 1;
                    captions.tables.insert(index, (number, text));
                    captions.caption_paragraphs.insert(caption_index);
                }
            }
            MarkdownBlock::Image { alt, src } => {
                let alt = alt.trim();
                // 用户的 PDF 扫描件在 Word 里只能写成附件说明，三端一致不编号、
                // 不占号；Mermaid 物化图（mermaid-cache/）导出 PDF 时是矢量 .pdf，
                // 仍是正文插图，正常编号（与预览的 Diagram、Word 的 PNG 一致）。
                let mermaid = src.replace('\\', "/").contains("mermaid-cache/");
                if !alt.is_empty() && (!is_pdf_source(src) || mermaid) {
                    figure += 1;
                    captions.figures.insert(index, (figure, alt.to_string()));
                }
            }
            MarkdownBlock::Diagram { caption, .. } => {
                let caption = caption.trim();
                if !caption.is_empty() {
                    figure += 1;
                    captions
                        .figures
                        .insert(index, (figure, caption.to_string()));
                }
            }
            _ => {}
        }
    }
    captions
}

/// 插图路径是不是 PDF（可带 `#page=…` 之类的页码锚）。
pub(crate) fn is_pdf_source(src: &str) -> bool {
    let name = src.rsplit('/').next().unwrap_or(src);
    let name = name.split('#').next().unwrap_or(name);
    name.to_ascii_lowercase().ends_with(".pdf")
}

impl OfficialCaptions {
    /// 预览用：键从块下标换成各块的源码起点（`LocatedBlock::range.start`）。
    /// 预览按附件 / 正文拆成 `&LocatedBlock` 序列后全局下标就丢了，而源码起点
    /// 在全文唯一，各层拿着 located 就能查到同一份题注。
    pub(crate) fn rekey_by_source(self, located: &[LocatedBlock]) -> Self {
        let mut keyed = Self::default();
        for (index, block) in located.iter().enumerate() {
            let key = block.range.start;
            if let Some(entry) = self.tables.get(&index) {
                keyed.tables.insert(key, entry.clone());
            }
            if let Some(entry) = self.figures.get(&index) {
                keyed.figures.insert(key, entry.clone());
            }
            if self.caption_paragraphs.contains(&index) {
                keyed.caption_paragraphs.insert(key);
            }
        }
        keyed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::parse_markdown_located_with_numbering;
    use crate::models::NumberingConfig;

    fn captions_of(markdown: &str) -> OfficialCaptions {
        let blocks = parse_markdown_located_with_numbering(markdown, &NumberingConfig::default());
        official_captions(&blocks)
    }

    #[test]
    fn leading_and_trailing_table_captions_are_paired_and_numbered() {
        let captions = captions_of(concat!(
            "表：甲表\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\n",
            "正文。\n\n| c | d |\n| --- | --- |\n| 3 | 4 |\n\n表：乙表\n",
        ));
        let mut tables: Vec<_> = captions.tables.values().collect();
        tables.sort();
        assert_eq!(
            tables,
            vec![&(1, "甲表".to_string()), &(2, "乙表".to_string())]
        );
        assert_eq!(captions.caption_paragraphs.len(), 2);
    }

    #[test]
    fn numbered_table_marker_between_caption_and_table_still_pairs() {
        let captions = captions_of(
            "表：甲表\n\n<!-- [序号表] -->\n\n| 事项 | 说明 |\n| --- | --- |\n| 一 | 甲 |\n",
        );
        assert_eq!(captions.tables.len(), 1);
        assert_eq!(captions.caption_paragraphs.len(), 1);
    }

    #[test]
    fn hand_written_numbers_are_stripped_and_not_counted_twice() {
        let captions = captions_of("表 1.2 甲表\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n");
        assert_eq!(
            captions.tables.values().collect::<Vec<_>>(),
            vec![&(1, "甲表".to_string())]
        );
    }

    #[test]
    fn figures_share_one_running_number_and_empty_alt_does_not_count() {
        let captions = captions_of(concat!(
            "![甲图](images/a.png)\n\n",
            "![](images/b.png)\n\n",
            "```mermaid\nflowchart LR\n A[收] --> B[办]\n```\n图：丙图\n\n",
            "![丁图](images/d.pdf)\n\n",
            "![戊图](images/e.png)\n",
        ));
        let mut figures: Vec<_> = captions.figures.values().collect();
        figures.sort();
        assert_eq!(
            figures,
            vec![
                &(1, "甲图".to_string()),
                &(2, "丙图".to_string()),
                &(3, "戊图".to_string())
            ]
        );
    }

    #[test]
    fn caption_not_adjacent_to_a_table_stays_a_paragraph() {
        let captions =
            captions_of("表：独立段落\n\n正文。\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n");
        assert!(captions.tables.is_empty());
        assert!(captions.caption_paragraphs.is_empty());
    }

    #[test]
    fn table_without_caption_does_not_count() {
        let captions = captions_of(concat!(
            "| a | b |\n| --- | --- |\n| 1 | 2 |\n\n",
            "表：乙表\n\n| c | d |\n| --- | --- |\n| 3 | 4 |\n",
        ));
        assert_eq!(captions.tables.len(), 1);
        assert_eq!(
            captions.tables.values().collect::<Vec<_>>(),
            vec![&(1, "乙表".to_string())]
        );
    }
}
