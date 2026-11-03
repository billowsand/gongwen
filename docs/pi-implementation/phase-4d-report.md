# 第四阶段 D 实现报告（worker）

- 基线：`main` / `74e9133`（开工时工作区干净，只有主代理放入的未跟踪 `docs/pi-implementation/phase-4d.md`）。
- 分工：本机 Pi 实现，主代理审查真实 diff、修正并验收。未提交、未推送、未打 tag、未发布、未安装、未改版本号。
- 授权：按 `pi-execution-authorization.md`，未读取用户真实配置、密钥、稿件、`.env`、`.pi/auth`、
  `model_api_test.md.txt` 或真实 Pi 运行工作区；测试只用自造夹具与本机临时目录。

## 一、完成内容

### D1 技能迁移（15 份旧技能 → 8 份标准技能）

- 保留并改写五份：`gongwen-draft`、`gongwen-revise`、`gongwen-review`、`gongwen-imitate`、
  `gongwen-research`；新增三份：`gongwen-reply`（复函）、`gongwen-extract`（要点提炼）、
  `gongwen-policy-check`（政策依据核对）。
- 润色 / 精简扩写 / 语气 / 规范化写成修改技能的参考文件（`references/polish.md`、
  `condense.md`、`tone.md`、`normalize.md`），不各立主技能；材料成文归起草
  （`gongwen-draft/references/material-drafting.md`）；研究报告吸收旧 `policy-report`
  的论证与出处要求（`gongwen-research/references/argumentation-and-sources.md`）；
  事实核对归审核（`gongwen-review/references/fact-check.md`）；政策依据核对保留专门技能
  （`gongwen-policy-check/references/citation-check.md`）；自由任务由自动路由处理，不建旧 DSL。
- 仿写补 `references/style-analysis.md`：可分析、可应用写法；明确**不声称把风格学习结果保存进
  风格库**，只读人工维护的档案。
- 保留细则：复函主送认来函落款、不替用户表态；找不到政策原文要明说；精简不删任务/责任/时限；
  材料冲突不悄悄二选一；样稿只供写法。
- 每份技能都写明按需读 `gongwen-markdown`、材料角色、选区、`doc_elements`；只有方向性问题才
  `ask_user`，授权要素不建议/不出题；没有「先确认清单才能生成」「事实勾选才能进正文」。
- 交付分流：只审核/提炼/风格分析 → `finish_report`；生成/修改 → `submit_document` 完整 Markdown；
  `check_document` 优先传 `path`；研究报告只用应用支持的 `[@key]` 引用，没有文献库就不写。

逐项映射、未覆盖能力与原因见新增 `docs/pi-skill-migration.md`。

### D2 内置完整包

- `src/pi_agent/skills/mod.rs` 新增 `FILES` 资源表（技能名、包内相对路径、`include_str!` 内容），
  `BUILTINS` 改为 8 个技能名；`builtin_label` 补三份中文名。
- `store::builtin_package` 改为从 `FILES` 组装整包；新建工作区、复制为用户副本、导出 ZIP/文件夹
  都带 references，不再只有 SKILL.md。
- 新增测试：磁盘资源与 `FILES` 一致、每份技能都有 SKILL.md 与参考文件、SKILL.md 里点名的
  `references/*.md` 真实存在、复制/导出/恢复内置都带参考文件、快照索引哈希与实际文件一致。
- 既有安全规则不变（路径、体积、文件数、深度、坏副本不回退、用户同名覆盖优先、公共
  `gongwen-markdown` 不可覆盖、快照隔离）。

### D3 只读风格工具

- `src/pi_agent/tools.rs` 新增 `PiStyles` 窄接口与生产实现 `StyleArchives`：只读
  `StyleBook` / `style::render`，不调第二个模型、不写库、不记使用次数、不执行 shell。
- 新工具 `style_list`（当前文种启用且适用的档案，最多 20 份，超限标 `truncated`）与
  `style_read`（按 id 读；未知 / 停用 / 不适用返回 `available:false` + `reason`，不换风格；
  文本有 token 与字符上限）。
- 结果固定标注 `style_sample`：只提供写法，不登记证据、不进 `fact_sources`；测试用注入的假风格，
  不读真实 `styles.json`。
- `pi-host/src/bridge.mjs` 注册同名工具与 schema，`prompt.mjs` 补工具说明和「不会保存回风格库」，
  host 测试覆盖工具清单与提示词。

### D4 可重复业务评测

- 新增 `tests/fixtures/pi-writing-eval/`：`README.md`、`cases.json`、`bridge.json` 与 8 个自造案例
  （材料起草、选区精简、复函未知态度、材料冲突、政策找不到原文、只提炼、研究报告出处、风格应用
  不搬事实），材料 / 表单要素 / 知识库 / 稿件库 / 词库 / 风格全部自造。
- 新增 `scripts/pi-writing-eval.mjs`：启动真实 `pi-host/src/main.mjs`，复制真实 8 份标准技能与
  完整 `gongwen-markdown` 语法包，搭建与生产同构的工作区、技能索引与 request；用夹具回答
  工具调用与 `ask_user`（只回答夹具给定的答案，不编新事实）；分开判定技术交付与业务判据；
  人工判断项单列；关键词覆盖明确标注「不是写作质量总分」。
  - CLI：`--case`、`--out`（默认 `tmp/pi-writing-eval/<run>`，必须位于仓库 `tmp/`）、
    `--model-config`（临时 JSON 或 `-` 读 stdin）、`--timeout`、`--check <已有运行目录>`、
    `--selftest`、`--list`。
  - 不默认读取 `.env`、`.pi/auth`、真实配置或测试密钥；拒绝 `.env`/`.pi/auth` 路径；没有显式
    模型配置就拒绝跑模型；`run.json` 只写脱敏模型标识（模型名、地址主机、窗口/上限），不保存
    init、带钥配置或完整请求；事件与 stderr 里的密钥串替换为 `[已隐藏]`。
  - 失败保留 `workspace/`、`events.jsonl`、`result.json` 与产物快照。
- 新增 `scripts/pi-writing-eval.test.mjs`：10 项离线单测（夹具自检、桥接夹具、判据的好/坏结果
  区分、选区外越界、清单/正文分流、工具失败、CLI 严格解析、模型配置脱敏与凭据文件拒绝、
  自带 `--selftest`），不需要模型、不加依赖。
- 新增 `docs/pi-writing-eval.md`（复现、指标定义、真实/夹具边界、输出结构与限制）与
  `docs/pi-skill-migration.md`；`docs/pi-runtime.md` 更新八份技能、内置包与风格工具说明。

## 二、实际文件

修改：

- `assets/pi-skills/gongwen-draft/SKILL.md`
- `assets/pi-skills/gongwen-revise/SKILL.md`
- `assets/pi-skills/gongwen-review/SKILL.md`
- `assets/pi-skills/gongwen-imitate/SKILL.md`
- `assets/pi-skills/gongwen-research/SKILL.md`
- `src/pi_agent/skills/mod.rs`、`store.rs`、`catalog.rs`、`snapshot.rs`
- `src/pi_agent/tools.rs`
- `src/pi_agent/process.rs`（仅一处注释：五份 → 八份）
- `pi-host/src/bridge.mjs`、`pi-host/src/prompt.mjs`、`pi-host/test/bridge.test.mjs`
- `docs/pi-runtime.md`

新增：

- `assets/pi-skills/gongwen-draft/references/material-drafting.md`
- `assets/pi-skills/gongwen-revise/references/{condense,normalize,polish,tone}.md`
- `assets/pi-skills/gongwen-review/references/fact-check.md`
- `assets/pi-skills/gongwen-imitate/references/style-analysis.md`
- `assets/pi-skills/gongwen-research/references/argumentation-and-sources.md`
- `assets/pi-skills/gongwen-reply/{SKILL.md,references/incoming-letter.md}`
- `assets/pi-skills/gongwen-extract/{SKILL.md,references/extraction-checklist.md}`
- `assets/pi-skills/gongwen-policy-check/{SKILL.md,references/citation-check.md}`
- `docs/pi-skill-migration.md`、`docs/pi-writing-eval.md`
- `scripts/pi-writing-eval.mjs`、`scripts/pi-writing-eval.test.mjs`
- `tests/fixtures/pi-writing-eval/{README.md,cases.json,bridge.json}`
- `tests/fixtures/pi-writing-eval/cases/{draft-from-material/materials/01-meeting.md,`
  `condense-selection/current.md,reply-unknown-stance/materials/01-incoming.md,`
  `material-conflict/materials/01-report.md,material-conflict/materials/02-report.md,`
  `policy-source-missing/current.md,extract-only/current.md,`
  `style-apply/materials/01-facts.md,style-apply/materials/02-sample.md}`

范围说明：

- 未改 `assets/agent-skills/` 旧内容、表单/正文交付/撤销、模型设置、Cargo 依赖与版本、打包脚本、
  Typst/预览/IME/导出/GWX；未做 4E 新 API 工具或 4F 删除。
- `docs/pi-implementation-plan.md` 在本轮开始时并不在 `git status` 里，之后显示有修改；内容
  是主代理写的「第四阶段 D 启动」，**不是本 worker 改的**，未动它。
- 临时验证脚本 `tmp/pi-eval-smoke.mjs`、`tmp/pi-eval-smoke-report.mjs` 与
  `tmp/pi-writing-eval/smoke*` 在 `tmp/`（生成物，不入库）。

## 三、命令与结果

环境：本机 Windows；Rust/Node 测试按 4C 主审记录把 `TEMP`/`TMP` 指到 `D:\gongwen\tmp`
（不设时会有 2 个与沙箱权限相关的临时目录失败，属环境问题）。

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过（先 `cargo fmt --all` 格式化本次改动的 Rust 文件） |
| `cargo check --locked --all-targets` | 通过 |
| `cargo clippy --locked --all-targets -- -D warnings` | 通过，无忽略警告参数 |
| `cargo test --locked --all-targets` | **2785 passed / 0 failed / 84 ignored** |
| `cargo test --locked pi_agent::` | 87 passed / 0 failed / 1 ignored |
| `cargo test --locked pi_agent::skills` | 27 passed / 0 failed |
| `cargo test --locked pi_skill_manager` | 6 passed / 0 failed |
| `cargo test --locked ai_panel::` | 133 passed / 0 failed / 7 ignored |
| `cargo test --locked skill_pack` | 10 passed / 0 failed / 1 ignored |
| `cd pi-host && node --test` | **49 passed / 0 failed / 1 skipped**（跳过的是本机 symlink 能力限制，与 4C 相同） |
| `node --test scripts/pi-writing-eval.test.mjs` | 10 passed / 0 failed |
| `node scripts/pi-writing-eval.mjs --selftest` | 通过（不连模型） |
| `node scripts/pi-writing-eval.mjs --list` | 8 个案例全部列出 |
| `node scripts/pi-writing-eval.mjs --check tmp/pi-writing-eval/smoke` | 重新判定通过 |
| `git diff --check` | 通过（仅有 Git 的 LF→CRLF 提示，不是空白错误） |

另做两次**本机 HTTP 桩**链路冒烟（不是模型效果）：正文案例 `condense-selection` 与清单案例
`policy-source-missing` 各跑一遍真实 host，技术判据 12/12、7/7，业务判据 5/5、3/3；
产物在 `tmp/pi-writing-eval/smoke*`，凭据检查未发现桩密钥出现在 `run.json`/`report.md`/`events.jsonl`。

## 四、失败、限制与已知坑

1. **没有真实模型业务评测**：按分工，真实端点与密钥由主代理注入执行；worker 只跑离线单测与
   本地 HTTP 桩。桩链路通过不代表写作质量。
2. **评测的资料源是夹具**：`check_document` 是脚本侧的简化字面出处检查，不是生产 Rust 闸门；
   知识库 / 稿件库 / 词库 / 风格都是自造，不冒充内网联机。
3. **没有 GUI 目视**：技能管理页只跑了既有 egui 渲染回归；新增标签/描述的实际观感未目视。
4. **一次回归冲突已按范围内方式解决**：`ai_panel::ui::tests::a_slash_prefix_only_offers_and_pins_pi_skills`
   用 `contains("润色")` 判「不列旧技能」，而新 `gongwen-revise` 的描述里出现了「润色」。
   为避免改范围外的测试，改成在技能描述里不出现该旧技能名（正文仍可提「润色」），测试通过。
   若主代理认为该测试断言应改成精确匹配，可另行处理。
5. **测试环境**：不设 `TEMP`/`TMP` 到仓库 `tmp` 时，`snapshot`/`process` 两个用例会因沙箱拒绝
   临时目录写入而失败；设置后全绿。未为环境放宽任何判据。
6. **未覆盖能力**（详见 `docs/pi-skill-migration.md`）：
   - AI 学出风格档案并保存进风格库、描述类要素建议卡接入 Pi、通用附件协议；
   - 旧 DSL 的 `calc.*` / `http.call` / `finding.add` 工具族（4E 未开工）；
   - 旧程序算子：`fit_length` 字数循环、`plan split/list` 程序化要点拆分、`vocab.normalize`
     自动替换、引文程序比对与「审校抽屉逐条采纳」；
   - 评测不覆盖内网知识库检索质量、真实文档结构与权限。
7. **样式/引用**：研究报告没有文献库时判据禁止 `[@` / `doi:`，但模型若写了「参考文献」段但
   没有 `[@key]`，脚本不会判失败；这属于人工判断项。
8. **`--out` 限制**：必须位于仓库 `tmp/` 下，避免误写；需要其他位置要先改脚本或复制结果。
9. **未提交/推送/发布**：按要求到此停止，等待主代理 review。
