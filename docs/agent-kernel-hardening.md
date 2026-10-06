# AI 智能体内核加固设计（工具契约 · 逐块接受 · 用量留痕 · 检查点）

> 起因：2026-10-06 对照 LangChain / LangGraph 的能力面做了一次测绘，结论是**编排内核对得上
> 大半「库」的部分，缺的是「图」的部分**——没有每步检查点、没有分支、没有子图、工具参数没有
> 类型。2026-10-07 又做了两轮独立复核并对过代码，调整如下（决定见第十一节）：
>
> - 工具契约提到第 1 期：数据接口那边已经有完整的 schema、校验与自纠（`apidef::tooling`），
>   内置工具照着迁，改一处全部技能受益；
> - 新增「提案逐块接受」：红线 1 与工作台文档 8.3 承诺过，代码里没有；
> - 用量统计与「提案卡留痕」保留，完整 tracer 与轨迹导出砍掉；
> - 检查点仍是地基，但排在前三期之后；控制流（`goto` / `loop`）与子图**暂缓**，等有具体技能
>   需要再开。
>
> 读者：继续开发 `src/agent/`、`src/ai_panel/` 与 `src/app/ai_proposal.rs` 的人。改任何一期之前
> 先读 `docs/ai-architecture.md` 的三条红线，再读本文的「与红线的关系」。
>
> 与其它文档的关系：本文**不改变** `docs/ai-agent-workbench.md` 定的三层结构（技能 / 算子 /
> 工具）与领域设计，只加固内核；功能与分期仍以那份文档为准。数据接口见
> `docs/api-workbench.md`，与本文第 1 期（工具契约）共用 `apidef::tooling` 的校验代码（3.1）。

---

## 一、目标与不做的事

### 1.1 目标

一句话：**让模型调得对、让人合得细、让花费看得见、让流程停得下也接得上。**

1. **可契约**：工具参数有类型、有 JSON Schema，模型给的参数先校验再执行；工具多时分两级给。
   内网中等模型受益最大。
2. **可局部合并**：提案按变更块接受，「AI 改了三段，只要第一段」不必整篇放弃重跑。
3. **可度量、可倒查**：每轮花了多少 token、多久；采纳进正文的段落能查到出自哪个技能、依据哪
   几条证据。
4. **可恢复**：任何一步之后进程死掉 / 关窗，重开稿件能从最近一步接着跑；到预算上限停在检查点，
   由用户决定继续还是收尾。

### 1.2 不做的事（明确否决，别再讨论）

| 不做 | 理由 |
|---|---|
| Runnable / LCEL 式通用组合子 | 声明式 `step` 列表 + 算子已够用；通用组合子在 Rust 里要靠泛型体操，错误信息会变差 |
| 把 `Board` 换成 StateGraph + reducer | `Board` 绝大多数字段已有明确类型与语义（`src/agent/board.rs:24`），只有 `vars` 是弱类型；换了只会更啰嗦 |
| 库边界 / 公开 API / 拆 crate | 只为对外发布服务；本轮目标是加固自家产品（2026-10-06 决定） |
| async / tokio 迁移 | 闸门、`ask_user` 暂停、协作式取消都必须待在循环内部，沿用 `thread::spawn` + 阻塞 reqwest（`ai-agent-workbench.md` 第七节的论证继续成立） |
| MCP / 开放工具协议、通用代码执行工具 | 同上，D5 未变；代码执行还违反「不访问文件系统」 |
| 多 provider 适配层 | 只连 OpenAI 兼容端点，`ModelRef` + 提供商管理已覆盖；同一协议下的备用模型在第 3 期做 |
| 多智能体 / 反思架构 | 公文是一条有闸门的流水线，判据必须可计算、可回放；SOP 编排层撤销的理由（`ai-architecture.md` 阶段 4）仍然成立 |
| 通用长期记忆 / 向量记忆库 | 风格档案、会话历史、忽略记忆、词表已是按领域设计的记忆；通用记忆会把来源不明的内容带进起草，撞红线 2、3 |
| 知识库自动回流（自改进 RAG） | AI 自动写知识库就是污染源；只做人工——检索不到时卡片上提示「可把《X》存入知识库」，由用户点 |
| 模型响应缓存 | 起草要的就是按最新材料重新生成 |
| 现在就换向量索引（sqlite-vec / HNSW） | 先度量再优化，见 5.4 |
| 完整 tracer、span、轨迹导出界面 | 第 3 期只做用量统计与提案卡留痕；真需要回放再议 |
| 技能评测集（真模型 + 断言跑整组） | 维持 F19「命令行评测不做」（2026-10-07 复议后仍不做）；流程层面的退化由 `builtin_tests` 的脚本模型测试兜住 |
| LangGraph 式完整时间旅行 | 不做「改历史状态再分支重跑」；只做第 4 期的「从某个检查点重跑」，且重跑覆盖该点之后的产物 |

### 1.3 与红线的关系（每次改内核都要过这一遍）

1. **正式稿只能由人合并**：
   - 检查点存的是 AI 工作稿与黑板，不含也不写正文。恢复后照旧走 `SkillReport` → 定稿 → 提案
     → 用户接受。**续跑路径不得绕过闸门**：`run_engine` 的 `Outcome::Done` 分支里那道强制闸门
     （`reviewed_draft`）对续跑同样生效，这是同一条代码路径。
   - 逐块接受只是让「人合并」的粒度更细，入口仍然只有 `accept_ai_proposal` 一个（4.3）。
2. **要素分两级**：检查点**不存**只读区（要素、正文快照、选区、预设、发起请求）。恢复时用
   **当前界面**的值重灌（6.6）。理由：从旧检查点恢复时若不重灌，会把用户已经改过的授权类
   要素「复活」，等于 AI 间接写了要素。
3. **确定性闸门是裁判**：
   - 逐块接受后合出来的是一份**没过闸门的新文本**，落地前必须再跑一遍（4.4）。
   - 第 5 期预算进 DSL 之后，**闸门仍然在流程之外**（`run_engine` 里，引擎跑完才跑），技能作者
     写不出绕过闸门的流程。

---

## 二、现状（断点清单，带证据）

| 断点 | 证据 | 归哪期 |
|---|---|---|
| 内置工具参数无类型，schema 里只有 description | `src/agent/tools/mod.rs:190`、`src/agent/toolcall.rs:52` | 第 1 期 |
| 同一项目两套标准：数据接口工具已有完整 schema、调用前校验、出错回给模型改 | `src/agent/apidef/tooling.rs:69`、`:261` | 第 1 期（照搬） |
| 内置工具没有两级访问：「自由任务」一次开放 37 个工具加全部数据接口 | `assets/agent-skills/free-task/SKILL.md:7`；接口侧已做（`tooling.rs:19`） | 第 1 期 |
| 提案只能整篇接受，文档承诺的「不要这处」不存在 | `src/app/ai_proposal.rs:257` 整篇 `take_generated`；`docs/ai-agent-workbench.md` 8.3 | 第 2 期 |
| SSE 的 `usage` 从不读，请求也没带 `stream_options` | `src/lmstudio/` 下无 `usage` 读取 | 第 3 期 |
| 采纳进正文的段落查不到来源（哪次运行、哪个技能、哪几条证据） | 提案卡只有技能名 | 第 3 期 |
| 主模型挂了没有备用，只有逐句复核那里有「失败重试一次」 | `src/lmstudio.rs:159` | 第 3 期 |
| 检索每次把全部 embedding 解码进内存算余弦，规模无度量 | `src/rag.rs:547` → `src/knowledge.rs:503` | 第 3 期（只度量） |
| 只在挂起瞬间产生一次快照，运行中不落盘；崩溃读回即 `Interrupted`，不可续 | `src/ai_panel/skill_job.rs:925`、`src/ai_panel/session.rs:169` | 第 4 期 |
| 唯一续跑位置是「算子主动提问」，用 `resume_at: usize` 记下一个下标 | `src/agent/engine.rs:44`、`:99` | 第 4 期 |
| 文档设计的 `budget` 没实现，护栏散在算子代码里 | `src/agent/ops/agent.rs:159`、`src/agent/ops/gap_loop.rs:41`、`docs/ai-agent-workbench.md:178` | 第 5 期 |
| 缺口检索、引用核对、逐句核验都是一个一个顺序调模型 | `gap_loop.rs:67`、`cite_check.rs:378`、`cite_check.rs:284` | 第 5 期 |
| `when` 只能跳过，不能改走向；唯一的真循环焊在 `gap_loop` 里；技能不能调技能 | `src/agent/engine.rs:212`、`src/ai_panel/skill_job.rs:924` | 暂缓（第八节） |

已有的好地基（不要推翻）：

- `testkit::ScriptedModel` + `Driver` 能在黑板上确定性地跑完整技能并代用户答题
  （`src/agent/testkit.rs`），`builtin_tests` 已用它覆盖内置技能；第 4 期的验收几乎全部可以架在
  它上面。
- `apidef::tooling` 的 schema 生成、`validate`、签名与示例回喂、两级访问，第 1 期照搬。
- 版本变更第 ③ 期的逐块还原 `diff_hunks::revert`（`src/draft_page/diff_hunks.rs:173`），测过
  随机顺序逐块还原逐字节回到基准；第 2 期直接复用。

---

## 三、第 1 期：工具契约（参数类型 · 两级访问 · 结构化输出）

不依赖其它期，改一处全部技能受益。参照物就在项目里，**不要另写一套校验**。

### 3.1 参数类型：复用 `apidef::tooling`

`Input` 现在只有 `{name, required, doc}`（`src/agent/tools/mod.rs:190`）。改成：

```rust
pub(crate) struct Input {
    pub(crate) name: &'static str,
    pub(crate) kind: InputKind,      // 复用 src/agent/api.rs 的 InputKind
    pub(crate) required: bool,
    pub(crate) doc: &'static str,
    pub(crate) example: &'static str,
}
```

- 先把 `apidef::tooling` 里与接口无关的部分抽出来：「按 schema 校验一组参数」（现在的
  `validate` / `check_value`）、「生成参数签名」、「拼正确示例」改成吃 `&Value` schema 的通用函数，
  接口与内置工具共用。抽的时候接口侧行为不能变（`apidef/tests.rs` 全过即为证）。
- 内置工具由 `Input` 生成**真正的 JSON Schema**（`type`、`required`、`enum`、`description`）。
- 工具的表驱动测试（`src/agent/tools/mod.rs:457`）加上「每个输入都要声明 kind」与「example
  必填」。

**逐工具迁移**：先加 `kind`（默认 `Text`，行为不变），再按工具改成强类型反序列化目标
（`#[derive(Deserialize)] struct Args`），每次改一个工具、跑一次全量测试。

### 3.2 参数错了要能自纠

现在缺必填直接返回错误文字（`src/agent/tools/mod.rs:322`），自主步骤里模型只能看到一句话。改成
与数据接口一样：把**参数签名 + 错误 + 一个正确示例**回给模型，允许重试一次（计入 `max_calls`）。
工具出错时给人看的那一行说明也顺手改成人话（现在常是原始错误串）。

### 3.3 内置工具两级访问

照搬接口侧（`tooling.rs:19` 的 `TWO_LEVEL_AT`）：

- 一个技能能用的工具（内置 + 接口）超过阈值时，不再逐个给，改成**常用工具直给 + 其余走
  `tool_search` / `tool_call` 两个元工具**。
- 「常用」默认是读正文、读写工作稿、`note`；技能可以在 frontmatter 用 `direct: [...]` 覆盖。
- 接口侧已有的 `api_search` / `api_call` 并进来，不再出现两套元工具：搜索结果里接口与内置工具
  同列，各自带签名。
- **权限不变**：`tool_call` 调的工具仍要在技能 `tools:` 白名单里，校验在 `tool_call` 执行时做，
  不在搜索时做（搜也只搜白名单内的）。
- 文本兜底模式（模型不支持原生工具调用）同样分两级：系统提示里只列直给工具与两个元工具。

### 3.4 结构化输出（只用在程序要解析模型输出的地方）

请求体现在没有 `response_format`（`src/lmstudio/converse.rs:116`）。新增可选参数，**只在两处
用**，收益最大、风险最小：

1. `finish` 的 `summary` / 自主步骤的完成判定；
2. `clarify` 的选择题（现在是「每行一题、全角竖线分隔」的文本约定，
   `assets/agent-skills/research-draft/SKILL.md:54`，最容易被模型写坏）。

端点不支持时**必须**降级回现有正则/文本约定（部分 OpenAI 兼容实现会 4xx，参考现有的 4xx 降级
手法 `src/lmstudio.rs:243`）。判定依据沿用「设置页测试工具调用」那套按「端点 + 模型名」缓存。

### 3.5 顺手清账

`check.layout`：`docs/ai-agent-workbench.md` 16.3 有、代码无（`src/agent/tools/check.rs` 开头
注释里挂着）。本期二选一：补齐，或从文档删掉。别留着。

### 3.6 验收

1. 抽出通用校验后，`apidef` 全部测试不改一行照过。
2. 每个内置工具的 schema 有 `type`；给错类型 / 缺必填时，模型收到签名 + 示例，重试一次成功
   （脚本模型造一条先错后对的回答）。
3. 「自由任务」发给模型的工具数降到阈值以内；脚本模型走 `tool_search` → `tool_call` 能完成一次
   `calc.date`；调白名单外的工具被拒。
4. 结构化输出：支持的端点走 `response_format`；脚本端点回 4xx 时自动降级，选择题仍能解析。
5. 真机：内网 DeepSeek v4 flash 与 MiniMax 2.7 各跑一遍「自由任务」与研究式起草，工具调用出错
   次数对比改造前记录（不要用 LM Studio 的结果做取舍）。
6. `cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`、
   `cargo test --locked --all-targets` 全过（`cargo test` 里约 5 个沙箱相关失败是预期结果）。

**改动面**：`src/agent/apidef/tooling.rs`（抽通用部分）、`src/agent/tools/{mod,*}.rs`、
`src/agent/toolcall.rs`、`src/agent/ops/agent.rs`、`src/agent/skill.rs`（`direct:`）、
`src/lmstudio/converse.rs`、`src/agent/clarify.rs`。

---

## 四、第 2 期：提案逐块接受

红线 1 写的是「接受提案或采纳修订建议（可以逐块排除）」，`docs/ai-agent-workbench.md` 8.3 写的是
「每个变更块可以『不要这处』」，代码里只有整篇接受 / 放弃。用户可感知、兑现已有承诺、代码基本
现成，所以排第 2。

### 4.1 交互

- 审阅窗（`src/app/ai_proposal.rs`）里每个变更块旁加一个「不要这处」开关，排除的块变灰并标
  「保留原文」；再点一次恢复。
- 横幅上的「接受提案」在有排除时改成「接受其余 N 处」。
- 侧栏结果卡上的「采用」仍是整篇；要挑着要，点「对照」进审阅窗。
- 关键事实变化表跟着排除联动（4.4），已排除的块带来的事实变化从表里去掉。

### 4.2 数据

`AiProposal`（`src/draft_page.rs:286`）加一个字段：

```rust
/// 用户排除的变更块（按 `diff_hunks::hunks` 的序号）。提案正文变了（答完核实题、撤回概括
/// 等会在已完成的提案上继续修订）就清空，按新的块重新挑。
pub(crate) excluded: BTreeSet<usize>,
```

### 4.3 合并

```text
merged = 提案正文；把 excluded 里的块按序号从大到小逐个 revert(块, before, merged)
```

- 复用 `diff_hunks::hunks` 与 `diff_hunks::revert`，不另写合并。**从后往前**还原，前面块的行号
  不受影响（版本变更第 ③ 期的测试已覆盖「从后往前逐块还原回旧文本」）。
- 审阅窗现在用的是 `manuscript_diff_ui` 的块（`DiffBlock::Changed`），要和 `Hunk.changes`
  对上序号：一个 hunk 可能覆盖几条相邻的变更，开关挂在 hunk 上。
- 入口仍然只有 `accept_ai_proposal`：它多接一个 `excluded`，空集合就是现在的整篇接受。

### 4.4 合并后重过闸门（红线 3 的落点）

合出来的 `merged` 是一份新文本，提案生成时跑过的闸门对它不作数。`accept_ai_proposal` 在落地前
对 `merged`：

1. 照旧检查「提案生成后正文又改过」与 `document_reference::ensure_preserved`；
2. 用 `reviewed_draft` 重新生成 `GeneratedDraft`（要素校验、版式提示重算），不沿用提案的
   `warnings`；
3. 重算 `ai_guard::compare_key_facts(before, merged)`：结果必须是已确认列表的**子集**；多出来
   任何一项（排除组合造出了新的事实变化，罕见但可能），退回审阅窗要求重新确认。

### 4.5 会话记录

`resolve_proposal(true)` 记成「已写入（排除 N 处）」，追问时模型知道哪些改动没有落进正文。

### 4.6 验收

1. 单元测试：三块提案排除中间一块，`merged` 等于「before 只改第一、三块」；全排除等于 `before`
   且接受按钮不可用。
2. 事实联动：一块改日期、一块改措辞，排除改日期那块后，事实变化表为空、不再要求确认。
3. 闸门：排除组合造出新的事实变化时被拦回审阅窗。
4. 提案被继续修订后 `excluded` 清空。
5. 真机：研究式起草出一份三处以上改动的提案，挑着接受，导出 Word / PDF 正常。

**改动面**：`src/app/ai_proposal.rs`、`src/draft_page.rs`（`AiProposal`）、
`src/diff_view.rs`（块旁开关）、`src/ai_panel/session.rs`（记录），测试放 `src/app/` 下。

---

## 五、第 3 期：用量、留痕、备用模型

都是小件，一期做完。**不做**完整 tracer、span、轨迹导出（1.2）。

### 5.1 用量

- 流式请求带 `stream_options: {"include_usage": true}`，读最后一帧的 `usage`；端点 4xx 时去掉
  这个字段重试，与现有的「去掉关闭思考的开关重试」同一手法（`src/lmstudio.rs:243`）。读不到就
  按字数估算，并标「估」。
- 引擎加一个累加器（挂在 `Env` 上），每次模型调用记 `prompt / completion tokens、耗时、角色`。
- 任务流里本轮末尾显示一行：`模型调用 N 次 · 输入 X / 输出 Y token · 用时 Z`。
- 现有 `Event` 流继续只做界面用，累加器只做记录，两者不合并。

### 5.2 提案卡留痕（重开 F19 的「留痕」部分，2026-10-07）

送审材料被质疑时要能回答「这段话是哪次运行、哪个技能、依据哪几条证据生成的」。

- 提案卡与审阅窗标一行：`研究式起草 · DeepSeek-V4-Flash · 证据 K1–K7 · 2026-10-07 14:22`。
- 字段都已在手：技能名、本轮模型（`ModelRef` 解析结果）、`SkillReport.evidence` 的编号、提案
  生成时间；写进会话这一轮的记录，随稿件的 AI 会话保存。
- **边界**：会话是本机工作现场，不进版本快照、不进同步 ZIP（`src/manuscript/ai_sessions.rs:1`）。
  所以留痕只在本机可查；要随稿件流转，以后再考虑写进版本说明，本期不动版本图。

### 5.3 备用模型

- 「模型服务」里起草 / 复核 / 知识库每个功能可选配一个备用模型（`ModelRef`）。
- 主模型**在吐出第一个字之前**失败（连不上、超时、5xx）就换备用跑这一次，任务流里写一行
  「主模型不可用，本次改用 X」。4xx 不换（是请求的问题，换了也一样）；**已经开始输出后断流不换**
  （换了会接出两份前缀不同的稿子），照现在的方式报错。
- 内网的两个模型（DeepSeek v4 flash、MiniMax 2.7）正好互为备用。

### 5.4 检索规模：先度量

- 知识库页显示「N 篇 / M 块」，并记最近一次检索的耗时。
- 不加向量索引。等真机数据出来（块数与检索耗时），再定要不要上 sqlite-vec / HNSW，阈值也按
  数据定。

### 5.5 验收

1. 脚本端点返回带 `usage` 的最后一帧，任务流显示正确累加；端点拒 `stream_options` 时降级为估算。
2. 留痕：提案卡上证据编号与 `SkillReport.evidence` 一致；重开稿件后会话里仍能看到。
3. 备用模型：主端点连不上 → 改用备用并有说明；主端点输出到一半断开 → 不切换、照常报错。
4. 真机：内网跑一次研究式起草，记下 token 与耗时，写进交接。

**改动面**：`src/lmstudio/{stream,converse}.rs`、`src/lmstudio.rs`、`src/agent/tools/mod.rs`
（`Env`）、`src/ai_panel/{skill_job,ui,session}.rs`、`src/app/provider_settings.rs` 或模型服务设置、
知识库页。

---

## 六、第 4 期：检查点与恢复

**这一期的核心设计决定：把「挂起」重新定义成「一种检查点」。** 挂起恢复与崩溃恢复从此走同一
个入口；第 5 期「超限停下可续」与暂缓的子图（第八节）都建在这上面。

### 6.1 数据模型

```rust
/// 从哪一步接着跑。现在是单个下标；子图需要「第 2 步里的第 1 个子步骤」，`for_each` 也要记
/// 项序号，所以第一天就按路径设计，别等以后返工。
pub(crate) type StepPath = Vec<usize>;

/// 为什么停在这里。
pub(crate) enum Reason { Start, Step, Ask, Limit, Error }

/// 一次可恢复的现场。挂起就是 reason == Ask 的检查点。
pub(crate) struct Checkpoint {
    pub(crate) at: StepPath,        // 下一个要跑的步骤
    pub(crate) reason: Reason,
    /// 卡片上给人看的一行，如「已完成 算子 gap_loop」。
    pub(crate) label: String,
    pub(crate) board: Board,        // 可变区，全量
    pub(crate) partial: bool,       // 见 6.3 的尺寸上限
}
```

`Suspension` 改成持有检查点，字段含义不变：

```rust
pub(crate) struct Suspension {
    pub(crate) checkpoint: Checkpoint,   // reason = Ask
    pub(crate) questions: Vec<Question>,
    pub(crate) save_as: Option<String>,
}
```

`engine::run` 的签名不变，只把 `start: usize` 换成 `at: &StepPath`；只读区由调用方在跑之前
重灌（6.6）。

**兼容**：`resume_at` 从 `usize` 变 `Vec<usize>`，旧会话读回时格式对不上。`SavedRun` 里给这个
字段加一个宽容反序列化器（接受数字与数组；认不出就当空路径），坏数据退回「该轮已中断」而不是
整轮读不出来。会话是**本机工作现场**（不进版本快照、不进同步 ZIP，`src/manuscript/ai_sessions.rs:1`），
这个代价值得。

### 6.2 落点：引擎里的钩子

引擎在后台线程跑（`run_engine`），需要一个可注入的落盘口，测试时能换成内存版：

```rust
/// 检查点的去处。实现方负责落盘；失败只能记一条说明，不能中断流程。
pub(crate) trait CheckpointSink {
    fn save(&self, run: &RunRef, ckpt: &Checkpoint) -> Result<(), String>;
}
```

挂在 `Env` 上（`src/agent/tools/mod.rs:156` 的 `Env` 加一个 `ckpt: &dyn CheckpointSink`），
在 `engine.rs` 的每个步骤**成功返回之后**调一次：

- `tool:` 步骤成功 → save(reason = Step)
- 算子成功返回 `Flow::Next` → save(reason = Step)
- 要提问（`Flow::Suspend*`）→ save(reason = Ask) 后再返回 `Outcome::Suspended`
- `for_each` 每处理完一项 → save（路径 = 父路径 + 项序号），长循环里也能续
- 到上限停下（第 5 期）→ save(reason = Limit)
- 流程跑完 → save(reason = Step)，最终检查点即为「已完成」的现场

**为什么重跑当前步是安全的**（2026-10-07 修正：原稿把理由写成了「`ws.replace` 有 `expect`
保护、`ws.write` 没有」，不对）：检查点存的是**整块黑板**，工作稿在里面。从检查点恢复时工作稿
回到这一步之前的样子，重跑这一步不会把写入叠两遍，与工具是否幂等无关。真正要管的是**黑板之外**
的副作用，对过代码只有这几样：

- 发给界面的事件：恢复时任务流里会再出现一遍这一步的工具行，工作稿显示可能停在崩溃前的半截。
  所以恢复第一件事是发一个 `Event::Workspace(检查点里的工作稿)`，并在任务流里插一行
  「从『×××』之后接着跑」；
- 数据接口调用：只有「只查询且给 AI 用」的接口能编成工具（`apidef::tooling::usable`），重查无害；
- 风格学习只把档案写进变量，存不存由用户点（`src/agent/ops/style_learn.rs:1`），无外部写入。

以后新增会写黑板之外的工具或算子，必须在这里登记并说明恢复时怎么办。

不做每步**之前**的检查点：它与上一步之后的检查点内容相同，多存一份没有意义。

写盘失败不中断：与「工具出错只留一条说明，流程接着走」（`src/agent/engine.rs:168`）同一个风格，
emit 一条 `Event::Note`。

### 6.3 落盘时机与保留策略

- **不节流**。一步的代价是一次 LLM 调用或一次检索，底下是一次单行 INSERT。会话落盘那条
  「最多一秒一次」的节流（`src/ai_panel/session.rs:28`）是给每帧变的界面字段用的，检查点不适用。
- **表**（`src/manuscript/ai_sessions.rs` 的 DDL 里追加，与会话同级联删除）：

  ```sql
  CREATE TABLE IF NOT EXISTS ai_run_checkpoints (
      session_id TEXT    NOT NULL,
      turn_id    INTEGER NOT NULL,
      seq        INTEGER NOT NULL,
      at_path    TEXT    NOT NULL,   -- "[2,1]"
      reason     TEXT    NOT NULL,
      label      TEXT    NOT NULL DEFAULT '',
      partial    INTEGER NOT NULL DEFAULT 0,
      data       TEXT    NOT NULL,   -- Checkpoint 的 JSON
      created_at TEXT    NOT NULL,
      PRIMARY KEY (session_id, turn_id, seq)
  );
  ```

- **并发**：稿件库已是 WAL + `busy_timeout` 2000ms（`src/manuscript.rs:346`），后台线程写检查点
  与界面线程写会话可以并存；检查点必须是**单行、短事务**，不要在事务里做别的事。
- **保留**：每轮留「序号最小的一个 + 最近 5 个」，其余从中间删。原因：`Board` 含工作稿与证据包，
  单份可能到几十万字节。
- **尺寸上限**：单份超过 512 KB 时，证据包只留最近 20 条、`partial = true`；恢复时在卡片上写
  「较早的证据已省略，需要可以重新检索」。宁可少存也不要把库撑爆。

### 6.4 恢复：一条路

```rust
// 挂起后答完（现有路径，src/ai_panel/skill_job.rs:528 附近）
let ckpt = engine::apply_answers(&mut run.checkpoint, &replies);   // 回答落到黑板
start_skill(request, Some((turn_id, run.checkpoint, run.skill)))

// 崩溃后重开（新增路径）
let latest = store.latest_run_checkpoint(session_id, turn_id)?;    // 最新一份
start_skill(request, Some((turn_id, latest.checkpoint, skill)))
```

两处最终都进 `engine::run(board, &env, &ckpt.at, emit)`。**「接着跑」按钮和「提交回答」按钮走
的是同一个函数**，这样闸门、定稿、提案生成的代码只有一份。

技能改动过怎么办：沿用现有的指纹比对（`src/ai_panel/session.rs:109`），不一致就提示一句、
按新版接着跑；技能删了就标「不能续」，行为与现在一致。

### 6.5 界面

`TurnState::Interrupted` 现在只有一个「重新生成」。改成：

- 找到检查点 → 卡片上给三个动作：**接着跑**（从最新检查点）/ **从头重来** / **丢弃**，
  并注明停在哪一步、什么时候存的。
- 找不到检查点 → 保持现在的行为（只有「重新生成」），不要假装能续。
- 「**从这里重跑**」（选一个旧检查点重跑）**放到本期最后**，前面都验收了再做。重跑前必须确认
  **正文没动过**（提案已被接受进正文就不能从旧点重跑），这条判定复用现有提案失效那套逻辑。

### 6.6 只读区重灌（红线 2 的落点）

新增一个小结构，由界面线程在跑之前组装；检查点里**不存**它：

```rust
pub(crate) struct RunInput {
    pub(crate) draft: DraftInput,       // 要素：以界面为准
    pub(crate) request: String,
    pub(crate) document: String,        // 发起时的正文快照
    pub(crate) selection: Option<String>,
    pub(crate) preset: String,
    pub(crate) system_prompt: String,
    pub(crate) time_sources: String,
    pub(crate) refs: Vec<Reference>,
    pub(crate) history: String,
    pub(crate) style: String,
}
```

`Board::reinject(&mut self, input: &RunInput)` 把这批字段按当前值覆盖。恢复路径**必须**先
`reinject` 再跑，测试里要有一条专门断言：从检查点恢复后，若界面上把成文日期改过，进模型的要素
是**新值**而不是检查点里的旧值。

### 6.7 `SkillRun` / `SavedRun` 的调整

`SkillRun`（`src/ai_panel/skill_job.rs:33`）与 `SavedRun`（`src/ai_panel/session.rs:74`）现在
持有 `board` + `suspension`。改成持有 `board` + `checkpoint`（`at` 路径 + reason）+
`suspension`（只有挂起时有）。`skill_hash` 机制不变。

### 6.8 验收

1. `testkit::Driver` 加 `run_from(&mut self, at: &StepPath)`；新测试：跑 3 步技能 → 检查点 3 份
   → 在第 2 份处恢复 → 结果与不中断跑完**逐字节相同**。
2. 挂在 `for_each` 中间恢复：已完成的项不重做（用假工具记调用次数断言）；正在做的那一项从头
   重做，且工作稿里这一章**只出现一次**（用真的 `ws.write` / `ws.replace` 跑，证明 6.2 的理由）。
3. 恢复后界面先收到检查点里的工作稿（6.2 的 `Event::Workspace`）。
4. 红线回归：造一份「来源不明的日期」的材料，从检查点续跑，确认硬闸门照样挡落地。
5. 只读区重灌断言（6.6）。
6. 真机：跑研究式起草，跑到一半在任务管理器里结束进程，重开稿件点「接着跑」，能续上。
7. fmt / clippy / test 全过（同 3.6 第 6 条）。

**改动面**：`src/agent/{engine,board,skill}.rs`、`src/agent/tools/mod.rs`（`Env`）、
`src/agent/testkit.rs`、`src/ai_panel/{skill_job,session,ui}.rs`、`src/manuscript/ai_sessions.rs`，
测试 `src/agent/{engine_tests,builtin_tests}.rs`、`src/ai_panel/ui_tests.rs`。

---

## 七、第 5 期：全流程预算与只读并行

### 7.1 全流程预算（还档 D 的债）

`budget` 进 frontmatter 并真正生效：

```yaml
budget:
  steps: 50          # 整条流程最多执行几步，超了停在检查点
  llm_calls: 40
  tool_calls: 80
```

引擎里计数（与 5.1 的用量累加器同一处），超限时 save(reason = Limit) 后返回，卡片上说明「到
上限停在这里」，给「继续（再给一份预算）」与「就这样收尾」两个动作。**这是第 4 期检查点带来的
新能力**：以前超限只能整轮失败。算子里散落的上限（`agent` 的 `max_turns` / `max_calls`、
`gap_loop` 的轮数）保留，作为单步内的上限，全流程预算管总量。

### 7.2 边界测试（写进代码注释与测试）

闸门在流程之外，预算停在哪里都绕不过它：写一个技能在第 2 步就撞上限，选「就这样收尾」，确认
闸门仍然在 `Outcome::Done` 之后跑，且硬闸门仍然挡落地。

### 7.3 算子内的只读并行

内网 vLLM 吃得下并发，而缺口检索（`gap_loop.rs:67`）、引用核对（`cite_check.rs:378`）、逐句核验
（`cite_check.rs:284`）都是一条一条顺序调。**并行放在算子内部，不进 DSL**：现有技能里能并行的
都在这几个算子里，`policy-report` 的逐章写作要按顺序（后一章看前一章），不在此列。

- `ops` 里加一个有界并发的 `par_map`（`std::thread::scope`，不引入 async），并发数来自模型服务
  设置，**默认 1**（行为与现在完全相同），内网可调到 4。
- 只用于**不写黑板**的子任务：每个子任务拿只读输入、返回结果，算子在主线程**按原顺序**合回黑板，
  证据编号与顺序跑时一致。
- 取消照旧：每个子任务开始前查 `check_cancel`。
- 事件：子任务的工具行先缓冲，按原顺序吐给界面，不交错。

**已知坑**：`Env` 里挂着 `&dyn` 的模型、知识库等，未必 `Sync`；先确认能不能跨线程共享，不能就
只把「发请求」那一层（阻塞 reqwest 客户端本身是 `Sync`）拿出来并行。

验收：并发 1 与并发 4 跑同一份脚本，黑板**逐字节相同**；真机内网记下缺口循环耗时对比。

---

## 八、暂缓：控制流与子图

2026-10-07 定为暂缓。理由：现有技能都用不上——`imitate` 用互补 `when` 表达了分支
（`assets/agent-skills/imitate/SKILL.md:21`），唯一的真循环在 `gap_loop` 里跑得好好的，没有哪个
技能需要调另一个技能。子图还会让「改 A 技能炸了 B 技能」成为常态。

**重开条件**：出现第二个需要循环的技能，或出现一个确实要复用另一技能整条流程的技能。届时按下面
的设计做，第 4 期的 `StepPath` 已为它们留好位置。

### 8.1 控制流（设计保留）

```yaml
flow:
  - id: 查资料                     # 可被 goto 引用
    step: retrieve
    when: has_sources
    goto: 起草                     # 条件成立就直接跳过去
  - step: clarify
  - id: 起草
    step: generate
  - step: 核验
    loop:                          # 带退出条件的循环
      until: { var: 核验通过 }
      max_rounds: 3
      do:
        - step: verify
        - step: ask
```

- `Flow` 增加 `Goto(GotoTarget)`；步骤增加 `id`；`goto` 支持按 `id` 或按序号。
- `loop` + `until` + `max_rounds`：上限强制，不写用默认值（3），夹在 1..=6。
- 不做 `if/else` 关键字，用「两个互补 `when` + `goto`」表达。
- 静态校验：`goto` 目标必须存在；不可达步骤报问题不报错；递归 `goto` 环不禁止，但受第 5 期
  全流程 `budget.steps` 约束。
- 边界测试：流程最后 `goto` 回第一步，闸门仍在 `Outcome::Done` 之后跑。

### 8.2 子图：技能即节点（设计保留）

```yaml
flow:
  - step: skill
    skill: research-draft
    args: { request: "{request}" }
    exports: [baseline_id]
```

| 数据 | 规则 |
|---|---|
| `workspace`、`evidence`、`ledger`、`findings`、`pinned`、`notes` | **共享** |
| `vars` | 子技能自有作用域：进入时拷一份，退出时只把 `exports` 写回本层 |
| `questions` | **冒泡**到顶层一起问；挂起路径带前缀（`StepPath`） |

限制：递归深度上限 3 层；存盘校验时做依赖图环检测；子技能只能用自己 `tools:` 白名单里的工具
（权限只收敛不放宽）；子技能的 `output` 不生效。

---

## 九、分期与依赖

| 期 | 内容 | 依赖 | 用户可感知的收益 |
|---|---|---|---|
| 第 1 期 | 工具参数类型（复用 `apidef` 校验）、出错自纠、两级访问、结构化输出 | — | 内网模型调工具少出错 |
| 第 2 期 | 提案逐块接受，合并后重过闸门 | — | 只要其中几处改动不必整篇放弃 |
| 第 3 期 | 用量统计、提案卡留痕、备用模型、检索规模度量 | — | 知道花了多少、段落查得到出处、主模型挂了不中断 |
| 第 4 期 | 检查点、恢复、只读区重灌、（最后）从检查点重跑 | — | 关窗 / 崩溃不丢，能接着跑 |
| 第 5 期 | 全流程预算进 DSL、算子内只读并行 | 第 4 期（Limit 检查点）、第 3 期（累加器） | 超限可停可续；内网上缺口循环更快 |
| 暂缓 | `goto` / `loop`、子图 | 第 4 期（`StepPath`） | 等有具体技能需要 |

第 1–3 期互不依赖，可以并行开工。第 4 期单独做完并验收再动第 5 期；**`StepPath` 与「挂起即
检查点」这两条设计不能简化成单个下标**，否则重开子图时要返工。

---

## 十、风险与对策

| 风险 | 对策 |
|---|---|
| 抽通用校验时改坏了数据接口的行为 | 先抽后迁，`apidef` 测试不改一行照过才算抽完（3.1） |
| 两级访问让模型多走一步、反而更慢 | 常用工具直给，只有超阈值才分级；真机对比改造前后的出错次数与轮数（3.6） |
| 逐块排除拼出前后矛盾的正文（如只要了改标题、没要改正文对应处） | 这是用户的选择；合并后照常重跑审校，软闸门问题以修订建议出现；事实变化超出已确认范围时拦回（4.4） |
| 拿不到 `usage` | 降级为字数估算并标「估」（5.1） |
| 备用模型在输出中途切换，拼出两份稿子 | 只在第一个字之前切换（5.3） |
| `Board` 全量快照把库撑大（含证据包与工作稿） | 保留「首 + 最近 5」+ 512 KB 上限 + `partial` 标记（6.3）；真机跑一周后看 `ai_run_checkpoints` 的实际大小再调 |
| 检查点写盘与界面会话写盘争锁 | 稿件库已是 WAL + 2000ms busy_timeout；检查点严格单行短事务，写失败只记说明不中断 |
| `resume_at` 格式变更读坏旧会话 | 宽容反序列化 + 坏数据退回「已中断」，不整轮读不出（6.1） |
| 恢复后界面显示与黑板不一致 | 恢复第一件事发 `Event::Workspace`；新增有黑板外副作用的工具必须在 6.2 登记 |
| 从旧检查点恢复「复活」已改的要素 | 只读区不存、恢复必 `reinject`（6.6），并有断言测试 |
| 并行改变了证据编号或顺序 | 只读子任务、主线程按原顺序合并；并发 1 与 4 黑板逐字节相同（7.3） |

---

## 十一、决定记录

### 2026-10-06

1. **只加固内核，不做通用框架、不拆 crate、不对外开放 API。** 理由是差距清单里「库边界」那类
   差距只对「给别人用的产品」有意义，而自身产品的短板集中在可恢复与可分层。
2. **挂起重新定义为一种检查点**，恢复只有一个入口。这条决定了子图能不能冒泡挂起。
3. **检查点不存只读区**，恢复时从界面重灌。这是红线 2（要素分两级）在恢复路径上的落点。
4. **只在步骤成功之后落盘**，恢复语义是「重跑没做成的那一步」。
5. **`at: Vec<usize>` 第一天就用路径**，不先做单个下标。
6. **不做 async、不做 MCP、不做 Runnable 通用组合子**（沿用 `ai-agent-workbench.md` 第七节）。

### 2026-10-07（两轮独立复核合并后，用户拍板）

7. **分期重排**：工具契约 → 提案逐块接受 → 用量留痕备用模型 → 检查点 → 预算与并行。前三期
   独立、成本低、用户直接受益；检查点仍是后两期与暂缓项的地基。
8. **工具契约复用 `apidef::tooling`**，不另写校验；内置工具照接口侧做两级访问，两套元工具合一。
9. **新增提案逐块接受**：兑现红线 1「可以逐块排除」与工作台文档 8.3；复用 `diff_hunks::revert`；
   合并后必须重过闸门。
10. **重开 F19 的「留痕」部分**：只做提案卡上一行来源（技能 · 模型 · 证据编号 · 时间），存在本机
    会话里，不进版本图。**评测部分不重开**：技能评测集不做，维持 F19。
11. **完整 tracer 与轨迹导出砍掉**，只留用量统计。
12. **控制流（`goto` / `loop`）与子图暂缓**，设计保留在第八节，按重开条件再做。
13. **并行放在算子内部、只做只读子任务、默认并发 1**，不进 DSL。
14. **向量索引先度量再说**；多智能体、通用记忆、自改进 RAG、模型响应缓存不做。
15. 修正 10-06 稿 3.2 的理由：重跑当前步安全，是因为检查点含整块黑板，与工具是否幂等无关；要管
    的是黑板之外的副作用（6.2）。

---

## 十二、交接

**进度**：0 / 5 期。本文是方案，代码未动。

**下一步**：第 1、2、3 期可以并行，建议先做第 1 期——

1. 先把 `apidef::tooling` 的校验、签名、示例抽成吃 `&Value` schema 的通用函数，`apidef` 测试
   照过，单独提交；
2. `Input` 加 `kind` / `example`（默认值保持行为不变），schema 带上 `type`，单独提交；
3. 逐工具迁成强类型参数，每个工具一个提交；
4. 两级访问与元工具合并；
5. 结构化输出两处；`check.layout` 二选一。

第 4 期开工时的顺序建议：先 6.1 的数据模型与 `StepPath` 改造（纯类型改动，一次编译通过），再
6.2 的 `CheckpointSink` 钩子（`Env` 加字段，`testkit` 加内存实现），再 6.3 的表与保留策略，最后
6.4–6.6 的恢复入口、界面与重灌；「从这里重跑」放在本期最末。每一步单独提交一次，跑一次全量测试。

**已知坑**：

- 第 1 期：接口工具名有 `wire_name` 的 ASCII 化与截短加哈希（`docs/api-workbench.md` 第十二节），
  元工具合并后 `tool_call` 的参数里写的是哪种名字要定清，建议统一用 `wire_name`。
- 第 2 期：审阅窗的块（`DiffBlock::Changed`）与 `diff_hunks::Hunk` 不是一一对应，一个 hunk 可能
  盖几条变更，开关挂在 hunk 上。提案在已完成后还会被继续修订（`install_ai_proposal` 的注释），
  `excluded` 要随之清空。
- 第 4 期：`engine::run` 的 `start: usize` 换成 `&StepPath` 会同时改到 `testkit::Driver.next`、
  `skill_job` 的两处调用点、以及 `engine_tests` 里所有手写 `run_board(..., start)` 的用例；
  这是这一期最大的一次机械改动，建议单独一个提交。
- 第 4 期：`SavedRun` 里 `board` 是整块克隆（`src/ai_panel/skill_job.rs:926`），加检查点之后有一次
  额外的克隆；`Board` 含证据包，别在热路径上多次克隆。
- 第 4 期：会话落盘的节流（`src/ai_panel/session.rs:28`）只覆盖界面字段；检查点走独立表、独立
  事务，别把两者塞进同一次写。
- 第 4 期真机验收要动「跑到一半结束进程」，注意别把编辑中的稿子一起丢了——先另存一份再试。
- 第 5 期：`Env` 的 `Sync` 问题见 7.3。
