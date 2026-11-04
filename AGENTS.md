# AGENTS.md — 公文助手（gongwen）

基于 Rust + egui 的离线中文公文写作桌面应用。模型只起草 Markdown 正文，行文要素、
版式规则与导出结果由本地程序控制。

## Pi 写作最新产品口径（2026-11-04）

用户已确认首版：面向 256K 及以上窗口，按实际容量优先整篇材料，超限按大章节切分；每轮提供当前日期/星期/时区及标准组织背景，词库按职能、人员简介及所属单位查询。Pi 新任务不做事实核查、不产生事实疑点清单，全面审校与政策核查技能退出新任务；现有文字复核、格式与引用保护、授权表单权限、基线和精确撤销保留。此口径取代下文 Pi 事实疑点“只提示”的旧表述，不改变旧提案历史的权限。详见 `docs/pi-implementation/writing-context-v1.md`。

2026-11-04 后续：设置页风格学习也已统一到 Pi 的 `gongwen-style-learn`，支持库内/上传样稿，`submit_style` 交独立JSON草稿，用户编辑保存；不走辅助模型、不写正文。普通写作与风格学习按任务类型隔离技能和交付权限，见 `docs/pi-implementation/style-learning-v1.md`。Pi 内置技能共 7 个（6 个写作 + 1 个风格学习），全面审校 `gongwen-review` 与政策核查 `gongwen-policy-check` 已退役、只保留历史文件。交付通道按任务类型分三类：正文 `document`（`submit_document`）、清单 `report`（`finish_report`）、独立风格文件 `style`（`submit_style`，只进设置页编辑器、不能写正文）。

## 技术栈

- Rust edition 2024（stable），GUI 用 `eframe` / `egui` 0.35，纯 CPU 渲染 PDF 用 `hayro`。
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
  `src/rag.rs`、`src/rag_client.rs`）。

## 常用命令

```bash
cargo run -- --new-window      # 开发运行（不加时，本机开着新版公文助手会把启动转交给它后退出）
cargo check --all-targets      # 快速基线
cargo fmt --all -- --check     # CI 零容忍，改完必跑
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --release --locked
```

- 开发构建给依赖开 `opt-level = 3`（见 `Cargo.toml`），首次编译依赖约需一分钟，属正常。
- `cargo test` 有**约 5 个与沙箱环境相关的失败，是预期结果**，不要为此改动测试。
- Linux 变体：Arch/Omarchy 包（随 Release 发布 x86_64 的 `.pkg.tar.zst`）用
  `cargo build --release --locked --no-default-features --features linux-portal-dialogs`。
- **文件对话框不能占住主线程**。Wayland 合成器（Hyprland 等）每 1.5 秒给客户端发一次
  存活探测，主线程一停就弹「应用无响应」系统对话框。系统文件框一律走
  `src/app/file_dialog.rs` 的 `request_file` / `request_files`：portal 后端（Linux/Wayland
  变体）在后台线程开窗、主线程每帧问一次结果；GTK3 后端是本进程自己的窗口，只能留在
  主线程（即 `linux-gtk-dialogs` 变体仍是阻塞式）。起草页、输入法词表页要开文件框时，
  登记 `DraftAction` / `PageAction` 交给应用层，不要在页面里直接调 `rfd`。

## 代码组织

单文件超过约 2000 行就该按功能域拆成模块文件夹。已拆过的：`src/app/`、`src/draft_page/`、
`src/preview/`、`src/lexicon/`、`src/export/{docx,typst}/`、`src/ime/`。拆分流程见 skill
`split-rust-module`（纯代码移动，每拆一个文件单独提交一次，零警告验证）。

`skills/gongwen-markdown/` 是给外部 AI 工具用的技能包（Agent Skill），讲的是起草页
Markdown 语法与各文种正文规则，由 `src/skill_pack.rs` 编进二进制、设置页「上手指引」导出，
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

使用帮助（菜单「使用帮助」/ `F1`）是 `assets/help/` 下的**一篇研究报告**：按文件名升序拼接
（`00-front.md` 是题名、目录与摘要，其后每章一个文件，章标题挂 `{#chap:…}`），应用内直接用
研究报告预览排、「生成 PDF」走研究报告 Typst 链，帮助不再有自己的渲染器。示意图一律写
Mermaid，只有界面截图是位图（`assets/help/images/help-*.png`）。改帮助就是改研究报告：写法
见技能包 `research-report.md`，新增文件登记进 `src/help/content.rs` 的 `FILES` / `IMAGES`；
`cargo test --locked help::` 核对清单、交叉引用与图，`help_pdf_sample`（ignored）整本出 PDF 目视检查。

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
  （起草 / 复核 / 知识库）选模型（`ModelRef` 引用 + `AppConfig` 中央解析器），
  旧内联地址字段随启动自动迁移；详见 `docs/ai-agent-workbench.md` 第十三节。对比分析、数据分析段落、正文区审阅再讨论（见第十六节 F16；opencode 接入已作废）。
  方案、红线修订记录、分期、各期进度与已知坑见 `docs/ai-agent-workbench.md`（单文件，第十三节是交接）。
  **2026-11-04 审查后 Pi 修复（待真机验收）**：单轮执行上限改为 30 分钟（等用户回答不计，
  卡片显示「执行上限 30 分钟」）；工具调用常规 60 次、分阶段由宿主按阶段追加、硬上限 84 次；
  事实来源改为 `sources/items/<sha256>.md` + `sources/index-<hash>.json`（单项 2 MiB / 索引
  512 KiB / 恢复读取 8 MiB，旧格式可读），落盘失败不再中止本轮；运行目录按稿件保留最近 3 轮
  并新增自动清理与 AI 管理页占用/清理入口；生产 host 改为 `env_clear` 后最小环境白名单并透传
  代理变量；macOS `darwin-arm64` / `darwin-x64` 运行时、打包、签名公证（可选）与 CI 骨架已补齐，
  待真机与发布验证。细节与已知限制见 `docs/pi-runtime.md`。
- **公文要素填写框架**（AI 按标准词库补全表单要素：主送、抄送、呈报领导、承办单位、
  联系人、联系电话、发文单位、落款单位，分四期）：**第 1 期（字段登记表
  `src/element_fields.rs`、统一写值路径 `apply_field`、表单下拉框改调它、红线 2 文档
  修订）、第 2 期（「要素抽取」提示词与摘录核对、`element_fields::plan` 分类、
  `Target::Field` 出题含多选与人员随单位过滤、回答落黑板的 `field_suggestions`
  建议清单与已确认信息、与六要素去重）、第 3 期（侧栏要素建议卡
  `src/ai_panel/field_card.rs`：`Event::FieldSuggestions` 事件 + 答题合入两路、
  采纳 / 全部采纳 / 撤销 / 过期检测 / 词库与文种校验、随会话存盘、交付提案提醒）、
  第 4 期（技能参数 `fields` / `field_questions` 与六要素开关拆开、复函技能自带
  「要素抽取」节、两个内网同款模型联机实测、使用帮助）已完成，**四期全部实现，
  待真机验收（GUI 清单见设计文档第 3 期交接）**。
  红线 2 已按方案第二节修订（描述类扩到发文、落款、呈报、承办；授权类不出题不建议）。
  方案、分期、进度与已知坑见 `docs/element-fill-design.md`（单文件，第六节是交接）；动
  要素表单写值或 AI 要素建议前先读它。
- **数据接口工作台**（以 OpenAPI 为中心重做数据接口）：要的是三个结果——接口调通、用测试集
  （用例 + 期望）证明接口可用、大模型能正确调用（AI 用例：一句问题 → 期望的接口与参数）；不另做
  调试台界面，都放在现有「试一下」里。**第一期（中心格式、标准文件导入导出、旧 `apis.json`
  迁移）、第二期（接口用例：期望、整组跑一遍留记录、会改数据的接口确认后人工发送）、第三期
  （AI 调用：工具参数带 schema 与调用前校验、接口多时两级访问、「说一句话让 AI 调」与 AI 用例）
  已完成，待真机验收**。接口存在
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
  覆盖、色卡样张、使用帮助）未开工。代码在 `src/theme/omarchy/`，样本在 `tests/fixtures/omarchy/`。方案、出入与交接见
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

## 三条不可逾越的红线（改 AI 相关代码前必读 `docs/ai-architecture.md`）

> 2026-10-03 按智能体工作台方案修订（`docs/ai-agent-workbench.md` 第二节 D1–D3）。
> 2026-11-02 按用户已确认的产品修订（`docs/pi-implementation/pi-direct-write-task.md`）
> 只在 **Pi 智能写作**这条路径上改了第 1 条：Pi 明确提交的有效 Markdown 由界面线程
> 直接写进当前正文，每次写入保留精确的修改前后文本与运行标识，卡片上可整次撤销；
> Pi 路径不做事实核查、不产生事实疑点清单（2026-11-04 口径）。小模型分阶段流程、旧提案接受、
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
- `ci.yml` 在 push `main` 时触发：Windows x64、Linux ARM64（GLIBC 2.28）与 macOS arm64
  （`macos-14`）三平台跑 fmt / clippy / test；Debian buster 作业会组装 `linux-arm64` 运行时
  并把 `runtime/pi/bin` 加进 PATH，Pi 的进程 / 锁 / 交付 Rust 测试真正执行，Linux 与 Windows
  作业还跑 `cargo test --locked --release shipped_pi_runtime_manifest -- --ignored`；
  另在干净的 Ubuntu 20.04 ARM64 里装 deb 并用 Xvfb 真正启动一次，见 `scripts/smoke-launch-linux.sh`。
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
  （`--no-default-features --features linux-portal-dialogs`，依赖名由 `pacman -Qoq`
  反查）。提权优先用免密 `sudo`，没有就走 `pkexec`；本机已装同版本时自动把 `pkgrel`
  加一（`GONGWEN_PKGREL` 可指定），否则 `pacman -U` 会因版本相同而跳过。字体来自
  `gongwen-runtime` 的 `runtime.zip`，解到 `runtime/`（清单用仓库里的
  `runtime/SHA256SUMS.txt` 核对校验值，字体目录本身不入库）。

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
