# 富文本粘贴转 Markdown 设计

从 Word、WPS、网页、Excel 复制的富文本粘贴进起草页时，自动转成公文助手的 Markdown：
一方面转格式（标题样式、加粗、列表、表格、引用块），一方面按文字内容还原结构
（「一、」→ `##`、「（一）」→ `###`、「1.1」→ `###` 等）。全程只用确定性规则，不调用模型。

本文是单文件方案：第一至九节是设计，第十节是分期，第十一节是进度与交接。

## 一、已定决策

2026-10-23 与用户确认，均按推荐项：

1. **Ctrl+V 默认智能转换**；Ctrl+Shift+V（macOS 为 ⌘⇧V）始终原样粘贴纯文本；智能粘贴后
   状态提示里给「改为原样粘贴」，一键换回原文。
2. **多行纯文本也做编号识别**（微信、记事本、即时通讯里复制的文字只有纯文本）。
3. **公文文种里的 1 / 1.1 / 1.1.1 按 `##` / `###` / `####` 处理**，导出时由程序重编成
   「一、（一）1.」——版式本来就由程序控制。
4. **图片放第三期**；第一、二期略过图片，只在状态提示里报张数。

## 二、现状与可复用的代码

| 能力 | 位置 | 说明 |
|---|---|---|
| 编号 → 标题 / 列表 | `src/doc_import/structure/` | 第X章、一、（一）、1.、（1）、①；一是二是；附件；版头版记；Word 自动编号把「一、」转成「1、」（`from_list`）；层级压紧时第一层保持固定层级（只粘「（一）」仍是 `###`） |
| 文本清洗 | `src/doc_import/clean.rs` | `clean_before_structure`：噪音、空格、标点、表格、空行 |
| 整篇整理 | `src/draft_page/ribbon.rs` 的 `CleanAction::BusinessToMarkdown` | 对整篇稿子跑 `restructure(Options::restructure_only())` |
| HTML → 文档模型 | `vendor/anydoc/src/shared/html.rs` 的 `to_blocks` | 给 EPUB 用；认 h1–h6、table（跨行跨列）、blockquote、pre、math（转 TeX）、br、img、b/strong、del/s、列表的 start/type/value，以及一小组语义 CSS（粗、斜、删除线、`display:none`）。只吃格式严格的 XHTML 元素树，**模块是私有的** |
| 文档模型 → Markdown | `anydoc::document_to_markdown` | 已有的 vendor 补丁公开的 |
| 读 HTML 剪贴板 | `arboard 3.6.1`（egui-winit 的间接依赖）的 `get().html()` | Windows 读 `HTML Format` 并只取 StartFragment…EndFragment（**`<head>` 里的 `<style>` 拿不到**）；macOS 读 `public.html`；Linux 走 X11 |
| 带撤销点的替换 | `src/draft_page/diff_editor.rs` 的 `replace_with_undo` | 前后各记一个撤销点，Ctrl+Z 一次回到替换前 |
| 拦截粘贴事件的先例 | `src/ai_panel/composer_ui.rs` 约 90 行 | 输入框有焦点时在 TextEdit 之前 `input.events.retain` 吃掉 `Event::Paste` |
| 选区 / 光标 | `src/draft_page/markdown.rs` 的 `editor_selection`、`editor_cursor`、`insert_block` | 块级插入时前后补空行 |

egui 自己只给 `Event::Paste(String)` 纯文本。egui-winit 0.36 判定粘贴是
`command && V`、`Paste` 键、Windows 上的 `Shift+Insert`，判定后直接 `return`，**不再发
V 的 Key 事件**，所以 Ctrl+Shift+V 只能靠同一帧的 `input.modifiers.shift` 区分。

## 三、总体流程

```
Ctrl+V（起草页 Markdown 编辑框有焦点、可编辑）
 → 接管条件不满足（光标前有非空白字符、内容只有一行、Ctrl+Shift+V）→ 交还 TextEdit 原样粘贴
 → 读剪贴板 HTML（arboard，卡体积）
     有 HTML：容错解析（html5ever 系）→ 预清洗 → 转成 anydoc Element → to_blocks
              → 在文档模型上按文种裁剪语法 → document_to_markdown
     无 HTML：用 Event::Paste 里的纯文本，先判断是不是已经写好的 Markdown
 → clean::clean_before_structure → structure::restructure(粘贴专用选项) → clean::tidy
 → 结果和原文只差空白：当原样粘贴处理
 → 替换选区 / 在光标处插入（前后补空行），一个撤销点，重新校验
 → 状态提示「已按格式粘贴：3 个标题、1 张表……」+「改为原样粘贴」
```

粘贴是用户的键盘输入，可以直接改 `generated_markdown`，不涉及 AI 红线。

## 四、格式层映射

| 来源 | 转成 | 备注 |
|---|---|---|
| h1–h6、Word 标题 1–6（剪贴板里就是 `<h1>`…） | `#` 级别 | 段首有编号时由 structure 按编号重定层级（`styled`） |
| Word `class=MsoTitle` | `#` | 片段里没有样式表，只能认类名 |
| Word 列表段落（`mso-list:l0 level1 lfo1`，编号在 `mso-list:Ignore` 的 span 里） | 保留编号文字 | 交给 structure 判标题还是列表 |
| `ul` / `ol` / `li` | `- ` / `1. ` | 公文只认两级，第二期按文种压平 |
| `b` / `strong` / `font-weight≥600` | `**…**` | 整段加粗的短行由 `bold_unnumbered` 处理 |
| `i` / `em` | 去掉标记留文字 | 公文和研究报告都不识别斜体（技能包 markdown-syntax 第 4 节） |
| `del` / `s` / `strike` | 连文字一起丢 | Word 修订的删除、网页划掉的旧内容 |
| `ins` | 留文字 | |
| `blockquote`、Word「引用」样式 | 研究报告 `> `；其余文种拆成普通段落 | 公文不收引用块 |
| 文框候选：`<aside>`、只有一格的 `<table>`（Word「文本框表格」）、带边框或底纹的 `div` / `section` | 研究报告 `> [!名称] 标题`；其余文种不处理 | 只认第一段是**整段加粗的短标题**（≤ `TITLE_MAX`）且后面还有正文的；名称取标题前缀（专栏 / 案例 / 例子 / 做法 / 延伸阅读 / 知识链接，可带编号与冒号），取不到写「专栏」并在状态栏提示（第三期） |
| `table` | 管道表 | 复用跨行跨列网格；只有一格的表格（Word「文本框表格」、网页排版用表格）拆回段落 |
| `a` | 只留文字 | 公文不收链接 |
| `img` | 第一、二期略过并计数 | **第三期的图片入库待真实样本后做**（`file:` / `data:` 走 `embedded.rs` 入库、网络图片永不下载）；缺样本时按现状略过并计数 |
| `pre` / `code` 围栏 | 拆成普通段落 | 技能包规定正文不写代码围栏 |
| `math`（MathML） | 研究报告 `$…$`；其余文种丢 | **Word 公式（OMML）待真实样本后做**；网页的 MathML 已认，公文一律丢 |
| `br` | 段内换行 | structure 按「硬换行」处理 |
| 噪音 | 删除 | `script` / `style` / `head` / `noscript` / `template` / `iframe` / 注释、`<o:p>`、`mso-hide:all`、零宽字符、段首 `&nbsp;` 与全角空格缩进「　　」 |

## 五、文本层：编号定级

**已有（`structure/mod.rs` 与 `structure/numbering.rs`）**：第X章比一、浅一层；一、→`##`；（一）→`###`；1. →`####` 或列表；
（1）→`#####` 或列表；① → 列表；段内一是二是拆行并加粗引子；附件；版头版记。

**第二期新增：点分编号方案**

| 编号 | 层级 |
|---|---|
| `1` / `1.` / `1、`（点分方案下） | `##` |
| `1.1` / `1.1.` | `###` |
| `1.1.1` | `####` |
| `1.1.1.1` | `#####` |

- **先定方案再定级**：片段里至少出现两个点分编号，而且有相邻两处**严格**衔接（同级下一号 1.1→1.2、
  回到上一层的下一号 1.1.2→1.2、下一层从 1 起 1.1→1.1.1、父级进位 1.2→2.1；跳号不算），
  并且第一处的上级有着落（它是 `x.1`，或片段里有「3 标题」这样的顶层行）、顶层有着落（顶层是 1、
  有顶层行，或跨两个以上顶层号），才整段切到点分方案，这时段首「1 标题」是 `##` 而不是 `####`。
  认错的代价是删掉数字（日期「10.1」、数量「1.2 个百分点」），所以宁严勿宽。
- **和小数区分**：编号后跟半角 / 全角空格或直接跟汉字，但不能跟计量单位；整行不超过 `TITLE_MAX`；
  示意的父级要存在（成体系的 1.1、1.2 自然满足）。
- structure 里的测试 `enumerations_and_decimals_are_not_numbering` 写死了「3.2.1 技术路线」
  不是编号：已改成「孤立的 3.2.1 不是编号、成体系的点分编号是」，`dotted_numbering_scheme_sets_levels_and_strips_numbers`
  管后者。
- 罗马数字 Ⅰ、Ⅱ 当最浅一层（`##`，与「第X章」同级）；ASCII 的 `I.` 要成组出现（`I.`、`II.`）才认。
  `A.` / `（A）` / `(a)` 是第六层，压不到 `#####` 以内时当列表。
- 「第一，」「首先，」开头的段落归入「一是……」那类段内枚举，引子加粗，不升成标题。
- 原文跳号（一、三、）时状态提示里说一句，因为编号由程序重新生成。
- 字体线索（黑体一级、楷体二级、仿宋加粗三级）只在有编号但层级模糊时消歧，不单凭字体升标题。
  **已挪到第三期**，原因见第十节。

## 六、粘贴和整篇导入的区别

| 情况 | 处理 |
|---|---|
| 内容只有一行 | 不接管，原样粘贴（复制一个词就是一个词） |
| 光标所在行光标前有非空白字符 | 压成一行行内文字插进去：去标题 / 列表记号，保留原文编号文字与加粗，段落直接相连；含表格时不接管 |
| 有选区 | 块级粘贴时用转换结果替换选区；选区从句中开始时按行内粘贴 |
| 认文档标题（`detect_title`） | 稿子里还没有 `# ` 行时才打开 |
| 剥版头版记（`strip_layout`） | 只在稿子为空时打开（相当于整篇复制一份公文） |
| 纯文本 `lines_are_paragraphs` | 打开（即时通讯、记事本里一行就是一段）；HTML 来源关掉 |
| 纯文本已经是 Markdown（有 `#` 标题行、`<!-- [` 标记、管道表、代码围栏） | 原样粘贴 |
| 文种禁用的语法（白头件、红头呈批件里的列表） | 列表项转成各自独立的普通段落，不自动补「一是二是」 |
| 光标所在章节的层级 | 只平移**没有编号**的样式标题（`<h1>`–`<h6>`、Word 标题样式）：整体往下，最浅的一个落到「插入点上方最近标题 + 1」，稿子里已有 `# ` 时不许出现 `# `；超过 `#####` 转加粗段落。有编号的标题按编号定级，不动 |

## 七、交互

- 智能粘贴后：状态栏写「已按格式粘贴：N 个标题、M 张表、K 个列表项（略过 J 张图片）」；研究报告里
  认出文框而名称是默认值时，再补一句「识别出 N 个文框，名称暂写「专栏」，请按需改成案例、做法等」；
  原文编号跳号时补一句跳号提示。同时给一个「改为原样粘贴」按钮。按钮在下一次编辑、换稿件或超时后消失；点它时先核对插入的那段原文还在原处
  （同 `PreviewAnchor::range_in` 的做法），再用纯文本替换，同样只占一个撤销点。
- Ctrl+Z 一次回到粘贴前。
- 已知限制：Windows 上 `Shift+Insert` 带着 Shift，会被当成原样粘贴。
- AI 侧栏输入框的粘贴：大段内容成为临时材料；剪贴板里有 HTML 时材料用转换后的 Markdown，
  表格是管道表（第三期）。

## 八、安全与限制

- 剪贴板 HTML 先卡体积（Word 几页内容的 HTML 就能带出几 MB 样式），超了退回纯文本，上限登记在
  `src/file_limits.rs`。
- 解析限节点数与嵌套深度，超了退回纯文本；任何解析失败都退回原样粘贴并在状态栏说明，不 panic。
- 不下载任何远程资源（图片、样式表、字体）；不执行脚本。
- 不新增 `unsafe`：剪贴板 FFI 由 arboard 负责。Linux 上 arboard 已经随 egui-winit 链接进来，
  不增加系统依赖，不影响 GLIBC 2.28 的 deb。
- 测试夹具只放脱敏或自拟内容；用户从真实公文里采的样本先脱敏再入库。

## 九、各来源的已知特点（第一期按样本核实）

- **Word（Windows）**：剪贴板只取片段，`<head><style>` 没了，类名样式失效，只能靠行内 `style`
  和标签；列表编号在 `<span style='mso-list:Ignore'>一、<span>&nbsp;&nbsp;</span></span>` 里；
  条件注释 `<![if !supportLists]>`；`<o:p>`。
- **WPS**：同样提供 HTML Format，类名与 Word 不同，需要样本。麒麟上的 WPS 也要一份。
- **政府网站**：`<p style="text-indent:2em">` 再加全角空格；`<strong>` 包一、开头的整行；常用
  表格排版。
- **微信公众号**：层层 `section` / `span`，背景图、空段落多。
- **Excel**：HTML 表格，带合并单元格。
- **浏览器复制**：macOS / Linux 给整页 HTML（含 `<style>`），可以把样式表喂给 `Stylesheet::add`。

## 十、分期

1. **第一期**：读 HTML 剪贴板；HTML → anydoc Element（vendor 补丁公开 HTML 入口）；按文种裁剪语法；
   粘贴专用的 structure 选项；接管条件；纯文本多行识别；替换选区 + 单个撤销点；状态提示与「改为
   原样粘贴」；合成样本夹具 + 采集真实剪贴板样本的 ignored 测试；使用帮助加一小节。
2. **第二期**：点分编号方案与小数区分；罗马数字与字母编号；跳号提醒；按光标所在章节定层级
   （只动没有编号的样式标题）；光标在行中时只保留行内格式；列表整形（紧凑、只留两级、白头件与
   呈批件拆成段落）。提示词见 `docs/rich-paste-phase2-prompt.md`。
3. **第三期**：Word / 网页图片入库（本机临时文件与 `data:`，网络图片不下载）；Word 公式；研究报告
   文框的半自动识别；AI 侧栏粘贴保留表格。提示词见 `docs/rich-paste-phase3-prompt.md`，开工前要
   先采几份真实剪贴板样本（提示词第 0 节）。
4. **待定（要真实样本）**：字体线索消歧。原排第二期、再挪到第三期，现在单列：`anydoc` 的文档模型
   不带字体，要从 `html.rs` 另开一条通道传给 `structure`，而有编号时层级已经由编号样式决定。等真实
   样本里出现字体能救、编号救不了的情形再做。

## 十一、进度与交接

- 2026-10-23：方案定稿，四项决策见第一节。第一期提示词见 `docs/rich-paste-phase1-prompt.md`。
- 2026-10-23：**第一期已完成**，待真机验收（Word / WPS / 浏览器 / Excel 的真实剪贴板）。
  下文是交接。

### 做了什么

- 依赖：`arboard 3.6.1`（`default-features = false`，剪贴板 HTML）与 `scraper 0.29.0`
  （`default-features = false`，容错 HTML 解析）。**选了 scraper 而不是直接用
  html5ever**：html5ever 只给 HTML5 解析算法与 `TreeSink` trait，要么自己写一份 DOM
  （一百多行、还要维护树构建的各种边界），要么引 `markup5ever_rcdom`（版本与 html5ever
  不同步，停更快）。scraper 的 DOM（`Html::parse_document` + `ElementRef`）正是需要的
  那层，代价是多一份 `selectors` 0.41 与 `ego-tree`（`lol_html` 用的是 `selectors` 0.37，
  两份并存；许可类别 MPL-2.0 与原有依赖相同）；许可证 ISC，与 GPL-3.0-or-later 兼容。
- `vendor/anydoc` 补丁二：`pub mod html` 转出 `Node` / `Element` / `Attr` 与
  `html::to_document(body, css)`，内部 `HtmlCtx` 让链接只留文字、图片一律不取。
  `package/xml.rs` 加了四处 `#[allow(missing_docs)]`（公开后 `missing_docs` 会刷屏）。
  见 `vendor/anydoc/README.md`。
- 转换模块 `src/rich_paste/`：`mod.rs`（类型与入口）、`clipboard.rs`（读 HTML、线程内测试
  替身）、`html.rs`（容错解析 + 预清洗 + 节点/深度上限）、`syntax.rs`（在文档模型上裁语法、
  Markdown 判定与计数）、`pipeline.rs`（两条路汇到 `clean` + `structure`）、
  `samples.rs`（夹具测试、导出、采样工具）、`tests.rs`（12 组样例测试）。
- `structure::Options::paste(detect_title, lines_are_paragraphs, strip_layout)`：现有两个
  构造函数没动，`tests/fixtures/import` 的期望输出逐字未变。
- 起草页接入：`src/draft_page/paste.rs`。`markdown_editor` 里在 `TextEdit` 之前调
  `smart_paste`（删掉本帧的 `Event::Paste`、用 `replace_with_undo` 写回、光标落到插入
  内容末尾、`revalidate`），编辑区上方一行 `recent_paste_strip` 画「改为原样粘贴」。
  `DraftSession::recent_paste` 保存插入范围 / 插入内容 / 原始纯文本 / 过期时间，
  `reset_transient`（换稿）清掉。
- `file_limits::MAX_CLIPBOARD_HTML_BYTES = 8 MiB`。
- 夹具：`tests/fixtures/paste/` 五份合成样本（Word 片段、Word 整页、政府网站、微信公众
  号、Excel）+ `expected.md` + README（采样与脱敏流程）；`capture_clipboard_html`
  （ignored）读真实剪贴板，`GONGWEN_PASTE_SAMPLE=名字` 指定输出文件名。
- 文档：使用帮助第 6 章加「从 Word、网页粘贴」一小节；`AGENTS.md`「进行中的工作」加一条。

### 与方案的出入

1. ~~`<br>` 与 Word 软回车在结构还原时被接回一行。~~ 验收时改掉了：接回一行会把
   「联系人：张某<br>联系电话：」接成「张某联系电话」。现在 `syntax.rs` 在文档模型上把
   段落里的换行拆成两段（标题里的换行仍接成一行），`structure` 的规则没动。见下文「验收记录」。
2. **「改为原样粘贴」按钮的位置与失效条件。** 方案第七节说状态栏或编辑区上方、下一次
   编辑 / 换稿 / 超时后消失。实际放在编辑区上方一行（状态栏那一行有「只许一个
   horizontal、行高钉死」的布局约束，塞按钮容易把面板顶高），失效条件是「插入的那段
   文字被改过」而不是「任何一次编辑」——在别处打字不影响还原，这比方案更精确。
3. **Word 的「引用」样式**额外按类名 `MsoQuote` / `MsoIntenseQuote` 转成引用块（方案
   表格里写了「Word 引用样式」，但没给实现路径；剪贴板片段里没有样式表，只能认类名）。
4. **单格表格**在 `syntax.rs` 里就算成正文段落（`Table::is_single_cell`），没有留给
   anydoc 的 `TableKind::Layout` 分支——剪贴板 HTML 里的 `<table>` 一律是 `Data`。
5. **纯文本来源的判断顺序**：先看「已经是 Markdown」（有 `# ` 标题行、`<!-- [` 标记、
   管道表分隔行、代码围栏）再看内容行数；单行的纯文本在 UI 层就不接管，所以
   `convert` 对单行纯文本通常不会被调用。
6. **`Options::paste` 的签名**取三个 bool（`detect_title`、`lines_are_paragraphs`、
   `strip_layout`），由 `pipeline::restructure` 从 `Context` 算出来再传，保持
   `structure` 不依赖 `rich_paste` 的类型。

### 已知坑（第二期接手前必读）

- **`Event::Paste` 只有纯文本**，HTML 必须另外读剪贴板；读剪贴板与解析都可能失败，
  失败一律退回原样粘贴，不能 panic、不能吞掉剪贴板内容。
- **Ctrl+Shift+V 只能靠 `input.modifiers.shift` 区分**：egui-winit 0.36 把
  `command && V` 直接换成 `Paste` 事件、不再发 V 的按键事件。Windows 的 `Shift+Insert`
  因此也被当成原样粘贴（写进帮助的「已知限制」）。
- **Windows 的剪贴板 HTML 是片段**（`StartFragment…EndFragment`，没有 `<head>`），
  所以类名样式（`MsoTitle`、`MsoQuote`）要按类名认，不能指望样式表；macOS / Linux 的
  整页有 `<style>`，已经喂给 `Stylesheet::add`，但只支持 tag / class / tag.class 三种选择器。
- **`scraper` 的 `parse_document` 对片段也能用**（会自动补 `html` / `head` / `body`），
  所以两条来源走同一条路。别改成 `parse_fragment`：整页输入时片段模式会丢 `<body>` 里
  的块级结构。
- **节点上限 200 000、深度 64、样式表 256 KiB**（`html.rs` 顶部常量）。上限测试用小额度走
  `parse_with_limits`，业务代码走 `parse`。改上限要同时看 `MAX_CLIPBOARD_HTML_BYTES`。
- **`arboard` 的 feature 会被 `egui-winit` 统一开关**：即使我们写
  `default-features = false`，`egui-winit` 仍会打开 `image-data`（多编 `image`，本来就在
  依赖树里）。这不影响行为，但 `Cargo.lock` 里别以为能省掉。
- **数字与标题计数按最终 Markdown 数**（`syntax::count_markdown`），不是按模型块数：
  `structure` 会把「一、」段落升成标题。改计数逻辑要两边一起看。
- **`recent_paste` 的范围是字节范围**，替换前后都用 `text.get(range) == Some(inserted)`
  核对；`layout`/`find` 的 `pending_source_jump` 是同一套光标机制，改一处要看另一处。
- **测试不能碰真剪贴板**：`clipboard::set_test_html` 是线程内的替身，必须在同一个测试
  线程里设置与清理（`clear_test_html`），否则并行跑的别的测试会读到假数据。
- 预览/导出的解析器对管道表、嵌套列表的处理与「贴进去看着对」不完全一致（列表项之间
  会留下空行，是 anydoc 的 loose list 写法）；第二期按文种压平列表时一并解决。
- **`anydoc` 把只含行内内容的 `div` / `section` 当透明容器**，挨着的几个会接成一段。
  `html.rs` 的 `is_leaf_container` 把它们改成 `p` 再交出去；别去改 anydoc，EPUB 的输出要保持不变。
- **有序列表一律改成 `- `**（`syntax.rs`）：`1. ` 开头的行会被 `structure` 当成段首编号
  重新定级，嵌套子项就压平了；公文助手里有序 / 无序列表编号效果相同。
- **字段分隔空格**（`gaps.rs`）：占位符 U+FDD0 在 `html.rs` 逐个文本节点、`pipeline` 汇总前
  各护一遍，所以 `protect` 必须能重复调用；最后统一换回一个全角空格。导入用的
  `clean::normalize_spaces` 没改，导入同样的文字仍会删掉这个空格。
- **复制的本来就是 Markdown 时原样粘贴**：VS Code 一类编辑器复制时也带一份 HTML，
  `pipeline::convert` 先看纯文本像不像 Markdown，像就不走 HTML。

### 验收记录（2026-10-23）

审阅了四个提交，`fmt`、`clippy -D warnings`、全量测试都通过。又用探针输入跑了一遍，修了
下面这些（随验收提交）：

1. **VS Code / Typora 源码模式复制的 Markdown 被改坏**：剪贴板同时有 HTML，走 HTML 后
   `## 标题` 被转义成 `\## 标题`，几行接成一行。修法：纯文本已像 Markdown 时整体不接管。
2. **用 `div` 分段的网页、编辑器内容接成一段**：见上面「已知坑」的 `is_leaf_container`。
3. **`<br>` 两边接成一串**：改成拆段（出入第 1 条）。
4. **字段之间的全角空格被删**：政府网站夹具的期望输出里本来就是「张某联系电话」，属于
   把错误结果写进了期望。新增 `gaps.rs`，夹具期望改成「张某　联系电话」。
5. **嵌套有序列表被压平成同级**：有序列表改成 `- `。
6. **选区从句子中间开始时也接管**：会把那句话从中间拆开。现在按选区起点判断，与「光标前
   有字」同样不接管。
7. `THIRD_PARTY_NOTICES.md` 里「`selectors` 与 `lol_html` 是同一份依赖」不对（0.41 与 0.37），已更正。

还实测了 Windows 真剪贴板：按 CF_HTML 规范写入 UTF-8 的 `HTML Format`，
`capture_clipboard_html` 用 arboard 读回来的正是 StartFragment…EndFragment 那一段。
顺带发现：Windows PowerShell 5.1 的 `Set-Clipboard -AsHtml` 按 GBK 写入，arboard 按
UTF-8 解码失败、返回 `None`，这时走纯文本兜底，行为正确。有的老程序可能也这样写剪贴板，
真机验收时留意。

**仍待真机验收**（GUI 由用户在本机做，本次未操作应用窗口）：

- Word、WPS（含麒麟）、Chrome / Edge、Excel 各复制一段公文粘进起草页：标题层级、表格、
  加粗、列表是否符合预期，状态栏计数是否正确；
- `Ctrl+Shift+V` 原样粘贴，`Ctrl+Z` 一次撤回；
- 「改为原样粘贴」那一行出现与消失时，编辑区会不会上下跳动、遮挡第一行；
- 光标在行中间、选区从句中开始时，确实走普通粘贴；
- 用 `capture_clipboard_html` 采几份真实样本，脱敏后补进 `tests/fixtures/paste/`。

### 第二期从哪接手

第二期要做的（方案第五节与第十节第 2 条）：点分编号（1 / 1.1 / 1.1.1）、罗马数字与
字母编号、跳号提醒、字体线索消歧、光标所在章节定级、光标在行中时只保留行内格式、按
文种裁剪列表（白头件、呈批件）。

- 编号与层级全在 `src/doc_import/structure/`：`parse_prefix`、`resolve_ambiguous_levels`、
  `heading_level_map`、`classify`。**先改那里的规则并补它自己的测试**，粘贴这条路只是
  传选项；不要为了迁就粘贴改 `lines_are_paragraphs` 的语义。
- `enumerations_and_decimals_are_not_numbering` 这条测试是按方案第五节要重写的那条
  （改成「孤立的 3.2.1 不是编号、成体系的点分编号是」），别直接删。
- 光标在行中：`src/draft_page/paste.rs` 的接管条件（`markdown[line_start..anchor].trim().is_empty()`，
  `anchor` 是光标或选区起点）是唯一的入口；改成「只保留行内格式、压成一行」时，要把转换结果压成 inline 形态，
  再走 `replace_with_undo`。
- 按文种裁剪列表需要 `Context` 里加文种（现在只有 `research` 一个布尔）；`Context` 是
  `pub(crate)` 结构体，加字段要同步 `pipeline.rs` 与 `paste.rs`。
- 第三期（图片入库、Word 公式、文框建议、AI 侧栏粘贴）另开方案，不要在第二期顺手改
  `syntax.rs` 里丢图片的那一行——那时要接 `src/doc_import/embedded.rs` 的入库路径与
  `data:` / `clip_image*.png` 的临时文件处理。

### 第二期（编号补全与粘贴位置）

- 2026-10-23：**第二期已完成**，待真机验收。下文是第二期交接（第一期的内容与验收记录
  在上面，未删改）。

#### 做了什么

- 先把 `structure.rs` 按功能域拆成 `structure/{mod,numbering,tests}.rs`（纯移动，单
  独一个提交），再在 `numbering.rs` 里加 `Numbering`（整段文字定编号方案：点分 / ASCII 罗马 / 字母）、
  `dotted_prefix` / `bare_number_prefix` / `roman_prefix` / `letter_prefix`、
  `NUMBER_UNITS`（区分小数与计量单位）、`chinese_number`（汉字序号读成数字）。
  `Prefix` / `Item` 记下原文编号文字 `marker` 与点分前缀 `dotted`。
  新层级：罗马数字 = 0（与「第X章」同级），字母编号 = 6；`heading_level_map` 不再在 5
  截断，`render` 见到超过 5 的降成列表。
- `structure::restructure_with_report(markdown, options) -> (String, Report)`：原来的
  `restructure` 签名与行为不变，内部转调。`Report` 只带同级跳号（`NumberJump{from,to}`，
  用原文编号写），点分编号只在同一个父前缀下比、圈号不比、重新开始的编号不算跳号。
- 段内枚举：`split_enumeration` 改走 `enumeration_starts` / `enumeration_lead_len`，
  认出 `一是`、`第X，`、`首先` / `其次` / `再次` / `最后`；`第一条`、`第一章` 不在此列。
- 粘贴管线：`Context` 加 `kind`（文种）与 `section_level`（插入点上方最近标题级数），
  `research` 变成由 `kind` 推出来的方法；`Stats` 加 `number_jumps`（不再 `Copy`），
  `Stats::notice` 追一句跳号提示（最多列三处）。
- `headings.rs`：把 anydoc 出来的样式标题整体往下平移（`structure::marker_scheme` /
  `marker_level` 判断「这一行有没有编号」），超过 `#####` 转 `**标题**`。
- `lists.rs`：把 `- ` 列表整理成紧凑两级（列表前空行、子项缩进两格、第三级并进第二级）；
  白头件、红头呈批件（`TemplateKind::is_approval`）的列表项转成独立段落。
- `pipeline::convert_inline`：粘在句子中间时把多行压成一行行内文字（`#`/`- `/`> ` 记号去掉、
  原文编号与加粗保留、中文直连西文补空格、`normalize_spaces` 收尾），含表格返回 `None`。
- `paste.rs`：按插入点是「行首空白」（块级）还是「前面有字」（行内）分流；行内粘贴用选区
  或光标处替换、不补空行，同样只占一个撤销点、同样有「改为原样粘贴」。
- 夹具：`dotted-report.html`、`jump-numbers.html`、`web-headings.html`、
  `whitepaper-list.html`；后两份另有专门的上下文测试（粘到 `##` 下面、白头件不收列表）。

#### 与方案的出入

1. ~~点分方案的判定比方案松一档~~（跳号也算衔接、不要求上级存在）。验收时收紧了：这样会把
   「10.1 国庆假期安排」「10.8 恢复上班」的日期、「1.2 个百分点」「1.5 个百分点」的数量当编号删掉。
   现在要有一对严格衔接、上级与顶层都有着落（见第五节），跳号仍能在别处有严格衔接时报出来。
   见下文「第二期验收记录」。
2. **计量单位表只收不会当标题开头的单位**：个、项、家、人、分、时、年、月这些单字量词
   不收。它们同时是常见词的首字，收进来会把「2.2分析框架」「3.1项目管理」判成数量
   （清洗还会顺手删掉编号后的空格，连「2.2 分析框架」也躲不过）。
3. **字母编号压不进 `#####` 时按列表排**：公文助手的 Markdown 只有 `1. `/`- ` 两种列表，
   没有「A.」标记，所以降级后按 `1. 2. 3.` 编号，原文的字母不再出现。
4. **平移是整段等比平移，不是逐个夹到下限**：方案说「使最浅的一个落到 `##` 或更深」，
   实现就是整体加同一个 delta；已经比下限深的不往上提。
5. **平移在 anydoc 出来的 Markdown 上做**（方案给的另一个选项是用 `structure` 的报告区分
   「样式标题」与「编号升出来的标题」）。选前者是因为平移后 `structure` 直接看到目标层级，
   而且 `structure` 只需暴露一个「这行有没有编号」的查询（`Scheme` / `marker_level`），
   不用把光标概念带进去。
6. **行内粘贴在渲染后的 Markdown 文本上收尾**（`#`/`- `/`>` 记号、段落拼接），没有走
   模型层：这一段要的就是「一行文字」，模型里的块结构反而要再拆一遍。
7. **列表整形只动 `- ` 开头的行**：`1. ` 是 `structure` 自己排出来的列表（已经紧凑），
   也可能是它认别的编号时的写法，不能碰。

#### 已知坑（第三期接手前必读）

- **编号方案是整段文字定的，按单行认不出来**：`Numbering::detect` 会先剥掉行首的 `#`
  （Word 标题样式里的「1.1」在 `##` 后面），别在检测前把标题行滤掉。
- **清洗会删掉编号后的空格**：「1.1 标题」到 `parse_prefix` 时只剩「1.1标题」，所以
  `dotted_prefix` / `bare_number_prefix` 必须接受「编号直接跟汉字」。
- **`Numbering` 只小写一份在 `structure/numbering.rs`**：`Scheme` 是它的公开包装，给
  `rich_paste::headings` 用；不要另写一份方案判定，否则两处会打架。
- **跳号报告在 `resolve_ambiguous_levels` 之后算**：Word 自动编号的「（N）」定级要先落定，
  否则同一个编号会被算到两个层级上。
- **`Stats` 不再是 `Copy`**：加字段时注意调用方是否按值搬过它。
- **行内粘贴只在「内容多行」时走到**：单行的门槛在 `paste.rs`，`convert_inline` 自己不判。
- **列表整形的空行规则**：只把「后面还是列表项」的空行当组内分隔删掉；列表项里带第二段
  正文时（`- 第一项` 后空行再缩进两格的正文）保留空行，否则那一段会被解析器当成子项。
- **计量单位表**：`NUMBER_UNITS` 的取舍见上文；加单位前先想「这个字会不会是标题第一个字」。
  单字量词不收，靠的是方案判定够严（第五节），别为了认更多点分编号把判定放宽。
- 帮助与设计文档都已按实现更新；技能包语法没变，不用改。

#### 第二期验收记录（2026-10-23）

审阅了五个提交，`fmt`、`clippy -D warnings`、全量测试（2281 个）都通过，导入样例输出未变。
探针实测后修了下面几处（随验收提交）：

1. **点分编号认错、删掉数字**：「10.1 国庆假期安排」「10.8 恢复正常上班」变成两个 `###`，日期
   没了；「1.2 个百分点…」「1.5 个百分点…」同样丢了数字，还报了「跳号 1.2→1.5」。`numbering.rs`
   的 `dotted_scheme` 改成严格衔接 + 上级 / 顶层有着落，新增 `top_candidate` 收集「3 标题」这类
   顶层行；`structure/tests.rs` 加了 `dates_and_quantities_are_not_dotted_numbering`。
2. **行内粘贴把占位符写进正文**：HTML 那条路的文字带着 `gaps` 的 U+FDD0，`inline_text` 没换回来；
   纯文本那条路又会把「张某　联系电话」接成一串。`inline_text` 也护一遍、换回来，加了测试。
3. **会议议程粘出 `- ` 列表**：`validator` 要求议程用「1. 事项」、从 1 连续、不用项目符号。
   `lists.rs` 给 `MeetingAgenda` 拉平成连续编号的 `1. `。
4. 使用帮助补了点分编号「宁可不认」的几种情形与会议议程的列表。

确认过、没改的：「第一，」「一是」每行一个时不加粗（同一段里有两个以上才拆行加粗），与导入一致；
行内粘贴段落之间直接相连（第二期提示词的规定，读起来会挤，有「改为原样粘贴」兜底）。

**仍待真机验收**：第一期清单之外，再加——粘在 `##` 一节下面的网页文章层级、白头件 / 会议议程里
粘列表、句中粘贴的效果、跳号提示。

#### 第三期从哪接手

- 图片入库：`syntax.rs` 里丢 `Inline::Image` 的那一行接 `src/doc_import/embedded.rs` 的
  入库路径，处理 `data:` 与 Word 的 `clip_image*.png` 临时文件；“含表格时不作行内粘贴”的
  判断在 `syntax::has_table`，图片同理可以加一个 `has_image` 决定要不要接管。
- Word 公式：`syntax.rs` 现在按文种决定留不留 `Inline::Math` / `Block::Math`，Word 的
  OMML 要先在 `html.rs` 里转成 MathML 再交给 anydoc。
- `> [!文框]` 的半自动建议：引用块已经按文种保留 / 拆段，建议层单独做。
- AI 侧栏粘贴：`src/ai_panel/composer_ui.rs` 目前只把大段文本转成材料，富文本表格「不散」
  可以复用 `rich_paste::convert`（材料是纯文本，需要另一条 ToString 的出口）。
- 字体线索消歧：需要从 `html.rs` 另开一条通道把字体传给 `structure`，前面两期已经把
  编号与层级定了，先拿真实样本看有没有字体能救的情形再做（已从第三期单列为「待定」，见第十节）。

### 第三期（图片、公式、文框、侧栏材料）

- 2026-10-23：**第三期部分完成**。文框识别与 AI 侧栏材料已实现、待真机验收；**图片入库与 Word
  公式按提示词第 0 节先停下**——仓库里没有 `real-word-image.html`、`real-word-equation.html`、
  `real-web-image.html` 这三份真实剪贴板样本，输入格式（Word 图片的 `file:` 临时路径与 VML 双份、
  WPS 的写法、Word 公式的条件注释与 OMML 写法）只能看真样本，凭印象写解析会错。采样命令见
  `tests/fixtures/paste/README.md`：

  ```bash
  GONGWEN_PASTE_SAMPLE=real-word-image cargo test --locked capture_clipboard_html -- --ignored --nocapture
  GONGWEN_PASTE_SAMPLE=real-word-equation cargo test --locked capture_clipboard_html -- --ignored --nocapture
  GONGWEN_PASTE_SAMPLE=real-web-image cargo test --locked capture_clipboard_html -- --ignored --nocapture
  ```

  采样后脱敏（单位、人名、图片内容）再补 `.expected.md`，然后按第十节第 3 条接着做。

#### 做了什么

- `html.rs`：`parse(source, boxes)` 多一个「要不要认文框」的开关（只有研究报告为真）。
  文框候选三类——`<aside>`、只有一格的 `<table>`、`style` 里有 `border` / `background` 的
  `div` / `section`；判定「第一段是整段加粗的短标题」后，把候选改写成 `blockquote`，首段换成
  `\u{FDD1}名称\u{FDD2}标题` 占位。`Parsed` 带回文框个数与其中默认名称的个数。
- 文框首行最后才成形：anydoc 渲染会转义 `[`，`clean` 又会把半角 `!` 转成全角 `！`，所以模型里
  只放非字符占位，`html::restore_boxes` 在整条管线**最末尾**（结构还原、列表整形之后）换回
  `> [!名称] 标题`。行内粘贴遇到文框只留标题。
- `syntax.rs` 不用改：研究报告本来就保留引用块，其余文种照旧拆成段落。
- `Stats` 加 `boxes` / `default_boxes`；`notice` 在用了默认名称时补一句「识别出 N 个文框…」。
- AI 侧栏：`rich_paste::material_text(plain)` 有 HTML 就用 `pipeline::to_material` 转出来的
  Markdown（表格是管道表），没有或失败就退回纯文本；`composer_ui.rs` 的大段粘贴改调它。
  `to_material` 只做 HTML → Markdown 与语法裁剪（研究报告规则），不做编号重排、剥版头版记、
  认标题、列表整形、图片入库。`src/agent/` 一行没动，三条红线不受影响（材料仍是全文，闸门照常核对）。
- 夹具：`research-box.html`（四种来源 + 两个反例，另有研究报告上下文的测试）、
  `material-table.html`（`material-` 前缀走材料那条路）。单测覆盖名称前缀、默认名称与提示、
  非研究报告不出文框、材料保留表格且不重排编号、`file:`/`data:` 之外不读（后者随图片一项待做）。
- 使用帮助与 `AGENTS.md` 都按「文框、侧栏材料已做；图片与公式待采样」写。

#### 与方案的出入

1. **图片与公式没做**（缺真实样本，见上）。方案第四节那两行按现状标注为「待样本」。
2. **文框候选的判定放在 `html.rs`**（DOM 层），不在 `syntax.rs`（模型层）：模型里候选与真正的
   `<blockquote>` 都是 `Block::BlockQuote`，分不出「这一段网页旁注」与「作者写的引文」；DOM 层
   才知道来源，也才知道名字前缀要剥到哪儿。
3. **单格表格当文框时不再留表格结构**：把单元格里的块提上来直接做引用块内容，避免
   `tbody/tr/td` 在引用块里被 anydoc 当行内容器走。
4. **文框首行用非字符占位**（`U+FDD1`/`U+FDD2`）而不是直接写 `[!名称]`：渲染器的转义规则
   （`[` 后面有 `]` 就转义）与清洗的半角标点归一都会把它改坏。占位最后一步才换回来。
5. **`to_material` 用研究报告的语法规则**（保留引用块与公式、单格表格拆段），材料不分文种；
   图片在材料里照旧按未导入跳过。
6. **`TITLE_MAX` 从 `structure` 公开成 `pub(crate)`**，文框短标题的长度口径与结构还原一致。

#### 已知坑（补做图片 / 公式时必读）

- **`html::parse` 的 `boxes` 参数**：只有研究报告为真；材料与行内粘贴都为假。别把
  「要不要认文框」塞进 `Context` 的别处。
- **文框占位不能提前换回来**：`clean::normalize_punctuation` 会把 `[!` 里的 `!` 变全角，
  `structure`/`lists` 之前换回来都会坏；`pipeline::restructure` 的返回值是唯一正确的位置。
- **`entirely_bold` 的口径**：只看文字节点，`<br>` 这类空元素不算内容；`font-weight`
  小于 600 或 `normal` 会把外层的粗体取消（网页常见的 `<b style="font-weight:normal">`）。
- **图片那一期要注意**：`html.rs` 现在一遇到 `<img>` 就 `continue`（只计数）；接 `file:` /
  `data:` 时要在同一个分支里取字节，并记住 Word 的 VML `<v:imagedata>` 已经在 `should_skip`
  里按「带冒号」丢掉，只留 `<img>` 那一份，别重复入库。入库走 `embedded::render`（它已经处理
  格式识别、去重、图元文件转换、独占一行、未导入注释、alt 置空）。
- **单行带图的接管**：`paste.rs` 的「只有一行不接管」在图片那一期要改成「先读 HTML，有 `<img>`
  才接管」；egui-winit 在剪贴板里没有文字时不发 `Event::Paste`，只复制一张图做不到，别改它。
- **材料入口不做结构还原**：`to_material` 要的就是原文，任何「顺手整理一下」都会改变材料。

#### 还剩什么

1. 图片随粘贴入库（等 `real-word-image.html`、`real-web-image.html`；WPS 有更好）。
2. Word 公式（等 `real-word-equation.html`）。
3. 只复制一张图时的粘贴（egui-winit 不发事件，属平台限制，写进帮助即可）。
4. 字体线索消歧（待真实样本，见第十节第 4 条）。
