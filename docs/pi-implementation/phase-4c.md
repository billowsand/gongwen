# TASK：第四阶段 C——技能管理真正管理 Pi 标准技能

## 基线、分工与数据范围

- cwd `D:/gongwen`，main，HEAD `f0c00a5`。本轮起点干净；快照 `tmp/pi-implementation/phase-4c-baseline/`。
- 用户要求按原分工继续 4C：本机 Pi 实现，主代理写提示词、审查、修正和验收；固定 `opencode-go/deepseek-v4.1-flash`。此前用户已明确允许代码、任务和自造测试数据发至此目的地，本轮沿用同一分工和数据范围；不读取真实稿件、用户 Pi 运行工作区、配置/认证密钥、.env 或模型测试凭据。认证仅由 Pi 本身处理，不打印。
- 本 TASK、计划、主审记录属于主代理；worker 交付 `docs/pi-implementation/phase-4c-report.md` 后立即停止写入。不建子代理，不提交/推送/tag/发布/安装，不改版本。
- 必读 AGENTS.md、docs/security-hardening.md、docs/ai-architecture.md、docs/pi-writing-product-design.md、docs/pi-implementation/phase-4b-review.md。
- 用户正在另行讨论「Pi 完成后取消强制人工接受」，尚未合入本 TASK。本轮不修改交付/事实闸门/接受正文路径；不把讨论中的产品变化混入技能管理。

## 目标与现状

现在 AI 管理「技能」仍在编辑旧 flow DSL；Pi 只加载五个编译内置技能，侧栏 `/`、自动索引和 host 提示也写死五份。要让页面的编辑、启停、导入导出真正作用于后续 Pi 任务，并保持已开始任务可恢复、行为可复现。

### C1：标准技能目录与存储

1. 将 `src/pi_agent/skills.rs` 按职责拆成模块目录（如 catalog/parser/store/package/snapshot），仍由 pi_agent::pi_skills 对外提供窄接口。新技能目录采用配置目录 `pi/skills/<name>/`；停用信息单独存储，不复用/覆盖旧 `skills/`。
2. 内置五份作为只读默认；同名用户副本覆盖默认，并可「恢复内置」。支持新建、自有技能编辑/复制/删除、内置启停。禁用不是删除用户文件，所有改动原子保存。错误不能静默丢弃用户文本或清空目录。
3. 标准 SKILL.md：用现有 serde_yaml 正确解析 YAML frontmatter（支持多行 description、CRLF、普通标准 metadata 等），name/description/正文有效，name 与目录一致。name 采用小写字母数字与单短横线、最长64；不允许路径/设备名。五份内置中文显示名保持，用户技能可用 name 与 description 显示，或支持标准 metadata 中的可选显示名；不要发明执行 DSL。
4. 不解释 flow/step/variables/when/tools 白名单等旧执行字段。检测到旧 flow 文档时明确说明需要改写为自然语言标准技能，不能自动把旧流程当成 Pi 指令执行；不能因为自然语言正文出现「flow:」字样就误拒绝。未知标准 metadata 保留原文。
5. 包完整保留 references/assets/scripts 等相对辅助文件；脚本仅作为文件，不自动执行、不扩大 host 权限。不允许链接/junction/父目录/绝对路径/Windows 特殊路径绕出包或写到配置其它位置。
6. 体积、文件数、目录深度、frontmatter 大小有上限；ZIP 同时卡声明大小和实际解压大小，单文件上限不超过 host 的文件读取能力。复用现有 file_limits/atomic_write 和可抽取的安全包原语；不要另加依赖、复制整套旧技能解析器。若提取旧 package 的通用包读写，必须保持旧调用方验证与安全判据，形成共享原语加两种格式的薄适配。

### C2：管理界面

1. AI 管理「技能」变为 Pi 标准技能的管理入口：可筛选/选择、查看说明与 SKILL.md、包文件树、原文编辑保存、新建、复制、启停、恢复内置、导入文件夹/标准技能包与导出包。每个可见按钮实际调用新存储，不再把操作写到旧 DSL 目录。
2. 删除该页面旧的 flow/算子/工具白名单/接口变量解释与流程图。后台旧引擎、旧技能文件与会话兼容保留，不执行 4F，不删除旧用户配置。
3. 可明确提示旧流程技能仍保留但不供 Pi 执行；不要自动转换或覆盖。语法规范包 gongwen-markdown 仍是程序随包只读公共资源，不允许用户技能覆盖该公共目录。
4. 复用现有主题和工作区交互；长目录/长正文要有滚动，保存失败保留编辑缓冲，切技能不丢未保存文本。内置资源需要编辑时先创建用户副本。
5. 所有文件对话框继续走 request_file/FileDialogSpec。导入导出失败给可读反馈；导入同名冲突不能静默覆盖，提供明确选择或拒绝并提示先复制/删除。
6. 可直接改现有 skill_manager 模块，也可用新的 Pi 管理模块替代原 UI 装配；旧 UI 不再入口可达，但这不意味着删除旧 engine/skills 数据。复用无关页面与类型不受影响。

### C3：实际加载与快照一致性

1. 侧栏下拉与 `/` 从同一份有效 Pi 目录取已启用技能；不再只有五个静态名字。选择禁用/删除/损坏技能必须报清楚，不能偷偷切为自动。无启用技能时不启动任务。
2. UI 目录采用适当缓存/刷新时机：不能每帧读盘遍历整个技能包；管理页保存/启停后，后续任务与侧栏能获得新目录。
3. 每次新任务在后台准备时，将本轮已启用技能包（含辅助文件）复制成独立工作区快照，并用这同一份快照生成索引。不能先列一版、复制另一版，不能把全局可变目录 junction 到工作区。固定本轮 name、description、文件哈希/索引，便于排错。
4. 新任务的手选技能在实际快照中验证；不允许“列表里已禁用但上次选择仍执行”。运行中修改全局技能不改变已开始任务；恢复原任务继续读原快照，不用新版覆盖旧文件。从头新建任务再使用当前目录。
5. host 的自动技能提示也以本轮 input 索引为准，移除自动模式硬编码五份技能的逻辑，不能 UI 看见用户技能而 host 不知道。尽量沿用 input/request.md，避免不必要的协议/会话格式升级；若确需字段，新增向后兼容并双侧测试。
6. 内置五份、公共 gongwen-markdown 语法包、文件权限、领域工具白名单与 submit_document/finish_report 显式交付均保持，不新增 shell/联网/可执行扩展。

## 文件归属与边界

允许修改：src/pi_agent/{skills.rs,skills/**,workspace.rs}；必要的 src/pi_agent.rs、src/pi_agent/recovery.rs（仅快照索引兼容）与相关测试；src/ai_panel.rs、src/ai_panel/{composer_ui,pi_job,ui,ui_tests,session_ui}.rs 的技能选择/缓存/装配；src/app/skill_manager/**、src/app/agent_settings.rs、src/app/ai_manage.rs、src/app.rs；必要新增 src/app/pi_skill_manager/**。

允许提取通用技能包原语：src/agent/skill_files/{package.rs,package/**}、src/agent/skill_files.rs、src/ai_services.rs 及 ai_services 下相关新模块，只为安全包读写与旧兼容适配，不更改旧技能业务语义。

允许 host 改动：pi-host/src/{prompt,protocol}.mjs 及其相关测试，限动态目录/兼容验证；尽量不动 agent 生命周期/恢复/预算。允许现有 Rust 构造测试做新技能元数据适配，不改其原有安全和交付判据。

主代理范围补充：允许 pi-host/src/agent.mjs 仅在 systemPrompt 调用处读取并传入工作区动态技能索引，不改生命周期/恢复/预算。当前该装配改动必需；后续清单外文件仍需先报告。

主代理拥有本 TASK、docs/pi-implementation-plan.md 与最终 review。需要其它文件先报告，不先全局替换再补授权。

## 非目标

- 不改人工审阅/事实确认/自动写入正文，不改 app/pi_jobs.rs、accept_ai_proposal、字段采纳规则。
- 不执行 4D 业务技能扩充、4E API 新工具、4F 旧引擎删除，不做用户旧 DSL 无损迁移承诺。
- 不改 Cargo依赖/版本、打包脚本、pi-durable版本、Typst、IME、导出、GWX。
- 不读真实配置/稿件做测试，不运行外部业务模型；使用隔离配置目录与自造包。

## 验收与停止

- 新增有意义的包安全、标准 frontmatter、内置覆盖/恢复、禁用、复制/编辑持久化、冲突处理与快照隔离测试。
- 端到端验证：自定义技能在管理目录启用 → 侧栏可选 → 工作区索引与辅助文件一致；禁用后新任务不可用，旧工作区仍可恢复。不能只测文件保存而不测真正加载。
- 旧 flow 不能导入为可执行 Pi 技能；检查上限、ZIP 路径/链接越界、截断/YAML失败，错误不损坏已有包和编辑缓冲。
- cargo fmt --all -- --check、cargo check --locked --all-targets、cargo clippy --locked --all-targets -- -D warnings；相关 Rust 与 host 测试。
- 不跑 release/打包/真实外部业务测试；最终全量由主代理执行。完成 phase-4c-report.md 后立即停止修改，给出实际文件、验证结果、范围请求和剩余限制。不得把程序未跑的检查说成通过。
