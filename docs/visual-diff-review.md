# feat/visual-diff-engine 审查意见（5ba65f9）

对照 `docs/version-diff-redesign.md` 第四节与第六节第 ① 期。

## 基线
- `cargo fmt --check` 通过；`cargo clippy --all-targets -D warnings` 零警告（Linux 特性组合）；
  `cargo test --all-targets` 1127 通过、0 失败。
- 问题出在**行为**上：下面每一条都用附带的探针测试（文末）实际跑出来过，不是推测。

## 阻断项（必须修）

### B1 在段首插一段，后面所有段落都被标成「移来」
- 复现：旧版三段，新版在最前面加一段。结果：新段加框（正确），**原来三段每段都带「（本段由原第 N 段移来）」**。
  这是最常见的编辑动作。
- 同源问题：
  - 段落拆分 + 别处改一个字（P7）：没动过的 B 段被标「移来」。
  - 真正的调序（P8，第三段挪到最前）：被标移动的是**没动的**第一、二段，真正挪动的第三段反而没有注记。
- 根因：`build_segments` 把连续正文段并成一个 Stream 段，只要其中有改动，整个段就进 `resolve_run`。
  `resolve_run` 用「第几个段落槽位」判断是不是移动（`same_slot`），序号一错位就算移动。
- 修法：在 run 内先对段落的归一化文本做一次 LCS。LCS 上的段落就是「没动 / 就地改写」。
  只有 LCS 之外、且相似度 ≥ 0.8 的删 / 增配对才算移动。这样移动集合最小，真正挪动的段才有注记。

### B2 归并规则吃字：删除 / 新增两侧还原不回原文
- 复现（P2）：`请市教育局于八月前…` → `请省教育厅于九月前…`。花脸稿去掉新增后得不到旧版，「于」丢了。
- 根因：`absorb_gaps` 把夹缝的 Same 片段整体改成一侧的类型：
  - 删 / 增交界时并进 Added，旧版一侧就少了这几个字；
  - 删-夹缝-删时并进 Deleted，**新版一侧也会少字**。
- 修法：夹缝吸收时两侧各放一份——删除片段末尾追加夹缝文字，新增片段开头追加夹缝文字。
- 必须加不变式测试：对任意输入，「去掉 Added」== 旧文，「去掉 Deleted」== 新文。
  现有的 `stripped()` 辅助函数调用的是 `diff(new, new)`，**自己和自己比**，这条不变式实际上从没被测过。

### B3 分句归并退化成整段替换
- 复现（P4）：一段四句，只在第三句的三个分句里各改了一个词，结果**整段删旧插新**。
  P4b 三句各改一个词，也是整段替换。这正是方案要避免的「看不出改了哪个字」。
- 根因：`replace_heavy_sentences` 只在片段末尾切分句。未改动的 Same 片段常常跨过好几个句号，
  于是整段落进一个 region，`pieces >= 3` 必然触发。
- 修法：先把 Same 片段在句读处切开（`。；！？，：` 之后切），再划 region。
  判定按单个分句算；片段数阈值也按单个分句统计。

### B4 独立列表在花脸稿里全部编成「1.」
- 复现（P10）：三项独立列表改一项，重新解析后三项的 `number` 都是 1。
- 根因：`serialize` 在块之间一律用 `\n\n` 拼接。每个 `- 项` 都被空行隔开，解析器把每一项都当成新的一组。
  列表的起始编号（`3.` 开头）也丢了。
- 修法：
  - 相邻列表项用单换行拼接，和表格行同一处理；
  - 首项按原起始号写 `N.`；
  - 删除的列表项不要插在两项中间把组打断：可以折进前一项末尾，或者单独约定一种「不计号的列表行」。

### B5 段内列表的圈号顺移被标成改动（违反需求 6）
- 复现（P3）：段内列表在最前插一项，后面的 `②③` 全部标成 `~②~[③]`。
- 根因：解析器把段内列表的圈号（`render_list_number(numbering.list1, …)`）直接拼进了 Paragraph 文本，
  视觉模型原样拿来比较。
- 修法：模型层把生成的编号和正文分开（在 parse 里保留每个 ParagraphPart 的编号边界，
  或者在 model 里识别并剥掉），比较时不含编号，序列化时由导出器按新版重新生成。
- 另外：`DocumentModel::from_markdown` 固定用默认编号样式，而导出用的是用户设置。
  用户改过列表编号样式时，花脸稿的编号会和定稿不一致。要么模型不含编号（推荐，顺手解决），
  要么把 `numbering` 传进来。

### B6 加粗被标成新增（违反需求 7）
- 复现（P1）：`请按时报送材料。` → `请**按时**报送材料。`，结果是 `请[**按时**]报送材料。`。
- 根因：`MarkdownBlock::Paragraph` 的文本还带着 `**`，模型注释里说「Markdown 标记已剥离」，实际并没有。
- 修法：比较键用 `inline_segments` 之后的可见文字。序列化时把标记投影回带样式的片段：
  加粗边界和增删边界相交时，要保证 `**` 不被哨兵切断。

### B7 研究报告的区段标记被丢掉
- 复现（P6）：`<!-- [摘要] -->`、`<!-- [参考文献] -->` 在花脸稿 Markdown 里消失了。
  mdx 靠这些标记分区，研究报告的花脸稿会丢掉摘要 / 参考文献的版式。
  `<div …>` 块、`<!-- [目录] -->`、`<!-- [附录] -->` 也一样。
- 根因：`emit_block` 对 `MarkdownBlock::Html` 返回空。
- 修法：新版的 Html 块原样写回；删除的 Html 块不写。

### B8 标题折行后，第二行的标记丢了
- 复现（P9）：新增文字跨过标题折行点时，第二行里的新增部分变成了 Same。
- 根因：`redline_slice_lines` 只按字符把哨兵分到各行，没有在行尾补闭合、行首补开启。
- 修法：切行时记录当前的标注状态。行尾有未闭合的标注就补闭合哨兵，下一行开头补对应的开启哨兵。
  补一个跨行标注的单测。

### B9 公文要素：没有接上，接上后也不对
- `DocumentModel::from_inputs` 是 `#[allow(dead_code)]`，`redline::build` 只传 Markdown，**要素标注实际没有交付**。
  其实旧版的 `DraftInput` 在 `export_redline` 里拿得到（`get_manuscript_version(id, base).snapshot`）。
- 更要紧的是：一旦接上，`serialize` 会把要素写成 `【主送机关】……` 这种正文行，印在正文里。
  方案要求的是**在版头 / 版记原位标注**。
- 处理：这一项放到第 ② 期和预览一起做（版头、版记的绘制函数都要接受标注）。本期先删掉 `【】` 序列化这条路，免得有人误接。

## 次要项（顺手修）
- **移动注记**用了 `mark_added`（蓝框），读者会以为这句注记是新写进正文的字；而且它是用 `\n` 拼进同一段的。
  应改成不带增删含义的小注样式（比如小号楷体括注），或者至少不用新增框。
- **新增标题的编号**没有进框（导出侧的编号前缀在 `marked_runs` 之外）。方案要求新增标题整体（含编号）加框。
- **删除的标题**降成了普通正文段（首行缩进、仿宋）。方案只要求不显示编号；最好保留标题字体，只去掉编号。
- **研究报告的后处理风险**（本机没有 tectonic / mdx，未实测）：
  - `inject_tex_redline` 在 mdx 生成的 TeX 上按哨兵包宏。哨兵如果跨过 `\textbf{…}` 这类命令的花括号，会产生不配对的 `{}`，导致编译失败；
  - 研究报告里的 `\GwAdd` 没有按标点切块，长插入会冲出版心。
  - 请至少补单测：哨兵跨加粗、长插入。Windows 上实编译一份含加粗改动和长插入的研究报告花脸稿。
- `diff::merged_spans` 加了 `#[allow(dead_code)]` 却还留着。确认没用就删掉，连同它的单测。

## 验收标准
1. 文末探针测试全部改为**断言**并通过（期望值写在每条的注释里）。
2. 新增不变式测试：随机或成组的中文段落对，「去 Added == 旧文」「去 Deleted == 新文」。
3. fmt / clippy / test 全绿。

## 探针测试（放进 `src/visual_diff/mod.rs` 的测试模块，按注释改成断言）
期望：
- P1：加粗前后无任何标记。
- P2：两侧不变式成立。
- P3：只有「新增工作」一项加框，圈号无标记。
- P4 / P4b：只标改动的词，不整段替换。
- P5：插入 / 删除标题，其余标题无标记，删除的标题无编号。
- P6：摘要 / 参考文献标记保留。
- P7：拆分 + 改字只标「改」，无移动注记。
- P8：只有第三段带移动注记。
- P9：两行都带 Added。
- P10：编号是 1、2、3。
- P11：只有新段加框，无移动注记。

```rust
#[cfg(test)]
mod review_probes {
    use super::*;
    use crate::export::{REDLINE_ADD_CLOSE, REDLINE_ADD_OPEN, REDLINE_DEL_CLOSE, REDLINE_DEL_OPEN};

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
                    if !in_add { old.push(ch) }
                    if !in_del { new.push(ch) }
                }
            }
        }
        (old, new)
    }

    #[test]
    fn p1_bold_only() {
        let out = run("请按时报送材料。", "请**按时**报送材料。");
        eprintln!("P1 bold: {out:?}");
    }

    #[test]
    fn p2_invariant() {
        let cases = [
            ("请市教育局于八月前报送有关材料。", "请省教育厅于九月前报送相关材料。"),
            ("各单位要高度重视，认真组织，确保按时完成。", "各部门要充分重视，精心组织，确保如期完成。"),
            ("会议定于下周一上午召开。", "会议定于下周三下午在三楼召开。"),
        ];
        for (old, new) in cases {
            let out = run(old, new);
            let (o, n) = sides(&out);
            eprintln!("P2 {out}\n   old={o}\n   new={n}");
            assert_eq!(n, new, "新版文字被归并吃掉/多出");
            assert_eq!(o, old, "旧版文字被归并吃掉/多出");
        }
    }

    #[test]
    fn p3_inline_list_renumber() {
        let old = "工作要求如下：\n1. 甲项工作\n2. 乙项工作\n3. 丙项工作";
        let new = "工作要求如下：\n1. 新增工作\n2. 甲项工作\n3. 乙项工作\n4. 丙项工作";
        eprintln!("P3 {:?}", run(old, new));
    }

    #[test]
    fn p4_multi_sentence_paragraph() {
        let old = "第一句保持不变。第二句也不变动。第三句这里写甲，后面写乙，最后写丙。第四句不变。";
        let new = "第一句保持不变。第二句也不变动。第三句这里写丁，后面写戊，最后写己。第四句不变。";
        eprintln!("P4 {:?}", run(old, new));
        let old = "关于报送材料的通知已经收悉。请各单位于月底前报送。逾期不报的视为放弃。";
        let new = "关于报送材料的通知已经收到。请各部门于月底前报送。逾期未报的视为放弃。";
        eprintln!("P4b {}", run(old, new));
    }

    #[test]
    fn p5_heading_insert() {
        let old = "## 工作目标\n\n目标段。\n\n## 保障措施\n\n保障段。";
        let new = "## 工作目标\n\n目标段。\n\n## 职责分工\n\n分工段。\n\n## 保障措施\n\n保障段。";
        eprintln!("P5 {}", run(old, new));
        let old2 = "## 工作目标\n\n目标段。\n\n## 职责分工\n\n分工段。\n\n## 保障措施\n\n保障段。";
        eprintln!("P5del {}", run(old2, "## 工作目标\n\n目标段。\n\n## 保障措施\n\n保障段。"));
    }

    #[test]
    fn p6_research_markers() {
        let text = "<!-- [摘要] -->\n\n摘要内容。\n\n## 引言\n\n正文。\n\n<!-- [参考文献] -->\n\n[1] 文献。";
        let changed = text.replace("正文。", "正文改。");
        eprintln!("P6 {:?}", run(text, &changed));
    }

    #[test]
    fn p7_split_plus_edit() {
        let old = "A 第一句。A 第二句。\n\nB 段落。";
        let new = "A 第一句。\n\nA 第二句。\n\nB 段落改。";
        eprintln!("P7 {}", run(old, new));
    }

    #[test]
    fn p8_move() {
        let old = "第一段内容比较长一些用于识别。\n\n第二段内容也比较长用于识别移动。\n\n第三段内容同样足够长可以识别。";
        let new = "第三段内容同样足够长可以识别。\n\n第一段内容比较长一些用于识别。\n\n第二段内容也比较长用于识别移动。";
        eprintln!("P8 {}", run(old, new));
    }

    #[test]
    fn p9_title_wrap_marks() {
        use crate::export::{mark_added, redline_slice_lines};
        let title = format!("关于进一步{}的通知", mark_added("加强和改进全省教育系统安全生产工作"));
        let plain = crate::export::strip_redline(&title);
        let mid = plain.chars().count() / 2;
        let l1: String = plain.chars().take(mid).collect();
        let l2: String = plain.chars().skip(mid).collect();
        let lines = redline_slice_lines(&title, &[l1, l2]);
        for line in &lines {
            let chunks = crate::export::redline_chunks(line);
            eprintln!("P9 line: {:?}", chunks.iter().map(|c| (c.kind, c.text.clone())).collect::<Vec<_>>());
        }
    }

    #[test]
    fn p10_standalone_list_numbering() {
        let old = "要求如下。\n\n1. 甲项工作\n2. 乙项工作\n3. 丙项工作";
        let new = "要求如下。\n\n1. 甲项工作\n2. 乙项工作改\n3. 丙项工作";
        let overlay = diff_documents(&DocumentModel::from_markdown(old), &DocumentModel::from_markdown(new));
        let md = to_marked_markdown(&overlay);
        eprintln!("P10 md={md:?}");
        for b in crate::export::parse_markdown(&md) {
            if let crate::export::MarkdownBlock::OrderedListItem { number, text } = b {
                eprintln!("P10 item #{number}: {}", crate::export::strip_redline(&text));
            }
        }
    }

    #[test]
    fn p11_insert_paragraph_first() {
        let old = "第一段内容比较长一些用于识别。\n\n第二段内容也比较长用于识别移动。\n\n第三段内容同样足够长可以识别。";
        let new = format!("新写的一段开头。\n\n{old}");
        eprintln!("P11 {:?}", run(old, &new));
    }
}
```

---

# 修复记录（feat/visual-diff-engine 分支，紧随 5ba65f9）

B1-B9 全部修复，探针 P1-P11 已转为断言（`src/visual_diff/mod.rs` 的
`review_probes` 模块）并全数通过；不变式测试扩充为五组用例（`p2_invariant`）。
fmt / clippy / test 全绿（1131 通过）。

- **B1**：`resolve_run` 内先对段落做分数最大化 LCS（相似度 ≥ 0.6、公共字符
  ≥ 4、排除包含关系才算锚定）——LCS 上的段是没动 / 就地改写；LCS 之外
  相似度 ≥ 0.8 的删 / 增配对才算移动。插段、拆分合并不再触发连锁注记。
- **B2**：`absorb_gaps` 改为夹缝文字两侧各放一份（前片段末尾追加、后片段
  开头追加），「去 Added == 旧文、去 Deleted == 新文」逐字成立。
- **B3**：归并前先把片段在句读处切开（含夹缝复制出的文字），分句区与
  片段数阈值都按切开后的分句统计，四句段只改第三句不再整段替换。
- **B4**：独立列表项按原起始号写 `N. `、相邻项单换行拼接；删除的列表项
  折进前一项同一行，不打断组。
- **B5**：`export::parse` 在 `LocatedBlock` 上暴露段内列表生成编号的字符
  范围（`generated_prefixes`），模型构建时剥掉编号比较（编号样式不一致的
  问题随编号剥离一并消失）；序列化按记录的项起点把各项拆回列表行，圈号
  由导出器按设置重新生成。
- **B6**：模型中文本块以行内标记剥离后的纯文本参与比较；序列化
  `styled_marked` 把标注投影回带样式的原文（`**` 成对归属，哨兵不切断
  加粗边界），Deleted 片段以纯文本包哨兵（旧文字不在新版原文里）。
- **B7**：`Html` 块原样写回（摘要 / 参考文献 / 目录 / `<div>` 保留）；
  序号表与居中 / 居右标记行除外——那两类由表格、对齐行的序列化负责。
- **B8**：`redline_slice_lines` 切行后逐行平衡标注状态：行尾未闭合补
  闭合哨兵、下一行行首补开启哨兵（P9 断言两行各自成对）。
- **B9**：要素序列化的 `【】` 正文行出口删除（要素在版头 / 版记原位标注
  是第 ② 期的事）；`DocumentModel::from_inputs` 与要素比较规则保留并
  有单测。

次要项：移动注记改为不带增删含义的普通括注行（无蓝框）；新增标题的
编号并进框（docx 前缀 run 同边框、LaTeX `marked_heading_tex` 编号进首个
`\GwAdd` 块）；研究报告后处理加了两道保险——`\GwAdd` 按标点切块、
`wrap_tex_macro` 花括号平衡（哨兵跨 `\textbf{` 也能编译），均有单测；
`diff::merged_spans` 及其单测已删除（唯一调用方是旧花脸稿）。

遗留（ acknowledged，不在本期）：删除的标题降成普通文字行（保留标题
字体需要导出侧支持「无编号标题」，随第 ② 期预览一起做）；Windows 上
实编译含加粗改动 / 长插入的研究报告花脸稿依赖本机 Tectonic，CI 环境
未覆盖，以单测锁定后处理行为。
