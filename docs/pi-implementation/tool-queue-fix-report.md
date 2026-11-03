# 工具队列修复报告（Pi 批量工具调用）

- 日期：2026-11-03
- 仓库 / cwd：D:/gongwen
- 基线 HEAD：74e913306d8583d93b0688f8d4f9060a6274c6f7（main）
- 分工：本机 Pi 实现；未提交、未推送、未打包、未安装、未切分支。

## 改动文件（仅 TASK 允许范围）

| 文件 | 说明 |
| --- | --- |
| `pi-host/src/bridge.mjs` | 工具桥改为单槽调度：`call` / `ask` 共用一条有界 FIFO |
| `pi-host/test/tool-queue.test.mjs`（新增） | 10 个确定性队列测试 |
| `docs/pi-implementation/tool-queue-fix-report.md` | 本报告 |

未改：`pi-host/src/prompt.mjs`、`pi-host/test/bridge.test.mjs`、`src/pi_agent/*`、SDK、依赖。
未读取真实配置、凭据、稿件或用户运行工作区；只读了 `tmp/pi-implementation/tool-queue-baseline/`
里的桥源码快照与本仓库源码，用来确认 4D 既有改动未被覆盖。

## 保留 4D 既有改动

改动前 `bridge.mjs` 就是 dirty 状态（4D 的 `style_list` / `style_read` 工具、`ask_user`
描述等）。本次只替换 `createToolBridge` 与常量区，`DOMAIN_TOOLS`、
`createDomainTools` 等 4D 内容逐字未动。

核对方式（只读比较）：

```
git diff --no-index tmp/pi-implementation/tool-queue-baseline/pi-host/src/bridge.mjs pi-host/src/bridge.mjs
git diff --no-index --stat tmp/pi-implementation/tool-queue-baseline/pi-host/test/bridge.test.mjs pi-host/test/bridge.test.mjs
git diff --no-index --stat tmp/pi-implementation/tool-queue-baseline/pi-host/src/prompt.mjs pi-host/src/prompt.mjs
```

结果：`bridge.mjs` 的增量只有本次队列代码；`bridge.test.mjs`、`prompt.mjs` 与基线一致
（后者快照里没有，`git status` 显示仍是改动前的 dirty 内容，本次没碰）。

## 实现要点（`pi-host/src/bridge.mjs`）

1. **单槽 FIFO**：新增 `queue`（未发出）、`active`（已发出、等回复）；`pump()` 在
   `active === null` 且队列非空时发队首。收到匹配回复后 `handle()` 结算并 `pump()` 下一个。
   首批发出的请求仍在 `call` / `ask` 调用栈内同步 `emit`，接口
   `{ call, ask, handle, abortAll, pending }` 不变。
2. **callId 归属**：`pending`（Map）继续持有所有未结束请求（在途 + 排队），`pending()`
   返回其规模。`handle()` 只认 `active.callId`：未知 ID、排队中 ID、重复 / 迟到回复一律
   返回 `false`，不结算、不推进队列。
3. **失败不卡死**：`emit` 抛错或 Rust 回 `ok:false` 只 `settle` 这一条（`settle` 用
   `settled` 标志幂等），`pump` 的 `while` 继续发下一个。`pause` / `resume` 自身抛错被
   捕获，不影响调度。
4. **防重入**：`pumping` 守卫；`emit` 里同步回调 `handle()` 时，`settle` 已清空
   `active`，外层 `while` 继续而不是递归调度。`settle` 先把 waiter 标记为结束、从
   `pending` 删除、再 resolve/reject，重复结算无副作用。
5. **取消**：`abortAll` 置 `closed = true`、清空 `queue`、拒绝在途与排队的全部 Promise。
   之后 `call` / `ask` 立即以 `BridgeError("本轮已取消…")` 拒绝，不再 `emit`；晚到回复
   因 `active === null` 被丢弃；重复调用幂等，不会二次 `resume`。
6. **预算**：`calls` 仍在 `call()` 入口自增（排队也计数），超过 60 直接拒绝；
   `batches` 仍在 `ask()` 入口自增，每批最多 3 题、最多 2 批。另加防御性
   `MAX_PENDING_REQUESTS = MAX_TOOL_CALLS + MAX_QUESTION_BATCHES`，把 `pending` 规模封死
   （正常路径被预算先挡住，到不了这个上限）。
7. **计时**：`ask` 只有真正被 `pump` 发出时才 `pause()` 并在 waiter 上记 `paused`；
   结算（成功 / 失败 / 取消 / emit 抛错）时恰好 `resume()` 一次。排队中的提问不暂停，
   因此普通工具执行时间不会被算成等用户。

## 测试（`pi-host/test/tool-queue.test.mjs`）

| 测试 | 覆盖契约 |
| --- | --- |
| 同批两个请求按 FIFO 逐个发出… | 1、2：第二个等首个结果；排队 ID 伪回复不推进；重复 / 迟到回复 `false` |
| 工具失败只拒绝那一个 Promise，队列继续推进 | 3 |
| emit 抛错只拒绝当前请求，后续排队请求不被卡死 | 3（含排队中的 ask 也被明确拒绝） |
| emit 里同步回来的回复不破坏调度，批量请求全部完成 | 3、7：三条请求同步完成且结果一一对应 |
| 提问排在工具后面…恰好恢复一次 | 1、6：排队提问不 pause，发出时 pause，回答后 resume 一次，后续工具不 pause |
| 取消拒绝在途与排队的请求…晚到回复被忽略 | 2、4：三个 Promise 都被拒绝且都有拒绝处理器；取消后不再 emit / 接纳 |
| 取消正在等待回答的提问时恰好恢复一次计时 | 4、6 |
| 排队也计入工具 60 次预算… | 5：60 条全部排队只发 1 条，第 61 条立即拒绝，随后逐个推进 |
| 提问批次上限对排队的批次同样生效 | 5 |
| 首批发出的请求仍是同步立即 emit，公开接口保持兼容 | 7 |

## 实际执行的命令与结果

```
cd D:\gongwen\pi-host
node --test test\bridge.test.mjs test\tool-queue.test.mjs
# tests 20 / pass 20 / fail 0

node --test test\agent.test.mjs test\protocol.test.mjs test\paths.test.mjs
# tests 31 / pass 30 / fail 0 / skipped 1（既有 e2e 跳过）

node --test
# tests 60 / pass 58 / fail 1 / skipped 1
# fail: recovery.test.mjs「an uncaught error after completed cannot be overwritten by the success exit timer」
```

## 失败与限制（如实记录）

1. **`node --test` 唯一失败与本改动无关，属环境路径问题**：
   `recovery.test.mjs:43` 的用例把临时目录（`C:\Users\...\Temp\...`）里的引导脚本用
   `path.relative(hostDir, entry)` 转成相对路径；仓库在 `D:`，临时目录在 `C:`，Windows 上
   `path.relative` 会返回带盘符的绝对串，随后 `path.join(hostDir, script)` 拼成
   `D:\gongwen\pi-host\C:\Users\...\late-error.mjs`，`node` 启动即 `MODULE_NOT_FOUND`。
   失败发生在 spawn 阶段、host 与桥都没运行；`recovery.test.mjs`、`src/main.mjs`、
   `src/delivery.mjs` 本次均未改动（`git status` 可核对），基线同样会失败。
2. **未做真机模型评测**：按要求没有读模型配置、没有发起业务模型评测。真实
   DeepSeek / MiniMax 在同一轮里批量调工具、以及 Durable SDK 是否对并行 `execute` 设
   单工具超时，未在本机验证。
3. **Rust 侧未改**：单槽、权限、来源持久化、60 次预算保持原样。顺序安全性依据
   `src/pi_agent/process.rs` 的读取：`pending_tool = None` 在把 `tool_result` 交给 host
   之前执行，因此桥收到回复后立刻发下一条不会撞上单槽保护；这一点只有源码阅读，未做
   联机往返验证。
4. **取消后不可复用**：`abortAll` 置 `closed` 是终态（与 `agent.mjs` 每轮新建桥、
   取消即结束本轮一致）。若将来出现「取消后继续用同一桥」的需求，需要另开一次性语义。
5. **`MAX_PENDING_REQUESTS` 是防御性上限**：正常由 60 / 2 的预算先挡住，测试只验证了
   预算路径，没有单独构造该上限。

## 未解决项

- 真机（内网模型 + 真实工作区）跑一轮含并行工具调用的 Pi 任务，确认不再出现
  「Pi host 同时发起了多个工具调用，已停止」。
- Durable SDK 并行 `execute` 时的超时 / 取消语义与本队列的交互（若 SDK 对每个工具单独
  计时，串行执行可能让靠后的调用超时），需要真机观察后决定是否另开 TASK。
