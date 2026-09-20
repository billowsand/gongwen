# vendor/qingjian — 随仓库分发的输入法内核

这份代码不是本仓库写的，是从**字在输入法**（青简 Qingjian 的 Windows 分支）里
整块搬过来的：应用内输入法的拼音引擎、词库、bigram 整句与用户学习。

- 上游：`https://github.com/billowsand/zizai`（青简上游 `https://github.com/qingjian-team/qingjian`）
- 取自提交：`9b643e1c8315c30e040e7798a9dadba0519a1807`（"feat(windows): 集成本地语音听写"，2026-09）
- 许可证：GPL-3.0-or-later（见同目录 `LICENSE`）。**本仓库因此也以 GPL-3.0-or-later 分发。**
- 名字与 logo 不在 GPL 授权范围内，已全部剔除，仓库里不出现"字在""青简"的品牌资源。

## 搬了哪些 crate

只搬**平台无关**的那几层。上游 `apps/`（TSF DLL、Server 进程、语音 Worker、自绘渲染器、
设置程序）一个都没搬——那些是"做系统输入法"才需要的壳，本项目用 egui 自己当壳。

| crate | 作用 | 为什么要 |
| --- | --- | --- |
| `format` | `.qj` 二进制容器：mmap 打开、零拷贝视图、哈希索引 | 词库与语言模型的载体 |
| `dictionary` | 词库查询（按音节位置二分） | 候选与整句词图 |
| `core` | 拼音解析、候选、排序、整句 Viterbi、学习接口 | 引擎本体，`Engine` 是唯一门面 |
| `lm` | bigram 语言模型 | 整句转换的前提 |
| `translate` | 释义表 / 词汇等级表 | 只为让 `learning` 编译；本应用不加载这两张表 |
| `learning` | 用户词频、用户词、个人 n-gram、敲错表、输入日志、输入统计 | 词随人走 |

**没有搬** `neural`（candle + 54MB 权重）：上游自己的回放里它也换不到正收益
（`docs/notes/neural-rescoring.md`：基线整句首选 89.5%，神经最好一档 88.4%），
而且那份权重是 GPL、还要拖进 candle/gemm 这条跨平台最麻烦的依赖。

## 与上游的差异（只有这些）

1. **清单改写**：上游各 crate 用 `version.workspace = true` 之类的 workspace 继承，
   本仓库不是 workspace，所以六份 `Cargo.toml` 改成了自包含的写法（写死版本号、
   依赖写真实版本）。`[lints] workspace = true` 与 `[dev-dependencies]` 去掉——
   vendor 的 crate 不作为 workspace 成员，它们的 `#[cfg(test)]` 代码不会被编译。
2. **`src/` 下一行未改。** 同步上游时直接整目录覆盖 `src/`，冲突只会出在清单里。

## 同步上游的办法

```bash
# 1. 取上游新提交
git -C <上游仓库> fetch && git -C <上游仓库> checkout <新 sha>
# 2. 覆盖六个 crate 的 src（清单保持本仓库的版本）
for c in format dictionary core lm translate learning; do
  rm -rf vendor/qingjian/$c/src
  cp -r <上游仓库>/crates/qingjian-$c/src vendor/qingjian/$c/
done
# 3. 有新增外部依赖时补进本仓库根 Cargo.toml，并跑
cargo check --all-targets && cargo test
```

注意 `.qj` 数据文件的 `FORMAT_VERSION`（`crates/qingjian-format/src/layout/header.rs`）
必须与这份代码配套：上游一升版本，`data/ime/` 里的 `dict.qj` / `lm.qj` 就要重新生成，
两边一起提交。这也和 `runtime` 资产那套（独立 tag）是一个道理。
