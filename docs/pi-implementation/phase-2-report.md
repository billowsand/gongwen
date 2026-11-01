# 第二阶段交付报告：Pi 智能写作产品化

日期：2026-11-01
实现者：本机 Pi Coding Agent 1.4.0
仓库：`D:\gongwen`，分支 `main`，基线 HEAD `a9ed9eec3015c673d1804a73215deb656176cea3`。
未切分支、未提交、未推送、未改版本号。

## 一、结论

第二阶段按 TASK 打通了 Pi 试用闭环的产品路径：

1. Pi 有独立的 `pi_model` 选择，小模型 `draft_chat` 不受影响；未配置时明确报错，不回落。
2. 随包 5 份标准技能（起草 / 修改 / 审核 / 仿写 / 研究报告），host 给索引、模型按需读；
   没有旧 flow / step 解释器。
3. 粘贴材料与 `@` 引用在后台读成本轮编号材料快照，区分事实材料、来函、写法样稿、
   结构范文；读不到明确失败。
4. 只读领域工具 `kb_search` / `kb_read` / `ms_search` / `ms_read` / `vocab_search` /
   `check_document` / `doc_elements` 走 host→Rust 工具桥，结果带证据编号与真实出处；
   Rust 保留本轮事实台账，交付与 `check_document` 共用。
5. 选区由 Rust 按 UTF-8 边界校验，提交仍交完整 Markdown，选区外逐字比较；旧候选稿
   （排除块之后）可继续修改，最终提案仍对照真实正文。
6. `ask_user` 由模型按需调用，题目复用 `clarify::agent_questions` 拦授权要素，答案按
   `callId` + 题目 ID 绑定；等待时间不计执行超时。
7. 生成 / 修改必须 `submit_document`；审核 / 找资料用 `finish_report`，只进侧栏。

未做跨重启续跑、安装包、默认入口替换、旧框架删除、HTTP 数据接口工具、任意 shell、
要素自动写表单；仍保留独立 Pi 入口，不自动迁移小模型用户。

## 二、实际改动

### 新增

| 路径 | 内容 |
| --- | --- |
| `assets/pi-skills/gongwen-draft/SKILL.md` 等 5 份 | 标准 frontmatter + 中文自然语言步骤；引用现有 `skills/gongwen-markdown/` 语法与文种规则，不含 flow / step / 变量插值 |
| `src/pi_agent/skills.rs` | 技能编译进二进制、frontmatter 校验、安装到工作区只读 `skills/pi-skills/`、索引与选择解析 |
| `src/pi_agent/materials.rs` | 材料角色、编号文件名 + `index.json`、单份 / 总量 / 数量上限、事实角色过滤、样稿不授权 |
| `src/pi_agent/tools.rs` | 窄接口 `PiKnowledge` / `PiManuscripts`、生产适配现有 `RagSearch` / `SqliteManuscripts`、证据台账、7 个只读工具、`doc_elements` 只读表单 |
| `src/ai_panel/pi_ask.rs` | 侧栏提问卡（选项 / 自由填写 / 跳过）、回答提交、Pi 审核清单卡 |
| `pi-host/src/bridge.mjs` | `callId` 工具桥、批次 / 题数 / 调用次数上限、取消清空等待者、领域工具与 `ask_user` 定义 |
| `pi-host/test/bridge.test.mjs` | 错 ID / 迟到回复、结构化失败、取消、暂停计时、上限、材料路径、协议字段 |
| `docs/pi-implementation/phase-2-report.md` | 本报告 |

### 修改（主要）

- `src/models/config.rs`：新增 `AppConfig::pi_model`（旧配置缺省为空）与 `pi_chat()`；
  独立解析，不回落起草模型。
- `src/app/ai_manage.rs`：「模型服务」新增「智能写作（Pi）」模型选择器。
- `config.example.json`：补非秘密 `pi_model` 默认字段。
- `src/pi_agent/protocol.rs`：init 增加技能 / 选区 / 材料目录与元数据；新增
  `tool_request`、`questions`、`report` 事件与 `PiReportItem`。
- `src/pi_agent/workspace.rs`：材料快照、技能安装、request 文件附技能与材料索引。
- `src/pi_agent/process.rs`：工具桥后台执行、错 ID / 未知工具 / 参数超限 / 次数上限、
  提问等待暂停超时、取消后丢弃迟到工具结果、`finish_report` 结果、选区与证据随交付返回、
  `PiBridge` 回答通道。
- `src/pi_agent/delivery.rs`：`DeliveryCheck` 统一上下文；新增选区外不变检查、材料 / 工具
  证据来源、旧候选不作为新事实来源、工作区路径泄漏检查。
- `src/ai_panel/pi_job.rs`：读取 `pi_chat`、锁定选区、`@` 引用与粘贴材料后台读入、角色、
  Pi 技能选择、旧技能明确说明未套用、未采纳候选稿作为基准、提问 / 报告事件回投。
- `src/ai_panel.rs`、`src/ai_panel/composer_ui.rs`、`src/ai_panel/ui.rs`：Pi 技能与材料 /
  引用用途选择、提问卡与清单卡、停止时清理题卡与桥。
- `src/app/jobs.rs`、`src/app/pi_jobs.rs`：`PiOutcome`（正文 / 清单）、提问回投、清单只进
  侧栏、选区违规与报告不装提案的集成测试。
- `src/app/ai_proposal.rs`、`src/app.rs`：只提取 `pi_base`（未采纳候选稿排除块之后）读取
  辅助函数，不改变既有接受保护。
- `pi-host/src/prompt.mjs`、`agent.mjs`、`main.mjs`、`protocol.mjs`、`README.md`：技能索引、
  材料角色、选区说明、工具清单、提问暂停、报告事件、工具结果路由；修正第一阶段提示词中
  与技能文件冲突的表格文案。
- `src/ai_panel/pi_jobs.rs`/测试、`pi-host/test/agent.test.mjs`：新增回归。

未修改 `Cargo.toml`、`Cargo.lock`、根版本、发布脚本；`docs/pi-implementation-plan.md`
是开始前已有的未提交改动，本次未触碰。

## 三、关键边界

- **独立模型**：`pi_model` 为空时 `pi_chat()` 返回「还没有选择智能写作（Pi）模型」；
  `draft_chat()` 与小模型路径完全未改。生成参数沿用现有起草设置（窗口、超时、思考深度），
  输出上限按真实窗口收紧。
- **技能**：5 份技能是 Markdown 指导，不是执行语言；Rust 安装、host 给索引，模型自读全文。
  `submit_document` / `finish_report`、权限与闸门都在程序里。
- **材料**：文件名由程序编号生成，标题 / 来源只进元数据。只有 `fact` 与 `incoming_letter`
  进事实来源；`style_sample` / `structure_sample` 永不进闸门来源。
- **工具**：只读，不写正文、不写表单、无 shell。Rust 校验工具名、参数体积、调用次数、
  `requestId` / `runId` / `callId`；未知工具与坏参数作为结构化失败回模型，不冒充成功。
  RAG 在后台线程执行，取消 / 超时不被检索阻塞；停止后的迟到结果丢弃。
- **证据**：工具实际返回的片段登记为 `E1…`，带来源、版本 / 标记、哈希、片段与角色；
  `check_document` 与最终交付共用同一台账。`vocab_search` 只证明规范名称存在，不产生证据。
- **`doc_elements`**：只读当前表单的描述类字段与标题，授权类字段只列值并标注只读；
  不写表单、不出题、不建议。表单里**用户已填**的描述类值可作为事实来源；词库整项结果与
  授权类值不作为事项事实来源，新增「词库查到人 / 单位不足以授权该人承担任务」回归。
- **选区**：只接受侧栏锁定过的选区；编辑器临时选中而未锁定直接拒绝。提交完整 Markdown，
  程序比较选区前后文本；越界即拒绝提案，不会全篇自动接受。
- **追问**：`ask_user` 最多 3 题 / 2 批；授权要素、文种题由 `clarify::agent_questions`
  拒绝。等待不计执行超时（host 与 Rust 两边停表），超过 30 分钟停止；回答只影响本轮
  Pi 上下文与工作文件，不直接写 `DraftInput` 或正文，不调用旧 `engine::apply_answers`。
- **交付**：正文只走 `submit_document`，审核只走 `finish_report`；两者不能同时交付。
  未提交、未过闸门、哈希不符、取消、崩溃都不会出现提案或清单成功卡。

## 四、命令与结果

在 `D:\gongwen` 实际执行：

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo check --locked --all-targets` | 通过（无警告） |
| `cargo clippy --locked --all-targets -- -D warnings` | 通过 |
| `cargo test --locked pi_` | **121 passed / 0 failed / 2 ignored**；含工具桥往返、未知工具 / 参数超限、取消时慢检索、提问暂停 / 授权题拒绝、报告不装提案、选区 / 词库边界 / 样稿 / 伪造引用 / 旧候选来源等回归 |
| `cargo test --locked ai_panel::ui::tests` | **63 passed / 0 failed / 7 ignored**（现有侧栏 UI 回归未受破坏） |
| `cargo test --locked pi_follow_up` | **1 passed**（未采纳候选稿作为下一轮基准；正文改动后回到真实正文） |
| `cd pi-host; node --test` | **35 passed / 0 failed / 1 skipped**；跳过项仍是本机无权限创建文件符号链接。新增工具桥、检索→写文件→提交、`finish_report`、init 技能 / 选区 / 材料角色测试 |

未执行：全量 Rust 回归（按 TASK 留给主代理）、真实模型联机、GUI 真机、离线安装包 /
Linux 真机、跨重启恢复。上述未跑项目不声称通过。

## 五、已知限制与风险

1. **材料 / 引用角色是一轮一组**：同一轮所有粘贴材料共用一个角色，所有 `@` 引用共用另一个
   角色；同时需要「事实材料 + 写法样稿」时应分成两轮，界面上已有用途标签。
2. **选区仅支持侧栏锁定选区**：编辑器里临时选中会被拒绝并提示先锁定；这是刻意的确定性边界。
3. **工具参数上限 1 MiB**：`check_document` 对超长正文的分段检查需要模型分次传 text；
   最终交付检查仍对完整成品执行。
4. **`doc_elements` 的表单值边界**：用户已填描述类值可作为事实来源，授权类值只读；
   词库查询本身不产生证据（已有回归）。若后续要求「表单值也不能作为事实来源」，需要产品确认。
5. **提问批次计数**：host 把被 Rust 拒绝的提问尝试也计入 2 批；模型若连续问授权要素会更快
   用完批次。属于安全侧偏保守，不是功能回归。
6. **报告 / 提问不跨重启**：题卡只在当前进程内；进程关闭后显示中断，不假装恢复。报告清单
   也未持久化到稿件库。
7. **工具结果队列有界**：`tool_result` 走容量 4 的 stdin 队列；极端情况下写入失败会让本轮
   报错而不是静默丢结果。
8. **未做真实模型试验**：5 份技能与工具提示对目标模型的遵循程度、`ask_user` 触发频率、
   `finish_report` 使用率均未联机测量；需要主代理用真实内网模型和 GUI 验收。
9. **RAG / rerank 未改**：沿用现有知识库设置与检索链；Pi 只读结果并登记证据。若知识库未启用，
   `kb_search` 明确失败，`@` 知识库引用也会明确报错，不静默丢。
10. **旧技能选择**：输入框上旧的 15 个内置技能不套用到 Pi；本轮会明确写入上下文说明，
    界面仍保留旧技能标签，需要用户改选 Pi 技能或自动。没有把旧技能硬映射成 Pi 流程。

## 六、停止点

本报告写出后停止修改代码，等待主代理审查、修正与全量回归。未提交、未推送、未发布。
