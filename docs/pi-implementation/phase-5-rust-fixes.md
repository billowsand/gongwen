# TASK：修正第五阶段 Rust/UI 主审缺陷

仓库与cwd `D:\gongwen`，HEAD仍为 `3d56f73b604b3cd6a033da6f2d75688a6c612bfb`。完整第五阶段diff已在工作区，不覆盖、不回退。主代理正在独占修正 `pi-host/`，你**绝对不要修改pi-host或评测脚本**。持续授权与模型沿用phase-5-task，worker不提交/不派生/不切分支。请只改以下Rust文件及其模块内有意义的回归测试，完成后立即停止交付。

先读AGENTS.md、docs/ai-architecture.md和phase-5-task.md。文件归属：`src/lmstudio.rs`、`src/lmstudio/context.rs`、`src/ai_panel.rs`、`src/ai_panel/{composer_ui,pi_job,ui,ui_tests,mention}.rs`、`src/pi_agent/{recovery,process,tools,protocol}.rs`。必要的src/app/ai_manage.rs可改。新测试文件可在上述模块目录内增加。报告只写 `docs/pi-implementation/phase-5-rust-fixes-report.md`。

范围补充：`src/draft_page/writing/`、`src/draft_page/ribbon.rs`，仅限旧任务的只读历史访问、显式迁移与停止旧执行入口；主代理不修改这些Rust文件。保留原始任务与来源，不删除用户历史。

具体修复：

1. `parse_native_loaded_window`只能采信`loaded_context_length`，不能max_context_length回退。`probe_native_window`仅对loopback本机已配置模型地址探测（本机12345可用），远端不盲目请求原生路径/携带key。服务更小上限继续约束手动值。补原生字段缺失、非本机不探测、较小手动优先测试。
2. 当前恢复 `resume_pi_job`用composer当前pi_strategy，原Phased重启后变Auto。把偏好保存到PiRunRecord（serde默认Auto兼容旧记录），新任务保存，恢复用运行记录值。实际策略常规转phased的恢复宿主会根据progress.json决定，主代理修host。测试明确Phased跨序列化/重启不变。
3. `copy_turn_to_pi`清理pi_ref_roles，旧材料/引用角色默认Fact，不继承输入框残留角色。更好地从旧writing任务材料/风格/框架复制时保留对应用途（旧writing面板此前ribbon隐藏，别丢用户历史任务；先查现有入口，提供只读历史访问/显式迁移，禁止新建旧写作执行）。
4. `composer_ui`删除引用按source+id，不能按title删除所有同名引用。当前`@《标题》`标记无法区分同名，给插入标记增加可消歧的显示名或明确阻止不确定绑定，不能漏删/串绑；更新live_refs与unlink的一致逻辑，补同名不同来源删除一项另一项仍生效测试。最小可用：存放Reference里的title字段用于唯一显示名（不影响来源真实标题，发送时read_reference会取真实题名），第二个同名显示补来源/编号。不要迁移时编造对应关系。
5. 旧卡片`CardAction::Rerun`仍调用start_panel_request，旧Reported/Failed/Stopped/Interrupted等按钮可新建旧流程；统一改为CopyToPi或Pi恢复/重试，不允许旧入口再调度。删除无用枚举/函数避免dead_code，不把旧历史展示删除。旧writing历史不再提供运行/重新生成按钮，保留查看/导出/迁移。依赖旧UI的自动测试按新行为更新，不能用allow(dead_code)掩盖生产旧调用。
6. kb_read/ms_read固定24k不随预算。主代理会在host的bridge中把隐藏参数`limit_chars`覆盖为本次readBudgetChars；你在Rust读取参数并clamp1..MAX_READ_CHARS用于read_page，缺省保持24k兼容，`next_offset`与unread正确。不能接受模型通过limit_chars突破上限。字段不必暴露给模型工具schema，host负责注入。补小limit_chars分页回归。
7. 在AI管理页统一模型文案：不要再提示“小模型分阶段和Pi两条产品线”，现有旧模型字段若被API/辅助工具复用则保留但说明具体用途，避免无关配置迁移破坏API。

测试：cargo fmt --all、cargo check --all-targets、必要的Rust相关测试；TEMP/TMP指D:/gongwen/tmp。完整Rust回归主代理最后统一执行，避免重复长跑。主代理同时改host测试，不运行node测试以免抢在半成品。不可读取真实配置/稿件/凭据，不直接调用真实模型。按主审明确how修复，不需要询问用户。完成报告列出文件与实际测试、剩余项并停止写入。
