# 公文助手技能包模型评测报告

- 评测对象：`skills/gongwen-markdown/`（给外部大模型用的 Agent Skill），检验**中等规模模型照着技能文档起草的 Markdown，能否被公文助手的解析器与自检脚本接受**。
- 评测日期：2026-10-18（Asia/Shanghai）。
- 评测脚本：`scripts/gongwen-skill-eval.py`（本仓库新增，可复用）。
- 判定脚本：`skills/gongwen-markdown/scripts/check_gongwen_md.py`，sha256 `b08db644166f56a8e5de400f9df405c58293fdf1c694c8cc74b39213fdc5d907`（前 16 位 `B08DB644166F56A8`），评分前 `--selftest` 12/12 通过。
- 结论一句话：新版技能包（v3）在七牛三个模型上把通过率从 **83.3% 提到 91.7%**，改善**全部来自 minimax-m2.7**；deepseek 与 qwen 在基线就已是 12/12。残留 3 例全是模型自身的格式滑落，不是文档没写。

---

## 1. 结论摘要

| 指标（七牛 qny，各 36 次调用） | before（HEAD 版技能树） | after（v2） | after3（v3，交付版） |
| --- | ---: | ---: | ---: |
| 通过（0 ERROR） | 30/36 | 32/36 | **33/36** |
| 通过率 | 83.3% | 88.9% | **91.7%** |
| ERROR 合计 | 24 | 8 | **7** |
| WARN 合计 | 6 | 0 | 1 |
| 接口失败 | 0 | 0 | 0 |

- **按模型**：deepseek/deepseek-v4-flash `12/12 → 12/12 → 11/12`；minimax/minimax-m2.7 `6/12 → 8/12 → 10/12`（ERROR 24 → 8 → 5）；qwen/qwen3.8-flash-next `12/12 → 12/12 → 12/12`。
- **按条件**：A（只给 SKILL.md）`14/18 → 15/18 → 16/18`；B（含 references + 模板）`16/18 → 17/18 → 17/18`。
- **两个专项模式**（文档作者最关心的两个错误）：`标题手写编号` 文档数 6 → 1 → **0**；`附件标题写“附件1”` 2 → **0** → 0。此外新增下钻的 `标题以“数字+空格”开头` 1 → 0，`表格缺少合格分隔行` 0 → 2 → 1。
- **改善发生在哪里**：minimax 条件 A 的手写编号 4 → 1 → 0、条件 B 2 → 0 → 0；附件1 2 → 0。deepseek / qwen 基线已满分，**不能写成“所有模型都从失败变成功”**。
- **残留**（v3，3 个用例、7 个 ERROR，详见第 6 节）：研究报告表格分隔行（minimax 条件 A）、未闭合 ` ```markdown ` 开头围栏（minimax 条件 B）、序号表分组行写成 5 格（deepseek 条件 A）。三条都属模型没照文档写，其中两条文档可再加反例降低复发。

---

## 2. 方法

### 2.1 目标与判定

判定不看文采，只看「能不能被公文助手接受」：

1. 清洗：严格复刻 `src/prompt.rs::sanitize_model_markdown` 的三步——剥思考块（` thinking`、`<thinking>`）、剥最外层代码围栏（只认空信息串与 `markdown`/`md`/`text`）、剥首尾寒暄。**清洗不修正任何格式**，否则测不出模型的错。
2. 判定：`python skills/gongwen-markdown/scripts/check_gongwen_md.py <clean.md> --kind <文种>`，记录每条 ERROR/WARN 与退出码；脚本自身异常单独记为 `check-error`。
3. 失败模式分类：对 check 的 ERROR 逐条归因，再加一层客观文本判据（首行、围栏残留、手写编号、附件区段缺失、表格判据、相对日期等）。同一条 ERROR 与文本判据只计一次，模式按「文档数」计数。
4. 判定口径冻结后，用 `rescore` 子命令对**所有历史 run 的已保存清洗稿**重跑判定与分类（不重新调模型、不重新清洗），保证三版数字来自同一把尺子。

Lead 另用 Rust 侧 `parse_markdown` + `validator::validate` 独立复验了 v2 的 36 份产物，逐例与 check 脚本一致；v3 产物由 Lead 通过 `manifest.json` 再复验（本报告只给 check 侧数字）。

### 2.2 模型与调用参数

| 提供商 | 模型全名 | 用途 |
| --- | --- | --- |
| qny `https://api.qnaigc.com` | `minimax/minimax-m2.7`、`qwen/qwen3.8-flash-next`、`deepseek/deepseek-v4-flash` | before / after / after3 主矩阵（各 36 次） |
| dmxapi `https://www.dmxapi.cn` | `MiniMax-M2.7`、`deepseek-v4-flash` | 交叉验证 |
| siliconflow `https://api.siliconflow.cn` | `deepseek-ai/DeepSeek-V4-Flash`、`Qwen/Qwen3.8-27B` | 交叉验证 |

- `temperature=0.3`，非流式；`max_tokens=32768`（推理型模型若把预算全花在 reasoning 且 `finish_reason=length`，脚本自动放大到 65536 重试一次，并记录 `max_tokens_used`）。
- HTTP 全部走 Python `urllib` + `ssl`（本机 schannel 的 curl/PowerShell TLS 会 `SEC_E_NO_CREDENTIALS` 失败）。按提供商限并发并拉开间隔，429/5xx 退避重试。
- 记录 `prompt_tokens`、`completion_tokens`、`reasoning_tokens`、`total_tokens`、耗时、`finish_reason`。上下文规模：条件 A 平均 prompt 约 2.7k tokens（before）/4.7k（v2），条件 B 约 8.9k（before）/19.7k（v2）；v3 因中断续跑，只有 3 次新调用有 token 记录（见 §8 限制）。

### 2.3 两种上下文条件

- **条件 A**：system 只给 `SKILL.md`（模拟只能读到技能正文、没有文件读取能力的接入方式）。
- **条件 B**：system 给 `SKILL.md` + `references/document-types.md` + `references/markdown-syntax.md` + 该文种模板；研究报告另加 `references/research-report.md`（模拟能读技能文件的工具型模型）。
- 每个文种的 user 消息完全相同（固定素材 + 写作要求），素材写在脚本里，含明确时间、地点、数据、金额，便于发现编造；条件 A/B 只差 system。

### 2.4 六个固定用例

| 文种 | 素材要点 |
| --- | --- |
| `official-letter` | 公函商请共享地下管线数据；含两份附件全文（需求清单表 + 时间安排表）；要求正文引用“见附件1/2” |
| `plain-document` | 政务数据资源共享开放工作要点；含一张序号表（两组 6 项）与一张统计表（3 行数据） |
| `meeting-agenda` | 冬季供水保障专题会议；时间地点、参加人员、4 项议程 |
| `white-paper` | 追加供水管网改造专项资金的请示；要求段内枚举“一是……；二是……”，不得用列表 |
| `phone-notice` | 防冻保温电话通知；三项要求 + 时间节点 + 报告渠道 |
| `research-report` | 供水管网漏损治理研究；要求区段标记、锚点与交叉引用、一张漏损率表、不给文献库 |

### 2.5 技能树冻结与哈希

技能包在评测期间被多个任务并行修改，因此**每轮 run 使用一份冻结快照**（`--skill-dir`），run 目录内另存 `skill-snapshot/`（逐文件内容）与 `SOURCES.json`（逐文件 sha256）。三份树：

| 树 | 目录 | 树哈希 `skill_tree_sha256` | 内容 |
| --- | --- | --- | --- |
| before | `tmp/gongwen-skill-eval/skill-before` | `c1ca346d5db5e599eb9136a6a278ef9519afc1508ffa5c956500c25725f48ee8` | `git HEAD` 版（所有未提交改动都不算） |
| after（v2） | `tmp/gongwen-skill-eval/skill-after` | `bff8e2291d0bab99fedbb9272161d5edc9f5ba0a935cd252e2149da0e7ba8bec` | 2026-10-18 11:19 定稿的第 2 版 |
| after3（v3，交付版） | `tmp/gongwen-skill-eval/skill-after3` | `500786ac693a4d0078dcf1012f9cb49e0b25f05b98516bba35f1feae6c407720` | 2026-10-18 11:57 定稿的第 3 版 |

关键文件 sha256 前 12 位（完整表见附录 B）：

| 文件 | before | after v2 | after3 v3 |
| --- | --- | --- | --- |
| `SKILL.md` | `7A0DDDEF599C` | `6316EA98F87C` | `1D5DA9D8A11D` |
| `references/document-types.md` | `91103C6C2BEA` | `637C0E998F79` | `637C0E998F79` |
| `references/markdown-syntax.md` | `9BD0742FE68E` | `B8A351833509` | `FE0391E79E2C` |
| `references/research-report.md` | `BC8388930DEB` | `7680FA5A3C2D` | `7680FA5A3C2D` |
| `references/writing-style.md` | `56317D47BCCB` | `7BC2A85B2A92` | `7BC2A85B2A92` |
| `scripts/check_gongwen_md.py` | `150FC2F8AC70` | `3243E57731B4` | `B08DB644166F` |
| 8 个模板 | 见附录 B | 同 v3（v2→v3 未改模板） | 同附录 B |

> 判定脚本另有工作区版本（= after3 树里的那份，`B08DB644166F…`），三版技能树的产物都用它重评，它是「仪器」，不随被测树变。

### 2.6 复现命令

```powershell
# 0) 密钥：脚本从 D:\gongwen\model_api_test.md.txt（已 gitignore）或环境变量读，仓库里没有任何密钥
python scripts/gongwen-skill-eval.py smoke

# 1) 主矩阵（每个 run 36 次：3 模型 × 6 文种 × A/B）
python scripts/gongwen-skill-eval.py run --matrix baseline --run-id after3-20261018 `
    --skill-dir tmp/gongwen-skill-eval/skill-after3

# 2) 交叉验证（dmxapi + siliconflow 4 模型 × 6 文种 × 条件 A）
python scripts/gongwen-skill-eval.py run --matrix cross --run-id after3-cross-20261018 `
    --skill-dir tmp/gongwen-skill-eval/skill-after3 --conditions A

# 3) 统一口径重评（不重新调模型、不重新清洗）
python scripts/gongwen-skill-eval.py rescore --all

# 4) 三版对比与单 run 报告
python scripts/gongwen-skill-eval.py compare before-20261018 after-20261018 after3-20261018 `
    --out tmp/gongwen-skill-eval/compare-3way-qny.md
```

断点续跑：同一 `run-id` 再跑一次 `run`，已有清洗稿直接复用并重判，只补缺失的调用（`--force` 才会全部重调）。

---

## 3. 三版对比（重评后，qny，36 次/版）

### 3.1 总体

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

### 3.3 按模型 / 条件（通过/成功（ERROR））

| 分组 | before | after(v2) | after3(v3) |
| --- | --- | --- | --- |
| deepseek/deepseek-v4-flash | 12/12（0） | 12/12（0） | 11/12（2） |
| minimax/minimax-m2.7 | 6/12（24） | 8/12（8） | 10/12（5） |
| qwen/qwen3.8-flash-next | 12/12（0） | 12/12（0） | 12/12（0） |
| 条件 A（只给 SKILL.md） | 14/18（17） | 15/18（6） | 16/18（4） |
| 条件 B（含 references） | 16/18（7） | 17/18（2） | 17/18（3） |

### 3.4 失败模式（文档数）

| 失败模式 | before | after(v2) | after3(v3) |
| --- | ---: | ---: | ---: |
| 标题手写编号 | 6 | 1 | **0** |
| 标题以“数字+空格”开头（年份也会被剥） | 0 | 1 | **0** |
| 附件标题写了“附件1/附件1：” | 2 | **0** | 0 |
| 代码围栏未剥净 | 0 | 0 | 1 |
| 首行不是唯一的 `#` 主标题 | 0 | 0 | 1 |
| 表格列数不一致（序号表分组行多一格） | 0 | 0 | 1 |
| 表格缺少合格分隔行（整表会按正文印出） | 0 | 2 | 1 |

### 3.5 重点模式下钻（模型 × 条件，箭头为 before → v2 → v3）

| 模型 · 条件 | 手写编号 | 附件1 标题 | 数字+空格标题 | 分隔行不合格 |
| --- | --- | --- | --- | --- |
| minimax/minimax-m2.7 · A | 4 → 1 → **0** | 1 → 0 → 0 | 0 → 1 → **0** | 0 → 1 → 1 |
| minimax/minimax-m2.7 · B | 2 → 0 → **0** | 0 → 0 → 0 | 0 → 0 → 0 | 0 → 1 → 0 |
| deepseek/deepseek-v4-flash · A | 0 | 1 → 0 → 0 | 0 | 0 |
| qwen/qwen3.8-flash-next · A / B | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |

按文种看手写编号：before 为 official-letter 2、plain-document 1、white-paper 1、phone-notice 1、research-report 1、meeting-agenda 0；v3 全部为 0。

### 3.6 v3 相对 v2 的文档改动（供核对）

1. `SKILL.md` 第 0 节：标题行首「一个编号字符都不许有」，含“数字+空格”；六错表扩成八错（新增「`### 2026 年第三季度数据` → `### 2026年第三季度数据`」与「两列表格分隔行写成 `| ---: |`」）；默查第 ① 条同步；第 2 节开头加「骨架里的 `##`/`###` 照原样写」；文末新增「落笔前最后三句」。
2. `SKILL.md` 把骨架里的**说明文字全部移出代码围栏**（公函、普通公文、白头件、研究报告骨架），避免模型把“给模型看的说明”当正文抄。
3. `references/markdown-syntax.md` §2.1：补「标题以『数字+空格』开头也会被剥，四位年份也不例外」。
4. 第四级标题默认编号实为半角 `(1)`（`src/models.rs`），v2 已改正。

---

## 4. 判定口径变化（#1~#4，单列）

判定脚本在评测期间改过四处。这些是**仪器的变化，不是模型变好**；报告数字全部用 `rescore` 以最终口径（#1~#4）重算，原始输出与清洗稿未改动一个字。

| # | 变化 | 影响 |
| --- | --- | --- |
| #1 | 手写编号前缀与 `src/export/headings.rs` 的 10 条逐条对齐 | 以前 `第三部分部署`（中间无空格）漏报、`第1部` 误报；现在该报的报、不该报的不报。`^\d+\s+` 使“标题以数字+空格开头”成为可判事项 |
| #2 | 表格判据改用解析器口径：含 `|` 且 ≥2 格才算行；分隔行每格 ≥3 短横；省略首尾竖线也校验列数；围栏内不算 | 比以前严；`甲 | 乙` 这类无首尾竖线的表也会被校验 |
| #3 | 新增 `check_gongwen_md.py --selftest`（12 条判据自测） | 评分前先证明仪器可用，本报告所有 run 均 12/12 通过 |
| #4 | 「缺合格分隔行」分两级：一组多行（或下一行像分隔行）→ **ERROR**；孤立单行含 `|` → WARN | 原先一律 WARN。理由：解析器要求分隔行紧跟表头下一行，不合格整张表会按正文印出（纸上会印出竖线），是真缺陷 |

**重评前后的差异（如实写）**：

- `after-20261018`（v2）从 **34/36 降为 32/36**，ERROR 4 → 8：minimax 的 research-report 条件 A、B 各有 2 条分隔行问题，在旧口径下只记 WARN，新口径（#4）升为 ERROR。这是**口径变严**，不是模型变差。
- `before-20261018` 30/36 不变（其错误几乎全是手写编号，两种口径下都是 ERROR）。
- 出现一个新失败模式 `标题以“数字+空格”开头`：由 #1 的 `^\d+\s+` 与学生模型“`### 2026 年第三季度…`”写法共同触发；v3 文档补了该条后归零。

---

## 5. 交叉验证

| run | 技能树 | 调用 | 成功 | 通过 | 通过率 | ERROR | 备注 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| `after-cross-20261018` | v2 | 48（4 模型 × 6 文种 × A/B） | 43 | 41 | 95.3% | 5 | 5 次 siliconflow Qwen 超时（api-error）；ERROR 全部来自 dmxapi `MiniMax-M2.7` |
| `after3-cross-20261018` | v3 | 24（4 模型 × 6 文种 × A） | 22 | 22 | **100%** | 0 | 2 次 siliconflow Qwen 超时 |

- `dmxapi MiniMax-M2.7` 在 v2 交叉里的 ERROR 与七牛 `minimax/minimax-m2.7` 同源：official-letter 条件 B 手写编号 3 条、research-report 条件 A 分隔行 2 条；v3 条件 A 复跑该模型 6 个文种全部 0 ERROR。
- `siliconflow Qwen/Qwen3.8-27B` 的请求多次在 120–240 秒超时（同一模型在七牛 `qwen/qwen3.8-flash-next` 是 12/12 满分），交叉样本因此不完整；这不影响主矩阵结论。
- 交叉验证不改变主结论：**改善集中在 minimax 系（七牛与 dmxapi 两个入口一致）**。

---

## 6. 残留失败清单（after3/v3，供 Lead 用 Rust 解析器复验）

3 个用例、7 个 ERROR。路径均相对 `D:\gongwen`。

### 6.1 minimax/minimax-m2.7 · research-report · 条件 B（3 ERROR）

- 文件：`tmp/gongwen-skill-eval/after3-20261018/B/research-report/qny__minimax-minimax-m2.7.md`（原始输出同名 `.raw.md`）
- ERROR：`第1行 文件以代码围栏开头：交付的 .md 内容里不能带 ``` 包裹`、`第1行 第一行应是唯一的 \`# 正式标题\``、`第1行 只用 \`\`\`mermaid 围栏画图表，其他代码块不要写`
- 原因：模型输出**未闭合的 ` ```markdown ` 开头围栏**（整篇被一个围栏包住但没有结束围栏）。清洗只剥“首尾两处围栏”的整段包裹，单围栏不剥，所以清洗稿第一行仍是围栏。**这是模型自身的错**（技能文档已多处写“不要代码围栏”），不是文档没写；防长上下文“就近遗忘”的「落笔前最后三句」已加，仍偶发。

### 6.2 minimax/minimax-m2.7 · research-report · 条件 A（2 ERROR + 1 WARN）

- 文件：`tmp/gongwen-skill-eval/after3-20261018/A/research-report/qny__minimax-minimax-m2.7.md`
- ERROR：`第29行/第31行 这一组含 | 的行不构成合格表格：分隔行必须是紧跟表头下一行的 | --- | --- |（每格至少 3 个短横、格数与表头一致）`
- 片段：表头为两列 `| 年度 | 漏损率（%） |`，分隔行却写成单格 `| :---: |`，解析器会判整表不成立、按正文印出。
- WARN：`第49行 中文里用了半角引号`。
- 性质：模型没照文档写；`references/research-report.md` 与 `markdown-syntax.md` 的表例都是 3 列以上，**没有给“两列最小表”的正例**，下一轮可补（见 §7）。

### 6.3 deepseek/deepseek-v4-flash · plain-document · 条件 A（2 ERROR）

- 文件：`tmp/gongwen-skill-eval/after3-20261018/A/plain-document/qny__deepseek-deepseek-v4-flash.md`
- ERROR：`第19行/第23行 这一行有 5 格，多于表头的 4 列`
- 片段：序号表的分组行写成 `| || 基础支撑组 |||`（5 格），正确写法是 `| 组名 |||`（组名放首格、其余留空）。
- 性质：模型没照文档写；`markdown-syntax.md` §6 已有分组行说明，但没有把「错误写法」摆出来，下一轮可补一行反例。
- 注：deepseek 在 v2 run 里此用例是 0 ERROR（11/12 → 12/12 的差别就来自这一条），属**单次采样波动**，不能据此判定 v3 文档倒退。

### 6.4 附加：`mixed-20261018`

评测开始阶段技能文档正被并行改写，最初一轮 run 中途换过 SKILL.md 版本，已更名为 `mixed-20261018` 并**不计入任何对比**（仅作排查线索）。

---

## 7. 按技能文件定位的改法建议（下一轮可执行）

按性价比排序。所有建议都不放松判定口径，只改文档。

1. **`skills/gongwen-markdown/references/markdown-syntax.md` §6「序号表」**：把分组行写成唯一正确形态并配反例。观察证据：deepseek 条件 A 写 `| || 基础支撑组 |||`（5 格）被判整表列数错。建议在“分组行”条目下加两行：
   - ✓ `| 基础支撑组 |||`（组名放首格，其余留空）
   - ✗ `| || 基础支撑组 |||`（组名前多一根竖线，整行变 5 格，本表列数报错）
2. **`references/markdown-syntax.md` §6「表格」+ `references/research-report.md`「表格与表题」**：补一个**两列最小表**完整代码块（表头 2 格、分隔行 2 格），并加一句「分隔行格数必须等于表头格数，只写 `| ---: |` 一格时整表按正文印出」。观察证据：minimax 研究报告 A/B 两轮都栽在 2 列漏损率表的分隔行上（v3 中 A 仍复发）。
3. **`SKILL.md` §0 八错表**：可加第 9 错「整篇不要用 ` ```markdown ` 包住；**未闭合的单个围栏不会被程序剥掉**——文件会以围栏开头、直接被判不合格」。观察证据：minimax 条件 B 出现未闭合开头围栏（§6.1）。这条改的是提醒强度，不保证根治（模型不遵循指令属能力边界）。
4. **`references/document-types.md` 研究报告节「最容易被模型写错的地方」**：把“表格分隔行格数与表头一致”并入该文种清单，与研究报告中表格出现频率高的特点匹配。
5. **不建议**放宽 `check_gongwen_md.py`：口径 #4 的严格化已被 Rust 侧解析器复验逐例印证（不合格分隔行确实整表降级），放宽会掩盖真实缺陷。

---

## 8. 结论边界

**有实测支撑的结论**

- 在 2026-10-18 的七牛三个模型、6 个固定用例、条件 A/B 各一次采样下：v3 技能树的通过率为 91.7%（33/36），v2 为 88.9%，HEAD 基线为 83.3%；三版数字同一判定口径（脚本 sha `B08DB644166F…`）。
- 「标题手写编号」与「附件标题写附件1」两个模式在 v3 归零，改善可定位到 minimax 模型与 SKILL.md 第 0 节/骨架示例的修改。
- 交叉验证显示 dmxapi 的 `MiniMax-M2.7` 与七牛 `minimax/minimax-m2.7` 行为同源；v3 条件 A 交叉 22/22 通过。
- v2 产物已由 Rust 侧解析器 + 校验器独立复验，与 check 逐例一致（口径 #4 生效后）。

**只是文档一致性论证、没有模型实测的部分**

- 8 个模板与 references 的其它规则（红头呈批件、电话记录单、联合发文、横页等未进入本次 6 个用例的文种）没有模型实测，只有“文档与解析器一致”的静态核对。
- §7 的改法建议基于每类 1~2 个观测样本，属于“值得一试”，不是统计显著结论。

**方法学限制**

- 每个 `(模型, 文种, 条件)` 单元只有 1 次采样（temperature 0.3），存在**单次波动**：deepseek 条件 A 的序号表错误只在 v3 run 出现，v2 同用例为 0 ERROR。
- 条件 A/B 的 system 长度差异大（A 约 2.7k tokens，B 约 9k~20k tokens），条件间对比同时受“信息量”和“上下文长度”两个因素影响。
- `after3-20261018` 曾因会话中断只跑完 33/36，后按断点续跑补齐，但**复用产物的 token/耗时统计缺失**（v3 只有 3 次新调用的用量记录）；v3 的模型输出与判定不受影响。`after-cross-20261018` 因中断后重建汇总，同样只有部分任务有 token 记录。
- siliconflow `Qwen/Qwen3.8-27B` 多次超时（api-error），交叉验证样本不完整；主矩阵未受影响。
- 评测只覆盖「能否通过解析器与自检脚本」，不评价文风、事实准确性与编造情况（素材已按可发现编造的方式设计，但未做自动判定）。

---

## 9. 产物与目录

```
tmp/gongwen-skill-eval/                     # 已 gitignore
├── skill-before/  skill-after/  skill-after3/     # 三份冻结技能树（含逐文件内容）
├── before-20261018/                              # HEAD 基线，36 次
├── after-20261018/                               # v2，36 次
├── after3-20261018/                              # v3 主验证，36 次
├── after-cross-20261018/                         # v2 交叉，48 次
├── after3-cross-20261018/                        # v3 交叉，24 次
├── mixed-20261018/                               # 中途换文档的排查轮（不计入对比）
├── compare-3way-qny.md  compare-cross.md
└── <每个 run>/
    ├── manifest.json          # {"run","created","skill_dir","skill_tree_sha256","cases":[...]}
    ├── summary.tsv / summary.json   # 逐次结果、聚合、失败模式、重点模式下钻
    ├── report.md              # 该 run 的自动报告
    ├── prompts/<kind>-<A|B>.md      # 实际 system + user（可复核提示词）
    ├── skill-snapshot/        # 本轮实际使用的技能文件副本 + SOURCES.json（逐文件 sha256）
    └── <A|B>/<kind>/<provider>__<model>.md / .raw.md   # 清洗稿 / 原始输出
```

- `manifest.json` 顶层含 `skill_dir` 与 `skill_tree_sha256`，由 Rust 侧 `model_outputs_survive_the_parser_and_validator` 读取复验。
- 本报告所有 ERROR/WARN 数字都可用 `compare`、`rescore` 从这些产物复算；原始输出与清洗稿从未被修改（`rescore` 只改判定字段）。

---

## 附录 A：复现与自查命令

```powershell
# 判定脚本自检（12 条）
python skills/gongwen-markdown/scripts/check_gongwen_md.py --selftest

# 单个清洗稿判定
python skills/gongwen-markdown/scripts/check_gongwen_md.py `
  tmp/gongwen-skill-eval/after3-20261018/A/research-report/qny__minimax-minimax-m2.7.md `
  --kind research-report

# 断点续跑（只补缺失调用，不重复花钱）
python scripts/gongwen-skill-eval.py run --matrix baseline --run-id after3-20261018 `
  --skill-dir tmp/gongwen-skill-eval/skill-after3

# 全量重评（同一把尺子）
python scripts/gongwen-skill-eval.py rescore --all
```

## 附录 B：三份技能树的逐文件 sha256（前 12 位）

| 文件 | before | after(v2) | after3(v3) |
| --- | --- | --- | --- |
| `SKILL.md` | `7A0DDDEF599C` | `6316EA98F87C` | `1D5DA9D8A11D` |
| `references/document-types.md` | `91103C6C2BEA` | `637C0E998F79` | `637C0E998F79` |
| `references/markdown-syntax.md` | `9BD0742FE68E` | `B8A351833509` | `FE0391E79E2C` |
| `references/research-report.md` | `BC8388930DEB` | `7680FA5A3C2D` | `7680FA5A3C2D` |
| `references/writing-style.md` | `56317D47BCCB` | `7BC2A85B2A92` | `7BC2A85B2A92` |
| `scripts/check_gongwen_md.py` | `150FC2F8AC70` | `3243E57731B4` | `B08DB644166F` |
| `templates/meeting-agenda.md` | `31B916CE996A` | `8FF41FB6B763` | `8FF41FB6B763` |
| `templates/official-letter.md` | `A7DEC1C24569` | `6E6868559289` | `6E6868559289` |
| `templates/phone-notice.md` | `911946C69D25` | `68B3B36A452A` | `68B3B36A452A` |
| `templates/phone-record.md` | `E9D7EB680340` | `DBC6B1856CF0` | `DBC6B1856CF0` |
| `templates/plain-document.md` | `E5E4C59FEAAC` | `FE9D1F4A6648` | `FE9D1F4A6648` |
| `templates/red-head-approval.md` | `2EAB4A7C3794` | `7B2DCAE479E6` | `7B2DCAE479E6` |
| `templates/research-report.md` | `93585F5C8C77` | `93585F5C8C77` | `93585F5C8C77` |
| `templates/white-paper.md` | `47685F06F536` | `15401D5CBD9D` | `15401D5CBD9D` |

树哈希（对所有文件的相对路径与内容算）：

- before：`c1ca346d5db5e599eb9136a6a278ef9519afc1508ffa5c956500c25725f48ee8`
- after(v2)：`bff8e2291d0bab99fedbb9272161d5edc9f5ba0a935cd252e2149da0e7ba8bec`
- after3(v3)：`500786ac693a4d0078dcf1012f9cb49e0b25f05b98516bba35f1feae6c407720`

判定脚本（工作区，即 v3 树内那份）：`b08db644166f56a8e5de400f9df405c58293fdf1c694c8cc74b39213fdc5d907`。
