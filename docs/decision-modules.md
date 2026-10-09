# 人机决策模块设计（决策形态 · 通用清单确认 · 待决事项 · 自主步骤提问）

> 起因：2026-10-09 盘点 AI 工作台里「问用户」的几处——动笔前澄清、定文种、六要素题、大纲确认、
> 待核实缺口题、通用选择题 `ask.choice`——结论是**挂起 / 提问 / 续跑的机制已经通用，决策本身
> 还不是模块**：决策写死在各自的算子里，语义靠 `clarify::Target` 枚举分派，界面靠认出「这一步是
> `plan mode: outline`」来显示大纲的修改按钮；自主步骤（`agent`）里模型一道题也问不了。
>
> 读者：继续开发 `src/agent/`（`decision.rs`、`clarify.rs`、`engine.rs`、`ops/`）与 `src/ai_panel/`
> 的人。动手前先读 `docs/ai-architecture.md` 的三条红线、`docs/agent-kernel-hardening.md` 的检查点
> 一节（挂起即一种检查点、恢复只有一个入口，本文不改变这两条）。
>
> 与其它文档的关系：三层结构（工具 / 算子 / 技能）与领域设计以 `docs/ai-agent-workbench.md` 为准，
> 澄清的分层规则见那份文档 14.11；本文只把「问用户」这件事从算子里抽出来，成为可组合的决策。

---

## 一、现状（2026-10-09 对过代码）

### 1.1 已经通用的部分

| 机制 | 位置 | 说明 |
|---|---|---|
| 统一的题目结构 | `clarify::{Question, Choice, Action, Target}` | 定文种、动笔前、六要素、缺口、通用选择都用它表示 |
| 挂起即检查点 | `ops::Flow::{Suspend, SuspendAgain, …}`、`engine::Suspension` | 答完从 `checkpoint.at` 接着跑；`SuspendAgain` 回到本步重做（「一批只问一层」） |
| 回答统一落黑板 | `engine::apply_answers` | 四类回答各有确定性规则；切文种由界面线程执行（红线 2） |
| 通用选择题 | 工具 `ask.choice` | 流程里 `tool: ask.choice` 就能问；`from:` 从变量列表出选项 |
| 选项尽量由程序给 | 定文种、缺口的「自己填写 / 另行通知 / 删去 / 保留待核实」 | 确定性规则是裁判 |

### 1.2 缺口

1. **决策嵌在算子里**：大纲确认在 `plan` 的清单收尾里，缺口填报是 `ask` 算子 + 台账 + `gap_revise`
   三处拼起来的，动笔前澄清在 `clarify` 算子里。想「检索问题列出来先让人看一眼」「材料要点让人
   删几条」，只能改 Rust。
2. **新加一种决策要动四五处**：`clarify.rs`、`engine::apply_answers`、`skill_job::asking_labels`、
   `ui.rs` 都按 `Target` 分支。大纲的「反复优化」是专门写的：`outline::step_index` 认出步骤是
   `plan mode: outline` 才显示「优化大纲」，换一种清单就用不上。
3. **自主步骤问不了**：`agent` 算子把 `Permission::AskUser` 的工具过滤掉了（`ops/agent.rs`
   `allowed_tools`），模型拿不准只能写「【待核实】」等后面的 `ask` 步骤。
4. **答案改变不了流程走向**：`when: { var: x }` 只看变量有没有，不比较值；`ask.choice` 一次一题、
   不能多选，推荐项固定是第一个且没有理由。

## 二、目标与不做的事

### 2.1 目标

一句话：**决策是可组合的一步，不是算子的私有代码；该问的问清楚，不该打断的不打断。**

1. **可组合**：技能作者在 SKILL.md 里就能在任意位置插一个决策（确认清单、多选、表单……），不改
   Rust。
2. **按形态渲染**：挂起时带一份「决策描述」，界面按描述的形态画卡片、按描述的文字出标题与按钮，
   不再认步骤。
3. **少打断**：决策分阻塞与不阻塞；不阻塞的进「待决事项」，什么时候答都行。
4. **模型也能问**：自主步骤能提问，但问什么、问几道由程序把关。

### 2.2 不做的事

| 不做 | 理由 |
|---|---|
| 决策 trait + 动态注册表 | 决策形态是有限几种、每种都要配界面，用枚举 + 每种一个构造函数足够；trait 对象换不来什么，反而要处理序列化 |
| 自主步骤循环中途挂起、存整段对话 | 违反「恢复只有一个入口」的简单性，改用「提问即结束本轮」（第五节） |
| 通用表单引擎 / 自定义控件 | 只做公文里真用得上的几种形态 |
| 控制流 `goto` / `loop` | 维持加固文档的暂缓决定；答案影响走向只做 `when` 取值比较 |
| 记住回答后自动填 | 只把上次的回答作推荐项（附「上次的回答」），用户点了才算；授权类要素不在此列 |

### 2.3 红线约束（每加一种决策都要过一遍）

1. **决策只改黑板与工作稿**：回答落到变量、已确认信息、台账、工作稿；正文仍只经提案由用户接受
   （红线 1）。
2. **授权类要素不出题、不给选项**：密级、文号、份号、签发人、成文日期、印发机关；程序在出题处
   拦，模型出的题碰到这些整题丢弃（红线 2）。描述类要素（标题、主送等）可以给候选，候选必须
   注明出自材料或词库哪里。
3. **模型出的题照旧过闸门**：`clarify::parse_model_questions` 的长度、选项数、去重规则不放宽；
   事实类选项由程序给（红线 3）。
4. **一批只问一层**：会改变其余题前提的决策单独一批、`SuspendAgain` 回到本步（14.11）。

## 三、核心概念

**决策 = 出题 + 形态 + 落地。**

- **出题**：谁给题目和选项——程序（文种、台账、清单变量）或模型（过闸门）。
- **形态**（`decision::Decision`）：挂起时随 `Suspension` 一起交给界面、随会话保存，告诉界面画哪种
  卡片、标题与按钮写什么、能不能「让 AI 按要求重来」。
- **落地**：回答怎么写回黑板。确定性，在 `engine::apply_answers` 里按题目的 `Target` 走。

**修订的通用机制**（第一期从大纲确认里抽出）：用户在确认卡片上写修改要求 → 界面把「当前文本 +
修改要求」作为修订请求写进挂起现场的黑板（变量 `decision::REVISION`），续跑位置拨回挂起的那一步 →
那一步见到修订请求就按要求重列、再挂起确认。**修订请求由挂起的那一步自己处理**，所以产出清单的
算子（`plan`）和单独的确认算子（`confirm`）都能被修订，修订不用知道清单是谁产出的。

## 四、决策形态清单

| 形态 | 状态 | 用在哪 | 选项来源 | 阻塞 |
|---|---|---|---|---|
| 一批选择题 `Choose` | 已有 | 定文种、动笔前、六要素、缺口、`ask.choice` | 程序 / 模型（过闸门） | 是 |
| 确认清单 `ConfirmList` | **第一期** | 大纲、要点、检索问题、任意清单变量；可直接改，可让 AI 按要求重列 | 清单变量 | 是 |
| 待决事项 | **第二期** | 正文里还没定的「【待核实】」：提案接受之后也能逐条填，答完局部修订出提案 | 程序：扫正文占位 | 否 |
| 事实冲突裁决 | **第二期** | 稿里来源不明的数字 / 日期，在材料或资料里同一处写的是别的值 | 程序：同类事实 + 上下文字面比对，带出处摘录 | 随所在批次 |
| 多选取舍 | 第四期 | 材料要点写哪几条、来函事项、审校问题批量采纳 | 清单变量 | 是 |
| 表单填报 | 第四期 | 要素多的文种（会议通知），缺口批量填；一张表一次填完 | 六要素清单 / 台账 | 是 |
| 方案比选 | 第四期 | 标题、开头段、结构，并排给 2–3 个版本挑 | 模型（标题候选注明出处） | 是 |
| 证据取舍 | 第四期 | `retrieve` 之后起草之前勾掉过时、无关的文件 | 程序：命中清单（标题、日期、是否废止） | 是 |
| 改稿力度 | 第四期 | 润色、精简、改语气之前：只改字词 / 可调句式 / 可调结构 | 固定档位 | 是 |
| 依据定位 | 第四期 | `cite_check` 报「未找到原文」时从相近文件里选对的 | 程序：知识库候选（`ask.choice from:`） | 是 |
| 预算关口 | 随加固第 5 期 | 缺口循环、检索到上限：再查两轮 / 就此交稿 | 固定 | 是 |

贯穿各形态的两条：**推荐要有理由**（`Choice.detail` 写程序判断、材料原文或「上次的回答」）；
**决策要留痕**（选了什么记进提案卡的留痕，事后说得清这句为什么这么写）。

## 五、自主步骤提问（第三期）

**提问即结束本轮**：给 `agent` 步骤加一个程序提供的 `ask` 工具（与 `finish` 并列）。模型调它时：

1. 题目先过闸门（`parse_model_questions` 同一套规则 + 授权类要素拦截），不合格的打回给模型；
2. 合格的，`agent` 步骤把已做的事（工作稿、`agent_summary`）留在黑板，以 `SuspendAgain` 挂起；
3. 用户答完，回答记进 `board.notes`（与动笔前澄清同一个落点），**从头重跑这个 `agent` 步骤**，
   提示词里带上已确认信息与上一轮的摘要。

这样不保存对话中途的状态，检查点模型不变；代价是重跑时重新读一遍资料，用摘要控制。闸门：
一次运行最多问 `max_asks` 次（默认 2），一次最多 3 题；超了 `ask` 工具从工具表里拿掉。

## 六、分期

| 期 | 内容 | 验收 |
|---|---|---|
| **①** | 决策形态 `Decision` 挂在挂起上；大纲确认改造成通用的清单确认（`ConfirmList` + 修订机制）；新算子 `confirm`；界面按形态渲染；旧会话兼容 | 原有大纲测试全过；`confirm` 能确认 / 修订任意清单；旧会话里挂着的大纲确认读回后仍能优化 |
| ② | 待决事项（扫正文占位、事后作答、局部修订出提案）；事实冲突裁决（来源不明的事实给材料候选） | 提案接受后填一处占位，出一份只改那一处的提案；冲突题选材料值即替换 |
| ③ | 自主步骤提问（第五节） | 「自由任务」里模型问一题、答完接着做完 |
| ④ | 多选、表单、方案比选、证据取舍、改稿力度、依据定位；`when` 取值比较 | 各形态一个内置技能用上 |

## 七、第一期设计

### 7.1 类型

```rust
// src/agent/decision.rs
pub(crate) enum Decision {          // 随 Suspension、SavedRun、AiTurn 保存，serde 缺省 Choose
    Choose,                         // 一批选择题，界面照旧按 Target 画
    ConfirmList(ListConfirm),
}
pub(crate) struct ListConfirm {
    label: String,                  // 「大纲」「要点」「检索问题」
    var: String,                    // 确认后的清单存进哪个变量
    revisable: bool,                // 能不能「让 AI 按要求重列」
    hint: String,                   // 编辑框占位
    instruction_hint: String,       // 修改要求框占位
    submit: String,                 // 确认按钮
}
```

`ops::Flow::SuspendInto(题, 变量)` 换成 `Flow::Confirm(题, ListConfirm)`；引擎把 `ListConfirm`
放进 `Suspension.decision`，`save_as` 取 `ListConfirm.var`。回答照旧按 `Target::Pick` 落地（选
「就按这个」变量不变，改了就存改后的文本），行为与原来一致。

### 7.2 修订

- `decision::REVISION`（变量名沿用 `_outline_revision`，旧检查点里挂着的修订请求照样认）；
- `decision::revise(&mut Suspension, 当前文本, 修改要求)`：只认可修订的 `ConfirmList`，且续跑位置
  是顶层一步；写入修订请求、把 `checkpoint.at` 拨回挂起的那一步；
- `decision::take_revision(ctx, step, spec)`：挂起的那一步开头调用；有修订请求就按「优化{label}」
  提示词段（大纲沿用「优化大纲」段与原来的内置写法）重列、强制再确认；模型失败保留当前文本、
  记一条说明，仍回到确认。

### 7.3 算子 `confirm`

```yaml
- step: confirm
  over: queries          # 要确认的清单变量（列表，或一行一条的文字）
  label: 检索问题        # 卡片上的叫法，默认「清单」
  revise: true           # 允许让 AI 按要求重列，默认否（清单若来自材料原文，别让模型改）
  max: 10                # 重列时最多几条
```

变量不存在或为空时记一条说明、跳过。技能校验：缺 `over` 报错。

各产出清单的 `plan` 模式：`outline`、`list` 可修订（本来就是模型列的）；`split` 不可修订（按原文
拆的，不让模型改事实）。

### 7.4 界面

- `AiTurn.decision` 与 `turn.questions` 一起在 `AiPanel::ask` 里设置、随会话保存；
- `questions_ui` 见到 `ConfirmList` 就画清单确认卡片（原 `outline_ui` 泛化），标题、占位与按钮文字
  取自 `ListConfirm`，不可修订的不画「修改要求」与「优化」按钮；
- `ReplyDraft` 的 `outline_instruction / outline_base / outline_candidate` 改名为
  `revise_instruction / revise_base / revise_candidate`，`serde(alias)` 读旧会话；
- `CardAction::RefineOutline` → `ReviseList`，`DraftPage::refine_outline` → `revise_list`。

**有意的小变化**：`plan mode: split` 与 `mode: list`（`confirm: true`）的确认原来走普通选择题卡片
（一个「就按这个写」选项 + 预填的多行框），现在走清单确认卡片；`mode: list` 多了「优化」。

### 7.5 兼容

- 旧会话里挂着的大纲确认（`SavedRun` 没有 `decision`）：读回时按原来 `outline::step_index` 的规则
  认出来，补成可修订的 `ConfirmList`（`decision::upgrade`）；
- 检查点表结构不变（`Checkpoint` 不含决策）。

## 八、第二期设计

2026-10-09 与起草人确认的两处取舍：**待决事项从正文里扫占位**（不另存台账）；**事实冲突只做
「来源不明的事实给材料候选」**（材料之间互相打架的不做，免得动笔前多一次打断、多一批误报）。

### 8.1 待决事项

- **从哪来**：正文里的「【待核实：…】」就是待决事项本身，`gaps::Ledger::from_placeholders`
  每次现扫、全部记成要问用户的缺口（可检索的也问——起草时已经检索过了）。不另存，不会和正文
  对不上；起草人自己敲的占位一样算。来源不明的事实不扫：接受进正文的事实起草人已经过目。
- **入口**：侧栏顶上一行「正文里还有 N 处待核实 · 逐条填写」。有任务在跑、有题挂着
  （`Asking` / `Pending`）、有提案没处理、稿件只读时不提示——那时正文还会变。
- **卡片**：新状态 `TurnState::Pending`，一批最多 4 题（`skill_job::BATCH_QUESTIONS`），题目与
  选项和交付后的缺口题完全一样（`clarify::gap_questions`），`turn.research` 存列出时的正文与台账。
  随会话保存，重开稿件还在。
- **答完**（`DraftPage::apply_pending_answers`）：
  1. 正文和列出时不一样了 → 这批回答不落地，撤掉卡片，按现在的正文重新列；
  2. 一题没填 → 提示一句，卡片留着；
  3. 否则撤掉这张卡片，走与交付后缺口题**同一条**修订路径（`revise_by_answers`，从
     `apply_research_answers` 抽出）：要填、要删的交模型写进所在段落、过闸门，不过就直接替换；
     结果定稿成对照当前正文的提案，由用户接受（红线 1）。没轮到的占位挂在新提案上接着问；
     提案处理完，侧栏顶上又会提示剩下的。
- **为什么撤掉卡片**：回答已经写进修订那一轮的说明（「排查完成时限：12月1日」），没定的仍在
  正文里随时能再列；留着一张「已答」的待决卡片只会让追问的模型以为还有题没答。

### 8.2 事实冲突裁决

- **认法**（`gaps::conflict_candidates`，纯字面，不调模型）：来源不明的事实（`GapKind::Untraced`）
  所在句里，取它前后各 4 个字（去空白、到句末或换行为止）；在出处里找**同类**事实（日期对日期、
  数量对数量，`ai_guard::extract_key_facts`）、值不同、前文或后文同样几个字（至少 3 个字）的，
  记成候选（值、出处、前后 14 字的摘录），最多 2 个。长事实里的碎片（「23日」）不单独算，与稿里
  同值的写法（`equivalent_forms`）不算。
- **出处**（`Board::candidate_sources`）：起草人的材料与已确认的回答记作「材料」，证据包里的资料
  按「《题名》」；不含要素与日期（那是程序拼的）。
- **什么时候找**：`ask` 算子出缺口题之前（`Ledger::attach_candidates`）；候选存在 `Gap.candidates`
  （`serde(default)`），随台账进提案，交付后接着问的题也带着。
- **出题**（`clarify::gap_question`）：有候选时问「「12项」与材料里的说法对不上，按哪个写？」，
  选项「按材料：15项」（说明是摘录）在前，然后「改为待核实」「保留原文」；**只有一个候选时推荐它**
  （材料是起草人给的），两个候选说不准就推荐先占位。选了候选就是 `Action::Fill`，走现成的按回答
  修订（来源不明事实的填写本来就支持）。
- **不做**：材料之间互相打架；授权类要素（日期里的成文日期不在缺口台账里，不受影响）。

## 九、进度与交接

### 第 ① 期（2026-10-09，已完成，待 GUI 真机验收）

**改动**

- `src/agent/decision.rs`（新）：`Decision` / `ListConfirm`、`REVISION`、`revisable_step`、
  `revise`（界面写修订请求）、`upgrade`（旧会话补形态）；原 `src/agent/outline.rs` 删除，三个大纲
  测试搬过来改成走通用接口，另加 `confirm` 算子、不可修订清单、旧会话升级三个测试。
- `src/agent/ops/confirm.rs`（新）：`confirm` 算子；`list`（`plan` 列完清单的收尾，原
  `prepare::finish_list`）；`take_revision`（挂起那一步开头处理修订请求，原 `plan_list` 里的大纲
  专用分支）。
- `ops::Flow::SuspendInto` → `Flow::Confirm(题, ListConfirm)`；`engine::Suspension` 加 `decision`
  （`serde(default)`）；技能校验认 `confirm` 缺 `over` 与 `revise_prompt`；技能管理页步骤名「确认清单」。
- 界面：`AiTurn.decision`（与 `questions` 一起在 `AiPanel::ask` 里设、随 `SavedTurn` 存）；
  `outline_ui` → `list_ui`，文字取自 `ListConfirm`；`ReplyDraft.outline_*` → `revise_*`（`serde(alias)`
  读旧会话，有测试）；`CardAction::RefineOutline` → `ReviseList`；`refine_outline` → `revise_list`；
  `SavedRun` 加 `decision`，读回时 `decision::upgrade`。

**与方案的出入**

- 修订提示词的变量除 `{current}` 外仍给 `{outline}`（旧技能的「优化大纲」段写的是它），另给 `{label}`；
  大纲沿用原来的内置写法，其余清单用 `ops::confirm::REVISE_TEMPLATE`。
- 题目文字沿用「按这个{label}写吗？…」，没改（清单卡片不显示它，只进历史）；自己填写框的提示改取
  `ListConfirm.hint`（大纲是「每行一章：章标题：要点」）。

**已知坑**

- 流程参数**整串**只写一个变量（`text: '{queries}'`）时，引擎把变量的原值（数组）原样交给工具，
  `ws.write` 的 `text` 收到数组就写不进去；要写进文字就在变量外加别的字（`'问题：{queries}'`）。这是
  `Board::render_value` 一直以来的行为，不是这一期引入的；清单确认后选「就按这个写」变量仍是数组，
  改过才是文字。
- 修订只认顶层的一步（`for_each` 里本来就不能挂起）。

**下一步**：GUI 真机看一遍研究报告的大纲确认（优化、优化期间手改、确认）与 `split` 要点确认的新
卡片。

### 第 ② 期（2026-10-09，已完成，待 GUI 真机验收）

在分支 `feat/decision-phase2`（worktree `tmp/decision-phase2`）上做：同期 `main` 的工作区里有
别人在改 `src/ai_panel/`（粘贴材料），没编译通过，为了不互相踩分开做，合并时留意
`ai_panel.rs`、`skill_job.rs`、`ui_tests.rs` 的冲突。

**改动**

- `gaps.rs`：`Candidate`、`Gap.candidates`、`Ledger::attach_candidates`、`Ledger::from_placeholders`、
  `conflict_candidates`（附三个测试：数量按前文、日期按后文、无关数字不算；占位全列）。
- `clarify.rs`：`gap_question` 的冲突题分支、`dedup_join`。
- `board.rs`：`candidate_sources`；`ops/finish.rs`：`ask` 出题前找候选。
- 侧栏：`TurnState::Pending`（标签「待决事项」，历史里写「列出正文里待核实的几处」）、
  `AiPanel::pending_blocked` / `open_pending`、顶上提示行 `pending_banner_ui`、`apply_pending_answers`
  与抽出的 `revise_by_answers`、`CardAction::Answer` 按状态分派。
- 测试：`decision::tests::ask_offers_the_material_value_for_a_conflicting_fact`、
  `ui_tests::pending_items_in_the_document_can_be_answered_into_a_proposal_later`（提示行、列出、没填、
  正文改过重列、填一处出提案、提案未处理时不提示）。

**与方案的出入**

- 原方案的「不阻塞的题随稿件保存」换成扫正文占位（起草人选的），没有新的存储。
- 待决卡片列出时不调模型出「建议写法」（交付后的缺口题会出）：点「逐条填写」要立刻出来，不等模型。
- 冲突题去掉了「删去这个说法」，选项控制在 4 个以内；要删可以自己写。

**已知坑**

- `ai_guard` 的数量只认它那张量词表（元、人、项、件……），「12起」「3公顷」不算事实，所以也不会
  有冲突候选。这张表是硬闸门用的，不要为冲突裁决去放宽；要加量词得连闸门一起评估。
- 上下文比对是字面的：材料里换了说法（「隐患排查15项」对「排查隐患12项」）就对不上，题照旧按
  「来源不明」问，不会错，只是少一个候选。
- 待决事项一批只列 4 处；提示行的计数是全文的。

**下一步**：GUI 真机看提示行与待决卡片、冲突题的选项与摘录；合并回 `main`。第 ③ 期（自主步骤
提问）动 `ops/agent.rs`，动手前先读 `docs/agent-kernel-hardening.md` 检查点一节。
