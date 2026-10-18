# 公文助手技能包模型评测报告

- 评测对象：`skills/gongwen-markdown/`（给外部大模型用的 Agent Skill），检验**中等规模模型照着技能文档起草的 Markdown，能否被公文助手的解析器与自检脚本接受**。
- 评测日期：2026-10-18（Asia/Shanghai）；第三轮 v5 复测跨到 10-19 凌晨（七牛当日 token 配额重置后补跑）。
- 评测脚本：`scripts/gongwen-skill-eval.py`（本仓库新增，可复用；支持断点续跑、`--repeats` 方差观测、`rescore` 同尺重评、`compare` 多轮对比）。
- 判定脚本：`skills/gongwen-markdown/scripts/check_gongwen_md.py`，sha256 `b08db644166f56a8e5de400f9df405c58293fdf1c694c8cc74b39213fdc5d907`（前 16 位 `B08DB644166F56A8`），每轮评分前 `--selftest` 均为 12/12。**判定口径全程未放宽。**
- 一句话结论：技能包从 HEAD 的 **83.3%** 逐轮提到 v5 的 **97.2%**（同一 36 次矩阵、同一判定口径），改善全部来自 `minimax-m2.7`（手写编号 → 附件1 → 数字+空格 → 未闭合围栏 → 序号表缺表头，逐轮修掉）；`deepseek-v4-flash` 与 `qwen3.8-flash-next` 基线即满分。**v5 修掉了最后一个可归因于文档的靶点，剩余 1 例是间歇性模型滑落**；另有 4 个厂商模型（GLM / Step / 豆包 / Qwen3.7）在条件 A 下 **24/24 全过**。

> 状态标注：**v5 为交付版，主矩阵 36 次全部跑完**（35 通过、1 失败、0 接口失败），18 次靶点重复轮 **18/18 全过**。第三轮跨到 10-19 凌晨：七牛当日 token 配额（TPD）耗尽后，3 次主矩阵调用与 18 次重复轮全部是 HTTP 429，配额重置后由断点续跑补齐，**429 样本一律不计入通过率**，重跑后无接口失败。

---

## 1. 结论摘要

### 1.1 五轮主矩阵（七牛 qny，3 模型 × 6 文种 × A/B，各 36 次）

| 指标 | before(HEAD) | after(v2) | after3(v3) | after4(v4) | after5(v5，交付版) |
| --- | ---: | ---: | ---: | ---: | ---: |
| 通过（0 ERROR） | 30/36 | 32/36 | 33/36 | 34/36 | **35/36** |
| 通过率 | 83.3% | 88.9% | 91.7% | 94.4% | **97.2%** |
| ERROR 合计 | 24 | 8 | 7 | 2 | **2** |
| WARN 合计 | 6 | 0 | 1 | 6 | **0** |
| 接口失败 | 0 | 0 | 0 | 0 | **0**（补跑后） |
| 技能树 sha256 前 12 位 | `c1ca346d5db5` | `bff8e2291d0b` | `500786ac693a` | `48aae0229d94` | `2ab1b8b46ac9` |

### 1.2 按模型（各轮通过/成功（ERROR））

| 模型 | before | v2 | v3 | v4 | v5 |
| --- | --- | --- | --- | --- | --- |
| `deepseek/deepseek-v4-flash` | 12/12（0） | 12/12（0） | 11/12（2） | 12/12（0） | 12/12（0） |
| `minimax/minimax-m2.7` | 6/12（24） | 8/12（8） | 10/12（5） | 10/12（2） | 11/12（2） |
| `qwen/qwen3.8-flash-next` | 12/12（0） | 12/12（0） | 12/12（0） | 12/12（0） | 12/12（0） |

按条件：v5 条件 A **17/18**、条件 B **18/18**（v4 为 16/18 与 18/18）。

### 1.3 方差观测（v5 重复轮，条件 A，每格 3 次独立采样）

| 靶点 | v4 的 4 次采样 | v5 的 3 次采样 |
| --- | --- | --- |
| `minimax` · plain-document | **4/4 失败**（缺表头/分隔行） | **3/3 通过** |
| `minimax` · research-report | 2/4 失败（`>` 包表） | **3/3 通过** |
| `deepseek` · plain-document | 4/4 通过 | 3/3 通过 |
| `deepseek` · research-report | 3/4 通过（1 次未闭合围栏） | 3/3 通过 |
| `qwen` · plain-document / research-report | 全部通过 | 全部通过 |

合计 `after5-repeats-20261018`（18 次）**18/18 通过**。

### 1.4 专项模式（文档数；箭头为 before → v2 → v3 → v4 → v5）

| 失败模式 | before | v2 | v3 | v4 | v5 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 标题手写编号 | 6 | 1 | **0** | 0 | 0 |
| 附件标题写“附件1” | 2 | **0** | 0 | 0 | 0 |
| 标题以“数字+空格”开头 | 0 | 1 | **0** | 0 | 0 |
| 代码围栏未剥净 / 未闭合围栏 | 0 | 0 | 1 | **0** | 0 |
| 表格缺少合格分隔行 | 0 | 2 | 1 | 1 | **1** |
| 表格列数不一致 | 0 | 0 | 1 | 1 | **0** |

- 手写编号按模型下钻：before 仅 `minimax`（条件 A 4、条件 B 2），v3 起归零。
- 附件1 标题：before 为 `minimax` 条件 A 1 + `deepseek` 条件 A 1，v2 起归零。
- 数字+空格标题：v2 出现 1（`minimax` plain-document A），v3 文档补规则后归零。
- 未闭合围栏：v3 出现 1（`minimax` research-report B），v4 九错表 + 默查后 4/4 采样全过。
- 表格分隔行：v3 起集中在 `minimax` research-report A / plain-document A；v5 序号表示例把 plain-document A 修掉（重复轮 3/3），research-report A 在重复轮 3/3 通过、主矩阵偶发 1 次（见 §5）。

### 1.5 改善归因（如实写）

- `deepseek` 与 `qwen` 在 HEAD 基线就是 12/12，**所有轮次的通过率提升都来自 `minimax-m2.7`**；报告不写成「全部模型都从失败变成功」。
- `minimax` 的五类错误分别由 v2（手写编号/附件1/版头版记）、v3（数字+空格）、v4（未闭合围栏）、v5（序号表缺表头）逐轮修掉；v5 之后剩余的是研究报告里“把表格包进 `>` 引用块”的**间歇性模型滑落**（同族模型在 dmxapi 通道 3/3 通过，见 §5.3）。

---

## 2. 方法

### 2.1 目标与判定

1. **清洗**：严格复刻 `src/prompt.rs::sanitize_model_markdown` —— 剥思考块（` thinking`、`<thinking>`）、剥最外层代码围栏（只认空信息串与 `markdown`/`md`/`text`）、剥首尾寒暄。**不修正任何格式**，否则测不出模型的错。
2. **判定**：`python skills/gongwen-markdown/scripts/check_gongwen_md.py <clean.md> --kind <文种>`，记录每条 ERROR/WARN 与退出码；脚本异常单独记 `check-error`。
3. **失败模式分类**：对 check 的 ERROR 逐条归因，再加一层客观文本判据（首行、围栏残留、手写编号、附件区段缺失、表格判据、相对日期等）；同一条问题只计一次，模式按「文档数」计数。模式级别区分 ERROR 级与 WARN 级（例如「附件1 标题」check 只给 WARN，不计入 ERROR 通过率）。
4. **同尺重评**：判定口径冻结后用 `rescore` 对历史 run 的**已保存清洗稿**重跑判定与分类（不重新调模型、不重新清洗、不改原始输出），保证五轮数字可比。重评只改判定字段与严重级别。
5. Lead 另用 Rust 侧 `parse_markdown` + `validator::validate` 独立复验了 v2/v4 的产物，逐例与 check 结论一致。

### 2.2 模型与调用参数

| 提供商 | 模型全名 | 用途 |
| --- | --- | --- |
| qny `https://api.qnaigc.com` | `minimax/minimax-m2.7`、`qwen/qwen3.8-flash-next`、`deepseek/deepseek-v4-flash` | 五轮主矩阵（各 36 次） |
| qny | `z-ai/glm-5.3-flash`、`stepfun/step-3.7-flash`、`bytedance/doubao-seed-2-1-turbo`、`qwen/qwen3.7-flash` | 跨厂商补充验证（v4 树，条件 A） |
| dmxapi `https://www.dmxapi.cn` | `MiniMax-M2.7`、`deepseek-v4-flash` | 交叉验证；v5 靶点独立复核 |
| siliconflow `https://api.siliconflow.cn` | `deepseek-ai/DeepSeek-V4-Flash`、`Qwen/Qwen3.8-27B` | 交叉验证 |

- `temperature=0.3`，非流式；`max_tokens=32768`。推理型模型若把预算全花在 reasoning 且 `finish_reason=length`，脚本自动放大到 65536 重试一次并记录 `max_tokens_used`（stepfun 的 research-report 实际触发了这条路径并成功，未被记成空输出）。
- HTTP 全部走 Python `urllib` + `ssl`（本机 schannel 的 curl/PowerShell TLS 会失败）。按提供商限并发并拉开间隔，429/5xx 退避重试；**超出重试的 429/超时/空输出记为 `api-error`，不计入模型写错，也不进通过率分母**。
- 记录 `prompt_tokens`、`completion_tokens`、`reasoning_tokens`、`total_tokens`、耗时、`finish_reason`。

### 2.3 两种上下文条件

- **条件 A**：system 只给 `SKILL.md`（模拟只能读到技能正文、没有文件读取能力的接入方式）。
- **条件 B**：system 给 `SKILL.md` + `references/document-types.md` + `references/markdown-syntax.md` + 该文种模板；研究报告另加 `references/research-report.md`。
- 每个文种的 user 消息固定（素材 + 写作要求），A/B 只差 system。

### 2.4 六个固定用例

| 文种 | 素材要点 |
| --- | --- |
| `official-letter` | 公函商请共享地下管线数据；含两份附件全文（需求清单表 + 时间安排表）；要求正文引用“见附件1/2” |
| `plain-document` | 政务数据资源共享开放工作要点；含一张序号表（两组 6 项）与一张统计表（3 行数据） |
| `meeting-agenda` | 冬季供水保障专题会议；时间地点、参加人员、4 项议程 |
| `white-paper` | 追加供水管网改造专项资金的请示；要求段内枚举“一是……；二是……”，不得用列表 |
| `phone-notice` | 防冻保温电话通知；三项要求 + 时间节点 + 报告渠道 |
| `research-report` | 供水管网漏损治理研究；要求区段标记、锚点与交叉引用、一张漏损率表、不给文献库 |

### 2.5 技能树冻结与方差观测

技能包在评测期间被多轮修改，因此**每轮 run 使用一份冻结快照**（`--skill-dir`），run 目录内另存 `skill-snapshot/`（逐文件内容）与 `SOURCES.json`（逐文件 sha256）。

| 树 | 目录 | 树哈希 `skill_tree_sha256` | 内容 |
| --- | --- | --- | --- |
| before | `tmp/gongwen-skill-eval/skill-before` | `c1ca346d5db5e599eb9136a6a278ef9519afc1508ffa5c956500c25725f48ee8` | `git HEAD` 版 |
| after(v2) | `tmp/gongwen-skill-eval/skill-after` | `bff8e2291d0bab99fedbb9272161d5edc9f5ba0a935cd252e2149da0e7ba8bec` | 10-18 11:19 定稿 |
| after3(v3) | `tmp/gongwen-skill-eval/skill-after3` | `500786ac693a4d0078dcf1012f9cb49e0b25f05b98516bba35f1feae6c407720` | 10-18 11:57 定稿 |
| after4(v4) | `tmp/gongwen-skill-eval/skill-after4` | `48aae0229d94745beee571ac333ff5c9f7479009c82bf9e6e54ee6066b9b37da` | 10-18 20:57 定稿 |
| after5(v5，交付版) | `tmp/gongwen-skill-eval/skill-after5` | `2ab1b8b46ac91e2c73992042f7cbb4847b52add0af7ba161e9d673eb11124e74` | 10-18 21:49 定稿，之后冻结 |

关键文件 sha256 前 12 位（完整表见附录 B）：

| 文件 | before | v2 | v3 | v4 | v5 |
| --- | --- | --- | --- | --- | --- |
| `SKILL.md` | `7A0DDDEF599C` | `6316EA98F87C` | `1D5DA9D8A11D` | `AFA7A27B1A8B` | `FAE94FDD61C7` |
| `references/document-types.md` | `91103C6C2BEA` | `637C0E998F79` | 同 v2 | 同 v2 | 同 v2 |
| `references/markdown-syntax.md` | `9BD0742FE68E` | `B8A351833509` | `FE0391E79E2C` | `27290A66500E` | 同 v4 |
| `references/research-report.md` | `BC8388930DEB` | `7680FA5A3C2D` | 同 v2 | 同 v2 | 同 v2 |
| `references/writing-style.md` | `56317D47BCCB` | `7BC2A85B2A92` | 同 v2 | 同 v2 | 同 v2 |
| `scripts/check_gongwen_md.py` | `150FC2F8AC70` | `3243E57731B4` | `B08DB644166F` | 同 v3 | 同 v3 |

**方差观测设计**：`--repeats N` 让每个用例在本次 run 内独立采样 N+1 次（文件后缀 `-r1`…`-r(N+1)`，`r1` 也是新调用）。对三个靶点各做「主矩阵 1 次 + 重复轮 3 次 = 4 次独立采样」：

- run `after4-repeats-A-20261018`：`research-report × A` 与 `plain-document × A` × 3 模型，`--repeats 2`（18 次）。
- run `after4-repeats-B-20261018`：`research-report × B × minimax`，`--repeats 2`（3 次）。
- run `after5-repeats-20261018`：v5 树的同两个条件 A 靶点（18 次；首次 18 次全被 429 挡住，配额重置后重跑）。

### 2.6 复现命令

```powershell
# 0) 密钥：脚本从 D:\gongwen\model_api_test.md.txt（已 gitignore）或环境变量读，仓库里没有任何密钥
python scripts/gongwen-skill-eval.py smoke

# 1) 主矩阵（每个 run 36 次：3 模型 × 6 文种 × A/B）
python scripts/gongwen-skill-eval.py run --matrix baseline --run-id after5-20261018 `
    --skill-dir tmp/gongwen-skill-eval/skill-after5

# 2) 方差观测（同用例重复；每个用例共 3 次采样）
python scripts/gongwen-skill-eval.py run --matrix baseline --run-id after5-repeats-20261018 `
    --skill-dir tmp/gongwen-skill-eval/skill-after5 --conditions A `
    --only-kind plain-document --only-kind research-report --repeats 2

# 3) 跨厂商补充（4 个厂商模型 × 6 文种 × 条件 A）
python scripts/gongwen-skill-eval.py run --matrix vendors --run-id extra-20261018 `
    --skill-dir tmp/gongwen-skill-eval/skill-after4 --conditions A

# 4) 统一口径重评（不重新调模型、不重新清洗）
python scripts/gongwen-skill-eval.py rescore --all

# 5) 多轮对比
python scripts/gongwen-skill-eval.py compare before-20261018 after-20261018 `
    after3-20261018 after4-20261018 after5-20261018 --out tmp/gongwen-skill-eval/compare-5way-qny.md
```

断点续跑：同一 `run-id` 再跑一次 `run`，已有清洗稿直接复用并重判，只补缺失调用（`--force` 才全部重调）。七牛每日 token 配额耗尽时接口返回 429 `UID rate limit reached for TPD`，脚本记为 `api-error` 并可次日续跑。

---

## 3. 第一轮：HEAD → v2 → v3（已定稿）

### 3.1 总体（重评后）

| 指标 | before | after(v2) | after3(v3) |
| --- | ---: | ---: | ---: |
| 调用 / 成功 | 36 / 36 | 36 / 36 | 36 / 36 |
| 通过（0 ERROR） | 30 | 32 | 33 |
| 通过率 | 83.3% | 88.9% | 91.7% |
| ERROR 合计 | 24 | 8 | 7 |
| WARN 合计 | 6 | 0 | 1 |

### 3.2 按文种（通过/成功（ERROR））

| 文种 | before | after(v2) | after3(v3) |
| --- | --- | --- | --- |
| meeting-agenda | 6/6（0） | 6/6（0） | 6/6（0） |
| official-letter | 4/6（8） | 5/6（3） | **6/6（0）** |
| phone-notice | 5/6（3） | 6/6（0） | 6/6（0） |
| plain-document | 5/6（5） | 5/6（1） | 5/6（2） |
| research-report | 5/6（4） | 4/6（4） | 4/6（5） |
| white-paper | 5/6（4） | 6/6（0） | 6/6（0） |

### 3.3 按条件

| 条件 | before | after(v2) | after3(v3) |
| --- | --- | --- | --- |
| A（只给 SKILL.md） | 14/18（17） | 15/18（6） | 16/18（4） |
| B（含 references） | 16/18（7） | 17/18（2） | 17/18（3） |

### 3.4 失败模式（文档数）

| 失败模式 | before | after(v2) | after3(v3) |
| --- | ---: | ---: | ---: |
| 标题手写编号 | 6 | 1 | 0 |
| 标题以“数字+空格”开头 | 0 | 1 | 0 |
| 附件标题写“附件1” | 2 | 0 | 0 |
| 代码围栏未剥净 | 0 | 0 | 1 |
| 首行不是唯一的 `#` 主标题 | 0 | 0 | 1 |
| 表格列数不一致 | 0 | 0 | 1 |
| 表格缺少合格分隔行 | 0 | 2 | 1 |

### 3.5 v2/v3 改了什么（概要，详见 §10 对照表）

- **v2**：SKILL.md 改为自足版（第 0 节六错表、逐文种最小骨架、会议议程唯一例外、附件规则）；references 与 8 个模板由 task-1/2/3 对齐解析器。
- **v3**：SKILL.md 第 0 节补「标题行首一个编号字符都不许有，含数字+空格」，八错表；骨架里的说明文字移出代码围栏；研究报告骨架去编号；`markdown-syntax.md` §2.1 补数字+空格说明；第四级编号改半角 `(1)`。

---

## 4. 第二轮（v4）复测

### 4.1 v4 改了什么（只动两份文档）

| 文件/节 | 改动 | 对应失败 |
| --- | --- | --- |
| `SKILL.md` 第 0 节 | 八错表 → 九错：新增「整篇用 ``` 包起来（尤其只写开头没写结尾）」，写明未闭合单围栏程序剥不掉；默查三条 → 四条（加表格分隔行格数、围栏） | v3 的 `minimax · research-report · B` 未闭合围栏（3 ERROR） |
| `references/markdown-syntax.md` §6.1 | 补**两列最小表**正例，写明「分隔行格数必须等于表头格数，只写 `\| ---: \|` 一格整表按正文印出」 | v3 的 `minimax · research-report · A` 两列漏损率表分隔行 |
| `references/markdown-syntax.md` §6.4 | 序号表分组行补 ✗/✓（`\| \|\| 基础支撑组 \|\|\|` ✗ → `\| 基础支撑组 \|\|\|` ✓） | v3 的 `deepseek · plain-document · A` 分组行 5 格 |

### 4.2 主矩阵结果（v4，36 次）

- **34/36（94.4%）**，ERROR 2、WARN 6；条件 A 16/18（2 ERROR）、**条件 B 18/18（0 ERROR）**。
- 按模型：`deepseek` 12/12（0）、`qwen` 12/12（0）、`minimax` 10/12（2）。
- 按文种：meeting-agenda 6/6、official-letter 6/6、phone-notice 6/6、white-paper 6/6、plain-document 5/6（1）、research-report 5/6（1）。
- 失败模式（文档数）：表格列数不一致 1、表格缺少合格分隔行 1；手写编号 / 附件1 / 数字+空格 / 代码围栏均为 0。
- 相对 v3：通过 33 → 34，ERROR 7 → 2；**`minimax` research-report B 的未闭合围栏消失**；`deepseek` 从 11/12 回到 12/12（v3 的 plain-document A 表格错误未复现）。

### 4.3 方差观测（每格 4 次独立采样）

| 靶点（条件 A） | 主矩阵 | 重复 r1 | r2 | r3 | 失败/采样 | 结论 |
| --- | --- | --- | --- | --- | --- | --- |
| `plain-document × minimax` | E1 | E2 | E1 | E1 | **4/4 失败** | 顽固；根因是文档歧义（见 4.4） |
| `research-report × minimax` | E1 | 通过 | 通过 | E1 | 2/4 失败 | 间歇性 |
| `plain-document × deepseek` | 通过 | 通过 | 通过 | 通过 | 0/4 失败 | v3 那次 E2 是**单次噪声** |
| `research-report × deepseek` | 通过 | 通过 | 通过 | E3 | 1/4 失败 | 未闭合围栏的间歇性滑落 |
| `plain-document × qwen` | 通过 | 通过 | 通过 | 通过 | 0/4 失败 | — |
| `research-report × qwen` | 通过 | 通过 | 通过 | 通过 | 0/4 失败 | — |
| `research-report × B × minimax` | 通过 | 通过 | 通过 | 通过 | 0/4 失败 | v4 九错表修掉了未闭合围栏 |

（`E1/E2/E3` = 该次输出被判出的 ERROR 数；重复轮的 `r1` 也是独立调用，不是主矩阵那份文件。）

### 4.4 v4 两个残留的根因（原文证据）

1. `tmp/gongwen-skill-eval/after4-20261018/A/plain-document/qny__minimax-minimax-m2.7.md`：模型把 `<!-- [序号表] -->` 当成“表格本身”，**没写表头行**，直接写分组行 `| 基础支撑组 ||` 再接三格分隔行：
   ```
   <!-- [序号表] -->

   | 基础支撑组 ||
   | --- | --- | --- |
   ```
   check 判「表头列数与分隔行不一致」。**这是文档歧义，不是模型能力问题** —— SKILL.md 旧文只有一句“表前加一行 `<!-- [序号表] -->` 可自动编号、分组”，条件 A 的模型可能理解为标记代替表头。v5 已给完整序号表示例。
2. `tmp/gongwen-skill-eval/after4-20261018/A/research-report/qny__minimax-minimax-m2.7.md`：整张漏损率表被 `>` 包成引用块（`> 表：…`、`> | 年份 | 漏损率（%） |`），解析器当引文处理、表格整张作废。v5 已在研究报告节写明“表格照常写普通管道表、不要用 `>` 包起来”。

---

## 5. 第三轮（v5）复测

### 5.1 v5 改了什么（只动 `SKILL.md`）

| 文件/节 | 改动 | 对应失败 |
| --- | --- | --- |
| `SKILL.md` §3 表格要点 | 「表头行 + `\| --- \|` 分隔行 + 数据行，**三样都不能省**」；新增**序号表完整示例**（表头、分隔行、分组行 `\| 基础支撑组 \|\|\|`、数据行），写明「标记只是修饰」「分组行只有这一种写法」 | v4 的 `minimax · plain-document · A` 缺表头/分隔行 |
| `SKILL.md` 研究报告节 | 表格照常写普通管道表、**不要用 `> ` 包起来**；表题写表格正上方单独一行「表：题名」 | v4 的 `minimax · research-report · A` 表格被包进引用块 |
| `SKILL.md` 默查第 ③ 条 | 扩成「表头行、分隔行、数据行是不是都写了（标了序号表的也一样）」 | 同上 |

### 5.2 主矩阵结果（36 次全部跑完）

- **35/36 通过 = 97.2%**，ERROR 2、WARN 0、接口失败 0。第三轮先被七牛当日 TPD 配额挡住（3 次 `deepseek` 调用为 HTTP 429，单列不计）；配额重置后断点续跑补齐，补齐后这 3 次**全部通过**。
- 按模型：`deepseek` 12/12、`qwen` 12/12、`minimax` 11/12（2 ERROR）。
- 按条件：A **17/18**、B **18/18**。
- **v5 生效证据**：`minimax · plain-document · A` 从 v4 的 E1 变为 **E0**，且重复轮 3/3 通过（v4 时该靶点 4/4 失败）；`minimax · research-report · B` 继续保持 E0。
- 仍在失败的 1 例：`tmp/gongwen-skill-eval/after5-20261018/A/research-report/qny__minimax-minimax-m2.7.md` — 2 ERROR + 0 WARN：
  - `第25行 交叉引用 {@tbl:loss} 找不到对应的锚点 {#tbl:loss}`（锚点被写进被 `>` 包住的表题里，或未落在行尾）；
  - `第29行 这一组含 | 的行不构成合格表格…`（整张表仍被 `>` 包成引用块，并在块内插了 `<!-- [漏损率表格] -->`）。
  - 性质：**间歇性模型滑落**，不是文档没写。同一靶点：v5 重复轮 3/3 通过、dmxapi 同族 `MiniMax-M2.7` 3/3 通过（见 5.3），主矩阵这一次是 6 次采样里的唯一一次失败。

### 5.3 另一通道独立复核（Lead 提供，本机同一判定脚本复验）

Lead 用 **dmxapi 通道**、冻结的 v5 prompt（`after5-20261018/prompts/*-A.md`）复现两个靶点，产物在 `tmp/lead-confirm-v5/`。本机用同一把尺子（`check_gongwen_md.py`，sha `B08DB644…`）逐份复验：

| 靶点（条件 A） | provider / 模型 | 重复 | check 结果 |
| --- | --- | ---: | --- |
| `plain-document` | dmxapi `MiniMax-M2.7` | 3 | **3/3 通过（0 ERROR、0 WARN）** |
| `research-report` | dmxapi `MiniMax-M2.7` | 3 | **3/3 通过（0 ERROR、0 WARN）** |
| `plain-document` | dmxapi `deepseek-v4-flash` | 1 | 通过（0 ERROR、0 WARN） |

结论：v5 的序号表修复在另一通道同样成立；七牛 `minimax` research-report A 的 `>` 包表问题**换通道不复现**，与方差观测（4 次里 2 次失败）方向一致 → 归为**间歇性模型滑落**，不再归因文档。

### 5.4 靶点重复轮（配额重置后补齐）

| 靶点（条件 A） | v4 采样 | v5 采样（`after5-repeats-20261018`） |
| --- | --- | --- |
| `minimax` · plain-document | 4/4 失败 | **3/3 通过** |
| `minimax` · research-report | 2/4 失败 | **3/3 通过** |
| `deepseek` · plain-document | 4/4 通过 | 3/3 通过 |
| `deepseek` · research-report | 3/4 通过 | 3/3 通过 |
| `qwen` · plain-document / research-report | 全部通过 | 各 3/3 通过 |

合计 **18/18 通过（0 ERROR、0 WARN）**：v5 的两个靶点修复在重复采样下站得住，v4 里顽固的 `minimax · plain-document · A` 已彻底消失。补齐命令与配额说明见 §2.6 与附录 A。

---

## 6. 跨厂商补充验证（v4 树，条件 A）

目的：把「中等规模模型」从 3 个扩到 7 个厂商模型，验证技能包不是只对七牛那三个模型有效。使用 **v4 树**（`48aae0229d94`）与同一判定脚本；run `extra-20261018`，4 模型 × 6 文种 × **条件 A** = 24 次。

| 模型（qny） | 通过/成功 | ERROR | WARN | 备注 |
| --- | --- | ---: | ---: | --- |
| `z-ai/glm-5.3-flash`（智谱） | 6/6 | 0 | 0 | 推理型；32768 预算下正常出正文 |
| `stepfun/step-3.7-flash`（阶跃） | 6/6 | 0 | 1 | research-report 首次 reasoning 占满 32768，放大到 65536 后成功（脚本自动重试，未记空输出）；WARN 为半角引号 |
| `bytedance/doubao-seed-2-1-turbo`（豆包） | 6/6 | 0 | 0 | — |
| `qwen/qwen3.7-flash`（通义上一代） | 6/6 | 0 | 0 | — |
| **合计** | **24/24（100%）** | 0 | 1 | 接口失败 0 次 |

结论边界：这是**条件 A、v4 树、单次采样**的结果，只能说明“这四个厂商模型在只读 SKILL.md 时能一次写对本次六个用例”；条件 B（更长上下文）未跑，不做外推。

---

## 7. 判定口径变化（#1~#4，仪器变化，不是模型变好）

判定脚本在评测期间改过四处。报告数字全部用 `rescore` 以最终口径重算，原始输出与清洗稿未改动。

| # | 变化 | 影响 |
| --- | --- | --- |
| #1 | 手写编号前缀与 `src/export/headings.rs` 的 10 条逐条对齐 | `第三部分部署`（中间无空格）从此该报、`第1部` 不报；`^\d+\s+` 使“标题以数字+空格开头”可判 |
| #2 | 表格判据改用解析器口径：含 `\|` 且 ≥2 格才算行；分隔行每格 ≥3 短横；省略首尾竖线也校验列数；围栏内不算 | 比以前严 |
| #3 | 新增 `check_gongwen_md.py --selftest`（12 条判据自测） | 每轮评分前先证明仪器可用，本报告所有 run 均 12/12 通过 |
| #4 | 「缺合格分隔行」分两级：一组多行（或下一行像分隔行）→ **ERROR**；孤立单行含 `\|` → WARN | 原先一律 WARN。理由：不合格分隔行会让整张表按正文印出（纸上印竖线），是真缺陷 |

**重评影响（如实写）**：`after(v2)` 从 34/36 降为 **32/36**（研究报分隔行由 WARN 升 ERROR，口径变严）；`before` 30/36 不变；v3/v4 本身已在最终口径下运行，重评无变化。`after(v3)` 的 7 ERROR 里有 2 条是 #4 从 WARN 升级而来。**没有任何一轮因为口径变化而“变好”。**

---

## 8. 交叉验证（v2/v3 通道，历史存档）

| run | 技能树 | 调用 | 成功 | 通过 | 通过率 | ERROR | 备注 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| `after-cross-20261018` | v2 | 48（4 模型 × 6 文种 × A/B） | 43 | 41 | 95.3% | 5 | 5 次 siliconflow Qwen 超时（api-error）；ERROR 全是 dmxapi `MiniMax-M2.7` |
| `after3-cross-20261018` | v3 | 24（4 模型 × 6 文种 × A） | 22 | 22 | 100% | 0 | 2 次 siliconflow Qwen 超时 |

- dmxapi `MiniMax-M2.7` 在 v2 交叉里的 ERROR 与七牛 `minimax/minimax-m2.7` 同源（official-letter B 手写编号 3、research-report A 分隔行 2）；v3 条件 A 复跑 6 个文种全部 0 ERROR。
- siliconflow `Qwen/Qwen3.8-27B` 多次在 120–240 秒超时（同一模型在七牛 12/12 满分），交叉样本不完整，不影响主矩阵结论。

---

## 9. 残留失败清单与状态（累计）

| 轮次 | 用例 | ERROR | 状态 | 文件 |
| --- | --- | ---: | --- | --- |
| v3 | `minimax · research-report · B` 未闭合围栏 | 3 | **v4 已修**（4/4 采样通过） | `after3-20261018/B/research-report/qny__minimax-minimax-m2.7.md` |
| v3 | `minimax · research-report · A` 两列分隔行 | 2 | v4/v5 仍有间歇；换通道 3/3 通过 | `after3-20261018/A/research-report/…` |
| v3 | `deepseek · plain-document · A` 序号表分组行 5 格 | 2 | **v4 起未复现**（噪声） | `after3-20261018/A/plain-document/qny__deepseek-deepseek-v4-flash.md` |
| v4 | `minimax · plain-document · A` 缺表头/分隔行 | 1（4/4 采样失败） | **v5 已修**（E0；dmxapi 同族 3/3） | `after4-20261018/A/plain-document/qny__minimax-minimax-m2.7.md` |
| v4 | `minimax · research-report · A` 表格被 `>` 包住 | 1（2/4 采样失败） | v5 仍有 1 例（间歇） | `after4-20261018/A/research-report/…` |
| v5 | `minimax · research-report · A` 表格被 `>` 包住 + 交叉引用锚点丢失 | 2 | 重复轮 **3/3 通过**、dmxapi 同族 3/3 通过 → **间歇性模型滑落**（6 次采样里 1 次） | `after5-20261018/A/research-report/qny__minimax-minimax-m2.7.md` |

v5 的 3 次 429 api-error（`deepseek` official-letter A / phone-notice B / research-report B）**不是失败用例**；配额重置后补齐，3 次全部通过，接口失败归零。

---

## 10. 每轮「改了哪份文档哪一节 → 对应哪条失败」对照（核心）

| 轮次 | 改的文件/节 | 改动 | 目标失败（证据来自上一轮） | 效果 |
| --- | --- | --- | --- | --- |
| v2 | `SKILL.md` 第 0 节 + 各文种最小骨架 + 附件规则；references/document-types、markdown-syntax、8 个模板 | 自足化；六错表；会议议程“唯一允许手写一、二、三”；附件标题不带“附件1” | `minimax` 手写编号（before 6 个文档）、附件1（2 个文档）、版头版记 | 手写编号 6→1、附件1 2→0；通过 30→32 |
| v2 | `references/research-report.md` 结构示例 | 去掉 `## 第一章标题` 示例污染，改裸标题 | `minimax · research-report · B` 的 `### 一、` 手写编号 | 条件 B 手写编号 2→0 |
| v3 | `SKILL.md` 第 0 节 + 默查；骨架说明文字移出围栏；research 骨架去编号 | 数字+空格标题规则；八错表 | v2 的 `minimax · plain-document · A`「`### 2026 年第三季度…`」 | 数字+空格 1→0；手写编号 1→0；通过 32→33 |
| v3 | `references/markdown-syntax.md` §2.1 | 「数字+空格开头的标题也会被剥，四位年份也不例外」 | 同上 | 同上 |
| v4 | `SKILL.md` 第 0 节（八错→九错、默查加围栏） | 未闭合单围栏程序剥不掉 | v3 的 `minimax · research-report · B` 未闭合 ` ```markdown ` | 该靶点 4/4 采样通过；通过 33→34 |
| v4 | `references/markdown-syntax.md` §6.1 | 两列最小表正例 + 分隔行格数规则 | v3 的 `minimax · research-report · A` 两列表分隔行 | 部分（v4 仍间歇） |
| v5 | `SKILL.md` §3 表格要点 | 序号表完整示例、三样不得省 | v4 的 `minimax · plain-document · A` 缺表头/分隔行 | **已修**：主矩阵 E0 + 重复轮 3/3；通过 34→35 |
| v5 | `SKILL.md` 研究报告节 | 表格不要用 `>` 包、表题单独一行 | v4 的 `minimax · research-report · A` `>` 包表 | 显著减少：重复轮 3/3 通过、主矩阵 6 次采样里 1 次仍失败（间歇） |
| v5 | `SKILL.md` 默查第 ③ 条 | 表头/分隔行/数据行都要写 | 同上 | 同上 |
| v4 | `references/markdown-syntax.md` §6.4 | 分组行 ✗/✓ 反例 | v3 的 `deepseek · plain-document · A` 分组行 5 格 | 噪声，v4 未复现（4/4 通过） |
| v5 | `SKILL.md` §3 表格要点 + 序号表完整示例 | 表头/分隔行/数据行三样不能省；分组行只有一种写法 | v4 的 `minimax · plain-document · A` 缺表头（4/4 失败） | **v5 主矩阵 E0**；dmxapi 同族 3/3 通过 |
| v5 | `SKILL.md` 研究报告节 + 默查第③条 | 表格不要用 `> ` 包起来；表题单独一行 | v4 的 `minimax · research-report · A` 表格被包进引用块 | 间歇：v5 主矩阵 1/1 失败、重复轮 3/3 通过、dmxapi 同族 3/3 通过（6 次采样里 1 次） → 判为模型滑落 |

---

## 11. 下一轮改法建议与遗留风险

技能包已由仓库所有者定为 **v5 最终版**（`SKILL.md` sha 前 12 位 `FAE94FDD61C7`），本轮之后不再改文档。因此下面只列「如果还有下一轮」的可执行项，并说明为什么现在不做：

1. **不再改文档**：v5 已给全 ✗/✓ 反例（未闭合围栏、数字+空格标题、两列最小表、序号表分组行、完整序号表示例、不要用 `>` 包表）。剩余失败是模型没有遵循已经写明的规则，继续加文字边际收益低。
2. 若未来要覆盖更小的模型，可考虑：
   - `markdown-syntax.md` §6 把「表格三样不能省」再提前到 §1 之前的速查位置（目前已在 SKILL.md §3，条件 A 可读到）；
   - 在 `SKILL.md` 研究报告节给一份**完整可抄的研究报告表格块**（表题 + 表头 + 分隔行 + 数据行 + 正文引用），比单句“不要用 `>` 包”更抗遗忘；
   - 这些都是「提醒强度」改进，不能保证消除模型滑落，报告不据此承诺通过率。
3. **不放宽判定**：口径 #4 的严格化已被 Rust 侧解析器复验逐例印证（不合格分隔行确实整表降级），任何通过率提升都必须来自模型/文档，不能来自放水。
4. v5 补跑已完成（主矩阵 36/36、重复轮 18/18 全部有结果，0 接口失败）；仍失败的 1 例原始文件保留在 `after5-20261018/A/research-report/`，供 Rust 侧复验；不为了数字好看重跑挑选结果。

---

## 12. 结论边界

**有实测支撑**

- 五轮主矩阵（七牛 3 模型 × 6 文种 × A/B）：通过率 83.3% → 88.9% → 91.7% → 94.4% → **97.2%（v5，35/36）**；ERROR 24 → 8 → 7 → 2 → 2；条件 B 从 v4 起已 18/18。
- v5 重复轮 18/18 通过（条件 A 的两个历史靶点各 3/3），说明 v5 的两处文档修复在重复采样下站得住；v5 主矩阵 36 次全部跑完，**0 接口失败**。
- 改善全部来自 `minimax-m2.7`；`deepseek`/`qwen` 基线即满分，不是「所有模型都从失败变成功」。
- 逐轮归因（§10）：每一类被修掉的失败都能对应到具体文档改动与上一轮原文证据。
- 方差观测：v4 每格 4 次采样——`minimax plain-document A` 4/4 失败（文档歧义，v5 修）、`minimax research-report A` 2/4 失败（间歇）、`deepseek plain-document A` 0/4 失败（v3 是噪声）、`minimax research-report B` 0/4 失败（v4 修复）；v5 每格 3 次采样全部通过。
- 跨厂商（v4 树、条件 A）：4 个新厂商模型 24/24 全部通过。
- dmxapi 独立复核（同一把尺子）：v5 两个靶点 3/3、3/3 通过，说明七牛 research-report A 的残留换通道不复现。
- Rust 侧独立复验（Lead）：v2/v4/v5 产物与 check 结论逐例一致（v5：33 份里 32 通过、唯一失败与 check 同一条）。

**只是文档一致性论证、没有模型实测**

- 8 个模板与 references 中未进入本次 6 个用例的文种（红头呈批件、电话记录单、联合发文、横页等）只有静态核对。
- §11 的下一轮建议基于 1~2 个观测样本，属“值得一试”，不是统计显著结论。

**方法学限制**

- 每个 `(模型, 文种, 条件)` 单元默认 1 次采样（temperature 0.3），存在单次波动；v4 起对关键靶点补到 4 次采样，才把噪声与顽固区分开。
- 条件 A/B 的 system 长度差异大，条件间对比同时受信息量与上下文长度影响。
- v5 主矩阵先有 3 次因七牛**当日 TPD 配额**（HTTP 429）失败、重复轮 18 次全部被配额挡住；配额重置后断点续跑补齐，**429 样本一律不计入通过率**，补齐后 0 接口失败（§5.4、附录 A）。
- siliconflow `Qwen/Qwen3.8-27B` 多次超时，v2/v3 交叉样本不完整。
- 评测只覆盖「能否通过解析器与自检脚本」，不评价文风、事实准确性与编造情况。

**一处更正（与本次数字无关）**：此前口述过「白头件附件在 Word/Typst 两条链不一致」，核对代码后确认**两条链表现一致**（`src/export/docx.rs` 照排附件，`assets/typst/gongwen.typ` 的 `letter-like` 同样 `for a in doc.attachments`；typst 只在会议议程丢弃附件）。真正存在的是「起草规则说白头件不带附件、渲染链对粘贴进来的附件照排」的宽渲染/严起草分工，属正常设计，本轮未改 `src/`。

---

## 13. 产物与目录

```
tmp/gongwen-skill-eval/                     # 已 gitignore
├── skill-before/  skill-after/  skill-after3/  skill-after4/  skill-after5/   # 五份冻结技能树
├── before-20261018/  after-20261018/  after3-20261018/  after4-20261018/  after5-20261018/
├── after-cross-20261018/  after3-cross-20261018/          # dmxapi/siliconflow 交叉
├── after4-repeats-A-20261018/  after4-repeats-B-20261018/ # v4 方差观测
├── after5-repeats-20261018/                              # v5 方差观测（18 次，配额重置后补齐）
├── extra-20261018/                                       # 跨厂商补充（v4 树、条件 A）
├── mixed-20261018/                                       # 中途换文档的排查轮（不计入对比）
└── compare-*.md
tmp/lead-confirm-v5/                        # Lead 用 dmxapi 复现 v5 两个靶点的产物
```

每个 run 目录：

```
manifest.json          # {"run","created","skill_dir","skill_tree_sha256","cases":[...]}
summary.tsv / summary.json   # 逐次结果（含 repeat）、聚合、失败模式、重点模式下钻
report.md              # 该 run 的自动报告
prompts/<kind>-<A|B>.md      # 实际 system + user
skill-snapshot/        # 本轮使用的技能文件副本 + SOURCES.json（逐文件 sha256）
<A|B>/<kind>/<provider>__<model>[-rN].md / .raw.md
```

- `manifest.json` 顶层含 `skill_dir` 与 `skill_tree_sha256`，供 Rust 侧 `model_outputs_survive_the_parser_and_validator` 读取复验。
- 所有数字可用 `compare`、`rescore` 从产物复算；`rescore` 只改判定字段，原始输出与清洗稿未改。

---

## 附录 A：复现与自查命令

```powershell
# 判定脚本自检（12 条）
python skills/gongwen-markdown/scripts/check_gongwen_md.py --selftest

# 单份清洗稿判定
python skills/gongwen-markdown/scripts/check_gongwen_md.py `
  tmp/gongwen-skill-eval/after5-20261018/A/research-report/qny__minimax-minimax-m2.7.md `
  --kind research-report

# 七牛配额恢复后补齐 v5（断点续跑，只补缺失调用）
python scripts/gongwen-skill-eval.py run --matrix baseline --run-id after5-20261018 `
  --skill-dir tmp/gongwen-skill-eval/skill-after5
python scripts/gongwen-skill-eval.py run --matrix baseline --run-id after5-repeats-20261018 `
  --skill-dir tmp/gongwen-skill-eval/skill-after5 --conditions A `
  --only-kind plain-document --only-kind research-report --repeats 2
```

## 附录 B：五份技能树逐文件 sha256（前 12 位）

| 文件 | before | v2 | v3 | v4 | v5 |
| --- | --- | --- | --- | --- | --- |
| `SKILL.md` | `7A0DDDEF599C` | `6316EA98F87C` | `1D5DA9D8A11D` | `AFA7A27B1A8B` | `FAE94FDD61C7` |
| `references/document-types.md` | `91103C6C2BEA` | `637C0E998F79` | `637C0E998F79` | `637C0E998F79` | `637C0E998F79` |
| `references/markdown-syntax.md` | `9BD0742FE68E` | `B8A351833509` | `FE0391E79E2C` | `27290A66500E` | `27290A66500E` |
| `references/research-report.md` | `BC8388930DEB` | `7680FA5A3C2D` | `7680FA5A3C2D` | `7680FA5A3C2D` | `7680FA5A3C2D` |
| `references/writing-style.md` | `56317D47BCCB` | `7BC2A85B2A92` | `7BC2A85B2A92` | `7BC2A85B2A92` | `7BC2A85B2A92` |
| `scripts/check_gongwen_md.py` | `150FC2F8AC70` | `3243E57731B4` | `B08DB644166F` | `B08DB644166F` | `B08DB644166F` |
| `templates/meeting-agenda.md` | `31B916CE996A` | `8FF41FB6B763` | `8FF41FB6B763` | `8FF41FB6B763` | `8FF41FB6B763` |
| `templates/official-letter.md` | `A7DEC1C24569` | `6E6868559289` | `6E6868559289` | `6E6868559289` | `6E6868559289` |
| `templates/phone-notice.md` | `911946C69D25` | `68B3B36A452A` | `68B3B36A452A` | `68B3B36A452A` | `68B3B36A452A` |
| `templates/phone-record.md` | `E9D7EB680340` | `DBC6B1856CF0` | `DBC6B1856CF0` | `DBC6B1856CF0` | `DBC6B1856CF0` |
| `templates/plain-document.md` | `E5E4C59FEAAC` | `FE9D1F4A6648` | `FE9D1F4A6648` | `FE9D1F4A6648` | `FE9D1F4A6648` |
| `templates/red-head-approval.md` | `2EAB4A7C3794` | `7B2DCAE479E6` | `7B2DCAE479E6` | `7B2DCAE479E6` | `7B2DCAE479E6` |
| `templates/research-report.md` | `93585F5C8C77` | `93585F5C8C77` | `93585F5C8C77` | `93585F5C8C77` | `93585F5C8C77` |
| `templates/white-paper.md` | `47685F06F536` | `15401D5CBD9D` | `15401D5CBD9D` | `15401D5CBD9D` | `15401D5CBD9D` |

树哈希：

- before：`c1ca346d5db5e599eb9136a6a278ef9519afc1508ffa5c956500c25725f48ee8`
- v2：`bff8e2291d0bab99fedbb9272161d5edc9f5ba0a935cd252e2149da0e7ba8bec`
- v3：`500786ac693a4d0078dcf1012f9cb49e0b25f05b98516bba35f1feae6c407720`
- v4：`48aae0229d94745beee571ac333ff5c9f7479009c82bf9e6e54ee6066b9b37da`
- v5：`2ab1b8b46ac91e2c73992042f7cbb4847b52add0af7ba161e9d673eb11124e74`

判定脚本（工作区，即 v3~v5 树内那份）：`b08db644166f56a8e5de400f9df405c58293fdf1c694c8cc74b39213fdc5d907`。
