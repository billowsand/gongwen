# Pi 写作业务评测夹具

这份目录是 `scripts/pi-writing-eval.mjs` 的自造案例。它证明的是两件事，且只证明这两件：

- **资料源是夹具**：知识库、稿件库、词库、风格档案、表单要素都由 `bridge.json` 与
  `cases/` 下的自造文本回答，没有连接真实内网知识库、用户配置、稿件或风格库。
- **Pi host 与模型是真实执行**：脚本启动 `pi-host/src/main.mjs`，用真实的标准技能
  （`assets/pi-skills/`）与完整语法包（`skills/gongwen-markdown/`）搭工作区，
  受真实 host 的交付、回执、路径与提问限制，不使用假 provider、不直接让模型回一段正文。

所以评测结论只说明「在这几个自造任务上，模型 + 真实 host + 标准技能」的表现，
不代表内网知识库联机效果，也不代表写作质量总分。

本夹具的 `bridge.json` 一直是 JavaScript 模拟资料，不是 Rust 检索证据。真实临时 SQLite/FTS
资料链由针对性 Rust 用例 `pi_tool_context_uses_real_temporary_sqlite_knowledge_chain` 覆盖；
该用例的 embedding loopback 端点故意不可用，因此只证明 SQLite/FTS 与 Pi 工具包装接通，
不证明真实 embedding / rerank 服务。

## 案例

| 名称 | 考什么 |
| --- | --- |
| `draft-from-material` | 按会议纪要起草通知，保留材料里的时间、数字与单位 |
| `condense-selection` | 只改选区、按字数精简，任务 / 责任单位 / 时限不丢，选区外逐字不变 |
| `reply-unknown-stance` | 复函：主送认来函落款、逐项答复、用户没给意见的事项不替用户表态 |
| `material-conflict` | 两份材料时限冲突：两个值都留下并标待核实，不悄悄二选一 |
| `policy-source-missing` | 政策依据核对：知识库没有原文时必须说「未找到原文」，不得说已核实 |
| `extract-only` | 只提炼要点：走 `finish_report`，不改正文 |
| `research-sources` | 研究报告：关键判断有出处、不编文献（没有文献库时不写 `[@key]`） |
| `style-apply` | 按风格档案写法成文，风格/样稿里的单位、日期、数字不得搬进正文 |
| `phased-multi-source` | 强制分阶段：两份事实材料逐块提炼、提纲、逐节、合稿，最后只交一份完整成品；宿主核对 `progress.json` 登记与引文 |

## 判据

`cases.json` 里每个案例的 `expect` 分三类：

- `delivery`：`document`（必须 `submit_document`）、`report`（必须 `finish_report`）。
- `strategy`：可选 `"phased"`。带这个字段的案例会向 host 传显式分阶段倾向，并额外核对
  `strategy` 事件、`progress.json` 里的材料 / 提纲 / 章节 / 合稿登记；不能只看最终提交。
- 技术判据（脚本自动、对所有案例）：进程正常退出、`completed` 事件、回执与哈希、
  产物快照、选区外不变、工具调用没有未知工具或失败。
- 业务判据（按案例声明）：`required` 必须出现的事实、`forbidden` 不得出现的样稿事实、
  `requirePhrasesAny` 至少命中一组的说法、`requiredToolsAny` 必须调过的工具。
  `reportOnly` 等价于 `delivery: report` 加「正文未变」。

关键词命中只回答「指定事实在不在」；写作质量（结构、语气、得体、可用性）由人工按
`manual` 列表逐项看，不折算成分数，也不与关键词通过率相加。数字与日期紧邻的空白按等价
处理（「10 月 8 日」=「10月8日」），但字数与选区上限始终按交付原文计数，不因归一化放宽。

## 运行

```powershell
node scripts/pi-writing-eval.mjs --list
node scripts/pi-writing-eval.mjs --selftest
node scripts/pi-writing-eval.mjs --case draft-from-material `
  --model-config tmp/pi-eval-model.json --out tmp/pi-writing-eval/manual-1
node scripts/pi-writing-eval.mjs --check tmp/pi-writing-eval/manual-1
```

`--model-config` 由跑评测的人临时提供，形如：

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

脚本**不读** `.env`、`.pi/auth`、真实应用配置或外部测试密钥文件；没有显式配置就拒绝
跑模型。运行目录只写脱敏的模型标识、任务、工具轨迹与产物，不保存带密钥的配置，也不
保存 init 原文。

## 离线自测

`scripts/pi-writing-eval.test.mjs` 用 Node 内置测试跑判据与 CLI 的离线单测，不连模型：

```powershell
node --test scripts/pi-writing-eval.test.mjs
```
