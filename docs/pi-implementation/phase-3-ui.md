# 第三阶段主审修正：后台准备与运行引用落盘

本机 Pi worker：延续上次任务，直接修复下面明确的 how 缺陷。你上轮已停写，主代理现已接管并发现问题。

## 协作与边界

- cwd D:/gongwen，基线HEAD d7c236e；工作区是第三阶段未提交改动，全部保留。你不提交/推送，不改版本，不创建子代理。
- 你只修改 `src/ai_panel/pi_job.rs`、必要的新 `src/ai_panel/pi_*.rs`、`src/ai_panel.rs`、`src/app/{jobs,pi_jobs}.rs`、必要的 `src/ai_panel/{session,session_ui}.rs` 和对应测试。允许新增受控 DocJob 准备完成事件及确认通道，引用结构可以放 `src/pi_agent/recovery.rs`（仅事件元数据辅助，避免与主代理修改的其它pi_agent模块冲突）。
- 主代理同时拥有 `src/pi_agent/{runtime,process,workspace}.rs`、其它recovery主逻辑、全部host JS、scripts、workflow、说明/主审记录；你不改这些文件。遇到必须改先在回复中说明并停止，不静默扩展。
- 不修改主代理TASK/实施计划；完成后写 `docs/pi-implementation/phase-3-ui-report.md` 并停止。不要重新跑全套耗时检查，fmt/check和本次针对性测试即可，最终clippy/全量回归由主代理执行。
- 禁止读取auth/config/.env/模型密钥。只用自造材料与假依赖。

## 明确缺陷

1. 当前 start_pi_job 与 resume_pi_job 把 PiRuntime::locate（子进程版本检查、清单读取/hash）、@引用读取、工作区创建与材料快照、恢复快照/来源读取全部搬到了GUI主线程，注释“为了开跑前落盘必须在界面线程准备”是错误约束。这会阻塞UI。请恢复为后台准备：GUI只做纯内存校验/快照、创建Waiting轮次与job；后台线程完成所有运行时/引用/文件/恢复读取校验和准备。
2. 准备完以后，必须在host开跑前把稳定 PiRunRef 交回界面线程绑定原轮次，并对已有稿件强制保存会话，再确认给后台继续启动。可以增加 `DocJob::PiPrepared { reference, ack }`：后台等待有界/可取消的确认；应用线程核对 key/seq/轮次仍有效，更新引用并调用既有保存路径，成功才ack。不新增通用任务引擎，不把PiLaunch整份传GUI（含密钥且变体巨大）。尚未入库新稿沿用已说明的仅本进程恢复条件；保存出错不得悄悄启动假可恢复任务。参考 app/session.rs 的既有 save_session 调用。
3. 准备失败目前在 begin_job 后 `?` 返回，会把 doc.busy 永久留true。改为准备线程统一发 PiDone(Err) 收尾；失败、取消、旧job迟到、关闭标签都不能留下busy或启动未绑定的host。准备完成事件若过期应丢弃ack让后台结束；等待ack不能无限挂住，不在GUI用阻塞send。
4. 当前 start和restart两处先 `read_request`，后 `write_candidate`（该方法还追加request说明），造成记录 input_request 哈希对应旧内容，后续恢复必然拒绝。有候选稿时先写candidate及说明，再读取实际request快照生成哈希。补覆盖未采纳候选继续修改后中断恢复的回归。
5. restart时仍把旧文种与旧 system_date 套到当前新稿。如果用户改了文种/表单，新一轮从头写应使用当前 kind / TimeContext，旧requirement和安全校验材料仍保留；resume须校验当前文种匹配原record.kind，避免按旧规则生成后用新文种清洗。旧选区过期时目前默默放宽到全文，这会违背“只改选区”：应拒绝并提示重新锁定/重新发送，或做明确的用户确认；本次优先直接拒绝，别扩大权限。
6. 准备期间不要清空已存在候选提案，直到成功绑定新运行（失败应保留可继续审阅的旧提案）。恢复完成仍走原提案硬闸门，AI不能写正式正文/表单。

## 验收

- 慢引用读取/慢运行时检查替身：启动立即返回，GUI仍可取消；准备失败busy被收尾。
- 准备完成与停止/新job/关闭标签竞态：旧事件不绑定/不启动，ack失败后台结束。
- 有稿件库时先保存运行引用，再允许host启动；保存失败明确停止。
- candidate更新后的实际request哈希一致。
- 过期选区不放宽全文；resume拒绝文种不匹配；restart使用当前文种/日期。
- 只补本次行为回归，不削弱原测试。报告真实修改与命令结果，停止写入。
