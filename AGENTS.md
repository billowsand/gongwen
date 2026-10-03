# AGENTS.md — 公文助手（gongwen）

基于 Rust + egui 的离线中文公文写作桌面应用。模型只起草 Markdown 正文，行文要素、
版式规则与导出结果由本地程序控制。

## 技术栈

- Rust edition 2024（stable），GUI 用 `eframe` / `egui` 0.35，纯 CPU 渲染 PDF 用 `hayro`。
- PDF 全部由 **Typst** 进程内排版（`typst` 0.15.1，World 在 `src/typst_engine.rs`；
  `typst-layout` vendor 在 `vendor/typst-layout/` 打了标点收缩补丁）。公文模板
  `assets/typst/gongwen.typ`、数据由 `src/export/typst/` 生成；研究报告模板
  `assets/typst/research.typ`、数据由 mdx 的 `typst_research` 整理（`src/export/typst/research.rs`）。
  原 Tectonic / TeX 链路已移除。改版式前先读 `docs/typst-engine.md`，改完出样张
  （`cargo test --locked typst_samples -- --ignored`）目视检查。
- 导出链：Markdown → DOCX（`docx-rs`；研究报告的 Word 由 vendor 的 mdx 生成）/ PDF（Typst）、
  XLSX（`rust_xlsxwriter`）、稿件库与词表用 `rusqlite`。研究报告支持 LaTeX 写法的数学
  公式（`$...$` / `$$...$$`）：预览与导出都用 `latex-rust` 进程内排版（vendor 在
  `vendor/latex-rust/`，加了中文字形回退与数组行距 / 数学轴居中补丁；导出出 SVG 嵌进 PDF）。
- 中文处理：`jieba-rs`（含用户词典）、`pinyin`；文档读取用 `anydoc`。
- 输入法：应用内词表输入法，按完整编码查询基础表与公文四码表，不使用拼音、
  整句模型、自动调频或独立进程。代码在 `src/ime/`，设计与交接见 `docs/table-ime.md`。
  - 初始基础表：本机 `assets/fuma/quan.txt`；打包时若存在，独立复制到
    `runtime/ime/base.txt`，不嵌入二进制。源码中的本机资源不入库。
  - 用户基础表与个人词表：`config_dir()/ime/base.txt`、`tables.json`；基础表可动态
    修改，公文表可导入、停用与撤销。公文词表只加载已接受且为四码的词。
  - 旧 `dict.qj`、`lm.qj`、辅码与拼音学习数据不再使用，也不再随安装包分发。
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
- Linux 变体：Arch/Omarchy 包（已不随 Release 发布，需自行构建）用
  `cargo build --release --locked --no-default-features --features linux-portal-dialogs`。

## 代码组织

单文件超过约 2000 行就该按功能域拆成模块文件夹。已拆过的：`src/app/`、`src/draft_page/`、
`src/preview/`、`src/lexicon/`、`src/export/{docx,typst}/`、`src/ime/`。拆分流程见 skill
`split-rust-module`（纯代码移动，每拆一个文件单独提交一次，零警告验证）。

`skills/gongwen-markdown/` 是给外部 AI 工具用的技能包（Agent Skill），讲的是起草页
Markdown 语法与各文种正文规则，由 `src/skill_pack.rs` 编进二进制、设置页「上手指引」导出，
`scripts/package-portable.ps1` 另打一份 `.skill` 随安装包分发。改了解析器语法、标题编号、
附件或文种规则（`src/export/parse.rs`、`src/prompt.rs`）要同步这里的文档；新增文件要登记进
`skill_pack::FILES`，测试会检查两边一致。

顶层模块清单在 `src/main.rs`。注意 `mod` 声明里 `outline`、`proofread_rules` 等
并非全部集中在文件头部，改动前先 `grep -n "^mod " src/main.rs`。

## 进行中的工作

- **版本变更与花脸稿改造**（Zed 式 diff + 花脸稿预览，分五期）：方案见
  `docs/version-diff-redesign.md`，进度、与方案的出入、下一步和已知坑见
  `docs/version-diff-handoff.md`。接手这项工作前先读这两份；每完成一期更新交接说明。
- **离线稿件身份识别与版本合并**（加密 ZIP 多机往返同步）：方案、实施进度、
  出入与已知坑见 `docs/offline-sync-redesign.md`（单文件）。核心是稿件 UUID +
  不可变版本图（UUID、双父、SHA-256），旧 ZIP 仍可读；改同步/导入逻辑前先读它。
- **Typst 排版引擎**（分支 `feat/typst-engine`，已替代 Tectonic）：六个文种与研究报告、附件、
  横页、联合发文、份号、花脸稿、孤行探针都已接入，移除 TeX 前逐行对照过，**待真机验收**
  （打印、送批材料合并、本机字体、研究报告公式与文献）。设计、对照结论、已知差异与坑见
  `docs/typst-engine.md`（单文件）。
- **AI 工作台智能体化**（替代 AI 起草工作台）：右侧 AI 侧栏 + 流式输出 + SKILL.md 技能
  + 分权限工具 + 自研智能体循环（不引入 rig / rmcp），分五期，**尚未开工**。方案、红线
  修订记录与分期见 `docs/ai-agent-workbench.md`（单文件）。
- **送批材料**（呈批件挂随行件、按提交版合并成一个 PDF、归档钉版、随同步 ZIP 携带）：
  五期已全部实现，**待真机验收**（排版→合并整条链、ZIP 往返）。方案、各期进度、
  与方案的出入和已知坑见 `docs/send-package-design.md`（单文件）。版本与校验值复用离线
  同步的版本图，随行件按稿件 UUID 引用；改导出、归档或同步导入前先读它。

## 三条不可逾越的红线（改 AI 相关代码前必读 `docs/ai-architecture.md`）

> 2026-10-03 按智能体工作台方案修订（`docs/ai-agent-workbench.md` 第二节 D1–D3）。

1. **AI 可以写自己的工作稿，正式稿只能由人合并。** 智能体在 AI 工作稿里可以反复写、反复改；
   `generated_markdown` 只能被两种操作改写：用户键盘输入，以及用户在审阅视图里接受
   提案或采纳修订建议（可以逐块排除）。没有任何工具能直接写正文。
2. **公文要素分两级。** 授权类（密级、文号、份号、签发人、成文日期、印发机关）：AI
   只读不写，也不给建议。描述类（标题、主送、抄送、附件说明）：AI 可以给建议，但值
   必须来自标准词库或材料原文并注明出处，采纳要用户点。AI 不得直接回写 `DraftInput`。
3. **确定性闸门既是工具也是关卡。** `ai_guard`（事实比对与溯源）、`validator`（要素
   校验）、`proofread` / `proofread_rules`（词表与规则复扫）可以由智能体在循环中调用、
   用来自查自改；落地前程序必须再强制跑一遍。**硬闸门**挡住落地（未确认的关键事实
   变化、来源不明的数字和日期、授权要素被改动）；**软闸门**的问题转成修订建议附在
   提案上。小模型逐句复核的产物仍然是闸门不过就丢弃。

推论：确定性规则是 AI 的裁判，不是竞争者，不要为了迁就模型放宽规则。工具可以由
模型自由调用，权限和「做完了没有」由程序判定。

## 版本与发布

- 版本号只维护在两处：`Cargo.toml` 的根包 `version` 和 `Cargo.lock` 里
  `name = "gongwen-assistant"` 的 `version`。
- 本机**没有 `pwsh`**，不要调用 `scripts/bump-version.ps1`，手动编辑上述两处。
- `ci.yml` 在 push `main` 时触发（Windows x64 / Linux ARM64（GLIBC 2.28）两平台跑 fmt / clippy / test；
  另在干净的 Ubuntu 20.04 ARM64 里装 deb 并用 Xvfb 真正启动一次，见 `scripts/smoke-launch-linux.sh`）；
  `release.yml` 在 push `v*` tag 时触发，只产出 4 个资产：Windows x64 setup.exe、
  Linux ARM64 deb（Debian buster 容器构建，GLIBC 锁死 2.28，兼容 Ubuntu 20.04 / 麒麟 V10）
  及各自 `.sha256`。macOS、Linux AMD64、Arch/Omarchy 包不再发布。
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
- **本地 post-commit 钩子**（`scripts/git-hooks/post-commit`，首次使用跑
  `scripts/install-git-hooks.sh`）每次 commit 后自动跑
  `cargo build --release --locked`，保证 `target/release/` 始终是最新二进制；
  Linux 主机自动加 `--no-default-features --features linux-portal-dialogs`。
  失败仅打印告警、不阻塞提交。
- 许可证是 **GPL-3.0-or-later**：项目继续按 GPL 分发；旧输入法内核已移除。改许可证相关的东西要同步 `LICENSE`、`Cargo.toml` 的 `license`、
  `README.md` 许可节、`THIRD_PARTY_NOTICES.md` 四处。
- 基础词表是独立文本资源，用户修改保存在配置目录；不再随包携带拼音词库与语言模型。
- 代码注释、文档、README 一律中文。
- 应用输出的 `dist/`、`output/`、`tmp/`、`target/` 均为生成物，不要提交。
- `config.json`、`.env*` 是本机配置，不入库；`config.example.json` 是模板。

## 环境

- 远程 `git@github.com:billowsand/gongwen.git`，发布分支 `main`。
- 使用 `gh` 管理 release（账号 `billowsand`，需 repo scope）。
