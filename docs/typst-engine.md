# Typst 排版引擎

全部 PDF（六个公文文种与研究报告）都由进程内的 Typst 排版：不起子进程、不落中间文件，
一份公文 5–50 ms。原先的 Tectonic（XeLaTeX）链路已整体移除——`gonghan-gwa.cls`、
mdx 的 `md2tex.cls`、`texcompile`、随包 Tectonic 与离线 TeX bundle 都不在了，要看旧实现
翻 git 历史（移除前最后一版见 `feat: 研究报告改用 Typst 排版` 那次提交）。

两套模板都是对着当年内置 Tectonic 的 PDF 逐行对照调出来的；模板里凡标「实测」的数值，
都是在 TeX 产物上量出来的落点，注释里提到的 `\GwaTail`、`\gwa@placeclosing`、
`\clearemptydoublepage` 之类指的是原 TeX 实现里对应的宏。

## 结构

```text
公文：     Markdown ─ export::parse ─ export::typst ─ JSON ─┐
研究报告： Markdown ─ export::research（frontmatter、插图、文献装进临时目录）    ├─ typst_engine ─ PDF
                    ─ mdx::typst_research ─ research_redline ─ math ─ JSON ─┘
```

- 入口：`export::export_artifacts`（起草页导出）、`export::write_pdf` /
  `export::write_pdf_for_kind`（稿件库批量导出、送批材料、花脸稿）。研究报告在这里分流到
  `export::typst::research`。
- `src/typst_engine.rs`：Typst 的 `World`（模板是主文件，`/doc.json` 是数据，插图按用户
  目录解析且不许越界，公式 SVG 等内存文件另挂；字体 = 随包 `runtime/fonts` + 设置里生效的
  本机字体，按路径缓存）、编译、诊断翻译、孤行探针报告、横页结构调整。模板二选一：
  `Template::Official`（`assets/typst/gongwen.typ`）、`Template::Research`
  （`assets/typst/research.typ`），都编进二进制。

### 公文

- `src/export/typst/`：把解析结果变成模板数据，判定全部在 Rust 侧（标题编号与紧缩、标题
  断行与压缩比、智能列宽、居中格排布、要素显示值、花脸稿哨兵），模板只负责排。
  - `data.rs`：数据结构，字段名与模板一一对应；
  - `runs.rs`：行内文字 → 片段（加粗、括号楷体、花脸稿标注、中西文 2pt 间隙、跨片段的相邻标点挤压）；
  - `body.rs`：正文与附件分区、附件说明、横向附件判定；
  - `frame.rs`：各文种的版头、落款、版记、红头呈批件承办区、份号；
  - `table.rs` 里的 `to_typst_table`：表格。

### 研究报告

- `vendor/mdx/src/typst_research.rs`：沿用 mdx 的解析与区段规则（摘要、目录、正文、部分、
  附录、版本变更记录、参考文献、不编号子树），章节号、图表号（不编号章的流水号、附录的
  A.1）、文框号、列表序号都按原 `md2tex.cls` 的计数器规则算好；交叉引用的编号由模板按
  锚点处的 metadata 查。文献键、交叉引用的校验与原 TeX 路径相同，不过就报错。
- `src/export/typst/research_redline.rs`：花脸稿哨兵 → 片段上的 `m`（删除 / 新增）。
- `src/export/typst/math.rs`：公式由 latex-rust 进程内排版成 SVG（字形转路径，STIX Two
  Math；`\text{中文}` 回退随包方正书宋），与预览同一排版器。尺寸折成 em，模板按所在字号
  缩放、按盒模型对基线。`vendor/latex-rust` 为此改了几处：matrix、cases、array、align 的
  行加支柱（按正文 14bp/24pt 的 `\baselineskip`）、竖排的盒子按数学轴居中（`\vcenter`）、
  align 右列开头补空 Ord（`&=` 两侧照常留关系符空）。排不出的公式按源码印出并给提示，
  不让整份排版失败。
  SVG 导出与 PNG 预览均按回退字形标记选择字体轮廓，并按该字体的 `unitsPerEm` 换算
  大小；SVG 轮廓缓存按字体与 glyph id 隔离。中文条件（如 `\text{为偶数}`）、中文上下标
  与分数均有回归用例，`research-math` 样张包含这些写法。
- 字体固定用随包的方正书宋 / 黑体 / 楷体 / 小标宋、TeX Gyre Termes、JetBrains Mono，
  不受设置页本机字体影响；生僻字由内置宋体兜底。缺字体时只挡研究报告
  （`portable_runtime::validate_research_fonts`）。
- 文献：Typst 内置 hayagriva，样式 `gb-7714-2015-numeric`，引用不上标。

## 版式要点（公文）

- **字宽与断行**：汉字固定一个字宽（三号 16pt，不压字距），行太长只压标点、行太短字间
  拉开（上限 `0.08\baselineskip`，即 xeCJK CJKglue 的 plus）。不罚段末孤字（`costs.runt: 0%`），
  孤行交给探针报到审校面板。
- **标点压缩上限**（`vendor/typst-layout` 补丁）：两端对齐时标点外侧最多压
  `min(半宽, |外侧空白 − 内侧空白|)`，按字形墨迹算——与 xeCJK 全角式逐个标点硬挤标定的结果
  一一吻合。原版 Typst 一律可压半宽，括号引号压得比 TeX 狠。
- **相邻标点跨片段挤压**：Typst 只在同一段文字内挤「。（」「）。」，换字体（括号楷体）、
  加粗、花脸稿都会切段；`runs.rs` 在跨段处补较小字号半个字宽的负间隙。
- **中西文间隙**：xeCJK 的 CJKecglue 是 2pt，Typst 内置只有 1/4em，改由 Rust 在汉字类字符与
  西文之间插 2pt 弱间距；全角标点两侧、花脸稿标注边界不插。
- **表格**：列宽保留 tabularray 语义（`X` 比例按内容宽分、`Q` 定宽，每列左右 6pt 留白另计），
  行高 = 线 + 2pt + 21bp 支柱 + 2pt；跨页重复表头，「下一页继续」用每页重复的表尾、只在
  表格未完的页上有内容；相邻两表的间距相加。
- **红头呈批件首页窄栏**：Typst 没有 `\parshape`，模板用 `measure` 二分出首页额度内的最长
  前缀，把那一段切开，前半段末行两端对齐、后半段到第二页全宽续排、首行不缩进；续排部分
  不以行首禁则标点开头。
- **落款定位**：先试空 3 行，依次 2 行、1 行，都放不下另起一页标「此页无正文」。位置从落款
  **之前**的零高锚点块取——用 context 自身的 `here()` 会因为它输出的分页改变自己的位置而
  不收敛；裸 metadata 紧挨分页会被带到下一页，所以要包进零高的块。
- **横向附件**：Typst 先按横页排，`typst_engine::rotate_landscape_pages` 再改成竖向纸张 +
  内容逆时针转 90° + `/Rotate 90`（pdflscape 的结构），双面打印时装订边不因打印机的自动
  旋转方向而错位。页码仍在竖页时的位置（横页左缘，旋转 90°）。
- **份号逐份编制**：每份之前把页码计数器归 0（页码在每页开头自动加一），新一份首页才是 1；
  只有第一份带孤行探针。
- **孤行探针**：模板在段首段尾放带位置的 metadata，`typst_engine::proof_report` 拼成
  `.gwaproof` 格式（沿用原 TeX 探针的格式），交给 `orphan_probe`。

## 版式要点（研究报告）

- **版心与字号**：A4，内侧 28mm、外侧 26mm、上 37mm、版心高 225mm；正文 14bp，行距
  24 texpt；页首第一行基线在版心顶下 4.09mm（字身上缘取 11.59pt）。
- **页码**：「— N —」四号，页码盒按原 fancyhdr 的 `\headwidth` 偏离版心正中（奇数页以版心
  左缘、偶数页以右缘为准）。模板在每处「清空到奇数页且空白页不印页码」的地方放零高的
  `<gw-clear>`（带页码样式与起始值），页脚按标记推算：封面无页码、摘要单独编页时小写罗马、
  目录大写罗马、目录后接回目录之前的阿拉伯页号。章另起奇数页，中间的空白页照印页码；
  部分页之后的空白页不印。
- **章 / 节 / 部分**：章题小二黑体居中，基线在版心顶下 28.71mm，下一行基线 51.20mm；节
  缩进两字、黑体四号、前后不加距离；部分独占一页，「第一部分」基线 77.23mm、题名 94.42mm。
  PDF 书签用看不见的 heading（`bookmark`），版面上的标题另排。
- **表格**：表题放进表头，跨页每页重复，续页缀「（续表）」；表题基线距上一行基线 10.46mm，
  紧跟节标题再多 1.90mm、紧跟章题落在版心顶下 53.23mm。
- **文框、引文、插图、脚注、代码、文献**：间距都是实测值，见模板注释。
- **目录**：按每个进目录的标题前放的 `<gw-toc>` 生成，页码按该页的页码样式印。

## 样张

```bash
cargo test --locked typst_samples -- --ignored --nocapture          # 各文种、花脸稿、标题紧缩
cargo test --locked typst_samples_research -- --ignored --nocapture # 研究报告：封面、部分、公式……
```

PDF 与模板数据（`doc.json`）在 `tmp/typst-samples/<用例>/`。改模板后对着样张目视检查；
`typst_samples_real_docs` 可用 `GW_SAMPLE_DOCS` 指向一批真实稿件出样张。发布流程另跑
`shipped_runtime_typesets_every_kind`，用随包 runtime 把每个文种排一遍。

## 移除 Tectonic 前的对照结论

公文（2026-10-01，Typst 0.15.1 + 补丁，12 个用例）：页数一致（电话通知除外）；同文字的行
基线差都在 0.35mm 以内，绝大多数 < 0.05mm；断行逐字一致，只有极个别段落 TeX 宁可把两个
逗号挤到极限（badness≈23）也要多排一个字，Typst 按代价稍拉开、把那个字放到下一行，段落
行数不变。研究报告（2026-10-01，3 个用例）：除下列差异外，同文字的行基线逐页对齐。

刻意没有复刻的 TeX 行为（TeX 那边是缺陷或无关紧要）：

- TeX 的电话通知会多出一页：落款紧跟在分页之后时误判放不下，另起一页标「此页无正文」；
- 「此页无正文」的高度在 TeX 里随上一页末尾的胶漂 1–1.4mm；Typst 固定；
- 花脸稿新增框所在段落之后紧跟表格时，TeX 的表格低 0.8mm；
- 横向附件跨多页时，双面印刷的第二张横页沿用第一张的上下边距（TeX 逐页按奇偶换）；
- 研究报告的列表从二级回到一级时序号接着数（TeX 重开一组，从 ⑴ 再数）；
- 研究报告的参考文献只排一个标题（TeX 在区段标题之后另起一页再排一遍，多出两页）；
- 研究报告正文加粗的汉字是粗的（TeX 的 `AutoFakeBold` 在原类文件里没生效）；
- 研究报告的公式字形是 STIX Two Math（TeX 是 Computer Modern），大括号、矩阵括号略小；
  PDF 里公式是矢量路径、不可选中复制；
- 文献条目细节随 hayagriva：标题大小写保留原样、网络文献年份带括号，编号与正文的间距
  略宽；多篇连续引用印 `[2,3]`（gbt7714 印 `[2-3]`）。

## 坑

- `scale(reflow: true)` 是块级元素，直接放进段落会被**静默丢掉**（只有一条警告）——外面包 `box`。
- 段落里的 `align` 不起作用，要 `align(center, par(...))`。
- 顶层 `#let` 里换行写 `else` 会让表达式提前结束、后半截掉进正文——`else` 与 `}` 同行。
- 块之间夹硬间距 `v` 时，`v` 叠在下一段的段间距之上；弱间距会与段间距合并成一个。
- 块间距取上一块 `below` 与下一块 `above` 的较大者，不是相加：要「比默认多空一点」得把
  较大的那一边加上去。
- 计数器（页码）在每页开头自动加一：要让新一份从 1 起，在上一份末尾归 0。
- 紧跟在分页之后的裸 metadata / context 可能落在上一页：要标「这一页」的东西放进新页的块里，
  要标「上一页」的东西包进零高的块、放在分页之前。
- `image` 放进 `place` 时按行内元素排、底边贴基线，`dy` 不是图顶的位置——外面套一个定高的
  `box`。
- `heading` 自带字号与粗细，`show heading: it => it.body` 也去不掉；版面上的标题自己排，
  heading 只拿来生成书签（`place(hide(heading(...)))`）。
- `context` 里比较含 `em` 的长度要先 `.to-absolute()`。

## 升级 Typst

`typst`、`typst-pdf`、`typst-layout` 三者同版本一起升；`typst-layout` 是 vendored 副本，按
`vendor/typst-layout/README.md` 重新打补丁，再出一遍样张对比。
