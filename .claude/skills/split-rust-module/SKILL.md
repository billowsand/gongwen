---
name: split-rust-module
description: 把超大 Rust 源文件按功能域拆成模块文件夹（每个文件一次提交、零警告验证）。当用户要求拆分大文件、重构模块结构、整理代码组织时使用。
---

# Rust 大文件拆分 — Agent Skill Guide

把单个超大 `.rs` 文件（建议阈值：>2,000 行）按功能域拆成同模块下的子文件目录，纯代码移动、不改变行为。本仓库已用此流程拆分 `app.rs`（10,130 行）、`draft_page.rs`（7,159 行）、`preview.rs`、`export/docx.rs`、`export/latex.rs`、`export/mod.rs`，可参照其提交历史（`git log --oneline` 中 `refactor: 拆分 …` 系列）。

## 工具

- `scripts/splitter.py` — 通用拆分器：读取 `config_<module>.py` 配置，按条目边界切分、解析导入、包裹 impl、生成子文件与瘦身后的根文件。
- `scripts/config.example.py` — 配置模板（含全部字段说明）。

## 工作流

### 0. 前置

- `git status` 干净；先跑一次 `cargo check` 确认基线。
- 确认模块的外部依赖：`grep -rn "模块名::" src/ --include="*.rs" | grep -v 源文件`，这些名字必须继续可从模块根访问（根保留或再导出）。

### 1. 分析边界

```bash
grep -nE "^(pub\(crate\) )?(pub )?(struct|enum|impl|fn|mod|const|type) |^    (pub\(crate\) )?(pub )?fn " <file> > /tmp/boundaries.txt
```

- 记下每个条目的 1 基行号与名字，规划功能域分组（先按文件内方法名 / 文档注释归类）。
- 特殊条目：
  - 大 `impl X { … }`（如 `impl GongwenApp`）：方法行逐个列出，`IMPL_WRAP` 指定范围让每个子文件自行包裹 impl；impl 头行用 `"REMOVE"` 目标，其收尾 `}` 加入 `REMOVE_LINES`（先跑一次看脚本提示，或用 `git show HEAD:file | sed -n` 确认行号）。
  - 小 `impl Foo`（如 `impl TableEdit`）：impl 头与全部方法行列给同一文件，脚本自动带上收尾 `}`。
  - 常量、`type` 别名：要么留根，要么整体随功能域移动。
  - 带 `#[cfg(test)]` 的条目：只能测试构建可见，再导出必须用 `#[cfg(test)]` 标注。

### 2. 写配置

复制 `scripts/config.example.py` 为 `.tmp/config_<module>.py`，填 `TARGETS` / `ROOT_ITEMS` / `RE_EXPORTS` / `MODS` / `DOCS`。

- `MODULE`：模块在 crate 里的路径（`app`、`export::docx` 等），决定子文件里 `use crate::MODULE::{…}` 的前缀。
- `HEADER_STRIP`：0 基行区间，覆盖原模块文档 + use 块；末尾停在第一个条目之前（勿吃掉它的文档注释）。
- 首轮 `RE_EXPORTS` 可以把所有被移动的条目都放进主块，跑完 `cargo check` 后按 unused 警告把「仅测试使用」的挪到 `#[cfg(test)]` 块、把「纯内部」的直接删除——警告列表就是最可靠的修剪依据。
- 先 `python3 .tmp/splitter.py .tmp/config_<module>.py` 确认无 "target line N is not a detected boundary" 报错（有则说明行号表有误或边界正则未识别）。

### 3. 迭代编译

```bash
git checkout -- <file> && rm -rf <out_dir>   # 每次重跑前先还原
python3 .tmp/splitter.py .tmp/config_<module>.py
cargo check 2>&1 | tee /tmp/chk.txt | grep -E "^error" -A6
```

按错误逐类修复（多数是配置/脚本层问题，见「常见坑」）：

- **缺导入**（cannot find / undeclared）：把名字加进 `ROOT_ITEMS`（子文件从 `crate::MODULE::` 导入）或 `PER_FILE_EXTRA`（强制行）。
- **字段/类型私有**（E0616 / E0603 / "more private than the item"）：`PUB_UPGRADES` / `ROOT_PUB_UPGRADES` 列出对应行。
- **方法私有**（E0624）：脚本已把所有被移动的缩进方法统一升为 `pub(crate)`，一般不会遇到。
- 改完配置**重新生成**（先 checkout + rm，再跑 splitter），不要手改生成物后忘记同步配置。

### 4. 清理警告

`cargo check` 0 错误后处理 unused-import 警告。惯用三条规则：

1. 根文件 import 里「仅测试使用」的名字 → 从根删掉，加进 `mod tests` 的 `use`。
2. 根再导出里「仅测试使用」的名字 → 挪进 `#[cfg(test)] pub(crate) use …;`。
3. 根再导出里「纯内部使用」（子文件内部互相调用）的名字 → 直接删除。
4. 子文件里多余的 import → 删除（用 `grep -n "use " 文件` 定位精确行）。

可以用一个小 python 脚本做精确字符串替换（参照仓库 `.tmp/cleanup_*.py` 的写法），并让它可重跑。

### 5. 验证与提交

```bash
cargo check 2>&1 | grep -cE "^(error|warning)"   # 期望 0 0
cargo clippy 2>&1 | grep -cE "^(warning|error)"  # 期望 0
cargo build
cargo test   # 结果须与拆分前一致（本项目有 5 个沙箱环境性失败，属预期）
```

每拆一个文件**单独提交**一次，提交信息说明拆分前后行数与子模块清单。

## 常见坑

- **跨行原始字符串**：LaTeX 模板 `r"..."` / `r#"..."#` 可跨行且内含大量 `{}`。扫描器已支持原始字符串与生命周期 `'static`（与字符字面量 `'a'` 区分）；遇到新的边界情况先小范围复现。
- **`#[cfg(test)]` 属性归属**：`#[cfg(test)]` / `#[allow]` 等属性行属于**下一个**条目（如 `mod tests`）。脚本会把它留给根；若子文件尾部出现悬空属性导致 "expected item after attributes"，说明 trim 逻辑被改动过，需恢复「尾部 `#[` 行还给根」的行为。
- **大 impl 的收尾 `}`**：`impl GongwenApp { … }` 的 `}` 若随最后一个方法范围被带走，根会出现多余 `}`；反之留根则子文件 impl 缺 `}`。用 `REMOVE_LINES` + `IMPL_WRAP` 组合处理，并核对生成的子文件 `impl` 块完整。
- **`use super::{…}`**：子文件里 super 指向**模块根**而非原文件的 super。配置 `SUPER_MAP` 把原 `use super::{…}` 解析为 `crate::<映射>`，让子文件从正确路径导入。
- **glob 导入**（`use docx_rs::*;`）：不会触发 unused 警告，适合放进 `PER_FILE_EXTRA["*"]` 统一给所有子文件。
- **测试区间分析要用拆分前的文件**：用 `git show HEAD:file | sed -n '测试起始,末尾p'` 分析测试依赖，不要用拆分后的文件（行号已变）。
- **每次重跑前 checkout 还原**：splitter 是整体重写生成物，基于上一次的生成结果重跑会产生叠加错误。
