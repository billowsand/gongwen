# AI 智能体内核加固设计（检查点 · 控制流 · 子图 · 工具契约）

> 起因：2026-10-06 对照 LangChain / LangGraph 的能力面做了一次测绘，结论是**编排内核对得上
> 大半「库」的部分，缺的是「图」的部分**——没有每步检查点、没有分支、没有子图、状态是扁平
> 一坨、工具参数没有类型。本文把该补的部分定成五期方案。
>
> 读者：继续开发 `src/agent/` 与 `src/ai_panel/` 的人。改任何一期之前先读
> `docs/ai-architecture.md` 的三条红线，再读本文的「与红线的关系」。
>
> 与其它文档的关系：本文**不改变** `docs/ai-agent-workbench.md` 定的三层结构（技能 / 算子 /
> 工具）与领域设计，只加固内核；功能与分期仍以那份文档为准。数据接口见
> `docs/api-workbench.md`，与本文第 4 期（工具契约）交界处在 6.3 说明。

---

## 一、目标与不做的事

### 1.1 目标

一句话：**让流程能停下来、能接着跑、能分层复用，并且状态是可控的。**

四条具体目标：

1. **可恢复**：任何一步之后进程死掉/关窗，重开稿件能从最近一步接着跑，不再只剩「重新生成」。
2. **可分层**：流程能表达分支、循环与「技能调技能」，把现在焊死在算子 Rust 代码里的编排搬到 DSL。
3. **可契约**：工具参数有类型、有 JSON Schema，模型给的参数先校验再执行——本地 7B 模型受益最大。
4. **可度量**：token 用量、耗时、每一步的输入输出有地方看、能导出。

### 1.2 不做的事（明确否决，别再讨论）

| 不做 | 理由 |
|---|---|
| Runnable / LCEL 式通用组合子 | 声明式 `step` 列表 + 算子已够用；通用组合子在 Rust 里要靠泛型体操，错误信息会变差 |
| 库边界 / 公开 API / 拆 crate | 只为对外发布服务；本轮目标是加固自家产品（2026-10-06 决定） |
| async / tokio 迁移 | 闸门、`ask_user` 暂停、协作式取消都必须待在循环内部，沿用 `thread::spawn` + 阻塞 reqwest（`ai-agent-workbench.md` 第七节的论证继续成立） |
| MCP / 开放工具协议 | 同上，D5 未变 |
| 多 provider 适配层 | 只连 OpenAI 兼容端点，`ModelRef` + 提供商管理已覆盖 |
| LangGraph 式完整时间旅行 | 不做「改历史状态再分支重跑」；只做第 1 期的「从某个检查点重跑」，且重跑覆盖该点之后的产物 |

### 1.3 与红线的关系（每次改内核都要过这一遍）

1. **正式稿只能由人合并**：检查点存的是 AI 工作稿与黑板，不含也不写正文。恢复后照旧走
   `SkillReport` → 定稿 → 提案 → 用户接受。**续跑路径不得绕过闸门**：`run_engine` 的
   `Outcome::Done` 分支里那道强制闸门（`reviewed_draft`）对续跑同样生效，这是同一条代码路径。
2. **要素分两级**：检查点**不存**只读区（要素、正文快照、选区、预设、发起请求）。恢复时用
   **当前界面**的值重灌（见 3.6）。理由：从旧检查点恢复时若不重灌，会把用户已经改过的授权类
   要素「复活」，等于 AI 间接写了要素。
3. **确定性闸门是裁判**：控制流进 DSL 之后，**闸门仍然在流程之外**（`run_engine` 里，引擎跑完
   才跑），所以技能作者写不出绕过闸门的流程。第 2 期的静态校验要把这条写成注释与测试。

---

## 二、现状（断点清单，带证据）

| 断点 | 证据 |
|---|---|
| 引擎是顺序 `for` 走 `Vec<StepSpec>`，无分支、无并行 | `src/agent/engine.rs:97`、`src/agent/engine.rs:196` |
| 唯一续跑位置是「算子主动提问」，用 `resume_at: usize` 记下一个下标 | `src/agent/engine.rs:44`、`src/agent/engine.rs:99` |
| 状态是一坨扁平 `Board`，扩展区是弱类型抽屉 | `src/agent/board.rs:24`、`src/agent/board.rs:42` |
| 只在挂起瞬间产生一次快照，运行中不落盘 | `src/ai_panel/skill_job.rs:925`、`src/ai_panel/session.rs:74` |
| 运行中崩溃读回即 `Interrupted`，不可续 | `src/ai_panel/session.rs:169` |
| 技能不能调技能（全仓只有一处 `engine::run` 调用点） | `src/ai_panel/skill_job.rs:924` |
| 工具参数无类型，schema 里只有 description | `src/agent/tools/mod.rs:190`、`src/agent/toolcall.rs:32` |
| 护栏散在算子代码里，文档设计的 `budget`/`done`/`fallback_steps` 没实现 | `src/agent/ops/agent.rs:176`、`src/agent/ops/gap_loop.rs:41`、`docs/ai-agent-workbench.md:178` |

已有的好地基（不要推翻）：`testkit::ScriptedModel` + `Driver` 能在黑板上确定性地跑完整技能并代
用户答题（`src/agent/testkit.rs`），第 1 期的验收几乎全部可以架在它上面。

---

## 三、第 1 期：检查点与恢复

**这一期的核心设计决定：把「挂起」重新定义成「一种检查点」。** 挂起恢复与崩溃恢复从此走同一
个入口，这是第 3 期子图能成立的前提——否则子技能的挂起没法冒泡。

### 3.1 数据模型

```rust
/// 从哪一步接着跑。现在是单个下标；第 3 期子图需要「第 2 步里的第 1 个子步骤」，
/// 所以第 1 期就按路径设计，别等第 3 期返工。
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
    pub(crate) partial: bool,       // 见 3.3 的尺寸上限
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
重灌（3.6）。

**兼容**：`resume_at` 从 `usize` 变 `Vec<usize>`，旧会话读回时格式对不上。`SavedRun` 里给这个
字段加一个宽容反序列化器（接受数字与数组；认不出就当空路径），坏数据退回「该轮已中断」而不是
整轮读不出来。会话是**本机工作现场**（不进版本快照、不进同步 ZIP，`src/manuscript/ai_sessions.rs:1`），
这个代价值得。

### 3.2 落点：引擎里的钩子

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
- 到上限停下 → save(reason = Limit)
- 流程跑完 → save(reason = Step)，最终检查点即为「已完成」的现场

不做每步**之前**的检查点：恢复的语义是「重跑当前这一步」，而工具与算子都可能改工作稿，重跑同
一步两次比丢掉一步更危险（`ws.replace` 有 `expect` 保护，但 `ws.write` 没有）。所以规则是
**只在步骤成功之后落盘，恢复时从「没做成的那一步」重跑**。

写盘失败不中断：与「工具出错只留一条说明，流程接着走」（`src/agent/engine.rs:168`）同一个风格，
emit 一条 `Event::Note`。

### 3.3 落盘时机与保留策略

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

### 3.4 恢复：一条路

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

### 3.5 界面

`TurnState::Interrupted` 现在只有一个「重新生成」。改成：

- 找到检查点 → 卡片上给三个动作：**接着跑**（从最新检查点）/ **从头重来** / **丢弃**，
  并注明停在哪一步、什么时候存的。
- 找不到检查点 → 保持现在的行为（只有「重新生成」），不要假装能续。
- 第 1 期顺手加「**从这里重跑**」：点开检查点列表，选一个重跑。重跑前必须确认**正文没动过**
  （提案已被接受进正文就不能从旧点重跑），这条判定复用现有提案失效那套逻辑。

### 3.6 只读区重灌（红线 2 的落点）

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

### 3.7 `SkillRun` / `SavedRun` 的调整

`SkillRun`（`src/ai_panel/skill_job.rs:33`）与 `SavedRun`（`src/ai_panel/session.rs:74`）现在
持有 `board` + `suspension`。改成持有 `board` + `checkpoint`（`at` 路径 + reason）+
`suspension`（只有挂起时有）。`skill_hash` 机制不变。

### 3.8 验收

1. `testkit::Driver` 加 `run_from(&mut self, at: &StepPath)`；新测试：跑 3 步技能 → 检查点 3 份
   → 在第 2 份处恢复 → 结果与不中断跑完**逐字节相同**。
2. 挂在 `for_each` 中间恢复：一项都不重复处理（用假工具记调用次数断言）。
3. 红线回归：造一份「来源不明的日期」的材料，从检查点续跑，确认硬闸门照样挡落地。
4. 只读区重灌断言（3.6）。
5. 真机：跑研究式起草，跑到一半在任务管理器里结束进程，重开稿件点「接着跑」，能续上。
6. `cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`、
   `cargo test --locked --all-targets` 全过（`cargo test` 里约 5 个沙箱相关失败是预期结果）。

**改动面**：`src/agent/{engine,board,skill}.rs`、`src/agent/tools/mod.rs`（`Env`）、
`src/agent/testkit.rs`、`src/ai_panel/{skill_job,session,ui}.rs`、`src/manuscript/ai_sessions.rs`，
测试 `src/agent/{engine_tests,builtin_tests}.rs`、`src/ai_panel/ui_tests.rs`。

---

## 四、第 2 期：控制流进 DSL

现在 `when` 只能「跳过」，不能改走向（`src/agent/engine.rs:212`）；唯一的真循环 `gap_loop` 把编
排焊在 549 行 Rust 里（`src/agent/ops/gap_loop.rs`）。这一期把它搬回声明层。

### 4.1 新增的 DSL 能力

```yaml
flow:
  - id: 查资料                     # 新增：可被 goto 引用
    step: retrieve
    when: has_sources
    goto: 起草                     # 新增：条件成立就直接跳过去
  - step: clarify
  - id: 起草
    step: generate
  - step: 核验
    loop:                          # 新增：带退出条件的循环
      until: { var: 核验通过 }
      max_rounds: 3
      do:
        - step: verify
        - step: ask
```

- `Flow` 增加 `Goto(GotoTarget)`（算子可以要求跳到某个 `id`）。
- 步骤增加 `id`；`goto` 支持按 `id` 或按序号（`3`）。
- `loop` + `until` + `max_rounds`：**上限是强制的**，技能不写就用默认值（3）。
- 分支：不做 `if/else` 关键字，用「两个互补 `when` + `goto`」表达（现在的 `imitate` 技能已经
  用互补 `when` 表达分支，`assets/agent-skills/imitate/SKILL.md:21`，沿用这个风格，少一套语法）。

### 4.2 静态校验（`skill::validate` 同步）

- `goto` 目标必须存在；按序号必须落在范围内。
- 不可达步骤要**报问题不报错**（技能作者可能故意留着）。
- `loop` 必须有 `max_rounds`，且夹在 1..=6。
- 递归 `goto` 环不禁止（有上限就不会死循环），但要检查**整条流程的步数上限**。

### 4.3 全流程护栏（还档 D 的债）

`budget` 进 frontmatter 并真正生效：

```yaml
budget:
  steps: 50          # 整条流程最多执行几步，超了停在检查点
  llm_calls: 40
  tool_calls: 80
```

引擎里计数并在超限时 save(reason = Limit) 后返回，卡片上说明「到上限停在这里」。**这是第 1 期
检查点带来的新能力**：以前超限只能整轮失败，现在可以停下来让用户决定继续还是收尾。

### 4.4 边界（写进代码注释与测试）

闸门在流程之外，`goto` 到任何地方都绕不过它。这条要有测试：写一个技能，流程最后 `goto` 回第一
步，确认闸门仍然在 `Outcome::Done` 之后跑，且硬闸门仍然挡落地。

---

## 五、第 3 期：子图（技能即节点）

### 5.1 用法

```yaml
flow:
  - step: skill
    skill: research-draft        # 把另一个技能当节点
    args: { request: "{request}" }
    exports: [baseline_id]       # 把子技能的这几个变量带回本层
```

### 5.2 作用域规则（必须一次定清，否则变量互相踩）

| 数据 | 规则 |
|---|---|
| `workspace`、`evidence`、`ledger`、`findings`、`pinned` | **共享**：子技能的产出必须回到本层，否则白干 |
| `vars` | **子技能有自己的作用域**（进入时拷一份，退出时只把 `exports` 里的名字写回本层） |
| `questions` | **冒泡**到顶层一起问；子技能的挂起路径带上前缀（这正是 3.1 用 `StepPath` 的原因） |
| `notes` | 共享 |

### 5.3 限制与校验

- 递归深度上限 **3 层**。
- **环检测**：A→B→A 在存盘校验时就拒绝（把技能依赖图建出来做一次拓扑检查）。
- 子技能只能用**自己 `tools:` 白名单**里的工具，不能继承父技能的白名单（权限只收敛不放宽）。
- 子技能的 `output` 不生效（它不产提案），产出由顶层技能定。

---

## 六、第 4 期：工具契约（参数类型 + 结构化输出）

### 6.1 参数类型

`Input` 现在只有 `{name, required, doc}`（`src/agent/tools/mod.rs:190`），发给模型的 schema 里
`properties` 只有 description 没有 `type`（`src/agent/toolcall.rs:32`）。改成：

```rust
pub(crate) struct Input {
    pub(crate) name: &'static str,
    pub(crate) kind: InputKind,      // 复用 src/agent/api.rs 的 InputKind
    pub(crate) required: bool,
    pub(crate) doc: &'static str,
    pub(crate) example: &'static str,
}
```

生成**真正的 JSON Schema**（`type`、`required`、`enum`、`description`），工具的表驱动测试
（`src/agent/tools/mod.rs:457`）加上「每个输入都要声明 kind」与「example 必填」。

**逐工具迁移**：不要求一次全改。先加 `kind`（默认 `Text`，行为不变），再按工具改成强类型
反序列化目标（`#[derive(Deserialize)] struct Args`），每次改一个工具、跑一次全量测试。

### 6.2 参数错了要能自纠

现在缺必填直接返回错误文字（`src/agent/tools/mod.rs:322`），自主步骤里模型只能看到一句话。改成
把 **schema + 错误 + 一个正确示例**回给模型，允许重试一次（计入 `max_calls`）。第 4 期和第 1 期
的自主步骤都要吃到这个改动。

### 6.3 结构化输出（只用在程序要解析模型输出的地方）

请求体现在没有 `response_format`（`src/lmstudio/converse.rs:116`）。新增可选参数，**只在两处
用**，收益最大、风险最小：

1. `finish` 的 `summary` / 自主步骤的完成判定；
2. `clarify` 的选择题（现在是「每行一题、全角竖线分隔」的文本约定，
   `assets/agent-skills/research-draft/SKILL.md:54`，最容易被模型写坏）。

端点不支持时**必须**降级回现有正则/文本约定（部分 OpenAI 兼容实现会 4xx，参考现有的 4xx 降级
手法 `src/lmstudio.rs:174`）。判定依据沿用「设置页测试工具调用」那套按「端点 + 模型名」缓存。

---

## 七、第 5 期：用量与轨迹

- **`Tracer` 抽象**（不铺 callback 体系）：一次运行一个 tracer，记每步的 `tool/算子、输入摘要、
  输出摘要、耗时、token`。从 SSE 的 `usage` 字段累加（现在完全不读）。
- **落地**：任务流里显示本轮的 token 与耗时；能导出该轮轨迹 JSON 到配置目录（给人排查用）。
- **不要再造一套**：现有 `Event` 流继续做界面用，tracer 只做记录，两者不合并。
- 顺手对齐文档与代码的不一致：`check.layout`（`docs/ai-agent-workbench.md:1595` 有、代码无）
  要么补齐要么从文档删掉——第 5 期必须二选一，别留着。

---

## 八、分期与依赖

| 期 | 内容 | 依赖 | 用户可感知的收益 |
|---|---|---|---|
| 第 1 期 | 检查点、恢复、从检查点重跑、只读区重灌 | — | 关窗/崩溃不丢，能接着跑 |
| 第 2 期 | 分支/goto/loop、全流程步数与调用上限进 DSL | 第 1 期 | 技能能表达真流程，超限可停可续 |
| 第 3 期 | 技能即节点、作用域规则、环检测 | 第 1 期（`StepPath`）、第 2 期（节点引用） | 复杂技能能拆开复用 |
| 第 4 期 | 工具参数类型、JSON Schema、结构化输出 | 独立 | 小模型工具调用成功率上升 |
| 第 5 期 | 护栏进 DSL 的剩余部分、tracer 与用量 | 第 1 期 | 知道花了多少、能排查 |

第 1 期必须单独做完并验收——它是其余各期的地基，**`StepPath` 与「挂起即检查点」这两条设计如果
返工，第 3 期要重做。**

---

## 九、风险与对策

| 风险 | 对策 |
|---|---|
| `Board` 全量快照把库撑大（含证据包与工作稿） | 保留「首 + 最近 5」+ 512 KB 上限 + `partial` 标记（3.3）；真机跑一周后看 `ai_run_checkpoints` 的实际大小再调 |
| 检查点写盘与界面会话写盘争锁 | 稿件库已是 WAL + 2000ms busy_timeout；检查点严格单行短事务，写失败只记说明不中断 |
| `resume_at` 格式变更读坏旧会话 | 宽容反序列化 + 坏数据退回「已中断」，不整轮读不出（3.1） |
| 恢复时重跑当前步导致重复写入工作稿 | 只在步骤**成功之后**落盘（3.2）；`for_each` 按项落盘；测试断言一项都不重复 |
| 从旧检查点恢复「复活」已改的要素 | 只读区不存、恢复必 `reinject`（3.6），并有断言测试 |
| 控制流进 DSL 后被写出绕过闸门的流程 | 闸门在引擎之外，结构上绕不过；第 2 期补这条测试（4.4） |
| 子图作用域没定清导致变量互相踩 | 第 3 期 5.2 的表格是硬约定，`vars` 进入即拷贝、退出只回 `exports` |

---

## 十、决定记录（2026-10-06）

1. **只加固内核，不做通用框架、不拆 crate、不对外开放 API。** 理由是差距清单里「库边界」那类
   差距只对「给别人用的产品」有意义，而自身产品的短板集中在可恢复与可分层。
2. **挂起重新定义为一种检查点**，恢复只有一个入口。这条决定了第 3 期子图能不能冒泡挂起。
3. **检查点不存只读区**，恢复时从界面重灌。这是红线 2（要素分两级）在恢复路径上的落点。
4. **只在步骤成功之后落盘**，恢复语义是「重跑没做成的那一步」。
5. **`at: Vec<usize>` 第一天就用路径**，不先做单个下标。
6. **不做 async、不做 MCP、不做 Runnable 通用组合子**（沿用 `ai-agent-workbench.md` 第七节）。

---

## 十一、交接

**进度**：0 / 5 期。本文是方案，代码未动。

**下一步**：第 1 期（第三节），顺序建议——先 3.1 的数据模型与 `StepPath` 改造（纯类型改动，
一次编译通过），再 3.2 的 `CheckpointSink` 钩子（`Env` 加字段，`testkit` 加内存实现），再 3.3
的表与保留策略，最后 3.4–3.6 的恢复入口、界面与重灌。每一步单独提交一次，跑一次全量测试。

**已知坑**：

- `engine::run` 的 `start: usize` 换成 `&StepPath` 会同时改到 `testkit::Driver.next`、
  `skill_job` 的两处调用点、以及 `engine_tests` 里所有手写 `run_board(..., start)` 的用例；
  这是这一期最大的一次机械改动，建议单独一个提交。
- `SavedRun` 里 `board` 是整块克隆（`src/ai_panel/skill_job.rs:926`），加检查点之后有一次额外的
  克隆；`Board` 含证据包，别在热路径上多次克隆。
- 会话落盘的节流（`src/ai_panel/session.rs:28`）只覆盖界面字段；检查点走独立表、独立事务，别把
  两者塞进同一次写。
- 真机验收要动「跑到一半结束进程」，注意别把编辑中的稿子一起丢了——先另存一份再试。
