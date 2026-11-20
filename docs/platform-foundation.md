# 平台内核（地基）方案

状态：草案，2026-11-20。范围：正文检索、制度文件有效性、通讯录导出、发文字号台账、文件汇编、会议材料（秩序册后置）。

这份文档只定“地基”和“接法”，不写具体界面。目标不是把所有代码重写一遍，而是把已经出现多份的算法收成几块，让新功能不再各开一套库、各拼一次文号、各写一条加密路径。

## 0. 2026-11-20 范围修订

- **通讯录要两列电话**：词库人员从“一个联系电话”改为“办公电话 + 手机”；旧数据、旧 Excel 模板、校验和表单都要兼容。旧号码只有 11 位手机号进“手机”，其余（区号、短号、8 位固话）一律进“办公电话”；承办联系人取号办公优先、手机回落。
- **三个独立文号池**：公函、红头呈批件、会议纪要各自编号，不共用序列。**纪要代字是单位属性**：一个单位要么有、要么没有；有就所有纪要都带 `某纪〔2026〕3号`，没有就都保持 `〔2026〕3号`。同一单位同一年度不会同时出现两种。台账按“单位 + 年度”成池，代字只影响显示。
- **台账允许手工补录**：纸质件、旧年文件可以只登记、不对应稿件库记录。
- **不做批量套打**：席卡、桌牌、证书、信封、档案盒、标签都不在本次范围；会议签到也不做。
- **会议不建独立实体**：通知就是公函里的电话通知，议程就是现有会议议程，主持词和须知就是普通公文；相关文件用“文件汇编”从文稿库里选。秩序册等前几项做完再单独做，不纳入本次地基。
- **文件汇编**是会议材料的共同出口：从文稿库勾选要打印的文件，排序后合并成一份 PDF；议程、秩序册以后也走这条路。

## 1. 为什么要收口

单篇公文这条链已经完整：要素、版式、校对、打印、版本、加密、光盘。问题是同一件事在多处实现，改一处容易漏另一处：

- 文号显示至少有三种算法：`models/draft.rs::document_year`、`manuscript/search.rs::number_label`、`export/naming.rs::prefix`，导出数据层还会再拼一次。
- 密级有两套取数：`DraftInput::security_marking` 会分研究报告（读 `research.security`），而 `export/element_display.rs::security_parts` 只读 `profile.security_level`。列表、版式、送批目录各自决定用哪套，靠调用方自觉。
- 检索至少有四种：`quick_find` 的拼音模糊匹配、稿件库的 SQL `LIKE`、知识库/维基的 FTS5、任务助手的搜索。分词、片段、排序各写各的。
- 导出至少有五条路：`export::export_artifacts`、送批材料合并、打印、`proofread_xlsx`/`vocabulary_xlsx`、销毁审批表/签收单/传发任务单。加密、文件名、页数、清理旧成品各写各的。
- 建表有 8 个 `ensure_schema` 入口、56 处 `CREATE TABLE`，迁移顺序靠 `ManuscriptStore::migrate` 里的手工调用列表。
- 模板判断散在 `TemplateKind` 的 match 分支里，Typst、DOCX、预览、表单、校验各看一遍。

这些都不算错，但每加一个功能就要再抄一遍，最后一定出现“列表里是 A、导出里是 B”的不一致。这几个新功能里，检索、台账、通讯录、汇编都会踩到这些老路，所以先把地基做对。

## 2. 边界

**做：**

- 新功能只通过平台取数、检索、渲染、校验；
- 同一概念只有一个实现，其他位置只做适配；
- 旧代码不重写，碰到哪块迁哪块；
- 平台不依赖 UI，也不依赖模型，非 AI 版必须能编译、能跑。

**不做：**

- 不拆 crate，仍在单 crate 内按模块分层；
- 不做通用 OA/审批流/权限树；
- 不做批量套打、通用套打设计器、自由画布；
- 不改现有 AI 链路，也不让平台依赖 AI 模块；
- 不做“大爆炸”式重构，不追求一次把旧代码全迁完。

## 3. 分层

```text
页面 / 后台任务（egui）
    ↓ 只编排：收集输入、调用用例、展示结果
应用服务（SearchService / LedgerService / RenderService / MeetingService）
    ↓ 事务、权限、加密、错误
平台内核（Doc / Db / Search / Render / Ledger / Check / Template）
    ↓ 纯数据与算法，不画界面
适配器（SQLCipher / Typst / DOCX / XLSX / 合并 / 打印 / 系统字体）
```

三条硬规矩：

1. **页面不直接开库、拼文号、拼文件名、做加密、拼模板。** 页面只调应用服务，拿 `Artifact`、`SearchHit`、`Check` 这类结果。
2. **内核不依赖 UI 和 AI。** 内核只依赖 `models`、`storage` 这类底层能力；`#[cfg(feature = "ai-workbench")]` 的代码不能进内核。
3. **旧代码是适配器，不是被抛弃的包袱。** 现有 `ManuscriptStore`、`export::export_artifacts`、`validator` 先包一层接进平台，等新功能稳定后再逐步替换内部实现。

## 4. 内核与契约

### 4.1 Doc：通用数据（单位、人员、文号、文稿）

先有三组通用数据，上面所有功能都用它们，不再各取各的：

```rust
pub(crate) struct UnitCodes {
    pub letter: String,     // 发函代字
    pub approval: String,   // 呈批代字
    pub minutes: Option<String>, // 纪要代字；None = 本单位纪要无代字
}

pub(crate) struct PersonContact {
    pub office: String,     // 办公电话
    pub mobile: String,     // 手机
}

impl PersonContact {
    /// 承办联系人取号：办公优先，没有办公再用手机。
    pub fn preferred_office_first(&self) -> &str;
    /// 手机只认 11 位、1[3-9] 开头；其余（区号、短号、8 位固话）都算办公。
    pub fn is_mobile(number: &str) -> bool;
    /// 旧 `phone` 兼容：手机号进 mobile，其余进 office；空串保持空。
    pub fn migrate_legacy(phone: &str) -> Self;
}

pub(crate) enum NumberingScheme { Letter, Approval, Minutes }

impl NumberingScheme {
    /// 公函 → Letter，红头呈批件 → Approval，会议纪要 → Minutes；
    /// 其他文种没有编号，返回 None。
    pub fn for_kind(kind: DocKind) -> Option<Self>;
    /// 纪要的代字可以为空（单位没设就不印），公函/呈批件必须有。
    pub fn code_is_optional(self) -> bool;
    pub fn code<'a>(&self, codes: &'a UnitCodes) -> Option<&'a str>;
}

pub(crate) struct DocNumber {
    pub scheme: NumberingScheme,
    pub code: String,
    pub year: String,
    pub serial: String,
}

impl DocNumber {
    /// 按 kind 选 scheme；代字取表单里的 `profile.department_code`。
    /// 纪要以单位为准：单位有纪要代字就带出，没有就保持空；
    /// 同一单位不允许逐篇切换“有字/无字”。
    pub fn from_draft(draft: &DraftInput, codes: &UnitCodes) -> Option<Self>;
    /// 有代字：`某纪〔2026〕3号`；无代字：`〔2026〕3号`（兼容旧纪要）。
    pub fn display(&self) -> String;
}

pub(crate) struct DocumentMeta {
    pub id: DocId,                 // 稿件 / 登记簿 / 台账条目
    pub title: String,
    pub kind: DocKind,
    pub number: Option<DocNumber>,
    pub date: Option<NaiveDate>,
    pub security: SecurityMark,    // 密级 + 期限，研究报告也走这里
    pub recipients: Vec<Recipient>,
    pub copies: u32,
    pub signer: Option<PersonRef>,
    pub status: DocStatus,
}
```

- **文号只按 `NumberingScheme` 算一次**：公函用发函代字，红头呈批件用呈批代字，会议纪要用纪要代字。生效代字仍存 `profile.department_code`；词库的 `UnitCodes` 负责带出和校验。纪要是单位属性：单位有纪要代字就带，没有就不带，同一单位不会一半有一半没有。三者的池不同，但格式化、查重、找断号是同一套算法。
- 表单的“文号/编号”行只按 scheme 换标签和候选，不再写 `minutes_number_row` 这种专用分支。选发文单位时按单位带出或清空：单位有纪要代字就只读显示，没有就不显示代字输入框；只有手填的陌生单位才允许手填代字。旧稿没有代字时仍按 `〔2026〕3号` 显示。
- 人员电话只认 `office` / `mobile` 两列；旧 `phone` 只在反序列化时兼容，载入时用 `migrate_legacy` 拆走，之后代码里不再出现“一个联系电话”。
- 承办联系人、收件人、传输单、签收单统一用 `preferred_office_first()`：办公优先，手机回落。校验时两个号码都算命中，但默认填办公。
- 已知单位的纪要编号形态由单位决定，表单不提供“这一篇加不加代字”的开关；`Check` 加一条：已知单位的纪要代字与词库不一致时提示（有字没带、无字却带了），不自动改稿。
- `DocumentMeta::from_draft` 是唯一入口；列表、搜索、导出、台账、汇编都用它，不再让 `number_label`、`prefix`、`document_year` 各拼一遍。
- `SecurityMark` 负责“密级 + 期限 → `机密★20年`”，研究报告与普通公文都在这里归一。

### 4.2 Db：一个开库口、一张迁移表

```rust
pub(crate) struct Db { /* SQLCipher connection */ }

impl Db {
    pub fn open(path: &Path) -> Result<Self>;       // 统一 apply_db_key + PRAGMA
    pub fn transaction<T>(&mut self, f: impl FnOnce(&Transaction) -> Result<T>) -> Result<T>;
    pub fn migrate(&mut self, migrations: &[Migration]) -> Result<()>;
}
```

- 所有新表在一个 `Migration` 列表里登记，版本号只增不减；旧的 `ensure_schema` 先保留，新增表不再各写一个入口。
- `Db::open` 统一做 `storage::apply_db_key`、`foreign_keys`、WAL、`busy_timeout`；以后不允许模块自己 `Connection::open`。
- 后台线程各开各的连接（`rusqlite::Connection` 不能跨线程共享），但都走 `Db::open`，密钥、PRAGMA、迁移只有一条路。

### 4.3 Search：一个查询、一种命中

```rust
pub(crate) struct SearchQuery {
    pub text: String,
    pub corpus: Corpus,            // 稿件正文 / 标题 / 登记簿 / 词库 / 维基
    pub filters: SearchFilters,    // 密级、年度、单位、文种、状态
    pub limit: usize,
}

pub(crate) struct SearchHit {
    pub doc: DocId,
    pub score: f32,
    pub snippets: Vec<Snippet>,    // 片段 + 字符区间
}
```

- `Tokenizer` 统一 jieba + 汉字二元组；FTS5 的建表、触发器、补建、删除清理收进一个 `SearchIndex` 适配器。
- 稿件库正文检索是第一个使用者；知识库、维基以后可以换成同一个 `SearchIndex`，但它们的 `knowledge_local_fts` / `wiki_fts` 先不动。
- `quick_find` 的拼音模糊匹配是另一种查询（命令面板），不强行并进 FTS；它只复用 `Snippet` 和排序结果的形状。

### 4.4 Render：一次渲染、一个产物

```rust
pub(crate) struct RenderPlan {
    pub source: SourceRef,         // 稿件 / 多篇汇编 / 外部 PDF
    pub template: TemplateId,
    pub format: OutputFormat,      // Pdf / Docx / Xlsx / TypstPackage
    pub options: RenderOptions,    // 编号、份号、页码、目录、页眉
    pub protection: Protection,    // 明文 / 独立密码 / 工作区密钥
}

pub(crate) struct Artifact {
    pub bytes: Vec<u8>,            // 内存成品，打印/加密都不落明文临时文件
    pub file_name: String,
    pub pages: Option<u32>,
}
```

- `Renderer` 适配 Typst、DOCX、XLSX、PDF 合并；`BatchPlan` 负责“多篇 → 目录 + 连续页码 + 一个 PDF”，文件汇编、会议相关文件都走它。
- 文件名、42 字符上限、份号清单、签收单/传输单的命名收进 `Render`，不再散在 `export/naming.rs` 和各调用点。
- `Protection` 是唯一加密入口：明文、独立导出密码、工作区密钥三种模式，谁都不能绕过去直接写盘。

### 4.5 Ledger：登记不是一张大表

```rust
pub(crate) struct LedgerEntry {
    pub doc: DocumentMeta,
    pub direction: Direction,      // 发文 / 收文 / 外部引用
    pub pool: Pool,                // 单位 + NumberingScheme + 年度；纪要以单位为池
    pub status: LedgerStatus,      // 已用 / 占号 / 废号 / 现行 / 废止
    pub links: Vec<DocLink>,       // 稿件 UUID、登记簿 id
    pub note: String,
}
```

- **不合并成一张大表**：发文台账 `issue_register`、外部文件/制度 `document_registry` 各自建表，共享 `DocumentMeta` 和 `DocLink`；台账条目允许 `manuscript_id` 为空，手工补录纸质件。
- `LedgerStore` 只定义“查、写、查重、找空号、统计”这几个动作；发文台账按 `NumberingScheme` 分三个池，池键是“单位 + 年度”，不是代字；纪要有代字/没代字都算同一单位的同一序列，代字只影响显示。池内用“年度 + 序号”查重、找空号；制度清单只做状态与替代关系。
- 制度有效性只加状态与替代关系，不把登记簿改成审批流。

### 4.6 Check：一条提示、一个来源

```rust
pub(crate) struct Check {
    pub rule: RuleId,
    pub level: CheckLevel,         // 必错 / 疑似 / 提示
    pub message: String,
    pub span: Option<Range<usize>>,
    pub fix: Option<Fix>,
    pub basis: &'static str,
}
```

- `validator`、`proofread_rules`、`proofread` 词表、孤行探针、打印检查先各出一个适配器，统一成 `Vec<Check>`；UI 的审校抽屉只认 `Check`。
- 新功能（制度已废止、名单倒序、交付检查单）只加规则，不再往 `warnings: Vec<String>` 里塞自由文本。
- `Check` 带 `span` 就能跳转，带 `fix` 就能一键改；没有位置的检查（如“文号重号”）`span = None`，在检查单里单独分组。

### 4.7 Template：后置，不在地基里

本次不引入 `TemplateDescriptor` 和模板注册表。现有 `TemplateKind` 够用：会议通知就是电话通知，议程就是会议议程，主持词和须知就是普通公文，都走现有版式。

等秩序册真要做时再评估：先写一份具体模板，出现第二个同类需求再收注册表；不为未来先建抽象。新模板也不新增 `TemplateKind`，优先“骨架 + 现有版式”。

### 4.8 会议材料：不建独立实体

- 通知 = 公函里的电话通知（`PhoneNotice`）；
- 议程 = 现有会议议程（`MeetingAgenda`）；
- 主持词、须知 = 普通公文（`PlainDocument`）；
- 相关文件 = 文件汇编：从文稿库勾选要打印的文件，排序后合并成一份 PDF；
- 会议公函是主件，议程、秩序册、相关文件走现有附件说明/合并件，不建 `MeetingInfo`、会议包实体。

秩序册后置：它需要从标准词库名单里勾选填充，单独设计模板，等前几项做完再另立方案。本次不做签到表、席卡、座次表。

### 4.9 通用算法清单

| 算法 | 入口 | 用在哪 |
|---|---|---|
| 手机/办公分类 | `PersonContact::is_mobile` | 旧数据迁移、通讯录导入 |
| 取联系人号码 | `PersonContact::preferred_office_first` | 承办联系人、收件人、传输单、签收单 |
| 选编号池/代字 | `NumberingScheme::for_kind` + `UnitCodes::code` | 公函、呈批件、纪要的表单/校验/台账（纪要按单位带出） |
| 编号格式化 | `DocNumber::display` | 列表、导出文件名、版头、台账 |
| 查重/断号 | `LedgerStore::check_pool` | 发文台账、纪要台账，以后收文 |
| 取文稿事实 | `DocumentMeta::from_draft` | 搜索、导出、台账、汇编 |
| 批量合并 | `BatchPlan` | 文件汇编、会议相关文件 |
| 检索 | `SearchIndex` | 正文检索 |
| 统一提示 | `Check` | 审校、制度废止、排序、交付检查单 |

上面这些只写一次。以后新功能先看这张表：能用现成的就用，不能用的才加算法，不在页面里另写一份。

## 5. 怎么迁：绞杀者 + 金样

1. **新代码进 `src/platform/`，旧代码不动。** 新功能只依赖平台；旧模块先包适配器接进来。
2. **碰到哪块迁哪块。** 例如做正文检索时，把 `manuscript/search.rs` 和 `quick_find` 的取数改成 `DocumentMeta`；不做无关的重写。
3. **金样测试先立。** 迁移前把现有各文种的导出文件名、Typst `doc.json`、DOCX 文本、PDF 页数、份号清单抓成样例；迁移后逐项比对，除了有意改动，输出必须一致。
4. **一次一个内核，一次一个 PR。** 每个内核先有契约和单元测试，再接第一个使用者；不把五个内核塞进一个提交。
5. **旧入口保留兼容。** `export_all`、`ExportArtifacts`、`validator::validate` 这些老 API 先留着，内部逐步换成平台；外部调用不用同时改。

## 6. 分期

| 阶段 | 内容 | 产出 | 粗估 |
|---|---|---|---|
| F0 | 盘点、契约、金样 | 本文档 + `src/platform/` 类型骨架（含 `UnitCodes`/`PersonContact`/`DocNumber`/`NumberingScheme`）+ 金样清单 | 3–5 天 |
| F1 | Doc + Db | `DocumentMeta`、`DocNumber`/`NumberingScheme`、`Db`/`Migration`；列表/搜索/导出命名接上 | 1 周 |
| F2 | Search + 正文检索 | `SearchIndex`、`manuscript_fts`、片段/跳转/筛选 | 1.5–2 周 |
| F3 | 通讯录（两列电话）+ Render | 词库办公电话/手机（旧号码非手机一律进办公、承办联系人办公优先）、旧 Excel 兼容、通讯录 PDF/XLSX、`RenderPlan`/`Artifact`/`Protection` | 2 周 |
| F4 | 编号统一 + Ledger + 台账/制度 | 词库“纪要代字”（单位属性，有就带、无就不带）、纪要编号走 `NumberingScheme`、`LedgerEntry`、`issue_register` 三个池、手工补录、制度状态与引用提示 | 2–3 周 |
| F5 | Batch + 文件汇编 | `BatchPlan`、勾选/排序/合并/打印，目录与连续页码按需 | 2 周 |
| 后置 | 秩序册 | 从词库勾选名单的单独模板，另立方案 | 另估 |

F0–F1 只做地基，F2–F5 每期先抽内核、再让当期功能当第一个使用者。F3 与 F4 可以互换：先做通讯录是为了早一点暴露词库两列电话的迁移问题；如果更急着看台账，也可以先 F4。每期结束都能单独发版，不要求等全部完成。

## 7. 验收

- 迁移前后，各文种的导出文件名、Typst `doc.json`、DOCX 文本、PDF 页数、份号清单逐项一致；有意改动单独列出。
- 旧库（含 `user_version` 1–6、无密钥/有密钥）打开不报错，新表迁移幂等。
- 正文检索：中文短语、文号、英文、跨段命中；结果能跳转；1 万篇量级不卡界面。
- 通讯录：旧配置只有一个 `phone`、旧 Excel 只有“联系电话”一列时导入不丢号码；11 位手机号进“手机”，其余进“办公电话”；新导出同时有两列；承办联系人默认填办公、没有办公才用手机；表单与校验两个号码都认。
- 台账：公函、红头呈批件、会议纪要三个池互不影响；纪要按单位带字：单位有纪要代字就显示 `某纪〔2026〕3号`，没有就显示 `〔2026〕3号`；同一单位同一年度不会出现两种形态；重号/断号按单位 + 年度查；手工补录的纸质件不要求关联稿件。
- 文件汇编：勾选、排序、合并、打印顺序一致；目录/连续页码按选项生效。
- 非 AI 版 `cargo build --no-default-features` 能编译；平台内核不引用 `#[cfg(feature = "ai-workbench")]` 的模块。
- 每个内核至少有契约测试和一条真实数据回归；`fmt`、`clippy -D warnings`、全量测试通过。

## 8. 风险与开放问题

**风险：**

- **过度抽象**：契约没有两个以上使用者之前，不急着抽 trait；先写具体类型，出现第二处再收接口。
- **大爆炸**：任何阶段都不允许“把旧代码全搬完再发版”；金样和适配器就是防这个。
- **加密绕过**：平台所有产物必须走 `Protection`；打印、预览、另存、剪贴板都不例外。发现直写路径就当缺陷修。
- **线程与锁**：`rusqlite::Connection` 不跨线程；后台任务各开各的 `Db`，WAL + busy timeout，写事务短。
- **会议范围膨胀**：会议只复用现有模板 + 文件汇编，不建会议实体、不做签到/席卡/座次/审批流；秩序册单独后置。

**已定（2026-11-20）：**

1. 台账允许手工补录纸质件，`manuscript_id` 可空。
2. 公函、红头呈批件、会议纪要三个独立文号池；三者都按“单位 + 年度 + 序号”查重。纪要代字是单位属性：单位有就带、没有就不带，不能逐篇切换。
3. 通讯录加“办公电话 + 手机”两列；旧号码只有 11 位手机号进手机，其余（区号、短号、8 位固话）一律进办公；承办联系人办公优先、手机回落。
4. 不做批量套打；会议不建独立实体，通知/议程/主持词/须知复用现有文种，相关文件走文件汇编。
5. 秩序册后置，另立方案。

**还要拍板：**

1. 制度有效性的状态是否进离线同步 ZIP？登记簿现在不进；如果多机维护，需要单独决定。
2. 文件汇编默认要不要目录？建议默认不加，勾选“加目录”再生成，先把“勾选—排序—合并—打印”跑通。
3. 台账 v1 是否要“占号/废号”？建议先只做“已用 + 手工补录 + 重号/断号”，占号/废号等真实使用后再加。

## 9. 下一步

先做 F0：把这份文档的契约落成 `src/platform/` 的类型骨架和单元测试，抓各文种的金样，再把词库两列电话、三个文号池、文件汇编的表结构草案定下来。F0 不改任何现有行为，做完可以先合入，后面 F1–F5 都在它上面加。

秩序册不在地基里，等 F1–F5 做完再单独立项。

## 10. F0 落地记录（2026-11-20）

### 已落地

- `src/platform/doc.rs`：`UnitCodes`、`PersonContact`、`NumberingScheme`、`DocNumber`。
  - 手机只认 11 位 `1[3-9]`；其余一律办公；旧号码按此迁移。
  - 承办联系人取号 `preferred_office_first()`：办公优先、手机回落。
  - 公函/呈批件/纪要三种方案；纪要代字可为 `None`，两种纸面写法都支持。
- `src/platform/ledger.rs`：`Pool`、`LedgerEntry`；池键是“单位 + 方案 + 年度”，
  带字/不带字纪要共用同一个池（已用测试锁住）。
- `src/platform/search.rs`、`render.rs`、`check.rs`：检索、渲染、检查的公共类型。
- `src/main.rs`：`#[allow(dead_code)] mod platform;`，F1 接第一个使用者后删掉。
- 平台专项测试 10 项通过；`cargo clippy --locked --all-targets -- -D warnings` 通过。
- 一条兼容断言：纪要无代字时，`DocNumber::display()` 与现有
  `export::minutes::minutes_number_text()` 完全一致。

### 表结构草案（F4/F5 再落库）

```sql
-- 发文台账：三个池共用一张表，池键由 unit_key + scheme + year 决定。
CREATE TABLE issue_register (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    manuscript_id INTEGER REFERENCES manuscripts(id) ON DELETE SET NULL,
    unit_key      TEXT NOT NULL,              -- 单位 identity 或名称
    scheme        TEXT NOT NULL,              -- letter / approval / minutes
    code          TEXT NOT NULL DEFAULT '',   -- 显示用；纪要可为空
    year          TEXT NOT NULL,
    serial        TEXT NOT NULL,
    title         TEXT NOT NULL DEFAULT '',
    kind          TEXT NOT NULL DEFAULT '',
    doc_date      TEXT NOT NULL DEFAULT '',
    security      TEXT NOT NULL DEFAULT '',
    status        TEXT NOT NULL DEFAULT 'used', -- used / reserved / void
    note          TEXT NOT NULL DEFAULT '',
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    UNIQUE (unit_key, scheme, year, serial)
);

-- 制度有效性：在现有 document_registry 上补列，不重建表。
ALTER TABLE document_registry ADD COLUMN status TEXT NOT NULL DEFAULT 'active';
ALTER TABLE document_registry ADD COLUMN replaced_by INTEGER;
ALTER TABLE document_registry ADD COLUMN effective_date TEXT NOT NULL DEFAULT '';
ALTER TABLE document_registry ADD COLUMN abolished_date TEXT NOT NULL DEFAULT '';

-- 稿件正文检索：按知识库那套“tokens 列 + rowid = manuscripts.id”建。
CREATE VIRTUAL TABLE manuscript_fts USING fts5(tokens, tokenize='unicode61');
```

词库不是 SQL 表，改的是 `VocabularyEntry` 与 Excel 列：新增 `office_phone`、`mobile`、
`minutes_department_code`；旧 `phone` 只做反序列化兼容，载入时迁走并清空。

### 下一步（F1）

接 `DocumentMeta`（标题、文种、文号、日期、密级、份数、状态）到一个入口，
把列表/搜索/导出命名改成从它取；再落 `Db`/`Migration` 的统一开库与迁移表。
F1 先不碰台账、检索、通讯录的界面。
