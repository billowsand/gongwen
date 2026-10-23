# 任务：富文本粘贴转 Markdown · 第三期

你在 `gongwen`（公文助手，Rust + egui 离线桌面应用）仓库里工作，分支 `main`。前两期已经让起草页
Ctrl+V 能把 Word / 网页的富文本和多行纯文本转成 Markdown，编号、层级、列表、句中粘贴都处理了。
第三期补四件事：**图片随粘贴入库**、**Word 公式**、**研究报告文框的半自动识别**、**AI 侧栏粘贴
保留表格**。

## 0. 前置条件：真实剪贴板样本

这一期的输入格式（Word 图片的临时文件路径、Word 公式的 OMML 写法、WPS 的写法）只能看真样本，
**不要凭印象写解析**。开工前检查 `tests/fixtures/paste/` 里有没有下面这些以 `real-` 开头的样本
（用户用 `capture_clipboard_html` 采的，已脱敏）：

| 样本 | 内容 |
|---|---|
| `real-word-image.html` | Word 里一段文字 + 一张插入的图片，一起复制 |
| `real-word-equation.html` | Word 里一段文字 + 一个公式编辑器写的公式（行内或独立一行） |
| `real-web-image.html` | 浏览器里一段带图片的网页正文 |
| `real-wps-image.html` | WPS 文字里带图片的一段（有就用，没有不阻塞） |

缺前三份中的任何一份：对应的那一项（1.1 图片 / 1.2 公式）**先停下**，在汇报里请用户采样（采样命令
见 `tests/fixtures/paste/README.md`），别的项照做。样本里如果出现了你没料到的写法，以样本为准，
并把写法记进交接的「已知坑」。

## 1. 先读这些

1. `AGENTS.md`：约定、常用命令、**三条红线**、提交规范。代码注释、文档一律中文。
2. `docs/rich-paste-design.md`：方案全文。第四节是格式层映射，第十节第 3 条是本期范围，**第十一节
   两期的交接、已知坑、验收记录全部读完**。本提示词与方案有出入时按方案来，并在交接里写明。
3. `src/rich_paste/`（全部）、`src/draft_page/paste.rs`。
4. `src/doc_import/embedded.rs`（文档导入的图片入库：按字节认格式、同一份字节只存一次、EMF/WMF
   转 PNG、图片挪到独占一行、收不了的留 `<!-- [图片未导入：原因] -->`、alt 一律留空），
   `src/images.rs`（`import_bytes`、`is_supported`），`src/file_limits.rs`。
5. `vendor/anydoc/README.md`（已有两处补丁）、`vendor/anydoc/src/lib.rs` 的 `pub mod html`、
   `vendor/anydoc/src/shared/html.rs`（`HtmlCtx::image_source`）、`vendor/anydoc/src/shared/math/omml.rs`、
   `vendor/anydoc/src/shared/assets.rs`（EPUB 是怎么把图片字节放进 `Document::assets` 的）。
6. `skills/gongwen-markdown/references/research-report.md`：图片（alt 即图题）、公式、引用与文框
   （`> [!名称] 标题`）的写法。
7. `src/ai_panel/composer_ui.rs` 约 85–115 行与 `src/ai_panel/paste.rs`：AI 侧栏把大段粘贴转成材料。
   再读 `docs/ai-architecture.md` 的红线部分。

## 2. 本期范围

### 2.1 图片随粘贴入库

目标：Word / WPS / 网页里和文字一起复制的图片，粘贴后入库并在正文里成为 `![](images/…)`，
走的是**文档导入同一套**入库逻辑，不另写一份。

- **来源**，在 `html.rs` 里解析 `<img src>`（现在是直接丢掉并计数）：
  - `file:` URL（Word / WPS 的剪贴板临时文件，如 `msohtmlclip1/01/clip_image001.png`）：解码成本机
    路径，必须是普通文件，先按 `MAX_IMAGE_BYTES` 卡体积再读；
  - `data:` URI（网页与部分编辑器）：只认 `image/*;base64`，解码后同样卡体积；
  - `http:` / `https:` 及其它：**一律不下载**（离线软件，也防止粘贴一段网页就向外发请求），按
    「网络图片」记为未导入。
  - Word 剪贴板里同一张图常同时有 VML（`<v:imagedata>`，已经因为带冒号被丢掉）和
    `<![if !vml]><img …><![endif]>`，只取 `<img>` 那一份，别重复入库。
- **入库**：在 `vendor/anydoc` 的 `pub mod html` 上加一个带图片解析回调的入口（例如
  `to_document_with_images(body, css, resolve: &dyn Fn(&str) -> Option<Vec<u8>>)`），把取到的字节放进
  `Document::assets`、图片引用成 `ImageSource::Asset`；取不到的是 `Unavailable`。然后**直接用
  `embedded::render`** 渲染（它负责按字节认格式、去重、图元文件转换、挪到独占一行、留未导入注释、
  alt 置空），不再自己调 `anydoc::document_to_markdown`。补丁改动记进 `vendor/anydoc/README.md` 与
  `THIRD_PARTY_NOTICES.md`；EPUB 的行为不变。
- `syntax.rs` 里丢 `Inline::Image` 的那一行改成保留；`Stats` 改成报「入库 N 张、未导入 M 张」，
  状态栏文案照导入的 `ImageReport::notice` 写法。
- 读本机文件只读 `<img>` 指向的那一个文件：不跟随目录、不展开通配符；读到的字节不是应用认的图片
  格式就按未导入处理（`embedded.rs` 已经按字节判断）。
- **接管门槛**：现在内容只有一行就不接管。剪贴板 HTML 里有 `<img>` 时，单行（甚至纯文本为空白）
  也要接管，否则图片就丢了。`paste.rs` 里调整判断顺序：单行时也读一次 HTML，有 `<img>` 才接管。
  注意 egui-winit 在剪贴板**没有文字**时根本不发 `Event::Paste`：只复制一张图（截图、网页里单独
  右键复制图片）这一版做不到，写进帮助的已知限制，不要去改 egui-winit。
- 句中粘贴（行内）遇到图片：和表格一样不接管（图片要独占一行）。
- 研究报告里 alt 是图题：`embedded::render` 已经把 alt 置空，原文图下的题注是单独一段，照常保留。

### 2.2 Word 公式

- 先看 `real-word-equation.html`：Word 把公式放在条件注释里（`<!--[if gte msEquation 12]>…<![endif]-->`，
  里面是 OMML，可能夹着 HTML 的 `<span>` / `<i>`），后面跟一份 `<![if !msEquation]>` 的图片或文字
  作为后备。以样本为准。
- 研究报告：把条件注释里的 OMML 转成 TeX，行内公式写 `$…$`、独立公式写成独占一行的 `$$…$$`
  （写法见技能包 `research-report.md`）。OMML → TeX 复用 anydoc 的 `shared/math/omml.rs`，在
  `pub mod html` 旁边再公开一个最小入口（例如 `omml_to_tex(xml: &str) -> Option<String>`，内部补上
  `m:` 命名空间声明再解析），同样记进补丁说明。转换成功时**丢掉后备图片**，别公式和图片各来一份。
- 其他文种不收公式：用后备内容——后备是图片就按 2.1 入库，是文字就留文字；都没有时留
  `<!-- [公式未导入] -->`。
- 公式转换失败（OMML 不完整、夹了认不出的标签）：研究报告里也退回后备内容，并在状态栏说一句。

### 2.3 研究报告文框的半自动识别

只在研究报告里做。网页的 `<aside>`、Word 里只有一格的表格（「文本框表格」，现在拆成正文段落）、
带边框或底纹的 `<div>`，如果**第一段是整段加粗的短标题**（不超过 `TITLE_MAX`），就转成文框：

```markdown
> [!专栏] 标题
>
> 正文……
```

- 名称：标题以「专栏」「案例」「例子」「做法」「延伸阅读」「知识链接」开头（可带编号与冒号，如
  「案例1：××」）时用那个词、标题去掉这个前缀；否则写「专栏」。
- 状态栏补一句「识别出 N 个文框，名称暂写「专栏」，请按需改成案例、做法等」（只有用了默认名称时才说）。
- 不满足条件的单格表格照旧拆成段落；非研究报告的文种照旧不出引用块。
- 「半自动」的意思是：程序按确定性规则先写好，名称与范围由用户核对，不做任何弹窗或二次确认。

### 2.4 AI 侧栏粘贴保留表格

- 现在 AI 侧栏输入框里粘贴大段文字（`paste::is_large`）会变成临时材料，内容是纯文本，网页 / Word
  表格粘进来就散成一堆制表符。
- 改成：剪贴板里有 HTML 时，材料内容用 `rich_paste` 转出来的 Markdown（表格是管道表、标题与列表
  还在）；转换失败或没有 HTML 时照旧用纯文本。小段粘贴（不成材料的）不变。
- 材料要的是**原文**：这条路不做编号重排、不剥版头版记、不认文档标题、不整理列表、不入库图片
  （图片按未导入计数即可）。在 `rich_paste` 里加一个专门的入口（例如 `to_material(html) -> Option<String>`），
  只做 HTML → Markdown 与语法裁剪（研究报告的规则即可，材料不分文种）。
- 只改输入侧的材料文本；不碰 `src/agent/`，不改闸门（`ai_guard` 等照常拿材料全文核对）。三条红线
  不受影响，交接里写一句确认。

### 不做

字体线索消歧（方案第十节第 4 条，待真实样本）；只复制一张图时的粘贴（egui-winit 不发事件）；
任何网络下载。

## 3. 约束

- 三条红线（`AGENTS.md`）不碰：粘贴不调用模型；2.4 只改材料文本。
- 不写 `unsafe`；读本机文件、解 base64 都先卡体积（`file_limits`）；任何失败都退回原样粘贴或
  「未导入」注释，不 panic、不丢剪贴板内容。
- 前两期修过的行为不能回退（方案第十一节两份验收记录里列的每一条都有测试）。
- `embedded::render` 是导入和粘贴共用的：改它要同时跑 `cargo test --locked doc_import`，
  `tests/fixtures/import` 的样例输出必须逐字不变。
- 跟着周围代码的风格写；单文件超过约 800 行就按功能拆。

## 4. 测试

- 单元测试：`file:` 与 `data:` 的解析与体积上限、网络图片不下载、VML 与 `<img>` 只入库一次、非图片
  字节按未导入、单行带图的 HTML 也接管、句中粘贴遇图不接管；OMML → TeX（研究报告）、后备内容（公函）、
  转换失败退回；文框的四种来源、名称前缀、默认名称与提示、非研究报告不出文框；材料入口保留表格、
  不重排编号。
- 入库用临时目录（参照 `images::import_bytes_into` 与 `embedded.rs` 的测试写法），**测试不能往用户的
  图片资源库里写文件**。
- 夹具：第 0 节的真实样本各配一份 `.expected.md`（入库的文件名带时间戳，比对前先把
  `images/…` 路径归一成占位符）；再补两份合成样本：研究报告文框、AI 材料里的表格。
- UI 测试（`src/draft_page/paste.rs`、`src/ai_panel/ui_tests.rs`）：带图粘贴后正文出现图片引用、
  一次 `Ctrl+Z` 撤回；侧栏粘贴带表格的 HTML 后材料里是管道表。

## 5. 验收

依次跑，全部通过（偶有与沙箱环境相关的失败是预期的，**不要为此改测试**，在交接里逐个列名并确认
与本次改动无关）：

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo test --locked doc_import
cargo test --locked rich_paste
cargo test --locked draft_page::paste
cargo test --locked ai_panel
cargo test --locked help::
```

## 6. 文档

- `docs/rich-paste-design.md`：第四节的图片、公式、引用块行按实现更新；第十一节追加第三期交接
  （做了什么、与方案的出入、已知坑、还剩什么），**前两期的交接与验收记录不要删改**。
- `AGENTS.md`「进行中的工作」里富文本粘贴那一条改成「三期均已完成，待真机验收」。
- 使用帮助 `assets/help/06-editor.md` 的「从 Word、网页粘贴」：图片随粘贴入库（网络图片不下载、
  只复制一张图时粘不进来）、研究报告的公式与文框、AI 侧栏粘贴保留表格。改完跑 `cargo test --locked help::`。
- 技能包语法没变，不用改。

## 7. 提交

中文 Conventional Commits，分开提交，**不要 push**：

1. `chore: anydoc 的 HTML 入口支持图片与 OMML 公式`
2. `feat: 粘贴时图片随文字入库，网络图片不下载`
3. `feat: 粘贴 Word 公式，研究报告认文框`
4. `feat: AI 侧栏粘贴的材料保留表格`
5. `docs: 富文本粘贴第三期交接与使用帮助`

## 8. 做完后汇报

用中文简短汇报：每次提交的哈希与内容；哪些项因为缺样本没做、需要用户采哪几份；真实样本里和预期
不一样的写法；与方案的出入；验收命令结果（失败的测试逐个列名）；需要用户真机验收的清单。
