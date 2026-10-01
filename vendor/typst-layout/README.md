# vendor/typst-layout

Typst 排版引擎 `typst-layout` 的一份副本，带一处公文排版补丁，经根 `Cargo.toml`
的 `[patch.crates-io]` 接入。公文 PDF 的 Typst 引擎见 `src/typst_engine.rs`。

## 来源

- crates.io `typst-layout` **0.15.1**（与 `typst`、`typst-pdf` 同版本，升级时三者一起换）
- 上游：<https://github.com/typst/typst>，许可证 Apache-2.0（见同目录 `LICENSE`）
- 只拷了 `src/` 与 `Cargo.toml`（crates.io 规范化后的版本）；本 README 是本仓库写的说明。

## 补丁：两端对齐时标点的收缩上限按 xeCJK

原版两端对齐时，行内的 CJK 标点一律可压到半个字宽。xeCJK（全角式、`RubberPunctSkip`）的
上限按字形墨迹定：外侧最多压 `min(半宽, |外侧空白 − 内侧空白|)`——逗号句号顿号这类墨迹贴
一边的仍是半宽，括号、书名号、引号这类只能压 0.18–0.23 字。用内置 Tectonic 逐个标点硬挤
标定过（`hbox to` 比自然宽度少 4 个字），与这个公式一一吻合。不改的话括号引号压得比 TeX
狠，一行偶尔多排一个字，后面的断行、分页、孤行提示跟着对不上。

改法：`ShapedGlyph` 新增 `justification_shrinkability()`，非 CJK 标点同 `shrinkability()`，
CJK 标点再按上面的公式封顶；两端对齐（`ShapedText::shrinkability`、生成字形时按比例收缩）
与断行估算（`linebreak.rs` 的 `Estimates::compute`）改用它。以下仍用原来的
`shrinkability()`，行为不变：

- 行首、行尾标点压缩（`line.rs` 的 `adjust_cj_at_line_start/end`）；
- 相邻标点挤压（`shaping.rs` 的 `calculate_adjustability`）。

改动处：

```text
src/inline/shaping.rs   ShapedGlyph::justification_shrinkability、xecjk_punct_shrink_cap（新增）
                        ShapedText::shrinkability
                        ShapedText::build 里两端对齐的逐字收缩
src/inline/linebreak.rs Estimates::compute
```

## 升级

1. 把新版本的 `typst-layout` 源码替换进来（`src/`、`Cargo.toml`，保留本 README 与 `LICENSE`）；
2. 按上面清单重新打补丁；
3. 跑 `cargo test --locked typst_samples -- --ignored` 出样张，与升级前的样张对比断行、分页
   （见 docs/typst-engine.md）。
