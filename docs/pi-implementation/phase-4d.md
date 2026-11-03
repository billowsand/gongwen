# TASK：第四阶段 D——标准技能内容迁移与真实写作评测

## 基线、分工与授权

- cwd D:/gongwen，main，HEAD 74e9133；开始时干净。此前 fe20ebe 已完成 4C 与直接写入/撤销，后续 74e9133 是独立预览修复，不可覆盖。快照 tmp/pi-implementation/phase-4d-baseline/。
- 用户要求继续原分工：本机 Pi 实现，主代理写 TASK、审查真实 diff、修正并验收。固定 opencode-go/deepseek-v4.1-flash，thinking high，不创建子代理，不提交/push/tag/发布/安装/改版本。
- 持续授权见 pi-execution-authorization.md：同一 opencode.ai 目的地可以处理项目源码、提示和自造测试数据，无需重复询问。不得读取真实稿件、用户配置、认证/密钥、.env、model_api_test.md.txt 或真实 Pi 运行工作区。Pi 自身处理认证，不输出凭据。
- 主代理拥有本 TASK、计划、最终 phase-4d-review.md；worker 写 phase-4d-report.md 后立即停止。主代理另负责真实模型评测的凭据注入与执行，worker 仅用隔离夹具，不访问真实业务模型。
- 必读 AGENTS.md、docs/ai-architecture.md、docs/security-hardening.md、docs/pi-writing-product-design.md、phase-4c-review.md、pi-direct-write-review.md。

## 目标：写作能力落入标准技能，不复活旧 flow

### D1 技能迁移

1. 盘点 assets/agent-skills/ 中全部 15 份旧技能，输出逐项映射：迁到哪份 Pi 主技能/参考文件、保留的业务判据、未覆盖能力及原因。不照搬 flow/params/旧变量/固定出题或人工确认步骤，不假称完全兼容旧行为。
2. 现有五份主技能保留：gongwen-draft/revise/review/imitate/research。新增 gongwen-reply（复函）、gongwen-extract（要点提炼）、gongwen-policy-check（政策依据核对），总共八份。
3. 修改技能把润色、精简、语气、规范化写成按任务选择的 references，不做四个并列主技能。材料成文归起草的参考文件；研究报告吸收旧 policy-report 的论证/出处要求；事实核对归审核，政策依据核对保留专门技能。自由任务由自动路由自然处理，不另建旧 DSL。
4. 仿写补“分析样稿风格 / 学习表达”的参考规则，可报告风格特征，也可应用风格改写；**不声称已把风格学习结果保存进风格库**。本阶段只读已有人维护的风格档案，不新增 AI 自动写库、表单采纳链或通用附件协议。这两项差距写清楚后续收口。
5. 保留有价值的细则：复函主送认来函落款而不是来函抬头；答复态度不能替用户表态；政策核对找不到原文要说明，不能凭空说已核实；精简不能删任务/责任/时限；材料冲突不悄悄二选一；样稿只供写法。
6. 每份技能按需读取共同 gongwen-markdown 语法、材料角色、选区、已填要素。只有确实阻止写作的方向问题才 ask_user；已给的信息不重复问，授权要素不建议/不出题。不得重新引入“先确认清单才能生成”“事实勾选才能进入正文”。
7. 明确交付分流：只审核/提炼/风格分析 finish_report；用户要求生成或修改正文才 submit_document 完整 Markdown。可以中间多文件，最后仍只交一个完整正文。check_document 优先传 path，不能重复全文耗尽输出。研究报告使用应用支持的引用语法，不捏造参考文献。

### D2 内置完整包与实际加载

1. 当前 builtin_package 只编进 SKILL.md；本轮 references 必须真正编入二进制并由新建工作区、复制为用户副本、导出携带，不能只在源码目录存在。
2. 可用明确的编译资源表或扩展现有构建辅助；加入磁盘资源与注册表一致性测试。每个入口/相对引用有效，文件路径/体积/命名与既有包安全规则不变。公共 gongwen-markdown 仍只读且不可覆盖。
3. 不再写死五份；更新标签与相关测试，保留目录缓存、禁用/覆盖、坏副本不回退、快照隔离和恢复原快照的判据。用户同名覆盖仍优先，不能自动覆盖用户技能更新。

### D3 必要的只读风格工具

1. 在现有 Rust/host 工具桥薄接 style_list 与 style_read（可起同义清晰名字，但两侧一致）。复用 ai_services::style::StyleBook/render，工具不调用第二个模型，不写风格库、不记使用次数、不执行 shell。
2. 当前文种有效且启用的风格可列出，按 id 读取；未知/停用/不适用应给结构化错误，不悄悄换风格。列表数量、查询、id、返回文本都有限额。测试使用显式注入的假数据或隔离配置，不读真实库。
3. 风格内容和范例都是 style_sample，不登记为新事实来源；不能让样稿里的单位/日期/数字通过 fact_sources。工具结果清楚标明这一点，提示词/技能选择真实存在的工具。
4. doc_elements + vocab_search 已满足本阶段表单只读需要；描述类建议卡接入缺口写清，不临时扩大自动写表单权限。

### D4 可重复业务评测（实现脚本与自造案例，主代理跑真实模型）

1. 新增 tests/fixtures/pi-writing-eval/（自造材料与 cases.json、README）和 scripts/pi-writing-eval.mjs。至少覆盖：材料起草、精简/选区修改、复函逐项答复与未知态度、材料冲突、政策核对找不到原文、只提炼不改稿、研究报告出处、风格应用不搬事实。
2. 脚本启动**实际 pi-host/src/main.mjs** 和真实标准技能/完整语法包、工作区与回执；不是让外部模型直接回复一段正文冒充 Pi host。不修改生产编排来配合测试。
3. 提供 --case、--out（默认 tmp/pi-writing-eval/<run>）、--model-config（主代理提供的临时 JSON，或对应环境变量）。worker 不读取真实 config 或凭据；不得默认读取 .pi/auth/.env/真实应用配置或外部测试密钥文件。报告不可回显 API key，不保存带钥配置或 init 完整请求；仅保存脱敏模型标识、任务、工具轨迹、真实产物/报告与判定。
4. 案例的知识库/稿件/词库/风格均可用自造领域工具桥响应（与生产 schema 一致），明确标注“资料源是夹具，Pi host/模型是真实执行”，不冒充内网知识库联机。ask_user 只能返回夹具明确给定的答案，不能脚本偷偷替模型编新事实。
5. 断言明确区分技术交付（submitted/report/completed、进程正常、哈希、快照）与业务判据（保留指定事实/不搬样稿事实/选区外不变/只审不改/说明未找到原文）。不只看 exit 0，不依赖另一个大模型评审，不为过测清洗纠正模型正文。人工质量判断单列，不能用关键词通过率冒充写作质量总分。
6. 失败保留完整可诊断的自造产物，不写死标准答案、不强制模型调用不必要的固定工具顺序。合理超时/取消、输出上限、凭据脱敏与路径限制。真实模型失败如实记，不通过调宽断言掩盖。
7. 给脚本的判据/CLI 做离线单测，支持仅校验已有结果或自测，不要求 CI 连接真实模型。不要加依赖。

## 范围

允许 assets/pi-skills/**；src/pi_agent/skills/**（注册/内置包与相关测试）；src/pi_agent/tools.rs（必要可拆 tools/ 模块，保留窄 facade）；pi-host/src/{bridge,prompt}.mjs 与对应测试；必要 src/pi_agent/process.rs 仅新增工具名测试/适配，不动生命周期/预算。
允许新增 scripts/pi-writing-eval.mjs 及小型 eval 辅助模块/单测、tests/fixtures/pi-writing-eval/**；如需要 Rust ignored 验收测试可在 pi_agent 下新增窄 eval 模块并登记。
允许 docs/pi-skill-migration.md（映射与缺口）、docs/pi-writing-eval.md（复现与指标定义），docs/pi-runtime.md 的技能/工具说明。
其它文件先报告再改；尤其不改表单/正文交付/撤销、模型设置、Cargo 依赖/版本、旧 assets/agent-skills 内容、打包脚本、Typst/预览/IME/导出/GWX，不执行 4E 新 API 工具与 4F 删除。

主审范围补记：允许主代理将 `src/ai_panel/ui_tests.rs` 的旧入口排除判据从子串匹配改为精确文本匹配，
避免新技能的自然语言说明含“润色”时误报旧入口复活；不改侧栏生产行为。

## 验证与停止

- cargo fmt --all -- --check、cargo check --locked --all-targets、cargo clippy --locked --all-targets -- -D warnings；相关技能包/工具/快照 Rust 测试，host 工具桥测试和 eval 离线判据测试。
- 不附加 -A 或源码 allow 掩盖 warning；不重复已通过检查。编译器环境硬链接告警与业务警告分开记录。
- 不跑 release/打包/安装/提交。主代理做最终全量回归和真实模型业务评测。
- 完成 phase-4d-report.md，列实际文件、命令结果、失败/限制和未覆盖能力后立即停止写入交接。不要在报告后继续重构。
