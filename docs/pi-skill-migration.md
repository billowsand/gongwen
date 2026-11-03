# Pi 标准技能迁移：旧 15 份技能的去向与缺口

本轮把旧 AI 工作台的 15 份 `assets/agent-skills/` 技能收进 Pi 的 8 份标准技能。
**旧技能原样保留，Pi 写作不再执行旧 DSL**，Pi 不读取、不迁移这些旧文件；下面的映射说明「哪些业务判据
进了哪份标准技能/参考文件」，以及「哪些能力没有覆盖、为什么」。

这不是兼容层：Pi 没有 `flow` / `params` / 旧变量 / 固定出题 / 人工确认清单，也不
假称与旧行为完全一致。技能是自然语言步骤，权限、事实来源与交付闸门由 Rust 程序判定。

## 一、旧技能逐项映射

| 旧技能 | 迁到 | 保留的业务判据 | 未覆盖 / 原因 |
| --- | --- | --- | --- |
| `condense` 精简扩写 | `gongwen-revise/references/condense.md` | 精简不删任务、责任单位、时限、数字与依据；先删重复空话；差 15% 再改一次 | 旧 `fit_length` 是程序数字数并驱动二次生成；现在由模型自己数，没有独立算子 |
| `extract` 要点提炼 | 新主技能 `gongwen-extract` | 选区优先、条数上限、保留时间数字单位、按原文顺序、不加评论、只出清单 | 旧 `plan mode:list` 由程序按行/句拆分；现在由模型提炼，靠清单判据约束 |
| `fact-check` 事实核查 | `gongwen-review/references/fact-check.md` | 逐条核时间/数字/文件，分「有出处 / 矛盾 / 未找到」，只出清单 | 旧 `check.facts` 程序台账与内网数据接口核查未接；本阶段用 `kb_*`/`ms_*` 检索 + `check_document` |
| `free-task` 自由任务 | 自动路由（不另建技能） | 模型自己决定检索、写作、提问；改了稿交正文，没改稿交清单 | 旧 DSL 的 `finding.add`、`calc.*`、`http.call` 工具族不做（4E 未开工） |
| `imitate` 仿写 | `gongwen-imitate` + `references/style-analysis.md` | 基准稿只作写法、旧稿事实不沿用、先问清变化、样稿不进证据 | 旧「检索候选稿 → 用户选稿 → 选仿写方式 → 缺口循环 → 核验」的固定流程不做；改为模型自主检索与提问 |
| `material` 材料成文 | `gongwen-draft/references/material-drafting.md` | 只用材料、事项原样保留、冲突不悄悄二选一、缺事实写待核实、只问方向性问题 | 旧「程序按原文拆要点 → 用户确认 → 成文」的确认清单不做；材料原文本身就是依据 |
| `normalize` 规范化 | `gongwen-revise/references/normalize.md` | 标点/数字/用语规范、单位人员按标准词库写法、事实锁定 | 旧 `vocab.normalize` 程序自动替换未接；`vocab_search` 只读，写值仍由模型改、由程序闸门查 |
| `policy-basis` 政策依据核对 | 新主技能 `gongwen-policy-check` + `references/citation-check.md` | 名称/文号/引文三项分开比、年份用六角括号〔〕、找不到原文必须明说、只提示不改 | 旧的程序引文比对与「审校抽屉逐条采纳」未接；现在是侧栏清单 |
| `policy-report` 政策研究报告 | `gongwen-research` + `references/argumentation-and-sources.md` | 逐章论证、每个关键判断有出处、摘要只写正文内容、引用落应用支持的语法、不编文献 | 旧「大纲确认 → pick_evidence → 逐节生成 → gap_loop → cite 落库」的固定流程不做；分节由模型自主安排 |
| `polish` 润色 | `gongwen-revise/references/polish.md` | 事实锁定、只改要求部分、选区保护、只处理病句与啰嗦 | 旧 `generate mode:rewrite` 与 preset 变量不做 |
| `reply-letter` 复函 | 新主技能 `gongwen-reply` + `references/incoming-letter.md` | **主送认来函落款而不是抬头**、逐项答复、没给意见不替用户表态、来函事实照录、标题文号缺失写待核实 | 旧「程序要素抽取 → 要素建议卡 → 逐题确认」未接；`doc_elements` 只读，缺的写待核实 |
| `research-draft` 研究式起草 | 并入 `gongwen-draft`（含 `material-drafting.md`） | 先查后写、证据编号只用于自查、缺事实标待核实、只问方向性问题、已填要素不重复问 | 旧「预研问题 → 检索 → gap_loop → verify → 出题」的循环不做；不让模型为「解锁交付」反复重交 |
| `review` 全面审校 | `gongwen-review` | 只出清单不改稿、结构/表述/逻辑/用语逐条给位置与依据、程序检查优先 | 旧「改法片段必须唯一、进审校抽屉逐条采纳」的程序闸门未接；Pi 走 `check_document` + 人工看清单 |
| `style-learn` 风格学习 | `gongwen-imitate/references/style-analysis.md` | 只学写法不学事实、统计特征（句长/段长/编号/开头结尾/常用说法）、样稿事实要标出 | **不写风格库**：没有「学出档案 → 保存 → 采纳」链路；只读人工维护的风格档案（`style_list` / `style_read`），见第三节 |
| `tone` 语气调整 | `gongwen-revise/references/tone.md` | 上行文谦恭、平行文商洽、下行文明确；结语规则；只改语气不动事实 | 旧 preset 变量与固定算子不做 |

## 二、八份标准技能

| 技能 | 目录 | 参考文件（按需读） |
| --- | --- | --- |
| 起草 `gongwen-draft` | `assets/pi-skills/gongwen-draft/` | `material-drafting.md` |
| 修改 `gongwen-revise` | `assets/pi-skills/gongwen-revise/` | `condense.md`、`polish.md`、`tone.md`、`normalize.md` |
| 审核 `gongwen-review` | `assets/pi-skills/gongwen-review/` | `fact-check.md` |
| 仿写 `gongwen-imitate` | `assets/pi-skills/gongwen-imitate/` | `style-analysis.md` |
| 研究报告 `gongwen-research` | `assets/pi-skills/gongwen-research/` | `argumentation-and-sources.md` |
| 复函 `gongwen-reply` | `assets/pi-skills/gongwen-reply/` | `incoming-letter.md` |
| 要点提炼 `gongwen-extract` | `assets/pi-skills/gongwen-extract/` | `extraction-checklist.md` |
| 政策依据核对 `gongwen-policy-check` | `assets/pi-skills/gongwen-policy-check/` | `citation-check.md` |

几个刻意的合并：

- **润色、精简、语气、规范化不各立主技能**，写成修改技能按任务选择的参考文件，
  避免四份技能抢同一个「改稿」入口。
- **材料成文归起草**：它就是把材料变成公文，不是另一条文种。
- **研究报告吸收旧 `policy-report`**：论证与出处要求写进参考文件，主技能仍是研究报告。
- **事实核对归审核**，政策依据核对保留专门技能：前者核时间/数字/出处，后者核名称/
  文号/引文，判据不同。
- **自由任务不建技能**：自动路由会按说明挑一个最接近的技能；都不合适时模型按用户
  要求与现有工具工作，不另建旧 DSL。

## 三、风格学习：这一轮只做只读

- 模型可以**分析**样稿的写法（结构、句长、编号、开头结尾、常用说法、力度与归因分寸），
  也可以把学到的写法**用在本次改写里**。
- 模型**没有**把风格学习结果保存进应用的风格库。要长期复用，用户得在
  「AI 管理 → 风格」里手动建立或修改档案。
- 新增的 `style_list` / `style_read` 工具只读已有档案：按当前文种列出启用的档案、
  按 id 取回写法与范例；未知、停用、不适用的档案给结构化错误，不悄悄换一份。
  工具不调第二个模型、不写库、不记使用次数、不执行 shell。
- 风格档案与范例都按 `style_sample` 对待：只提供写法，不是本次事项的事实来源，
  也不登记进证据台账；其中的单位、人员、日期、数字不能搬进正文。

**后续收口（未做）**：AI 学出档案 → 用户审阅 → 保存进风格库的表单采纳链；
描述类要素建议卡接入 Pi；通用附件协议。这几项涉及写用户配置与表单，需单独设计
授权与撤销，本阶段不动。

## 四、保留的业务细则

这些是旧技能里值得留的判据，已写进对应文件：

- 复函主送认来函**落款**单位，不是来函**抬头**（抬头是本单位）。
- 答复意见不能替用户表态；没给意见的事项写「【待核实：…】」。
- 政策核对找不到原文要写「未找到原文，需人工核对」，不能写「已核实」。
- 精简不能删任务、责任单位、时限、数字和政策依据。
- 材料冲突写「【待核实：哪处矛盾】」并留两处原话，不悄悄二选一。
- 样稿只供写法；事实全部来自本次材料、来函和工具实际返回的资料。

## 五、每份技能的读取、提问与交付口径

- **按需读**：先读 `input/current.md`、`input/request.md`、材料索引与当前文种的语法，
  再按任务读自己的参考文件；不要求一次读完整个语法包。
- **提问**：只有会让整篇方向写错的事（行文方向、受文对象、目的、篇幅、文种明显不合适）
  才 `ask_user`；已给的信息不重复问；授权要素不建议、不出题。
- **不设确认闸门**：没有「先确认要点清单 / 大纲 / 事实勾选才能生成」这一步，
  也不要求为「解锁交付」反复重交。
- **交付分流**：只审核 / 提炼 / 风格分析 → `finish_report`；要求生成或修改正文 →
  `submit_document` 交**完整** Markdown。中间可以写多个工作文件，最后只交一份成品。
- `check_document` 优先传 `{path:"work/文件名.md"}`，不要把整篇正文再复制到 `text`。
- 研究报告只用应用支持的引用语法（`[@key]`，key 必须在用户提供的文献库里；没有文献库
  就不写引用），不手写参考文献条目，不捏造文献。

## 六、内置包的完整性（D2）

- `assets/pi-skills/` 下的每个文件都通过 `src/pi_agent/skills/mod.rs` 的 `FILES` 表
  `include_str!` 编进二进制；`BUILTINS` 只存 8 个技能名。
- 新建工作区、复制为用户副本、导出 ZIP / 文件夹都从这一份内存包取文件，参考文件不会
  只在源码目录里存在。`every_builtin_file_on_disk_is_embedded` 与
  `builtin_packages_carry_their_references_into_copies_and_exports`、
  `builtin_skill_references_reach_the_workspace_snapshot` 盯着这一点。
- 用户同名副本仍优先于内置；用户技能不能占用公共语法包 `gongwen-markdown`；
  坏副本不回退内置、快照隔离、恢复原内置等 4C 判据不变。
- 已经复制过内置技能的用户副本不会自动吸收本轮参考文件与规则。测试新的默认技能前，
  在技能管理页核对来源；需要更新时先自行导出副本留存，再恢复内置，不自动覆盖用户修改。
- 包体积与路径规则不变：单包 32 MiB / 256 文件、单文件 8 MiB、路径最多 8 段；
  每轮技能包合计最多 128 MiB、索引最多 512 KiB。
