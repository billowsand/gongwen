//! `structure` 的单元测试：编号定级、结构还原、版头版记与段内枚举。
//!
//! 从 `mod.rs` 拆出来（只移动，不改行为），免得一个文件又长回两千行。

use super::*;

fn clean(markdown: &str) -> String {
    restructure(markdown, &Options::import(true, false))
}

#[test]
fn chinese_numbered_paragraphs_become_headings_with_run_in_body() {
    let raw = "一、总体要求\n\n坚持稳中求进。\n\n\
               （一）加强组织领导。各单位要高度重视，明确责任分工。\n\n\
               （二）工作目标：到2027年基本建成。\n\n\
               **二、主要任务**";
    assert_eq!(
        clean(raw),
        "## 总体要求\n\n坚持稳中求进。\n\n\
         ### 加强组织领导\n各单位要高度重视，明确责任分工。\n\n\
         ### 工作目标\n到2027年基本建成。\n\n\
         ## 主要任务"
    );
}

#[test]
fn long_first_sentence_stays_a_paragraph() {
    let raw = "一、根据市委市政府关于进一步加强和改进新形势下安全生产工作的部署要求和全年工作安排，现将有关事项通知如下。";
    assert_eq!(clean(raw), raw);
    // 冒号前带逗号的是正文引语，不是标题。
    let raw = "一、根据工作安排，现将有关事项通知如下：";
    assert_eq!(clean(raw), raw);
}

#[test]
fn short_items_become_a_list() {
    let raw = "请报送以下材料：\n\n1.工作总结；\n\n2.统计报表；\n\n3.典型案例。\n\n以上材料请于5月10日前报送。";
    assert_eq!(
        clean(raw),
        "请报送以下材料：\n\n1. 工作总结；\n2. 统计报表；\n3. 典型案例。\n\n以上材料请于5月10日前报送。"
    );
}

#[test]
fn items_with_paragraphs_under_them_become_level_three_headings() {
    let raw = "（一）重点任务\n\n1.排查隐患\n\n各单位要全面排查。\n\n\
               2.整改落实\n\n对排查出的问题限期整改。";
    assert_eq!(
        clean(raw),
        "### 重点任务\n\n#### 排查隐患\n\n各单位要全面排查。\n\n\
         #### 整改落实\n\n对排查出的问题限期整改。"
    );
}

#[test]
fn run_in_items_with_long_bodies_become_headings() {
    let raw = "1.加强组织领导。各单位要高度重视，成立工作专班，明确责任分工，确保各项任务落到实处。\n\n\
               2.强化经费保障。财政部门要统筹安排专项资金，加强资金监管，提高资金使用效益和管理水平。";
    assert_eq!(
        clean(raw),
        "#### 加强组织领导\n各单位要高度重视，成立工作专班，明确责任分工，确保各项任务落到实处。\n\n\
         #### 强化经费保障\n财政部门要统筹安排专项资金，加强资金监管，提高资金使用效益和管理水平。"
    );
}

#[test]
fn colon_items_with_short_bodies_stay_a_list() {
    let raw = "1.会议时间：5月10日上午9时。\n\n2.会议地点：市政府3楼会议室。";
    assert_eq!(
        clean(raw),
        "1. 会议时间：5月10日上午9时。\n2. 会议地点：市政府3楼会议室。"
    );
}

#[test]
fn sub_items_make_their_parent_a_heading() {
    let raw = "1.检查内容\n\n（1）制度建设情况；\n\n（2）台账管理情况。\n\n2.检查方式\n\n采取实地查看方式。";
    assert_eq!(
        clean(raw),
        "#### 检查内容\n\n1. 制度建设情况；\n2. 台账管理情况。\n\n#### 检查方式\n\n采取实地查看方式。"
    );
}

#[test]
fn word_auto_numbering_labels_map_to_levels() {
    let raw = "- 1、 总体要求\n\n正文。\n\n- （1） 指导思想\n\n正文。\n\n\
               1\\. 排查隐患\n\n- （1） 制度建设\n- （2） 台账管理";
    assert_eq!(
        clean(raw),
        "## 总体要求\n\n正文。\n\n### 指导思想\n\n正文。\n\n\
         #### 排查隐患\n\n1. 制度建设\n2. 台账管理"
    );
}

#[test]
fn styled_headings_are_relevelled_by_their_numbering() {
    let raw = "# 关于做好防汛工作的通知\n\n# 一、总体要求\n\n正文。\n\n## （一）指导思想\n\n正文。";
    assert_eq!(
        clean(raw),
        "# 关于做好防汛工作的通知\n\n## 总体要求\n\n正文。\n\n### 指导思想\n\n正文。"
    );
}

#[test]
fn document_title_is_detected_and_joined() {
    let raw = "关于做好2026年防汛工作\n\n的通知\n\n各区县人民政府：\n\n一、总体要求";
    assert_eq!(
        clean(raw),
        "# 关于做好2026年防汛工作的通知\n\n各区县人民政府：\n\n## 总体要求"
    );
    let off = restructure(
        "关于做好防汛工作的通知\n\n正文。",
        &Options {
            detect_title: false,
            ..Default::default()
        },
    );
    assert_eq!(off, "关于做好防汛工作的通知\n\n正文。");
}

#[test]
fn attachment_label_becomes_marker() {
    let raw = "正文。\n\n附件1\n\n2026年防汛物资清单\n\n一、物资种类\n\n正文。";
    assert_eq!(
        clean(raw),
        "正文。\n\n<!-- [附件] -->\n\n# 2026年防汛物资清单\n\n## 物资种类\n\n正文。"
    );
}

#[test]
fn enumerations_and_decimals_are_not_numbering() {
    for raw in [
        "一、二季度全市经济运行平稳。",
        "2、3月份降水偏多。",
        "1.5亿元资金已下达。",
        "1.1倍增长。",
        "2.3万元支出。",
        // 孤立的点分编号不认：没有第二处，谈不上体系。
        "3.2.1 技术路线",
    ] {
        assert_eq!(clean(raw), raw, "{raw}");
    }
}

#[test]
fn dotted_numbering_scheme_sets_levels_and_strips_numbers() {
    let raw = "1 总体要求\n\n正文。\n\n1.1 指导思想\n\n正文。\n\n\
               1.1.1 基本原则\n\n正文。\n\n1.2 工作目标\n\n正文。\n\n\
               2 主要任务\n\n正文。\n\n2.1 排查隐患\n\n正文。";
    assert_eq!(
        clean(raw),
        "## 总体要求\n\n正文。\n\n### 指导思想\n\n正文。\n\n\
         #### 基本原则\n\n正文。\n\n### 工作目标\n\n正文。\n\n\
         ## 主要任务\n\n正文。\n\n### 排查隐患\n\n正文。"
    );
    // 点分方案下标题同样拆「标题。正文」，编号由程序重编。
    assert_eq!(
        clean("1.1 加强组织领导。各单位要高度重视，明确责任分工。\n\n1.2 强化督导检查。"),
        "### 加强组织领导\n各单位要高度重视，明确责任分工。\n\n### 强化督导检查"
    );
}

#[test]
fn roman_and_letter_numbering_follow_their_levels() {
    // 全角罗马数字是最浅一层，与「第X章」同级。
    assert_eq!(
        clean("Ⅰ、总体要求\n\n正文。\n\nⅡ、主要任务\n\n正文。"),
        "## 总体要求\n\n正文。\n\n## 主要任务\n\n正文。"
    );
    // ASCII 罗马数字要成组出现才认。
    assert_eq!(
        clean("I. 总体要求\n\n正文。\n\nII. 主要任务\n\n正文。"),
        "## 总体要求\n\n正文。\n\n## 主要任务\n\n正文。"
    );
    let lone = "I. 这是一句英文。";
    assert_eq!(clean(lone), lone);
    // 字母编号在第六层，压不进 `#####` 就当列表。
    assert_eq!(
        clean("A. 制度建设\n\nB. 台账管理"),
        "1. 制度建设\n2. 台账管理"
    );
    // 只有 `A.` 一行也不认（要成组）。
    let lone = "A. 一家单位的情况。";
    assert_eq!(clean(lone), lone);
}

#[test]
fn inline_enumeration_recognizes_diyi_and_shouxian() {
    assert_eq!(
        clean(
            "会议强调，第一，加强组织领导，明确责任分工；\
             第二，强化督导检查，确保任务落实；第三，严格考核问责，倒逼工作落地。"
        ),
        "会议强调，\n\n**第一**，加强组织领导，明确责任分工；\n\n\
         **第二**，强化督导检查，确保任务落实；\n\n\
         **第三**，严格考核问责，倒逼工作落地。"
    );
    assert_eq!(
        clean("会议要求，首先，压实责任；其次，加强督导；最后，严格问责。"),
        "会议要求，\n\n**首先**，压实责任；\n\n**其次**，加强督导；\n\n\
         **最后**，严格问责。"
    );
    // 单独的「最后，」与「第一条」「第一章」都不拆段。
    let alone = "最后，请各单位抓好落实。";
    assert_eq!(clean(alone), alone);
    assert_eq!(
        clean("第一条　为规范管理，制定本办法。"),
        "**第一条**　为规范管理，制定本办法。"
    );
}

#[test]
fn number_jumps_are_reported() {
    let (markdown, report) = restructure_with_report(
        "一、总体要求\n\n正文。\n\n三、主要任务\n\n正文。",
        &Options::import(true, false),
    );
    assert_eq!(markdown, "## 总体要求\n\n正文。\n\n## 主要任务\n\n正文。");
    assert_eq!(
        report.jumps().to_vec(),
        [NumberJump {
            from: "一、".into(),
            to: "三、".into()
        }]
    );

    // 点分编号只在同一个父前缀下比。
    let (_, report) = restructure_with_report(
        "1.1 背景\n\n正文。\n\n1.2 现状\n\n正文。\n\n1.4 方法\n\n正文。",
        &Options::import(false, false),
    );
    assert_eq!(
        report.jumps().to_vec(),
        [NumberJump {
            from: "1.2".into(),
            to: "1.4".into()
        }]
    );

    // 顺着写的没有跳号。
    let (_, report) = restructure_with_report(
        "一、总体要求\n\n正文。\n\n二、主要任务\n\n正文。",
        &Options::import(true, false),
    );
    assert!(report.jumps().is_empty(), "{:?}", report.jumps());
}

#[test]
fn bold_lead_run_is_the_title() {
    let raw = "**（一）坚持问题导向，突出工作重点。**各地要聚焦薄弱环节。";
    assert_eq!(
        clean(raw),
        "### 坚持问题导向，突出工作重点\n各地要聚焦薄弱环节。"
    );
}

#[test]
fn soft_return_separates_title_from_body() {
    let raw = "一、总体要求\\\n坚持稳中求进工作总基调。";
    assert_eq!(clean(raw), "## 总体要求\n坚持稳中求进工作总基调。");
}

#[test]
fn tables_code_and_markers_are_untouched() {
    let raw = "| 1. 序号 | 名称 |\n| --- | --- |\n| 1 | 甲 |\n\n```\n一、不是标题\n```\n\n<!-- [附件] -->";
    assert_eq!(clean(raw), raw);
}

#[test]
fn plain_text_lines_are_paragraphs() {
    let raw = "关于开展安全检查的通知\n各单位：\n一、检查范围\n全市所有生产经营单位。";
    let cleaned = restructure(raw, &Options::import(true, true));
    assert_eq!(
        cleaned,
        "# 关于开展安全检查的通知\n\n各单位：\n\n## 检查范围\n\n全市所有生产经营单位。"
    );
}

#[test]
fn circled_items_are_always_a_list() {
    let raw = "检查内容包括：\n\n①制度建设；\n\n②台账管理。";
    assert_eq!(clean(raw), "检查内容包括：\n\n1. 制度建设；\n2. 台账管理。");
}

/// 清洗的产物要能被起草页解析器读成预期的块：标题、列表、段落。
#[test]
fn output_parses_as_expected_blocks() {
    use crate::export::{MarkdownBlock, parse_markdown};
    let cleaned = clean(
        "一、总体要求\n\n（一）加强组织领导。各单位要高度重视。\n\n\
         请报送以下材料：\n\n1.工作总结；\n\n2.统计报表。",
    );
    let blocks = parse_markdown(&cleaned);
    let headings = blocks
        .iter()
        .filter(|block| matches!(block, MarkdownBlock::Heading(..)))
        .count();
    let items = blocks
        .iter()
        .filter(|block| matches!(block, MarkdownBlock::OrderedListItem { .. }))
        .count();
    assert_eq!((headings, items), (2, 2), "{cleaned}\n{blocks:#?}");
}

#[test]
fn inline_enumeration_splits_and_bolds_each_lead() {
    let raw = "会议强调，一是加强组织领导，明确责任分工；二是强化督导检查，确保任务落实；\
               三是严格考核问责，倒逼工作落地。";
    assert_eq!(
        clean(raw),
        "会议强调，\n\n**一是加强组织领导**，明确责任分工；\n\n\
         **二是强化督导检查**，确保任务落实；\n\n\
         **三是严格考核问责**，倒逼工作落地。"
    );
}

#[test]
fn unnumbered_short_heading_is_bolded() {
    let raw = "一、总体要求\n\n各单位要高度重视，明确责任分工，确保各项任务落到实处。\n\n\
               工作目标\n\n到2027年基本建成，重点领域和关键环节取得实质性进展，形成一批可复制可推广的经验做法。";
    assert_eq!(
        clean(raw),
        "## 总体要求\n\n各单位要高度重视，明确责任分工，确保各项任务落到实处。\n\n\
         **工作目标**\n\n到2027年基本建成，重点领域和关键环节取得实质性进展，形成一批可复制可推广的经验做法。"
    );
}

#[test]
fn article_labels_are_bolded_not_promoted() {
    let raw = "第一条　为规范管理，制定本办法。\n\n第二条　本办法适用于本单位。";
    assert_eq!(
        clean(raw),
        "**第一条**　为规范管理，制定本办法。\n\n**第二条**　本办法适用于本单位。"
    );
}

#[test]
fn layout_head_and_tail_are_stripped() {
    let raw = "000123\n\n秘密★1年\n\n××市人民政府文件\n\n\
               ×政发〔2026〕12号\n\n签发人：张三\n\n\
               关于做好防汛工作的通知\n\n各区县人民政府：\n\n正文。\n\n\
               ××市人民政府办公室\n\n2026年5月10日\n\n\
               抄送：市委办公室。\n\n××市人民政府办公室　2026年5月10日印发";
    assert_eq!(
        clean(raw),
        "# 关于做好防汛工作的通知\n\n各区县人民政府：\n\n正文。"
    );
}

#[test]
fn chapter_headings_are_recognized() {
    let raw = "第一章 总则\n\n正文。\n\n一、总体要求\n\n正文。";
    assert_eq!(clean(raw), "## 总则\n\n正文。\n\n### 总体要求\n\n正文。");
}

#[test]
fn full_width_digits_and_half_paren_numbering_are_recognized() {
    assert_eq!(
        parse_prefix("１．总体要求", false, Numbering::default()).map(|prefix| prefix.level),
        Some(3)
    );
    assert_eq!(
        parse_prefix("1)具体事项", false, Numbering::default()).map(|prefix| prefix.level),
        Some(4)
    );
    assert_eq!(
        parse_prefix("第一章　总则", false, Numbering::default()).map(|prefix| prefix.level),
        Some(0)
    );
    assert!(parse_prefix("第一条　为规范管理", false, Numbering::default()).is_none());
}

#[test]
fn skipped_third_level_is_compressed_under_its_parent() {
    let raw = "一、总体要求\n\n正文。\n\n1.具体任务\n\n正文。";
    assert_eq!(
        clean(raw),
        "## 总体要求\n\n正文。\n\n### 具体任务\n\n正文。"
    );
}

#[test]
fn same_line_attachment_and_title_suffix_are_handled() {
    let raw = "关于做好防汛工作的通知\n\n（征求意见稿）\n\n正文。\n\n\
               附件1：2026年防汛物资清单\n\n一、物资种类\n\n正文。";
    assert_eq!(
        clean(raw),
        "# 关于做好防汛工作的通知（征求意见稿）\n\n正文。\n\n\
         <!-- [附件] -->\n\n# 2026年防汛物资清单\n\n## 物资种类\n\n正文。"
    );
}

#[test]
fn attachment_description_line_is_dropped_when_content_follows() {
    let raw = "正文。\n\n附件：1.物资清单 2.责任分工\n\n附件1\n\n物资清单\n\n内容。";
    assert_eq!(
        clean(raw),
        "正文。\n\n<!-- [附件] -->\n\n# 物资清单\n\n内容。"
    );
}

#[test]
fn ribbon_restructure_keeps_title_and_layout() {
    // 对已有稿子做结构整理：不认标题、不剥版头版记、不拆段内枚举。
    let raw = "关于做好防汛工作的通知\n\n一、总体要求\n\n一是加强领导；二是强化督导。";
    assert_eq!(
        restructure(raw, &Options::restructure_only()),
        "关于做好防汛工作的通知\n\n## 总体要求\n\n一是加强领导；二是强化督导。"
    );
}
