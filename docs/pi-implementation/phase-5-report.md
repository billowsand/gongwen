# Pi 统一写作第五阶段实现报告

- 任务ID：20261103-pi-phase5
- 基线：`main` / `3d56f73b604b3cd6a033da6f2d75688a6c612bfb`
- 状态：5A–5C 自动化闭环已实现并通过本地自动化测试；5D 真实模型评测与离线安装包由主代理执行；5E 只做到入口统一与旧任务显式复制，旧调度代码按验收门槛保留。

## 一、实际改动

### 1. host 内核（`pi-host/src/`）

新增：

- `budget.mjs`：保守 token 估算；实际窗口与工作预算分开；按任务规模、材料量、报告型任务与
  未验证能力选常规 / 分阶段；预算不足明确失败；给出单次工具结果与材料分块预算。
- `progress.mjs`：运行目录（工作区之外）`progress.json` 阶段成果登记。记录输入身份哈希、
  文件哈希与字节数、可回查原文位置、依赖与阶段；幂等登记；文件被改 / 来源被换 / 角色变化 /
  依赖失效会传播失效；恢复前核对 run / request / 正文 / 要求 / 材料 / 选区身份。
- `phases.mjs`：固定薄宿主阶段计划（提炼 → 提纲 → 逐节 → 合稿 → 交付；报告/提炼裁剪为
  提炼 → 清单交付；局部修改只交付）、章节展开、覆盖率与依赖检查、每步短指令。
- `read.mjs`：有界分页 `read`，按工作预算限制单次字符数，返回「已读行范围 / 未读范围 /
  offset=下一段」，路径仍走工作区守卫。
- `test/phases.test.mjs`：真实 Durable + faux 模型的完整阶段闭环、窗口路由、恢复跳过已登记
  单元、成果变更失效、身份不一致拒绝、半稿提前提交拒绝、有界读取。

修改：

- `agent.mjs`：统一策略事件；同一 Harness 内分阶段调度（每阶段最多两次纠正，无成果即暂停）；
  最终仍必须 `submit_document` / `finish_report`；中间阶段拒绝提交；报告任务交付前核对材料
  覆盖；常规未交成品且带材料时有一次有界转换；本机免密模型在 provider 内部用占位密钥；
  工具额度按阶段有上限上调；阶段进度与已读进度上报。
- `protocol.mjs`：协议 3 / host 0.3.0；可选 `strategy`（preference + capability）。
- `prompt.mjs`：系统提示加入分阶段契约与 `record_progress`，不改变权限红线。
- `bridge.mjs`：工具额度可按阶段上调，仍封顶。
- `test/openai-e2e.test.mjs`：协议版本同步。
- `package.json` / `package-lock.json`：host 0.3.0（仅版本同步，未新增依赖）。

### 2. Rust 侧（`src/pi_agent/`、`src/ai_panel/`、`src/lmstudio*`、`src/draft_page/ribbon.rs`）

- `pi_agent/strategy.rs`（新）：`auto` / `phased` 策略倾向。
- `pi_agent/protocol.rs`：init `strategy`（可选，旧 host 忽略）、`strategy` 事件（实际窗口 /
  工作预算 / 材料预算 / 原因 / 是否已验证）；协议 3、host 0.3.0。
- `pi_agent/process.rs`：发送策略倾向与未验证能力；解析策略事件，转成侧栏步骤与阶段提示。
- `pi_agent/tools.rs`：`kb_read` / `ms_read` 支持 `offset`，返回 `total_chars` /
  `next_offset` / `unread_chars` / `truncated`，单次上限 24k 字符，不再静默截断当全文。
- `lmstudio.rs` / `lmstudio/context.rs`：新增 LM Studio 原生 `/api/v0/models`
  `loaded_context_length` 解析与短超时探测；窗口来源增加「LM Studio 已加载窗口」；
  服务实际窗口小于手动值时以服务为准，手动值更小时保留更保守值；未知窗口仍回退。
- `ai_panel.rs`：Composer 增加策略倾向、逐篇材料用途（与材料同下标、旧会话回退默认值）、
  逐篇 `@` 引用用途（按「来源 + 编号」存，同名不同来源不串用）。
- `ai_panel/composer_ui.rs`：策略选项、逐篇材料 / 引用用途菜单；粘贴、删除、发送时同步角色。
- `ai_panel/pi_job.rs`：发送策略与逐篇角色；窗口探测放在后台准备线程，短超时、只问已配置地址；
  新增「复制旧任务为新 Pi 任务」（只搬要求、材料、引用与已确认信息，旧选区不迁移）。
- `ai_panel/ui.rs`：侧栏改名「智能写作」；旧轮次显示历史兼容说明与「复制为新 Pi 任务」。
- `draft_page/ribbon.rs`：入口改名「智能写作」，不再引导旧流程新建任务。
- `src/pi_agent/skills/mod.rs`：登记新参考文件。

### 3. 技能与评测

- `assets/pi-skills/gongwen-draft/references/phased-writing.md`（新）：公共分阶段参考，含
  Qwen 基线暴露的「重复主送、手写编号、忽略字数」硬规则。
- 八份 SKILL.md：分阶段时按宿主提示与 `record_progress` 做；报告类技能明确最后只交清单；
  起草技能补主送不重复、标题不手写编号、字数上限是硬约束。
- `scripts/pi-writing-eval.mjs`：用例支持显式 `strategy: "phased"`；数字 / 日期紧邻空白做
  等价比对（「10 月 8 日」=「10月8日」），字数与选区仍按原文计数；读取并判定
  `progress.json` 的阶段登记；新增分阶段判据与自测。
- `tests/fixtures/pi-writing-eval/cases.json` + `cases/phased-multi-source/`（新）：自造
  两份事实材料的分阶段长稿案例，强制提炼 / 提纲 / 章节 / 合稿登记。
- `scripts/pi-writing-eval.test.mjs`：协议版本一致、分阶段案例、数字空格等价、字数不放宽、
  分阶段成果登记判据。
- `scripts/pi-offline-acceptance.mjs`：在现有常规 + 回执重放验收后，新增一份分阶段工作区，
  逐阶段登记后交付，并校验 `progress.json` 单元齐全。
- `packaging/pi-runtime.json`：host 0.3.0 / 协议 3（与代码同步；未执行组装）。

### 4. 文档

- `docs/pi-runtime.md`：协议 3 的策略、阶段登记、失效与恢复、分页读取、旧任务复制说明。
- `docs/pi-skill-migration.md`：登记 `phased-writing.md`。
- 本报告。

## 二、实际执行的命令与结果

全部在本机 Windows x64 执行；host 测试按要求把 `TEMP` / `TMP` 指到 `D:/gongwen/tmp`。

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo check --all-targets` | 通过，无警告（报告收尾时再跑一次仍通过） |
| `cargo clippy --locked --all-targets -- -D warnings` | 通过 |
| `cargo test --locked --all-targets` | 2816 passed / 0 failed / 86 ignored |
| `node --test`（`pi-host`，TEMP/TMP=D:/gongwen/tmp） | 71 tests，70 passed，0 failed，1 skipped（OpenAI e2e 需真实端点） |
| `node --test scripts/pi-writing-eval.test.mjs` | 18 passed / 0 failed |
| `node scripts/pi-writing-eval.mjs --selftest` | 通过（未连模型） |
| `node --check scripts/pi-writing-eval.mjs` / `pi-writing-eval.test.mjs` / `pi-offline-acceptance.mjs` | 三个脚本语法检查通过 |

说明：`cargo test` 之前出现过一次 `pi_agent::skills::snapshot::tests::selected_disabled_or_missing_skill_fails_instead_of_falling_back_to_auto` 的全局目录顺序相关失败；单独运行与随后完整重跑均通过，未改动该测试。host 的 recovery 测试在默认 C 盘临时目录下会因跨盘相对脚本路径失败，这是已知环境问题；按指示改用 `D:/gongwen/tmp` 后全套通过，未削弱测试。

## 三、未完成 / 未验收

1. **5D 真实模型评测未跑**：按任务约束没有直接调用真实模型，也没有真实 Qwen 结果。
   `phased-multi-source` 只完成夹具与判据，需要主代理用本机 LM Studio 跑
   `node scripts/pi-writing-eval.mjs --case phased-multi-source`（以及原有用例）确认小模型
   能否按提示完成登记。真实窗口探测只做了纯函数与来源回退测试，未连真实 LM Studio。
2. **离线运行时组装与验收未执行**：只更新了 `packaging/pi-runtime.json` 与验收脚本；
   `scripts/stage-pi-runtime.ps1` / `pi-offline-acceptance.mjs` 需要组装后的 `runtime/pi`
   才能真跑。分阶段验收路径尚未在真实组装产物上执行。
3. **5E 旧编排未删除**：新入口已统一到「智能写作」，旧轮次保留只读历史与显式复制；旧
   `ai_panel::skill_job` / 旧检查点代码仍保留，退役条件（目标本地模型三类任务通过、较强模型
   回归、恢复/撤销/旧任务只读迁移、离线包一致）尚未由主代理真机验收，因此没有删除旧调度，
   也没有把旧检查点转换成 Pi 检查点。
4. **GUI 真机未验收**：策略原因、阶段进度、逐篇用途菜单、「复制为新 Pi 任务」只做了
   编译与单测 / 渲染断言，没有真实窗口截图目视。
5. **分阶段提示词仍需真实模型校准**：`record_progress` 的单位编号、引文摘录、依赖 `uses`
   对 9B 级模型是否一次就能写对，只能由真实评测确认；失败时当前实现是每阶段两次纠正后暂停，
   不会退回半稿，也不会自动放宽判据。
6. **逐篇材料用途只在当前会话内**：旧会话没有逐篇记录时回退到原默认用途（事实材料），
   没有为旧会话补写历史角色；运行记录里的角色仍以当时 `index.json` 为准。

## 四、风险与注意

- 协议升到 3 / host 0.3.0 后，旧运行时包会被启动校验拒绝，必须与本次 host 一起打包；
  主代理组装前先确认 `packaging/pi-runtime.json`、`pi-host/package.json`、
  `src/pi_agent/protocol.rs` 三处一致。
- 分阶段会多消耗模型调用次数；工具额度只按阶段有限上调（常规 60，分阶段最多 60+16），
  不会无限循环。每阶段最多两次纠正，仍无有效登记即报 `phase_incomplete`。
- 事实疑点仍然只提示；中间成果不写正文，最终写入与整次撤销仍走原有 Pi 交付路径。
- 旧流程的选区、提案和检查点不迁移；「复制为新 Pi 任务」只搬要求、材料、引用与已确认信息，
  用户需要在发送前重新检查正文基线。

## 五、交付状态

本轮实现到此停止，未提交、未打标签、未安装、未组装运行时。
后续由主代理做独立 review、真实 Qwen 评测、组装 `runtime/pi` 与离线验收、旧入口收口和开发包。
复核时建议先看 `pi-host/src/{budget,progress,phases,agent}.mjs` 与 `src/pi_agent/process.rs`、
`src/ai_panel/pi_job.rs` 的实际差异，再按 `docs/pi-runtime.md` 第六节跑常规与分阶段两条链。
