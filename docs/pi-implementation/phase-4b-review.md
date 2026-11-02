# 第四阶段 B 主审与验收

## 基线与分工

- 基线 `93efcc1`，main；阶段开始时工作区干净。快照：忽略的 `tmp/pi-implementation/phase-4b-baseline/`。
- 用户明确允许 Pi 将本阶段应用源码、任务提示和自造测试数据发送至 opencode.ai 的 DeepSeek V4.1 Flash，用于实现 4B；不包含真实稿件、用户工作区或配置密钥。
- 本机 Pi worker 的 RPC 事件已核对实际模型为 `opencode-go/deepseek-v4.1-flash`。
- 主代理编写 `phase-4b.md`、调度与审查修正；worker 实现报告为 `phase-4b-report.md`。
- 本阶段不自动提交、推送、发布或安装；不改变旧会话存储及正式稿人工接受红线。

## 主审关注点

1. 共享实现真正离开旧引擎，生产代码无 Board/Skill/ToolCtx/Event 的反向依赖，不只增加一层别名。
2. 知识库、稿件库版本读取、词库匹配与来源校验语义保持；不复制算法或整套旧工具上下文。
3. 分阶段写作、API 工作台、Pi、风格管理改用同一份基础实现；旧引擎只保留单向兼容导出。
4. Pi 思考设置一次迁移并独立保存；旧配置可读、旧模型参数不变；Pi 窗口/输出继续自动管理。
5. 实际改动清单、范围外文件、已有 dirty、验证结果与未做项逐一对账。

## 实际交付与主审结论

Pi 已完成 B1/B2/B3，RPC 最终 `settled: true, exitCode: 0` 后停止写入，主代理才接管修正。
未改 pi-host、打包、Cargo 依赖/版本、导出或旧会话结构。本轮没有业务模型联机验收；
worker 报告里的「未运行真实模型」仅指业务测试，实现本身按已授权范围调用了 Pi 外部模型。

新增 `ai_services` 共享层：模型调用/用量、输入预算、工具协议、风格、知识库、稿件库、词库、
文本/引用清理、引用 DTO 与 JSON 存盘。旧工具仍负责其 JSON/证据包装，旧引擎暂保留单向兼容导出。
主代理用 Git 基线逐文件对照核心搬移：backend/usage 内容完全一致；backend/budget/toolcall 仅文档、
辅助常量归属调整；style 仅导入、存盘调用路径与测试导入调整，算法/阈值/提示词/存储格式未变。

Pi 新增自己的 `pi_options.reasoning_effort`，旧配置已有 Pi 模型时一次复制原实际深度，
没有 Pi 模型时用服务端默认。持久化后与小模型相互独立，窗口与输出继续自动管理。
界面新增 Pi 自己的思考深度选择，不增加输出/窗口/重试等旋钮。

## Findings 与修正

1. **how：产品内部仍绕旧别名。** worker 改了起草页和工作台外壳，但
   `agent/writing{.rs,/**}`、`agent/api_import{.rs,/**}`、`agent/apidef/**` 和 `agent/api.rs`
   仍借旧 backend/budget/toolcall/text 别名。主代理将这些保留产品的内部引用也改向共享层，
   不更改逻辑；静态复查已无这些旧共享路径。worker 报告「后续可直接删薄壳」的结论在此修正后才适用。
2. **how：多余警告屏蔽。** 删除新加的 `allow(unused_imports)`；RagSearch 仅供旧测试的兼容导出
   改为 `cfg(test)`，其余实际使用的导出无需屏蔽。未添加 dead_code 例外。
3. **how：不必要地重置网关格式。** worker 在 pi_chat 重置 reasoning_format，但
   Pi host 不读取该字段，Rust 知识库回退调用仍可能用它。主代理保留原格式，只独立思考深度；
   新断言验证 ReasoningObject 保持，以免结构迁移改变原有网关请求协议。
4. **范围对账：** `src/prompt.rs` 一处 ignored 测试的 backend 导入被全局替换提前触及，
   主代理检查并批准，仅路径变化；共享 JSON 存盘提取的 api.rs 范围也已提前补记批准。

## 验证

- worker：check/fmt/clippy 通过，全量 2732 通过、85 忽略；配置 48、共享层 30、Pi 相关 157 项通过。
- 主代理最终修正后：`cargo test --locked --all-targets` 2732 通过、0 失败、85 忽略；
  `cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`、diff 检查通过。
  编译器提示增量缓存硬链接失败后改复制，属环境提示，不是代码 lint。
- 基线对照与静态检查：共享层生产代码不依赖旧 agent；Pi 不再借 agent::tools/backend/board 使用共享能力；
  分阶段写作、API 导入和接口工作台内部已改为共享层。版本读取、标准词库、来源清理与存盘权限保留。
- 本地日志在忽略的 `tmp/pi-implementation/phase-4b*`，不作为产品依赖。

## 仍保留与下一步

- `ai_services/style.rs` 测试仍用旧 `agent::testkit::ScriptedModel`，仅 cfg(test)，生产无反向依赖；
  4F 删除测试支持前须迁出。旧 engine/ops/检查点 UI/历史会话恢复本轮不删。
- Pi 的问题适配仍用旧 clarify 的部分纯类型/函数，后续删除旧模块前要提取，不宣称 agent/ 已整体解耦。
- 旧 API 产品类型/存储仍在 agent/api、apidef 等模块，此轮只解开共享模型/辅助函数依赖，不删除接口工作台。
- 下一步 4C 将技能管理切到 Pi 标准 SKILL.md；4D/4E 再补技能、风格/要素与接口工具，4F 才退役旧通用编排。
- 未做 GUI 真机、真实模型业务测试、Release/安装包重打；本次不提交/推送/发布/安装。

本阶段代码验收完成，可作为 4B 检查点审阅。安装目录与现有开发包仍是上一检查点，未冒充包含本轮变化。
