# TASK：写作/真实知识库效果验收与模型能力校准

- 任务 ID：20261103-pi-6ab
- 仓库 / 实际 cwd：`D:/gongwen`
- 分支 / HEAD：`main` / `4d0eed473d3e43c162475dbb650e0e9002949245`
- 基线 dirty：无；主代理先创建本 TASK 与忽略目录 runner，worker 保留它们。
- 协作：按用户持续选择，本机 Pi 实现，主代理负责需求、审查、修正和真实测试。固定 `opencode-go/deepseek-v4.1-flash`，thinking high；不派生子代理。
- 数据与目的地授权：`pi-execution-authorization.md`。仅相关源码、任务与自造夹具，不得读取真实用户配置、密钥、稿件、真实运行工作区；认证交 Pi 自身处理。
- 执行更新：本机 Pi 实际核对到上述模型，但连续4次 `401 Invalid credential`，自动重试3次后停止，未进行工具调用或实现。主代理按 ai-handoff 改由一个 Codex 协作子代理 `gpt-6-luna` / high 实现同一契约；不改用其他外部模型API、不读取或更改Pi凭据。原始事件留 `tmp/pi-implementation/phase-6ab.events.jsonl`。
- 文件归属：worker 独占下面实现/测试文件；主代理只改 TASK、审查记录和忽略目录独立检查脚本，不并发编辑实现。
- 提交策略：不切分支、不提交、不 push、不安装、不发布；本轮用户只授权实施两项，没有新授权提交/打包。

## 本轮范围

用户所选为上一轮优先级表的①真实写作与知识库效果验收、②模型能力校准接入。先读 AGENTS.md、docs/ai-architecture.md、docs/pi-unified-writing-design.md、docs/pi-implementation/phase-5-review.md、docs/pi-runtime.md。

已存在常规/分阶段共用 Durable、明确提交和直接写入/撤销。Qwen 实际加载窗口262144，但生产 InitCapability 一直 None/false，host 对所有模型按最多96K保守预算。既有 JS 评测中 kb/ms 是模拟，不能当真实知识库验收；Rust PiKnowledge 还丢弃 RAG warnings。

不要回做上轮审查中“专用复核模型独立解析/旧preset归档”两项，也不做接口接入和旧引擎删除。不要扩充通用框架/DSL，不动排版和表单权限。

## A：真实资料链与效果评测

1. Pi 的知识检索结果必须保留结构化降级说明（embedding/rerank不可达、只用关键词等），模型结果和侧栏运行记录都能看到；零命中与降级不能混为一谈。有命中也要保留降级说明。保留分页、预算和证据登记。
2. 为既有评测增加实际 Rust PiToolContext + 本地临时 SQLite 知识/稿件库模式。只用自造数据库，走生产 KnowledgeStore / rag::retrieve / Pi 工具包装；不能仍用 JS 子串查找却称真实检索。允许只在测试模块实现隔离的 loopback 桥接进程，或复用现有测试基础设施；不可向用户数据库导入夹具，不增加正常应用启动入口，不要求目标机装 Python/cargo。
3. 评测明确区分 JS 模拟资料、真实 SQLite/FTS、实际 embedding/rerank。即便只测到真实 FTS 也不能声称向量链全通过。桥接严格限 loopback、随机会话身份、请求/响应体积、调用数、超时和退出清理；不得开放任意路径、shell、SQL或任意服务调用。不得把真实配置传给辅助进程。
4. 扩展/复用通知、多材料长稿、仿写、复函、局部修改、冲突、研究报告的固定用例。支持同题自动/倾向分阶段对照（记录实际策略），记录输入/技能/host版本或哈希、技术/业务判据、耗时、可得用量、原始成品和人工评审表。不能拼多轮最好结果或自动清洗 Markdown。已有9例与原始失败判据保留。
5. 针对已知失败改善标准 skill 的行为指引：研究先找资料再下判断、未检索不能称无资料、零命中应缩短检索词；复函按来函落款提出主送建议且正文不重复主送，不代用户表态。不要将事实疑点重新做成强制人工确认或写入阻断；业务评测失败如实记录，不为通关削弱判据。

## B：可使用的部署能力档案

1. 必须接入生产：模型服务的 Pi 区域可发起一个明确的“测试写作能力”动作（后台、可停止，提示只发送程序自造材料），并显示未校准/校准时间及策略建议摘要。复用现有模型连接/后台任务/实际窗口探测和 Pi host；不能只增加一个从未读取的 JSON 文件。保持界面简单，不让用户填输出长度、工作预算、模型参数量等。
2. 校准过程与真实稿件隔离：不在当前稿件创建AI轮次、不写正文/要素/真实知识库、不影响稿件撤销；本地结果保存到配置目录 Pi 专属目录，固定格式有读取体积和条目数量限制、原子保存，损坏时提示并保守回退。
3. 身份至少包含规范化后的实际端点、模型ID、实际加载窗口、思考设置和校准判据版本/相关技能版本。持久化不存密钥，不存带userinfo/query/token的URL；使用安全摘要与可读非敏感摘要。不同服务/模型/窗口/思考设置不得串用；模型同名但能力重载不可无条件复用。状态过期时回退保守默认并给理由。
4. 能力结论须有可核验的合成测试证据：分别记录原生工具/约束遵循、同题常规与分阶段成功情况、测试覆盖规模。网络失败/鉴权/服务未启动只算测试失败，不据此给模型打“必须短步骤”的标签。单篇短稿成功不证明96K/262K可靠工作预算，不以服务宣传窗口直接填 verifiedWorkBudget。
5. 工作预算只有专门且通过的长输入/远距事实保留检查才可标验证，且不得超过实际窗口和实际测试覆盖；短测试覆盖不足可只得出工具/策略建议，verifiedWorkBudget继续None并如实展示。不要生成看似已校准的虚假上限。合成容量探测有总时限、请求数/材料体积上限，默认测试要合理，不静默发送几十万字或无限重试。
6. needsShortSteps 只能来自明确的同题策略对照，记录对照成功/失败与判据，不能只按9B或一次失败猜；没有足够证据则保持未知。可展示“本次未形成策略偏好”，不强行得出正/负结论。人工质量评审不伪装为程序已完成。
7. 新运行在后台准备、实际窗口确定后选择匹配档案，将真实能力传给 InitCapability，策略原因/记录可回看。恢复旧运行使用当次能力快照，不因后来重新校准改变原阶段计划/上下文；旧记录serde默认兼容。校准失败/旧证据失效不能阻断普通Pi写作。
8. 测试动作跨窗口、取消、过期结果和修改模型后返回的结果都要隔离。复用现有模式，避免全局线程局部目录覆盖影响正常应用。

## 允许修改

- `src/pi_agent/`、`src/pi_agent.rs`：能力档案/校准模块、实际资料评测桥接（测试专用）、工具结果、协议/恢复所需最小字段。
- `src/ai_panel/pi_job.rs`：在后台准备/恢复中传匹配能力快照；不动正文写入权限。
- `src/app/ai_manage.rs`、`src/app.rs`、`src/app/`新增独立Pi校准模块：上述测试入口、状态和后台回收所需最小集成。不要扩展修改其他设置页。
- `src/ai_services/knowledge.rs`、必要的 `src/rag.rs`/`src/knowledge.rs`：只限真实资料隔离路径和返回告警，不重写检索算法。
- `pi-host/src/`、`pi-host/test/`：必要的能力信息/策略原因与合成校准使用；固定Pi 1.4.0依赖，不增加依赖/通用编排。
- `scripts/pi-writing-eval.mjs`、`scripts/pi-writing-eval.test.mjs`、可新增专用评测脚本及测试；`tests/fixtures/pi-writing-eval/`。
- `assets/pi-skills/`：已知效果问题的最小技能修正。增加文件登记 `src/pi_agent/skills/mod.rs`。
- `pi-host/package.json`/lock、`packaging/pi-runtime.json`、`scripts/pi-offline-acceptance.mjs`：仅必要版本同步，依赖不变。
- `assets/help/13-ai-workbench.md`：校准和策略说明，用户可理解即可。
- `docs/pi-implementation/phase-6ab-report.md`：worker报告，记录真实命令与边界。
- `docs/pi-writing-eval.md`、`tests/fixtures/pi-writing-eval/README.md`：同步真实资料评测与能力证据边界（主代理扩展授权）。
- `docs/pi-runtime.md`：仅同步host补丁版本与能力校准/恢复快照的实际说明（主代理扩展授权）。
- 范围扩展（已由真实基线确认）：`src/export/headings.rs` 的标题编号识别及其既有测试，`skills/gongwen-markdown/references/markdown-syntax.md` 与 `skills/gongwen-markdown/scripts/check_gongwen_md.py` 的对应说明/判据。仅修复把合法年份标题（例如 `2027 年度…`）当序号拒绝/剥掉的同一缺陷，保持真正序号去除；不改字体/版式/其它解析功能。host判据与Rust/Python检查应一致。相关技能文件新增时仍登记FILES。
- 同一缺陷的测试同步扩展：允许 `src/skill_pack.rs` 更新年份标题识别的重复判据/旧预期，合法4位年份保留；必须同时保留 `### 1 事项` 等真实手写编号的拒绝反例，不取消解析器/模型产物检查。
- 主审修正扩展：允许 `src/pi_agent/skills/{catalog,snapshot}.rs` 及 `workspace.rs` 加内置技能专用快照构建入口与稳定的技能内容摘要。校准不得读配置目录用户技能/停用清单；身份摘要排除每轮selected选择、纳入实际技能包/公共语法内容，默认自动入口也能匹配同内容的校准。普通写作的用户技能加载语义不变。
- `src/app/management_samples.rs`：仅补齐新增校准状态的测试fixture初始化，以测试取消后收到完成消息不落盘及不改正文。
- 完整回归发现基线 `be56a58` 已删除润色预设，但 `src/agent/engine_tests.rs` 仍断言旧预设标题。允许仅更新该断言为实际用户要求“压缩”与无残留 `{preset}`，保持选区钉住、完整正文和写入事件断言，不恢复旧功能。
- 主审第二轮how修复分工：`review_phase5_rust` 独占 Rust 的窗口来源身份修正（capability.rs、process.rs/PiModel、protocol.rs版本、ai_panel/pi_job.rs、app/ai_manage.rs及必要构造测试）；主代理同时仅修改JS登记工具、阶段指令、标准skill与任务/审查记录，不并发编辑这些Rust文件。窗口来源必须明确区分NativeLoaded/Service/ModelTable/Default/Manual，估算不能标实际。此轮host/Rust同步0.3.2，判据版本3。
- 阶段登记新增可选原文行号定位：程序从允许范围内原文取quote再用原路径/哈希核验；兼容旧quote输入，不做空白归一化、不放松来源判据。行号越界、跨当前材料块、过长行、两种定位冲突必须拒绝。

其余文件如确需改先通过控制消息报告并暂停，不自行扩大范围。主代理保留本 TASK、`docs/pi-implementation/phase-6ab-review.md`。

## 验证与交付

- Rust单测：告警透传（零/有命中均覆盖）、真实临时库检索/分页/证据；档案身份隔离/过期/损坏/上限/原子持久化；短证据不能冒充长预算；负面网络失败不生成能力结论；生产Init确实携带匹配能力；恢复使用旧快照；取消/过期校准不能覆盖当前模型。
- host `node --test`，评测脚本单测/selftest，保留原判据且新增假成功/缺依赖反例；实际资料模式的桥接隔离、越权/超限拒绝。
- `cargo fmt --all -- --check`、`cargo check --locked --all-targets`、严格clippy；针对性Rust测试。不要与主代理并行跑全库/模型压力测试。
- worker不要直接调用真实模型或读取任何真实API配置/凭据。主代理统一调度用户授权的 `127.0.0.1:12345`，自造真实库并实际检查结果。没有真实结果就明确未测。
- TEMP/TMP统一 `D:/gongwen/tmp`，规避跨盘导入；不得删除历史失败样本、跳过失败测试、改判据冒充通过。
- 完成所有允许实现后写报告（实际变更、每条命令结果、未完成/风险、无提交），停止工具调用等待主审；不得在摘要里宣称全部效果或较强模型已验收。

主审模型产物过 Rust 解析器时发现 host 漏检“附录 A”等手写编号；允许仅补 host 对同类既有编号的技术反馈与回归反例，保持原始失败产物不变，不放宽 Rust 判据。

收尾授权更新：用户随后明确要求完成后提交并推送GitHub；仅主代理统一集成本轮改动，不改worker历史分工，不发布或安装。
