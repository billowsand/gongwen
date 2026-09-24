# 版本变更与花脸稿改造：交接说明

> 给接手的开发者（含本地 Claude）：先读 `docs/version-diff-redesign.md`（方案与需求结论），
> 再读本文（做到哪了、和方案有什么出入、下一步怎么做、有哪些坑）。
> 本文随进度更新；每完成一期，把「当前进度」和「下一步」改掉。

## 当前进度

| 期 | 内容 | 状态 |
|---|---|---|
| ① | 视觉 diff 引擎 + PDF / Word 花脸稿导出 | **已完成**，已合入 `main` |
| ② | 左 Markdown diff / 右花脸稿预览的界面、标记绘制、联动、打印预览 | **大部分完成**，已合入 `main`；剩「公文要素就地标注」（见下文，可与第 ③ 期并行） |
| ③ | 可编辑 diff + 逐块还原 | **下一步**，实施指南见文末 |
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

## 与方案的出入（有意为之，接手时别当 bug 改回去）
1. **方案 4.3 说三个消费方直接读 `RedlineOverlay`，实际导出走的是「overlay → 带哨兵的 Markdown
   → 原有导出器」**。好处是花脸稿与定稿走同一条排版链路，版式天然一致。
   第 ② 期的预览**建议也吃同一份带哨兵的 Markdown**（见下文），这样「预览 = 导出」由构造保证。
2. **公文要素（主送、抄送、成文日期等）的就地标注没有接上**。`DocumentModel::from_inputs` 已写好，
   但标了 `#[allow(dead_code)]`；序列化对 `VisualBlock::Element` 返回空。
   要素标注要在版头 / 版记的绘制处做（预览和导出都要），归在第 ② 期。
3. 研究报告的花脸稿是在 mdx 产物上后处理：给哨兵换宏，花括号不平衡时补齐。能编译，
   但如果标注跨过 `\textbf{…}` 这类命令，标注和加粗的范围可能有偏差。
4. （第 ② 期）**视觉 diff 仍在界面线程同步算**，按内容哈希缓存。本期左栏只读、改字要
   切回 Markdown 模式，停在对照模式时内容不变，缓存一直命中；第 ③ 期边改边看时再按
   方案第五节挪到后台线程 + 防抖。
5. （第 ② 期）预览里的新增框不像 TeX 那样把字往里挤 1.5pt：竖边画在进出新增块时
   留的 `leading_space` 缝里。研究报告公式混排的断行按原子宽度预算，那条路径**不留缝**
   （否则算好的断行会溢出），竖边会贴字。
6. （第 ② 期）稿件管理的对照窗、AI 工作台的比对仍用旧的分栏视图
   （`diff_view::manuscript_diff_ui`），但去掉了已无调用方的「跳源码 / 导出花脸稿」入口；
   第 ④ 期统一换成新视图。研究报告现在也能在对照页导出花脸稿（第 ① 期已覆盖）。

## 已知的坑
- **xeCJKfntef 的标注宏（`\GwDel` / `\GwAdd`）里面，字体切换只作用到第一个字。**
  加粗、括号楷体必须包在宏**外面**。所以一段新增按样式切开，分成
  `\GwAddOpen` / `\GwAddMid` / `\GwAddClose` 三种；括号小一号字之前先 `\GwBoxFreeze` 定死框高。
  以后往花脸稿里加任何样式，都照这个规则来。
- **删除线的颜色要写在平铺单元里**（`\GwStrikeUnit`），外层 `\textcolor` 只染文字。
- 导出器先按哨兵切块，再在块内配对 `**`。所以带哨兵的 Markdown 里 `**` 绝不能跨哨兵（`serialize::MarkWriter` 保证了这一点）。
- 框的上下边位置（`\GwBoxTop` 0.96em、`\GwBoxBottom` 0.24em）是用思源宋体调的，还**没有**在
  仿宋 / 小标宋上实测。如果视觉上偏紧或偏松，只调 `.cls` 和 `postprocess.rs` 导言区这两处；
  预览侧对应 `preview/marks.rs` 的 `BOX_TOP_EM` / `BOX_BOTTOM_EM`，三处一起改。
- （第 ② 期）预览里的新增框是**用单测验证的几何**（竖边条数、每行上下边），界面上的
  实际观感还没有人眼核对过：进对照模式看一眼框和删除线的位置，与「打印预览」的 PDF 对照。
- （第 ② 期）增删块的记号借用了 `TextFormat::underline`（线宽 0，颜色为新增蓝 / 删除红）。
  以后谁要给预览正文加真下划线，得换一个记号，否则会被当成增删去画框、画线。
- `highlight::tests::swapping_light_and_dark_relayouts_against_the_rebuilt_font_atlas`
  在全量并行跑时偶发失败、单独跑通过，与本改造无关（共享字体图集状态），别为它改测试。
- 可以不用内置 Tectonic 验证 TeX：
  1. 装 XeLaTeX（Debian 系：`texlive-xetex texlive-lang-chinese texlive-latex-extra texlive-plain-generic fonts-noto-cjk`）；
  2. 写个临时测试，调 `redline::build` + `export::write_tex_for_kind` 把 `.tex` 写到临时目录；
  3. 把 `.cls` 里的字体名替换成本机有的字体后执行 `xelatex`。
  - 空 `DraftInput` 会让 `\makeletter` 报 “There's no line here to end”，这是版记要素为空导致的，与花脸稿无关。

## 下一步

1. **第 ③ 期：可编辑 diff + 逐块还原**，按下面的实施指南做。
2. 公文要素就地标注（第 ② 期遗留，做法见上文）。它与第 ③ 期互不依赖，可以放在第 ③ 期之后，
   但要在第 ④ 期之前补上。
3. 人眼核对预览里的标记位置（见「已知的坑」），必要时微调 `marks.rs` 常量。

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
