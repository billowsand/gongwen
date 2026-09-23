# AGENTS.md — 公文助手（gongwen）

基于 Rust + egui 的离线中文公文写作桌面应用。模型只起草 Markdown 正文，行文要素、
版式规则与导出结果由本地程序控制。

## 技术栈

- Rust edition 2024（stable），GUI 用 `eframe` / `egui` 0.35，纯 CPU 渲染 PDF 用 `hayro`。
- 导出链：Markdown → DOCX（`docx-rs`）/ TeX → PDF（本机 Tectonic / XeLaTeX）、
  XLSX（`rust_xlsxwriter`）、稿件库与词表用 `rusqlite`。研究报告支持 LaTeX 数学
  公式（`$...$` / `$$...$$`）：导出走 tectonic + amsmath/mathtools（需要 runtime
  v0.6.0 起的 texbundle），预览用 `latex-rust` crate 进程内渲染（STIX Two Math，
  与导出字形不一致属预期）。
- 中文处理：`jieba-rs`（含用户词典）、`pinyin`；文档读取用 `anydoc`。
- 输入法：应用内拼音输入法，引擎、词库与整句模型都在本进程里，不用系统输入法、
  也没有独立进程。代码在 `src/ime/`，内核 vendor 自字在输入法（GPL-3.0-or-later），
  见 `vendor/qingjian/README.md`。
  - 数据：`runtime/ime/dict.qj`（必需）+ `lm.qj`（可选，44 MB，长句打得少可以不带）。
  - 学习数据与公文同一个用户目录：`config_dir()/ime/`——词频与用户词、公文词表
    导出的附加词库（`dicts/`）、辅码表（`fuma/`）。
  - 辅码（形码）表**不随包**：权利归方案作者、上游未获再分发授权，只能由使用者
    在设置页自己导入。
- 模型接入：本机 LM Studio / Ollama，走 OpenAI 兼容接口（`src/lmstudio.rs`、
  `src/rag.rs`、`src/rag_client.rs`）。

## 常用命令

```bash
cargo run                      # 开发运行
cargo check --all-targets      # 快速基线
cargo fmt --all -- --check     # CI 零容忍，改完必跑
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --release --locked
```

- 开发构建给依赖开 `opt-level = 3`（见 `Cargo.toml`），首次编译依赖约需一分钟，属正常。
- `cargo test` 有**约 5 个与沙箱环境相关的失败，是预期结果**，不要为此改动测试。
- Linux 变体：Arch/Omarchy 发布包用
  `cargo build --release --locked --no-default-features --features linux-portal-dialogs`。

## 代码组织

单文件超过约 2000 行就该按功能域拆成模块文件夹。已拆过的：`src/app/`、`src/draft_page/`、
`src/preview/`、`src/lexicon/`、`src/export/{docx,latex}/`、`src/ime/`。拆分流程见 skill
`split-rust-module`（纯代码移动，每拆一个文件单独提交一次，零警告验证）。

`skills/gongwen-markdown/` 是给外部 AI 工具用的技能包（Agent Skill），讲的是起草页
Markdown 语法与各文种正文规则，由 `src/skill_pack.rs` 编进二进制、设置页「上手指引」导出，
`scripts/package-portable.ps1` 另打一份 `.skill` 随安装包分发。改了解析器语法、标题编号、
附件或文种规则（`src/export/parse.rs`、`src/prompt.rs`）要同步这里的文档；新增文件要登记进
`skill_pack::FILES`，测试会检查两边一致。

顶层模块清单在 `src/main.rs`。注意 `mod` 声明里 `outline`、`proofread_rules` 等
并非全部集中在文件头部，改动前先 `grep -n "^mod " src/main.rs`。

## 三条不可逾越的红线（改 AI 相关代码前必读 `docs/ai-architecture.md`）

1. AI 永远不直接写入正文，产物只能是需用户采纳的「修订建议」。
2. AI 永远不碰公文要素（单位、人员、文号、密级、成文日期），不得回写 `DraftInput`。
3. 任何 AI 产物落地前必须过确定性闸门：`ai_guard`（事实比对）、`validator`（要素校验）、
   `proofread` / `proofread_rules`（词表与规则复扫）。闸门不过就丢弃。

推论：确定性规则是 AI 的裁判，不是竞争者，不要为了迁就模型放宽规则。

## 版本与发布

- 版本号只维护在两处：`Cargo.toml` 的根包 `version` 和 `Cargo.lock` 里
  `name = "gongwen-assistant"` 的 `version`。
- 本机**没有 `pwsh`**，不要调用 `scripts/bump-version.ps1`，手动编辑上述两处。
- `ci.yml` 在 push `main` 时触发（macOS / Windows / Linux 三平台跑 fmt / clippy / test；
  另在干净的 Ubuntu 20.04 里装 deb 并用 Xvfb 真正启动一次，见 `scripts/smoke-launch-linux.sh`）；
  `release.yml` 在 push `v*` tag 时触发，产出 8 个资产（Windows setup.exe、
  Linux ARM64/AMD64 deb、macOS ARM64 DMG 及各自 `.sha256`）。
- Release workflow 全程约 20–30 分钟。
- 完整发布流程（bump → tag → 监控 → 写中文 release notes → 验证）见 skill `release`。
  **硬性要求：release 正文必须手写，不能用 `--generate-notes` 的占位说明。**
- 打包一个**供本机安装测试的开发版**（不走发布）：跑
  `scripts/package-dev.ps1`（`powershell -ExecutionPolicy Bypass -NoProfile -File`），
  它构建 release → 组装 `dist\win-x64-full` → 用 Inno Setup 打出
  `dist\gongwen-assistant-<下一补丁号>-dev-win-x64-setup.exe`；只重打包不重编译加
  `-SkipBuild`。脚本含中文，文件头必须有 UTF-8 BOM，否则 PowerShell 5.1 按 GBK 读会解析失败。

## 约定

- 提交信息用中文 Conventional Commits：`feat:` / `fix:` / `test:` / `docs:` /
  `refactor:` / `style:` / `chore: release vX.Y.Z`。
- 许可证是 **GPL-3.0-or-later**：因为链接了 GPL 的输入法内核（`vendor/qingjian/`），
  整个程序都按 GPL 分发。改许可证相关的东西要同步 `LICENSE`、`Cargo.toml` 的 `license`、
  `README.md` 许可节、`THIRD_PARTY_NOTICES.md` 四处。
- 输入法数据（`runtime/ime/dict.qj`、`lm.qj`）是随包资源，本地开发副本不入库；
  `lm.qj`（44 MB）可选，缺了只是整句能力退化，输入法照常能用。
- 代码注释、文档、README 一律中文。
- 应用输出的 `dist/`、`output/`、`tmp/`、`target/` 均为生成物，不要提交。
- `config.json`、`.env*` 是本机配置，不入库；`config.example.json` 是模板。

## 环境

- 远程 `git@github.com:billowsand/gongwen.git`，发布分支 `main`。
- 使用 `gh` 管理 release（账号 `billowsand`，需 repo scope）。
