# TASK：Pi 成品直接进入正文并可撤销

## 授权与启动条件

2026-11-02 用户已明确确认：“按此方案，直接写入并保留撤销记录（推荐）”。
这是对原先 Pi 必须先人工确认事实、再接受提案才能进入正文约定的产品修订。
4C 技能管理已由 Pi 交付，主代理完成实际差异主审与修正。本任务随后实际启动，Pi 主体实现后遇到连接错误停止，由主代理接手审查、修正与验收，结果见 pi-direct-write-review.md。
延续 Pi 实现、主代理写提示词/主审/修正的分工，不提交、推送、发布或改版本。

## 结果契约

1. Pi 显式提交有效 Markdown 后，程序在界面线程直接更新当前可编辑正文。普通成功流程不弹人工校订窗、不要求勾事实、不要求接受提案。
2. 事实来源、数字日期变化、词表与文种问题仍可检查并展示，但作为提示，不能以人工确认状态阻止正常写入。检查结果用结构化类型区分阻止项与提示项，不解析中文字符串判类别。
3. 路径/内容哈希/体积/空内容/UTF-8 边界/只读稿件/当前任务身份/正文基线/授权选区等技术与权限保护仍有效。工具权限、授权表单字段不放开。报告型产物只进侧栏。
4. 写入仅更新 Markdown 及对应校对/预览状态，不借用旧整篇接受路径偷偷更新标题或其它 DraftInput。小模型分阶段流程与旧历史提案接受保持原行为。
5. 运行期间用户改过正文，Pi 结果应保存在轮次/工作区，明确提示冲突；不得覆盖或丢弃任何一方。无需造通用合并框架，可让用户按当前正文重做或显式取回 AI 成品。
6. 正常成功后正文文本框即可继续编辑。卡片以“已写入”展示，不再显示“接受提案”；差异查看是可选操作。

## 修改前版本与撤销

1. 每次写入保留精确的 before/after 与运行标识，支持一个清晰的“撤销本次写入”操作；按钮存在于对应轮次，不能撤销另一篇稿件。
2. 一次 Ctrl+Z 应撤回整次 Pi 替换。复用 diff_editor::replace_with_undo 或现有等价机制，注意后台结果可能属于非活动标签：不能把别的标签文本塞进全局 editor_id 的 undo 栈。
3. 若用户在写入后继续手工编辑，卡片撤销不能整篇覆盖这些新编辑；可禁用并明确说明，普通编辑器 Ctrl+Z 仍按顺序工作。
4. 修改前版本不是自动创建“正式提交版”。优先复用现有持久化会话/运行快照保留精确基线，写清已入库与未入库稿件的恢复范围；避免为此新增数据库版本图分支或发布/归档行为。
5. 需要保障正文写入、消费状态、撤销记录与会话恢复协调。先确认保留基线成功再替换；失败留存结果、报错，不形成无撤销记录的替换。不要静默吞持久化失败。
6. 已写入回执不可重复自动应用，撤销后也不能重启又被同一轮覆盖；保留旧 Accepted 状态/会话的兼容性。新状态或字段 serde 默认值必须兼容旧记录。

## 当前实现线索（开始前重新核对）

- src/app/pi_jobs.rs：apply_pi_done，当前两次 check_delivery + reviewed_draft，然后 install_ai_proposal 并 finish Proposed。
- src/pi_agent/delivery.rs：当前 check_delivery 混合选区/引用保护与事实来源问题；需结构化拆分并保留工具自查能力。
- src/ai_panel.rs：resolve_proposal 会标记 PiRunStatus::Accepted，但只找 Proposed，不能直接用于自动交付。
- src/pi_agent/recovery.rs 与 src/ai_panel/session.rs：运行回执、会话、恢复与 consumed 语义。
- src/draft_page/diff_editor.rs：replace_with_undo 使用统一 editor_id，非活动稿件必须处理隔离。
- src/app/jobs.rs：take_generated 直接替换正文并更新警告，不自动提供撤销，也要避免无意打开强制抽屉。
- src/draft_page.rs：read_only 保护 Published/Archived；运行结束再次检查，不能只依赖启动检查。

## 验收

- 正常空白起草和已有正文修改：直接进入文本框，无提案/事实确认；数字来源疑点可见且不阻止写入。
- 单次编辑器撤销恢复精确原文；卡片撤销；撤销后再次运行；写入后人工修改时卡片撤销不冲掉编辑。
- 标签切换与多稿件后台完成：撤销与状态不串文稿；已发布/归档、过期任务、基线冲突、选区外修改和哈希错误不覆盖正文。
- 任务失败/取消/报告无正文替换；恢复同轮不重复应用；重启后仍可找到已入库稿件修改前基线；旧提案/会话可读。
- 整个 DraftInput 前后一致，尤其标题、授权要素不随自动写入被修改。
- 同步 docs/ai-architecture.md、Pi 产品设计/运行文档和有冲突的 AGENTS.md 条款。仅修订 Pi 路径，不将小模型闸门一并放开。
- fmt/clippy、相关单元和状态持久化测试，主代理检查真实 diff 后运行适当全量验证；GUI 与跨平台只按实际执行记录。

## 基线、执行与文件归属

- cwd `D:/gongwen`，分支 main，HEAD f0c00a5，加上已经验收的 **未提交 4C 改动**。不得清理或回退工作区，也不得重做 4C。
- 精确已存在 dirty/staged/unstaged/untracked 快照在 `tmp/pi-implementation/pi-direct-write-baseline/`；这是本任务基线，而不是原始 HEAD。
- 固定本机 Pi `opencode-go/deepseek-v4.1-flash` / thinking high。2026-11-02 用户在被告知本任务将源码、提示和自造测试数据发送到 opencode.ai 的 DeepSeek V4.1 Flash 后明确回复：“好的，我授权pi启动，以后不要再为这种事情来问我了”。该授权覆盖本任务，并持续适用于同一目的地、同类项目开发数据和既定分工，后续不重复询问。不读真实用户配置、密钥、稿件、旧运行工作区或 .env。仅 Pi 本身处理已有认证，不打印。
- 必读 AGENTS.md、docs/ai-architecture.md、docs/security-hardening.md、docs/pi-implementation/phase-4c-review.md、docs/pi-implementation/pi-direct-write-discussion.md。本任务的已确认新产品契约优先于旧 Pi 提案/硬事实闸门约定。
- 允许修改：src/app/pi_jobs.rs、必要的 src/app/jobs.rs（Pi 分支）、src/ai_panel.rs、src/ai_panel/{ui,ui_tests,pi_job,session,session_ui,history}.rs、src/draft_page.rs、src/draft_page/{editor,diff_editor}.rs；可在上述目录新增范围明确的 Pi 交付/撤销小模块。
- 主代理核对后补充允许 src/ai_panel/workspace_ui.rs：必要的 Pi 已写入/冲突产物只读查看与可选差异展示，旧流程提案采纳保持兼容；不创建第二个强制审阅流程。
- 允许修改 src/pi_agent.rs、src/pi_agent/{delivery,recovery,process,tools}.rs，限交付检查分类、消费/恢复与所需新字段；不重构运行循环、预算、技能存储或工具权限。
- src/pi_agent/materials.rs 仅允许现有测试适配结构化 advisories，样稿不是事实来源的断言保持，不改生产材料规则。
- 允许修改 pi-host/src/prompt.mjs、必要的 pi-host/src/{delivery,agent}.mjs（仅交付契约/记录接口所必需）、对应 host 测试；assets/pi-skills/*/SKILL.md 只改过时的“人工接受”等交付文字，不扩充 4D 技能业务。
- 允许修改 AGENTS.md、docs/ai-architecture.md、docs/pi-writing-product-design.md、docs/pi-runtime.md，明确新 Pi 产品规则和适用范围。4C 已新增的技能管理/安全内容保持。
- 如果需要修改列表之外文件，先报告范围与必要性，等主代理明确补充，不先改后报。
- 主代理拥有本 TASK、讨论、实施计划和最终 pi-direct-write-review.md；worker 只写 pi-direct-write-report.md，报告真实验证与遗留限制后立即停止。不得建子代理、提交/push/tag/发布/安装/改版本。

## 实现纪律与停止点

- 只做本任务，不提前做 4D/4E/4F。不要引入通用事务框架、新依赖或新的数据库版本图；复用运行工作区的 input/current.md 与成品回执、现有会话序列化和编辑器撤销设施。
- 注意 editor_id 当前全局共用。为 Pi 交付实现必要的稿件隔离；不要悄悄扩大到整个编辑器重构。
- 开始和完成时核对 git status，但不把 4C 基线认作本轮改动。保存失败必须保留成品和可重试状态。
- 检查工具可以继续帮助模型自查事实；UI 最终交付不因事实确认而阻止正文写入。交付提示词要让模型知道这一点，不强迫它为“解锁提案”循环。
- 跑 cargo fmt --all -- --check、cargo check --locked --all-targets、cargo clippy --locked --all-targets -- -D warnings 和针对性测试。不得附加 -A 或源码 allow 掩盖未使用警告。
- worker 不跑 release、打包、真实外部业务模型评测。全量最终回归由主代理负责；报告后立即停止工具调用并交接。
