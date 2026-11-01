# Pi 智能写作实施与验收

2026-11-01 开始。用户授权：由本机 Pi agent 实现，主代理编写分期提示词、验收、修正。第一阶段代码验收后，用户另行授权提交该阶段并继续第二阶段；不推送或发布。

产品契约见 `pi-writing-product-design.md`：小模型分阶段写作保留，大模型以 Pi + 标准技能 + 文件工作区自主写作；最终单个 Markdown 经现有提案审阅进入正文。离线安装部署是硬性要求。

## 阶段与提示词

| 阶段 | 提示词 | 验收目标 | 状态 |
| --- | --- | --- | --- |
| 1. 文件交付闭环 | `pi-implementation/phase-1.md` | 本地 host 修改工作文件、显式提交成品，Rust 后台桥接并复用现有提案界面；协议/安全/交付测试 | 代码验收通过；真实 host 通过；GUI 待验 |
| 2. 技能与领域工具 | 第一阶段验收后细化 | 标准技能、材料/知识库/词库/检查工具；选区保护、提问与追问；无旧 flow 解释器 | 未开始 |
| 3. 恢复与离线分发 | 第二阶段验收后细化 | 任务与文件恢复、成品去重、多窗口；固定 Node 和完整运行依赖，断网安装启动 | 未开始 |
| 4. 收缩旧框架 | 前三阶段验收后细化 | 解耦小模型/API/风格等共享服务，迁移默认入口，旧通用编排退出 | 未开始 |

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
