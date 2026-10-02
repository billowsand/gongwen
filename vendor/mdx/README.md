# vendor/mdx

公文助手的研究报告文档类型由 mdx 提供：解析与区段规则、封面版式、Word 转换，以及给
公文助手 Typst 模板的排版数据（`typst_research`）。
这里是 mdx 源码**随本仓库分发的一份副本**，构建时以 `path` 依赖的方式接入
（见根 `Cargo.toml`）。

## 为什么不用 git 依赖

原先 `Cargo.toml` 直接 pin mdx 的某个 commit，构建因此依赖三件事同时成立：
GitHub 可达、仓库还在且公开、那个 commit 没有被 GC。任何一条不成立，`cargo build`
就直接失败——而 CI 每次都是干净环境，本地缓存救不了。收进仓库以后，从零克隆本仓库
即可离线构建，不再有这层外部依赖。

需要说清楚的是：这件事只影响**构建**。mdx 是 Rust 库，编译后静态链接进
`gongwen-assistant`，终端用户下载的可执行文件从来就不需要 mdx 存在。

## 来源

- 上游：<https://github.com/billowsand/mdx>
- 版本：v2.16.3
- 对应提交：`887fd946af089be25cfd4438e39d0ada0dadd6e9`
  （PR #16 数学公式 + PR #17 预热修复 + PR #18 交叉引用报错明细）

## 与上游的差异

只做了减法，一行逻辑都没改：

- 删掉两个可执行目标（`src/main.rs` 的 `mdx` 命令行、`src/bin/mdx-gui.rs`）
  及其专属依赖 `clap`、`eframe`、`rfd`。公文助手只用库入口 `mdx::convert`，
  砍掉 clap 一家五个 crate 编译也快一些。
- 删掉全部 TeX 输出：`tex_official`、`tex_research`、`tex_research_emitter`、
  `tex_compile`、`common/table_to_longtblr` 与整个 `resources/`（`md2tex.cls`、
  `official.cls`、模板、Tectonic 预热文件）；`OutputFormat` 只剩 `Docx`，
  `ConvertRequest` 去掉 `template` / `compile_pdf`。公文助手的 PDF 全部由 Typst 排版，
  研究报告的排版数据见 `typst_research`。
- 不保留上游的 `tests/`、`docs/`、`examples/`、`font/`、`scripts/`。
- `typst_research::visible_body_text` 共用研究报告解析器提取可见正文，供宿主的
  中英文计数使用；剔除链接地址、引用键和锚点，公式只提取文字说明，不改变排版。

### 待同步回上游的改动

研究报告封面重设计先在这里落地，尚未提交到上游，下次同步前须先把这些改动
合进 mdx，否则整目录替换会把它们冲掉：

- 新增 `src/cover.rs`（封面分类、汉字日期、稿次括号、版式坐标）并在 `lib.rs` 导出；
- `common/front_matter.rs` 新增 `标识行` / `署名` / `外文原题` / `题名分行` 四个键；
- `resources/research/template.tex` 封面改为 TikZ 按毫米定位；
- `tex_research/merger.rs` 渲染新增的封面变量；
- `docx_research.rs` 封面改为锚定页面的图文框。

表格的合并单元格与序号表同样先在这里落地：

- `common/table.rs` 新增 `TableSpan` / `ParsedTable` / `parse_cells`，按
  MultiMarkdown 写法解析 `||` 横向合并、`^^` 纵向合并；`Block::Table` 多了
  `spans` 与 `numbered` 两个字段；`lib.rs` 导出 `mdx::table`；
- `common/markers.rs` 新增 `is_numbered_table`：`<!-- [序号表] -->` 不再原样
  印出，紧随的表格标成序号表（整行合并的分组行靠左）；编号本身由调用方写进
  源码（公文助手见 `export::research::write_numbered_tables`）；
- `common/table_layout.rs`：`analyze_table` 跳过横向合并格，新增 `cell_alignment`；
- `common/table_to_longtblr.rs` 输出 `\SetCell[r=..,c=..]`，整行合并格套
  `\parbox[t]`；`docx_research.rs` / `docx_official.rs` 输出 `gridSpan` / `vMerge`；
- `parser.rs`：表前已有表题时不再吞掉表后那一行（那是下一张表的表题）。

研究报告的 Typst 排版数据同样先在这里落地：

- 新增 `src/typst_research.rs`（`lib.rs` 导出 `typst_research`）：沿用原 TeX 输出的解析
  与区段规则，产出可序列化的排版数据（封面、区段、算好的编号），交给公文助手的 Typst
  模板；报告题名的取法从原 `tex_research/merger.rs` 挪进来；`Cargo.toml` 为此加了 `serde`。

研究报告的目录标记同样先在这里落地：

- `common/markers.rs` 新增 `is_toc`，`common/ast.rs` 新增 `Block::Toc`：
  `<!-- [目录] -->` 不再原样丢弃，解析成原位的目录块（不是区段切换）；
- `resources/research/template.tex` 不再无条件排目录；`md2tex.cls` 新增
  `\mdxtableofcontents`：目录单用大写罗马页码（I、II、III），排完恢复阿拉伯
  页码并接着目录之前的页号数；`tex_research_emitter.rs` 在标记处输出它，只排一次；
  目录插在摘要与正文之间时，摘要前先输出 `\mdxfrontmatter`：摘要单用小写罗马
  页码（i、ii、iii），目录之后正文从 1 起；
- `docx_research.rs` 不再在封面后固定插目录，改为在标记处插入，只插一次；
  目录改写成段落里的 TOC 域（`TableOfContents` 放不进 `Section`）。全文按目录
  切成分节（`paginate`），各节配页脚“— N —”与起始页码，页码规矩与 PDF 相同；
  判定“目录插在摘要与正文之间”的 `toc_follows_abstract` 挪到 `common/ast.rs`，
  TeX 与 docx 共用。

居中 / 居右标记同样先在这里落地：

- `common/ast.rs` 新增 `Block::Aligned` 与 `LineAlign`；`common/markers.rs`
  新增 `align`：`<!-- [居中] -->` / `<!-- [居右] -->` 下方直到空行的各行
  逐行成段、整行居中或靠右，区内不认标题、列表、表格语法；
- `parser.rs` 识别该区；`tex_research_emitter.rs` / `tex_official.rs` 输出
  `{\noindent\centering ...\par}` / `{\noindent\raggedleft ...\par}`；
  `docx_research.rs` / `docx_official.rs` 输出居中 / 右对齐、无首行缩进的段落。

反斜杠转义同样先在这里落地（anydoc 0.2 导入的 Word 正文会带 `\$`、`\*` 等转义，
原先全部印成 `\textbackslash{}`）：

- `common/inline.rs`：被 `\` 转义的定界符不开启、也不闭合任何构造（强调与公式
  另查收尾定界符）；Text 里 `\` + ASCII 标点去掉反斜杠，新增 `pub fn unescape`；
  代码内容不动。行内公式与公文助手预览（`preview::math_flow::split_pieces`）
  对齐：公式内容可含 `\$`（正则 `\$(?:\\\$|[^$\n])+\$`），紧跟在另一个未转义
  `$` 之后的 `$` 不开启公式，行内 `$$x$$` 整体是文本；
- `parser.rs`：标题文字与表后表题（`: 标题`）不走行内解析，单独过 `unescape`。

部分标记与不编号标记同样先在这里落地：

- `common/ast.rs` 新增 `MarkerKind::Part` 与 `Block::Unnumbered`；`common/markers.rs`
  的 `detect` 认 `<!-- [部分] -->`，新增 `is_unnumbered`（`<!-- [不编号] -->`）；
- `parser.rs` 算好不编号的作用范围：标记管紧随标题的整棵子树（同级或更高一级
  的标题、区段标记结束），子树里每个标题前补一个 `Block::Unnumbered`；
- `common/heading.rs` 去编号规则 1 把"部分"当成一个词（原先字符类
  `[章节条部分]` 会把"第一部分"剥成"分"）；
- `tex_research_emitter.rs`：部分段的 `#` 输出 `\part`（落在主文件，章号跨部分
  连续），不编号标题输出 `\mdxunnumbered{part,chapter,section,...}`，不编号章前后
  切换 `\mdxfreenumbers` / `\mdxchapternumbers`；
- `resources/research/md2tex.cls`：`\ctexset{part=...}`（小一黑体、"第一部分"）、
  目录的部分字体与引导线（tocloft 默认的 `\large` 引导线要 8pt 的 cmmi8，随包
  texbundle 里没有，目录一出现部分条目就编译失败），新增上述不编号命令（`\phantomsection` + `\addcontentsline`
  进目录；不编号章的图表题注用全篇共用的不带章号流水号）；
- `docx_research.rs`：部分标题（Heading1、"第一部分"一行题目一行）、不编号标题
  与流水号题注；
- `tex_research/merger.rs` 与 `docx_research.rs::split_blocks`：报告题名只认正文
  区段里的第一个 `#`，部分、摘要、附录段里的 `#` 不再被拿去当题名删掉。

研究报告的引文与文框（Markdown 的 `>` 引用块）同样先在这里落地：

- 新增 `common/quote.rs`（行首 `>` 的认法、`[!名称]` 文框首行、`——` 出处行），
  `lib.rs` 导出 `mdx::quote` 供公文助手的预览解析器共用；
- `common/ast.rs` 新增 `Block::Quote`、`QuoteKind`、`QuoteItem` 与
  `quote_as_paragraphs`；`parser.rs` 把连续的 `>` 行解析成一个块（一行一段），
  文框首行的 `{#id}` 剥成 `Block::Label`；
- `common/crossref.rs`：锚点可以挂在文框上，引用块里的 `{@id}` 照样检查；
- `resources/research/md2tex.cls` 新增 `mdxquote` 环境、`\mdxquotesource`、
  `\mdxboxstep`（每种文框名称第一次出现时现建一对计数器：随章归零的与不编号章
  用的流水号）与 `mdxboxtblr` 长表（细框、浅灰底、不占表号）；
  `tex_research_emitter.rs` 按名称首次出现的先后给计数器分配 ASCII 键，
  文框单元格里的脚注、行内公式按表格单元格的规则降级；
- `docx_research.rs`：引文排成楷体缩进段落，文框排成一格表格（浅灰底、细框），
  每种名称各编各的号，口径与 TeX 相同；`tex_official.rs` / `docx_official.rs`
  按普通段落排。

## 改动规则

**不要直接改这里的代码。** 一旦这份副本与上游分叉，"研究报告的排版与 mdx 保持
一致"这个前提就没了，而这正是引入 mdx 的原因。

要改排版或转换逻辑，请：

1. 先在上游 <https://github.com/billowsand/mdx> 提交并合并；
2. 再按下面的办法把整个目录同步过来。

## 同步上游

整目录替换，不要逐文件挑拣；TeX 输出部分随后删掉（见上文「与上游的差异」），
`typst_research.rs` 与 `lib.rs` 的改动先合进上游再同步：

```bash
rm -rf vendor/mdx/src
cp -r /path/to/mdx/src vendor/mdx/
rm -rf vendor/mdx/src/bin vendor/mdx/src/main.rs vendor/mdx/src/cli.rs
rm -rf vendor/mdx/src/tex_* vendor/mdx/src/common/table_to_longtblr.rs
```

`vendor/mdx/Cargo.toml` 是本仓库维护的（上面那些减法），同步时**不要**覆盖它；
上游新增依赖时手工补进去。改完更新本文件里的版本号与提交号，然后跑：

```bash
cargo test --locked --all-targets
```

`src/export/research.rs` 与 `src/export/typst/research.rs` 里的测试会真跑一遍
`typst_research::build`，核对锚点、引用、表题、目录位置与题名口径；上游改了这些写法，
这几条测试会先红。
