# TASK：Pi 单一侧栏入口（第四阶段 A）

执行说明：启动本机 Pi 被自动审批拒绝，原因为可能向未明确授权的外部模型服务传输项目源码。
未重试或改用其它外部代理绕过。主代理在本地完成实现，后续范围补充为
`src/ai_panel/{ui,skill_job}.rs`、`src/app/{session,tabs}.rs`（删除失效菜单动作，保留旧会话后端文种检查）。
实际结果以 `phase-4a-review.md` 为准，没有 Pi worker 交付报告。

用户明确反馈当前输入区「Pi 试用 + Pi 设置 + 旧自动模式 + 旧发送」混杂，无法有效测试。直接改代码，不只给方案。你是用户指定的本机 Pi worker，主代理负责审查与修正。

## 基线与文件归属

cwd D:/gongwen，main，HEAD d7c236e。第三阶段大量未提交改动全部保留，快照在 tmp/pi-implementation/phase-4a-baseline。先读 AGENTS.md、docs/ai-architecture.md 红线、docs/pi-writing-product-design.md、docs/pi-implementation/phase-3-review.md。

允许修改 src/ai_panel/composer_ui.rs、src/ai_panel/ui_tests.rs、src/ai_panel/pi_job.rs、src/ai_panel.rs；必要时拆出 composer_pi.rs 并登记 mod，新增 docs/pi-implementation/phase-4a-report.md。其它文件先报告再等主代理决定。不要创建子代理、提交、推送、改版本、安装或读取配置/密钥。主代理拥有本TASK、计划、验收记录。

## 用户可见契约

1. 侧栏新请求只有 Pi 一条执行路径。普通圆形发送、Ctrl+Enter 都用同一入口启动 Pi；不得悄悄调用旧技能路由/旧 flow 或旧起草模型。Pi 未配置或运行时失败给明确提示，不回落旧引擎。保留运行中停止和现有红线。原来的「智能写作（Pi 试用）」额外启动按钮删除。
2. 输入区精简：一个可见明确的「Pi · 自动」或「Pi · 修改」技能下拉，加号、单一发送/停止、材料/引用/选区标签。下拉只列五种 Pi 标准技能（及自动）。材料用途放加号菜单内的分组，必要说明保持可发现。不要再把「Pi 设置」和旧「自动」两个体系摆一起，不在底栏放未被 Pi 使用的风格/预设/旧检索开关。
3. `/` 菜单和加号选技能只列 Pi 标准技能，选中写 composer.pi_skill。`@` 文档引用继续有效。旧会话残留 composer.skill/preset/use_rag/style 不得影响或误导 Pi 新请求；pi_job 中旧技能警告也应移除。知识库由 Pi 自主调用领域工具，不能保留一个实际上无效的检索开关。
4. 正常发送仍保留「把工作稿写入正文」的人工确认卡语义，不能让该指令自动接受提案；Pi 分支也应遵守。这是本地动作，不走旧流程。旧 /compact 不得误调用旧会话压缩；明确说明 Pi 由其运行时管理上下文。
5. 暂不全库删除旧智能体代码。旧会话可展示和恢复旧任务，小模型分阶段起草入口保持原样；但本输入区不能新发旧智能体请求。移除本输入区弃用的 UIfn，处理 dead_code，不加 allow(dead_code) 掩盖问题，不为保留旧入口而新增模式切换。
6. 侧栏空态/输入提示应明确 Pi，不能继续写旧 /技能/流程说明。如果该文本在范围外，先报告主代理。

## 验收

针对新契约写必要测试：普通发送确实创建 Pi 轮次/准备或明确 Pi 配置错误，绝不创建旧 flow；键盘与点击共用路由；Pi 菜单不会暴露旧技能；残留旧选择不进入 Pi prompt；提交正文仍需人工确认。保留旧历史恢复测试，不把历史恢复迁移到 Pi。现有旧发送测试如与新契约冲突，按新行为改写并说明，不削弱底层旧解析/路由测试。

cargo fmt --all -- --check、相关 Rust 测试、cargo clippy --locked --all-targets -- -D warnings。回环受沙箱影响时报告，不改原测试判据。不要跑全量/release/打包，主代理负责。完成报告后立即停止修改并交付。
