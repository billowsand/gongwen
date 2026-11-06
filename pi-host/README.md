# Pi 写作宿主机（阶段二）

公文助手随包的 Node 侧运行层：用 **Pi Durable** 在专属文件工作区里写作，最后只交出
一份固定版本的 Markdown。Rust 主程序负责收集正文与要求、启动本进程、校验交付并按 Pi
固定入口写入正文；本目录不直接改正式稿，也不实现新的流程引擎。

## 依赖与构建（只在开发 / CI 联网阶段做）

- Node.js **>= 22.19.0**（`node:sqlite` 与 Pi Durable 的最低要求）。
- 依赖版本精确固定，`package-lock.json` 包含全部传递依赖：

  ```bash
  cd pi-host
  npm ci --cache ../tmp/pi-implementation/npm-cache
  ```

  `node_modules/` 与生成物不入库（见 `.gitignore`）。目标机不运行 npm，也不访问公网
  模型目录；发布包携带 `runtime/pi/` 下的 Node、本目录源码与生产依赖。

- 直接 ESM JavaScript，没有 TypeScript 构建步骤。

## 测试

```bash
cd pi-host
node --test
```

覆盖：

- `test/protocol.test.mjs`：init 校验、协议版本、行 / 消息上限、密钥抹除。
- `test/encryption.test.mjs`：GWA2 固定向量与 AAD、GWA1 只读、路径 / 槽位防串、
  加密工作区与交付、Durable JSONL 提交及重开恢复。
- `test/paths.test.mjs`：绝对路径 / `..` / 符号链接与 Windows junction 越界、只写
  `work/`、原子提交快照。
- `test/agent.test.mjs`：**测试专用** faux provider，走真实 Durable harness、文件工具与
  `submit_document`；验证显式提交、提交后改文件不影响快照、未提交不算完成，以及
  「检索（工具桥）→ 写文件 → 提交」和 `finish_report` 只交清单。
- `test/bridge.test.mjs`：工具桥按 `callId` 往返、错 ID / 迟到回复丢弃、结构化失败、
  取消清空等待者；`ask_user` 暂停计时、批次 / 题数上限；`doc_elements` 在工具清单里；
  init 携带技能 / 选区 / 材料角色，非法材料路径被丢弃。
- `test/openai-e2e.test.mjs`：在本机起一个 OpenAI 兼容 HTTP 桩，让生产入口
  `src/main.mjs` 走真实的 `openai-completions` + Durable 路径，不访问公网。

测试不依赖外网。faux provider 只用于测试，生产入口永远构造 OpenAI 兼容 provider。

## 本地开发

不要手工设置全局环境变量。在仓库根目录：

```powershell
. .\scripts\pi-dev.ps1          # 只影响当前 PowerShell 窗口
. .\scripts\pi-dev.ps1 -Run     # 设置后直接 cargo run -- --new-window
```

脚本会：

1. 检查 `pi-host/node_modules` 已在联网构建阶段装好（不会自动下载）；
2. 在 `tmp/pi-runtime/host` 建一个指向 `pi-host/src` 的目录 junction（Node 仍能从
   `pi-host/node_modules` 解析依赖）；
3. 找到 Node 的绝对路径并校验版本；
4. 设置 `GONGWEN_PI_RUNTIME_DIR` 与 `GONGWEN_PI_NODE`。

应用端只认这两个显式路径或 exe 邻接的 `runtime/pi`，从不搜索 PATH 里的 `pi`。

## 生产运行时布局

```text
runtime/pi/
  node.exe              或 bin/node
  host/main.mjs         本目录 src 的发布副本
  host/*.mjs
  node_modules/         锁定的直接与传递生产依赖
  manifest.json         后续阶段补运行时 / 依赖 / 协议版本清单
```

应用启动本目录时传入：

- `GONGWEN_PI_RUNTIME_DIR`（可选，开发）或 exe 邻接 `runtime/pi`（生产）；
- 模型地址、密钥、模型名、上下文 / 输出上限、思考档位——全部来自应用当前设置；
- 工作区根目录与 **模型不可读写** 的 Durable JSONL 会话目录。

密钥只从 stdin 的 `init` 消息进入本进程，不写 argv、不写日志、不进事件。

## 协议（阶段二）

Rust → host（stdin，一行一条）：

```json
{"type":"init","protocol":4,"requestId":"…","runId":"…","workspace":{…},"model":{…},"task":{…},"limits":{…}}
{"type":"tool_result","callId":"c1","ok":true,"result":{…}}
{"type":"tool_result","callId":"q1","ok":true,"result":{"answers":[{"id":1,"value":"…"}]}}
{"type":"cancel"}
```

host → Rust（stdout，一行一条；stderr 只放诊断）：

| 类型 | 作用 |
| --- | --- |
| `ready` | 协议版本、host / Node 版本、模型名 |
| `progress` | 侧栏阶段提示 |
| `tool` | 一次工具调用（名称 + 简短说明，不含参数） |
| `tool_request` | 领域工具请求：`callId`、工具名、参数；Rust 校验后回 `tool_result` |
| `questions` | `ask_user` 的题：`callId`、1–3 道题与选项；Rust 校验授权要素后回答案 |
| `candidate` | `work/` 里最新 Markdown 候选，只作预览 |
| `report` | `finish_report` 的结构化清单，只进侧栏，不替换正文 |
| `submitted` | 提交的固定版本：路径、内容、SHA-256、字节数、说明 |
| `completed` | 本轮结束；`submitted: false` 表示没有交付正文 |
| `cancelled` / `error` | 停止 / 失败，均算完成 |

工具清单：`kb_search` / `kb_read`、`ms_search` / `ms_read`、`vocab_search`、
`check_document`、`doc_elements`、`ask_user`；正文交付仍是 `submit_document`，只审核 /
找资料用 `finish_report`。工具不写正文、不写表单、不提供 shell。

限制：单行输出 16 MiB、最多 50 000 条消息、提交 8 MiB、候选 2 MiB；Rust 侧再核对
`requestId` / `runId`、工具名 / 参数上限 / 调用次数、哈希、字节数与路径。错 `callId`
或迟到回复直接丢弃；RAG 在 Rust 后台线程执行，取消与超时不被检索堵住。

## 材料与技能

- 工作区中的 `input/materials/` 由 Rust 生成：程序编号文件名（`01.md`…）+
  `index.json`（标题、来源、角色、哈希、字节数），用户文字不进路径。
- 角色：`fact`（事实材料）、`incoming_letter`（来函）、`style_sample`（写法样稿）、
  `structure_sample`（结构范文）。只有前两种能作为事实来源。
- `skills/pi-skills/` 下有 5 份标准技能（gongwen-draft / revise / review / imitate /
  research），是 frontmatter + 自然语言步骤，不是流程解释器。
- `ask_user` 等待时间不计执行超时；取消会回收本轮进程。

## 文件环境

`src/paths.mjs` 实现 `WorkspaceEnv`，底层使用 `src/encrypted-files.mjs` 的统一受保护
`FileSystem`：

- 每次文件调用先解析真实路径（`realpath`），再检查仍在工作区内；
- 读允许 `input/`、`work/`、`skills/`、`resources/`；写只允许 `work/`；
- 绝对路径、`..`、指向工作区外的符号链接 / junction 一律拒绝；
- 单文件上限 8 MiB，写入用同目录临时文件 + `fsync` + `rename` 原子替换；
- 不注册 `bash`，`exec()` 明确返回 `shell_unavailable`；
- Durable JSONL 位于 `workspace/` 的兄弟目录，模型没有路径能读到；
- `init.workspace.encryption` 存在时，工作区文件、JSONL、交付与进度都写 GWA2；
  DEK 只来自 stdin，AAD 绑定槽位、用途 `pi` 与配置根相对路径，GWA1 / 明文只读兼容。

## 交付约定

- 模型必须调用 `submit_document({ path, summary })`，路径是 `work/` 下的完整
  UTF-8 Markdown；第一次提交的内容被复制成快照，之后改文件不影响已交付版本。
- 同一轮重复提交以第一次为准；不同内容会被拒绝。
- 没有提交、提交空文件、提交非法 UTF-8、路径越界、进程崩溃、取消、超时，一律在 Rust
  侧变成失败卡片，不安装提案。
- 写作指令在 `src/prompt.mjs`，包含公文 Markdown 最小语法与三条红线；技能正文来自
  应用随包的 `skills/gongwen-markdown/`，工作区里以只读文件提供。

## 已知边界

- 加密 JSONL 的追加需要解密并整份原子重写；真实长会话的性能、断电恢复和 Windows 文件占用
  仍需真机验收。
- 文件名、大小与修改时间仍可见；不防解锁后的恶意软件、内存读取或截屏。
- 旧 `session.sqlite` 不续跑；旧协议/旧 host 只在已有交付回执时重放，没有回执的中间任务
  需要从头写。
