# 第三阶段交付报告：恢复、交付去重与离线分发

日期：2026-11-02
实现者：本机 Pi 实现 worker（Pi Coding Agent 1.4.0）
仓库：`D:\gongwen`，分支 `main`，基线 `d7c236e`（未提交、未推送、未打 tag、未改版本号）。
TASK 与计划文档归主代理，本报告只记录实际改动与验证。

## 一、结论

1. **3A 可以恢复的真实写作任务**已实现：每轮写运行记录（身份、工作区相对引用、
   协议 / host 版本、正式基线与哈希、候选基准、要求 / 技能 / 选区 / 材料角色与哈希、
   模型引用、真实事实来源、已确认答案、交付回执与状态），与 `AiTurn` / `SavedTurn`
   关联；旧会话缺字段照常读。
2. **恢复入口**：中断 / 停止 / 失败轮次有「接着写」「从头写」；已归档 / 发布只读拒绝；
   恢复先校验工作区路径、快照完整性、host 协议 / 版本、当前正文基线与模型一致性，
   不匹配明确拒绝；沿用原工作区、Durable SQLite、runId / requestId。
3. **真实 Durable 恢复**：恢复启动 host，由 host 在 SQLite 所有权锁内 `Harness.open`
   + `harness.resume()`，用同一 `requestId` 找回提交；不复制 transcript，不搬任务图。
   崩溃的 write / edit 不是 replay-safe，Durable 会把中断调用交回模型，模型先读当前
   文件再继续（测试覆盖）。任务连续中断两次被 parked 时显示原因并给人工动作。
4. **停在问答**：旧 callId / 输入管道不沿用；恢复后旧提问以 interrupted 结果结束，
   模型继续或重新出题；授权类问题仍被 `clarify::agent_questions` 拒绝。
5. **成品去重**：`submit_document` / `finish_report` 先把内容复制成运行目录里的不可变
   快照并写回执（runId、requestId、路径、哈希、字节数、摘要、调用标识），再发事件；
   在「快照落盘 / 事件发送 / 界面装提案」任意点崩溃，恢复时由 host 在锁内重放回执，
   Rust 校验后交付同一产物；回执与文件绑定 run，哈希不符拒绝。
6. **多窗口所有权**：host 启动时用 Node 内置 `node:sqlite` 在运行目录的
   `host-owner.sqlite` 上开 `BEGIN EXCLUSIVE`，第二个 host 得到 `owner_busy`；锁随进程
   死亡由 OS 释放；工作目录名带 UUID。
7. **3B 固定运行时**：`runtime/pi`（Node 22.22.3 + host + 完整生产 node_modules +
   manifest / 许可证）已在本机 Windows x64 真实组装并通过无公网验收；打包脚本与
   CI / release 三平台接入；Linux 只写了脚本与 CI 接入，本机未真验。

## 二、3A 实际改动

### 新增

| 路径 | 内容 |
| --- | --- |
| `src/pi_agent/recovery.rs` | `PiRunRecord` / `PiRunRef` / `PiReceipt` / `PiRunLog`；运行记录与回执读写、上限、恢复校验（路径、快照、要求文件、材料、选区、状态）、事实来源文件 |
| `pi-host/src/delivery.mjs` | 回执与不可变快照；清单按 1 MiB 写上限裁剪并记 `omitted`；读回校验版本 / runId / 哈希 |
| `pi-host/src/owner.mjs` | 独立锁库上的 SQLite 独占事务；可读的 `owner_busy` |
| `pi-host/fixtures/host-hooks.mjs` | 测试专用引导：在 WorkspaceEnv 写盘后 / 回执写后同步阻塞，制造精确崩溃窗口 |
| `pi-host/fixtures/owner-holder.mjs` | 测试专用持锁进程 |
| `pi-host/test/recovery.test.mjs` | 回执 / 清单上限、真实双进程锁竞争与杀持锁者、回执写后强杀、写盘后工具结果未提交强杀、提问中断 |
| `docs/pi-runtime.md` | 运行时组装、manifest、验收、打包接入、恢复与工作区边界、干净机安装步骤 |

### 修改（主要）

- `src/pi_agent/protocol.rs`：协议 **2**、`HOST_VERSION=0.2.0`、init 增加
  `recovery { mode: "resume", requestId }` 与 `workspace.delivery`；`submitted` /
  `report` 增加 `recovered`。
- `src/pi_agent/process.rs`：运行记录随开跑写入；工具返回 / 用户回答在交给 host 之前
  落盘真实事实来源；回答另存纯答案值；交付以 host 回执与快照为准；恢复不绕过 host
  所有权锁；取消 / 超时 / parked 回写状态；`PiDelivery` / `PiReport` 增加
  `recovered`、`sources_sha256`。
- `src/pi_agent/workspace.rs`：运行目录带 UUID；新增 `run` 字段、`reopen`、
  `read_request`、材料回读；工作区仍在配置目录，模型不可见 `pi/`。
- `src/ai_panel/pi_job.rs`：开跑前同步准备运行时 / 工作区 / 运行记录，强制保存运行引用；
  `resume_pi_job(continue|restart)`；「接着写」才做正文 / 模型 / 库 / 选区校验，
  「从头写」以当前正文与当前模型建新 run，只安全读取旧材料；过期选区明确丢弃并提示；
  恢复重灌落盘事实来源与纯答案；会话重载后恢复原始依据。
- `src/ai_panel.rs` / `session.rs` / `session_ui.rs` / `ui.rs`：`AiTurn.pi_run` 与
  `SavedTurn.pi_run`（旧会话默认 None）；Pi 轮次的「接着写 / 从头写」按钮与未入库说明；
  切会话 / 新会话清掉 `pi_ask` / `pi_report` / `pi_bridge` / `pi_sources`；接受提案后把
  运行记录标成 Accepted（不再给恢复入口）；重载时按哈希恢复事实来源。
- `src/app/pi_jobs.rs` / `jobs.rs`：交付 / 清单成功时更新运行引用；`DocJob::PiDone`
  装箱（`PiOutcome` 带整篇正文，避免枚举变体过大）。
- `pi-host/src/{protocol,agent,main,prompt}.mjs`：协议 2、delivery 路径、回执写入、
  恢复模式、`restartLimit: 2`、`onReport` parked、恢复提示词、所有权锁、回执重放事件。

### 恢复语义（要点）

- 「接着写」：工作区路径必须在配置目录 `pi/workspaces` 下且不越界；`input/current.md`
  哈希、`input/request.md` 哈希、材料哈希 / 字节数、选区原文哈希、host 版本 / 协议、
  当前正式正文基线、当前 Pi 模型引用全部一致才启动；Durable 库缺失则明确提示改用
  「从头写」。恢复成功仍走本地 `check_delivery` 与 `reviewed_draft` 闸门。
- 「从头写」：不校验旧正文 / 旧库 / 旧模型；旧材料按路径 + 哈希 + 体积校验后复用，
  旧选区只在位置与原文都对得上时沿用，否则丢弃并写明。新 run 用新 UUID、新
  requestId，写新运行记录。
- 交付去重：有回执就由 host 在锁内重放；Rust 校验回执与快照后才交付，不重读 `work/`，
  不重复调用模型、不重复接受正文。取消 / 超时 / 失败 / 崩溃不产生成功回执。
- 孤立工作区：本期不自动清理；删除会话不递归删目录，运行目录保留在
  `配置目录/pi/workspaces/`，文档写明由用户手动清理。未入库新稿只在本次会话保留
  运行引用，界面明确说明重启后不能「接着写」。

## 三、3B 实际改动

### 新增

| 路径 | 内容 |
| --- | --- |
| `packaging/pi-runtime.json` | 固定 Node 22.22.3 与三个官方压缩包 SHA-256、Pi 1.4.0、host 0.2.0 / 协议 2、lockfile 路径 |
| `scripts/stage-pi-runtime.ps1` | Windows x64 组装：校验 SHA256 → 解包 → 随包 npm `ci --omit=dev --ignore-scripts` → 复制 host → 许可证 → manifest → 校验 |
| `scripts/stage-pi-runtime.sh` | Linux ARM64 / x64 同口径组装（jq / python3 / node 读配置） |
| `scripts/pi-runtime-manifest.mjs` | 生成 manifest（平台、版本、lockfile 哈希、全量文件 SHA256，`source: npm-ci`） |
| `scripts/pi-runtime-licenses.mjs` | 收集完整生产依赖的许可与原文到 `licenses/` |
| `scripts/pi-runtime-verify.mjs` / `verify-pi-runtime.ps1` | 打包前全量校验（平台、版本、`source`、文件哈希、必需入口、Node 版本） |
| `scripts/pi-offline-acceptance.mjs` | 从打包目录启动 host，本地假 OpenAI 走「读技能 → 读材料 → 写文件 → 提交」，再验回执重放与 SQLite |
| `scripts/pi-offline-guard.mjs` | 验收专用 preload：导入 host 前拦截 net / tls / dns / fetch，只放回环，拒绝并记录其它目标 |
| `scripts/check-node-glibc.sh` | 核查随包 Node 的 GLIBC 符号版本（默认 ≤ 2.28）、GLIBCXX 与解释器 |
| `scripts/pi-runtime-verify.mjs` | Linux 打包共用校验 |

### 修改（主要）

- `src/pi_agent/runtime.rs`：生产路径读 `runtime/pi/manifest.json`，校验平台 / Node /
  host / 协议与关键入口哈希；开发环境变量模式明确标注「开发运行时（未经安装包验证）」。
- `scripts/package-portable.ps1`：出包前调用 Pi 运行时校验，按 manifest 复制
  `runtime/pi/**`（含 manifest 自身），缺运行时 / 平台不符 / 哈希不符直接失败。
- `scripts/stage-linux-portable.sh`、`package-deb.sh`、`package-arch-pkg.sh`：同样校验并
  复制 / 拒绝出包；`verify-deb-install.sh`、`verify-arch-pkg-install.sh` 检查安装后的
  Node / host / manifest 与 `node --version`。
- `.github/workflows/release.yml`：Windows x64、Linux ARM64、Arch x86_64 三个作业都先
  组装运行时、跑无公网验收，再进入原打包流程。
- `.github/workflows/ci.yml`：Windows 作业用随包 Node 跑 `pi-host` 的 `node --test` 与
  无公网验收；Ubuntu 20.04 ARM64 冒烟作业组装运行时、复制进 deb staging 并跑验收。
- `.gitignore`：忽略生成的 `/runtime/pi/`。
- `THIRD_PARTY_NOTICES.md`：新增 Node / Pi / 传递依赖许可说明与随包许可证位置。

### 运行时布局与体积

```text
runtime/pi/{manifest.json,node.exe|bin/node,licenses/,host/{main.mjs,package.json,node_modules/}}
```

Windows x64 实测组装结果：**1.3 万个文件、约 162 MB**（`node.exe` 约 87 MB，
生产依赖约 75 MB；manifest 约 2.6 MB）。完整安装包会明显变大，增量升级包只带变化文件。

## 四、命令与结果

在 `D:\gongwen` 实际执行（日志在忽略的 `tmp/pi-implementation/`）：

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --locked --all-targets -- -D warnings` | 通过 |
| `cargo test --locked pi_` | **137 passed / 0 failed / 2 ignored**；含恢复校验、回执重放、回答先落盘、选区丢弃、旧会话兼容、Pi 卡片不跨会话 |
| `cargo test --locked ai_panel::session::tests` | **18 passed / 0 failed** |
| `cd pi-host; node --test` | **42 passed / 0 failed / 1 skipped**（跳过项仍是无权限创建符号链接）；含双进程锁竞争、杀持锁者后重获、回执写后强杀、写盘后工具结果未提交强杀、提问中断 |
| `scripts/stage-pi-runtime.ps1 -Force`（Windows，缓存 zip） | 通过：Node 22.22.3、host 0.2.0、12795 文件、92 个依赖许可 |
| `scripts/pi-offline-acceptance.mjs --runtime runtime/pi` | 通过：4 次模型调用、回执重放 0 次新调用；守卫生效、无非回环连接 |
| `scripts/pi-offline-acceptance.mjs --runtime dist/pi-check/runtime/pi` | 通过：**从打包目录运行**，不是 import 源码 |
| `scripts/package-portable.ps1 -Suffix win-x64 -OutputDir dist/pi-check -SkipBuild -Force` | 通过：校验 Pi 运行时并复制（含 manifest） |

未执行：Linux ARM64 / x64 真机组装与验收（本机只能真验 Windows）、完整 Inno 安装 /
卸载 / 升级真机、GUI 手动验收、真实内网模型联机、全量 Rust 回归。上述均不声称通过。

## 五、与方案的出入与取舍

1. **协议升到 2**：恢复是新的 init 字段，旧 host 必须拒绝而不是猜；`HOST_VERSION`
   升到 `0.2.0`，运行记录保存并在恢复时严格比对。
2. **`DocJob::PiDone` 装箱**：`PiOutcome` 现在带完整正文与来源，枚举变体过大触发
   clippy；用 `Box` 只改构造 / 解引用，不改变交付规则。
3. **恢复必须经过 host**：早期草稿曾在 Rust 直读回执，会绕过 SQLite 所有权锁；已改为
   host 在锁内重放，Rust 只做校验与本地闸门。
4. **事实来源增量落盘**：每次工具成功与用户回答都在交给 host 前写
   `pi/sources.md` + 哈希；恢复时重灌。`record.confirmed` 保留显示字符串，
   `record.answers` 另存纯答案值，避免把模型题面假设当事实。
5. **「从头写」不校验旧正文 / 旧模型**：只校验旧材料与路径；过期选区明确丢弃并提示。
6. **运行时 manifest 较大**：全量文件清单约 2.6 MB，启动定位时读一次；不每帧遍历
   `node_modules`。
7. **验收网络控制**：除 netstat / ss 采样外，增加验收专用 preload 拦截 net / tls /
   dns / fetch；它不是生产开关，只在验收命令里 `--import`。
8. **测试夹具位置**：`pi-host/fixtures/`（不在 `test/` 下，避免被 `node --test` 当测试文件）。
9. **未改同步包格式**：GWX / 工作区不进同步，按 TASK 要求。

## 六、已知风险与待主代理验收

- **Linux**：Node 的 GLIBC / GLIBCXX / 解释器只写了检查脚本与 CI 接入，未在真机跑；
  deb / Arch 安装后的启动、portal 文件框、Pi 运行均未真验。
- **Windows 安装包**：Inno 完整包 / 增量包未实际生成安装；`package-portable` 的 Pi
  复制已实测，Inno 侧依赖现有 `Source: "*"` 与 `InstallDelete`，需真机安装 / 升级验收。
- **真实模型**：只用本地假 OpenAI 与测试桩；真实内网模型的工具遵循、恢复行为未测。
- **GUI**：恢复按钮、未入库提示、parked 文案未在真机点过；Rust / host 侧逻辑有回归。
- **体积**：完整包增加约 162 MB；是否需要按平台裁剪 node_modules 待产品决定。
- **孤立工作区**：不自动清理，长期使用会累积；本期按要求只保留并在文档说明。
- **真实技能资源**：无公网验收用的是简化 SKILL 夹具；真实 5 份技能由
  `cargo test --locked skill_pack` 与界面集成覆盖，不能把验收脚本当真实技能验收。

## 七、停止点

本报告写出后停止修改代码，等待主代理审查、修正与全量回归。未提交、未推送、未发布。
