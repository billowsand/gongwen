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
mod model;
mod overlay;
mod postprocess;
mod serialize;
mod tokenize;

pub(crate) use compare::diff_documents;
pub(crate) use model::DocumentModel;
pub(crate) use postprocess::{redline_research_docx, redline_research_tex_files};
pub(crate) use serialize::to_marked_markdown;

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

    #[test]
    fn p1_bold_only() {
        // 期望：加粗是纯格式变化，前后无任何标记，原文样式保留。
        let out = run("请按时报送材料。", "请**按时**报送材料。");
        assert_eq!(out, "请**按时**报送材料。");
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
}
