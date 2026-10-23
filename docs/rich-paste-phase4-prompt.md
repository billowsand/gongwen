# 任务：富文本粘贴转 Markdown · 第四期（图片入库与 Word 公式）

你在 `gongwen`（公文助手，Rust + egui 离线桌面应用）仓库里工作，分支 `main`。前三期已经让起草页
Ctrl+V 能把 Word / 网页的富文本和多行纯文本转成 Markdown，编号、层级、列表、句中粘贴、研究报告
文框、AI 侧栏材料都处理了。第三期里的**图片入库**与 **Word 公式**因为缺真实剪贴板样本停下了，
这一期补上。拆成两块：

- **A 块（不需要真实样本，必须做完）**：图片随粘贴入库。`<img>` 的 `data:` 与 `file:` 地址、网络图片
  不下载，这些都是 HTML 标准里写清楚的，合成样本就能测。
- **B 块（需要真实样本）**：Word 公式。Word 把公式放在条件注释里，写法只能看真样本。

## 0. 前置条件：真实剪贴板样本

检查 `tests/fixtures/paste/` 里有没有这些以 `real-` 开头的样本（用户用 `capture_clipboard_html`
采的，已脱敏；采样命令见同目录 `README.md`）：

| 样本 | 用途 | 缺了怎么办 |
|---|---|---|
| `real-word-image.html` | 核对 Word 图片的 `file:` 路径、VML 与 `<img>` 双份 | A 块照做，用合成样本；交接里写明「Word 图片未经真样本核对」 |
| `real-web-image.html` | 核对浏览器给的 `<img>`（`https:` / `data:`） | 同上 |
| `real-word-equation.html` | B 块的唯一依据 | **B 块整块不做**，在汇报里请用户采样 |
| `real-wps-image.html` | WPS 的写法 | 不阻塞 |

有真样本时以真样本为准；样本里出现你没料到的写法，记进交接的「已知坑」。

## 1. 先读这些

1. `AGENTS.md`：约定、常用命令、**三条红线**、提交规范。代码注释、文档一律中文。
2. `docs/rich-paste-design.md`：方案全文。第四节是格式层映射，第十节是分期，**第十一节三期的交接、
   已知坑、验收记录全部读完**，尤其是第三期交接里「已知坑（补做图片 / 公式时必读）」那一段。本提示词
   与方案有出入时按方案来，并在交接里写明。
3. `src/rich_paste/`（全部）、`src/draft_page/paste.rs`。注意 `html::parse(source, boxes)` 的 `boxes`
   参数、`html.rs` 里遇到 `<img>` 就 `continue` 的那个分支、`gaps.rs` 与文框占位符要在管线最末尾才换回来。
4. `src/doc_import/embedded.rs`（文档导入的图片入库：按字节认格式、同一份字节只存一次、EMF/WMF 转 PNG、
   图片挪到独占一行、收不了的留 `<!-- [图片未导入：原因] -->`、alt 一律留空）与它的测试；
   `src/images.rs`（`import_bytes`、`import_bytes_into`、`is_supported`）；`src/file_limits.rs`。
5. `vendor/anydoc/README.md`（已有两处补丁）、`vendor/anydoc/src/lib.rs` 的 `pub mod html`、
   `vendor/anydoc/src/shared/html.rs`（`HtmlCtx::image_source`）、`vendor/anydoc/src/shared/assets.rs`
   （EPUB 怎么把图片字节放进 `Document::assets`）、`vendor/anydoc/src/shared/math/omml.rs`。
6. `skills/gongwen-markdown/references/research-report.md`：图片（alt 即图题）与公式（`$…$`、独占一段的
   `$$…$$`）的写法。

## 2. A 块：图片随粘贴入库

目标：和文字一起复制的图片，粘贴后入库并在正文里成为 `![](images/…)`，走**文档导入同一套**入库逻辑
（`embedded::render`），不另写一份。

### 2.1 取图片字节（`src/rich_paste/` 新增一个模块，例如 `images.rs`）

- `data:` URI：用 `data-url` crate 解析（已经在 `Cargo.lock` 里，直接声明依赖即可，不新增下载），只认
  `image/*`；解码后按 `MAX_IMAGE_BYTES` 卡体积。
- `file:` URL：用 `url` crate（同样已在依赖树里）转成本机路径，处理好百分号编码（用户名是中文时路径
  里全是 `%E5…`）。**带主机名的一律拒收**（`file://server/share/…` 是 Windows 共享，读它会发网络请求），
  `localhost` 当作没有主机。必须是普通文件（`symlink_metadata` 看一眼，不跟随目录），先看体积再读
  （`file_limits::read_bytes(path, MAX_IMAGE_BYTES, …)`）。
- `http:` / `https:` / 其它：**一律不下载**，记为「网络图片」未导入。离线软件，粘贴一段网页不能向外发请求。
- 读到的字节不是应用认的图片格式就按未导入处理（`embedded.rs` 已经按字节判断格式，别重复写一份）。
- 一次粘贴最多取 50 张（常量，写注释），超过的按未导入计数并在状态栏说明。
- Word 剪贴板里同一张图常同时有 VML（`<v:imagedata>`）和 `<![if !vml]><img …><![endif]>`：VML 已经在
  `should_skip` 里按「元素名带冒号」丢掉，只留 `<img>` 那一份。用测试钉住「同一张图只入库一次」。

### 2.2 接进 anydoc 与入库

- 在 `vendor/anydoc` 的 `pub mod html` 上加一个带图片回调的入口（例如
  `to_document_with_images(body, css, resolve: &dyn Fn(&str) -> Option<Vec<u8>>)`）：取到字节的放进
  `Document::assets`、引用成 `ImageSource::Asset`；取不到的是 `Unavailable`。补丁记进
  `vendor/anydoc/README.md` 与 `THIRD_PARTY_NOTICES.md` 的 anydoc 一节；EPUB 的行为不变。
- `html.rs` 的 `<img>` 分支不再直接 `continue`：保留 `<img>` 元素交给 anydoc（计数仍然要有）。
- `syntax.rs` 里丢 `Inline::Image` 的那一行改成保留（材料与行内粘贴两条路仍然丢，见 2.4）。
- 起草页的块级粘贴改用 `embedded::render` 渲染（它负责格式识别、去重、图元文件转换、挪到独占一行、
  未导入注释、alt 置空），不再自己调 `anydoc::document_to_markdown`。渲染之后照旧走结构还原、列表整形、
  占位符还原。确认 `structure` / `clean` 不会把 `![](images/…)` 与 `<!-- [图片未导入：…] -->` 改坏
  （导入那条路已经这样用了，照着核对）。
- 入库用 `images::import_bytes`；**测试用 `import_bytes_into` 加临时目录**，不能往用户的图片资源库里写。
- `Stats` 改成报「入库 N 张、未导入 M 张」，状态栏文案照 `ImageReport::notice` 的写法；原来的
  `skipped_images` 语义要么并进去，要么改名，别留两个意思相近的字段。

### 2.3 接管门槛（`paste.rs`）

- 现在内容只有一行就不接管。剪贴板 HTML 里有 `<img>` 时，单行（纯文本为空白）也要接管，否则图片就丢了：
  单行时也读一次 HTML，含 `<img` 才接管，否则照旧交还编辑框。
- 只复制一张图（截图、网页里单独右键复制图片）时剪贴板里没有文字，egui-winit 根本不发 `Event::Paste`，
  这一版做不到；写进帮助的已知限制，**不要去改 egui-winit**。
- 句中粘贴（行内）遇到图片：和表格一样不接管（图片要独占一行）。

### 2.4 不随粘贴入库的两条路

- AI 侧栏材料（`to_material`）：材料是给模型看的纯文本，图片照旧按未导入跳过，**不入库**。
- 行内粘贴：见 2.3，遇图不接管。

### 2.5 已知的遗留（写进交接，不用解决）

- 「改为原样粘贴」或 `Ctrl+Z` 撤回后，已经入库的图片文件留在资源库里（文档导入也是这样）。
- 图片读取与 EMF 转换在界面线程上做：量一下 10 张常见尺寸图片的粘贴耗时写进交接；超过 300 ms 再
  考虑挪到后台，这一期不做。

## 3. B 块：Word 公式（只在有 `real-word-equation.html` 时做）

- 先看样本：Word 把公式放在条件注释里（一般是 `<!--[if gte msEquation 12]>…<![endif]-->`，里面是 OMML，
  可能夹着 HTML 的 `<span>` / `<i>`），后面跟一份 `<![if !msEquation]>` 的图片或文字作后备。**以样本为准**。
  注意 html5ever 把条件注释整体当注释节点，现在 `html.rs` 直接丢掉注释，要在那里先把内容取出来。
- **研究报告**：OMML 转 TeX，行内公式写 `$…$`，独立公式写成独占一段的 `$$…$$`。转换复用 anydoc 的
  `shared/math/omml.rs`，在 `pub mod html` 旁边再公开一个最小入口（例如
  `omml_to_tex(xml: &str) -> Option<String>`，内部补上 `m:` 命名空间声明再解析，先剔掉夹在里面的 HTML
  标签），同样记进补丁说明。转换成功时**丢掉后备内容**，别公式和图片各来一份。
- **其他文种**不收公式：用后备内容——后备是图片就按 A 块入库，是文字就留文字；都没有时留
  `<!-- [公式未导入] -->`。
- 转换失败（OMML 不完整、夹了认不出的标签）：研究报告里也退回后备内容，并在状态栏说一句。
- 公式里的 `$` 与反斜杠不能被 anydoc 的渲染器或 `clean` 转义坏：像文框那样先用非字符占位、管线最末尾
  再换回来，或者用别的办法，测试要覆盖。

## 4. 不做

字体线索消歧（方案第十节「待定」）；只复制一张图时的粘贴；任何网络下载；AI 侧栏材料里的图片。

## 5. 约束

- 三条红线（`AGENTS.md`）不碰：粘贴不调用模型、不经过 `src/agent/`。
- 不写 `unsafe`；读本机文件、解 base64 都先卡体积；任何失败都退回原样粘贴或「未导入」注释，
  不 panic、不丢剪贴板内容。
- `embedded::render` 是导入和粘贴共用的：改它要同时跑 `cargo test --locked doc_import`，
  `tests/fixtures/import` 的样例输出必须逐字不变。
- 前三期修过的行为不能回退（方案第十一节三份验收记录里列的每一条都有测试）。
- 跟着周围代码的风格写；单文件超过约 800 行就按功能拆。

## 6. 测试

- 单元测试：`data:` 解析、非图片 MIME、体积上限；`file:` 的百分号编码、带主机名拒收、`localhost`、
  目录与不存在的文件；网络图片不下载且计数；VML 与 `<img>` 只入库一次；超过 50 张；单行带图的 HTML 也
  接管；句中粘贴遇图不接管；材料不入库。B 块：OMML → TeX（研究报告）、后备内容（公函）、转换失败退回、
  公式里的 `$` 与反斜杠不被转义。
- 夹具：A 块至少两份合成样本（`data:` 图片的网页片段；`file:` 指向测试临时目录图片的 Word 式片段——
  文件路径在测试里生成，别把本机路径写死进夹具）。有真样本就各配一份 `.expected.md`，比对前把
  `images/…` 里带时间戳的文件名归一成占位符。
- UI 测试（`src/draft_page/paste.rs`）：带图粘贴后正文出现图片引用、一次 `Ctrl+Z` 撤回。

## 7. 验收

依次跑，全部通过（偶有与沙箱环境相关的失败是预期的，**不要为此改测试**，在交接里逐个列名并确认与本次
改动无关）：

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo test --locked doc_import
cargo test --locked rich_paste
cargo test --locked draft_page::paste
cargo test --locked help::
```

## 8. 文档

- `docs/rich-paste-design.md`：第四节的 `img` 与 `math` 两行按实现更新；第十节第四期一条标上进度；
  第十一节追加第四期交接（做了什么、与方案的出入、已知坑、还剩什么），**前三期的交接与验收记录不要删改**。
- `AGENTS.md`「进行中的工作」里富文本粘贴那一条按实际进度改。
- 使用帮助 `assets/help/06-editor.md` 的「从 Word、网页粘贴」：图片随粘贴入库（网络图片不下载、只复制
  一张图时粘不进来、撤回后图片文件留在资源库）；做了 B 块再写研究报告的公式。删掉「图片这一版还带不
  进来」之类过时的说法。改完跑 `cargo test --locked help::`。
- 技能包语法没变，不用改。

## 9. 提交

中文 Conventional Commits，分开提交，**不要 push**：

1. `chore: anydoc 的 HTML 入口支持图片回调`（做了 B 块再加 OMML 入口，可以并进这一条）
2. `feat: 粘贴时图片随文字入库，网络图片不下载`
3. `feat: 研究报告粘贴 Word 公式`（只在做了 B 块时）
4. `docs: 富文本粘贴第四期交接与使用帮助`

## 10. 做完后汇报

用中文简短汇报：每次提交的哈希与内容；B 块做没做、缺哪几份样本；真实样本里和预期不一样的写法；与方案的
出入；10 张图的粘贴耗时；验收命令结果（失败的测试逐个列名）；需要用户真机验收的清单。
