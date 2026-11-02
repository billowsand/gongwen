# 第四阶段 B 报告：提取共享 AI 服务并拆开 Pi 模型参数

2026-11-02，worker 交付。基线 `93efcc1`，本轮只在允许范围内增改文件，未提交、未切分支、
未碰 pi-host / 打包 / 版本 / Typst / IME / 导出 / Cargo 依赖 / 许可证，未读用户配置与真实稿件，
未运行真实模型或外部服务。B1、B2、B3 已实现，`cargo fmt/check/clippy` 与全量测试通过。
下文按「实际路径 → 移动还是适配 → 验证结果 → 范围外事项 → 剩余耦合与风险」列出。

## 一、新增与删除的文件

新增（共享层）：

```text
src/ai_services.rs                       模块登记与分组说明
src/ai_services/backend.rs               模型调用接口（原 agent/backend.rs，整份搬）
src/ai_services/backend/usage.rs         用量记录（原 agent/backend/usage.rs，整份搬）
src/ai_services/budget.rs                输入预算与切块（原 agent/budget.rs，整份搬）
src/ai_services/toolcall.rs              对话与工具调用格式（原 agent/toolcall.rs，整份搬）
src/ai_services/style.rs                 风格档案（原 agent/style.rs，整份搬，改导入）
src/ai_services/knowledge.rs             KnowledgeSearch / RagSearch（从 tools/mod.rs 移出）
src/ai_services/manuscripts.rs           ManuscriptDoc / ManuscriptSource / SqliteManuscripts
src/ai_services/vocabulary.rs            词库匹配、职能建议、所属单位（从 tools/vocab.rs 移出）
src/ai_services/text.rs                  short / value_to_text / item_line / sentence_spans
src/ai_services/citations.rs             引用标记正则、编号抽取、剥离与交付清理
src/ai_services/references.rs            Reference / RefSource（序列化形态不变）
src/ai_services/persistence.rs           配置目录 JSON 读写（原 agent/api.rs 的三个函数）
```

删除：`src/agent/backend/usage.rs`（内容已进 `ai_services/backend/usage.rs`）。

## 二、纯移动与适配的区别

**整份搬、只改导入（算法、协议、文案、阈值一字未改）**

| 共享模块 | 原文件 | 改动 |
| --- | --- | --- |
| `ai_services/backend.rs` | `agent/backend.rs` | `super::toolcall` 路径不变（同为 ai_services 子模块）；新增 `ASSIST_SYSTEM` 常量（从 `tools/interact.rs` 移入） |
| `ai_services/backend/usage.rs` | `agent/backend/usage.rs` | 无 |
| `ai_services/budget.rs` | `agent/budget.rs` | 无 |
| `ai_services/toolcall.rs` | `agent/toolcall.rs` | 无 |
| `ai_services/style.rs` | `agent/style.rs` | 导入改指共享层：`backend::{ASSIST_SYSTEM,…}`、`references::RefSource`、`text::short`、`persistence::{read_json,write_json}`；`super::budget` 天然指共享 budget。测试里 `ScriptedModel` 改指 `crate::agent::testkit`（见剩余耦合 1） |

**搬出后旧模块留适配**

| 旧位置 | 实际变化 |
| --- | --- |
| `src/agent/backend.rs` | 整文件改为 `pub(crate) use crate::ai_services::backend::*;`（兼容旧编排与旧测试） |
| `src/agent/budget.rs` | 同上（glob re-export） |
| `src/agent/toolcall.rs` | 同上 |
| `src/agent/style.rs` | 同上 |
| `src/agent/board.rs` | 删掉 `Reference`/`RefSource` 定义与 `value_to_text`/`item_line`，改为单向 re-export；`Board` 其余字段、`render*`、`refresh_from_ui` 原位不动 |
| `src/agent/gaps.rs` | 删掉 `sentence_spans`、`push_trimmed`、`is_sentence_end`，re-export `sentence_spans`；`context()` 改从共享层取 `is_sentence_end`；`strip_citations` 改从共享层取 |
| `src/agent/evidence.rs` | 引用正则与 `citation_ids` / `strip_citations` / `cited_sentences` / `normalize_citations` / `delivery_markdown` 移到共享层，这里只 re-export；`Evidence` / `EvidencePack` / `EvidenceDoc` / 测试辅助全部原位不动（句切现在从共享 `text` 取） |
| `src/agent/tools/mod.rs` | 删掉 `KnowledgeSearch` / `RagSearch` / `short`，改为 re-export；`ASSIST_SYSTEM` 从共享 backend 取；`Env` / `ToolCtx` / `Tool` / `ArgKind` / `call` 等工具框架原位不动 |
| `src/agent/tools/store.rs` | 删掉 `ManuscriptDoc` / `ManuscriptSource` / `SqliteManuscripts` 与数据库适配，只留 6 个工具（JSON / 证据包装、`status_from`、`QUERY_POOL`、`keywords` 打分） |
| `src/agent/tools/vocab.rs` | 删掉 `matches` / `matches_with_duties` / `pairs` / `COMMON_PAIRS` / `suggest` / `unit_of` / `unit_name` / `unique_unit`，留 5 个工具；`suggest` / `unit_of` re-export 给旧算子与澄清 |
| `src/agent/tools/interact.rs` | 删掉 `ASSIST_SYSTEM` 常量，改从 `ai_services::backend` 导入（文案不变） |
| `src/agent/api.rs` | 删掉 `store_path` / `read_json` / `write_json`，改 re-export `ai_services::persistence`（主代理批准的本次最小提取）；`ApiStore` / `ApiSecrets` / 请求组装等业务一律未动 |
| `src/agent/tools/testing.rs` | 只改导入（`ManuscriptDoc` 从共享层取，补 `crate::rag::RetrievedChunk`） |

**Pi 与产品层改为直接导入真实共享模块（不再经 `agent::tools` / `agent::backend`）**

`src/pi_agent/tools.rs`、`src/pi_agent/delivery.rs`、`src/ai_panel.rs`、
`src/ai_panel/{session,skill_job,ui,history,mention,composer_ui,pi_job,ui_tests}.rs`、
`src/app/{agent_settings,style_settings,jobs}.rs`、`src/app/api_settings.rs` 与
`src/app/api_settings/{assist,import,detail}.rs`、`src/app/api_settings/detail/{ai_try,doc,try_tab}.rs`、
`src/draft_page/tasks.rs`、`src/draft_page/writing/{mod,live_tests,validation}.rs`、
`src/element_fields.rs`。只改导入路径与类型引用，没有改这些文件的逻辑、文案或测试判据。

静态核对结果：

- `src/ai_services/**` 生产代码没有 `crate::agent` 依赖；唯一命中是
  `ai_services/style.rs:827` 的 `use crate::agent::testkit::ScriptedModel;`，在 `#[cfg(test)] mod tests` 里（见剩余耦合 1）。
- `src/pi_agent/**` 已无 `agent::tools` / `agent::backend` / `agent::style` / `agent::board` 引用。

## 三、B3：Pi 思考配置独立

- `src/models/config.rs`
  - 新增 `PiOptions { reasoning_effort: ReasoningEffort }`，`Default` 为「沿用服务端」（不发深度参数），
    与起草侧 `LmStudioConfig` 默认一致，升级后新用户行为不变。
  - `AppConfig` 新增 `#[serde(default)] pub pi_options: Option<PiOptions>`；`Default` 为 `None`，
    留出「旧配置未迁移」的判定位。
  - `AppConfig::pi_options()`：`None` 时按默认值解析（不写回）。
  - `AppConfig::migrate_pi_options()`：`Some` 时返回 false；`None` 时——
    Pi 模型非空则复制 `lm_studio.reasoning_effort`（这就是旧版 Pi 实际隐式使用的深度），
    Pi 模型为空则用 `PiOptions::default()`；写入后返回 true，只迁一次。
    与 `migrate_providers` 分开，不混进提供商迁移，也没有引入新的全局配置框架。
  - `pi_chat()`：显式 `config.reasoning_effort = self.pi_options().reasoning_effort`，
    `config.reasoning_format = ReasoningFormat::default()`；`context_window = 0`、
    `max_tokens = 32_768` 保持原样（自动窗口 / 内部输出预算不变）。
- `src/models.rs`：re-export `PiOptions`；新增 3 个测试（见第五节）。
- `src/storage.rs`：`load_config` 在提供商迁移之后追加
  `if config.migrate_pi_options() && path.exists() { let _ = save(&config); }`，
  与既有迁移同样「写盘失败不影响本次运行、下次启动再迁」。
- `src/app/ai_manage.rs`：Pi 模型区新增自己的「思考深度」下拉（复用 `setting_row` + `ComboBox` 现有模式），
  说明文字写「默认『沿用服务端』，即不发送深度参数；与上方文字起草的思考深度互不影响」；
  删掉「思考深度暂沿用上方设置」。窗口 / 输出 / 重试等用户旋钮一律没有新增。
- `src/ai_panel/pi_job.rs`：`pi_model()` 的映射逻辑与 15 分钟上限、等回答暂停语义未改，
  只补了一句注释说明深度取自 `pi_chat`。

未改：提示词、模型协议、温度、Pi 的 `max_tokens` 与窗口解析、风格档案格式（`styles.json`）、
会话 / 检查点 / 工作区格式、GWX 与导出。

## 四、验证

```text
cargo check --locked --all-targets                      通过，0 警告
cargo fmt --all -- --check                              通过（先跑了一次 cargo fmt --all）
cargo clippy --locked --all-targets -- -D warnings       通过
cargo test --locked --all-targets                        2732 passed, 0 failed, 85 ignored
```

针对性测试（迁移后复用原测试，未写镜像实现测试）：

```text
cargo test --locked models::        48 passed
cargo test --locked ai_services::   30 passed, 1 ignored（live_style_learn）
cargo test --locked pi_             157 passed, 3 ignored（含 3 个新增 Pi 配置测试）
cargo test --locked pi_agent::      60 passed, 1 ignored
```

新增的 Pi 配置测试（`src/models.rs`）：

1. `legacy_pi_config_copies_the_thinking_depth_it_actually_used`：自造旧 JSON（有 `pi_model`、
   `lm_studio.reasoning_effort = "high"`、无 `pi_options`）→ 迁移一次得到 `High`，
   `pi_chat()` 为 `High`；再迁返回 false；把起草改成 `Low` 后 Pi 仍是 `High`。
2. `pi_without_a_model_starts_from_its_own_default`：旧 JSON 没有 `pi_model`、
   起草是 `max` → 迁移后是 `PiOptions::default()`（`Default`，不从起草借）。
3. `pi_thinking_depth_persists_and_stays_independent`：起草 `High`、Pi `Medium`，
   `draft_chat()`/`pi_chat()` 各走各的；窗口仍为 0（自动）、输出仍为 32768；
   JSON 往返后仍是 `Some(Medium)`，且不再触发迁移。

共享层的正则、句切、截断、预算、协议、引用剥离均复用原测试（`ai_services::citations` 4 个、
`ai_services::toolcall` 5 个、`ai_services::budget` 2 个、`ai_services::style` 9 个、
`ai_services::backend` 6 个），未新增重复实现测试。

## 五、范围外事项（均已报备、主代理已批准）

1. `src/prompt.rs` 的 ignored 模型测试 import 被一次全局路径替换提前触及（`agent::backend` →
   `ai_services::backend`），只此一行，未改提示词与测试判据。主代理核对后批准，未回退。
2. `src/agent/api.rs` 的 `store_path` / `read_json` / `write_json` 提取到
   `ai_services::persistence`（保留原子写、文件权限与错误语义），主代理批准；该文件其余 API 业务未动。
3. `docs/pi-implementation-plan.md`、`docs/pi-implementation/phase-4b.md`、
   `docs/pi-implementation/phase-4b-review.md` 属主代理，worker 未写。

没有其它越界改动、没有需要决策的额外用户可见契约选择。

## 六、剩余耦合与下阶段删除点

1. **测试专用**：`ai_services/style.rs` 的测试仍用 `crate::agent::testkit::ScriptedModel`
   （`source` 在 `#[cfg(test)]` 模块里）。生产代码无此依赖。下阶段把按脚本回话的假模型提到
   共享 test-support（或 `ai_services::backend` 的 `#[cfg(test)]` 子模块）后即可断开。
2. **四个兼容薄壳**：`agent::{backend,budget,toolcall,style}` 现在是 glob re-export，
   旧编排（`ops`、`engine`、`writer`、`api_import`、`apidef`、`testkit` 与各测试）仍从这里取。
   第四阶段 F 删除旧编排时整份删掉即可，不需要再迁移调用方。
3. **旧模块的 re-export 面**：`agent::board::{Reference,RefSource,value_to_text,item_line}`、
   `agent::gaps::sentence_spans`、`agent::evidence::{CITATION,citation_ids,cited_sentences,normalize_citations,strip_citations}`、
   `agent::tools::{KnowledgeSearch,RagSearch,ManuscriptSource,SqliteManuscripts,ManuscriptDoc,short,ASSIST_SYSTEM}`、
   `agent::api::{read_json,write_json}`、`agent::tools::vocab::{suggest,unit_of}`。删旧编排时一并清掉；
   产品代码已不再经这些路径取共享能力。
4. **Pi 生产代码仍依赖旧 agent 的一个模块**：`src/pi_agent/process.rs` 用
   `crate::agent::clarify::{Question, Reply, agent_questions}`（人机决策模块，本阶段不在提取范围）。
   它不是 `tools` / `backend` 那类已提取的共享能力，但删除旧 `agent/` 之前需要先把它挪到共享层或
   Pi 自己的模块——这是 4B 之外的真实耦合，建议在 4C/4F 前先处理。
5. **旧引擎 UI 侧**：`ai_panel/checkpoint.rs`、`manuscript/ai_checkpoints.rs`（测试）、
   `ai_panel/session.rs` 的检查点 / 旧会话恢复仍用 `agent::board::Board`（`Board` 未搬），属设计保留项。
6. `src/app/skill_manager/detail.rs` 仍 `use crate::agent::tools;`（技能管理界面读工具注册表），
   属第四阶段 C 的范围，本阶段未动。
7. `agent::apidef` 通过 `agent::api::{read_json,write_json}` 间接用共享 persistence；
   接口存储本身不属本阶段，保持原样。

## 七、风险与判断

- **Pi 默认思考深度选的是「沿用服务端」**（`ReasoningEffort::Default`，不发参数）：
  与升级前的有效行为一致，对不认该参数的网关最安全。旧配置有 Pi 模型时按迁徙规则复制原深度，
  因此本机实际用 `high` 的用户迁移后仍是 `high`。若产品上想要别的默认（如 `High`），只需改
  `PiOptions::default()` 与设置页那一句说明。
- **迁移会在升级后第一次成功载入配置时多写一次 `config.json`**（随后 `pi_options` 已存在，
  不再重复），写盘失败只推迟到下次启动，与现有提供商迁移行为一致；`path.exists()` 判断保留，
  全新安装不会凭空生成配置文件。
- `pi_chat()` 把 `reasoning_format` 复位为默认：Pi 的 host 映射不读这个字段，用户不可见，
  但能避免起草侧「OpenRouter 风格」的写法漏到 Pi 上。
- 文件行尾：`git status` 提示 `src/ai_panel/ui.rs` 的 CRLF 会被规范化；该文件 diff 只有 6 行，
  内容未整文件重写，属仓库既有换行设置，不是本次遗留问题。
- 未做真机 GUI 验收：Pi 设置页新增的下拉只做了编译与类型层面验证，需要主代理或用户在真机上看一眼
  布局与文案；Pi 真实模型的思考深度是否被目标服务接受，按原计划留给真机验收。
