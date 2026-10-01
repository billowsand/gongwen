# Typst 排版引擎（公文 PDF 双引擎）

公文 PDF 现在有两套引擎，设置页「导出格式 → PDF 引擎」切换，默认 Typst：

| | Typst（默认） | Tectonic（原链路） |
|---|---|---|
| 位置 | 进程内（`typst` crate），模板 `assets/typst/gongwen.typ` | 外部进程，`gonghan-gwa.cls` |
| 速度 | 一份公文 5–50 ms | 一份约 2 s |
| 产物 | 只出 `.pdf` | `.tex` + `.pdf` |
| 研究报告 | 不适用，固定走 Tectonic | mdx research 模式 |

两套版式逐项对照过（见下文「对照」），换引擎不改变版面。

## 结构

```text
Markdown ─ export::parse ─┬─ export::latex  → .tex → texcompile（Tectonic）→ PDF
                          └─ export::typst  → JSON → typst_engine（进程内）→ PDF
```

- `src/export/typst/`：把解析结果变成模板数据。**所有判定复用 TeX 路径的同一套函数**
  （标题编号与紧缩、标题断行与压缩比、智能列宽、居中格排布、要素显示值、花脸稿哨兵），
  模板只负责排。
  - `data.rs`：数据结构，字段名与模板一一对应；
  - `runs.rs`：行内文字 → 片段（加粗、括号楷体、花脸稿标注、中西文 2pt 间隙、跨片段的相邻标点挤压）；
  - `body.rs`：正文与附件分区、附件说明；
  - `frame.rs`：各文种的版头、落款、版记、红头呈批件承办区、份号；
  - `table.rs` 里的 `to_typst_table`：表格（与 `to_longtblr` 同一套判定）。
- `src/typst_engine.rs`：Typst 的 `World`（模板是主文件，`/doc.json` 是数据，插图按用户
  目录解析且不许越界；字体 = 随包 `runtime/fonts` + 设置里生效的本机字体，按路径缓存）、
  编译、诊断翻译、孤行探针报告、横页结构调整。
- `assets/typst/gongwen.typ`：模板，编进二进制。数值凡标「实测」的，都是对着内置
  Tectonic 的 PDF 量出来的落点。
- 入口：`export::export_all_with_engine`（起草页导出）、`export::write_pdf_for_kind`
  （稿件库批量导出、送批材料、花脸稿）。引擎由 `AppConfig::pdf_engine` 传入。

## 版式要点（与 gonghan-gwa.cls 的对应）

- **字宽与断行**：汉字固定一个字宽（三号 16pt，不压字距），行太长只压标点、行太短字间
  拉开（上限 `0.08\baselineskip`，即 xeCJK CJKglue 的 plus）。不罚段末孤字（`costs.runt: 0%`，
  TeX 不罚，孤行交给探针报到审校面板）。
- **标点压缩上限**（`vendor/typst-layout` 补丁）：两端对齐时标点外侧最多压
  `min(半宽, |外侧空白 − 内侧空白|)`，按字形墨迹算——与 xeCJK 全角式逐个标点硬挤标定的结果
  一一吻合。原版 Typst 一律可压半宽，括号引号压得比 TeX 狠。
- **相邻标点跨片段挤压**：Typst 只在同一段文字内挤「。（」「）。」，换字体（括号楷体）、
  加粗、花脸稿都会切段；`runs.rs` 在跨段处补较小字号半个字宽的负间隙，与 TeX 一致。
- **中西文间隙**：xeCJK 的 CJKecglue 是 2pt，Typst 内置只有 1/4em，改由 Rust 在汉字类字符与
  西文之间插 2pt 弱间距；全角标点两侧、花脸稿标注边界不插（与 TeX 一致）。
- **表格**：列宽保留 tabularray 语义（`X` 比例按内容宽分、`Q` 定宽，每列左右 6pt 留白另计），
  行高 = 线 + 2pt + 21bp 支柱 + 2pt；跨页重复表头，「下一页继续」用每页重复的表尾、只在
  表格未完的页上有内容；相邻两表的间距相加（TeX 加、Typst 默认取大）。
- **红头呈批件首页窄栏**：Typst 没有 `\parshape`，模板用 `measure` 二分出首页额度内的最长
  前缀，把那一段切开，前半段末行两端对齐、后半段到第二页全宽续排、首行不缩进；续排部分
  不以行首禁则标点开头。
- **落款定位**（`\gwa@placeclosing`）：先试空 3 行，依次 2 行、1 行，都放不下另起一页标
  「此页无正文」。位置从落款**之前**的零高锚点块取——用 context 自身的 `here()` 会因为它输出
  的分页改变自己的位置而不收敛；裸 metadata 紧挨分页会被带到下一页，所以要包进零高的块。
- **横向附件**：Typst 先按横页排，`typst_engine::rotate_landscape_pages` 再改成与 pdflscape
  同一种结构（竖向纸张 + 内容逆时针转 90° + `/Rotate 90`），双面打印时装订边不因打印机的
  自动旋转方向而错位。页码仍在竖页时的位置（横页左缘，旋转 90°）。
- **份号逐份编制**：每份之前把页码计数器归 0（页码在每页开头自动加一），新一份首页才是 1；
  只有第一份带孤行探针。
- **孤行探针**：模板在段首段尾放带位置的 metadata，`typst_engine::proof_report` 拼成与
  `\GwaTail` 相同的 `.gwaproof` 格式，`orphan_probe` 原样复用。

## 对照

```bash
cargo test --locked typst_tex_compare -- --ignored        # 12 个用例，双引擎各排一遍
cargo test --locked typst_tex_compare_redline -- --ignored # 花脸稿
python scripts/typst-compare.py                            # 逐行比基线、出对比图
```

PDF 与对比图在 `tmp/typst-compare/<用例>/`。用例覆盖公函（含附件、预览版 + 双面 + 指人专办、
联合发文三家、份号逐份编制）、电话通知、普通公文（紧缩 + 附件、双面横向附件）、白头件、
红头呈批件（正文跨页 / 不跨页）、会议议程、花脸稿。调模板参数时可用
`typst_tex_compare_typst_only` 只重排 Typst、沿用上次的 TeX 结果。

当前结果（2026-10-01，Typst 0.15.1 + 补丁）：

- 页数一致（电话通知除外，见下）；同文字的行基线差都在 0.35mm 以内，绝大多数 < 0.05mm；
- 断行逐字一致，只有极个别段落：「到2026年底，……提质改造，服」TeX 宁可把两个逗号挤到极限
  （badness≈23）也要排进「服」，Typst 按 Knuth-Plass 代价选择稍拉开、把「服」放到下一行
  （badness≈0.1）。段落行数不变，不影响分页；
- 红头呈批件首页窄栏的断点偶有一字之差（同上原因），行数一致。

## 已知差异（Typst 更对或无关紧要的，未刻意复刻）

- **TeX 的电话通知会多出一页**：落款紧跟在分页之后时，`\gwa@placeclosing` 拿到的是还没清空的
  上一页的 `\pagetotal`，误判放不下，另起一页标「此页无正文」。Typst 正确地把落款排在正文
  之后。
- 「此页无正文」的高度在 TeX 里随上一页末尾的胶（表格后、附件说明后）漂 1–1.4mm；Typst 固定。
- 花脸稿新增框所在段落之后紧跟表格时，TeX 因框的下边加深了行深，表格低 0.8mm。
- 横向附件跨多页时，双面印刷的第二张横页沿用第一张的上下边距（TeX 逐页按奇偶换）。

## 坑

- `scale(reflow: true)` 是块级元素，直接放进段落会被**静默丢掉**（只有一条警告）——外面包 `box`。
- 段落里的 `align` 不起作用，要 `align(center, par(...))`。
- 顶层 `#let` 里换行写 `else` 会让表达式提前结束、后半截掉进正文——`else` 与 `}` 同行。
- 块之间夹硬间距 `v` 时，`v` 叠在下一段的段间距之上；弱间距会与段间距合并成一个。
- 计数器（页码）在每页开头自动加一：要让新一份从 1 起，在上一份末尾归 0。
- `context` 里比较含 `em` 的长度要先 `.to-absolute()`。

## 升级 Typst

`typst`、`typst-pdf`、`typst-layout` 三者同版本一起升；`typst-layout` 是 vendored 副本，按
`vendor/typst-layout/README.md` 重新打补丁，再跑上面的对照。
