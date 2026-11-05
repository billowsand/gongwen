# Pi 智能写作实施与验收

> **当前进度入口（2026-11-05）：** [Pi 现状索引](pi-current-status.md)。下文阶段表和执行日志保留当时状态；4D 技能迁移、只读接口工具与风格学习已落地，旧编排整体退役仍待 5D/5E 门槛。旧写作页只读，但历史侧栏检查点仍可恢复执行，不能把全部旧会话称作只读。

> **2026-11-03 第五阶段：** 产品已切换为统一 Pi 写作，分阶段作为内部策略。5A–5C、新任务入口收敛、历史只读迁移已实现；Windows 自动回归、本机 Qwen 与离线运行时验证见 [第五阶段主审记录](pi-implementation/phase-5-review.md)。Qwen 效果仍有失败，较强模型对照与干净机安装尚未验收，因此保留旧兼容源码，不宣称 5D/5E 的全部退役门槛已通过。设计见 [统一写作设计](pi-unified-writing-design.md)。以下阶段 1–4 及双路径表述记录当时实施情况。

2026-11-01 开始。用户授权：由本机 Pi agent 实现，主代理编写分期提示词、验收、修正。第一阶段代码验收后，用户另行授权提交该阶段并继续第二阶段；不推送或发布。

当前产品契约见 `pi-unified-writing-design.md`：新任务统一由 Pi 执行，分阶段是内部策略；完整 Markdown 经技术与权限检查后直接写入正文并保留撤销。Pi 不做事实核查、不产生事实疑点清单。下文“小模型分阶段独立保留”“事实疑点只提示”等表述是历史契约；离线安装部署仍是硬性要求。

## 阶段与提示词

| 阶段 | 提示词 | 验收目标 | 状态 |
| --- | --- | --- | --- |
| 1. 文件交付闭环 | `pi-implementation/phase-1.md` | 本地 host 修改工作文件、显式提交成品，Rust 后台桥接并复用现有提案界面；协议/安全/交付测试 | 代码验收通过；真实 host 通过；GUI 待验 |
| 2. 技能与领域工具 | `pi-implementation/phase-2.md` | 独立模型配置、标准技能、材料/知识库/词库/检查工具；选区保护、提问与追问；无旧 flow 解释器 | Pi 已实现，主审修正与自动回归通过；GUI 入口与真实 host 已验，业务场景待验 |
| 3. 恢复与离线分发 | `pi-implementation/phase-3.md` | 任务与文件恢复、成品去重、多窗口；固定 Node 和完整运行依赖，断网安装启动 | 实现、主审回归、Windows 装箱及开发安装包生成通过；干净机安装/升级、GUI 恢复及 Linux 真机待验 |
| 4. 收缩旧框架 | `pi-implementation/phase-4a.md`、`phase-4b.md`、`phase-4c.md` | 解耦小模型/API/风格等共享服务，迁移默认入口，旧通用编排退出 | A 入口收敛完成；B 已提交 f0c00a5；C 标准技能管理已主审验收；F 旧编排退役未开始 |

后续阶段每次执行前根据真实代码补全提示词和基线，不预设尚未确定的文件/API。

## 本次基线

- 仓库：`D:\gongwen`，分支 `main`，HEAD `5e2af39ca191533929ec69839185f0f439ebd856`。
- 原有已修改文件：Cargo.toml、Cargo.lock、README.md、docs/ime-candidate-browser.md、docs/manual.md。
- 原有未跟踪：docs/pi-durable-research.md、docs/pi-writing-product-design.md、docs/release-notes/v0.10.1.md。
- 差异与调研文档快照：`tmp/pi-implementation/baseline/`（忽略的本地生成物）。以上文件不得被 worker 覆盖。
- 实现者：本机 Pi Coding Agent 1.4.0；已配置模型 `opencode-go/deepseek-v4.1-flash`，thinking high；最小连通检查返回 READY。
- 应用内拟接入的库：Pi Durable；“由 Pi Coding Agent 写代码”与“应用内使用 Pi Durable”是两件事。
- 执行期间另一项工作将原有版本号/说明改动提交为 `1627594 chore: release v0.10.1`；本任务保留该提交，不回退、不归入 Pi 实现差异。

## 验收纪律

每阶段 Pi 停止写入后，主代理审查真实文件/差异，核对红线与范围，修正已确定的实现缺陷；格式检查、针对性测试、必要的 cargo check/clippy。模型/GUI/跨平台/离线测试只按实际结果记录，不以脚本或模拟测试代替真机验收。

## 进度与复核

第一阶段已交付主实现并完成主审修正，真实 DeepSeek host 试验通过。第一阶段只增加独立入口，不删除旧引擎，不改变小模型分阶段写作，不修改发布版本。代码与验收记录见 `pi-implementation/phase-1-review.md`，Pi 原始报告见 `pi-implementation/phase-1-report.md`。

用户随后授权提交：第一阶段提交 `a9ed9eec3015c673d1804a73215deb656176cea3`（feat: 接入 Pi 文件工作区与文稿提案闭环），未推送。提交后的 release 构建由既有 post-commit 钩子执行，6 分 45 秒完成。

第二阶段由同一台本机 Pi 通过 RPC 实现，主代理保留中途纠偏入口；原始事件/错误日志与控制文件位于忽略的 `tmp/pi-implementation/phase-2.*`，不是产品运行依赖。

第二阶段主审发现并修正候选稿未实际传入、回答未进事实台账、规整后选区保护、报告持久化及追问原始依据衔接。
验收与限制见 `pi-implementation/phase-2-review.md`；Pi 原始交付报告保留在 `phase-2-report.md`，主审修正结论优先。

第二阶段提交 `338d6eb`，提交钩子 release 构建通过，未推送。第三阶段开始于 `d7c236e`，
保留其间两个其它任务提交；开始时工作区干净。仍由本机 Pi 实现，主代理验收与修正，
本阶段不自动提交或发布。Node 固定 22.22.3，官方下载校验值已核对，不宣称自动使用最新版本。

第三阶段主审结论见 `pi-implementation/phase-3-review.md`；Pi 两次原始报告分别为
`phase-3-report.md`、`phase-3-ui-report.md`。主审修正与最终验收记录优先于原始报告。

2026-11-02 根据用户截图反馈，提前实施第四阶段 A：收敛侧栏输入区，发送与快捷键均走 Pi，
不再并列旧技能/模式/检索开关。执行方式和验收见 `pi-implementation/phase-4a-review.md`。

同日根据真实 `not_submitted` 失败轮次定位到检查全文参数长度截断，补充文件路径检查与两次有界补交；
原因、恢复边界及验证见 `pi-implementation/not-submitted-fix.md`。
第二次真实失败进一步修正 Pi 默认 32k 窗口、压缩切点及人工继续预算，见 `pi-implementation/pi-budget-fix.md`。

用户已授权将第三阶段、第四阶段 A 和两轮实测修复合并为一次本地检查点提交；不推送、不发布。
各阶段报告中的「未提交」记录的是当时验收状态，最终提交以 Git 历史为准。

## 第四阶段 B 启动

基线 `93efcc1`，开始时工作区干净。用户授权继续原分工：Pi 实现，主代理编写提示词、主审修正和验收。
具体任务见 `pi-implementation/phase-4b.md`：共享领域/模型服务提取、Pi 思考参数独立；不提前删除旧引擎。
主代理已完成依赖盘点、基线快照和完整任务提示词，尚未修改业务代码。

尝试启动固定 `opencode-go/deepseek-v4.1-flash` 被自动审批拒绝；虽已确认仓库 PUBLIC，
审批仍要求用户明确授权将本地源码和任务内容发送到外部 opencode.ai/DeepSeek 目的地。
随后用户已明确允许该目的地与上述数据范围，用于实现 4B。按原任务继续启动 Pi，不扩大到真实稿件或凭据。
已实际启动本机 Pi，RPC 核对模型为 `opencode-go/deepseek-v4.1-flash`；实现与控制日志位于忽略的
`tmp/pi-implementation/phase-4b.*`，不是应用运行依赖。实现完成后按 `phase-4b-review.md` 主审。

4B worker 已停止交付，主代理补齐分阶段写作和 API 产品内部共享引用、移除警告屏蔽、保留知识库回退网关格式。
最终 2732 项 Rust 测试通过、85 忽略，fmt/clippy/diff 检查通过，代码验收完成；未重打安装包。
实施与剩余耦合见 `phase-4b-report.md`，最终主审结论以 `phase-4b-review.md` 为准；本阶段尚未提交。
用户随后授权提交 4B 本地检查点；历史报告中的未提交指验收时状态，不推送或发布。

## 第四阶段 C 与交付流程修订

4B 已提交为 `f0c00a5`，提交钩子 release 构建通过，未推送。4C 从该提交的干净工作区开始，
基线快照见 `tmp/pi-implementation/phase-4c-baseline/`。继续由本机 Pi 固定
`opencode-go/deepseek-v4.1-flash` 实现，主代理写 TASK、主审与修正；实际 RPC 已核对模型。
具体契约见 `pi-implementation/phase-4c.md`：Pi 标准技能目录、管理页、侧栏选择与运行快照接通，
保留旧 DSL 数据而不再用旧 UI 管理当前 Pi 技能。本阶段不自动提交或发布。

用户已明确确认后续 Pi 成品直接写入正文，保留修改前版本和撤销，事实疑点只提示，
不再要求人工确认事实/接受提案。该选择修订本文开头旧的提案交付约定；4C worker 不混改交付，
技能管理完成后按独立契约实施并同步架构说明。讨论和实施草案分别见
`pi-implementation/pi-direct-write-discussion.md` 与 `pi-implementation/pi-direct-write-task.md`。

4C 已完成主审与修正，最终 Rust 2756 passed / 84 ignored、host 49 passed / 1 skipped，fmt/clippy 通过。
具体 findings 与验证边界见 `pi-implementation/phase-4c-review.md`。本地未提交，作为下一项直接写入任务的 dirty 基线保存。

直接写入任务 TASK 和基线已准备。启动同一 Pi runner 被自动审批拒绝，理由是此前明确外发授权仅覆盖 4B，
需要用户明确允许将本任务源码、提示和自造测试数据发送至 opencode.ai / DeepSeek V4.1 Flash。
runner 未执行，未绕过拒绝，直接写入功能尚未实施；产品方案本身已经用户确认，无需再问产品选择。

用户随后明确回复“好的，我授权pi启动，以后不要再为这种事情来问我了”。已据此授权本次启动，并持续允许同一
opencode.ai / DeepSeek V4.1 Flash 目的地处理项目开发所需源码、提示和自造测试数据；不包括真实稿件、用户配置或密钥。
后续同范围启动沿用授权，不重复询问。

## Pi 直接写入与撤销

按用户明确确认的方案实施。Pi 已实际启动，固定模型核对无误；完成主体代码后连接中断，
自动重试结束并停止，未生成原报告。主代理接手实际审查和修正，执行交接补记见
`pi-implementation/pi-direct-write-report.md`，最终验收以 `pi-implementation/pi-direct-write-review.md` 为准。
最终 Rust 全量 2774 passed / 84 ignored、host 49 passed / 1 skipped，fmt 与严格 clippy 全部通过，代码验收完成。
代码仍未提交或打包，不提前做 4D/4E/4F。

用户随后授权提交 4C 与 Pi 直接写入/撤销的本地检查点，验收记录中的“未提交”指当时状态；
最终提交以 Git 历史为准。不推送、不发布，不重打安装包；提交后等待既有钩子 release 构建。

## 第四阶段 D 启动

4C 与直接写入提交为 fe20ebe，提交钩子 release 构建通过；随后独立预览修复提交 74e9133。
4D 从 74e9133 的干净工作区开始，用户授权继续 Pi 实现、主代理验收修正的分工。
具体任务见 pi-implementation/phase-4d.md：旧技能业务内容映射为八份 Pi 主技能及参考文件，
完整内置包、只读风格工具、自造案例与真实 host 业务评测脚本；不提前做 API 接入或旧引擎删除。
本机 Pi 已实际启动，模型核对为 opencode-go/deepseek-v4.1-flash；未提交或发布。
用户另授权达到 usage 上限时使用一次重置，主代理监控账户用量；重置不交给 Pi，不重复消耗。

## 第四阶段 D 本轮交付

Pi 已完成实现并停止，主代理完成实际 diff 主审、修正和真实 host 评测。八份标准技能、
完整内置参考包、只读风格工具及评测脚本已落地；具体结论见 `pi-implementation/phase-4d-review.md`。
host 49 passed / 1 skipped、eval 15 passed、风格工具10 passed，fmt/严格clippy通过；
最终Rust全量有已记录的Windows临时文件拒绝访问，不能称最后一轮全绿。

真实业务尚未全部通过：材料冲突仍有新增事项风险，研究报告仍有无据判断和不合规脚注。
资料索引/参考路径两处集成缺陷已修正并实测确认；保留原始失败，不拼接多轮最佳结果宣称全套通过。
后续优先收口上述效果问题，再进入4E；七牛DeepSeek停用使双模型验证暂未完成。
一次额度重置已确认生效，可用次数由3变2，没有再消耗。本轮未提交或打包。

2026-11-03 用户授权提交当前4D与工具队列修复的本地检查点；上述“未提交”指验收时状态。
队列修复与主审见 `pi-implementation/tool-queue-fix-review.md`，最终 host 回归62 passed / 1 skipped。
本次不推送、不打包、不安装；提交后等待仓库既有 post-commit release 构建。
