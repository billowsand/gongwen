# AGENTS.md — 公文助手（gongwen）

基于 Rust + egui 的离线中文公文写作桌面应用。模型只起草 Markdown 正文，行文要素、
版式规则与导出结果由本地程序控制。

## Pi 写作最新产品口径（2026-11-04）

用户已确认首版：面向 256K 及以上窗口，按实际容量优先整篇材料，超限按大章节切分；每轮提供当前日期/星期/时区及标准组织背景，词库按职能、人员简介及所属单位查询。Pi 新任务不做事实核查、不产生事实疑点清单，全面审校与政策核查技能退出新任务；现有文字复核、格式与引用保护、授权表单权限、基线和精确撤销保留。此口径取代下文 Pi 事实疑点“只提示”的旧表述，不改变旧提案历史的权限。详见 `docs/pi-implementation/writing-context-v1.md`。

2026-11-04 后续：设置页风格学习也已统一到 Pi 的 `gongwen-style-learn`，支持库内/上传样稿，`submit_style` 交独立JSON草稿，用户编辑保存；不走辅助模型、不写正文。普通写作与风格学习按任务类型隔离技能和交付权限，见 `docs/pi-implementation/style-learning-v1.md`。Pi 内置技能共 12 个（6 个写作/提炼 + 2 个任务助手作业 + 1 个风格学习 + 3 个维基知识库作业；任务作业见 2026-11-10 条目，维基作业见 `docs/wiki-knowledge-base-design.md` 第十三节），全面审校 `gongwen-review` 与政策核查 `gongwen-policy-check` 已退役、只保留历史文件。交付通道按任务类型分四类：正文 `document`（`submit_document`）、清单 `report`（`finish_report`）、独立风格文件 `style`（`submit_style`，只进设置页编辑器、不能写正文）、作业 JSON `task`（`submit_task`，按技能进任务助手或维基审核）。

## 技术栈

- Rust edition 2024（stable），GUI 用 `eframe` / `egui` 0.37，纯 CPU 渲染 PDF 用 `hayro`。
- PDF 全部由 **Typst** 进程内排版（`typst` 0.15.1，World 在 `src/typst_engine.rs`；
  `typst-layout` vendor 在 `vendor/typst-layout/` 打了标点收缩补丁）。公文模板
  `assets/typst/gongwen.typ`、数据由 `src/export/typst/` 生成；研究报告模板
  `assets/typst/research.typ`、数据由 mdx 的 `typst_research` 整理（`src/export/typst/research.rs`）；
  公函签收记录单模板 `assets/typst/receipt.typ`、数据由 `src/export/typst/receipt.rs` 生成
  （Word 侧同一份数据，见 `src/export/docx/receipt.rs`）。
  原 Tectonic / TeX 链路已移除。改版式前先读 `docs/typst-engine.md`，改完出样张
  （`cargo test --locked typst_samples -- --ignored`）目视检查。
- 导出链：Markdown → DOCX（`docx-rs`；研究报告的 Word 由 vendor 的 mdx 生成）/ PDF（Typst）、
  XLSX（`rust_xlsxwriter`）、稿件库与词表用 `rusqlite`。研究报告支持 LaTeX 写法的数学
  公式（`$...$` / `$$...$$`）：预览与导出都用 `latex-rust` 进程内排版（vendor 在
  `vendor/latex-rust/`，加了中文字形回退与数组行距 / 数学轴居中补丁；导出出 SVG 嵌进 PDF）。
- 中文处理：`jieba-rs`（含用户词典）、`pinyin`；文档读取用 `anydoc`（vendor 在 `vendor/anydoc/`，
  补丁公开了文档模型的渲染函数，起草页导入据此把内嵌图片入库，见 `src/doc_import/embedded.rs`；
  EMF / WMF 矢量图经 `emf-core` 转 SVG 再由 resvg 出 PNG，见 `src/metafile.rs`）。
- 输入法：应用内词表输入法，按完整编码查询基础表与公文四码表，不使用拼音、
  整句模型、自动调频或独立进程。代码在 `src/ime/`，设计与交接见 `docs/table-ime.md`。
  - 初始基础表：`assets/fuma/quan.txt`，由本项目自行维护并纳入版本管理；所有平台
    （Windows 安装包、Linux ARM64 deb、macOS 应用包）统一随包携带，打包时复制为
    `runtime/ime/base.txt`，不嵌入二进制。词表只收公文可用词，口语、方言、网络语、
    表情符号与亲属口头称谓不入库，维护口径见 `docs/table-ime.md`。旧本机码表
    `mabiao.txt` 只作旧机后备。
  - 用户基础表与个人词表：`config_dir()/ime/base.txt`、`tables.json`；基础表可动态
    修改，公文表可导入、停用与撤销。公文词表只加载已接受且为四码的词。
  - 旧 `dict.qj`、`lm.qj`、辅码与拼音学习数据不再使用，也不再随安装包分发。
- 模型接入：本机 LM Studio / Ollama，走 OpenAI 兼容接口（`src/lmstudio.rs`、
  `src/rag.rs`、`src/rag_client.rs`）。**2026-11-05 起模型通道收敛为两个用途**：
  写作模型（Pi，`pi_model`）同时服务知识库问答、检索重排、数据接口 AI 调用
  （`AppConfig::aux_chat`）；**任务助手的材料整理与表单填写自 2026-11-10 起走 Pi 作业**
  （`src/pi_agent/task_jobs.rs`，技能 `gongwen-task-extract` / `gongwen-task-fill`，
  交付 `submit_task`），不再走一次性调用；文字复核（`revise_model`）是唯一保留的专门小模型。
  「辅助模型」设置已删除，`draft_model` 只作旧配置兼容回落。

## 常用命令

```bash
cargo run -- --new-window      # 开发运行（不加时，本机开着新版公文助手会把启动转交给它后退出）
cargo check --all-targets      # 快速基线
cargo fmt --all -- --check     # CI 零容忍，改完必跑
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --release --locked
python3 scripts/build-editions.py  # 同时产出 AI 全版与非 AI 版二进制
```

- 双版本构建及功能特性见 `docs/edition-build-design.md`。增加可选模块时一起处理启动、后台事件、界面导航、快捷键和内嵌资源，并检查关闭特性后的构建。

- 开发构建给依赖开 `opt-level = 3`（见 `Cargo.toml`），首次编译依赖约需一分钟，属正常。
- `cargo test` 有**约 5 个与沙箱环境相关的失败，是预期结果**，不要为此改动测试。
- Linux 变体：Arch/Omarchy 包（随 Release 发布 x86_64 的 `.pkg.tar.zst`）用
  `cargo build --release --locked --no-default-features --features ai,linux-portal-dialogs`。
- **外部数据源与耗时任务不能占住主线程**。Wayland 合成器（Hyprland 等）每 1.5 秒给客户端发一次
  存活探测，主线程一停就弹「应用无响应」系统对话框。系统文件框一律走
  `src/app/file_dialog.rs` 的 `request_file` / `request_files`：portal 后端（Linux/Wayland
  变体）在后台线程开窗、主线程每帧问一次结果；GTK3 后端是本进程自己的窗口，只能留在
  主线程（即 `linux-gtk-dialogs` 变体仍是阻塞式）。起草页、输入法词表页要开文件框时，
  登记 `DraftAction` / `PageAction` 交给应用层，不要在页面里直接调 `rfd`。剪贴板读取、
  富文本转换、密码派生、稿件包复制/解密/数据库预览等也必须在后台线程执行；完成后
  发送 `WorkerResult` 并 `ctx.request_repaint()`，主线程只接收结果与更新界面状态。

## 代码组织

单文件超过约 2000 行就该按功能域拆成模块文件夹。已拆过的：`src/app/`、`src/draft_page/`、
`src/preview/`、`src/lexicon/`、`src/export/{docx,typst}/`、`src/ime/`。拆分流程见 skill
`split-rust-module`（纯代码移动，每拆一个文件单独提交一次，零警告验证）。

`skills/gongwen-markdown/` 是给外部 AI 工具用的技能包（Agent Skill），讲的是起草页
Markdown 语法与各文种正文规则，由 `src/skill_pack.rs` 编进二进制、AI 管理页「AI 技能包」导出，
`scripts/package-portable.ps1` 另打一份 `.skill` 随安装包分发。改了解析器语法、标题编号、
附件或文种规则（`src/export/parse.rs`、`src/prompt.rs`）要同步这里的文档；新增文件要登记进
`skill_pack::FILES`，测试会检查两边一致。

改完技能包跑三样：`cargo test --locked skill_pack`（模板能被解析、frontmatter 合规范）、
`python skills/gongwen-markdown/scripts/check_gongwen_md.py --selftest`（判据自测）、
`cargo test --locked model_outputs_survive -- --ignored`（拿模型产物过解析器与校验器，
需 `GONGWEN_SKILL_EVAL_MANIFEST` 指向评测 run 的 `manifest.json`）。要知道「模型照这份文档
能不能写对」用 `python scripts/gongwen-skill-eval.py`（密钥只从 gitignore 的
`model_api_test.md.txt` 或环境变量读），方法与最近一次实测数字见
`docs/gongwen-skill-model-eval.md`。

用户手册/使用帮助已退役（2026-11-12）：不再内置帮助正文、截图、菜单入口与 `F1`，
也不再维护 docs 手册。旧版留在 `images/` 的六张帮助截图与配置目录 `help/` 下的 PDF
只由 `src/retired_help.rs` 和备份排除规则做兼容，不要重新引入手册维护流程。

顶层模块清单在 `src/main.rs`。注意 `mod` 声明里 `outline`、`proofread_rules` 等
并非全部集中在文件头部，改动前先 `grep -n "^mod " src/main.rs`。

## 进行中的工作

- **维基知识库（资料库 → 知识库，2026-11-10）**：一、二、三期十个切片全部实现，
  **待真机验收**。2026-11-11 已复审并修复作业交付、版本溯源、旧提案覆盖与链接等问题；
  同日全流程复审又修了节点建/改名后的红链回填（含导览页删除保护）、自动取证只认有依据页、
  FTS 引号、审核台 diff 高度/双击防抖/版本窗串页、缺口清单滚动、页面改名与删除入口、
  host 与 Rust 闸门对齐（重名/uuid/双导览/change_note）、更新需复核页的材料矛盾等，
  并用 11-material 真实资料跑了真机评测，详见该文第十三节「2026-11-11 全流程复审与修复」；
  同日真机评测（DMXAPI deepseek-v4-flash）跑通归纳→整编→更新全链，评测驱动的修复：
  空依据分类进闸门、更新作业标明目标页类型、结构上限与〔E编号〕交叉校验前移 host 侧
  `submit_task`（模型轮内可改正，不再跑完整轮拒收）。ignored 真机评测见
  `src/pi_agent/wiki_eval.rs`（需 `DMXAPI_API_KEY` + `source scripts/pi-dev.sh`）。
  方案、分期、复审记录及已知差距见 `docs/wiki-knowledge-base-design.md` 第十三节。要点：存储层
  （`src/wiki.rs`，与稿件库同文件独立连接）；框架建立（词库式树编辑 + 大纲导入 +
  `gongwen-wiki-frame` AI 归纳）；页面整编 / 更新（`gongwen-wiki-write` /
  `gongwen-wiki-update` Pi 作业，交付 JSON 经确定性复核进审核台，人工逐页审核可编辑，
  采纳由界面线程写库，版本带来源与「审核已修改」标记）；缺口清单（红链聚合 +
  建页自动解析 + 空分类整编）；资料库联动（「维基」列、新资料关联提示、圈选整编）；
  写作侧四只读工具 + 任务助手接入（12 次/轮独立额度）；问答改维基优先双层。
  正文使用 Wiki 专用版式，阅读器支持 Markdown 源码与双栏对照，配色全取主题语义槽位。
  Pi 内置技能共 12 个（含 3 个维基作业技能）。发布前需重建随包 `runtime/pi`
  （bridge.mjs 加了四个域工具）。

- **智能体改造现状与退役收尾（2026-11-05）**：权威进度入口见 `docs/pi-current-status.md`，
  全面审查见 `docs/ai-agent-retirement-audit-2026-11-05.md`，本轮修复与实际验证见
  `docs/pi-implementation/retirement-fixes-2026-11-05.md`。新任务已统一 Pi；旧写作页只读，
  旧侧栏检查点在 5D/5E 门槛通过前仍可恢复执行。不要把历史报告中的待办当作当前需求，
  也不要整目录删除仍含 API 共享服务的 `src/agent/`。旧 DSL 第 5 期、goto/loop/子图与
  固定决策流程不再继续实现，新任务使用 Pi 阶段/预算/提问。
  本轮额度/备用模型/共享依赖已修复，孤立旧管理器已清理；当时为 host 0.3.6、协议 3
  （2026-11-06 已升到 host 0.4.0、协议 4，见下文「应用锁与安全存储」第六轮），
  Windows 运行时已重建并验收。Rust 2896 通过/89 忽略、Node 121 通过/1 跳过；
  真实模型与目标机门槛仍未通过，不据此删除旧历史执行链。

- **版本变更与花脸稿改造**（Zed 式 diff + 花脸稿预览，分五期）：方案见
  `docs/version-diff-redesign.md`，进度、与方案的出入、下一步和已知坑见
  `docs/version-diff-handoff.md`。接手这项工作前先读这两份；每完成一期更新交接说明。
- **离线稿件身份识别与版本合并**（加密 ZIP 多机往返同步）：方案、实施进度、
  出入与已知坑见 `docs/offline-sync-redesign.md`（单文件）。核心是稿件 UUID +
  不可变版本图（UUID、双父、SHA-256），旧 ZIP 仍可读；改同步/导入逻辑前先读它。
- **Typst 排版引擎**（分支 `feat/typst-engine`，已替代 Tectonic）：六个文种与研究报告、附件、
  横页、联合发文、份号、花脸稿、孤行探针、公函签收记录单（第三套模板
  `assets/typst/receipt.typ`）都已接入，移除 TeX 前逐行对照过，**待真机验收**
  （打印、送批材料合并、本机字体、研究报告公式与文献）。设计、对照结论、已知差异与坑见
  `docs/typst-engine.md`（单文件）。
- **AI 工作台智能体化**（已替代 AI 起草工作台）：右侧 AI 侧栏 + 流式输出 + SKILL.md 技能
  + 数据接口（内网 HTTP）+ 声明式流程引擎，不引入外部智能体框架。**第 ①（流式输出 + 侧栏骨架）、
  ②（研究式起草 + 选择题澄清 + SKILL.md）、③（工具 → 算子 → 技能三层、流程引擎、技能条）、
  ④（数据接口 `http.call`、技能 / 数据接口 / 工具调试台管理界面）、⑤（12 个内置技能：起草、修改、审核三类；
  旧的 AI 起草工作台已删除）、⑥（AI 设置全部收进 AI 管理页 `src/app/ai_manage.rs`；一体化输入框，`/` 技能与
  `@` 引用文章在光标处弹出，`@` 的文章按技能 `references:` 当证据 / 基准稿 / 材料 / 来函）期已完成**，代码在
  `src/agent/`（`tools/`、`ops/`、`engine.rs`、`references.rs`、`skill.rs`、`skill_files.rs`、`api.rs`、`router.rs`）、
  `assets/agent-skills/`、`src/ai_panel/`（`composer_ui.rs`、`mention.rs`）与 `src/app/agent_settings.rs`。
  第 ⑦ 期（`agent` 自主步骤：原生工具调用 + 文本兜底，`src/agent/ops/agent.rs`、`toolcall.rs`、
  `lmstudio/converse.rs`；政策依据核对 `cite_check`；兜底技能「自由任务」）已完成，DMXAPI 上的 DeepSeek-V4-Flash 与 MiniMax-M2.7 真机跑通；
  第 ⑧ 期（上下文窗口与压缩 `lmstudio/context.rs`、`agent/budget.rs`；会话随稿件保存与追问 `ai_panel/session.rs`、
  `history.rs`、`manuscript/ai_sessions.rs`；风格管理 `agent/style.rs`、`app/style_settings.rs`、技能「风格学习」）已完成，
  DMXAPI 上真机跑通。模型配置已提供商化：地址密钥收进「模型服务商管理」
  （`src/app/provider_settings.rs`，预设模板 + 提供商卡片），「模型服务」按功能
  （写作 / 复核 / 知识库）选模型（`ModelRef` 引用 + `AppConfig` 中央解析器），
  旧内联地址字段随启动自动迁移；详见 `docs/ai-agent-workbench.md` 第十三节。对比分析、数据分析段落、正文区审阅再讨论（见第十六节 F16；opencode 接入已作废）。
  方案、红线修订记录、分期、各期进度与已知坑见 `docs/ai-agent-workbench.md`（单文件，第十三节是交接）。
  **2026-11-04 审查后 Pi 修复（待真机验收）**：单轮执行上限改为 30 分钟（等用户回答不计，
  卡片显示「执行上限 30 分钟」）；工具调用常规 60 次、分阶段由宿主按阶段追加、硬上限 84 次；
  事实来源改为 `sources/items/<sha256>.md` + `sources/index-<hash>.json`（单项 2 MiB / 索引
  512 KiB / 恢复读取 8 MiB，旧格式可读），落盘失败不再中止本轮；运行目录按稿件保留最近 3 轮
  并新增自动清理与 AI 管理页占用/清理入口；生产 host 改为 `env_clear` 后最小环境白名单并透传
  代理变量；macOS `darwin-arm64` / `darwin-x64` 运行时、打包、签名公证（可选）与 CI 骨架已补齐，
  待真机与发布验证。细节与已知限制见 `docs/pi-runtime.md`。
  **2026-11-04 Pi host 0.3.5（待真机验收）**：交付通道与内置技能双向对齐（五个正文技能
  只能 `submit_document`、提炼技能只能 `finish_report`、风格学习只能 `submit_style`，
  Rust 与 host 两侧都拒错，恢复只认启动前已存在的同类回执）；分阶段依赖改由宿主自动补齐，
  模型不再手填 `uses`、重复登记 `sources` 会被拒绝并回可用单元编号；读工具按预算声明
  `outputLimits`，修掉 Durable Harness 默认上限二次截断长材料的问题；技能提示补「当前日期
  只解释相对时间，不是事项发生日」与风格档案不得编造禁令。评测新增日期人员、整篇长材料、
  风格学习三个用例，退役的政策核查用例删除。
- **AI 模型通道统一（2026-11-05，已完成，待真机验收）**：用户拍板把模型收敛为两个用途——
  写作模型（Pi）与文字复核小模型。删除「辅助模型」设置；知识库问答、检索重排、数据接口 AI
  调用统一走 `AppConfig::aux_chat`（Pi，旧配置回落 `draft_model`）；任务助手随后改走
  Pi 作业；文字复核
  只留启用/模型/检查器（送检范围、超时、窗口用程序默认值）；Pi 新增只读数据接口工具
  `api_search` / `api_call`。方案、决策与实施记录见
  `docs/ai-model-unification-design.md`；Pi host 源码已改，发布前需重建随包 `runtime/pi`。
- **任务助手 AI 全面转 Pi 智能体（2026-11-10，已实现，待真机验收）**：材料整理与单任务
  填写都改由 Pi 作业执行（host 0.5.0、协议仍 4），新增内置技能 `gongwen-task-extract` /
  `gongwen-task-fill`、交付通道 `submit_task`（`kind = task`，JSON）与只读工具
  `task_search` / `task_read`；随之放开单次调用的门禁：不再只注入最近 50 项任务、材料按
  512 KiB 分块最多 4 MiB、候选上限 100 条、任务作业不占写作门禁（可与 Pi 写作并行）。
  证据、日期、候选类型与目标 ID 的确定性复核不变；未配 Pi 模型或运行时缺失时任务助手
  不可用。改前读 `docs/pi-runtime.md`「任务助手作业」一节。
- **Pi 可靠性与写入体验第二轮（2026-11-05，已完成，待真机验收）**：按
  `docs/pi-implementation/reliability-review-2026-11-05.md` 修复——数据接口禁跟随重定向、
  清理只删真孤儿、接口截断如实上报、独立复核不依赖写作配置；停止后域工具在阶段边界收手、
  在途槽位上限 4；接口长结果可 `api_read` 续读（缓存落盘到运行目录，恢复后可读）。
  同一轮把 Pi 写入改成「写入前自动提交版本 + 直接覆盖正文」，去掉 AI 工作稿层与 Pi
  差异视图。后续只剩发布验收（压力、真实模型/接口、多平台）与旧运行时重建。
- **公文要素填写框架（2026-11-05 已退役）**：表单要素抽取（AI 按标准词库从材料里建议
  主送、抄送、呈报领导、承办、联系人、联系电话、发文、落款）曾分四期实现，入口是旧 AI
  写作流程的 `clarify` 步；统一写作改走 Pi 后新任务已不再使用它，用户拍板**直接退役、不迁
  到 Pi**。旧流程里的抽取生成链路已删除；`Target::Field`、`resolve_answers`、
  `board.field_suggestions`、`Event::FieldSuggestions` 与 `src/ai_panel/field_card.rs`
  仅保留给旧会话回放/采纳，旧技能文件也不动。Pi 写作只读表单（`doc_elements`），缺要素
  写【待核实】；红线 2 不变（写表单只能用户点采纳走 `element_fields::apply_field`，
  授权类不出题不建议）。历史方案见 `docs/element-fill-design.md`（已标退役）。
- **数据接口工作台**（以 OpenAPI 为中心重做数据接口）：要的是三个结果——接口调通、用测试集
  （用例 + 期望）证明接口可用、大模型能正确调用（AI 用例：一句问题 → 期望的接口与参数）；不另做
  调试台界面，都放在现有「试一下」里。**第一期（中心格式、标准文件导入导出、旧 `apis.json`
  迁移）、第二期（接口用例：期望、整组跑一遍留记录、会改数据的接口确认后人工发送）、第三期
  （AI 调用：工具参数带 schema 与调用前校验、接口多时两级访问、「说一句话让 AI 调」与 AI 用例）
  已完成，待真机验收**。**2026-11-05 起 Pi 写作也能调只读接口**：新增领域工具
  `api_search`（按用途搜只读接口）、`api_call`（按签名填参、调用前校验、返回进证据）与
  `api_read`（按 `result_id` 列出全部条目、分页读单条全文；结果缓存落盘到运行目录，恢复后可续读），
  只开放 `readonly && ai && 配置受支持` 的接口；旧 `http.call` 仍只服务旧会话。接口存在
  `配置目录/apis/<服务>.openapi.json`，代码在 `src/agent/apidef/`，`ApiEndpoint` 是编译产物、
  存盘时写回（`apidef::absorb`）。方案、各期进度、出入与已知坑见 `docs/api-workbench.md`
  （第十二节是交接）；替代 `docs/http-skill-design.md` 第一节起的接口类技能方案。
- **送批材料**（呈批件挂随行件、按提交版合并成一个 PDF、归档钉版、随同步 ZIP 携带）：
  五期已全部实现，**待真机验收**（排版→合并整条链、ZIP 往返）。方案、各期进度、
  与方案的出入和已知坑见 `docs/send-package-design.md`（单文件）。版本与校验值复用离线
  同步的版本图，随行件按稿件 UUID 引用；改导出、归档或同步导入前先读它。
- **导出 Typst 包**（模板 + `doc.json` + 插图 / Mermaid / 公式 SVG / `references.bib`
  打成 zip，不含字体；`doc.json` 的 fonts 换成系统字体别名数组，解压后
  `typst compile gongwen.typ` / `research.typ` 即可编译）：已实现，**待真机验收**
  （装有常见中文字体的机器实际编译一次）。代码在 `src/export/typst/package.rs`，
  方案与包结构见 `docs/typst-engine.md`「导出 Typst 包」一节。模板的字体角色同时接受
  字符串与数组，改 `fam()` / `chain()` 前先读那一节。
- **应用内打印对话框**（取代 Windows 老式 `PrintDlg` 与 Unix 上直冲默认打印机的 `lp`，
  三平台同一个对话框，分四期）：**四期均已实现，待真机验收**。第一期是选打印机与状态、
  份数、逐份、页码范围、记住上次的打印机；Windows 改用 DEVMODE + `CreateDCW`，按实际大小与
  纸张方向出图，修掉缩印约 4% 与横页缩到七成的问题。第二期是双面、颜色、纸张、纸盒、缩放
  五项（能力查询 `printer_caps`、CUPS `lpoptions -l` 解析、Windows `DeviceCapabilitiesW`）、
  按稿件给双面默认值与五条公文提醒（`PrintHints` + `src/print/inspect.rs` 从 PDF 自身测横排
  与红色）、Windows「打印机首选项…」（`edit_preferences` 的整块 DEVMODE）。第三期是
  按打印机记偏好（`PrintConfig.printers`）、手动双面（`pages::manual_duplex_passes` 与
  对话框状态机）、打印进度与取消（`print(job, progress, cancel)`、CUPS 任务号回读）、
  页面预览（`PdfSession::page_texture`，不为预览另起渲染线程）。第四期是首选项逻辑去掉
  `#[cfg(windows)]`、按 `print::HAS_DRIVER_PREFERENCES` 决定画不画按钮（macOS 可测、
  Windows 交叉检查能覆盖）、预览按需渲染（`preview_wanted` / `pump_preview` 复用 PDF 标签
  的队列）并按真实尺寸摆放（`src/print/dialog/preview.rs` 的 `preview_rect`）、起草页
  「打印…」导出 PDF 后自动弹对话框（`print_after_export` + `PdfSession::print_when_ready`）。
  代码在 `src/print/`，外壳在 `src/app/printing.rs`。Windows 代码在 macOS 上没法随整个项目
  交叉编译，交叉检查办法（仓库外小 crate 软链 `src/print/`，第四期起 `dialog/` 也用真文件）、
  方案、进度与已知坑见 `docs/print-dialog-design.md`（单文件，第十一节是交接）。
- **AI 智能体内核加固**（2026-10-07 重排为五期：工具契约（复用 `apidef::tooling` 的校验与
  两级访问）→ 提案逐块接受 → 用量 / 提案卡留痕 / 备用模型 → 每步检查点与崩溃续跑 → 全流程
  预算与算子内只读并行；控制流 `goto` / `loop` 与子图暂缓）：**第 1 期（工具契约）、第 2 期
  （提案逐块接受：排除后合并并重过闸门）、第 3 期（用量、提案卡留痕、备用模型、检索耗时）
  已完成、均待内网真机验收；第 4 期（每步检查点与崩溃续跑：挂起即一种检查点、恢复只有一个
  入口、只读区按界面当前值重灌、中断 / 停止 / 出错的卡片可「接着跑 / 从头重来 / 丢弃」、
  从旧检查点重跑）已完成，接口联机验证通过、**待 GUI 真机验收**，4/5 期**。第 3 期已通过七牛
  DeepSeek V4 Flash 与 MiniMax M2.7 的接口实测；第 4 期的检查点外键挂在 `ai_session_turns`
  上（轮次删了检查点也删），开跑前先强制存一次会话。
  方案、分期、验收、风险与交接见 `docs/agent-kernel-hardening.md`（单文件）。只加固自研内核，
  不做通用框架、不拆 crate、不引入 async；检查点那期「挂起即一种检查点、恢复只有一个入口」与
  `StepPath` 路径不能简化，返工代价最大。动 `src/agent/` 的编排代码或提案接受前先读它，并先读三条红线。

- **稿件包专用后缀 `.gwx`**（同步 ZIP 换成 `.gwx`，双击 / 拖放 / 启动参数直接进现有导入流程，
  包内格式不变、旧 `.zip` 照常导入，分三步）：**第一步（后缀与统一入口 `receive_import_package`、
  暂存副本与排队、启动参数、拖放）、第二步（已开窗口转交：`src/single_instance.rs` 锁文件 + 收件目录）
  、第三步（Inno 注册表登记、Linux MIME / desktop、
  `assets/doc-icon/` 文档图标）均已实现，待真机验收**（macOS 未接 Apple Event open-file，
  `.gwx` 双击在 macOS 不生效，用拖放或应用内导入；见 `docs/pi-runtime.md`）。方案、进度与已知坑见 `docs/gwx-package-design.md`（单文件，第十一节是进度）。
  **2026-11-07**：新导出 `.gwx` 已改为 7z AES-256（内容与包内文件名一起加密），
  PDF 集合改 `.7z`；旧 ZIP 格式 `.gwx` / `.zip` 仍按文件头识别导入，清单 v1/v2/v3
  与旧密码兼容。新包不能由旧程序读取，不能再靠改 `.zip` 后缀互传。见方案文末。

- **人机决策模块**（把「问用户」从算子里抽成可组合的决策：决策形态随挂起交给界面、通用清单确认
  与修订、待决事项、自主步骤提问，分四期）：方案、分期、进度与已知坑见 `docs/decision-modules.md`
  （单文件，第十节是交接）。**第 ①（决策形态 + 通用清单确认 `confirm`）、②（待决事项 +
  事实冲突候选）、③（自主步骤 `ask_user`：提问即结束本轮）、④（多选取舍、`when` 取值比较、
  证据取舍 `pick_evidence`、方案比选 `compare`）期已完成，待真机验收**。决策只改
  黑板与工作稿，授权类要素不出题；改 `clarify.rs`、`engine::apply_answers`、`ops/confirm.rs`、
  `ops/agent.rs` 的提问部分或侧栏题目卡片前先读它。

- **富文本粘贴转 Markdown**（起草页 `Ctrl+V` 把 Word / WPS / 网页 / Excel 的富文本与多行纯文本
  转成公文助手 Markdown，`Ctrl+Shift+V` 原样粘贴，智能粘贴后可「改为原样粘贴」，分四期）：方案与
  分期见 `docs/rich-paste-design.md`（单文件，第十一节是交接，第十二节是收尾与真机验收清单）。
  **开发已收尾，待真机验收**：四期都已完成，只有第四期 B 块（Word 公式）等 `real-word-equation`
  真实样本再单独做（采样命令见 `tests/fixtures/paste/README.md`）。
  代码在
  `src/rich_paste/`（`html.rs` 容错解析、预清洗与文框识别、`syntax.rs` 按文种裁语法、
  `images.rs` 剪贴板图片解析与未导入原因、`headings.rs` 按所在章节平移样式标题、`lists.rs` 列表整形、
  `pipeline.rs` 汇到 `clean` + `structure` + 行内粘贴 + 侧栏材料 + 图片入库、`clipboard.rs` 读剪贴板）与
  `src/draft_page/paste.rs`（接管 `Event::Paste`、块级 / 行内分流、`replace_with_undo` 写入、
  「改为原样粘贴」）；编号规则（点分 / 罗马 / 字母 / 跳号报告）在 `src/doc_import/structure/`
  （`numbering.rs` 的 `Numbering`、`mod.rs` 的 `restructure_with_report`）；
  `vendor/anydoc` 新增 `anydoc::html` 公开入口（补丁二）与 `to_document_with_images`（补丁三：
  回调给字节，库自己不联网不读盘），夹具与采样工具在 `tests/fixtures/paste/`。补做公式前先读交接里
  第三、四期两段（`html::parse` 的 `boxes` 参数、`<img>` 分支、`embedded::render`、单行接管的
  门槛、`prune` 要保留 `assets`）；字体线索消歧单列为待定。

- **会议纪要文种**（按 `examples/huiyijiyao.docx`：红头纪要标志与编号、正文「一是」楷体、
  出席 / 列席 / 请假名单、分送 / 抄送 / 印发版记，分三期）：**第一期（模型、表单、校验与
  Typst PDF）、第二期（Word 与预览）、第三期（提示词、技能包、使用帮助）已完成，待真机验收**
  （界面预览目视、真实模型技能评测）。方案、进度、
  与方案的出入和已知坑见 `docs/meeting-minutes-design.md`（单文件，第十节是交接）。
  三端共用的文字规则在 `src/export/minutes.rs`，改「一是」楷体或名单排法只改那里。

- **公函两类随行单据**（纸质《公文签收记录单》+ 电子《传发任务单》，只出 PDF、
  每家接收单位一份）：**已实现，待真机验收**（真机导出后目视单据与目录、
  多单位各一份、只导 Word 时也出传输单）。方案与实现取舍见
  `docs/signoff-transmission-design.md`（单文件）。要点：模板 `assets/typst/transmit.typ`
  （`Template::Transmit`，照用户给的 Word 模板复刻），数据 `src/export/typst/transmit.rs`，
  接线在 `src/export/mod.rs::export_artifacts`；**单子必须只有一页**，靠收紧阶梯
  （`FITS` 七档：题名空白 → 题名字号字距 → 表格字号 → 行高备注 → 页边距）排版后数页数
  重排实现，`TypstOutcome.pages` 为此加的字段；题名是**传输任务字典项**（设置页
  「加密传输单」节内增删并挑默认题名，留空按密级自动带；表单不逐稿选题名，
  `resolve_transmit_task`：旧稿件字段 → `TransmitDefaults.task` → 按密级自动），
  不是公文标题，公文标题印在表内
  「标题」一栏；收件人收在表单专门的「**传输单**」分段（`FormSection::Transmit`，
  仅公函的第 4 个分段，不在主体也不在版记；缩略导航图纸上三段、下方另画一张随行
  单据小块），按主送 + 抄送逐个单位登记，单位名前标「主送 / 抄送」；
  传输单文件名 `传发任务单-<单位>.pdf` **不带文种主干名**（带上会超
  42 字符上限、把公文标题截短）；设置页「公文排版」组下有「**加密传输单**」一节
  （`AppConfig.transmit_defaults`），配题名、传输任务字典、传字（只填「传」字，号码留白，不加方括号与横线）、默认签批人
  （可「留空手签」，空白栏不画横线）、时效要求（默认「急」）、发后处理（销毁 / 退还）、备注（可粘多行，
  按模板事例排成编号条目：仿宋小四、固定行距 14pt）、保密负责单位（末行整体替换占位，
  超长只横向压缩不换行），导出时照填。

- **跟随 Omarchy 主题**（读 Omarchy 当前主题的 `colors.toml` 现算界面配色，分三期）：
  **第一期（探测、解析、换算、`Theme::on_accent`、`ThemeName::Omarchy` 与设置页 / 菜单入口、
  23 套官方样本对比度测试）、第二期（后台每秒轮询即时跟随 `omarchy/watch.rs`、
  `theme::square_chrome()` 直角平面外壳）已完成，待 Omarchy 真机验收**；第三期（`gongwen.toml`
  覆盖、色卡样张）未开工。代码在 `src/theme/omarchy/`，样本在 `tests/fixtures/omarchy/`。方案、出入与交接见
  `docs/omarchy-theme-design.md`（单文件，第九节是交接）。只读用户的 Omarchy 配置，不写钩子与模板。

- **任务助手界面改版与「粘贴填表」**（三栏布局 + 视图栏、表单内「从文字填写」、待整理建议视图，
  分三期）：**第一期（三栏骨架、视图栏与计数、列表 / 详情 / 表单重排、中文日历、分类弹窗、
  流程断点 F1–F10、按数据代号缓存）已完成，待 GUI 真机验收**；第二期（表单「从文字填写」、
  材料标题选填）已完成，待真机验收；第三期（候选按材料分组与旧新对照、详情内粘贴更新）未开工。审阅结论、方案、
  进度与出入见 `docs/task-assistant-ui-redesign.md`（单文件，第六节是交接）；数据规则见
  `docs/personal-task-assistant.md`。代码在 `src/task_ui/`。本次要素精简已按第七节改为
  名称唯一必填、词库负责人、当前进展、下一步、风险与应对，保留状态与任务期限，待真机验收。
  第八节材料库（视图栏「材料」、重新识别只替换未处理建议、识别过的材料出卡片、删除时被引用的
  快照只隐藏，`src/task_ui/materials.rs`）已实现，待真机验收。

- **应用锁与安全存储**（双密码锁屏 + 诱饵空系统 + SQLCipher 全库加密 + 敏感文件
  AEAD，分三期）：**第一期（锁屏 + 双密码 + 诱饵 `alt-` 前缀 + 闲置自动锁定）与
  第二期（rusqlite 换 `bundled-sqlcipher-vendored-openssl`、DEK 包裹架构、解锁时
  `sqlcipher_export` 整库迁移、`config.json` 与密钥类文件 AEAD、停用解密、备份快照
  保持密文）已完成开发，待真机验收**；第三期（`images/`、`pi/` 运行目录加密、日志
  清理、使用帮助威胁模型）**开发已收尾、待真机验收**：图片、PDF 插图、Mermaid 缓存、
  Pi 工作区与 Durable 会话、个人词表、自定义技能、使用统计和诊断日志均已纳入保护；
  双槽备份/恢复状态机与 GWA2 已接通；加密工作区允许打印，但每次先确认队列/纸件
  风险（第七轮）。最终威胁模型与多平台、真实模型/长会话验收仍未完成。方案、分期、与方案的出入和踩坑见
  `docs/security-lock-design.md`（第八节是一期交接、第九节是二期交接与真机验收清单）；
  动密钥句柄、迁移、开库路径或密封写路径前先读第九节「二期踩过的坑」。
  代码在 `src/vault.rs`、`src/app/gate.rs`、`src/app/vault_settings.rs`、
  `src/storage.rs::apply_db_key`（六个开库点调用）、`src/data_backup.rs`、
  `src/ai_services/persistence.rs`。本机构建 vendored OpenSSL 需便携 Strawberry Perl，
  cargo 命令前 `export PERL="C:/Users/lisiy/perl/perl/bin/perl.exe"`。
  **2026-11-06 第三期细化**：资源全生命周期方案见 `docs/encrypted-resource-lifecycle.md`，
  覆盖导入/历史引用/稿件包/全量备份恢复/中间文件/Pi；Mermaid 缓存也需保护。
  首先接入加密模式的单次输出风险确认（`src/app/output_guard.rs`）；资源加密与
  迁移尚未完成，勿把确认框当作第三期已完成。实施状态与验证记录见该文末节。
  **2026-11-06 C 阶段前置（已实现、待真机验收）**：PDF 附件改为内存预览；
  研究报告 Markdown/文献/插图/Mermaid 使用内存快照，mdx 与 Typst 均禁止磁盘回落；
  Word PDF 插图进程内渲染，取消 Poppler 临时图。尚未开放图片密文写入/迁移，
  B/C 未整体完成；后续按 `docs/encrypted-resource-lifecycle.md` 末节继续。
  **2026-11-06 第二轮加固（已实现、待真机验收）**：对阶段 A 与二期加密做三路审计后
  修复了一批不依赖 B–E 的实现缺陷——明文物出确认补齐 13 个漏网入口（风格档案、标准
  词库、全量备份、四码词表、自定义技能、输入法词表、校对词表、OpenAPI、稿件包），
  确认改 fail-closed；密封名单统一并补齐 `api-envs`/`api-tests`/`styles`，接口定义与
  风格档案落密文；`storage::load` 不再把「密封但无密钥」当损坏改名、无密钥拒写明文，
  闸门增加「锁文件丢失但磁盘已有加密数据」状态；停用标记改 DEK 密封且只允许真实槽；
  备份排除停用标记、双槽库只对当前槽做带密钥快照、恢复后强制重解锁；`images/`、
  `ime/`、`skills/`、`metrics`、Pi 校准改槽位目录，预览副本/份号清单/粘贴临时文件/
  日志/收件箱/staging 的明文与权限收紧。
  **2026-11-06 第四轮（B 阶段资源加密，已实现、待真机验收）**：图片、PDF 插图与
  Mermaid 缓存整文件 AEAD，写入/读取/迁移/停用解密/稿件包与同步包往返全部接通；
  图表缓存同时按槽位隔离。剩下 Pi 运行目录与会话加密（E）、打印内存化、备份恢复
  状态机（D）与文件 AEAD 的 AAD/格式版本，详见 `docs/encrypted-resource-lifecycle.md`
  末节。
  **2026-11-06 第五轮（明文落盘审查修复，已实现、待真机验收）**：资源/接口迁移扫描
  改 fail-closed，锁文件丢失探测补齐动态资源，ZIP 密码无密钥拒写明文，显式 `alt-*`
  跨槽资源路径被拒绝，GWA1 读取上限按明文计算；批量 PDF 与送批材料改直接内存排版，
  不再写系统临时正文 PDF。验证数字、剩余风险与 Pi/打印/双槽备份/GWA2 的改造顺序见
  `docs/encrypted-resource-lifecycle.md` 末节。
  **2026-11-06 第六轮（E/D 与本机配置日志，已实现、待真机验收）**：GWA2 以 AAD 绑定
  槽位、用途和稳定逻辑路径；Pi Rust/Node 工作区、交付、接口缓存与 Durable JSONL 全程
  密文，host 0.4.0 / 协议 4，旧 SQLite 不续跑、已有交付回执可重放；双槽备份拒绝带未
  checkpoint WAL 的非当前槽，恢复先隔离暂存校验再换入；个人词表/历史、自定义技能、
  使用统计和诊断日志纳入保护，详细日志只在解锁后应用内查看；加密工作区禁止直接打印
  （该口径已在第七轮改为确认风险后放行）。`cargo test --locked --all-targets` 为 2976
  通过 / 0 失败 / 90 忽略，Node host 111 通过 / 1 跳过。方案、验证数字与真机边界见
  `docs/encrypted-resource-lifecycle.md` 末节和 `docs/pi-runtime.md`。
  **2026-11-06 第七轮（打印策略反转 + 加密链稳定性复核，已实现、待真机验收）**：
  用户拍板加密工作区**允许打印但每次确认风险**（`PRINT_WARNING` 说明队列/纸件/
  打印机缓存不受保护；打印前导出用合并文案；取消不打印），CUPS 与 Windows 都先
  复制中性随机副本（0600）并固定中性任务名/文档名，结束删除；启动期（解锁前）与
  锁定时清 pdf-preview/print/redline-preview 明文缓存，花脸稿打印预览改槽位目录
  + 0600 原子写。修复第六轮回归：`KeyHandle` 改显式槽位（诱饵密码解锁不再拿错槽），
  停用解密补齐 apis/配置副本并断言无密文残留，配置解密失败先留底再报错、写路径拒绝
  覆盖解不开的密文（可丢弃缓存例外）。Pi 稳定性：崩溃残留 `.tmp` 清理与跳过、信息
  `info` 用 stat 算明文长度、加密 append 64 MiB / 读 256 MiB 上限、原子写后目录
  fsync、owner 锁自愈、会话 `main.jsonl` 缺失/清零明确拒绝、Running 记录自动纠正、
  终态写盘失败不吞、迁移跳过 `*.lock`/`*.tmp` 与符号链接、逻辑资源 ID 按 realpath
  稳定；备份校验独立遍历、回滚删暂存、导出/恢复上限同源、导出期间 vault.dat 代次
  变化即取消。全量 `cargo test --locked --all-targets` 为 **3002 通过 / 0 失败 /
  90 忽略**，Node host **119 通过 / 1 跳过**。仍待：依赖库 JSONL 损坏硬失败、
  长会话压缩、绝对路径+符号链接、Windows 打印真机、三平台真机验收。

  **2026-11-07 第九轮（图片/PDF 插图/图表缓存并入 SQLCipher 主库，已实现、待真机验收）**：
  用户要求磁盘上不能看出「这是什么图」（原文件名、扩展名、导入时间）。此前 B 阶段
  是「一图一文件 + GWA2」，内容加密但文件名元数据裸露。本轮新增 `src/resources.rs`
  的 `resource_objects`/`resource_aliases`（明文 SHA-256 去重的 BLOB + 逻辑引用），
  图片、PDF 插图与 Mermaid 缓存全部改为 BLOB 进 `manuscripts.db`；新引用
  `images/<uuid>`、`mermaid-cache/<sha256>`（无原名/无扩展名/无时间），格式按魔数判断；
  旧图片在工作区入口解密迁移进库、`wal_checkpoint(FULL)` 后覆写删除，别名保证旧正文、
  历史版本与旧稿件包里的 `images/<原名>` 引用继续有效；稿件包导入在
  `begin_sync_import` 事务内写资源表、不再生成 `images/` 文件（清单新增可选
  `original_name`，旧包按路径末段兜底），旧 schema 1/2/3 包照常导入；预览/导出/
  DOCX/Typst/研究报告全部改走 `images::read` 资源层；孤儿清理改为扫活稿 + 全部历史
  版本并回收无引用对象。`cargo test --locked --all-targets` 为 **3026 通过 / 0 失败 /
  92 忽略**（含新增资源库/迁移/旧包兼容用例），clippy 零警告。仍待真机验收：
  GUI 导入大图/多图、旧 `.gwx` 包实际往返、SQLCipher 库膨胀与导出耗时。

- **Linux 图形环境兜底**（麒麟 V10 上一打开就崩：X 服务器回 `GLXBadContextTag`，
  winit 对任何 X 错误都直接 panic，于是「第一次显示窗口」时崩；panic 毒化了
  winit 的互斥量，清理阶段二次 panic 变成核心转储）：**已实现，待真机验收**。
  做法三件事：①建窗口之前在**子进程**里探测图形能力——用的请求跟 eframe/glutin
  实际发的完全一致（3.3 核心配置 → OpenGL ES 2.0 兜底 → 客户端没有
  `GLX_ARB_create_context` 时走旧式 `glXCreateNewContext`），确认建不出上下文就
  改用软件渲染（Mesa 的 llvmpipe）再探一次；②装 winit 的 Xlib 错误钩子，把迟到的
  扩展类错误（GLX 等，错误码 ≥ 128）接住记进日志，不再让程序崩，核心错误仍交回
  winit；③启动失败时给中文提示与自救办法，不再只丢一个日志路径。探测结论、GLX
  错误、降级过程写进配置目录 `logs/gl-boot.log`；探测全程在子进程里，裸 FFI 出错
  也只是子进程退出。代码在 `src/gl_boot/`（Linux 专有；`x11-dl` 动态加载
  libX11/libGL，不新增系统依赖；`winit` 与 `x11-dl` 本来就在依赖树里，只声明直接
  引用）。已知边界：eframe 在 Linux 固定「先 GLX 后 EGL」（`ApiPreference::FallbackEgl`
  实为 `GlxThenEgl`），老 Mesa 的 GLX 不支持核心配置时只能靠 ES 兜底或软件渲染，
  强制 EGL 或换渲染后端不在本次范围。改这块前先读 `scripts/smoke-launch-linux.sh`
  开头那段注释（老 Mesa 的 GLX 路径早就被 CI 标注为不安全）。

## 三条不可逾越的红线（改 AI 相关代码前必读 `docs/ai-architecture.md`）

> 2026-10-03 按智能体工作台方案修订（`docs/ai-agent-workbench.md` 第二节 D1–D3）。
> 2026-11-02 按用户已确认的产品修订（`docs/pi-implementation/pi-direct-write-task.md`）
> 只在 **Pi 智能写作**这条路径上改了第 1 条：Pi 明确提交的有效 Markdown 由界面线程
> 直接写进当前正文，每次写入保留精确的修改前后文本与运行标识，卡片上可整次撤销；
> **2026-11-05 起写入前还会自动提交一个「AI 写入前」版本**（正文非空且稿件已入库时），
> 版本历史/版本对照负责找回与看差异；Pi 不再保留「AI 工作稿」副本。Pi 路径不做事实核查、
> 不产生事实疑点清单（2026-11-04 口径）。小模型分阶段流程、旧提案接受、
> 授权要素权限与只读保护全部不变。

1. **AI 可以写自己的工作稿；正文只能经规定入口改写。** 智能体在 AI 工作稿里可以
   反复写、反复改。`generated_markdown` 只能被这几种操作改写：用户键盘输入；用户在
   审阅视图里接受提案或采纳修订建议（可以逐块排除）；以及 **Pi 明确提交的正文交付**
   ——后者由界面线程按 `src/app/pi_jobs.rs` → `ai_panel::pi_write` 的固定顺序写入，
   先落撤销记录（前后文本 + 运行标识）再替换正文，不可撤销的替换不得发生。没有任何
   工具能直接写正文，也没有任何模型消息能绕过这道程序检查。
2. **公文要素分两级。** 授权类（密级与保密期限、文号（年份与数字）、份号、签发人、
   成文日期、版记印发机关与印发日期）：AI 只读不写，也不给建议，不出题。描述类
   （标题、主送、抄送、附件说明，以及发文单位、落款单位、呈报领导、承办单位、
   联系人、联系电话）：AI 可以给建议，值都要注明出处：单位与人员必须是标准
   词库里的词条，标题与附件说明取自材料原文，采纳要用户点。发函代字、呈批代字随发文单位
   按表单规则带出（公函代字随发文单位更新，呈批代字为空时才填），与人在下拉框里选发文单位是同一条代码路径。
   任何情况下 AI 都不得直接回写 `DraftInput`，写表单只能由界面线程在用户点采纳时
   执行（`element_fields::apply_field`）。
3. **确定性闸门既是工具也是关卡。** `ai_guard`（事实比对与溯源）、`validator`（要素
   校验）、`proofread` / `proofread_rules`（词表与规则复扫）可以由智能体在循环中调用、
   用来自查自改；落地前程序必须再强制跑一遍。**硬闸门**挡住落地（未确认的关键事实
   变化、来源不明的数字和日期、授权要素被改动）；**软闸门**的问题转成修订建议附在
   提案上。小模型逐句复核的产物仍然是闸门不过就丢弃。
   **Pi 路径不做事实核查**：交付检查（`src/pi_agent/delivery.rs`）只保留技术性与权限
   保护——空稿、路径 / 体积 / 哈希、基线与选区、公文引用、只读；命中就拒绝**自动**写入。
   事实来源不参与交付判定，`DeliveryReport.advisories` 仅为兼容历史记录保留、新交付恒空。
   被硬闸门拦下的成品不丢弃，保留在这一轮里，只能由用户显式点「取回 AI 成品」写入正文：
   取回路径不重跑闸门，但保留精确撤销记录（2026-11-02 / 11-04 确认的产品口径）；上面说的
   「照旧拦下」指自动写入路径。界面与状态机都不解析中文原因串来判断类别。

推论：确定性规则是 AI 的裁判，不是竞争者，不要为了迁就模型放宽规则。工具可以由
模型自由调用，权限和「做完了没有」由程序判定。

## 版本与发布

- 版本号只维护在两处：`Cargo.toml` 的根包 `version` 和 `Cargo.lock` 里
  `name = "gongwen-assistant"` 的 `version`。
- 本机**没有 `pwsh`**，不要调用 `scripts/bump-version.ps1`，手动编辑上述两处。
- `ci.yml` 在 push `main` 时触发：**当前只跑 Windows x64**（fmt / clippy / test），并组装
  `win-x64` 随包运行时，Pi host 测试、离线验收与
  `cargo test --locked shipped_pi_runtime_manifest -- --ignored` 用真实运行时执行；
  另有一个 RustSec 依赖漏洞扫描作业（每周一自动跑一次）。macOS arm64、Linux ARM64
  （GLIBC 2.28）与 Ubuntu 20.04 ARM64 启动冒烟（`scripts/smoke-launch-linux.sh`）三个作业
  保留在 `ci.yml` 里但 `if: false` 停用，恢复时去掉那一行即可。代价是 Linux 专有代码
  （`linux-portal-dialogs` 变体等）不再随 CI 编译，改到那部分要走本地
  `cargo build --release --locked --no-default-features --features ai,linux-portal-dialogs`。
  `release.yml` 在 push `v*` tag 时触发，产出 Windows x64 setup.exe、Linux ARM64 deb
  （Debian buster 容器构建，GLIBC 锁死 2.28，兼容 Ubuntu 20.04 / 麒麟 V10）、
  Linux x86_64 的 Arch / Omarchy pacman 包（`archlinux:base` 容器构建，`makepkg` 打成
  `.pkg.tar.zst`，供离线机器 `pacman -U`）与 macOS arm64 的 `.dmg`，各带 `.sha256`。
  macOS 的签名与公证只在配置 `GONGWEN_MACOS_SIGN_IDENTITY` / `GONGWEN_MACOS_NOTARY_PROFILE`
  时执行，CI 未配 secrets 时发布的是未签名包（Gatekeeper 会拦，需右键打开）；x64 dmg
  可同法出包、暂不发布。Linux AMD64 的 deb 不再发布。
- Release workflow 全程约 20–30 分钟。
- 完整发布流程（bump → tag → 监控 → 写中文 release notes → 验证）见 skill `release`。
  **硬性要求：release 正文必须手写，不能用 `--generate-notes` 的占位说明。**
- 打包一个**供本机安装测试的开发版**（不走发布）：跑
  `scripts/package-dev.ps1`（`powershell -ExecutionPolicy Bypass -NoProfile -File`），
  它构建 release → 组装 `dist\win-x64-full` → 用 Inno Setup 打出
  `dist\gongwen-assistant-<下一补丁号>-dev-win-x64-setup.exe`；只重打包不重编译加
  `-SkipBuild`。脚本含中文，文件头必须有 UTF-8 BOM，否则 PowerShell 5.1 按 GBK 读会解析失败。
- **增量升级包**：`scripts/package-upgrade.ps1` 按新旧两份 `SHA256SUMS.txt` 做差，只把
  变化的文件打进安装包（字体没改动就不带，65 MB → 几 MB），安装前按生成的
  `[InstallDelete]` 片段删掉已消失的文件。它与完整包共用 `scripts/gongwen-assistant.iss`
  （同一 AppId、同一卸载日志），升级模式由 `/DMyUpgradeMode=1` 打开：不整目录删除
  `{app}\runtime`、不重建快捷方式、目标目录没有已安装程序就中止。基线默认取
  `%LOCALAPPDATA%\Programs\GongwenAssistant\SHA256SUMS.txt`，给别人做包传
  `-BaselineDir`/`-BaselineManifest`。注意 Inno 的 `#include` 与 `#if` 只认字符串，
  include 路径要正斜杠。
- **本机打 pacman 包**（Arch / Omarchy）：`scripts/package-arch-local.sh` 在本机一次
  跑完「编译 → 组装便携目录 → makepkg 打 `.pkg.tar.zst` → `pacman -U` 安装 → 静态
  校验」，口径与 `release.yml` 的 `build-linux-x86_64-arch` + `verify-arch-pkg` 一致
  （`--no-default-features --features ai,linux-portal-dialogs`，依赖名由 `pacman -Qoq`
  反查）。提权优先用免密 `sudo`，没有就走 `pkexec`；本机已装同版本时自动把 `pkgrel`
  加一（`GONGWEN_PKGREL` 可指定），否则 `pacman -U` 会因版本相同而跳过。字体来自
  `gongwen-runtime` 的 `runtime.zip`，解到 `runtime/`（清单用仓库里的
  `runtime/SHA256SUMS.txt` 核对校验值，字体目录本身不入库）。

## 安全存储最新口径（2026-11-07）

本轮按用户确认继续收口「敏感明文不落盘，必须释放时明确确认」：数据库迁移残留可恢复且清理失败阻止成功；普通模式记住密码禁用、旧明文密码清理，首次跳过锁需确认风险；进程转交改为仅本机回环通信，不写业务路径收件文件。Pi 工作文件/技能快照及用户技能的名称和内容一起进加密容器，接口定义合并为中性 `apis/services.json`。花脸稿预览、直接打印和另存排版进内存；CUPS 经 stdin、Windows 从字节解析，不再生成送打缓存；复制/剪切先确认外部剪贴板风险。

全量备份**直接采用新版加密 7z，不保留旧 ZIP 恢复兼容**（用户确认此前未使用）。备份要求已启用应用锁并完成加密，独立备份密码保护内容和文件名，内部数据保留应用锁密文；恢复只向暂存目录写密文，恢复后重新解锁。稿件 `.gwx` 的旧 ZIP 导入兼容继续保留，不能混同两类格式。Pi host 当前为 0.5.2、协议 4（任务助手作业从 0.5.0 接入，Pi 三包已锁定到 1.5.1，修复并行工具调用偶发卡住）；旧 host 不支持隐名文件树，发布必须重建随包运行时。实施与验收见 `docs/security-hardening-2026-11-07.md`，升级记录见 `docs/pi-runtime.md`「Pi 1.5 升级」一节。同日复核又修了 host 侧 1.5 适配缺陷（恢复路径 Parked 判定竞态、`task_interrupted` 报告文案与 tool/compaction 级误判、候选稿刷新语义、工具槽位跟踪清理），并补了「host/协议版本四处对账」与 14 条会话视图/报告映射回归用例；本机 darwin-arm64 运行时已按 0.5.1 源码与依赖锁重建（manifest 14158 文件），其余平台发布前照旧从 `pi-host` 源码重建。

## 独立密码加密导出（2026-11-07）

用户确认加密工作区采用文件原生加密：设置页「导出格式」提供可选“加密导出文档”（默认开启），并独立设置导出密码，不复用或派生登录密码。选择加密后不弹风险提示，未设密码则阻止加密导出；选择明文后每次导出先明确确认安全风险（普通工作区文档导出也确认）。关闭开关保留密码供旧加密 PDF 预览，打印/剪贴板等单独风险确认继续保留。Word 为 Office Agile AES-256，PDF 为 AES-256 打开密码，Markdown/Typst 源码包和份号清单为内容/包内文件名加密的 7z。主文、随行单据、花脸稿、送批材料及 PDF 另存全部先在内存加密再落盘；直接打印与预览继续内存处理。普通模式和其他出口边界不变。独立导出密码单独以槽位 GWA2 密文保存，登录者仍可能读取此保存值；停用应用锁清除该文件。实现和验收见 `docs/encrypted-export-design.md`，它取代旧文档中上述成品默认明文输出的口径。

## 约定

- **安全写法**：内存安全、崩溃面、资源上限、注入与文件操作的约定见
  `docs/security-hardening.md`。要点：全 crate `#![deny(unsafe_code)]`，`unsafe`
  只允许出现在 `mod` 声明处 `#[allow(unsafe_code)]` 的 FFI 模块；每个 `unsafe` 块写 SAFETY 注释；句柄用 RAII 守卫；
  读用户交来的文件先卡体积（`src/file_limits.rs`）；压缩包要有整包上限；
  用户数据不拼进命令、SQL 与模板源码；解析失败要变成提示而不是崩窗口。
- 提交信息用中文 Conventional Commits：`feat:` / `fix:` / `test:` / `docs:` /
  `refactor:` / `style:` / `chore: release vX.Y.Z`。
- **本地 post-commit 钩子**（`scripts/git-hooks/post-commit`，首次使用跑
  `scripts/install-git-hooks.sh`）每次 commit 后自动跑
  `cargo build --release --locked`，保证 `target/release/` 始终是最新二进制；
  Linux 主机自动加 `--no-default-features --features ai,linux-portal-dialogs`。
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
