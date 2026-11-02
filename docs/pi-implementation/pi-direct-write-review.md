# Pi 直接写入与撤销：主审记录

## 基线与授权

- main / f0c00a5，加上已验收但未提交的 4C。原有 dirty、staged/unstaged diff 与新文件快照在 `tmp/pi-implementation/pi-direct-write-baseline/`。
- 用户已确认直接进入正文、保留修改前版本和撤销、事实问题只提示，取消强制事实勾选和接受提案。
- 本轮启动的外发审批曾拒绝，随后用户明确授权并要求今后同范围不再询问。持续授权见 `pi-execution-authorization.md`；本次重试已获准。
- 本机 Pi RPC 实际核对 `opencode-go/deepseek-v4.1-flash` / thinking high；日志在忽略的 `tmp/pi-implementation/pi-direct-write.*`。
- 主代理拥有 TASK/讨论/计划/本记录；Pi 修改限定实现文件，报告后停止，主代理再 review/fix。未提交/推送/改版本/发布/重打安装包。

## 主审验收重点

1. 对照 dirty 基线核对本轮实际文件，保留 4C 目录、技能安全和动态加载实现。
2. 正常 Pi 成功真正更新 generated_markdown，无隐藏提案/事实勾选/强制审阅；DraftInput 完全不变。
3. 事实来源提示分类清晰，技术/权限检查不被放开；取消/失败/报告/只读/选区越界/陈旧基线不覆盖正文。
4. 精确 before/after 在替换前可恢复；消费状态和撤销协调，持久化失败保留成品，不吞错。
5. 多标签后台结果与 Ctrl+Z 隔离；卡片撤销在人工后续编辑后不覆盖，撤销后同回执不重放。
6. 原会话/提案兼容、可选差异和冲突产物可查看，文案与架构规则同步。

## 实际交接与 how 修正

Pi 在最后一次 clippy 后遇到 Connection error，自动重试结束后 agent_settled，runner exitCode 0。
这只表示进程已停止，不表示实现验收通过；worker 没能生成报告。主代理确认停止后审查真实文件，
并用只读审查补核保存/恢复边界，再接手修正。

1. 最后一轮 worker clippy 失败在 PiDiffView 手写 Default；改为 derive，未屏蔽警告。
2. 取回保留成品可绕过只读：commit 统一检查 read_only，补已发布/归档取回测试。
3. mark_pi_written 清 pending，真正保存会话失败后又清 write，导致无法重试；失败时从 write 恢复完整 pending，补实际数据库外键保存失败与解决后取回的回归。
4. 正文基线比较规整了列表标点，会忽略用户部分手工改动；改为精确全文比较，补标点修改冲突测试。
5. 后台标签仅直接赋值，无编辑器撤销；active_doc 索引也不足以证明共享 editor_id 的状态归属。增加小型按稿件隔离的 egui 状态绑定，后台写入临时绑定自己的状态后恢复当前标签，源码编辑器切换时绑定对应稿件。补真实 Ctrl+Z 跨标签隔离测试。
6. 首次绑定曾清掉预设光标，全量粘贴测试暴露 2 项退化；首次绑定保留已有 TextEditState，切换稿件才恢复各自状态，不改粘贴判据。
7. 同步消费状态到会话运行引用；Ctrl+Z/Ctrl+Y 的前后内容同步卡片撤销状态；撤销清理失效导出结果。
8. 保留成品的“查看差异”原先只显示正文，改为明确“查看成品”；成功写入的差异仍独立只读。更新旧提案文字和后台撤销说明。

首轮 root Rust 全量 2771 passed / 2 failed / 84 ignored，失败就是第 6 项粘贴光标退化；已修正并重跑。
root host 全量 49 passed / 0 failed / 1 skipped。最终完整结果见下一节。

## 最终验收

- cargo test --locked --all-targets：**2774 passed / 0 failed / 84 ignored**。第二轮中的一个预览逐帧用例曾报未画出纸张，未修改其判据；单独复核及最终完整重跑均通过。
- cargo clippy --locked --all-targets -- -D warnings：通过，无屏蔽警告参数。
- cargo fmt --all -- --check 与 git diff --check：通过。
- 最终 node --test：**49 passed / 0 failed / 1 skipped**。最终重跑第一次自动审批超时未执行，按工具允许重试一次后获准并通过；不是风险拒绝，没有请求用户重复确认。
- 验证包括：成功直接写入/无提案、事实疑点只提示、原表单不改、只读与选区保护、精确正文冲突、真实数据库保存失败成品保留与取回、卡片撤销防覆盖、跨标签 Ctrl+Z、会话重载保留前后文本与消费状态、旧会话兼容。
- 没有新增框架或依赖、没有放开工具权限，没有提前实施 4D/4E/4F。

本次代码验收完成。未做真实 GUI 目视或真实业务模型验收，未重打安装包；未提交、推送、改版本或发布。
已入库稿件的撤销记录随会话保存；未入库稿件保留任务基线与成品快照，进程内 Ctrl+Z 栈不承诺跨重启恢复。
