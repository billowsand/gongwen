# Pi 写作业务评测：怎么跑、怎么判、能证明什么

`scripts/pi-writing-eval.mjs` 用**真实 Pi host 与真实标准技能**跑一组**自造案例**，
把「技术交付」与「业务判据」分开记录。它不连内网知识库，不读写用户配置、稿件、
风格库，也不把关键词通过率当写作质量。

2026-11-02 主代理已在七牛 MiniMax M2.7 上执行真实 host 业务评测；初测、人工发现、
修正和复测见文末实测记录及 `pi-implementation/phase-4d-review.md`。
Pi 实现者此前只跑离线单测与本机 HTTP 桩，桩结果不是模型效果，不能引用为写作能力数据。

## 一、真实与夹具的边界

| 环节 | 是真实的 | 是夹具 |
| --- | --- | --- |
| 执行 | `pi-host/src/main.mjs` 真实进程、JSONL 协议、Durable 会话、文件沙箱 | — |
| 技能 | `assets/pi-skills/` 的 8 份标准技能整包复制进工作区 | — |
| 语法 | `skills/gongwen-markdown/` 完整包（真实文件） | — |
| 模型 | 主代理提供的 OpenAI 兼容端点与模型 | 案例内容自造 |
| 知识库 / 稿件库 / 词库 / 风格 | — | `tests/fixtures/pi-writing-eval/bridge.json`，返回结构与生产工具一致 |
| 材料 / 表单要素 / 提问回答 | — | `cases/` 与 `cases.json`，全部自造 |
| `check_document` | — | 夹具实现：只查日期 / 数字 / 书名号文件名的字面出处；**不是**生产 Rust 闸门 |
| 交付闸门 | 宿主侧（空稿、路径、哈希、回执、超时） | 选区外不变等 Rust 侧判据由脚本按同一规则复核 |

因此本评测回答的是「在这些自造任务上，模型 + 真实 host + 标准技能能否按判据交付」，
不回答「内网知识库联机效果」或「真实公文写作质量总分」。

另有 Rust 针对性用例 `pi_agent::tools::tests::pi_tool_context_uses_real_temporary_sqlite_knowledge_chain`：
它只写自造记录到临时 SQLite，实际经过 `KnowledgeStore`、生产 `rag::retrieve`、只读 `SqliteManuscripts` 与 `PiToolContext`，
核验 FTS 命中、稿件检索/读取、分页、证据登记和 embedding 失败降级告警。当前 loopback embedding 端点故意不可用，
所以它证明的是**真实 SQLite/FTS 链 + 模拟 embedding 服务失败**，不证明真实 embedding 或 rerank 可用。
现有 Node 桥接评测仍是 JS 模拟资料，两种结果分开报告，不得合并称作真实知识库/向量链验收。

生产能力校准的真实模型入口是 Rust ignored 测试 `live_qwen_synthetic_sqlite_paired_calibration`；
它用四次同题对照跑真实 PiJob、Pi host 和临时 SQLite/FTS 知识库 / 稿件库。模型必须从 `kb_search`、`kb_read`、
`ms_search`、`ms_read` 取证后在正文保留仅库内的日期与办理地点。它不访问真实用户库；loopback embedding 仍是故意不可用，
所以真实模型评测同时覆盖真实 FTS 和模拟向量服务故障。机器判据不替代人工质量评审。

在用户已经启动并授权的 LM Studio 与开发 Pi runtime 上，可显式这样运行（API key 可不设；运行结果保存在仓库 `tmp/`）：

```powershell
$env:TEMP = 'D:\gongwen\tmp'; $env:TMP = 'D:\gongwen\tmp'
$env:GONGWEN_PI_RUNTIME_DIR = 'D:\path\to\runtime\pi'
$env:GONGWEN_PI_NODE = 'D:\path\to\runtime\pi\node.exe'
$env:GONGWEN_PI_LIVE_CALIBRATION = '1'
$env:GONGWEN_PI_CALIBRATION_URL = 'http://127.0.0.1:12345/v1'
$env:GONGWEN_PI_CALIBRATION_MODEL = '当前已加载模型 ID'
$env:GONGWEN_PI_CALIBRATION_THINKING = 'high'
cargo test --locked pi_agent::capability::tests::live_qwen_synthetic_sqlite_paired_calibration -- --ignored --nocapture
```

测试要求 Rust 已能定位验证过的 Pi runtime；不联网安装依赖、不读取应用配置或真实凭据。结果档案与含四份合成成品的
单独证据文件写到 `tmp/pi-implementation/phase-6ab-live-evidence/`，文件摘要相互校验。

## 二、案例

`tests/fixtures/pi-writing-eval/cases.json` 共 9 个：

| 名称 | 场景 | 关键业务判据 |
| --- | --- | --- |
| `draft-from-material` | 按会议纪要起草通知 | 材料里的三个时限 / 数字 / 单位都要保留 |
| `condense-selection` | 只改选区、压到 80 字以内 | 任务、责任单位、两个时限不丢；选区外逐字不变 |
| `reply-unknown-stance` | 复函逐项答复，第二项用户没定 | 交付说明建议主送来函落款「市数据局」，正文不写抬头；不得出现「原则同意 / 暂缓」；未定项写待核实 |
| `material-conflict` | 两份材料时限冲突 | 两个日期都保留，并标「待核实 / 矛盾 / 不一致」；不能擅自安排成两阶段或新增通报 |
| `policy-source-missing` | 引用的文件知识库里没有原文 | 必须说「未找到原文」；不得说「已核实」；不改稿 |
| `extract-only` | 只提炼三条要点 | 走 `finish_report`，正文不变，事实保留 |
| `research-sources` | 写研究报告，没有文献库 | 知识库数据要出现；不得写 `[@` 或 `doi:`；调过知识库工具；说明资料边界，不把行业通病当作已证实本市事实 |
| `style-apply` | 按风格档案写通知 | 事实材料里的单位 / 日期 / 数字保留；样稿与档案里的单位 / 日期 / 数字不得出现；调过 `style_read` |

`cases.json` 的 `expect` 只放判据（事实、禁项、说法、工具），不放标准答案正文；
不限定工具调用顺序，也不要求模型调用不必要的工具。

## 三、判据与指标

### 技术交付（每个案例都跑）

- 进程退出码 0、未超时、有 `completed`、没有 `error`；
- 领域桥工具调用没有失败（夹具不知道或参数不合法也算失败）；此项仅覆盖桥返回结果，
  不声称识别了文件工具的全部可恢复错误，完整运行轨迹可查 Durable 会话；
- 正文任务：真的 `submit_document`、没有同时交清单、路径在 `work/`、非空、
  `submitted.sha256` 与内容一致、回执 `kind/sha256/bytes` 与交付一致、
  回执成品 `final.md` 与交付内容逐字相同；
- 清单任务：真的 `finish_report`、没有提交正文、清单非空；
- 选区任务：交付正文的选区前、选区后与基准逐字相同（与 Rust `check_delivery` 同一规则）。

### 业务判据（按案例声明）

- `required`：交付内容必须包含的事实字符串；
- `requiredSummary`：正文交付说明必须包含的建议，单独检查，不允许为了满足此项把主送写进正文；
- `forbidden`：不得包含（样稿事实、替用户表态的结论等）；
- `requirePhrasesAny`：至少命中一组的说法（如「未找到原文 / 未找到」）；
- `minChars` / `maxSelectionChars`：篇幅与选区压缩量；
- `requiredToolsAny`：至少调过一个指定工具（不限定顺序）。
- `inlineFootnotesOnly`：使用脚注时必须是应用支持的行内语法，不接受裸引用或尾部定义。

### 人工判断（单列，不折算分数）

每个案例的 `expect.manual` 是一组人工问题（结构、语气、得体、可用性），只出现在
`report.md` 的勾选清单里。**关键词覆盖与人工判断不相加**，`keywordCoverage` 明确标注
「不是写作质量总分」。

### 输出指标

- 技术判据通过率 = 技术判据全过的案例数 / 案例数；
- 业务判据通过率 = 业务判据全过的案例数 / 案例数；
- 单项判据明细（`report.md` 列出未过项）；
- 关键词覆盖（必含命中数 / 总数、禁项命中数 / 总数），只作诊断；
- 工具轨迹（工具名顺序），只记录事实，不要求固定顺序。

## 四、运行

```powershell
# 离线：列案例、判据自测
node scripts/pi-writing-eval.mjs --list
node scripts/pi-writing-eval.mjs --selftest

# 真实模型（模型配置由主代理临时提供，脚本不读任何本机凭据）
node scripts/pi-writing-eval.mjs --case draft-from-material `
  --model-config tmp/pi-eval-model.json `
  --out tmp/pi-writing-eval/run-1 --timeout 600

# 全部案例
node scripts/pi-writing-eval.mjs --model-config tmp/pi-eval-model.json

# 只对已有结果重新判定（不连模型）
node scripts/pi-writing-eval.mjs --check tmp/pi-writing-eval/run-1
```

- `--out` 必须是仓库 `tmp/` 下的目录；默认 `tmp/pi-writing-eval/<时间戳>-<pid>`。
- `--model-config` 也可用 `-` 从 stdin 读（密钥不落盘），或环境变量
  `GONGWEN_PI_EVAL_MODEL_CONFIG`；也支持
  `GONGWEN_PI_EVAL_BASE_URL` / `_API_KEY` / `_MODEL_ID`（可选 `_CONTEXT_WINDOW`、`_MAX_TOKENS`）。
- 没有显式配置就拒绝跑模型；拒绝 `.env`、`.pi/auth` 一类路径。
- 每个案例默认 600 秒超时；超时算技术失败并保留工作区。
- 服务商有 RPM 限制时可加 `--case-delay 65`，在相邻案例之间间隔 65 秒；
  不改变单个案例中的 Pi 调用，也不重写或修补模型产物。
- 输出目录必须为空，避免旧回执混入；重新判定已有结果请用 `--check`。

模型配置形如：

```json
{
  "baseUrl": "https://example.invalid/v1",
  "apiKey": "临时密钥",
  "modelId": "模型名",
  "contextWindow": 128000,
  "maxTokens": 8192,
  "reasoning": true,
  "thinkingLevel": "high"
}
```

## 五、输出结构与凭据

```text
tmp/pi-writing-eval/<run>/
  run.json                 脱敏模型标识、案例判据、耗时、产物路径
  report.md                人看的报告（技术 / 业务 / 关键词 / 人工勾选项）
  cases/<名称>/
    result.json            本轮事件、交付、回执、工具轨迹、问题与回答
    events.jsonl           协议事件（不含 init）
    artifacts/final.md     交付正文快照（有正文时）
    artifacts/receipt.json 交付回执
    artifacts/report.json  finish_report 清单
    workspace/             模型看到的工作区（input / skills / work）
    run/                   交付目录、回执、host-owner.sqlite、session.sqlite
```

- **不保存** init 原文、带密钥的配置或完整请求；`run.json` 只写模型名、地址主机名、
  窗口与输出上限。
- stderr、错误与事件里的密钥串统一替换为 `[已隐藏]`。
- 失败也保留 `workspace/`、`events.jsonl` 与 `result.json`，便于复现「模型看到了什么、
  调了什么、交了什么」。

## 六、离线单测

`scripts/pi-writing-eval.test.mjs`（Node 内置测试，无新依赖）：

```powershell
node --test scripts/pi-writing-eval.test.mjs
```

覆盖：夹具自检（案例名、技能、材料、选区唯一性、人工判断项）、桥接夹具结构、
判据对「好结果 / 缺事实 / 搬样稿事实 / 选区外被改 / 该交正文却只交清单 / 该交清单却交
正文 / 工具失败 / 进程异常 / 找不到原文却说已核实」的区分、`deliveredText`、
CLI 严格解析、模型配置脱敏与凭据文件拒绝、脚本自带 `--selftest`。

## 七、已知限制

- **不测内网知识库**：桥接返回是自造的；检索质量、真实文档结构、权限都不在评测范围。
- **不测生产闸门**：夹具 `check_document` 只做字面出处检查；Rust 的要素、词表、
  公文引用、事实台账在应用里跑，评测不重复实现。
- **不自动评判写作质量**：结构、语气、得体、可用性由人工按 `manual` 看；关键词
  通过率只说明指定事实在不在。
- **单次运行受模型随机性影响**：本脚本记录单次结果，不做多次平均；需要稳定性时由
  主代理重复运行并对比 `report.md`。
- **不强制工具顺序**：模型可以不检索、不读风格，只在业务判据要求时才判失败。
- **没有在 CI 里连真实模型**：本脚本的离线单测可单独执行，尚未加入 CI；真实模型评测由主代理手动执行。

## 八、2026-11-02 主代理实测

真实端点为七牛 MiniMax M2.7，所有资料均为夹具。为避免 RPM 限制，后续运行经本机透明代理，
逐请求至少间隔22秒，仅转发同一七牛端点；报告里的127.0.0.1为代理入口，不是本地模型。
代理/临时配置在tmp，不属于应用运行依赖。七牛DeepSeek V4 Flash返回model_disabled，无正文产出，
不能计为技能写作失败；转DMX的尝试被自动审批拒绝（该目的地未单独授权），没有执行。

v2完整8例原字面结果6/8，但人工查出收发方向、职责、材料冲突和无据推断等问题，
v3–v6是逐项诊断修正的复测，不把多轮结果混成一次质量通过率。最终逐例对账见
`pi-implementation/phase-4d-review.md`，机器可查明细在`tmp/pi-implementation/phase-4d-final-business-review.json`。

收尾仍有两项人工未通过：冲突稿新增材料未要求的措施；研究稿将“资料未记载”推为事实问题，
且混用裸脚注和尾部定义。后者即使关键词达标也不合格，因此追加了行内脚注反例判据。
评测中还修复了材料索引漏写与相对参考路径歧义；修复后实际轨迹确认研究参考文件读取成功。
早期其他案例没有在最终版本整套重跑，不声称最终全套通过。离线单测15项通过。
