//! 独立列表的缩进子项：并进父项成段内列表，只分两级。

use super::*;

fn item(number: usize, text: &str) -> MarkdownBlock {
    MarkdownBlock::OrderedListItem {
        number,
        text: text.into(),
    }
}

/// 块里生成编号占的字符，逐个取出来，便于直接看。
fn labels(located: &LocatedBlock) -> Vec<String> {
    let text = match &located.block {
        MarkdownBlock::OrderedListItem { text, .. } | MarkdownBlock::Paragraph(text) => text,
        _ => return Vec::new(),
    };
    let chars: Vec<char> = plain_text_of(text).chars().collect();
    located
        .generated_prefixes
        .iter()
        .map(|range| chars[range.clone()].iter().collect())
        .collect()
}

fn plain_text_of(text: &str) -> String {
    crate::export::plain_text(text)
}

#[test]
fn indented_items_under_a_standalone_item_join_it_as_inline_list() {
    let source = "1. 主要任务：\n   1. 完善制度；\n   1. 强化督查，\n2. 保障措施；\n3. 工作要求";
    let located = parse_markdown_located(source);
    let blocks: Vec<_> = located.iter().map(|l| l.block.clone()).collect();
    assert_eq!(
        blocks,
        vec![
            item(1, "主要任务：①完善制度；②强化督查；"),
            item(2, "保障措施；"),
            item(3, "工作要求。"),
        ]
    );
    // 父项的源码范围覆盖到最后一个子项，点击预览回跳整段。
    assert_eq!(
        &source[located[0].range.clone()],
        "1. 主要任务：\n   1. 完善制度；\n   1. 强化督查，"
    );
    assert_eq!(labels(&located[0]), ["①", "②"]);
    assert!(located[1].generated_prefixes.is_empty());
}

#[test]
fn children_restart_per_parent_and_use_the_inline_numbering_style() {
    let numbering = NumberingConfig {
        list1: crate::models::ListNumbering::FullParen,
        ..Default::default()
    };
    let source = "- 甲：\n    - 子一\n    - 子二\n- 乙：\n\t3. 子三\n\t- 子四";
    let blocks = parse_markdown_with_numbering(source, &numbering);
    assert_eq!(
        blocks,
        vec![
            item(1, "甲：（1）子一。（2）子二。"),
            item(2, "乙：（3）子三。（4）子四。"),
        ]
    );
}

#[test]
fn deeper_indentation_stays_in_the_same_child_group() {
    let blocks = parse_markdown("1. 甲：\n  1. 子一，\n      1. 孙一，\n  1. 子二，\n2. 乙。");
    assert_eq!(
        blocks,
        vec![item(1, "甲：①子一。②孙一。③子二。"), item(2, "乙。")]
    );
}

#[test]
fn one_column_of_indentation_is_not_a_child() {
    let blocks = parse_markdown("1. 甲\n 1. 乙");
    assert_eq!(blocks, vec![item(1, "甲。"), item(2, "乙。")]);
}

#[test]
fn indented_group_without_a_parent_stays_standalone() {
    let blocks = parse_markdown("说明：\n\n    1. 甲；\n    1. 乙");
    assert_eq!(
        blocks,
        vec![
            MarkdownBlock::Paragraph("说明：".into()),
            item(1, "甲；"),
            item(2, "乙。"),
        ]
    );
}

#[test]
fn inline_list_after_a_paragraph_flattens_indentation() {
    let blocks = parse_markdown("要求如下：\n1. 甲；\n   1. 子；\n1. 乙");
    assert_eq!(
        blocks,
        vec![MarkdownBlock::Paragraph("要求如下：①甲；②子；③乙。".into())]
    );
}

#[test]
fn parent_without_punctuation_keeps_its_text_and_bold_survives() {
    let source = "1. **主要任务**\n   1. 甲\n   1. **乙**";
    let located = parse_markdown_located(source);
    assert_eq!(located[0].block, item(1, "**主要任务**①甲。②**乙**。"));
    assert_eq!(labels(&located[0]), ["①", "②"]);
}

#[test]
fn nested_punctuation_normalization_keeps_the_colon_and_follows_the_parent_style() {
    let nested = |head: &str, children: &[&str]| NestedItem {
        head: head.to_string(),
        children: children.iter().map(|child| child.to_string()).collect(),
    };
    // 父项一级是分号风格（「保障措施；」）：子项中间也用分号，末子项落父项的分号。
    assert_eq!(
        normalize_nested_item_punctuation(&[
            nested("主要任务：", &["甲，", "乙"]),
            nested("保障措施；", &[]),
            nested("要求", &[]),
        ]),
        vec![
            nested("主要任务：", &["甲；", "乙；"]),
            nested("保障措施；", &[]),
            nested("要求。", &[]),
        ]
    );
    // 句号风格：子项与父项都落句号。
    assert_eq!(
        normalize_nested_item_punctuation(&[
            nested("主要任务：", &["甲，", "乙。"]),
            nested("保障措施；", &[]),
        ]),
        vec![
            nested("主要任务：", &["甲。", "乙。"]),
            nested("保障措施。", &[]),
        ]
    );
}

#[test]
fn source_punctuation_follows_the_nested_rule() {
    let source = "1. 主要任务：\n    1. 甲；\n    1. 乙\n2. 保障措施；\n3. 要求\n\n正文：\n1. 引出：\n   1. 子";
    assert_eq!(
        normalize_ordered_list_punctuation(source),
        "1. 主要任务：\n    1. 甲；\n    1. 乙；\n2. 保障措施；\n3. 要求。\n\n正文：\n1. 引出。\n   1. 子。"
    );
}

#[test]
fn source_renumbering_numbers_children_per_parent() {
    let source = "1. 甲\n    1. 子\n    1. 子\n1. 乙\n    5. 子\n    1. 子\n1. 丙\r\n\n正文：\n1. 段内\n   1. 拉平";
    assert_eq!(
        renumber_ordered_groups(source),
        "1. 甲\n    1. 子\n    2. 子\n2. 乙\n    5. 子\n    6. 子\n3. 丙\r\n\n正文：\n1. 段内\n   2. 拉平"
    );
}

#[test]
fn source_rewrites_agree_with_the_parser() {
    let source = "1. 主要任务：\n    1. 甲，\n    1. 乙，\n1. 保障措施，\n\t1. 丙\n1. 要求";
    let rewritten = renumber_ordered_groups(&normalize_ordered_list_punctuation(source));
    assert_eq!(parse_markdown(source), parse_markdown(&rewritten));
}

#[test]
fn list_item_number_ranges_put_the_own_number_first() {
    assert_eq!(
        list_item_number_ranges(2, &[5..6, 8..9]),
        vec![0..2, 7..8, 10..11]
    );
    assert_eq!(list_item_number_ranges(2, &[]), vec![0..2]);
}
