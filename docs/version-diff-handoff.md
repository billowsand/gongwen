# 版本变更与花脸稿改造：交接说明

> 给接手的开发者（含本地 Claude）：先读 `docs/version-diff-redesign.md`（方案与需求结论），
> 再读本文（做到哪了、和方案有什么出入、下一步怎么做、有哪些坑）。
> 本文随进度更新；每完成一期，把「当前进度」和「下一步」改掉。

## 当前进度

| 期 | 内容 | 状态 |
|---|---|---|
| ① | 视觉 diff 引擎 + PDF / Word 花脸稿导出 | **已完成**，已合入 `main` |
| ② | 左 Markdown diff / 右花脸稿预览的界面、标记绘制、联动、打印预览 | **大部分完成**，已合入 `main`；剩「公文要素就地标注」（见下文，可与第 ③ 期并行） |
| ③ | 可编辑 diff + 逐块还原 | **已完成**（见「第 ③ 期交付了什么」）；已做一轮实测并修正（见「第 ③ 期测试后的修正」），人眼验收待做 |
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

## 第 ② 期交付了什么

### 代码地图
- `src/preview/marks.rs`：预览里的花脸稿标记。
  - 删除：红字 + 自己画的 0.6pt 删除线（`LineMarks::paint_strike`），高度取被删汉字墨迹的
    上下沿中点。**不能用 egui 的 `strikethrough`**：它画在行盒中线上，28 磅固定行距下
    字挤在行盒上部，线会落到字脚、看着像下划线（用户实测报过）。
  - 新增：排版任务里打一个**不画出来的记号**（`underline` = 线宽 0 的新增蓝），画完字后
    `AddedBoxes` 按字形位置逐行画框：上下边每行都画，左右竖边只在整段新增的首尾各一笔，
    跨行处开口；框高按行内最大字号 0.96em / 0.24em 定死（对应 `\GwBoxFreeze`）。
    进出新增块时在 section 上留 `leading_space` 空隙给竖边。
  - 整块图形（公式）用 `paint_block_mark`：删除画一道中线、新增套完整框。
- 标记接入点：`layout.rs` 的 `append_inline`（正文 / 列表 / 对齐行 / 紧缩标题正文）、
  `line_block_runs`（标题、附件名，经 `marks::plain_keep_marks` 保留哨兵）、表格单元格、
  `render.rs` 的 `heading_job`（新增标题连编号加框，与 DOCX / TeX 一致）、
  `red.rs`（红头呈批件分页：`RedFlowSegment.mark` 跟着字跨页）、
  `math_flow.rs`（研究报告公式混排：标记状态跨公式片段接力，公式整体标注）。
- `src/visual_diff/serialize.rs` 的 `to_marked_markdown_with_spans`：序列化同时产出
  `MarkedSpan`（花脸稿里的字节范围 ↔ 新版 / 旧版源码范围）。`RedlineDoc.spans` 带着它。
- `src/version_link.rs`：`ChangeLinks`，代码 diff 变更块 ↔ 花脸稿块的换算（纯函数）。
  纯删除的变更按旧版行号换成旧版字节范围，去匹配整删旧块的来历。
- `src/diff_view.rs` 的 `unified_body_ui`：左栏统一视图（只读）。删除行红底在上、新增行
  绿底在下，双列行号 + 色条 + ±号，词级差异加深底色；未改动区折叠、变更块上下各留 1 行。
  `diff::ContextBlock` 为此多了 `old_line`。
- `src/draft_page/version_diff.rs`：起草页「版本对照」模式本体（从 `versions.rs` 挪出来）。
  工具栏（基准、共 N 处、上一处 / 下一处、折叠开关、打印预览、导出花脸稿、回退），
  左栏要素变化卡片 + 统一 diff，右栏 `official_preview` 吃花脸稿 Markdown。
  `sync_draft_diff` 同一次重算代码 diff、花脸稿与联动表，按内容哈希缓存。
- 打印预览：`DocJob::RedlinePrintPreview`，只出 PDF、写系统临时目录
  `gongwen-redline-preview/`，编完用内置 `pdf_viewer` 打开，不在导出目录留文件。
- 预览悬停回报：`preview::hovered_source(ctx)`（`layout.rs` 里记在 egui 临时数据，
  `official_preview` 开画前清空）。为一个只读结果给整条绘制链路加出参不值得。

### 联动规则
- 左栏单击变更 → 聚焦 + 右栏滚到对应块并常亮；双击 → 切回 Markdown 源码定位
  （纯删除落在紧随其后那一块的起点）。单击未改动的上下文行 → 右栏滚到那一段。
- 右栏单击带标注的块 → 左栏聚焦并滚到那处变更；点未改动的块只在预览里标亮。
- 悬停任一侧，另一侧淡色高亮（左栏描边；右栏借预览的锚点底色）。右栏只有一个锚点，
  优先级：左栏悬停 > 点过的未改动块 > 当前焦点变更。
- F7 / Shift+F7 与工具栏上下箭头走同一个 `DiffViewState::step`，首尾循环。

### 测试（不许删，只许加）
- `redline.rs` 一致性测试扩成三方：预览（`marks::body_sequence`）= DOCX = TeX。
- `preview::marks::tests`：跨行新增只有两条竖边、删除红字带删除线、新增标题编号在框里。
- `version_link::tests`：改段 / 整删段 / 新增标题的双向换算。
- `draft_page::version_diff::tests`：内存库 + 真 egui 上下文画整页，两栏都出字、
  每处变更在预览里都有落点、F7 首尾循环。

## 第 ② 期还没做的：公文要素就地标注（下一步）

方案需求第 9 条：版头、版记里的主送、抄送、成文日期、签发人、文号等改动，在版面原位
画删除线 + 方框。现在左栏顶部有「要素变化」卡片（字段级，沿用 `draft_changes`），
但右栏预览与导出的纸面上**还没有**要素标注。

没做的原因：纸面上印的不是原始字段值。主送 / 抄送要经 `UnitDisplay` 换全称或简称、
按顿号拆开，联合发文的机关标志与落款按列排，成文日期换成汉字，密级拼保密期限……
往 `DraftInput` 字段里塞哨兵会被这些显示变换打碎。必须在**每个字段的绘制处**各自接：

1. 算出旧版要素：`store.get_manuscript_version(id, base).snapshot`（`sync_draft_diff`
   里已经取了旧快照，放进 `RedlineView` 即可）。
2. 定义一个「要素标注」结构，按字段给出 `(旧显示值, 新显示值)`；显示值要用与绘制处
   **同一个**函数算（`UnitDisplay` 的各个方法、`chinese_date` 等），不能用
   `DocumentModel::from_inputs` 里那套原始拼接——那只适合做比较。
3. 预览：`preview/header.rs`（红头、文号、密级）、`preview/tail.rs`（主送、落款、日期、
   版记）、`preview/red.rs`（红头呈批件的承办区等）按字段查标注，有就排
   「删旧（红删除线）+ 插新（蓝框）」，可以复用 `marks::append_marked_text`。
4. 导出：DOCX 版头 / 版记段落用 `marked_runs`；TeX 侧 `official.rs` 的要素命令参数
   用 `marked_tex_escape`。注意 `.cls` 里有些要素是按宽度算版式的（红头字距、落款
   右对齐），标注后宽度变了，要实测。
5. 加一致性测试：同一组要素变化，预览 / DOCX / TeX 三边的 `(文字, 类型)` 一致。

## 第 ③ 期交付了什么

### 代码地图
- `src/draft_page/diff_gaps.rs`（spike）：在 `TextEdit` 的 galley 里开空隙。`open_gaps` 把某条
  源码行起的 `PlacedRow.pos.y` 下移、扩 `rect` / `mesh_bounds`；`gap_spans` 给出空隙位置；
  `line_first_rows` 做源码行 → galley 行。验证结论见实施指南里的 spike 表。
- `src/draft_page/diff_hunks.rs`：数据层（纯函数）。`hunks` 把 `BodyDiff` 的逐行变更聚成
  变更块（夹在两段未改动内容之间的一串连续变更），给出旧行（放空隙里）、新行（绿底）、
  空隙开在哪一行；`revert` 逐块还原——改写整段换回旧文本，纯新增删掉连同分隔空行，
  纯删除按旧版原样补回分隔空行。
- `src/draft_page/diff_editor.rs`：左栏编辑器本体。源码模式同一套高亮排版；布局器里开空隙，
  旧行用 `highlight::highlight` 排（与编辑器同字号），真正删掉的字加深红底；新行绿底经
  占位 `Shape::Noop` 压在字下面，改掉的字加深；行号槽写新 / 旧行号与增删色条；悬停或聚焦的
  变更块右上角出「还原」按钮。`replace_with_undo` 在还原前后各记一个撤销点。
- `src/draft_page/version_diff.rs`：
  - 可编辑时左栏换成 `diff_editor`，导航单位是变更块；已发布 / 归档稿件只读，仍用
    `diff_view::unified_body_ui`，导航单位是逐行变更（`DraftDiffState::unit_*` 两套换算）。
  - `sync_draft_diff` 重写：基准快照与备注只在换稿件 / 换基准版时读一次库（`Baseline`）；
    代码 diff 与变更块在正文哈希或 `DraftInput`（新加了 `PartialEq`）变化时同步重算；
    花脸稿第一次同步算，之后后台线程 + 150ms 防抖（正文每变一次重新计时），
    结果经新的 `WorkerResult::Redline { key, hash, doc }` 回来，`accept_redline` 只收哈希对得上
    的；导出 / 打印预览若右栏还没追上，就地同步重算一份用。
- `src/visual_diff/compare.rs`：长稿段落锚定提速（单独一个提交，见「与方案的出入」第 8 条）。

### 联动（在第 ② 期规则上）
- 编辑器光标**换了行**且落在某个变更块上，它就成为焦点，右栏跟着滚动常亮。只认「换了行」：
  F7 发出跳转的那一帧光标还在旧行，不能把焦点拉回去。
- F7 / Shift+F7 把光标移到变更块首行并滚过去（`editor_jump`，下一帧在滚动区里执行）。
- 右栏点带标注的块 → 焦点 + 光标跳到那一块。
- 还原 → 光标落在被还原块的起点，右栏滚过去；Ctrl+Z 一次撤回。

### 测试（不许删，只许加）
- `diff_gaps::tests`：spike 验证清单 13 条。
- `diff_hunks::tests`：聚块、空隙位置、逐块还原只去掉那一块、从后往前逐块还原回旧文本
  （8 组增删改组合）、还原后光标位置。
- `diff_editor::tests`：按范围切 section 铺底色、字级片段的字节 / 字符范围。
- `version_diff::tests`：旧行画在两条编辑器行之间、F7 / Shift+F7 移动焦点与光标并首尾循环、
  点空隙下方那行光标落在那行且焦点跟过去、还原后文本正确且 Ctrl+Z 撤回、改字后花脸稿
  防抖进后台且过期结果被丢掉。`long_draft_typing_probe`（`#[ignore]`）是人工跑的性能探针。
- `visual_diff::review_probes::a_long_headingless_draft_gives_the_same_redline_on_both_anchor_paths`：
  同一份长稿强制走两条锚定路径，花脸稿逐字节相同。

### 性能实测（release，2000 段、100 个变更块，`long_draft_typing_probe`）
- 进对照模式首帧：255ms（提速前 13.9s，几乎全在视觉 diff 引擎）。
- 打字帧：中位 22ms；空闲帧：中位 18ms。其中左栏编辑器约 2.7ms，**右栏公文预览约 15ms**——
  预览每帧整篇重排，是 `official_preview` 的既有行为（普通「公文预览」模式同样如此），
  不是本期引入的；长稿要再快得按可视页裁剪，见「下一步」。

## 与方案的出入（有意为之，接手时别当 bug 改回去）
1. **方案 4.3 说三个消费方直接读 `RedlineOverlay`，实际导出走的是「overlay → 带哨兵的 Markdown
   → 原有导出器」**。好处是花脸稿与定稿走同一条排版链路，版式天然一致。
   第 ② 期的预览**建议也吃同一份带哨兵的 Markdown**（见下文），这样「预览 = 导出」由构造保证。
2. **公文要素（主送、抄送、成文日期等）的就地标注没有接上**。`DocumentModel::from_inputs` 已写好，
   但标了 `#[allow(dead_code)]`；序列化对 `VisualBlock::Element` 返回空。
   要素标注要在版头 / 版记的绘制处做（预览和导出都要），归在第 ② 期。
3. 研究报告的花脸稿是在 mdx 产物上后处理（`visual_diff::postprocess`），不是像公文那样在
   导出器里接标记。TeX 侧（第 ③ 期测试后重写）：标记内容按顶层切段，样式命令包在宏外、
   公式 / 引用整体装盒另画、独立公式整块标注，见「第 ③ 期测试后的修正」第 2 条；只有
   花括号不平衡（哨兵跨过命令的括号，序列化保证不会发生）时才退回整段一个宏。
   Word 侧：标记未收尾时中间的 run 整个追加删除线 / 边框。研究报告 Word 里的公式本来就是
   `$…$` 源码文字（mdx 的既有行为），花脸稿里照样标。
4. （第 ② 期）**视觉 diff 仍在界面线程同步算**，按内容哈希缓存。本期左栏只读、改字要
   切回 Markdown 模式，停在对照模式时内容不变，缓存一直命中；第 ③ 期边改边看时再按
   方案第五节挪到后台线程 + 防抖。**第 ③ 期已完成**：首帧同步算，之后后台线程 + 150ms 防抖。
5. （第 ② 期）预览里的新增框不像 TeX 那样把字往里挤 1.5pt：竖边画在进出新增块时
   留的 `leading_space` 缝里。研究报告公式混排的断行按原子宽度预算，那条路径**不留缝**
   （否则算好的断行会溢出），竖边会贴字。
6. （第 ② 期）稿件管理的对照窗、AI 工作台的比对仍用旧的分栏视图
   （`diff_view::manuscript_diff_ui`），但去掉了已无调用方的「跳源码 / 导出花脸稿」入口；
   第 ④ 期统一换成新视图。研究报告现在也能在对照页导出花脸稿（第 ① 期已覆盖）。
7. （第 ③ 期）**可编辑时未改动区不折叠**。方案说「未改动区可以折叠」，但折叠要把
   编辑器里的行藏起来，与 `TextEdit` 的光标 / 选区 / 撤销都冲突，本期没做；只读视图仍折叠。
8. （第 ③ 期）视觉 diff 引擎的段落锚定对大 run（旧段数 × 新段数 > 25 万）改成「先锚一字不差
   的段、夹缝里再做加权 LCS」。上限以内结果逐字节不变；上限以外理论上可能与全局最优锚定
   不同（例如一个完全相同的段与一串高相似段交叉时），测试里的长稿两条路径结果相同。
9. （第 ③ 期）逐块还原的单位是「变更块」而不是逐行变更；F7 在可编辑时也按变更块跳。

## 已知的坑
- **xeCJKfntef 的标注宏（`\GwDel` / `\GwAdd`）里面，字体切换只作用到第一个字。**
  加粗、括号楷体必须包在宏**外面**。所以一段新增按样式切开，分成
  `\GwAddOpen` / `\GwAddMid` / `\GwAddClose` 三种；括号小一号字之前先 `\GwBoxFreeze` 定死框高。
  以后往花脸稿里加任何样式，都照这个规则来。
- **删除线的颜色要写在平铺单元里**（`\GwStrikeUnit`）。
- **xeCJKfntef 宏里的字，颜色 special 染不上**：它（底下是 ulem）把字逐个装盒再放，外层
  `\textcolor`、`textformat=\color{…}` 都只染到它跳过的标点，汉字照样是黑的。删掉的字要
  红色，只能在宏外面切字体颜色：`\addfontfeatures{Color=C00000}\addCJKfontfeatures{Color=C00000}`
  （与「样式包在宏外」同一条规则）。未实测：自动落到后备字体（宋体）上的生僻字可能不带这个颜色。
- **生成的 TeX 里，控制序列后面紧跟汉字要加 `{}`**：XeTeX 里汉字是字母，`\GwBoxBar为` 会被读成
  一个叫「GwBoxBar为」的未定义控制序列。宏定义里不受影响（定义时已切好词），只有往正文里
  直接写控制序列时才会踩到（研究报告后处理的收尾竖边写成 `\GwBoxBar{}`）。
- **公式、`\ref`、`\cite`、`\href` 不能放进 xeCJKfntef 的宏**（`\(…\)` 放进去直接编译失败），
  研究报告后处理把它们装盒另画：`\GwDelAtom`（红色 + 竖直中线）、`\GwAddAtom`（只画上下边，
  与两侧文字的框接上）、`\GwAddDisplay`（独立公式整块套框）。`\footnote` 装盒会丢，原样
  留在标记外面。
- 独立公式的哨兵写在 `$$` **里面**（`$$〔删:…〕$$`，每个公式单独成段）：mdx 只认行首的
  `$$`，哨兵挡在前面就退化成普通段落、印出公式源码。预览（`preview::research::display_mark`）
  与 TeX 后处理都按这个写法认整块。
- 导出器先按哨兵切块，再在块内配对 `**`。所以带哨兵的 Markdown 里 `**` 绝不能跨哨兵（`serialize::MarkWriter` 保证了这一点）。
- **新增框的断行与四角**（`gonghan-gwa.cls` 与研究报告导言区同一套）：
  - 框里的 1.5pt 留白用 `\kern`，不能用 `\hspace`——胶是断点，竖边会与字断开到两行；
  - 竖边与字之间 `\nobreak`，断点只留在框**前面**（`\GwBoxBarL` 里的 `\penalty5000`：正文里
    字间到处能断，几乎用不上；表格窄列这种没别处可断的，宁可在框前换行也不冲出格子）；
  - `\CJKunderdblline` 的上下边实际占 `[Top-0.5pt, Top]` 与 `[-Bottom-0.5pt, -Bottom]`，而且画不满
    框内留白，所以竖边加长 0.5pt 往下伸、左右竖边各带两截 2pt 短边（`\GwBoxStubs`）补齐四角。
    研究报告公式原子（`\GwAddAtom`）的上下边也按这两条线的位置画。
- 框的上下边位置（`\GwBoxTop` 0.96em、`\GwBoxBottom` 0.24em）是用思源宋体调的。第 ③ 期测试
  在内置仿宋 / 小标宋 / 黑体上量过（矢量线对照 1200dpi 字墨）：仿宋 16pt 上 1.0–2.0pt、
  下 1.8–2.4pt，小标宋 22pt 上下约 2.9pt，都不压字。要调的话只调 `.cls` 和 `postprocess.rs`
  导言区这两处；预览侧对应 `preview/marks.rs` 的 `BOX_TOP_EM` / `BOX_BOTTOM_EM`，三处一起改。
- 删除线在仿宋上偏高：约在字墨从上往下 0.38 处，比中线高约 1.8pt（`\GwWaveDepth = -0.5em`
  是按思源宋体调的）；且 xeCJKfntef 默认**跳过标点**，、，“”（）。处线是断的。待人眼确认。
- （第 ② 期）预览里的新增框是**用单测验证的几何**（竖边条数、每行上下边），界面上的
  实际观感还没有人眼核对过：进对照模式看一眼框和删除线的位置，与「打印预览」的 PDF 对照。
- （第 ② 期）增删块的记号借用了 `TextFormat::underline`（线宽 0，颜色为新增蓝 / 删除红）。
  以后谁要给预览正文加真下划线，得换一个记号，否则会被当成增删去画框、画线。
- （第 ③ 期）**Shift+F7 要先于 F7 判断**：egui 的 `consume_key` 按「逻辑上匹配」比修饰键，
  不带 Shift 的那条也会吃掉 Shift+F7。第 ② 期的写法先查 F7，「上一处」其实一直是「下一处」，
  本期修正并加了测试。
- （第 ③ 期）egui `TextEdit` 的撤销器只在状态稳定 1 秒后自动存点；外部改正文（还原）必须
  前后各 `add_undo` 一次，否则 Ctrl+Z 会连同之前的输入一起撤掉。撤销点里带光标位置：还原后
  用户挪了光标再按 Ctrl+Z，第一下只回到还原后的光标位置，这是 egui 撤销器的既有行为。
- `highlight::tests::swapping_light_and_dark_relayouts_against_the_rebuilt_font_atlas`
  在全量并行跑时偶发失败、单独跑通过，与本改造无关（共享字体图集状态），别为它改测试。
- 可以不用内置 Tectonic 验证 TeX：
  1. 装 XeLaTeX（Debian 系：`texlive-xetex texlive-lang-chinese texlive-latex-extra texlive-plain-generic fonts-noto-cjk`）；
  2. 写个临时测试，调 `redline::build` + `export::write_tex_for_kind` 把 `.tex` 写到临时目录；
  3. 把 `.cls` 里的字体名替换成本机有的字体后执行 `xelatex`。
  - 空 `DraftInput` 会让 `\makeletter` 报 “There's no line here to end”，这是版记要素为空导致的，与花脸稿无关。

## 第 ③ 期审查后的修正

1. **代码层 diff 改为逐行严格比较（空行、行首尾空白都算）**。从前 `diff::body_diff`
   只比非空行、还先 trim，结果只改空行（同段软换行 ↔ 分段、表格行 / 列表项之间多一个
   空行）的变更看不见，逐块还原到底也回不到基准。现在：
   - 空行是 `BlockRole::Blank` 的变更，界面上显示淡色「（空行）」；花脸稿（视觉层）照旧不标；
   - **两级对齐**（`diff::line_ops`）：先只拿非空行做 LCS 定锚，再在锚点之间的夹缝里连同
     空行逐行比。直接把空行丢进 LCS，空行彼此相等，会抢走正文的对齐（没动的段被算成
     删了又加）；
   - 变更块（`diff_hunks::hunks`）跨过**只隔着未改动空行**的两处变更仍算一块；
   - `revert` 改为整段对换「上一段未改动内容与下一段未改动内容之间的区域」，不再按块内
     各条变更的首末行拼（块跨空行时两侧行号对不齐，会把空行挪位）；
   - 「共 N 处」在只读视图里按行计，空行变更也算一处；可编辑视图按变更块计。
   - 测试：`random_edits_revert_back_to_the_exact_base_text`（2000 组随机改稿，含空行、
     行尾空白、文末换行，按随机顺序逐块还原，**逐字节**回到基准）；
     `blank_lines_never_steal_the_alignment_of_real_paragraphs`。
2. **后台花脸稿按「稿件 + 基准版 + 正文哈希」收取**。任务在途时换了基准版而正文没动，
   从前晚到的旧基准结果会覆盖右栏。测试 `a_late_redline_for_the_previous_baseline_is_dropped`。

## 第 ③ 期测试后的修正（2026-09-25）

用真实稿件（稿件库里的公函、普通公文、红头呈批件、研究报告）经内置 Tectonic + 随包字体实际编译、
转图片逐页核对，发现并修了下面这些（每条单独一个提交，括号里是回归测试）：

1. **视觉 diff 移动识别下标混用 → 闪退**（`review_probes::p13`）。`resolve_run` 的移动候选把
   组内块序号当段落序号又查了一遍；组里全是段落时两者相等，测不出来。同一节里有锚不上的
   表格 / 列表项、又挪了段落，就越界 panic——进版本对照首帧、导出、打印预览都在界面线程
   同步算，程序直接闪退；后台线程 panic 后右栏静默停在旧花脸稿。不越界时比错段落，移动被标成
   整段删 + 整段增。这是第 ① 期就有的问题。
2. **研究报告 TeX 后处理重写**（`postprocess::tests` 新增三条）：新增里带行内公式编译失败；
   新增里的加粗只粗第一个字；改过的独立公式印出源码（配合序列化把哨兵写进 `$$` 里）。
   做法见「已知的坑」。
3. **删除侧的公式被转义**（`serialize::tests::a_deleted_formula_is_written_verbatim`）：删除片段
   按纯文本写回，`$w_i$` 成了 `$w\_i$`。公式原样写，公式外照常转义。
4. **研究报告 Word 标记在加粗 / 公式处中断**（`postprocess::tests::ooxml_runs_between_sentinels_get_the_mark_too`）：
   只改写文字里带哨兵的 run，中间的 run 原样穿过。
5. **PDF 里删掉的字是黑色**（`redline::tex_tests::the_class_wires_the_new_mark_styles` 加了断言）：
   见「已知的坑」，改用字体颜色。四个文种删除线下的字实测全部 #C00000。
6. **加 / 删一个标题，下面没动的正文整段删了又加**（`review_probes::p14`）：章节按标题文字对齐，
   只有一边有的章单独对着「空」比。现在只有一边有的**普通标题**章并进前一组比较（标题本身
   作为内容块），正文照常锚定，只标标题；文档标题、正文 / 附件区段标记照旧单独成组
   （`compare::merge_orphan_sections`）。实测公函花脸稿 14→12 页、普通公文 10→9 页。
   注意：删掉的标题按设计不再带 `###`，所以 `assert_sides` 这类「两侧还原源码」的检查不适用于
   标题增删。
7. **左栏新增的空行不显示「（空行）」**（`version_diff::tests::an_added_blank_line_shows_a_placeholder`）：
   只有删掉的空行（画在空隙里）有。现在新增空行的行尾画同样的淡色标签，不进正文、不占光标。
8. **新增框的竖边与字断开到两行、四角合不上**（`redline::tex_tests::the_class_wires_the_new_mark_styles`
   加了断言）：表格窄列里删旧插新挤不下一行时，左竖边留在上一行行尾。做法见「已知的坑」的
   「新增框的断行与四角」。实测公函表格「96.8%」整框换到下一行，正文四角矢量线首尾相接。
9. **CI 的 Linux ARM64 上两个几何测试失败**（与上面的改动无关，之前就一直红）：容器里没有中文
   字体，退回 egui 自带字体后度量不同。`diff_gaps::find_highlight_backgrounds_move_with_their_rows`
   改为比较挪行前后高亮的位移，`preview::aligned_lines_sit_at_the_center_or_right_edge_of_the_content_width`
   改为量排版框而不是墨迹。以后写界面几何测试，别假设字体的上下伸量和墨迹宽度。

测试中确认**不是** bug、按现状保留的：

- 表格中间插一个空行、独立列表项之间加空行，右栏会出现标记——不是误报：解析器规定表格中间
  不能有空行，空行之后的行变成一段 `| … |` 文字；列表项之间加空行后末尾标点按组归一（「；」
  变「。」），纸上确实变了。左栏只显示「加了个空行」，右栏冒出大片标记，可以考虑加一句提示。
- 换基准那一帧同步重算花脸稿：2000 段长稿 release 约 200ms（debug 约 450ms），每切一次卡一下。
- 换基准竞态（防抖期内切、后台在算时切、A/B 来回切、边打字边切，旧结果倒序晚到）都对。

## 下一步

1. **第 ③ 期人眼验收**：长稿边打字边看（右栏不卡、左栏不跳），空隙里的旧行、绿底、
   还原按钮、新增空行「（空行）」标签的观感，打印预览的 PDF 与右栏一致；删除线高度与跳过
   标点、新增框在样式切换处约 0.4–1.6pt 的断缝（见「已知的坑」）。
2. 公文要素就地标注（第 ② 期遗留，做法见上文），要在第 ④ 期之前补上。
3. 长稿右栏预览按可视页裁剪（2000 段每帧约 15ms）。
4. 人眼核对预览里的标记位置（见「已知的坑」），必要时微调 `marks.rs` 常量。
5. `highlight::tests::swapping_light_and_dark_relayouts_against_the_rebuilt_font_atlas` 全量并行跑时
   偶发失败（本机约三次一次），单独跑必过；值得单独查一下共享字体图集的问题。
6. 第 ④ 期：版本时间轴合并浏览界面；稿件管理对照窗改用同一视图（只读路径已有
   `unified_body_ui`）；花脸稿自动归档。

## 第 ③ 期实施指南：可编辑 diff + 逐块还原

### 目标（方案需求第 2 条、第五节）
- 「版本对照」模式的左栏**就是编辑器本身**：直接改字，改动实时反映到两侧。
  删除的旧行以红底只读行显示在原位，新增 / 改动行绿底，未改动区可以折叠。
- 每个变更块旁边有「还原」按钮，一键退回基准版的那一块。还原要能用 Ctrl+Z 撤回。
- 只有「基准版 → 当前未提交内容」可编辑；看历史版本之间的对照（第 ④ 期）时仍然只读，
  所以只读的 `diff_view::unified_body_ui` 要保留，不要删。

### 第一步先做技术验证（spike），单独提交，验证通过再往上盖
最大的风险：egui `TextEdit` 的文本就是编辑内容，没法像 Zed 那样在行间插入「不属于文本的已删除行」。
已查过 egui / epaint 0.35 的源码，有一条比方案原先设想更干净的路：

- `epaint::Galley` 是 `Clone`，`rows: Vec<PlacedRow>` 与 `PlacedRow::pos`、`Galley::rect`、
  `mesh_bounds` 都是公开字段。
- 在 `TextEdit::layouter` 里先 `fonts.layout_job(job)` 得到 galley，**克隆一份，把某一行起之后的
  所有 `PlacedRow.pos.y` 下移「已删除行」的总高度**，同步扩 `rect` / `mesh_bounds`，返回
  `Arc::new(新 galley)`。
- 于是 `TextEdit` 画字、光标定位、点击命中、选区都按下移后的行坐标走，行间天然空出一块。
  `TextEdit::show` 之后，用 `output.galley_pos` + 行坐标，在空隙里用 painter 画红底旧行（只读、不可选中）。
- 源码行 → galley 行：`PlacedRow::ends_with_newline` 标出源码行尾，数到第 N 个换行就是第 N 行的首个 row。
  源码模式不自动折行时是一一对应；混合模式会折行，要按这个规则找首行。

spike 要逐项验证（写进提交说明）：
- 点击空隙上下两行、拖选跨过空隙、上下方向键跨过空隙、PageUp / PageDown；
- 滚动到光标（`TextEdit` 内部按光标矩形 `scroll_to_rect`）、查找高亮（`find.rs`）、行号（`paint_editor_line_numbers`）、
  混合模式装饰（`paint_hybrid_decorations`）是否跟着下移后的行走；
- 应用内输入法的候选框是否跟着光标（`src/ime/` 取光标矩形）；
- 连续打字时 galley 的缓存命中情况：egui 按 `LayoutJob` 哈希缓存，改了 galley 就要确认不会每帧重排。

spike 不通过时的退路：行号槽画删除三角，点开浮层显示被删的旧行（VS Code quick diff 的做法）。
换退路前要先告诉用户。

**spike 结论（已通过，代码在 `src/draft_page/diff_gaps.rs`，每条都有真 egui + 真 `TextEdit` 的测试）：**

| 验证项 | 结果 |
|---|---|
| 点击空隙下方那行 | 光标落在那行 |
| 点击空隙里 | 吸附到最近的一行（上半靠上、下半靠下），不会落到「被删的行」上——那里本来就没有文本 |
| 拖选跨过空隙 | 选区覆盖两侧 |
| ↑ / ↓ 跨过空隙 | egui 按行下标移动，一次跨过空隙、不停在空隙里 |
| PageUp / PageDown | egui 0.35 的 `TextEdit` 本来就不处理这两个键，现有编辑器也没接；开空隙不改变这一点 |
| 滚动到光标 | 光标移到空隙后面时滚动区会把它滚进来。注意：测试里一帧一键连发时 egui 会滚过头，**不开空隙也一样**（对照：该滚 81px 滚了 119px），是 egui 滚动目标叠加的行为；按键之间有空闲帧（真实界面）时正常。跨大空隙时会比最少所需多滚一些，但光标始终在可视区里 |
| 查找高亮 | 是 section 背景色，画在行网格里，跟着行一起下移（tessellate 后核对顶点坐标） |
| 行号 / 混合模式装饰 | 都走 `editor_line_visuals`，读的是 galley 行坐标，自动跟随 |
| 输入法候选框 | 应用内输入法取 egui 的 `IMEOutput::cursor_rect`，它由 galley 算出，落在挪过之后的行上 |
| 缓存 | egui 按 `LayoutJob` 哈希缓存 galley，文本不变时拿到同一个 `Arc`；开空隙只克隆行表（每行一个 `Arc<Row>`），2000 行 / 200 处空隙：release 25–50µs，debug 约 0.4ms |
| 源码行 → galley 行 | `ends_with_newline` 标出源码行尾；有折行时整段一起下移（测试覆盖） |

实现注意：
- 挪动量按 `pixels_per_point` 取整，否则字会发虚。
- `Galley::intrinsic_size` 是 crate 私有字段改不了，`TextEdit` 不用它，不影响。
- 同一行前的多段删除要由调用方合成一处空隙（`open_gaps` 要求行号严格升序）。

### 数据与性能
- **代码层 diff 在界面线程算**：`diff::body_diff(基准版, 当前文本)` 比较便宜，按文本哈希缓存，
  每次按键重算即可。它给出每个变更块在新文本里的字节范围、旧文本，以及插入位置的行号，
  空隙与绿底都由它算。
- **视觉 diff（花脸稿）挪到后台线程 + 150ms 防抖**（见「与方案的出入」第 4 条）：
  `redline::build` 要跑 jieba，长稿按键时同步算会掉帧。右栏允许比输入晚一拍，
  结果带上内容哈希，过期的直接丢掉。可以沿用 `app/jobs.rs` 的后台任务 + `WorkerResult` 模式。
- 现在的 `sync_draft_diff` 每帧都读库（`notes_of`）、每帧都把 `DraftInput` 序列化成 JSON 算哈希。
  可编辑之后调用更频繁，要改成：基准版快照在切换基准时取一次存起来，要素 / 备注只在变化时重算。
- 绿底画在字的**下面**：`TextEdit` 之前先 `let bg = ui.painter().add(Shape::Noop)` 占位，
  `show` 之后算出行矩形，再 `ui.painter().set(bg, …)` 填进去（egui 常用的占位手法）。
  删除行画在空隙里，不与字重叠，直接画就行。

### 逐块还原
- 按钮放在行号槽，悬停变更块时出现，或在变更块首行常驻一个小图标（与 Zed 相同）。
  点击后用基准版那一块的原文替换当前的字节范围。
- 撤销：egui 的 `TextEdit` 每帧都把 `(光标, 文本)` 喂给自己的撤销器（`TextEditState::undoer`），
  外部改了字符串，下一帧会被记成一个新状态（`builder.rs` 里的 `feed_state`；`TextEditState::undoer()` / `set_undoer()` 可以取出和写回撤销器）。**要实测**「还原 → Ctrl+Z」能回到还原前；
  不行就在还原前后手动 `feed_state`，或者走 `draft_page` 里现成的修订撤销机制（`revise.rs` 的 `undo_revision`），
  二选一，并写进测试。
- 还原后光标放在被还原块的起点，右栏跟着滚到对应位置（`ChangeLinks` 照用）。

### 联动（沿用第 ② 期规则）
- 编辑器里光标所在的变更块就是「当前焦点」，右栏跟着滚动并常亮。F7 / Shift+F7 仍然有效，
  跳转时把光标移到变更块起点。
- 右栏单击带标注的块 → 编辑器光标跳到对应源码（`jump_to_source` 已有）。

### 测试（至少这些）
- 纯函数：「源码行号 → galley 行下标」的换算（有折行、无折行）；按变更块计算空隙的高度和位置；
  还原操作（字节范围替换后，该块在代码 diff 里消失，其余块不受影响）。
- 集成：仿照 `draft_page::version_diff::tests`，用真 egui 上下文画一帧，断言：
  - 空隙存在、删除行画在空隙里；
  - 点击空隙下方那行时，光标落在那行（不会落到被删的行上）；
  - 还原后文本等于预期，且撤销能回来。
- 第 ① / ② 期的全部测试保持通过，**不许删**。

### 验收（人眼）
- 长稿（2000 行以上）边打字边看，右栏不卡、左栏不跳。
- 打印预览的 PDF 与右栏一致。
- 做完更新本文：进度表、代码地图（第 ③ 期交付了什么）、与方案的出入、下一步（第 ④ 期）。

## 工作约定（在 AGENTS.md 之外补充）
- 改视觉 diff 必须保持两侧不变式成立。**不许为了让测试通过而删测试或放宽断言**，行为确实变了，就在测试注释里写清楚为什么。
- 涉及 TeX 版式的改动要实际编译看一眼（方法见上文），光单测不够。
- 每完成一期，更新本文的「当前进度」和「下一步」。
