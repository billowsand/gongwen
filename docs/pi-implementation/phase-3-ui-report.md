# 第三阶段主审修正报告：后台准备与运行引用落盘

日期：2026-11-02
实现者：本机 Pi worker（延续第三阶段）
仓库：`D:\gongwen`，基线 `d7c236e`；未提交、未推送、未改版本号。

## 一、实际改动

### 1. 准备全部移回后台（缺陷 1）

`src/ai_panel/pi_job.rs` 重写为两段：

- **界面线程**只做纯内存校验与快照：只读 / busy / 选区锁定 / 要求非空 / 技能解析、
  `pi_chat()` 与模型参数、`generated_markdown` / `pi_base` 快照、粘贴材料与 `@` 引用清单、
  `begin_job()`、创建 `Waiting` 轮次、挂取消标志与提问桥、状态栏提示，然后立即返回。
- **后台线程**（`spawn_prepare`）完成：`PiRuntime::locate()`（含 Node 版本检查与 manifest
  读取 / 哈希）、`@` 引用读取（`PiToolContext::read_reference`）、`base_dir()` 与
  `PiWorkspace::prepare`、材料快照、候选稿写入、运行记录、工具上下文；恢复路径还在后台
  读取 `run.json`、快照 / 材料 / 来源文件并做全部校验。

`start_pi_job` / `resume_pi_job` 不再在界面线程 `?` 返回准备错误；所有准备失败都走
`DocJob::PiDone(Err)`，由既有 `apply_doc_job` 释放 `busy` 并结束轮次。

### 2. 运行引用交接与保存确认（缺陷 2）

新增 `src/ai_panel/pi_prepare.rs` 的 `PiPrepareAck`（只含一个容量 1 的确认通道）与
`DocJob::PiPrepared { turn_id, reference, dev_runtime, ack }`：

- 后台准备完成后先发 `PiPrepared`，只带稳定的 `PiRunRef` 与确认通道，不带密钥 / 正文 /
  `PiLaunch`；
- 界面在 `apply_doc_job` 里核对 `key` / `seq` / 轮次仍是本任务的 `Waiting` 且
  `usage_job_seq` 一致；过期事件直接丢弃 ack（后台 `Disconnected` 立即结束）；
- 有效时绑定 `turn.pi_run`，清旧提案与临时 Pi 卡片，然后对已有稿件库记录强制保存会话
  （复用 `app/session.rs` 的 `save_ai_session`，改为 `pub(super)` 供 `app/jobs.rs` 调用）；
- 保存失败（稿件库未打开、会话尚未与库对齐、写库报错）时撤掉刚绑定的 `pi_run`、回
  `ack(Err)`，后台不启动 host；`PiDone(Err)` 正常收尾 `busy`；
- 后台 `wait_prepared` 有界等待（30 秒）、每 100 ms 检查取消；取消 / 界面丢弃事件 /
  超时都不启动 host。

### 3. 失败、取消与竞态收尾（缺陷 3）

- 准备失败统一发 `PiDone(Err)`；`apply_doc_job` 对 `PiPrepared` 保留 `busy`，对
  `PiDone` 释放 `busy`。
- 停止会递增 `job_seq` 并置取消标志：旧 `PiPrepared` / `PiDone` 因序号不符被丢弃，
  ack 随之断开，后台结束；关闭标签时文档找不到，事件被丢弃，ack 断开。
- 不在 GUI 用阻塞 send；确认通道用 `try_send`，后台用 `recv_timeout`。

### 4. 候选稿与实际输入哈希顺序（缺陷 4）

新增 `finalize_input(workspace, base, before)`：先 `write_candidate`（它会往
`input/request.md` 追加「继续修改未采纳稿」说明），再 `read_request()` 取实际文件哈希。
`start` 与 `restart` 两处都改用这个顺序，运行记录的 `input_request` 与实际文件一致。
新增回归 `candidate_note_is_written_before_the_request_hash_is_taken`。

### 5. 文种 / 日期 / 选区（缺陷 5）

- 「从头写」不再套旧 `record.kind` / `record.system_date`，改用点击时的当前文种与
  `TimeContext::now()`；旧要求、技能、材料、事实来源、纯答案值仍保留。
- 「接着写」新增 `check_resume_kind(record.kind, 当前文种)`，不匹配直接拒绝并提示切回
  文种或「从头写」。
- 从头写时旧选区位置 / 原文哈希对不上直接报错（`restart_selection` 返回 `Err`），
  不再默默放宽到全文；提示重新锁定选区后发送。

### 6. 准备期间保留旧提案（缺陷 6）

`start_pi_job` / `resume_pi_job` 不再提前清 `doc.ai_proposal` / `ai_review_baseline`；
只有 `PiPrepared` 成功绑定并保存运行引用后才清提案。准备失败时旧提案仍在，可继续审阅。
恢复交付仍走原 `apply_pi_done` 的 `check_delivery` + `reviewed_draft` 硬闸门，AI 不写
正式正文 / 表单的规则未改。

### 7. 恢复记录接续（主审补充）

`PiRunLog::open_existing`（仍在 `pi_job.rs`）改为**只** `from_record`，不写回
`run.json`、不改状态 / message；工作区所有权与 Ready 后的状态更新由主代理在
`process.rs` / host 锁内处理。未新增对 `src/pi_agent/recovery.rs` 的修改。

## 二、新增回归

- `candidate_note_is_written_before_the_request_hash_is_taken`：先候选后哈希，且无候选
  时不改要求文件。
- `a_stale_selection_is_rejected_on_restart_instead_of_widening_to_full_text`：同偏移换
  内容 / 越界都报错，不回落全文。
- `resuming_with_a_different_document_kind_is_rejected`：文种不匹配的提示。
- `prepared_event_binds_the_run_reference_and_keeps_the_document_busy`：绑定引用、标开发
  运行时、清旧提案、保留 `busy`。
- `a_stale_prepared_event_is_dropped_without_binding`：过期事件不回 ack、不绑定。
- `a_library_document_must_save_the_run_reference_before_starting`：保存不了就 ack 错、
  撤恢复入口，`PiDone(Err)` 收尾 `busy`。
- `a_failed_preparation_keeps_the_existing_proposal`：准备失败保留旧提案并清 `busy`。

## 三、命令与结果

- `cargo fmt --all`：通过（`cargo fmt --all -- --check` 待最终确认）。
- `cargo check --locked --all-targets`：**被主代理正在修改的
  `src/pi_agent/runtime.rs:253` 编译错误阻断**（`Sha256: std::io::Write` 未实现，
  `std::io::copy(..., &mut hasher)`），该文件不在我的修改范围，未改动；同时
  `src/pi_agent/process.rs` 出现过 `lock_run` 未定义的中间态，属主代理同批改动。
  因此本次针对性 Rust 测试未能实际运行，**不声称通过**。
- `pi-host` 测试、无公网验收、全量回归未重跑（本轮只改 GUI 交接；且 Rust 编译被上述
  阻断）。

## 四、未做 / 待主代理

- 慢引用 / 慢运行时替身的「启动立即返回、GUI 可取消」没有自动化测试：`PiRuntime::locate`
  读环境变量、`read_reference` 走真实知识库 / 稿件库，构造替身会引入全局环境变量或
  新注入点；按本轮边界未扩展 `pi_agent` 接口。行为由代码路径保证：界面线程不再调用
  `locate` / `read_reference` / 文件准备，全部在 `spawn_prepare` 内。
- 未在 GUI 真机点恢复 / 准备中停止 / 关闭标签；Rust 侧事件与 ack 逻辑有回归。
- 主代理需在 `runtime.rs` / `process.rs` 修好后跑 `cargo check` / 针对性测试 / 全量回归。

## 五、停止点

本报告写出后停止修改代码。未提交、未推送、未发布。
