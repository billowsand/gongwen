# TASK：第一阶段——Pi 文件工作区到现有提案界面

你是用户指定的 Pi 实现 worker。请直接实现并验证，不只输出计划。完成后停止写文件，交付实际差异，由主代理审查与修正。

## 协作与基线

- 仓库与实际 cwd：`D:\gongwen`；分支 main；基线 HEAD `5e2af39ca191533929ec69839185f0f439ebd856`。
- 使用本次已配置 Pi 模型，最终报告实际模型。不要创建子代理。
- 基线已有改动/未跟踪文件见 `docs/pi-implementation-plan.md`；快照在 `tmp/pi-implementation/baseline/`。不能覆盖这些既有内容，尤其 Cargo.toml/Cargo.lock/README.md/版本文档。
- 不切分支、不 commit、不 push、不创建 PR、不升级版本、不启动发布。不读取或输出 auth.json、模型密钥测试文件及其它秘密。应用收到的 API key 不得写日志/命令行/工作目录。
- 项目约定先读 AGENTS.md、docs/ai-architecture.md 的三条红线、docs/security-hardening.md；设计读 docs/pi-writing-product-design.md。用户已授权重新选型，不因旧“不引入框架”决定停工。

## 目标与非目标

完成一个真正独立的 Pi Durable 运行闭环：应用收集本轮要求与正文快照 → JS host 在专属工作目录读写多个 Markdown → 显式 submit_document → Rust 交付校验 → 现有工作稿/差异/人工接受。

本阶段不删除旧引擎、不迁移 API 工作台、不实现全部知识库工具、不承诺旧会话检查点转换、不完成三个平台的安装包。后续步骤已有规划。禁止实现 mock Pi 并把它称作真实 Durable 接入。

## 允许改动范围与所有权

你拥有以下路径；主代理在你结束前不并发编辑：

- 新增 `pi-host/`：package.json、锁文件、JS 源码、测试、该子目录 .gitignore（忽略 node_modules 和生成物）、中文开发说明。优先直接 ESM JavaScript，减少构建依赖。
- 新增 `src/pi_agent.rs`、`src/pi_agent/`：协议、运行时定位、后台进程管理、工作区/提交验证、交付检查与测试；模块按功能拆分。
- `src/main.rs`：仅登记新模块。
- `src/ai_panel.rs`、`src/ai_panel/composer_ui.rs`、`src/ai_panel/ui.rs`：独立 Pi 试用入口与必要状态；必要时新增 ai_panel/pi_job.rs 并登记。
- `src/app/jobs.rs`、`src/draft_page.rs`：后台消息和必要的任务状态桥接；禁止绕过现有人工接受。
- `src/app/ai_proposal.rs`：仅确有必要的共享接入，不降低原有保护。
- `src/ai_panel/ui_tests.rs`：必要集成测试。
- 新增 `scripts/pi-dev.ps1`（PowerShell 5.1 可用，中文脚本带 UTF-8 BOM）：本地开发设置显式 runtime 路径，不下载依赖、不写全局配置。
- 新增 `docs/pi-implementation/phase-1-report.md`：实现、命令结果、风险、后续接口。

若需要其它文件或改变上述边界，先报告原因并停止相关改动，不能自行扩范围。你可以在 tmp/ 下保存日志、试验与依赖缓存。不要运行 cargo fmt 时顺便修改无关文件。

## 实现契约

### A. JS host 与实际 Pi Durable

1. 依赖精确固定 `@earendil-works/pi-durable`、pi-ai、chord 1.4.0，生成包含全部传递依赖的 lockfile；禁止调用目标机的 Pi CLI 或依赖用户个人 Pi 配置。允许在本机联网安装构建依赖；npm cache 指到仓库 tmp 子目录，避免改全局配置。通过 node_modules 包内容检查实际 API，不靠臆测。
2. host 使用 Node >=22.19；应用运行不得 npm install、刷新公网模型目录、OAuth 登录、检查更新或从公网加载技能。配置只来自 Rust 发来的当前应用模型设置，模型参数由输入显式确定。
3. JSONL 双向协议：stdout 仅协议，stderr 不含密钥；版本、request/run ID、事件类型明确；长度与数量有上限；错误、EOF、取消和退出码不能假装完成。不得让同步管道阻塞 UI。
4. 用户配置/密钥通过 stdin 输入。先支持应用现有 OpenAI 兼容模型，无需新 provider UI；兼容实际 base URL、model、context/output limits、reasoning 可选设置，不读取开发者 Pi auth。
5. host 安装文件 read/write/edit 与 submit_document 工具，模型可拆分合并文件；不要复制原来的 flow 或算子框架。最小写作指令必须带公文语法/三条红线，不能只用编程助手默认 prompt。
6. 文件环境只允许读 input、技能和工作区，写 work。cwd 不是沙箱：校验绝对路径、越界、符号链接/junction；先不注册 bash。文件写入用原子替换，约束单文件和总量，工作区持久保存，不写安装目录。Pi 内部数据库置于模型不可读写区域。
7. submit_document 明确选定一个完整 UTF-8 Markdown，不猜文件名，不从聊天抽取正文。提交读取固定内容/哈希并发送成品；同次调用可去重，提交之后的文件变化不改变已经交付的提案。未提交就结束是未交付，不可把最后聊天当正文。不要把不完整文件当完成。
8. 至少提供不调用外网模型的真实协议/工作区/提交测试；如用 fake provider，清楚标记测试用途，生产路径必须实际走 Durable。

### B. Rust 后台桥接与 UI

1. 显式开发/生产 runtime 发现：生产优先 exe 邻接 runtime/pi 中携带的 Node+host；开发可用文档化的 GONGWEN_PI_RUNTIME_DIR / GONGWEN_PI_NODE 绝对路径。缺依赖返回中文可操作提示，绝不运行 npm 或偷偷调用 PATH 中的 pi。
2. 在后台线程启动 node，Windows 隐藏窗口；使用结构化参数和 stdin，不能拼 shell 或把 key 放 argv。取消与异常保证进程回收，避免卡住 UI/孤儿进程，必要时复用项目已有进程管理模式。
3. composer 增加明确的“智能写作（Pi 试用）”独立操作，使用当前输入要求与正文。默认旧行为不变。运行时同一稿件不能并发覆盖。用 DocKey/job_seq 绑定，旧任务/停止后的迟到结果不能覆盖新任务。
4. 侧栏显示进度；当前候选文件写完后可刷新 AI 工作稿。草稿预览与可接受提案分开。未提交、超时、取消、进程崩溃均不能出现成功提案。
5. 最终仍完整交付 Markdown，经 `reviewed_draft` + 必需的交付关卡，走 `install_ai_proposal`、`accept_ai_proposal`。不要直接写 generated_markdown/DraftInput。正文变化导致旧提案过期的现有保护保留。
6. 来源检查是硬要求：当前正文、用户要求/显式材料是本阶段事实来源，样稿不能充当事实。参考 `draft_page/writing/validation.rs`；`reviewed_draft` 只含清理/validator，不等于事实来源全检查。check/proofread/proofread_rules 复用；如无法安全支持某复杂任务明确失败而不放宽闸门。
7. 有选区时本阶段可拒绝 Pi 入口并说明尚未接入，或实现选区外不变的确定性验证；不能静默忽略选区限制。同理不支持的 @引用/材料不得静默丢弃，需携带或明确拦下。

## 验收

- host：node --test，测试必须覆盖越界/链接、显式提交及快照稳定、非提交结束/错误、协议 framing/上限等高价值行为。
- Rust：cargo fmt --all -- --check；cargo check --locked --all-targets；相关 pi_agent/交付/异步迟到保护测试；cargo clippy --locked --all-targets -- -D warnings。所有失败如实报告，不能削弱现有测试。
- 明确区分 fake provider 自动测试、真实模型联机和 GUI 真机，未执行不声称通过。允许做本机 host smoke；密钥不得出现在日志中。测试不得依赖公网。
- 已有 dirty 不变；禁止修改锁文件中的根版本。完成写入后给出实际文件、测试结果、下一阶段建议与未解决风险，然后停止写入等待 review。
