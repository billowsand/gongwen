//! 视觉 diff 引擎：把两个版本展开成「印在纸上的字」再比较，产出附着在新版
//! 视觉块上的标注层 `RedlineOverlay`（方案见 docs/version-diff-redesign.md 第四节）。
//!
//! 与 `diff.rs` 的代码层 diff 各算各的：这里比的是编号已生成、Markdown 标记
//! 已剥离的渲染文本，只标真实文字的增删，格式变化（加粗、层级、段落拆合、
//! 空格、全半角）一律不标。导出侧把标注层序列化成带哨兵的 Markdown，交给原有
//! 导出链，哨兵在「解析后」注入，不再碰用户源码的语法结构。
//!
//! 比较规则（与方案第四节一一对应）：
//! 1. 按章节对齐：先比标题序列（只比文字），章节内再比正文；
//! 2. 正文按文字流比：段落边界是软分隔，拆分 / 合并不产生标记；
//! 3. 词级 token：jieba 分词，日期、数字 + 单位、文号、公式整体为一个 token；
//! 4. 归并：夹缝 ≤ 2 字吸收；分句改动过半或改动片段 ≥ 3 段则整句删旧插新；
//! 5. 归一化后再比：比较键忽略空白，标点改动照标；
//! 6. 移动识别：未能对齐的删 / 增块相似度 ≥ 0.8 配成移动，段首加移来注记；
//! 7. 公文要素字段整体替换：旧值删除线、新值加框，不做词级；
//! 8. 编号：标题与列表编号是程序生成的，不参与比较；新增标题整项加框，
//!    删除的标题不显示编号；
//! 9. 表格：逐格词级；整行增删整行标注；列数变化整表删旧插新；
//! 10. 公式：整体比较，变了就整体删旧插新。

mod compare;
// 接线在下一步（RedlineDoc.elements + redline::build_with_inputs）完成后去掉。
#[allow(dead_code)]
pub(crate) mod elements;
mod model;
mod overlay;
mod postprocess;
mod serialize;
mod tokenize;

pub(crate) use compare::diff_documents;
pub(crate) use model::DocumentModel;
#[cfg(test)]
pub(crate) use postprocess::REDLINE_PREAMBLE_TEX;
pub(crate) use postprocess::{redline_research_docx, redline_research_tex_files};
#[cfg(test)]
pub(crate) use serialize::to_marked_markdown;
pub(crate) use serialize::{MarkedSpan, SourceSide, to_marked_markdown_with_spans};

#[cfg(test)]
mod review_probes {
    //! 审查探针（docs/version-diff-redesign.md 第 ① 期验收）：每条对应一个
    //! 曾实际复现的行为回归，期望值写在注释里。

    use super::*;
    use crate::export::{
        REDLINE_ADD_CLOSE, REDLINE_ADD_OPEN, REDLINE_DEL_CLOSE, REDLINE_DEL_OPEN, RedlineKind,
    };

    fn run(old: &str, new: &str) -> String {
        let overlay = diff_documents(
            &DocumentModel::from_markdown(old),
            &DocumentModel::from_markdown(new),
        );
        to_marked_markdown(&overlay)
            .chars()
            .map(|ch| match ch {
                REDLINE_DEL_OPEN | REDLINE_DEL_CLOSE => "~".to_string(),
                REDLINE_ADD_OPEN => "[".to_string(),
                REDLINE_ADD_CLOSE => "]".to_string(),
                other => other.to_string(),
            })
            .collect()
    }

    /// 去掉 ~删除~ 得新版，去掉 [新增] 得旧版。
    fn sides(marked: &str) -> (String, String) {
        let (mut old, mut new) = (String::new(), String::new());
        let (mut in_del, mut in_add) = (false, false);
        for ch in marked.chars() {
            match ch {
                '~' => in_del = !in_del,
                '[' => in_add = true,
                ']' => in_add = false,
                _ => {
                    if !in_add {
                        old.push(ch);
                    }
                    if !in_del {
                        new.push(ch);
                    }
                }
            }
        }
        (old, new)
    }

    #[test]
    fn p1_bold_only() {
        // 期望：加粗是纯格式变化，前后无任何标记，原文样式保留。
        let out = run("请按时报送材料。", "请**按时**报送材料。");
        assert_eq!(out, "请**按时**报送材料。");
    }

    #[test]
    fn p2_invariant() {
        // 期望：任意输入，去 Added 得旧文、去 Deleted 得新文（逐字）。
        let cases = [
            (
                "请市教育局于八月前报送有关材料。",
                "请省教育厅于九月前报送相关材料。",
            ),
            (
                "各单位要高度重视，认真组织，确保按时完成。",
                "各部门要充分重视，精心组织，确保如期完成。",
            ),
            ("会议定于下周一上午召开。", "会议定于下周三下午在三楼召开。"),
            (
                "加强组织领导，落实工作责任。",
                "切实加强组织领导，全面落实工作责任。",
            ),
            (
                "请于8月10日前报送，逾期视为放弃。",
                "请于8月15日前报送相关材料，逾期视为自动放弃。",
            ),
        ];
        for (old, new) in cases {
            let out = run(old, new);
            let (o, n) = sides(&out);
            assert_eq!(n, new, "新版文字被归并吃掉/多出：{out}");
            assert_eq!(o, old, "旧版文字被归并吃掉/多出：{out}");
        }
    }

    #[test]
    fn p3_inline_list_renumber() {
        // 期望：只有「新增工作」一项加框，圈号顺移无标记（规则：编号是
        // 程序生成的版式，不算修改）。
        let old = "工作要求如下：\n1. 甲项工作\n2. 乙项工作\n3. 丙项工作";
        let new = "工作要求如下：\n1. 新增工作\n2. 甲项工作\n3. 乙项工作\n4. 丙项工作";
        let out = run(old, new);
        assert!(out.contains("[新增工作"), "新增项加框：{out}");
        assert!(
            !out.contains("~②~") && !out.contains("[③]"),
            "圈号顺移无标记：{out}"
        );
    }

    #[test]
    fn p4_multi_sentence_paragraph() {
        // 期望：只标改动的词，不整段替换。
        let old =
            "第一句保持不变。第二句也不变动。第三句这里写甲，后面写乙，最后写丙。第四句不变。";
        let new =
            "第一句保持不变。第二句也不变动。第三句这里写丁，后面写戊，最后写己。第四句不变。";
        let out = run(old, new);
        assert!(out.contains("~甲~[丁]"), "第一个词就地标注：{out}");
        assert!(
            !out.starts_with('~') && !out.starts_with('['),
            "不整段替换：{out}"
        );
        let old = "关于报送材料的通知已经收悉。请各单位于月底前报送。逾期不报的视为放弃。";
        let new = "关于报送材料的通知已经收到。请各部门于月底前报送。逾期未报的视为放弃。";
        let out = run(old, new);
        assert!(out.contains("~收悉~[收到]"), "每句只标改动的词：{out}");
        assert!(
            !out.starts_with('~') && !out.starts_with('['),
            "不整段替换：{out}"
        );
    }

    #[test]
    fn p5_heading_insert() {
        // 期望：插入 / 删除标题，其余标题无标记；删除的标题不显示编号。
        let old = "## 工作目标\n\n目标段。\n\n## 保障措施\n\n保障段。";
        let new = "## 工作目标\n\n目标段。\n\n## 职责分工\n\n分工段。\n\n## 保障措施\n\n保障段。";
        let out = run(old, new);
        assert!(out.contains("[职责分工]"), "新增标题整项加框：{out}");
        assert!(
            !out.contains("~工作目标~") && !out.contains("~保障措施~"),
            "其余标题无标记：{out}"
        );
        let out = run(new, old);
        assert!(out.contains("~职责分工~"), "删除的标题文字画删除线：{out}");
        assert!(
            !out.lines().any(|line| line.starts_with("## ~")),
            "删除的标题不排成标题行（不带编号）：{out}"
        );
    }

    #[test]
    fn p6_research_markers() {
        // 期望：摘要 / 参考文献等区段标记原样保留，mdx 靠它们分区。
        let text = "<!-- [摘要] -->\n\n摘要内容。\n\n## 引言\n\n正文。\n\n<!-- [参考文献] -->\n\n[1] 文献。";
        let changed = text.replace("正文。", "正文改。");
        let out = run(text, &changed);
        assert!(out.contains("<!-- [摘要] -->"), "摘要标记保留：{out}");
        assert!(
            out.contains("<!-- [参考文献] -->"),
            "参考文献标记保留：{out}"
        );
    }

    #[test]
    fn p7_split_plus_edit() {
        // 期望：拆分 + 别处改一个字，只标改的那个字，无移动注记。
        let old = "A 第一句。A 第二句。\n\nB 段落。";
        let new = "A 第一句。\n\nA 第二句。\n\nB 段落改。";
        let out = run(old, new);
        assert!(out.contains("[改]"), "只标改动的字：{out}");
        assert!(!out.contains("移来"), "拆分不触发移动注记：{out}");
    }

    #[test]
    fn p8_move() {
        // 期望：只有真正挪动的第三段带移动注记。
        let old = "第一段内容比较长一些用于识别。\n\n第二段内容也比较长用于识别移动。\n\n第三段内容同样足够长可以识别。";
        let new = "第三段内容同样足够长可以识别。\n\n第一段内容比较长一些用于识别。\n\n第二段内容也比较长用于识别移动。";
        let out = run(old, new);
        assert_eq!(out.matches("移来").count(), 1, "恰好一个移动注记：{out}");
        assert!(
            out.contains("（本段由原第3段移来）"),
            "注记在挪动的段上：{out}"
        );
    }

    #[test]
    fn p9_title_wrap_marks() {
        // 期望：标题折行后，两行的标注各自成对（行尾补闭合、行首补开启），
        // 第二行里的新增部分仍然是 Added。
        use crate::export::{mark_added, redline_slice_lines};
        let title = format!(
            "关于进一步{}的通知",
            mark_added("加强和改进全省教育系统安全生产工作")
        );
        let plain = crate::export::strip_redline(&title);
        let mid = plain.chars().count() / 2;
        let l1: String = plain.chars().take(mid).collect();
        let l2: String = plain.chars().skip(mid).collect();
        let lines = redline_slice_lines(&title, &[l1, l2]);
        assert_eq!(lines.len(), 2, "切成两行：{lines:?}");
        for line in &lines {
            let chunks = crate::export::redline_chunks(line);
            assert_eq!(
                chunks
                    .iter()
                    .filter(|chunk| chunk.kind == RedlineKind::Added)
                    .count(),
                1,
                "每行各有一个成对的新增块：{chunks:?}"
            );
            let opens = line
                .chars()
                .filter(|ch| matches!(*ch, REDLINE_DEL_OPEN | REDLINE_ADD_OPEN))
                .count();
            let closes = line
                .chars()
                .filter(|ch| matches!(*ch, REDLINE_DEL_CLOSE | REDLINE_ADD_CLOSE))
                .count();
            assert_eq!(opens, closes, "哨兵成对出现：{line:?}");
        }
    }

    #[test]
    fn p10_standalone_list_numbering() {
        // 期望：独立列表三项编号是 1、2、3（重新解析后仍是一组）。
        let old = "要求如下。\n\n1. 甲项工作\n2. 乙项工作\n3. 丙项工作";
        let new = "要求如下。\n\n1. 甲项工作\n2. 乙项工作改\n3. 丙项工作";
        let overlay = diff_documents(
            &DocumentModel::from_markdown(old),
            &DocumentModel::from_markdown(new),
        );
        let md = to_marked_markdown(&overlay);
        let numbers: Vec<usize> = crate::export::parse_markdown(&md)
            .into_iter()
            .filter_map(|block| match block {
                crate::export::MarkdownBlock::OrderedListItem { number, .. } => Some(number),
                _ => None,
            })
            .collect();
        assert_eq!(numbers, vec![1, 2, 3], "重新解析后编号连续：{md}");
    }

    #[test]
    fn p11_insert_paragraph_first() {
        // 期望：只在最前面插一段时，只有新段加框，后面的段落无移动注记。
        let old = "第一段内容比较长一些用于识别。\n\n第二段内容也比较长用于识别移动。\n\n第三段内容同样足够长可以识别。";
        let new = format!("新写的一段开头。\n\n{old}");
        let out = run(old, &new);
        assert!(out.contains("[新写的一段开头。]"), "新段加框：{out}");
        assert!(!out.contains("移来"), "后面的段落不误标移动：{out}");
    }

    #[test]
    fn p12_far_apart_delete_and_insert_stay_in_place() {
        // 期望：远处「删一段」与「增一段」互不相干——新段整段加框落在甲段后，
        // 丁段整段删除线留在丙、戊两段之间，不拼成一处整句替换。
        let tail = "，各地各校要深刻认识本项工作的重要意义。";
        let paragraph = |name: &str| format!("{name}{tail}");
        let old = ["甲段", "乙段", "丙段", "丁段", "戊段"]
            .map(paragraph)
            .join("\n\n");
        let added = "这是新增的一段话，专门用来说明新的工作要求。";
        let new = [
            paragraph("甲段"),
            added.to_string(),
            paragraph("乙段"),
            paragraph("丙段"),
            paragraph("戊段"),
        ]
        .join("\n\n");
        let out = run(&old, &new);
        let expected = [
            paragraph("甲段"),
            format!("[{added}]"),
            paragraph("乙段"),
            paragraph("丙段"),
            format!("~{}~", paragraph("丁段")),
            paragraph("戊段"),
        ]
        .join("\n\n");
        assert_eq!(out.trim_end(), expected);
    }

    #[test]
    fn p13_moves_next_to_a_changed_table_or_list() {
        // 第 ③ 期测试 F1：同一节里有锚不上的表格 / 列表项时，段落下标与组内块
        // 序号不再相等。移动识别曾把两套下标混用：越界 panic（界面线程上就是
        // 闪退），或者比错段落、把移动标成整段删 + 整段增。
        // 期望：不 panic；挪到最前的那段带移来注记、正文不加框。
        let paragraphs = |order: [usize; 3]| {
            let texts = [
                "第一段说明经费来源和预算科目。",
                "第二段说明经费用途和开支范围。",
                "第三段说明经费管理要求和报销流程。",
            ];
            order.map(|index| texts[index]).join("\n\n")
        };
        let rotated = paragraphs([2, 0, 1]);
        let original = paragraphs([0, 1, 2]);
        for (old_head, new_head) in [
            (
                "| 项目 | 金额 |\n| --- | ---: |\n| 会场 | 12000元 |",
                "| 项目 | 金额 |\n| --- | ---: |\n| 会场 | 15000元 |",
            ),
            (
                "- 甲项工作。\n\n- 乙项工作。",
                "- 甲项工作。\n\n- 乙项工作要细化。",
            ),
        ] {
            for tail in ["", "\n\n妥否，请指示。"] {
                let old = format!("{old_head}\n\n{original}{tail}");
                let new = format!("{new_head}\n\n{rotated}{tail}");
                let out = run(&old, &new);
                assert!(out.contains("本段由原第"), "应识别为移动：{out}");
                assert!(
                    !out.contains("[第三段说明经费管理要求和报销流程。]")
                        && !out.contains("~第三段说明经费管理要求和报销流程。~"),
                    "挪动的段不该整段删 + 整段增：{out}"
                );
                // 同组里改过的表格格 / 列表项照常就地标注（移动不做两侧还原
                // 检查：挪走的段只在新位置出现一次，带注记）。
                assert!(out.contains('['), "同组改动照常标注：{out}");
            }
        }
    }

    #[test]
    fn p14_adding_or_removing_a_heading_leaves_its_paragraphs_alone() {
        // 第 ③ 期测试 F9：只删一个标题（正文不动），或者在没动的段前插一个标题，
        // 下面的正文曾整段删一遍、再整段加一遍。期望：只标标题本身。
        let tail = "，各地各校要深刻认识本项工作的重要意义，确保各项部署落到实处。";
        let para = |name: &str| format!("{name}{tail}");
        let old = format!(
            "## 报送内容\n\n{}\n\n### 数据校验\n\n{}\n\n{}\n\n## 报送要求\n\n{}\n\n妥否，请指示。",
            para("甲段"),
            para("乙段"),
            para("丙段"),
            para("丁段"),
        );
        // 删掉三级标题「数据校验」，乙、丙两段并进上一章。
        let new = old.replace("### 数据校验\n\n", "");
        let out = run(&old, &new);
        assert!(out.contains("~数据校验~"), "删掉的标题画删除线：{out}");
        for name in ["甲段", "乙段", "丙段", "丁段"] {
            assert_eq!(
                out.matches(name).count(),
                1,
                "{name} 只出现一次、不标：{out}"
            );
        }
        assert_eq!(out.matches('~').count(), 2, "只有标题一处删除：{out}");
        assert!(!out.contains('['), "没有新增：{out}");
        // 在「妥否，请指示。」前插一个新标题 + 一段。
        let new = old.replace(
            "妥否，请指示。",
            "## 需要协调的事项\n\n请办公室协调会场保障。\n\n妥否，请指示。",
        );
        let out = run(&old, &new);
        assert!(
            out.contains("[需要协调的事项]") && out.contains("[请办公室协调会场保障。]"),
            "新标题与新段加框：{out}"
        );
        assert!(
            out.ends_with("妥否，请指示。") && !out.contains('~'),
            "没动的结尾段原样、没有删除：{out}"
        );
        // 删掉二级标题「报送要求」：它下面的丁段并进前一章。
        let new = old.replace("## 报送要求\n\n", "");
        let out = run(&old, &new);
        assert!(out.contains("~报送要求~"), "{out}");
        assert_eq!(out.matches("丁段").count(), 1, "丁段不标：{out}");
    }

    /// 带哨兵的 Markdown 还原两侧的可见文字：去新增得旧版、去删除得新版；
    /// 行内标记（`**`、转义）与空白不计。
    fn both_sides(marked: &str) -> (String, String) {
        let mut old = String::new();
        let mut new = String::new();
        let mut state = RedlineKind::Same;
        for ch in marked.chars() {
            match ch {
                REDLINE_DEL_OPEN => state = RedlineKind::Deleted,
                REDLINE_ADD_OPEN => state = RedlineKind::Added,
                REDLINE_DEL_CLOSE | REDLINE_ADD_CLOSE => state = RedlineKind::Same,
                _ => {
                    if state != RedlineKind::Added {
                        old.push(ch);
                    }
                    if state != RedlineKind::Deleted {
                        new.push(ch);
                    }
                }
            }
        }
        (visible(&old), visible(&new))
    }

    fn visible(markdown: &str) -> String {
        markdown
            .split("\n\n")
            .map(crate::export::plain_text)
            .collect::<String>()
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect()
    }

    fn assert_sides(old: &str, new: &str) {
        let overlay = diff_documents(
            &DocumentModel::from_markdown(old),
            &DocumentModel::from_markdown(new),
        );
        let marked = to_marked_markdown(&overlay);
        let (o, n) = both_sides(&marked);
        let shown = run(old, new);
        assert_eq!(o, visible(old), "去新增应得旧版：{shown}");
        assert_eq!(n, visible(new), "去删除应得新版：{shown}");
    }

    #[test]
    fn q1_gaps_between_same_kind_changes_keep_both_sides() {
        // 第二轮 R1：删-夹缝-删、增-夹缝-增时夹缝不得重复或丢失。
        for (old, new) in [
            (
                "请各有关单位认真组织学习并及时报送。",
                "请各单位组织学习并报送。",
            ),
            (
                "请各单位组织学习并报送。",
                "请各有关单位认真组织学习并及时报送。",
            ),
            ("甲乙丙丁戊己庚辛。", "甲丙戊庚。"),
            ("要加强领导，要落实责任，要强化督导。", "要落实责任。"),
            ("请**按时报送**材料。", "请**按期报送**材料。"),
            ("请按时报送材料。", "请**按时报送**有关材料。"),
            ("请**按时**报送材料。", "请按时报送相关材料。"),
            (
                "**一是**加强领导。**二是**落实责任。",
                "**一是**切实加强领导。**二是**全面落实责任。",
            ),
        ] {
            assert_sides(old, new);
        }
    }

    #[test]
    fn q2_edits_inside_bold_keep_position_and_style() {
        // 第二轮 R2：加粗区间内改字，不重复原文、删除不挪位、加粗不跨哨兵
        // （导出器先按哨兵切块再配对加粗，跨块的 `**` 会配错）。
        assert_eq!(
            run("请**按时报送**材料。", "请**按期报送**材料。"),
            "请~按时~[**按期**]**报送**材料。"
        );
        assert_eq!(
            run(
                "**一是**加强领导。**二是**落实责任。",
                "**一是**切实加强领导。**二是**全面落实责任。",
            ),
            "**一是**~加强~[切实加强]领导。**二是**~落实~[全面落实]责任。"
        );
        assert_eq!(
            run("请**按时报送有关**材料。", "请按时**报送**材料。"),
            "请按时**报送**~有关~材料。"
        );
    }

    fn parsed(old: &str, new: &str) -> Vec<String> {
        let overlay = diff_documents(
            &DocumentModel::from_markdown(old),
            &DocumentModel::from_markdown(new),
        );
        crate::export::parse_markdown(&to_marked_markdown(&overlay))
            .into_iter()
            .map(|block| match block {
                crate::export::MarkdownBlock::Paragraph(text) => format!("P:{}", readable(&text)),
                crate::export::MarkdownBlock::OrderedListItem { number, text } => {
                    format!("L{number}:{}", readable(&text))
                }
                other => format!("{other:?}"),
            })
            .collect()
    }

    fn readable(text: &str) -> String {
        text.chars()
            .map(|ch| match ch {
                REDLINE_DEL_OPEN | REDLINE_DEL_CLOSE => "~".to_string(),
                REDLINE_ADD_OPEN => "[".to_string(),
                REDLINE_ADD_CLOSE => "]".to_string(),
                other => other.to_string(),
            })
            .collect()
    }

    #[test]
    fn q3_inline_lists_keep_layout_and_numbering() {
        // 第二轮 R3：段内列表仍排成段内圈号；被删的项折进前一项、不占编号，
        // 也不凭空多出句号；引导句带加粗时编号照样剥得准。
        let cases: [(&str, &str, &[&str]); 6] = [
            (
                "要求：\n1. 甲项工作\n2. 乙项工作\n3. 丙项工作",
                "要求：\n1. 甲项工作\n2. 丙项工作",
                &["P:要求：①甲项工作。~乙项工作。~②丙项工作。"],
            ),
            (
                "要求：\n1. 甲项工作安排部署\n2. 乙项工作",
                "要求：\n1. 甲项工作\n2. 乙项工作",
                &["P:要求：①甲项工作~安排部署~。②乙项工作。"],
            ),
            (
                "要求：\n1. 甲项工作\n2. 乙项工作",
                "要求如下：\n1. 甲项工作\n2. 乙项工作",
                &["P:要求[如下]：①甲项工作。②乙项工作。"],
            ),
            (
                "**要求**如下：\n1. 甲项工作\n2. 乙项工作",
                "**要求**如下：\n1. 甲项工作\n2. 乙项工作改",
                &["P:**要求**如下：①甲项工作。②乙项工作[改]。"],
            ),
            (
                "要求如下。\n\n1. 甲项工作\n2. 乙项工作\n3. 丙项工作",
                "要求如下。\n\n1. 甲项工作\n2. 丙项工作",
                &["P:要求如下。", "L1:甲项工作。~乙项工作。~", "L2:丙项工作。"],
            ),
            (
                "要求如下。\n\n1. 甲项工作\n2. 乙项工作\n3. 丙项工作",
                "要求如下。\n\n1. 乙项工作\n2. 丙项工作",
                &[
                    "P:要求如下。",
                    "P:~甲项工作。~",
                    "L1:乙项工作。",
                    "L2:丙项工作。",
                ],
            ),
        ];
        for (old, new, expected) in cases {
            assert_eq!(parsed(old, new), expected, "{old:?} → {new:?}");
        }
    }

    /// 线性同余发生器：测试要可复现，不引入随机数依赖。
    struct Lcg(u64);

    impl Lcg {
        fn below(&mut self, n: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 33) as usize) % n.max(1)
        }
    }

    #[test]
    fn generated_edits_always_restore_both_sides() {
        // 生成式不变式：真实公文句子随机删、插、换 1～3 个词，偶尔给一个词
        // 加粗，花脸稿「去新增 == 旧版」「去删除 == 新版」逐字成立。
        const SENTENCES: [&str; 12] = [
            "请各有关单位认真组织学习，并于8月10日前将书面材料报送市教育局办公室。",
            "各地要高度重视安全生产工作，切实加强组织领导，全面落实主体责任。",
            "会议定于2026年9月28日上午9时在市政府三楼第一会议室召开。",
            "经研究，同意你单位关于开展全市中小学校园安全专项检查的请示。",
            "逾期未报送的，视为自动放弃评选资格，由此产生的后果由各单位自行承担。",
            "一是强化统筹协调，二是完善制度建设，三是加强督促检查。",
            "本通知自印发之日起施行，原有规定与本通知不一致的，以本通知为准。",
            "联系人：张三，联系电话：0551-12345678。",
            "项目总投资约3.5亿元，其中财政资金1.2亿元，社会资本2.3亿元。",
            "请于每月5日前报送上月工作进展情况，遇有重大情况随时报告。",
            "各部门要结合实际，制定具体实施方案，确保各项任务按期完成。",
            "现将有关事项通知如下，请认真贯彻执行。",
        ];
        const WORDS: [&str; 10] = [
            "切实",
            "全面",
            "进一步",
            "相关",
            "有关",
            "认真",
            "及时",
            "省",
            "工作",
            "，",
        ];
        let mut rng = Lcg(20_260_924);
        for round in 0..400 {
            let paragraphs = 1 + rng.below(2);
            let mut old_doc = Vec::new();
            let mut new_doc = Vec::new();
            for _ in 0..paragraphs {
                let sentence = SENTENCES[rng.below(SENTENCES.len())];
                let mut tokens: Vec<String> = super::tokenize::tokenize(sentence)
                    .into_iter()
                    .map(|token| token.text)
                    .collect();
                old_doc.push(sentence.to_string());
                for _ in 0..1 + rng.below(3) {
                    let at = rng.below(tokens.len());
                    let word = WORDS[rng.below(WORDS.len())].to_string();
                    match rng.below(4) {
                        0 if tokens.len() > 1 => {
                            tokens.remove(at);
                        }
                        1 => tokens.insert(at, word),
                        2 => tokens[at] = word,
                        _ => tokens[at] = format!("**{}**", tokens[at]),
                    }
                }
                new_doc.push(tokens.concat());
            }
            let old = old_doc.join("\n\n");
            let new = new_doc.join("\n\n");
            let shown = run(&old, &new);
            if shown.contains("移来") {
                continue; // 移动改变顺序，逐字比对不适用
            }
            let overlay = diff_documents(
                &DocumentModel::from_markdown(&old),
                &DocumentModel::from_markdown(&new),
            );
            let (o, n) = both_sides(&to_marked_markdown(&overlay));
            assert_eq!(o, visible(&old), "第 {round} 组去新增应得旧版：{shown}");
            assert_eq!(n, visible(&new), "第 {round} 组去删除应得新版：{shown}");
        }
    }

    #[test]
    fn q4_whole_sentence_replacement_keeps_shared_punctuation() {
        // 整句替换时，两句共有的句尾标点不删了再加：否则排出一个单独套框的
        // 「。」，还可能被挤到下一行行首。
        let out = run(
            "首先，这是一个语音输入法，还挺好用的，基于本地模型的。",
            "首先，这是一个语音输入法。",
        );
        assert!(!out.contains("[。]"), "句号不该删了再加：{out}");
        assert_sides(
            "首先，这是一个语音输入法，还挺好用的，基于本地模型的。",
            "首先，这是一个语音输入法。",
        );
    }

    /// 长稿：没有标题、几百段落在同一个 run 里，段落锚定改走「先锚一字不差的段、
    /// 夹缝里再做加权 LCS」（`compare::GLOBAL_ANCHOR_LIMIT`）。同一份输入分别强制走
    /// 全局算法与夹缝算法，产出的花脸稿必须逐字节相同；夹缝算法还要快。
    #[test]
    fn a_long_headingless_draft_gives_the_same_redline_on_both_anchor_paths() {
        use super::compare::ANCHOR_LIMIT_OVERRIDE;
        let paragraph = |index: usize| {
            format!("第{index}段，各地各校要深刻认识本项工作的重要意义，确保各项部署落到实处。")
        };
        let old_paragraphs: Vec<String> = (1..=400).map(paragraph).collect();
        let mut new_paragraphs = old_paragraphs.clone();
        // 改写几段、删一段、插一段、把第 51 段挪到第 300 段后面。
        for index in [10usize, 150, 277, 390] {
            new_paragraphs[index] = new_paragraphs[index]
                .replace("深刻认识", "充分认识")
                .replace("落到实处", "落到实处、见到实效");
        }
        new_paragraphs.remove(200);
        new_paragraphs.insert(
            100,
            "这是新增的一段话，专门用来说明新的工作要求。".to_string(),
        );
        let moved = new_paragraphs.remove(51);
        new_paragraphs.insert(300, moved);
        let old = old_paragraphs.join("\n\n");
        let new = new_paragraphs.join("\n\n");

        let run_with = |limit: usize| {
            ANCHOR_LIMIT_OVERRIDE.with(|cell| cell.set(Some(limit)));
            let started = std::time::Instant::now();
            let marked = run(&old, &new);
            ANCHOR_LIMIT_OVERRIDE.with(|cell| cell.set(None));
            (marked, started.elapsed())
        };
        let (global, global_time) = run_with(usize::MAX);
        let (gapped, gapped_time) = run_with(0);
        assert_eq!(gapped, global, "两条锚定路径的花脸稿必须相同");
        assert!(
            gapped_time < global_time,
            "夹缝算法应比全局快：{gapped_time:?} vs {global_time:?}"
        );
        // 结果本身也对：改写就地标注、移动带注记、新增整段加框、没改的段原样。
        // jieba 把「深刻认识」切成一个词，词级标注按整词删旧插新。
        assert!(
            gapped.contains("第11段，各地各校要~深刻认识~[充分认识]本项工作的重要意义，确保各项部署落到实处[、见到实效]。"),
            "改写就地词级标注"
        );
        assert!(gapped.contains("本段由原第52段移来"), "移动段带注记");
        // 远处「删一段、增一段」各留原位：新段整段加框落在第 100、101 段之间，
        // 第 201 段整段删除线落在第 200、202 段之间。
        assert!(
            gapped.contains(&format!(
                "{}\n\n[这是新增的一段话，专门用来说明新的工作要求。]\n\n{}",
                paragraph(100),
                paragraph(101)
            )),
            "新增段整段加框、落在原位"
        );
        assert!(
            gapped.contains(&format!(
                "{}\n\n~{}~\n\n{}",
                paragraph(200),
                paragraph(201),
                paragraph(202)
            )),
            "删除段整段删除线、落在原位"
        );
        for index in [1usize, 250, 399] {
            let text = paragraph(index);
            assert!(
                gapped.lines().any(|line| line == text),
                "第 {index} 段应原样"
            );
        }
    }
}
