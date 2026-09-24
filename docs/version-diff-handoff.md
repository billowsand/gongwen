# 版本变更与花脸稿改造：交接说明

> 给接手的开发者（含本地 Claude）：先读 `docs/version-diff-redesign.md`（方案与需求结论），
> 再读本文（做到哪了、和方案有什么出入、下一步怎么做、有哪些坑）。
> 本文随进度更新；每完成一期，把「当前进度」和「下一步」改掉。

## 当前进度

| 期 | 内容 | 状态 |
|---|---|---|
| ① | 视觉 diff 引擎 + PDF / Word 花脸稿导出 | **已完成**，已合入 `main` |
| ② | 左 Markdown diff / 右花脸稿预览的界面、标记绘制、联动、打印预览 | 未开始（**下一步**） |
| ③ | 可编辑 diff + 逐块还原 | 未开始 |
| ④ | 版本时间轴合并浏览界面；稿件管理对照窗改用同一视图；花脸稿自动归档 | 未开始 |
| ⑤ | AI 修订建议并入为待采纳块 | 未开始 |

## 第 ① 期交付了什么

### 代码地图
- `src/visual_diff/`：视觉 diff 引擎，全是纯函数，不碰存储与 UI。
  - `model.rs`：把一个版本展开成「视觉块」序列（复用 `export::parse`）。块文本是**剥掉行内
    标记后的可见文字**，同时保留原文 `raw`（带 `**`）和段内列表各项的起点 `inline_items`。
    段内列表的圈号 ①② 是生成的，模型里剥掉，所以编号顺移不产生标注。
  - `tokenize.rs`：jieba 分词；日期、数字 + 单位、文号、公式作为一个整体 token。
  - `compare.rs`：比较规则，与方案 4.2 一一对应。要点：
    - 按章节对齐，章节内做文字流比较，段落拆分 / 合并不产生标记；
    - 段落先做 LCS 锚定，锚定之外相似度 ≥ 0.8 的才算移动，所以在段首插段不会误标移动；
    - 归并（`apply_merge_rules`）：先抵消相邻的「删 X、增 X」；再吸收夹缝，按区域整体改写，
      只吸收同时含删和增的区域；再在句读处切开；最后做整句替换，共有的首尾标点不删。
  - `overlay.rs`：产物 `RedlineOverlay`，即附着在新版块上的 `(文字, Same/Deleted/Added)` 片段、
    整删块和移动注记。
  - `serialize.rs`：`RedlineOverlay` → 带哨兵的 Markdown。标注逐字投影回原文（`project`）：
    加粗按每个字的状态重新开合，并且**只在加粗闭合时开合哨兵**，`**` 永远不跨哨兵。
  - `postprocess.rs`：研究报告专用。mdx 转换出的 TeX / DOCX 落地后，再把哨兵换成宏或 run 格式。
- `src/redline.rs`：花脸稿入口，`build(old, new)` 和 `export_files`。
- 导出侧的标记：
  - `src/export/latex/text.rs`：`body_text_to_tex`、`redline_macro`、`marked_tex_escape`、`marked_heading_tex`；
  - `src/export/table.rs`：表格单元格；
  - `src/export/docx/runs.rs`：`apply_redline`、`marked_runs`；
  - `gonghan-gwa.cls`：「花脸稿标记」一节。

### 标记样式（三端必须一致）
- 删除：红色 `#C00000` 直删除线穿过字身，删除的文字也是红色。
- 新增：蓝色 `#1F4E9E`、0.5pt 方框，框内文字保持正文色。
- 新增框**能随正文断行**：跨行时在行尾开口，下一行接着画，左右竖边只在整段新增的首尾各一笔。
  预览（第 ② 期）画框也按这个规则。

### 测试（不许删，只许加）
- `src/visual_diff/mod.rs` 的 `review_probes`：两轮审查中实际复现过的问题，每条都是断言。
- `generated_edits_always_restore_both_sides`：生成式不变式。400 组真实公文句子随机增删改，
  逐字核对「去新增 == 旧版、去删除 == 新版」。**改归并规则或序列化后必须跑它**。
  它曾抓到过吃字和重复字的问题。
- `src/redline.rs` 的一致性测试：同一份花脸稿 Markdown，DOCX run 序列与 TeX 片段序列的
  `(文字, 类型)` 必须相同。
- `src/export/latex.rs` 的 `redline_styles_wrap_outside_the_mark_macros`：锁住「样式包在宏外面」这个写法。

## 与方案的出入（有意为之，接手时别当 bug 改回去）
1. **方案 4.3 说三个消费方直接读 `RedlineOverlay`，实际导出走的是「overlay → 带哨兵的 Markdown
   → 原有导出器」**。好处是花脸稿与定稿走同一条排版链路，版式天然一致。
   第 ② 期的预览**建议也吃同一份带哨兵的 Markdown**（见下文），这样「预览 = 导出」由构造保证。
2. **公文要素（主送、抄送、成文日期等）的就地标注没有接上**。`DocumentModel::from_inputs` 已写好，
   但标了 `#[allow(dead_code)]`；序列化对 `VisualBlock::Element` 返回空。
   要素标注要在版头 / 版记的绘制处做（预览和导出都要），归在第 ② 期。
3. 研究报告的花脸稿是在 mdx 产物上后处理：给哨兵换宏，花括号不平衡时补齐。能编译，
   但如果标注跨过 `\textbf{…}` 这类命令，标注和加粗的范围可能有偏差。

## 已知的坑
- **xeCJKfntef 的标注宏（`\GwDel` / `\GwAdd`）里面，字体切换只作用到第一个字。**
  加粗、括号楷体必须包在宏**外面**。所以一段新增按样式切开，分成
  `\GwAddOpen` / `\GwAddMid` / `\GwAddClose` 三种；括号小一号字之前先 `\GwBoxFreeze` 定死框高。
  以后往花脸稿里加任何样式，都照这个规则来。
- **删除线的颜色要写在平铺单元里**（`\GwStrikeUnit`），外层 `\textcolor` 只染文字。
- 导出器先按哨兵切块，再在块内配对 `**`。所以带哨兵的 Markdown 里 `**` 绝不能跨哨兵（`serialize::MarkWriter` 保证了这一点）。
- 框的上下边位置（`\GwBoxTop` 0.96em、`\GwBoxBottom` 0.24em）是用思源宋体调的，还**没有**在
  仿宋 / 小标宋上实测。如果视觉上偏紧或偏松，只调 `.cls` 和 `postprocess.rs` 导言区这两处。
- 可以不用内置 Tectonic 验证 TeX：
  1. 装 XeLaTeX（Debian 系：`texlive-xetex texlive-lang-chinese texlive-latex-extra texlive-plain-generic fonts-noto-cjk`）；
  2. 写个临时测试，调 `redline::build` + `export::write_tex_for_kind` 把 `.tex` 写到临时目录；
  3. 把 `.cls` 里的字体名替换成本机有的字体后执行 `xelatex`。
  - 空 `DraftInput` 会让 `\makeletter` 报 “There's no line here to end”，这是版记要素为空导致的，与花脸稿无关。

## 下一步：第 ② 期怎么做（建议）

目标：起草页的「版本对照」模式（`PreviewMode::VersionDiff`，入口 `src/draft_page/versions.rs` 的
`version_diff_mode_ui`）改成左右分栏：左边是 Markdown 统一 diff（本期只读），右边是花脸稿预览。

1. **右侧预览复用 `preview::official_preview`，吃 `redline::build(old, new).markdown`**：
   - 目前 `export::text::inline_atoms` 会跳过哨兵，所以预览能排出文字，但画不出标记。
     要在预览构建 `LayoutJob` 的地方按 `export::redline_chunks` 切块，给删除块加红色删除线和红字，
     给新增块画蓝框（按行分段画矩形，行尾开口、首尾竖边）。
     正文、标题、列表、表格单元格都要覆盖，入口在 `src/preview/render.rs` 和 `layout.rs`。
   - 加一个测试：预览片段序列与 DOCX / TeX 的 `(文字, 类型)` 序列一致（仿照 `redline.rs` 的一致性测试）。
2. **左侧统一 diff**：数据用现有的 `diff::manuscript_diff`（代码层 diff），渲染从 `diff_view.rs` 的
   分栏改成统一视图：删除行红底在上、新增行绿底在下，行号槽画色条，未改动区折叠。
3. **联动**：`RedlineOverlay` 里每个 `VisualBlock::Parsed` 带着新版源码的 `range`。
   建一张「预览块 → 源码范围 → 代码 diff 变更块」的映射，用来做点击互跳、悬停高亮，
   以及统一的上一处 / 下一处（F7 / Shift+F7）。
   注意：带哨兵的 Markdown 的字节位置和用户源码对不上，**不能**直接用预览回报的 range，要经 overlay 转换。
4. **公文要素就地标注**：接上 `DocumentModel::from_inputs`。旧版的 `DraftInput` 从
   `store.get_manuscript_version(id, base).snapshot` 取。版头 / 版记的绘制（`preview/header.rs`、`tail.rs`，
   以及导出侧对应函数）按字段整体删旧插新。
5. **打印预览按钮**：后台线程跑 `redline::export_files`（只出 PDF）→ 用内置 `pdf_viewer` 打开。

第 ③–⑤ 期见方案第六节。第 ③ 期的最大风险（在 egui `TextEdit` 上做可编辑的统一 diff）见方案第五节。

## 工作约定（在 AGENTS.md 之外补充）
- 改视觉 diff 必须保持两侧不变式成立。**不许为了让测试通过而删测试或放宽断言**，行为确实变了，就在测试注释里写清楚为什么。
- 涉及 TeX 版式的改动要实际编译看一眼（方法见上文），光单测不够。
- 每完成一期，更新本文的「当前进度」和「下一步」。
