# 任务：富文本粘贴转 Markdown · 第一期

你在 `gongwen`（公文助手，Rust + egui 离线桌面应用）仓库里工作，分支 `main`。这一期要做的是：
在起草页的 Markdown 编辑框里按 Ctrl+V 时，把 Word / WPS / 网页 / Excel 复制来的富文本
（剪贴板里的 HTML）和多行纯文本自动转成公文助手的 Markdown，再插到光标处。

## 0. 先读这些，读完再动手

1. `AGENTS.md`：项目约定、常用命令、红线、提交规范。**代码注释、文档一律中文。**
2. `docs/rich-paste-design.md`：本功能的完整方案。第一节是已定决策，第十节第 1 条是本期范围。
   **以它为准**；本提示词和它有出入时按方案来，并在交接里写明。
3. `src/doc_import/structure.rs` 文件头注释与 `Options`；`src/doc_import/clean.rs` 文件头注释。
   编号转标题已经在这里做好了，本期**复用，不重写**。
4. `vendor/anydoc/README.md`（已有补丁的写法）、`vendor/anydoc/src/lib.rs`、
   `vendor/anydoc/src/shared/html.rs`（`to_blocks`、`HtmlCtx`、`Stylesheet`）、
   `vendor/anydoc/src/package/xml.rs`（`Element` / `Node` / `Attr`）、`vendor/anydoc/src/model/`。
5. `src/ai_panel/composer_ui.rs` 约 85–115 行：在 TextEdit 之前拦截 `Event::Paste` 的现成写法。
6. `src/draft_page/editor.rs` 约 890–1000 行：起草页编辑框 `egui::TextEdit::multiline(text)`，
   id 是 `editor_id()`。
7. `src/draft_page/markdown.rs`：`editor_selection`、`editor_cursor`、`insert_block`、`blank_line_padding`；
   `src/draft_page/diff_editor.rs`：`replace_with_undo`（前后各记一个撤销点）。
8. `src/draft_page/ribbon.rs` 的 `apply_clean`：只读与 AI 工作区视图的守卫写法，以及改完调
   `self.revalidate()`、写 `*self.status` 的写法。`src/draft_page/tasks.rs` 的
   `insert_imported_markdown`：插入后打开结果抽屉的写法。
9. `src/file_limits.rs`；`docs/security-hardening.md` 的要点。
10. `src/doc_import.rs` 末尾的样例测试（`tests/fixtures/import/*.expected.md` 与 ignored 的
    `dump_import_sample_outputs`）：夹具测试照这个样子写。

## 1. 本期范围

### 1.1 依赖

- 根 `Cargo.toml` 加直接依赖 `arboard`，版本与 `Cargo.lock` 里现有的 3.6.1 对齐，
  `default-features = false`，不要引入新的系统库（Linux 只用它已有的 X11 后端；
  不开 `wayland-data-control`）。
- 加一个容错 HTML 解析器。优先 `scraper`（内部是 html5ever），也可以直接用 `html5ever` 加一个
  简单的 DOM；选哪个都要在交接里写理由。许可证必须与 GPL-3.0-or-later 兼容（MIT / Apache-2.0 可以）。
- `Cargo.lock` 一并更新、提交。之后所有命令都带 `--locked`。

### 1.2 anydoc 补丁：公开 HTML 入口

`anydoc` 的 `package`、`shared` 模块是私有的。照现有补丁的做法，在 `vendor/anydoc/src/lib.rs`
新增**最小的**公开入口，例如：

```rust
/// gongwen patch：公开 HTML 元素树 → 文档模型，供剪贴板粘贴使用。
pub mod html {
    pub use crate::package::xml::{Attr, Element, Node};
    /// `css` 是页面里收集到的样式表文本（可以为空）。链接只留文字，图片一律不取。
    pub fn to_document(body: &Element, css: &str) -> Result<crate::model::Document, crate::ConvertError>;
}
```

- 内部实现一个 `HtmlCtx`：`link_target` 返回 `None`（只留链接文字，先确认 `to_blocks` 里 `None`
  确实是这个效果）；`image_source` 返回 `Ok(None)`；`anchor_id` 按原样构造。
- 不改 `to_blocks` 本身的行为，EPUB 的输出必须不变。
- 更新 `vendor/anydoc/README.md` 的补丁清单（改动处、为什么、升级时怎么重打），以及
  `THIRD_PARTY_NOTICES.md` 的 anydoc 一节里对补丁的描述。
- 单独一次提交。

### 1.3 转换模块 `src/rich_paste/`

新建模块文件夹（在 `src/main.rs` 登记 `mod rich_paste;`，注意 `mod` 声明不全在文件头，先
`grep -n "^mod " src/main.rs`）。建议的拆分（可以调整，单文件别超过约 800 行）：

- `mod.rs`：入口与类型。
  ```rust
  pub(crate) enum Source { Html { html: String, plain: String }, Plain(String) }
  pub(crate) struct Context { pub research: bool, pub draft_empty: bool, pub has_title: bool }
  pub(crate) struct Outcome { pub markdown: String, pub stats: Stats }
  pub(crate) struct Stats { pub headings: usize, pub tables: usize, pub list_items: usize, pub skipped_images: usize }
  /// 返回 None 表示不必转换（照原样粘贴）。
  pub(crate) fn convert(source: &Source, ctx: &Context) -> Option<Outcome>;
  ```
- `clipboard.rs`：`read_html() -> Option<String>`，用 arboard 读 HTML；读不到、出错、超限都返回
  `None`。在 `src/file_limits.rs` 加一个上限常量（建议 8 MiB，带中文注释说明为什么）。
  **测试里不能碰真剪贴板**：留一个 `#[cfg(test)]` 的覆盖点（例如 thread_local），让 UI 测试能塞假 HTML。
- `html.rs`：HTML 字符串 → `anydoc::html::Element` 树，加预清洗：
  - 丢掉 `script`、`style`（但把 `<style>` 文本收集起来作为 `css` 传给 `to_document`）、`head`、
    `noscript`、`template`、`iframe`、`object`、注释、`o:p` 等带命名空间前缀的 Office 元素；
  - 丢掉行内样式含 `mso-hide:all` 或 `display:none` 的元素；
  - **保留** `mso-list:Ignore` span 里的文字：那就是 Word 自动编号（「一、」「（一）」「1.」），
    structure 靠它定级；只把其中的 `&nbsp;` 空白归一；
  - `class` 含 `MsoTitle` 的段落当 `h1`；
  - 段首的 `&nbsp;`、全角空格「　」缩进去掉；去掉零宽字符；
  - 限制节点数与嵌套深度（常量写在模块里，带注释），超限返回错误，由上层退回纯文本。
  - Windows 上 arboard 已经只返回 StartFragment…EndFragment 的片段；macOS / Linux 是整页。
    两种都要能处理（没有 `<body>` 时把整个片段当 body）。
- `syntax.rs`：在 anydoc 的**文档模型**上按文种裁剪语法，再调 `anydoc::document_to_markdown`。
  不要在 Markdown 文本上用正则做这件事。规则见 `docs/rich-paste-design.md` 第四节：
  - 斜体：去标记留文字；删除线（`del` / `s`）：连文字丢；链接：留文字；
  - 图片：丢掉并计数到 `skipped_images`；
  - 引用块：研究报告保留，其余文种拆成普通段落；
  - 代码块：拆成普通段落；数学：研究报告保留，其余丢；
  - 只有一格的表格拆回段落；
  - 统计 `tables`。
- `pipeline.rs`（或放 `mod.rs`）：
  1. HTML 来源：`html.rs` → `to_document` → `syntax.rs` → Markdown；任何一步失败就当纯文本走。
  2. 纯文本来源：如果**已经像 Markdown**（有 `#{1,6} ` 标题行、`<!-- [` 标记、管道表的分隔行、
     代码围栏），返回 `None`，原样粘贴。
  3. `clean::clean_before_structure` → `structure::restructure(&cleaned, &options)` → `clean::tidy`。
  4. 粘贴专用选项：在 `structure::Options` 上加一个构造函数 `Options::paste(...)`，不要改现有两个：
     - `detect_title = !ctx.has_title`；`strip_layout = ctx.draft_empty`；
     - `lines_are_paragraphs`：纯文本 `true`，HTML `false`；
     - `bold_unnumbered = true`；`inline_enumeration = true`；`repair_merged_paragraphs = false`。
  5. 结果和原文（纯文本来源）只差空白时返回 `None`。
  6. 统计 `headings`、`list_items`，交给状态提示。
- **不做**（第二期）：点分编号 1.1 / 1.1.1、罗马数字、字体线索、按光标所在章节定级、
  按文种裁剪列表、光标在行中时只保留行内格式。`structure.rs` 的编号规则本期不改。

### 1.4 起草页接入

在起草页编辑框 `TextEdit::multiline(text)` **之前**（同一帧，参照 composer_ui 的写法）拦截：

- 只在 `editor_id()` 有焦点、`editable`、`!self.doc.read_only()`、
  `self.doc.ai_panel.workspace_view.is_none()` 时接管。
- 遇到 `egui::Event::Paste(text)`：
  - `input.modifiers.shift` 为真（Ctrl+Shift+V / ⌘⇧V）→ 不接管，交给 TextEdit 原样粘贴。
    egui-winit 0.36 把 `command && V` 直接换成 `Paste` 事件且不再发 V 的 Key 事件，只能这样区分；
    Windows 的 Shift+Insert 因此也会走原样，这是已知限制，写进帮助和交接。
  - 内容只有一行，或光标所在行**光标前**有非空白字符 → 不接管。
  - 否则读剪贴板 HTML：有就用 `Source::Html { html, plain: text }`，没有就用 `Source::Plain(text)`；
    `convert` 返回 `None` → 不接管。
  - 接管时把这个事件从 `input.events` 里删掉，然后：取 `editor_selection`（没有就用光标，再没有就文末），
    用转换结果替换这段，前后按 `blank_line_padding` 补空行，通过 `replace_with_undo` 写回
    （一个撤销点，光标落在插入内容末尾），再 `self.revalidate()`；有警告或修订时照
    `insert_imported_markdown` 打开结果抽屉。
- 状态提示：`已按格式粘贴：3 个标题、1 张表、5 个列表项（略过 2 张图片）。`，数量为 0 的项不写。
- 「改为原样粘贴」：在稿件状态里记一条最近的智能粘贴（插入的字节范围、插入的文字、原始纯文本）。
  在状态栏或编辑区上方合适的位置画一个小按钮（先看现有状态栏怎么画的，跟着风格走）。按钮在下一次
  编辑、换稿件或约 15 秒后消失。点击时先核对那段文字还在原处（`text.get(range) == Some(inserted)`），
  不在就提示「粘贴的内容已改动，无法还原」；在就用原始纯文本替换，同样走 `replace_with_undo`。
- 这里改的是用户的正式稿 `generated_markdown`，属于用户键盘输入，**不要**经过任何 AI 代码路径。

### 1.5 测试

- **单元测试**（各模块文件末尾 `#[cfg(test)] mod tests`），覆盖至少这些，HTML 照真实剪贴板的样子手写：
  1. Word 片段：`mso-list:Ignore` 里的「一、」「（一）」「1.」三级 → `##` / `###` / `####`（编号被去掉）；
  2. Word 标题样式 `<h1>`、`<h2>`，以及 `class=MsoTitle`；
  3. 表格：`colspan` / `rowspan`、`<th>` 表头；只有一格的表格拆成段落；
  4. 政府网站：`<p style="text-indent:2em">　　一、……</p>`、`<strong>` 包住的整行；
  5. 引用块：研究报告保留 `> `，公函拆成段落；
  6. 链接留字、斜体去标记、删除线连字丢、图片丢且计数；
  7. 嵌套列表、`<br>` 段内换行；
  8. 微信公众号式层层 `section` / `span`；Excel 表格 HTML；
  9. `script` / `style` / 注释被丢掉，`<style>` 里的 `font-weight:bold` 类规则能生效（整页 HTML）；
  10. 畸形 HTML（不闭合、乱嵌套）不 panic；超过节点数或深度上限时退回纯文本；
  11. 纯文本：微信复制的多行「一、……（一）……」被识别；已经是 Markdown 的返回 `None`；
      只差空白时返回 `None`；
  12. `Options::paste` 的 `detect_title` / `strip_layout` 随 `Context` 变化。
- **夹具测试**：`tests/fixtures/paste/<名字>.html` + `<名字>.expected.md`，配一个读目录逐个比对的测试，
  和一个 ignored 的导出测试（照 `dump_import_sample_outputs`）。先放 5–8 份合成样本，内容自拟、
  不得含真实单位和人名。目录里放 `README.md` 说明怎么采样本。
- **采样工具**：一个 ignored 测试 `capture_clipboard_html`，读当前剪贴板的 HTML，写到
  `tests/fixtures/paste/$GONGWEN_PASTE_SAMPLE.html`（环境变量缺失就 panic 说明用法）。用户会拿它
  从 Word、WPS、浏览器、Excel 采真实样本，脱敏后再补 `.expected.md`。
- **UI 测试**：用仓库里现有的 egui 测试写法（参考 `src/draft_page/editor.rs` 的 `text_drag_tests`
  与 `src/ai_panel/ui_tests.rs`）验证：有焦点时 Paste 被接管并转换；Shift 按着时不接管；单行不接管；
  一次 Ctrl+Z 回到粘贴前；「改为原样粘贴」能换回原文。

### 1.6 文档

- `docs/rich-paste-design.md` 第十一节写交接：做了什么、和方案的出入、已知坑、第二期从哪接手。
- `AGENTS.md`「进行中的工作」加一条，格式照其他条目（方案文件、已完成第几期、待真机验收）。
- 使用帮助（`assets/help/`，是一篇研究报告，写法见技能包 `skills/gongwen-markdown/references/research-report.md`）
  在讲起草页编辑的那一章加一小节「从 Word、网页粘贴」：Ctrl+V 智能转换、Ctrl+Shift+V 原样、
  改为原样粘贴、图片暂不随粘贴带入、Shift+Insert 的限制。改完跑 `cargo test --locked help::`。
- 技能包语法没变，不用改。

## 2. 约束

- 三条红线（`AGENTS.md`）不碰：本功能不调用模型，不经过 `src/agent/`、`src/ai_panel/`。
- 全 crate `#![deny(unsafe_code)]`，本期不写 `unsafe`。
- 不下载任何远程资源；解析失败一律退回原样粘贴并在状态栏说明，不能 panic、不能丢用户的剪贴板内容。
- 不改 `structure.rs` 现有规则的行为；`tests/fixtures/import` 的样例输出必须逐字不变。
- 不改 `anydoc` 的 EPUB 行为。
- 跟着周围代码的风格：注释密度、命名、写法。单文件超过约 2000 行要拆。

## 3. 验收

依次跑，全部通过（`cargo test` 约有 5 个沙箱相关的失败是预期的，**不要为此改测试**，但要在交接里
列出失败的是哪几个，确认和你的改动无关）：

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo test --locked doc_import
cargo test --locked rich_paste
cargo test --locked help::
```

然后 `cargo run -- --new-window` 起应用，自己能做的就做：往起草页粘一段多行「一、……（一）……」
纯文本，看是否转成标题。Word / WPS / 浏览器的真机粘贴由用户验收，交接里列出验收清单。

## 4. 提交

中文 Conventional Commits，分开提交，**不要 push**：

1. `chore: 引入 arboard 与 HTML 解析依赖，anydoc 公开 HTML 入口`
2. `feat: 富文本与多行纯文本转公文助手 Markdown`（转换模块 + 单元测试 + 夹具）
3. `feat: 起草页粘贴时自动转换格式，Ctrl+Shift+V 原样粘贴`（接入 + UI 测试）
4. `docs: 富文本粘贴第一期交接与使用帮助`

本地 post-commit 钩子会在每次提交后跑 release 构建，慢是正常的。

## 5. 做完后汇报

用中文简短汇报：每次提交的哈希与内容；选了哪个 HTML 解析库及理由；和方案的出入；验收命令的结果
（失败的测试逐个列名）；需要用户真机验收的清单；第二期接手要注意的坑。
