# 第四阶段 C 实施报告（Pi 标准技能管理）

## 基线与范围

- 基线：`main` / `f0c00a5`，开工时工作区干净；本轮未提交、未推送、未打 tag、未改版本。
- 只改技能管理、Pi 技能加载与侧栏装配；未改交付、事实闸门、提案接受、正文写入路径。
- 未读取真实配置、密钥、稿件或用户 Pi 运行工作区；所有测试用临时配置目录与自造技能包。
- `pi-host/src/agent.mjs` 的清单外改动已按主代理补充批准：仅导入 `loadSkillIndex` 并在
  `systemPrompt` 装配处传入动态索引，没有改 agent 生命周期、恢复或预算。
- `docs/pi-implementation-plan.md` 与 `pi-direct-write-{discussion,task}.md` 的改动属于主代理，
  不是本轮 worker 修改。

## 实现内容

### C1 标准技能目录与存储

- 原 `src/pi_agent/skills.rs` 拆为 `src/pi_agent/skills/`：
  - `parser.rs`：用 `serde_yaml` 解析标准 frontmatter，支持多行 description、CRLF、BOM、
    `metadata.display_name`（也兼容 `display-name` / `title`）与未知 metadata 原样保留；
    name 只允许小写字母、数字和单个短横线，最长 64，拒绝路径与 Windows 设备名。
  - `catalog.rs`：内置五份 + `配置目录/pi/skills/<name>/` 用户覆盖 + 停用清单；
    坏副本保留为错误条目，不静默回退内置；公共语法包名 `gongwen-markdown` 被拒绝。
  - `store.rs`：新建、复制内置、删除、恢复内置、启停、SKILL.md/辅助文件保存、导入导出。
    新建/复制/导入先在同目录暂存再 rename；SKILL.md 保存走 `storage::atomic_write`。
    停用状态写在 `配置目录/pi/skills-disabled.json`，不复用旧 `skills/`。
  - `snapshot.rs`：按本轮已启用技能生成工作区快照与 `input/skills.json`。
- 旧 flow/step/variables/when/tools 等顶层字段被识别并拒绝，错误信息明确要求改写成自然语言
  标准技能；正文里出现 `flow:` 字样不会被误拒。旧流程不做自动转换。
- 包完整保留 references/assets/scripts；脚本只作为文件，不执行、不扩大 host 权限。
- 通用包安全读写提取到 `src/ai_services/skill_package.rs`，旧流程包与 Pi 包共用路径、链接、
  体积、ZIP 声明大小/实际大小/条目数原语；旧格式的入口校验与安装位置留在薄适配层。
  Pi 侧上限：单文件 8 MiB（不超过 host 读取上限）、整包 32 MiB、256 个文件、8 层目录、
  frontmatter 64 KiB、ZIP 条目 2048、声明解压总量 64 MiB。

### C2 管理界面

- 新增 `src/app/pi_skill_manager/`，AI 管理「技能」改为 Pi 标准技能入口：
  搜索/筛选、说明与启用状态、包文件树、原文编辑保存、新建（name + 可选显示名）、复制内置、
  启停、恢复内置、删除（二次确认）、导入标准技能包/文件夹、导出 zip。
- 旧 `src/app/skill_manager/` 不再注册，旧 UI 入口不可达；旧流程数据与旧引擎代码保留。
  页面提示“旧流程技能仍保留但不供 Pi 执行”，不自动转换、不覆盖。
- 内置技能只读，编辑前须先复制为我的技能；未保存缓冲按（技能, 文件）保存，切文件/切技能/
  保存失败都不丢文本。
- 文件对话框继续走 `request_file` / `FileDialogSpec`；导入同名冲突整体拒绝并提示先复制/删除，
  不静默覆盖。二进制辅助文件可预览大小但不能在文本编辑器里改。
- 页面说明与底部提示删除了旧 flow/算子/工具白名单/接口变量解释。

### C3 实际加载与快照一致性

- 侧栏下拉与 `/` 都从同一份缓存的 `Catalog` 取已启用技能；缓存按 `pi_skills::revision()` 失效，
  不每帧读盘。禁用/删除/损坏技能会明确报错，不偷偷切为自动；无启用技能时拒绝启动。
- 每次新任务在后台准备时重新读取有效目录，把本轮已启用技能包复制到
  `<工作区>/skills/pi-skills/<name>/`，并从同一份内存包生成 `input/skills.json`
  （name、displayName、description、文件路径/字节/sha256）和 `input/request.md` 的技能块。
- 手选技能在快照构建时再次验证；运行中修改全局技能不影响已开始任务。恢复继续读原工作区，
  `check_workspace_files` 还核对手选技能的 SKILL.md 仍在原快照里。
- host 自动模式不再硬编码五份技能：`prompt.mjs` 读取工作区 `input/skills.json` 生成索引；
  没有索引的旧工作区退回扫描自己的 `skills/pi-skills/`，不读取全局配置。协议版本未变。

## 实际改动文件

新增：

- `src/pi_agent/skills/{mod,parser,catalog,store,snapshot}.rs`
- `src/ai_services/skill_package.rs`
- `src/app/pi_skill_manager/{mod,tests}.rs`
- `docs/pi-implementation/phase-4c-report.md`

修改：

- `src/pi_agent/workspace.rs`：准备时写技能快照，`prepare` 增加目录/手选参数。
- `src/pi_agent/recovery.rs`：恢复时核对手选技能仍在原快照。
- `src/ai_panel.rs`、`src/ai_panel/composer_ui.rs`、`src/ai_panel/pi_job.rs`：
  技能目录缓存、下拉与 `/` 动态列表、发送前解析、标题显示。
- `src/app.rs`、`src/app/agent_settings.rs`、`src/app/ai_manage.rs`：装配新管理页与文案。
- `src/ai_services.rs`：注册共享技能包原语。
- `src/agent/skill_files.rs`、`src/agent/skill_files/package.rs`：改用共享包原语，
  保留旧格式入口校验/安装/分组语义。
- `pi-host/src/prompt.mjs`、`pi-host/src/agent.mjs`、`pi-host/test/bridge.test.mjs`：
  动态技能索引、旧工作区回退扫描与测试。

删除：

- `src/pi_agent/skills.rs`（内容拆到新模块目录）。

未注册但仍在磁盘上：`src/app/skill_manager/**`（旧 UI 代码，旧数据与旧引擎未删除）。

## 验证结果

已运行：

- `cargo fmt --all -- --check`：通过（无输出）。
- `cargo check --locked --all-targets`：编译通过，但**有 32 条 warning**，见下文失败项。
- `cargo test --locked pi_agent::`：76 passed、0 failed、1 ignored。
- `cargo test --locked pi_agent::skills`：20 passed、0 failed（并行连跑 3 次均通过）。
- `cargo test --locked pi_skill_manager`：5 passed、0 failed。
- `cargo test --locked skill_files`：7 passed、0 failed。
- `cargo test --locked a_new_task_freezes_the_enabled_skill_catalog_into_its_workspace`：
  1 passed。
- `cargo test --locked a_slash_prefix_only_offers_and_pins_pi_skills`：1 passed。
- `cargo test --locked pi_composer_has_one_execution_path`：1 passed。
- `cd pi-host && node --test test/bridge.test.mjs test/protocol.test.mjs test/paths.test.mjs`：
  28 passed、0 failed、1 skipped。

未通过：

- `cargo clippy --locked --all-targets -- -D warnings`：**失败**，32 条 error，全部是移除旧技能
  管理入口后失去生产调用者的代码：
  - `src/agent/skill_files.rs` 的旧管理 API（TEMPLATE、valid_id、check_id、user_file、
    set_enabled、source_text、has_user_file、check_text、problems_of、save、create）及其
    `export/import/remove` 重导出；
  - `src/agent/skill_files/package.rs` 的旧包读写函数；
  - `src/agent/skill.rs::builtin_text`；
  - `src/agent/tools/mod.rs::describe`（旧工具白名单说明）；
  - `src/mermaid.rs` 的 `render_interface` 重导出与 `src/mermaid/interface.rs::render_interface`
    （旧流程图）；
  - `src/ai_services/skill_package.rs` 的旧常量 `MAX_FILE/MAX_PACKAGE/MAX_FILES/
    MAX_ARCHIVE_TOTAL/MAX_ARCHIVE_ENTRIES` 与 `Package::{insert,read_dir,read_zip}`。
  按用户要求**没有**用 `-A dead_code`、`-A unused_imports` 或源码 `#[allow]` 掩盖这些警告。
  这些是旧 UI 支撑代码的去留问题，需要主代理决定（删除旧 UI 专用 helper、把旧管理 API
  收进 `cfg(test)`，或保留一个旧 UI 入口）；worker 没有扩大删除旧引擎。

其它尝试与限制：

- `cd pi-host && node --test` 全量运行：47 passed、1 failed、1 skipped；失败是
  `test/recovery.test.mjs` 在 Windows 上把绝对路径拼成
  `D:\gongwen\pi-host\C:\Users\...\late-error.mjs`，与本次技能改动无关；没有为它改测试。
- 未运行全量 `cargo test --locked --all-targets`（按 TASK 由主代理最终全量验收）。
- 未做 GUI 真机、真实业务模型、真实安装包/离线运行时验收。
- 旧工作区没有 `input/skills.json` 时，host 回退扫描该工作区自己的技能目录；这是兼容路径，
  不保证旧工作区也有文件哈希索引。

## 需要主代理决定 / 后续

1. 处理上述 clippy 失败：建议把旧流程管理 API 收进 `cfg(test)` 或删除仅服务于旧 UI 的
   `tools::describe` 与 `mermaid::render_interface`；`builtin_text` 同理。若保留旧 UI 入口，
   则需重新允许旧页面可达，这与 C2.2 冲突，worker 未擅自选择。
2. `docs/pi-implementation-plan.md` 已有主代理并行改动；本轮未改版本、未提交、未发布。
3. 下一步仍由主代理做真实 diff 审查、全量测试与最终验收。
