# 会议纪要文种设计

> 状态：方案已确认，第一期完成（2026-10-27）。样例：`examples/huiyijiyao.docx`。
> 四处岔路已由用户定下：名单紧随正文、名单网格排超宽退连排、红头取表单而标题可选、
> 「一是……。」自动楷体。

## 一、样例拆解

逐段读 `document.xml` 得到的版式（页面 A4，上 37 / 下 35 / 左 28 / 右 26 mm，
正文行距固定 28.95pt，与现有公文一致）：

| 位置 | 内容 | 字体字号 | 排法 |
|---|---|---|---|
| 第 1 行 | 份号 `00X` | 黑体三号 | 顶格 |
| 第 2 行 | `密级★期限` | 黑体三号 | 顶格 |
| 空一行 | | | |
| 纪要标志 | `【单位名称】会议纪要` | 方正小标宋 26pt，红色 | 居中 |
| 空一行 | | | |
| 编号 | `〔202X〕  号` | 仿宋三号 | 居中，无机关代字 |
| 红色分隔线 | | 1.5pt 红 | 紧贴编号行下方，通栏 |
| 空一行 | | | |
| 正文首段 | 时间、地点、人物、事件六要素简述 | 仿宋三号 | 首行缩进二字 |
| 正文 | `会议认为，……。一是……。二是……。` | 仿宋三号；`一是……` 到句号为楷体 | 「会议认为 / 指出 / 要求 / 明确」各成一段 |
| 名单 | `出席：` `列席：` `请假：` | 标签黑体三号，人名仿宋三号 | 无框表格，左空二字，人名每行 6 格居中，两字名补成三字宽 |
| 版记 | `分送：……` / `抄送：……（共印30份）` / `印发机关　……印发日期印发` | 仿宋四号 | 首末粗线（1.5pt），抄送与印发之间细线（0.7pt），贴末页底 |
| 页脚 | `— 6 —` | 四号 | 居中 |

样例里名单前垫了二十几个空段，用户定的是**按国标紧随正文**，不照样例推到底。

## 二、版式规格（PDF 与 Word 一致）

### 版头

1. **份号**：版心第 1 行顶格，黑体三号。沿用公函的份号逻辑（`number_copies` 关时固定
   `01`，开时逐份编制，`format_copy_number` 两位）。样例写的是三位 `00X`，这里统一沿用两位，
   不另开一套。
2. **密级**：第 2 行顶格，复用 `security-line`（含「指人专办」）。
3. 空一行。
4. **纪要标志**：方正小标宋 26pt 红色居中。文字取表单「纪要名称」，默认
   「发文单位＋会议纪要」，可改成「××局局长办公会议纪要」。超宽时与红头一样压字距。
5. 纪要标志与编号之间不空行（2026-10-28 按用户要求调整）。
6. **编号**：`〔2026〕3号`，仿宋三号居中。年份与序号复用 `document_year` / `document_number`
   （授权类）；没有机关代字。预览版与现有文种一致：保留位置、不写流水号。
7. **红色分隔线**：通栏，粗细同公函红线（0.53mm），在编号行基线下约 4mm。
8. 空一行。

### 主体

9. **标题（可选）**：正文写了 `# 标题` 就在这里排小标宋二号居中标题，后空一行；没写就直接
   接正文。标题是描述类要素，规则不变。
10. **正文**：仿宋三号，首行缩进二字；`##` / `###` 可用（多议题纪要分节），编号规则与其它
    文种相同。附件、附件说明照常支持。
11. **「一是」楷体**（只对会议纪要生效，三端统一）：正文段落里，`一是`……`十是` 出现在段首或
    紧跟 `。；：` 之后时，从序号起到下一个 `。` 之前排楷体（句号本身不变）。边界：前一个字是
    其它汉字（「统一是」「唯一是」）不算；到段尾都没有句号、或这一截超过 40 字，就只把
    `X是` 两字排楷体，避免整段变楷体。

### 名单

12. 正文（有附件说明则在附件说明）之后**空一行**排名单，顺序：出席、列席、请假；**空项不印**，
    三项都空就不排。
13. 标签左空二字，`出席：` 黑体三号；人名仿宋三号。
14. **网格**：标签之后的宽度均分 6 格，人名居中；两字名中间插一个全角空格凑成三字宽
    （样例写法）。一行排满换行，回行从第一格起。
15. **退连排**：某一项里有任何一个名字放不进一格（如带单位「××局张三」、四字以上姓名），
    这一项改成连排：`出席：甲、乙、丙`，回行与冒号后首字对齐。各项独立判断。

### 版记

16. 贴末页版心底，复用公函版记「量剩余高度、放不下自己换页、`v(1fr)` 推到底」的写法。
17. 行与线：顶粗线 → `分送：……`（若有）→ `抄送：……`（若有）→ 细线 → `印发机关`（左空一字）
    与 `××年×月×日印发`（右空一字）→ 底粗线。分送、抄送冒号后回行悬挂对齐；仿宋四号。
    线宽沿用现有版记（粗 0.6mm、细 0.3mm），不按样例另设一套。
18. **印数**：`（共印N份）` 排在分送 / 抄送中最后一行的右端（同公函版记写法）。N 自动算：
    出席 + 列席 + 分送人数 + 抄送单位数，不另设填写框（与公函「份数就是版记那个数」的口径一致）。
    分送、抄送都空时不印印数。
19. **印发日期**复用 `draft.date` / `date_is_auto`（会议纪要没有落款与成文日期，这个字段在
    表单上就叫「印发日期」）；预览版只写到月、日空着（样例写法）。

### 页码

20. 沿用现有：单面居中、双面奇右偶左、首页不印。

## 三、数据模型

新增枚举值 `TemplateKind::MeetingMinutes`，标签「会议纪要」，排在会议议程之后；
`manuscript.rs` 的稿件串为 `"MeetingMinutes"`。旧稿不受影响。

字段尽量复用，只为会议纪要独有的东西新开一个结构，挂在 `DraftInput` 上，空值不写入快照
（同 `phone_record`，旧稿校验值不变）：

```rust
/// 会议纪要独有的要素；空值不写入旧稿快照。
pub struct MinutesMetadata {
    pub heading: String,     // 纪要名称（红头文字），空时取「发文单位＋会议纪要」
    pub nonvoting: String,   // 列席，顿号分隔
    pub absent: String,      // 请假，顿号分隔
    pub distribute: String,  // 分送（人名），顿号分隔
    pub printing_unit: String, // 印发机关，空时取发文单位
}
```

| 要素 | 存放 | 分级（红线 2） |
|---|---|---|
| 密级、保密期限 | `profile.security_*`（已有） | 授权 |
| 份号 | `profile.number_copies`（已有） | 授权 |
| 发文单位 | `profile.issuing_unit`（已有） | 描述 |
| 纪要名称 | `minutes.heading`（新） | 描述 |
| 编号年份、序号 | `profile.document_year` / `document_number`（已有） | 授权 |
| 会议时间、地点 | `meeting_time` / `profile.meeting_location`（已有，喂给模型写首段） | — |
| 出席 | `attendees`（已有，会议议程同字段） | 描述 |
| 列席、请假、分送 | `minutes.*`（新） | 描述 |
| 抄送 | `profile.copies_to`（已有） | 描述 |
| 印发机关、印发日期 | `minutes.printing_unit`（新）、`date`（已有） | 授权 |

`TemplateKind` 的几个判定函数：`has_copy_numbering` 加上会议纪要；`has_document_number`
的含义是「代字＋年份＋序号」，会议纪要另加 `has_minutes_number`，不混用，免得表单和校验
去找代字。

## 四、表单（起草页）

- **版头**（副标题「密级 · 纪要名称 · 编号」）：密级、保密期限、指人专办、逐份编制份号、
  发文单位（下拉，同其它文种）、纪要名称（默认值随发文单位带出，可改）、年份、序号。
- **主体**（「会议时间 · 地点 · 人员」）：会议时间、会议地点、出席、列席、请假。
  出席、列席、请假与会议议程的「参会人员」同一种多值输入。
- **版记**（「分送 · 抄送 · 印发」）：分送、抄送、印发机关（默认发文单位）、印发日期、
  双面打印。
- 必填（`check_form` 与 `validator` 同步）：密级、发文单位、出席；正式版另要求序号、印发日期。

## 五、AI 侧

1. `kind_rules`：会议纪要的正文规则——首段写会议六要素（时间、地点、主持人、出席范围、
   议题，素材有什么写什么，取锁定元数据）；其后按「会议听取了……」「会议认为 / 指出 /
   要求 / 明确 / 决定，……」分段；段内枚举用「一是……。二是……。」，每项先用一句概括
   以句号收住再展开；多议题用 `##` 分节。不写落款、日期、名单、分送抄送和版记。
2. 输出范围：标题可选（只在专题纪要时写），名单、版记一律由程序排，写了就是重复。
3. `style_guide` 照常适用；行文方向取 `Unknown`（纪要既记录又部署）。
4. 澄清关键词：`纪要`、`会议纪要`、`办公会纪要` 映射到会议纪要（`agent/clarify.rs`）。
5. 本期**不**给出席、列席、请假、分送接 AI 要素建议（要素填写框架的 `element_fields` 只登记
   为描述类、不出题）；要接再单开一期，按词库人员词条走 `apply_field`。

## 六、技能包与帮助

- `skills/gongwen-markdown/templates/meeting-minutes.md`（新增，登记进 `skill_pack::FILES`），
  `references/document-types.md` 加一节，`SKILL.md` 的文种数 8 → 9。
- `check_gongwen_md.py` 加会议纪要判据（禁止手写名单、版记、落款）。
- 使用帮助在文种一章加会议纪要一节，配一张样张截图。

## 七、改动面

| 区域 | 文件 |
|---|---|
| 模型 | `src/models/template.rs`、`src/models/draft.rs`、`src/manuscript.rs`、`src/storage.rs` |
| PDF | `assets/typst/gongwen.typ`（`minutes-header`、`minutes-roster`、`minutes-record`）、`src/export/typst/frame.rs`、`data.rs`、`runs.rs`（一是楷体） |
| Word | `src/export/docx.rs`、`src/export/docx/content.rs`、`src/export/docx/record.rs`（新版记三行）、新增 `src/export/docx/minutes.rs`（版头与名单） |
| 预览 | `src/preview/header.rs`、`src/preview/tail.rs`、`src/preview.rs` |
| 表单与校验 | `src/draft_page/form.rs`、`src/validator.rs`、`src/element_fields.rs`、`src/export/element_display.rs` |
| AI | `src/prompt.rs`、`src/agent/clarify.rs`、`src/agent/elements.rs`、`src/proofread_rules.rs`、`src/rich_paste/lists.rs` |
| 技能包 / 帮助 | 见第六节 |
| 测试 | `typst/sample_tests.rs` 加样张、`page_count_tests.rs`、`docx/tests.rs`、`validator` 与 `runs` 单测 |

其余 `match kind` 的地方由编译器逐个报出来，按「同会议议程」或「同普通公文」归类补齐。

## 八、分期

1. **模型与 PDF**：枚举、`MinutesMetadata`、表单、校验、Typst 版头 / 名单 / 版记、一是楷体；
   出样张（`cargo test --locked typst_samples -- --ignored`）与样例逐项目视对照。
2. **Word 与预览**：Word 导出（名单用无框表格，同样例）、应用内预览，与 PDF 对照。
3. **AI 与技能包**：提示词、澄清、技能包模板与判据、使用帮助。

每期 `cargo fmt` / `clippy -D warnings` / `test` 过了单独提交。

## 九、已知取舍

- 份号两位、线宽沿用现有版记，与样例（三位、1.5pt）略有出入，换来全系统统一。
- 名单不推到底（按国标），与样例视觉不同。
- 印数自动算，不能手改；若常需手改，再加一个覆盖字段。
- 网格每行固定 6 格；更窄的纸面（横页）会议纪要不支持横排。

## 十、进度与交接

### 版头间距调整——2026-10-28

按用户要求，红色纪要标志与编号之间不空行；预览、Typst PDF 与 Word 同步去掉
这一处额外空行。第一节保留原始样例的拆解，现行规格以第二节为准。

### 第一期（模型、表单、校验与 PDF）——已完成，2026-10-27

- 枚举 `TemplateKind::MeetingMinutes`、`MinutesMetadata`（`src/models/draft.rs`，空值不写快照）、
  `has_minutes_number`、`has_copy_numbering` 加会议纪要；稿件串 `"MeetingMinutes"`。
- 起草页表单三段（`src/draft_page/form.rs`）：版头发文单位 / 纪要标志 / 编号
  （`minutes_number_row`），主体会议时间 / 地点 / 出席 / 列席 / 请假，版记分送 / 抄送 /
  印发机关 / 印发日期 / 双面 / 份号；`check_form` 与 `validator` 同口径必填。
- 印数与份号清单：`docx::record::minutes_recipients`（出席、列席、分送、抄送依次），
  `automatic_print_copies` 与 `copy_roster` 都走它。
- 文字规则集中在 `src/export/minutes.rs`：`roster`（空项不出、逐项判网格 / 连排）与
  `enumeration_kai_spans`（「一是」楷体范围），第二期的 Word 与预览直接复用。
- Typst：`frame.rs` 的 `minutes_frame`，模板 `minutes-header` / `minutes-roster` /
  `minutes-record-block` / `minutes-doc`；版记贴底与函稿共用 `pin-to-bottom`（原
  `footer-record` 拆出来的）。片段新增 `h` 标记（楷体三号），只有会议纪要正文段会带。
- 样张：`typst_samples` 里的 `minutes`（网格名单、无标题）与 `minutes-titled`（带标题、
  连排名单、预览版），会议纪要用例另出 `page-N.png`。
- 提示词（`prompt.rs`）已给出可用的会议纪要规则与输出范围，第三期再细化并补测试。

**与方案的出入**

- 「一是」楷体的截止标点放宽为「。；！？」中最先出现的那个（方案只写了句号）：分号隔开的
  「一是……；二是……」不放宽的话会把后面几项一起排成楷体。
- 名单网格两字名按样例中间空一字，格宽约 3.8 字，格间空隙（约 0.8 字）比名内空隙略窄，
  与样例同样观感；没有另做调整。

### 第二期（Word 与预览）——已完成，2026-10-27

- 共用规则再收进 `src/export/minutes.rs`：`minutes_heading`（纪要标志）、`minutes_number_text`
  （编号）、`kai_pieces`（按楷体范围切片段）。PDF、Word、预览三端的「一是」切点都从
  `enumeration_kai_spans` + `kai_pieces` 取，不各写一套。
- Word（`src/export/docx/minutes.rs`）走公文主链 `write_docx_with_numbering`，只在三处
  分叉：版头（`add_header`：份号、密级、纪要标志、编号放进一格无框表、表下边框即红线）、
  正文段（`body_paragraph` 多一道楷体）、名单（`add_roster`：网格是无框表，连排是悬挂缩进段）
  与版记（`add_record`：浮动表贴版心底，和函稿版记共用 `record::add_record_table`）。
  没写 `# 标题` 不排标题；附件、附件说明照常。
- 预览（`src/preview/minutes.rs`）：`header` / `roster_block` / `record_block`，由
  `header_block`、`official_preview`、`footer_record` 分派；「一是」楷体靠 `Metrics` 的
  `enumeration_kai` 开关在 `append_numbered_inline` 里切。预览不分页，版记紧跟名单。
- 样张：`typst_samples` 的两个会议纪要用例另出 `word.docx`；拿本机 Word 转 PDF 与
  `typst.pdf` 对照过，版头、名单（网格 / 连排）、版记三行与印数一致。
- 测试：`docx/tests.rs` 的 `meeting_minutes_docx_has_header_roster_and_record`、
  `preview.rs` 的 `minutes_preview_draws_header_roster_and_record_in_order`，第一期临时跳过
  会议纪要的两个 Word 测试已恢复。

**出入与已知坑**

- Word 里纪要标志用 34pt 固定行高（字号 26pt，按正文网格的 28.85pt 会被 Word 裁掉字顶），
  多出来的 103 缇从它上面的空行扣回；编号到红线的 4mm 用单元格下边距 113 缇撑出，同样从
  下一个空行扣回，后面的行仍在网格上。
- Word 版只出一份，份号固定 01（与函稿 Word 一致）；逐份编号只在 PDF。
- 预览没在真机界面里目视过（开发版窗口没截到图），只靠预览测试核对了内容与先后顺序；
  第三期顺带在界面里看一眼。

### 第三期（提示词、技能包与使用帮助）——已完成，2026-10-27

- 提示词（`src/prompt.rs`）：锁定元数据给会议纪要另加列席、请假与纪要名称；
  `strip_metadata_echoes` 只对会议纪要剔除模型照抄的「出席 / 列席 / 请假 / 分送：」行、
  纪要标志（含写成 `# ` 的）、编号与「……印发」行；联机抽查加了一个局长办公会场景，
  硬违例查列表与名单版记。
- 澄清（`src/agent/clarify.rs`）：「会议纪要」「纪要」映射到会议纪要。顺带修了一个因此暴露的
  问题：`kind_question` 原来对整段 request 找文种词，`@` 引用材料的标题（如「10月10日会议纪要」）
  会被当成要写的文种；现在只看「【要求】」之后那段。
- 审校（`src/proofread_rules.rs`）：标题规则对会议纪要生效、写了标题必须以「纪要」结尾；
  没写标题时仍按「纪要」判夹带请示结语。
- 技能包：新增 `templates/meeting-minutes.md`（不写 `# 标题`）并登记进 `skill_pack::FILES`；
  `document-types.md` 加第 6 节（原 6–8 节顺延为 7–9）、表单要素与附件、文种选择、模板表；
  `SKILL.md` 九种文种、最小骨架、禁忌表与自检清单；`check_gongwen_md.py` 认会议纪要，
  标题可选、禁列表、禁名单版记行，自测 15/15；`skill_pack` 的模板与模型产物判据对会议纪要
  放开「第一块必须是标题」（至多 1 个）。`scripts/gongwen-skill-eval.py` 加会议纪要场景，
  基线变成 3 模型 × 7 文种 × 2 条件 = 42 次。
- 使用帮助：`05-profiles.md` 九种文种表与「会议纪要」一节（Mermaid 纸面结构图、表单字段、
  排版规则），其余三章的「八种」改「九种」；README 文种清单同步。

**出入与已知坑**

- 方案第五节写的 `element_fields` 登记描述类没有做：出席、列席、请假、分送暂不接 AI 要素建议，
  要接再单开一期（按词库人员词条走 `apply_field`）。
- 方案第六节说帮助配一张样张截图，改成了 Mermaid 纸面结构图（帮助的示意图约定一律用
  Mermaid，界面截图要真机截）。
- 没跑真实模型的技能评测（`gongwen-skill-eval.py` / `model_outputs_survive_the_parser_and_validator`），
  会议纪要场景已备好，等有密钥时跑一次。
- 整本帮助 PDF 有 Typst「未收敛」提示，加这一节前后条数相同，是原有问题。
