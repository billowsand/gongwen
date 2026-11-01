# 第一阶段交付报告：Pi 文件工作区到现有提案界面

日期：2026-11-01
实现者：本机 Pi Coding Agent 1.4.0
实际模型：`opencode-go/deepseek-v4.1-flash`（thinking high）
仓库：`D:\gongwen`，分支 `main`，基线 HEAD `1627594`（任务文件写的 `5e2af39` 之上已有一个
`chore: release v0.10.1` 提交；未切分支、未提交、未推送）。

## 一、结论

阶段一闭环已实现：应用侧栏新增独立的「Pi 试用」入口，用当前输入要求与正文快照启动
随包 Node host；host 用 **Pi Durable 1.4.0** 在专属工作区里读写多个 Markdown，最终只能
通过 `submit_document` 交出一份固定版 Markdown；Rust 后台线程校验协议、哈希、路径与
来源，再复用 `reviewed_draft`、`install_ai_proposal` 与现有审阅/接受入口。

未删除旧引擎，未迁移 API 工作台，未实现知识库/词库/内网接口工具，未做会话恢复与安装包，
未修改版本号，未提交。

## 二、实际改动

### 新增

| 路径 | 内容 |
| --- | --- |
| `pi-host/` | 独立 ESM Node host：`src/main.mjs`、`protocol.mjs`、`paths.mjs`、`prompt.mjs`、`agent.mjs`；`package.json`、`package-lock.json`（含全部传递依赖）、`.gitignore`、中文 `README.md` |
| `pi-host/test/` | 22 个 `node --test` 用例：协议/上限、路径越界与 junction、损坏链接、显式提交快照、未提交失败、faux provider 真实 Durable 闭环、本机 OpenAI 兼容 HTTP 桩走生产入口 |
| `src/pi_agent.rs` | 模块入口与导出 |
| `src/pi_agent/protocol.rs` | JSONL 协议结构、版本、行/条数/成品上限 |
| `src/pi_agent/runtime.rs` | 生产 `runtime/pi` 与开发 `GONGWEN_PI_RUNTIME_DIR` / `GONGWEN_PI_NODE` 定位、Node 版本检查、Windows 隐藏窗口 |
| `src/pi_agent/workspace.rs` | 每次运行一个持久工作区：`input/`、`work/`、`skills/`、`resources/`，SQLite 放在 `workspace/` 之外 |
| `src/pi_agent/process.rs` | 后台启动、stdin 写 init、stdout/stderr 分流、取消/超时/EOF 处理、子进程回收、交付校验、Node 夹具测试 |
| `src/pi_agent/delivery.rs` | 哈希/字节/路径校验、事实来源检查、公文引用保全、未改动稿拒绝 |
| `src/ai_panel/pi_job.rs` | 侧栏「Pi 试用」发起逻辑、模型设置映射、选区/`@`/材料明确拒绝 |
| `scripts/pi-dev.ps1` | PowerShell 5.1 可用的开发运行时设置（UTF-8 BOM），只建 `tmp/pi-runtime/host` junction、设置当前窗口环境变量，不下载依赖、不写全局配置 |
| `docs/pi-implementation/phase-1-report.md` | 本报告 |

### 修改

- `src/main.rs`：只登记 `mod pi_agent;`。
- `src/ai_panel.rs`：登记 `pi_job`，新增 `pi_progress`。
- `src/ai_panel/composer_ui.rs`：底栏新增独立「Pi 试用」按钮与处理；默认发送不变。
- `src/app/jobs.rs`：新增 `PiProgress` / `PiTool` / `PiCandidate` / `PiDone` 回投；Pi 成功才走
  `reviewed_draft` + 来源闸门 + `install_ai_proposal`；失败只留失败卡片。新增迟到结果与
  来源失败两个集成测试。

未修改 `Cargo.toml`、`Cargo.lock`、根版本、`README.md`、现有版本文档；未碰基线里的
未跟踪文档。

## 三、关键实现

### 1. 真实 Durable 运行

- 依赖精确固定：`@earendil-works/pi-durable@1.4.0`、`@earendil-works/pi-ai@1.4.0`、
  `@earendil-works/chord@1.4.0`；`package-lock.json` 覆盖传递依赖。
- 生产 provider 用 `createProvider` + `openAICompletionsApi()`，地址、密钥、模型名、
  上下文 / 输出上限、reasoning 与 thinking 档位全部来自 init；不读 `~/.pi`、不刷新模型
  目录、不 OAuth、不检查更新、不联网加载技能。
- `Harness.open` + SQLite（`node:sqlite`）+ `root.submit` + `submission.wait`；工具调用、
  流式进度与任务状态由 Durable 持久化。
- `submit_document` 是唯一的成品出口：读取 `work/` 下文件、校验非空/UTF-8/体积、计算
  SHA-256，把内容复制成快照；之后的文件修改不影响已交付版本；同轮重复提交以第一次为准。

### 2. 协议与进程

- Rust → host：`init`（协议 1、requestId/runId、工作区、模型、任务、限制）与 `cancel`。
- host → Rust：`ready`、`progress`、`tool`、`candidate`、`submitted`、`completed`、
  `cancelled`、`error`、`pong`；stdout 只走协议，stderr 只放诊断。
- 上限：输出单行 16 MiB、最多 50 000 条消息、提交 8 MiB、候选 2 MiB；Rust 再核对
  requestId/runId、UTF-8、路径、字节数与哈希。
- EOF、非零退出、取消、超时、协议错、未提交、`completed(submitted: false)` 全部返回错误；
  不会把最后聊天或半截文件当成品。取消先发 `cancel`，2 秒内不退出再 kill；`ChildGuard`
  在任何返回路径 kill + wait。
- 错误与 stderr 在 Rust 侧再次抹掉 API key；密钥不进入 argv、事件或日志。

### 3. 工作区与文件环境

```text
配置目录/pi/workspaces/doc-<key>/job-<seq>-<时间>/
  workspace/
    input/current.md      只读正文快照
    input/request.md      只读本轮要求
    skills/gongwen-markdown/  随包技能（只读）
    work/                 模型可写
    resources/            预留只读
  pi/session.sqlite       模型不可读写的 Durable 数据库
```

`WorkspaceEnv` 包住 `NodeExecutionEnv`，每次文件调用先 `realpath` 再检查边界：绝对路径、
`..`、指向区外的符号链接 / Windows junction 一律拒绝；读允许 `input/`、`work/`、
`skills/`、`resources/`，写只允许 `work/`；单文件 8 MiB；写入用同目录临时文件 + fsync +
rename 原子替换；不注册 `bash`，`exec()` 直接 `shell_unavailable`。

### 4. UI 与交付闭环

- 输入框底栏新增「Pi 试用」，与默认发送并列；运行中禁用。
- 有编辑器选区、已附选区、`@` 引用或粘贴材料时明确拒绝，不静默丢弃。
- 进度写进侧栏卡片；`candidate` 只刷新「AI 工作稿」预览，不产生可接受提案。
- `PiDone` 成功后依次执行：
  1. `verify_submission`（路径、UTF-8、字节、SHA-256、8 MiB 上限）；
  2. `check_delivery`（当前正文 + 用户要求为事实来源；新单位/人员/日期/数字/文件依据拒绝；
     公文引用不得删改；工作区路径不得泄漏；与当前正文完全相同不算交付）；
  3. `reviewed_draft`（清理、文种规整、validator、版式估算）；
  4. `proofread` / `proofread_rules` 复扫，问题作为软提示随提案展示；
  5. `install_ai_proposal` 装成提案，用户接受仍走 `accept_ai_proposal`。
- 未提交、超时、取消、崩溃、来源检查不过、哈希不符都不会出现成功提案；`PiDone` 绑定
  `DocKey + job_seq`，停止或新任务后的迟到结果直接丢弃（有集成测试）。

## 四、命令与结果

在 `D:\gongwen` 实际执行：

| 命令 | 结果 |
| --- | --- |
| `cd pi-host; node --test` | **21 passed / 0 failed / 1 skipped**，约 0.7 s；跳过的是本机无权限创建文件符号链接的用例（目录 junction 越界用例已通过），含本机 OpenAI 兼容 HTTP 桩的生产入口测试 |
| `cargo fmt --all -- --check` | 通过 |
| `cargo check --locked --all-targets` | 通过（无警告） |
| `cargo test --locked pi_agent` | **13 passed / 0 failed**（含 Node 夹具的提交/未提交/错误/错 ID/取消/EOF） |
| `cargo test --locked pi_job_tests` | **2 passed / 0 failed**（迟到结果保护、来源失败不装提案） |
| `cargo test --locked --all-targets` | **2655 passed / 0 failed / 83 ignored**（本机未复现文档里提到的约 5 个沙箱失败） |
| `cargo clippy --locked --all-targets -- -D warnings` | 通过 |

另做本机手工 smoke：

- `node src/main.mjs` 接本机回环 OpenAI 桩（真实 `openai-completions` + Durable + 文件工具）：
  依次 `ready` → `progress` → `tool` → `candidate` → `submitted` → `completed`，退出码 0，
  成品内容正确，协议与 stderr 都没有出现密钥。
- 同入口接一个不可达的本机端口：`ready` → 重试进度 → `error`，退出码 1，没有假装完成；
  输出不含密钥。
- `scripts/pi-dev.ps1` 实际 dot-source：创建 `tmp/pi-runtime/host` junction，正确输出
  `GONGWEN_PI_RUNTIME_DIR` / `GONGWEN_PI_NODE`。

## 五、测试性质说明

| 测试 | 性质 |
| --- | --- |
| `pi-host/test/agent.test.mjs` | **fake provider 自动测试**：faux provider 注入，但走真实 Durable harness、`WorkspaceEnv`、文件工具与 `submit_document` |
| `pi-host/test/openai-e2e.test.mjs` | **本机协议/生产路径自动测试**：本机 HTTP 桩模拟 OpenAI 兼容端点，`src/main.mjs` 走真实 `openai-completions`，不访问公网 |
| `src/pi_agent/process.rs` 测试 | **本机 Node 夹具自动测试**：真实子进程与 JSONL，不调用模型 |
| 真实模型联机 | **未执行**（没有在 GUI 里配置并运行真实内网/公网模型） |
| GUI 真机 | **未执行**（没有启动 `cargo run` 目视侧栏/按钮/提案） |
| 三平台安装包 / 离线安装 | **未执行**（阶段三） |

## 六、风险与已知边界

1. **事实来源闸门偏严**：本阶段只把当前正文与用户要求当来源。模型如果写出材料里没有的
   日期、数字、单位或文件依据，会直接失败而不是转成软提示；空白起草时尤其明显。没有
   为了“让它过”放宽规则。
2. **只用 `toggle` 的模型可能无法交付**：`submit_document` 是工具调用，模型必须支持
   function calling。不支持工具的小模型会以“没有提交成品”失败；这是有意的，不退回聊天
   抽取正文。
3. **Durable 恢复尚未接入产品**：每次运行是新工作区、新 SQLite；中断后当前 UI 只显示失败，
   不会跨重启续跑。阶段三处理恢复、提交去重持久化与多窗口宿主。
4. **工作区只增不减**：持久保存便于排查，但还没有清理策略；后续要按稿件/会话管理生命周期。
5. **`check_delivery` 在 UI 线程执行**：`reviewed_draft` 与复扫是本地纯计算，8 MiB 上限内
   通常可接受，但超大稿可能造成短暂卡顿；后续可移到后台。
6. **模型参数映射是保守近似**：`context_window == 0` 时固定按 32768；用户设置的
   `max_tokens` 接近或超过窗口时会抬高窗口而不是截断输出；`Max` 思考档位映射为 `high`
   （Pi 的 `max` 需要模型显式声明）。这些都在 `src/ai_panel/pi_job.rs` 有注释。
7. **候选预览每次整文件发送**（上限 2 MiB），大文件会多占带宽；阶段一不做增量 diff。
8. **未验证多窗口**：`doc.busy` 只保护单个应用窗口内的同一稿件；跨窗口/跨进程并发写入
   尚未定义。
9. **Windows 依赖 `CreateProcess` + 隐藏窗口**；Linux/macOS 的 Node 动态库与离线安装未验证
   （阶段三）。
10. **`workspace.state` 必须是绝对路径**；运行时会自行建父目录（Rust 已建 `pi/`）。若配置
    目录不可写，会在启动前返回中文错误。

## 七、下一阶段接口建议

- **技能与材料**：工作区已给 `skills/` 只读目录；阶段二把标准技能、材料快照、知识库/词库/
  检查工具接进同一套 `WorkspaceEnv` 与工具注册，不复制旧 flow 解释器。
- **选区保护**：提交仍是完整 Markdown；Rust 以本轮快照对比选区外文本，越界即拒绝，复用
  `check_delivery` 的位置。
- **提问与决策**：`ask_user` 应作为 host → Rust 的事件与下一轮输入，不复用旧引擎挂起。
- **恢复**：复用 `pi/session.sqlite` 与工作区持久化；先定义“稿件 UUID + 会话 UUID”与提交
  回执表，再做跨重启续跑。
- **打包**：按 `runtime/pi/` 布局把 Node、`host/`、`node_modules/` 与 `skills/` 装进现有
  runtime 清单；`scripts/pi-dev.ps1` 只留在开发侧。

## 八、停止写入

实现、格式化、检查、测试与文档已完成；未提交、未推送、未创建 PR、未改版本。等待主代理
审查与修正。
