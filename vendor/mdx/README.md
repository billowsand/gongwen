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
- 版本：v2.15.4
- 对应提交：`b897ce1b9013e9f218fe51e62a64029d36018e46`
  （PR #15，"宿主注入 `\MdxFontPath` 时按文件加载字体"）

## 与上游的差异

只做了减法，一行逻辑都没改：

- 删掉两个可执行目标（`src/main.rs` 的 `mdx` 命令行、`src/bin/mdx-gui.rs`）
  及其专属依赖 `clap`、`eframe`、`rfd`。公文助手只用库入口 `mdx::convert`，
  砍掉 clap 一家五个 crate 编译也快一些。
- `resources/` 只保留 `.tex` 与 `.cls`，删掉上游仓库里混进去的编译产物
  （`.aux`、`.log`、`.pdf`、`.synctex.gz` 等）。
- 不保留上游的 `tests/`、`docs/`、`examples/`、`font/`、`scripts/`。

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
