# TASK：实施 Pi 统一写作第五阶段

- 任务ID：20261103-pi-phase5
- 仓库与实际cwd：`D:\gongwen`
- 基线：`main` / `3d56f73b604b3cd6a033da6f2d75688a6c612bfb`
- 已有dirty：`docs/pi-implementation-plan.md`、`docs/pi-writing-product-design.md` 修改；`docs/pi-unified-writing-design.md`、`docs/pi-implementation/qwen-local-baseline.md` 新增。均为本次产品设计，保留，不覆盖历史内容。
- 基线快照：`tmp/pi-implementation/phase-5-baseline/`。
- 协作：用户明确选择本机Pi实现，主代理规划/审查/修正/提交/打包；不派生其他worker。
- 实际模型：启动后由RPC核对，指定 `opencode-go/deepseek-v4.1-flash`，thinking high。持续外发授权见 `pi-execution-authorization.md`。
- 文件归属：worker独占下列实现文件；主代理只做独立基线/测试准备，不并发修改实现。原设计正文、主审报告、提交与打包由主代理负责。
- 提交策略：worker不提交、不切分支、不push、不安装。用户已授权最终统一提交与生成开发安装包，由主代理完成。

## 目标

先读 AGENTS.md、docs/ai-architecture.md、docs/pi-unified-writing-design.md、docs/pi-implementation/qwen-local-baseline.md、docs/pi-runtime.md。

实现5A到5C与5E能安全提前完成的部分，形成一个真实可用的常规/分阶段共用Pi执行内核，不能只做路由提示。5D真实模型评测和离线打包由主代理完成；你必须先完成自动化。旧流程退役受设计验收门槛约束，不得为了声称5E完成破坏历史。

## 允许修改范围

- `pi-host/src/`、`pi-host/test/`：预算、阶段状态、单Harness调度、受限读取、恢复、占位鉴权兼容；可增加小模块，避免agent.mjs巨型化。
- `pi-host/package.json`、`pi-host/package-lock.json`：只允许同步host自身版本，固定Pi依赖不变。
- `src/pi_agent/`、`src/pi_agent.rs`：任务策略、元数据、协议/进度、工具分段读取、恢复及必要导出；不改变正文授权。
- `src/ai_panel/`、`src/ai_panel.rs`：逐篇材料用途、策略选项、进度与来源展示、任务参数。
- `src/ai_services/`、`src/lmstudio/`、`src/lmstudio.rs`：必要的模型实际窗口探测/共享服务；不引入在线依赖。
- `src/app/ai_manage.rs`、`src/app/agent_settings.rs`、`src/app/pi_jobs.rs`、`src/app/pi_settings.rs`（若存在）、`src/draft_page/ribbon.rs`、`src/draft_page/writing/`：统一新任务入口、历史兼容及显式旧任务迁移。
- `assets/pi-skills/`：公共分阶段参考+现有技能的按需引用；新增文件必须登记内置FILES，完整复制/导出/快照。
- `scripts/pi-writing-eval.mjs`、`scripts/pi-writing-eval.test.mjs`、`tests/fixtures/pi-writing-eval/`：新增明确强制分阶段案例/策略参数及对应回归，修正数字空白的等价匹配，保留真实字数/选区约束与原始结果。不得降判据。
- `packaging/pi-runtime.json`、`scripts/pi-offline-acceptance.mjs`：只限host/协议版本一致及新流程离线验收必需调整，保持固定依赖与禁止公网检查；主代理最后实际组装运行时。
- `docs/pi-implementation/phase-5-report.md`：实现报告；`docs/pi-runtime.md`：新协议/恢复说明；`docs/pi-skill-migration.md`：新增标准参考登记。
- `src/models/`、`src/models.rs` 只允许策略默认值/兼容所需最小变更；不改排版或传输单等无关功能。

必要扩展范围先在报告/控制消息中说明并停下。禁止读取真实应用配置、model_api_test.md.txt、.pi认证、真实稿件或真实运行工作区。测试用例只用自造数据。不要直接调用真实模型测试：主代理统一调度本机LM Studio。

## 行为契约

1. 统一新任务智能写作入口，内部自动/倾向分阶段；自动依据真实窗口、任务规模和有限能力元数据，不能硬编码9B或256K。保留实际窗口与工作预算两个字段；不添加输出长度用户设置。LM Studio当前仅`/api/v0/models`返回实际loaded_context_length，标准`/v1/models`只有ID。请求在后台、有限超时，不阻塞GUI，不对任意服务盲目探测无关主机。
2. 本机免密模型不用用户填假密钥；SDK要求鉴权时在本地兼容provider内部处理空值。不更改远端真实密钥/不打印。
3. 分阶段最小闭环必须真实调度：按材料块提炼→提纲→逐节写作→合稿检查→显式提交。报告/提取/选区任务按需求裁剪。使用同一Durable Harness；中间阶段只接受已验证成果登记，不报not_submitted；最终仍必须submit_document/finish_report。阶段定义是固定薄宿主，禁止重建通用DSL/旧engine。
4. 宿主登记状态置工作区之外的run目录，记录输入身份/哈希、成果依赖和哈希、阶段、重试；模型不可编辑它。只信任文件实际存在且路径UTF8大小合法；事实摘录必须可回读原文定位，材料角色不让模型伪造。幂等登记+恢复：崩溃在登记后/提交前不重复已验证单元；文件被改需失效依赖；不可通过先前登记提前提交半稿。不要让模型自己声称全部读完代替覆盖检查。
5. 原文读取与kb/ms全文返回都必须受工具结果预算限制，支持范围/分页+明确未读范围，不能隐式截断当全文。保留现有路径、junction、UTF8、体积、安全检查。提交最终稿是完整读取，不受片段读取影响。
6. 常规→分阶段有界转换，工具无效、压缩失败、无成果、输出截断分别有界处理。60工具额度可以按阶段合理有上限调整，不无限放宽。复用现有停止/恢复。重开恢复需检查任务输入身份以及阶段成果一致，不把新正文混旧任务。
7. 材料逐篇用途支持粘贴/导入/@引用，每篇事实/样稿/结构/来函，删除后无串用，兼容旧会话默认值。对相同标题不同来源/ID避免映射冲突。样稿不入事实证据。
8. UI展示策略原因、实际读材料/阶段进度，直接写正文和整次撤销保持原样；事实疑点只提示。旧任务历史可看/导出并显式复制为新Pi任务；旧checkpoint不直接导成Pi checkpoint。暂存旧运行能力到主代理真实本地模型验收后再决定退役。新增入口集中到Pi，旧入口只标注历史兼容，不再引导新建第二路线。
9. 继承八技能与公文语法，针对Qwen基线的主送重复、手写编号和不遵循字数，在技能/阶段反馈中改善，禁止提交前无条件重写/清洗内容掩盖失败。确定性权限和事实提示红线不放松。

## 测试与Done

- node --test（pi-host目录）及评测selftest；新增真实Durable faux模型集成验证完整阶段、不同窗口路由、登记后崩溃续跑、成果变动失效、半稿提前提交拒绝、真实范围读取与工具错误、常规不退化。
- Rust相关测试：模型窗口来源、材料逐项角色/持久化、协议兼容、引用同名、恢复身份与最终写入撤销；fmt、check、严格clippy（主代理之后全量复核）。
- 不用mirror实现的简单mock代替真实Harness集成；不删除已有测试掩盖失败。
- 生产运行依赖离线固定，不改lock引入无必要包，不用动态npm安装。
- 报告每条实际命令/结果、实际改动文件、未完成及风险，不宣称真实模型或真机GUI已通过。
- 完成全部范围后写phase-5-report并停止工具调用，交付未提交diff等待review。

主代理会补充基于真实Qwen结果的修正、完整回归、旧入口最终收口与开发包。当前先一次交付可运行的5A–5C，不要只写设计或列待办就结束。
