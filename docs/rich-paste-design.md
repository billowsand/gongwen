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
2. **第二期**：点分编号方案与小数区分；罗马数字与字母编号；跳号提醒；字体线索消歧；按光标所在
   章节定层级；光标在行中时只保留行内格式；按文种裁剪列表（白头件、呈批件）。
3. **第三期**：Word / 网页 `data:` 图片入库；Word 公式；`> [!文框]` 的半自动建议；AI 侧栏粘贴。

## 十一、进度与交接

- 2026-10-23：方案定稿，四项决策见第一节。第一期提示词见 `docs/rich-paste-phase1-prompt.md`。
  **第一期未开始。**
