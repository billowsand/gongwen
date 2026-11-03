# 第 6ab 期实现与验证记录

> 本文是 worker 交付时的历史报告，版本0.3.1与日期/电话语料已被主审后续修正。最终代码、0.3.2/判据3、地点语料和真实模型验收以 phase-6ab-review.md 为准；以下保留原始记录，不当作最终验收结果。

日期：2026-11-03。实现未提交、未推送、未安装；worker 没有读取真实配置、密钥、稿件或用户运行工作区，也没有调用真实模型。真实 Qwen 与冻结基线对照留给主代理在代码冻结后运行。

## 实现

- Pi 知识库搜索现在保留结构化 `warnings`，并分别报告 `zero_hits`。降级警告仍随 JSON 工具结果交给模型；Pi Rust 运行记录也把检索降级提示显示为 `kb_search` 工具事件。保留分页、证据登记与硬预算。
- 新增合成能力校准：管理页后台动作测实际窗口并允许停止；使用 `with_all_sources` 明确注入临时 SQLite 知识 / 稿件库和空风格源。每种策略重复两次，总计四轮，单轮 120 秒、总计 8 分钟。知识库经过生产 `KnowledgeStore`、`rag::retrieve`、Pi 工具封装；稿件库经过生产 `SqliteManuscripts`。loopback embedding 端点故意不可达，因此仅检查 SQLite/FTS + 关键词降级，不宣称向量 / rerank 通过。
- 单独证据文件保留自造任务、材料、KB/MS事实、每轮真实策略、机器判据、耗时和成品，文件名为 SHA-256；profile 只保存摘要和证据哈希。证据和 profile 分开原子保存，各自检查体积；身份包含规范化端点、模型、实际窗口、思考档位、host/判据版本和技能快照摘要，不保存凭据。恢复使用 `PiRunRecord` 里的能力快照；新 Init 只采用身份匹配且证据哈希可校验的 profile。短测的 `verifiedWorkBudget` 恒为空；策略偏好只在两组完整、工具覆盖与技术交付均有效且 auto=regular / phased=phased 的同题对照支持时形成。
- 新增显式 ignored 本机模型入口：`pi_agent::capability::tests::live_qwen_synthetic_sqlite_paired_calibration`。完整命令和环境变量见 `docs/pi-writing-eval.md`。本 worker 未运行该入口或真实模型。
- Pi host 改为 0.3.1（Pi 依赖仍锁定 1.4.0）。标题校验现在区分年份标题与真实序号；宿主标题判据、Rust 导出器、Python checker 与 `skill_pack` 反例同步。分阶段 `record_progress` 同一单元连续失败三次或全阶段八次后，由 host 取消当前 Harness 并返回可恢复错误，保留工作文件；前两次仍可按错误提示修正。摘录失败提示只显示本单元允许范围内最多 220 字原文并强调数字 / 日期 / 空格逐字复制，非提炼阶段提示无需重复填 `sources`，没有放宽校验。
- 研究技能补充摘要与背景不能先断言本地短板、之后再改称未知；复函指引与原有来函主送、未知事项占位契约一并保留。

## 验证

所有 shell 验证设置 `TEMP` 和 `TMP` 为 `D:\gongwen\tmp`。

| 命令 / 检查 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo check --locked --all-targets` | 通过 |
| `cargo clippy --locked --all-targets -- -D warnings` | 通过，退出码 0；Windows 增量缓存报告 hard-link 不可用并回退复制 |
| `cargo test --locked capability -- --nocapture` | 8 通过，1 个显式 live 模型测试忽略 |
| `cargo test --locked pi_tool_context_uses_real_temporary_sqlite_knowledge_chain -- --nocapture` | 通过；真实临时 SQLite/FTS 知识库、真实临时稿件库搜索 / 读取、分页、证据登记与 embedding 降级均覆盖 |
| `cargo test --locked heading_number_cleanup_preserves_year_titles_and_clears_short_manual_prefixes -- --nocapture` | 通过；年份标题保留，数字和中文真实序号剥除 |
| `cargo test --locked restarting_uses_the_original_request_but_the_current_kind_and_date -- --nocapture` | 通过；覆盖恢复策略与能力快照沿用 |
| `cargo test --locked skill_pack -- --nocapture` | 10 通过，1 个依赖模型产物的 ignored 测试未运行 |
| `node --check pi-host/src/agent.mjs` | 通过 |
| `node --test pi-host/test/agent.test.mjs` | 14/14 通过，含真实 Durable Harness 连续错误三次后终止并留存 `work/` 的用例；**这是 agent.test 单文件，不是 pi-host 全目录测试套件** |
| `node --test scripts/pi-writing-eval.test.mjs`、`node scripts/pi-writing-eval.mjs --selftest` | 20/20；自测通过 |
| `python skills/gongwen-markdown/scripts/check_gongwen_md.py --selftest` | 19/19 通过 |

`cargo test --locked --all-targets`、pi-host 全目录 `node --test`、`cargo test --locked model_outputs_survive -- --ignored` 和真实模型没有由 worker 执行。主代理后续应报告 pi-host 全目录测试的实际数字；不得把本报告里的 14/14 当成全目录通过。首次 `cargo test --locked skill_pack` 暴露旧的年份标题预期；在获批同步测试判据后重跑为上表结果。

## 未验证边界

- 本次短通知合成任务不能证明 96K、16128 或 262144 token 工作预算；profile 不填 `verifiedWorkBudget`，也不把容量窗口探测视为可靠工作预算。
- 本 worker 未执行真实 Qwen，当前实现还没有 live 选区约束案例。主代理应先运行文档中的 ignored 入口并检查临时库独有日期 / 电话确实经搜索与读取进入正文，再对冻结用例做同题 auto / phased 与选区评测；真实向量 / rerank 仍未验收。
- 机器判据、关键词检查与 host 技术成功均不代表人工写作质量通过。既有真实模型基线中人工指出的研究报告无据短板断言仍需人工复核；程序通过不能覆盖该意见。
- 没有跑 UI 截图 / 人工 GUI 验收；后台任务、窗口变化前后探测、当前模型身份检查和 Rust Init / 恢复快照有代码测试与编译检查，真实安装版交互由主代理另验。
