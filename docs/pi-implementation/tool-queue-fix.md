# TASK：修复 Pi 正常批量工具调用触发 Rust 单槽保护而中断

- 日期：2026-11-03；仓库与 worker cwd：D:/gongwen。
- 基线 HEAD：74e913306d8583d93b0688f8d4f9060a6274c6f7，main。
- dirty：已有整套未提交4D改动，不能回退或覆盖；完整状态/差异及本次文件内容快照在 tmp/pi-implementation/tool-queue-baseline/。
- 分工：本机 Pi 实现，固定 opencode-go/deepseek-v4.1-flash / high；主代理审查实际增量与修正。用户既有执行授权继续适用。
- 文件归属：worker 只改 pi-host/src/bridge.mjs、pi-host/test/bridge.test.mjs；可新增 pi-host/test/tool-queue.test.mjs。主代理保留其它文件和本TASK、报告。
- 提交策略：不提交、不推送、不打包、不安装、不切分支；不读取真实配置、凭据、稿件或用户运行工作区，不派生代理。

## 已确认的问题

用户报错“Pi host 同时发起了多个工具调用，已停止”，源码 src/pi_agent/process.rs:758 的 pending_tool 单槽收到第二个 tool_request 即中断。
Pi Durable 支持模型同一轮批量调用；createToolBridge 当前同时 emit 每个请求，Rust 顺序执行的契约与之不匹配。
修复桥的调度即可：接纳同批多个调用，按FIFO只发出一个等待Rust回复；保留Rust的单槽、权限、来源持久化和60次预算。
不要通过提示词禁止模型批量调用，不改 SDK，不新增依赖，不并行跑Rust工具，不削弱正文/表单/交付检查。

## 行为契约

1. call 和 ask 共用一条有界FIFO，最多一个已发出且等待答复的桥请求。只有收到当前请求匹配的回复才发下一个。
2. 所有Promise对应自己的callId；未知ID、尚未发出的排队ID、重复或迟到回复不能错误完成请求或推进队列。
3. 某工具返回失败或emit抛错不能卡死后续；明确拒绝该Promise，队列仍可推进。处理同步emit/回复时不重入破坏状态。
4. 取消时拒绝正在执行和所有排队Promise，清空队列；取消后不再emit或接纳新的工具/问题。晚到回复忽略，无未处理Promise拒绝。
5. 工具60次、问题每批3题/最多2批限制不变，排队也计入预算，不能无限积累。
6. ask只有真正发出时暂停计时，完成/失败/取消时恰好恢复一次；排队问答不能把普通工具执行时间当成等待用户。
7. 首个请求仍立即发出，公开 bridge.call/ask/handle/abortAll/pending 接口兼容既有调用。

## 验证

- 新增可确定复现的多请求测试：至少两个并发领域调用，第二个须等首个结果；验证每个Promise的对应结果。
- 覆盖错误后推进、工具/提问混合排队与计时、排队ID伪回复、取消活动和排队/取消后新请求、预算上限。
- node --test（pi-host目录）通过，环境失败如实记录；不要读真实模型配置或发起业务模型评测。
- 交付报告 docs/pi-implementation/tool-queue-fix-report.md（唯一额外可写文档），列改动、实际命令、失败/限制和未解决项后停止写入。

## 主审范围补记

Pi 已停止后，主代理允许在 pi-host/test/agent.test.mjs 补真实 Durable harness 的同轮双工具调用到提交回执的集成回归。
该文件内容快照保存为基线目录 agent.test.mjs；不改生产 Rust、SDK、依赖或模型配置。
另补真正已有积压队列时的 emit 失败/同步回复测试，区别于逐次立即完成的单请求。
