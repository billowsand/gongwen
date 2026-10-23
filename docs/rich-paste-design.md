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
| 编号 → 标题 / 列表 | `src/doc_import/structure.rs` | 第X章、一、（一）、1.、（1）、①；一是二是；附件；版头版记；Word 自动编号把「一、」转成「1、」（`from_list`）；层级压紧时第一层保持固定层级（只粘「（一）」仍是 `###`） |
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
| `blockquote`、Word「引用」样式 | 研究报告 `> `；其余文种拆成普通段落 | 公文不收引用块；`> [!案例]` 文框认不准，不自动生成 |
| `table` | 管道表 | 复用跨行跨列网格；只有一格的表格（Word「文本框表格」、网页排版用表格）拆回段落 |
| `a` | 只留文字 | 公文不收链接 |
| `img` | 第一、二期略过并计数 | 远程图片永不下载；第三期 Word 的 `clip_image*.png` 本机临时文件走 `embedded.rs` 入库 |
| `pre` / `code` 围栏 | 拆成普通段落 | 技能包规定正文不写代码围栏 |
| `math`（MathML） | 研究报告 `$…$`；其余文种丢 | 第三期再管 Word 公式 |
| `br` | 段内换行 | structure 按「硬换行」处理 |
| 噪音 | 删除 | `script` / `style` / `head` / `noscript` / `template` / `iframe` / 注释、`<o:p>`、`mso-hide:all`、零宽字符、段首 `&nbsp;` 与全角空格缩进「　　」 |

## 五、文本层：编号定级

**已有（structure.rs）**：第X章比一、浅一层；一、→`##`；（一）→`###`；1. →`####` 或列表；
（1）→`#####` 或列表；① → 列表；段内一是二是拆行并加粗引子；附件；版头版记。

**第二期新增：点分编号方案**

| 编号 | 层级 |
|---|---|
| `1` / `1.` / `1、`（点分方案下） | `##` |
| `1.1` / `1.1.` | `###` |
| `1.1.1` | `####` |
| `1.1.1.1` | `#####` |

- **先定方案再定级**：片段里至少出现两个点分编号且前缀前后衔接（1.1、1.2、2.1），整段切到点分
  方案，这时段首「1 标题」是 `##` 而不是 `####`。
- **和小数区分**：编号后跟半角 / 全角空格或直接跟汉字，但不能跟亿、万、元、%、倍、米等量词；整行
  不超过 `TITLE_MAX`；要有上一级（出现 2.3 前面得有 2 或 2.x）。
- structure 里的测试 `enumerations_and_decimals_are_not_numbering` 写死了「3.2.1 技术路线」
  不是编号：改成「孤立的 3.2.1 不是编号、成体系的点分编号是」，不能直接删。
- 罗马数字 Ⅰ、Ⅱ 当最浅一层；`A.` / `(a)` 字母编号；「第一，」「首先，」开头的段落归入一是二是那类
  段内枚举。
- 原文跳号（一、三、）时状态提示里说一句，因为编号由程序重新生成。
- 字体线索（黑体一级、楷体二级、仿宋加粗三级）只在有编号但层级模糊时消歧，不单凭字体升标题。
  **已挪到第三期**，原因见第十节。

## 六、粘贴和整篇导入的区别

| 情况 | 处理 |
|---|---|
| 内容只有一行 | 不接管，原样粘贴（复制一个词就是一个词） |
| 光标所在行光标前有非空白字符 | 第一期不接管；第二期改成只保留行内格式、压成一行 |
| 有选区 | 用转换结果替换选区 |
| 认文档标题（`detect_title`） | 稿子里还没有 `# ` 行时才打开 |
| 剥版头版记（`strip_layout`） | 只在稿子为空时打开（相当于整篇复制一份公文） |
| 纯文本 `lines_are_paragraphs` | 打开（即时通讯、记事本里一行就是一段）；HTML 来源关掉 |
| 纯文本已经是 Markdown（有 `#` 标题行、`<!-- [` 标记、管道表、代码围栏） | 原样粘贴 |
| 文种禁用的语法（白头件里的列表等） | 第二期按文种裁剪，第一期交给校验器提示 |
| 光标所在章节的层级 | 第二期：粘进 `###` 下面的「一、」是否降级，按光标上方最近的标题判断 |

## 七、交互

- 智能粘贴后：状态栏写「已按格式粘贴：N 个标题、M 张表、K 个列表项（略过 J 张图片）」，并给一个
  「改为原样粘贴」按钮。按钮在下一次编辑、换稿件或超时后消失；点它时先核对插入的那段原文还在原处
  （同 `PreviewAnchor::range_in` 的做法），再用纯文本替换，同样只占一个撤销点。
- Ctrl+Z 一次回到粘贴前。
- 已知限制：Windows 上 `Shift+Insert` 带着 Shift，会被当成原样粘贴。
- AI 侧栏输入框的粘贴（材料里的表格不散）放第三期。

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
3. **第三期**：Word / 网页 `data:` 图片入库；Word 公式；`> [!文框]` 的半自动建议；AI 侧栏粘贴；
   字体线索消歧（原排第二期，挪到这里：`anydoc` 的文档模型不带字体，要从 `html.rs` 另开一条
   通道传给 `structure`，而有编号时层级已经由编号样式决定，先拿真实样本看有没有字体能救的
   情形再做）。

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
   `structure.rs` 不依赖 `rich_paste` 的类型。

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

- 编号与层级全在 `src/doc_import/structure.rs`：`parse_prefix`、`resolve_ambiguous_levels`、
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
