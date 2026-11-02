# TASK：第四阶段 B——提取共享 AI 服务并拆开 Pi 模型参数

当前状态：提示词与依赖盘点已完成。此前自动审批要求明确的目的地授权，用户现已明确允许将
本阶段应用源码、任务提示和自造测试数据发送到 opencode.ai 的 DeepSeek V4.1 Flash，用于实现 4B。
真实稿件、用户运行工作区和配置密钥仍不在数据范围内。已核对实际模型为
`opencode-go/deepseek-v4.1-flash`；worker 已停止，主代理完成修正与验收，见 `phase-4b-review.md`。

## 基线与分工

- 仓库与 worker cwd：`D:/gongwen`，分支 main，HEAD `93efcc1`。本轮开始时干净。
- staged/unstaged/status 快照：`tmp/pi-implementation/phase-4b-baseline/`；任务文件由主代理新增，属于主代理。
- 用户指定本机 Pi 实现；主代理写提示词、审查真实 diff、修正、验收。本轮无子代理，不切分支、不提交、push、tag、发布或安装。使用现有配置中的 `opencode-go/deepseek-v4.1-flash`；由启动命令固定，不自己改提供商或模型。
- 主代理拥有本 TASK、`docs/pi-implementation-plan.md` 和最终 review；worker 拥有允许范围内实现与 `docs/pi-implementation/phase-4b-report.md`，交付后立即停止写入。
- 必读 AGENTS.md、`docs/ai-architecture.md` 红线、`docs/security-hardening.md`、`docs/pi-writing-product-design.md` 第七节、`docs/pi-implementation/pi-budget-fix.md`。旧会话存储不变，不更改 GWX/导出。

## 数据范围

只读写本阶段应用源码、自造测试数据和上列设计文档；这是 GPL 公开项目，主代理已通过 GitHub 元数据确认 `billowsand/gongwen` 为 PUBLIC。禁止读取用户真实稿件、运行工作区、config.json、model_api_test 文件、auth.json、.env 或任何凭据。模型认证只由 Pi 自身处理，不能打印。禁止向外发送真实业务文稿；不运行真实模型业务评测。网络仅由已固定的 Pi 模型服务执行实现任务，不自行调用其它模型或外部服务。

## 目标

为后续删除旧 DSL 引擎解除真实类型依赖，保留产品行为。新共享层不依赖 Board / Skill / engine::Event / ToolCtx / 算子。Pi、分阶段写作、API 工作台和旧兼容适配器调用同一份实现。

### B1：提取领域服务

新增 `src/ai_services.rs` 与 `src/ai_services/`（可采用以下模块划分，按真实代码适度调整）。登记进 main.rs。

1. `knowledge`：从旧 tools/mod.rs 移出 `KnowledgeSearch`、`RagSearch` 与实现，语义与降级说明不变。
2. `manuscripts`：从旧 tools/store.rs 移出 `ManuscriptDoc`、`ManuscriptSource`、`SqliteManuscripts` 与数据库适配，版本读取/状态过滤/限额不变。旧工具的 JSON/证据包装仍留在旧工具适配器中。
3. `vocabulary`：提取词库查询、候选匹配、职能建议、所属单位等纯能力（包括私有辅助函数的必要依赖）。不将 Tool/ToolCtx/旧工具注册代码搬进共享层，不复制一套词库匹配逻辑。
4. `text`：提取 `tools::short` 及确被多个产品使用的 `board::value_to_text`、`gaps::sentence_spans` 等纯函数；保留 Unicode、标点、字数边界行为。无需为了几个函数移动整个 Board/gaps。
   句切实际依赖 evidence::CITATION；可将引用正则/必要纯引用函数一并移到共享 citations 模块，由旧 evidence 单向引用。不得复制正则或让共享 text 反向导入旧 agent::evidence。
5. Pi 与产品层的使用方改为导入真实共享模块，不能只在新模块 re-export 旧 agent 实现。旧 agent 可临时单向 re-export 共享类型来兼容旧 tests/engine，不能反向依赖或重复实现。

### B2：提取模型、预算与风格基础

将通用 `agent/backend.rs`（包括 backend/usage.rs）、`budget.rs`、`toolcall.rs`、`style.rs` 的可独立实现移到 `ai_services/`。它们服务分阶段写作、API 工作台等，不属于待删除的编排。先审查真实依赖；若混有旧流程专有代码，就分离适配器，不把旧编排一起搬走。

迁移产品生产代码引用至共享层；旧 agent 的薄 re-export 仅作为本阶段兼容，报告剩余依赖与下阶段删除点。不改提示词、模型协议、温度/输出规则或风格档案格式。只做必要路径调整，不因移动重写算法。

已知直接依赖：style.rs 使用 board::RefSource、tools::ASSIST_SYSTEM。可将 Reference/RefSource 这类共享引用 DTO 移至 ai_services::references，把辅助模型的通用常量放到共享模型模块；旧 Board/工具单向引用或 re-export。序列化形态、文案和行为保持不变，不为此保留对旧引擎的反向依赖。

已知存盘依赖：style::StyleBook 使用 agent::api::{read_json,write_json}。主代理批准把这两个函数及 store_path 提取到 ai_services::persistence（或同等共享模块），旧 api 单向 re-export；必须保留 storage::atomic_write、错误语义与文件权限，不复制实现。`src/agent/api.rs` 为本项最小提取允许修改，其它 API 业务不动。

### B3：Pi 思考配置彻底独立

目前 Pi 窗口与输出已经自动管理，但 reasoning_effort 仍来自旧文字起草。新增最小独立 Pi 设置，只承载思考深度，不加输出长度/窗口/重试次数等用户旋钮。

- 配置可采用可迁移的 `pi_options: Option<PiOptions>`，`PiOptions` 有自己的默认思考深度。
- 旧配置已有 Pi 模型但没有新设置时，一次性复制原来实际使用的思考深度；旧配置没有 Pi 模型时使用新默认。持久化后不再随起草设置变化。不读取真实本机配置验证，用自造 JSON。
- 兼容 AppConfig 的 serde/default 与现有启动迁移入口：可新增独立迁移方法并由 storage::load_config 接入，不混淆成提供商迁移，不新增全局配置框架。
- AI 管理页 Pi 模型区显示自己的思考深度，明确默认含义；删掉“思考深度沿用上方”的说明。使用现有 UI 模式，正常用户不需要理解底层参数。
- `pi_chat` 不再隐式继承旧 reasoning_effort；保持自动窗口/输出规则，保持小模型配置不变、每轮15分钟内部上限和等待回答暂停语义不变。

## 文件范围

允许新增：`src/ai_services.rs`、`src/ai_services/**`、阶段 worker 报告。

允许修改：`src/main.rs`、`src/agent.rs`；`src/agent/{backend.rs,backend/**,budget.rs,toolcall.rs,style.rs,tools/**,board.rs,gaps.rs}` 的提取/兼容适配；`src/models/{config.rs,provider.rs}`、`src/models.rs`、`src/storage.rs`、`src/app/ai_manage.rs` 的最小 Pi 设置和迁移。

使用方路径更新可涉及 `src/pi_agent/**`、`src/ai_panel/**`、`src/agent/**`、`src/draft_page/**`、`src/app/**`、`src/task_ui/**`、`src/element_fields.rs`，但这些范围内只允许导入路径/类型引用更新，以及本 TASK 明确列出的 Pi 配置绑定。不要借机改其它产品行为。

主代理已检查并批准 `src/prompt.rs` 的 ignored 真实模型测试改用共享 backend 导入，仅允许这一处路径替换，不改提示词或测试内容。该范围在全局路径更新时被提前触及，worker 应在报告中如实注明。

需要范围外文件或用户可见契约选择，先报告并暂停该部分，等主代理批准，不擅自扩展。

## 非目标与红线

- 不执行 4C 技能管理、4D 技能内容迁移、4E 新 API 工具、4F 引擎删除。
- 不删除整个 agent/，不停止旧会话恢复，不迁移会话格式，不丢用户数据。
- 不加异步运行时、通用服务容器、依赖注入框架或新 crate；共享层是普通 Rust 模块与窄接口。
- 正式稿仍只能人工合并；AI 不写授权要素；来源闸门、词表校验和接受路径保持。
- 不改 pi-host、打包、版本、Typst、IME、导出、Cargo 依赖或许可证。

## 验收与停止

- `cargo fmt --all -- --check`、`cargo check --locked --all-targets`、`cargo clippy --locked --all-targets -- -D warnings`。
- 运行迁移后相关领域、分阶段写作、API 工作台、Pi 配置测试；纯移动优先复用原测试，不写镜像实现测试。
- 新增有意义的 Pi 配置 JSON 向后兼容/一次迁移/起草与Pi互不影响的测试。
- 静态核对：ai_services 生产代码无 crate::agent 的旧类型依赖；Pi 生产代码不再借 agent::tools/agent::backend 使用共享能力。
- 主代理负责最终全量、review、必要修正；worker 不跑 release/打包/真实外部模型测试，避免重复消耗。
- 完成 `phase-4b-report.md`，列实际路径、移动/适配区别、命令结果、范围外请求、剩余耦合与风险。然后立即停止工具调用并给最终交付，等待主代理审查。
