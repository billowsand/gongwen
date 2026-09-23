# vendor/mdx

公文助手的研究报告文档类型，TeX 转换与排版规范全部由 mdx 的 research 模式提供。
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
- `resources/` 只保留 `.tex` 与 `.cls`，删掉上游仓库里混进去的编译产物
  （`.aux`、`.log`、`.pdf`、`.synctex.gz` 等）。
- 不保留上游的 `tests/`、`docs/`、`examples/`、`font/`、`scripts/`。

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

研究报告的目录标记同样先在这里落地：

- `common/markers.rs` 新增 `is_toc`，`common/ast.rs` 新增 `Block::Toc`：
  `<!-- [目录] -->` 不再原样丢弃，解析成原位的目录块（不是区段切换）；
- `resources/research/template.tex` 不再无条件排目录；`md2tex.cls` 新增
  `\mdxtableofcontents`：目录单用大写罗马页码（I、II、III），排完恢复阿拉伯
  页码并接着目录之前的页号数；`tex_research_emitter.rs` 在标记处输出它，只排一次；
- `docx_research.rs` 不再在封面后固定插目录，改为在标记处插入，只插一次。

## 改动规则

**不要直接改这里的代码。** 一旦这份副本与上游分叉，"研究报告的排版与 mdx 保持
一致"这个前提就没了，而这正是引入 mdx 的原因。

要改排版或转换逻辑，请：

1. 先在上游 <https://github.com/billowsand/mdx> 提交并合并；
2. 再按下面的办法把整个目录同步过来。

## 同步上游

整目录替换，不要逐文件挑拣：

```bash
rm -rf vendor/mdx/src vendor/mdx/resources
cp -r /path/to/mdx/src /path/to/mdx/resources vendor/mdx/
rm -rf vendor/mdx/src/bin vendor/mdx/src/main.rs vendor/mdx/src/cli.rs
find vendor/mdx/resources -type f ! -name "*.tex" ! -name "*.cls" -delete
```

`vendor/mdx/Cargo.toml` 是本仓库维护的（上面那些减法），同步时**不要**覆盖它；
上游新增依赖时手工补进去。改完更新本文件里的版本号与提交号，然后跑：

```bash
cargo test --locked --all-targets
```

`src/export/research.rs` 里有一条契约测试
（`released_class_loads_research_fonts_from_the_injected_path`），会检查
`md2tex.cls` 仍然支持 `\MdxFontPath`、字体文件名与 `RESEARCH_FONT_FILES` 对得上、
`fontset` 仍被钉死。上游要是动了这几处，这条测试会先红。
