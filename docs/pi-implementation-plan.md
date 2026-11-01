# Pi 智能写作实施与验收

2026-11-01 开始。用户授权：由本机 Pi agent 实现，主代理编写分期提示词、验收、修正。第一阶段代码验收后，用户另行授权提交该阶段并继续第二阶段；不推送或发布。

产品契约见 `pi-writing-product-design.md`：小模型分阶段写作保留，大模型以 Pi + 标准技能 + 文件工作区自主写作；最终单个 Markdown 经现有提案审阅进入正文。离线安装部署是硬性要求。

## 阶段与提示词

| 阶段 | 提示词 | 验收目标 | 状态 |
| --- | --- | --- | --- |
| 1. 文件交付闭环 | `pi-implementation/phase-1.md` | 本地 host 修改工作文件、显式提交成品，Rust 后台桥接并复用现有提案界面；协议/安全/交付测试 | 代码验收通过；真实 host 通过；GUI 待验 |
| 2. 技能与领域工具 | `pi-implementation/phase-2.md` | 独立模型配置、标准技能、材料/知识库/词库/检查工具；选区保护、提问与追问；无旧 flow 解释器 | Pi 已实现，主审修正与自动回归通过；GUI 入口与真实 host 已验，业务场景待验 |
| 3. 恢复与离线分发 | `pi-implementation/phase-3.md` | 任务与文件恢复、成品去重、多窗口；固定 Node 和完整运行依赖，断网安装启动 | 实现、主审回归、Windows 装箱及开发安装包生成通过；干净机安装/升级、GUI 恢复及 Linux 真机待验 |
| 4. 收缩旧框架 | `pi-implementation/phase-4a.md` | 解耦小模型/API/风格等共享服务，迁移默认入口，旧通用编排退出 | A：侧栏新请求统一 Pi，移除旧输入区控件；后端全面裁剪未开始 |

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
