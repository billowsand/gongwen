# 随包 Pi 运行时（Node + host + 生产依赖）

本文说明智能写作（Pi）在目标机上的固定运行时：怎么组装、manifest 是什么、打包怎么接入、
怎么验收，以及恢复与所有权的边界。设计与协议见 `docs/pi-implementation/phase-3.md`、
`docs/pi-implementation/phase-3-report.md`。

## 一、目录与固定版本

运行时放在安装目录（或便携目录）的 `runtime/pi/`，**不写用户数据**：

```text
runtime/pi/
  manifest.json              平台、Node / Pi / host / 协议版本、lockfile 哈希、文件清单
  node.exe                   Windows（Linux 是 bin/node）
  licenses/LICENSE.node      Node 官方许可证
  licenses/DEPENDENCIES.json 完整生产依赖的 name / version / license
  licenses/deps/<name@ver>/  各依赖的 LICENSE / COPYING / NOTICE 原文
  host/
    main.mjs                 入口（其它 *.mjs 与它同目录）
    package.json             运行时的 type=module / main=main.mjs
    node_modules/            完整生产依赖（npm ci --omit=dev 产物）
```

固定版本写在受版本控制的 `packaging/pi-runtime.json`：

| 项 | 值 |
| --- | --- |
| Node | `22.22.3`（与本机已验版本一致，不跟随 latest） |
| `@earendil-works/chord` / `pi-ai` / `pi-durable` | `1.4.0` |
| host | `0.3.0`，协议 `3` |
| lockfile | `pi-host/package-lock.json` 的 SHA-256 写进 manifest |

官方压缩包与 SHA-256（来源 `https://nodejs.org/download/release/v22.22.3/SHASUMS256.txt`）：

- `node-v22.22.3-win-x64.zip`：`6c8d54f635feff4df76c2ca80f45332eb2ff57d25226edce36592e51a177ee33`
- `node-v22.22.3-linux-arm64.tar.xz`：`1c4a9933a5e45bc88f54f70b5f91232c127ec49f1a5989d23fb85824c7adf9b7`
- `node-v22.22.3-linux-x64.tar.xz`：`2e5d13569282d016861fae7c8f935e741693c269101a5bebcf761a5376d1f99f`

**升级步骤**：同时改 `packaging/pi-runtime.json`（版本 + 三个 SHA-256）、host 的
`PROTOCOL_VERSION` / `HOST_VERSION`（若协议或 host 变了，Rust 侧 `protocol.rs` 同步）、
重新组装并出 manifest，然后跑无公网验收。不允许「自动跟随 latest」。

## 二、组装

Windows（只能在本机真验 win-x64）：

```powershell
# 官方 zip 可放 tmp/pi-implementation/downloads/ 或传 -NodeArchive；先校验再解包
powershell -ExecutionPolicy Bypass -NoProfile -File scripts\stage-pi-runtime.ps1 -Force
```

Linux（ARM64 / x64，必须在目标架构上组装，不能拿 Windows 的 node_modules 顶替）：

```bash
bash scripts/stage-pi-runtime.sh --platform linux-arm64 --force
bash scripts/stage-pi-runtime.sh --platform linux-x64 --force
```

组装脚本只做这些事：

1. 找官方压缩包（缓存目录 / `-NodeArchive` / 构建期下载），**先校验 SHA-256 再解包**；
2. 只取 `node` / `node.exe` 与 `LICENSE`，不把 npm、头文件、文档塞进安装包；
3. 用**该 Node 自带的 npm** 在临时目录跑
   `npm ci --omit=dev --ignore-scripts --no-audit --no-fund`
   （`GONGWEN_PI_NPM_OFFLINE=1` 时加 `--offline`，需要完整 npm 缓存）；
4. 把 `pi-host/src/*.mjs` 平铺进 `host/`，生成运行时 `package.json`；
5. 收集依赖许可证，写 `manifest.json`，最后调用校验脚本。

`@google/genai`、`protobufjs` 等包带 `hasInstallScript`：这里**永远** `--ignore-scripts`，
不执行任何第三方安装脚本。运行资源（WASM / worker / JSON）是否齐全，用第三节的验收脚本
实际跑一轮来证明，不能因为脚本被跳过就默认完整。

安全边界：组装脚本只允许输出到 `runtime/` 或 `tmp/` 的**严格子目录**（不能等于根目录，
也不接受 `runtime-evil`、`tmp2` 这类前缀串），路径上任何已有 junction / 符号链接都拒绝；
替换目录前要求里面有 `manifest.json`（证明是本脚本产物）。正式打包只接受 `npm ci` 产物，
manifest 的 `source` 必须是 `npm-ci`，不接受把开发目录复制进来。

## 三、无公网验收

```powershell
runtime\pi\node.exe scripts\pi-offline-acceptance.mjs --runtime .\runtime\pi
# Linux
./runtime/pi/bin/node scripts/pi-offline-acceptance.mjs --runtime ./runtime/pi
```

脚本从**安装目录**启动 `host/main.mjs`，不 import 源码、不依赖开发 node_modules：

- 本地假 OpenAI 服务只监听 `127.0.0.1`；host 以 `node --import scripts/pi-offline-guard.mjs` 启动，
  守卫在导入 host **之前**接管 `net.connect` / `tls.connect` / `dns.lookup` / `dns.resolve*` /
  `fetch`：只放 `127.0.0.1` / `::1` / `localhost`，其它目标写进审计 JSONL 并直接报错；
  运行期间另用 `netstat -ano`（Windows）/ `ss -tnp`（Linux）采样 host 子进程的
  ESTABLISHED 连接作补充（采样只能观察，不能单独证明无公网）；守卫没生效或工具缺失
  都按失败处理；
- 子进程环境是最小白名单：`PATH` 只含随包 Node 与系统目录，`HOME` / `USERPROFILE` /
  `XDG_CONFIG_HOME` / `APPDATA` / `TEMP` 指向临时目录；不继承 `NODE_PATH`、
  `NODE_OPTIONS`、`LD_LIBRARY_PATH` 与任何模型 / 云凭据，不依赖用户缓存；
- 走完「读技能 → 读材料 → 写文件 → `submit_document`」，校验 `submitted` / `completed`、
  回执绑定与哈希、`final.md` 快照、Durable `session.sqlite` 已落盘；
- 再用 `recovery: { mode: "resume" }` 启动一次：必须交回同一份成品，且**不再调用模型**。

注意：验收脚本工作区里的技能是**简化夹具**（只验证文件工具与交付链），真实 8 份标准技能由
应用内嵌资源（`cargo test --locked skill_pack`）与界面集成验收覆盖，本脚本不冒充真实技能验收。

本机已对 Windows x64 组装运行时实测通过（4 次模型调用、回执重放 0 次新调用）。Linux ARM64 /
x64 的组装与验收只能在对应架构的 CI 上跑，本机结果不能代替。

## 四、打包接入

- `scripts/package-portable.ps1`（Windows 完整包 / 增量包、Linux ARM64 的 pwsh 路径）：
  出包前调用 `scripts/verify-pi-runtime.ps1` 校验 manifest、平台、Node / host / 协议版本
  与全部文件哈希，再按清单复制到 `runtime/pi/`；缺运行时、平台不符或哈希不符直接失败。
- `scripts/stage-linux-portable.sh`、`package-deb.sh`、`package-arch-pkg.sh`：同样先调用
  `scripts/pi-runtime-verify.mjs`，再复制 / 出包；`check-node-glibc.sh` 核查随包 Node 的
  GLIBC 符号版本不超过 2.28、并打印解释器与 GLIBCXX 需求。
- `release.yml` 三个平台作业都先组装运行时、跑无公网验收，再进入原有打包流程；
  `ci.yml` 在 Windows 与 Ubuntu 20.04 ARM64 上跑同样的组装 + 验收（Linux 真架构检查）。
- 最终 `SHA256SUMS.txt` 与增量升级差分自然包含 `runtime/pi/**`：Node 或依赖变了就进升级包，
  没变就不带。卸载 / 升级只按现有 `InstallDelete` 处理，不碰用户配置与稿件。

## 五、启动校验与开发模式

Rust 侧 `PiRuntime::locate()` 生产路径只认 exe 邻接 `runtime/pi`：

- 读 `manifest.json`，要求平台、Node `22.22.3`、host `0.3.0`、协议 `3` 与配置一致；
- 在执行 Node 前核对 Node 本身的 SHA-256，并检查依赖锁文件哈希；抽查关键入口
  （`host/main.mjs`、`host/package.json`、三个 Pi 包的 `package.json`）的
  SHA-256；不会每帧遍历 `node_modules`（manifest 只在每次开跑定位时读一次）；
- 缺文件 / 版本不符 / 哈希不符都给中文错误并指向「重装」。

开发时仍可用 `GONGWEN_PI_RUNTIME_DIR` / `GONGWEN_PI_NODE` 指向源码目录：这条路径会明确
标注「开发运行时（未经安装包验证）」，不读 manifest，**不冒充生产已验**。它不会被安装包
采用，也不参与发布验收。

## 六、所有权、恢复与工作区

### 统一策略、分阶段与阶段成果（协议 3，host 0.3.0）

新任务统一从「智能写作」入口发起，不再由用户选择小模型版 / 大模型版。init 增加可选
`strategy: { preference: "auto" | "phased", capability: { verifiedWorkBudget?, needsShortSteps? } }`：
用户只选「自动 / 倾向分阶段」，host 按**实际窗口**（`model.contextWindow`）与本轮任务规模
（要求、正文、材料字节、报告型任务）算出**工作预算**，两个字段分开上报在 `strategy` 事件里
（`window` / `workBudget` / `materialsBudget` / `reasons`）。默认工作预算未验证时按保守值，
不按模型名、参数量或 256K 阈值猜能力，也没有输出长度设置。

分阶段与常规共用同一个 Durable Harness 和同一套交付权限：

- `record_progress({ phase, unit, path, summary, sources?, uses?, sections? })` 登记中间成果。
  host 只认 `work/` 下真实存在、哈希对得上、引文能在 `input/` 原文里定位的成果；材料角色以
  本轮索引为准，样稿 / 范文不能登记成事实摘录。登记幂等，重复同一份成果不产生第二条记录。
- 登记写在运行目录（工作区之外）的 `progress.json`：记录输入身份哈希、单元、文件哈希、
  来源定位、依赖与阶段。文件被改、来源被换、依赖失效都会让成果失效；重新登记同一单元会让
  依赖它的章节与合稿失效重做。恢复前核对 run / request / 正文 / 要求 / 材料 / 选区身份，
  不一致就拒绝沿用旧成果。
- 阶段按任务裁剪：文档任务为材料提炼 → 提纲 → 逐节 → 合稿检查 → 交付；报告 / 提炼任务只做
  材料提炼 → 清单交付；局部修改只有交付。中间阶段禁止 `submit_document` / `finish_report`，
  宿主也会在最终交付前核对登记覆盖（提纲引用全部材料、合稿引用全部章节）；最终仍必须显式提交。
  每阶段最多纠正两次，无有效成果即暂停，不冒充完成。
- 常规一轮没交出成品且任务带材料时，有一次有界转换到分阶段，已登记成果为 0，不重复任何单元；
  没有材料的空转不转换，避免用阶段循环掩盖「无成果」。
- 工具调用额度按阶段有上限上调（常规 60，分阶段总计不超过 60 + 16）；`read` 改为有界分页，
  返回「已读第 x–y 行 / 未读第 y+1–N 行，继续用 offset」，单次结果受本次工作预算限制。
  知识库 / 稿件库的全文本工具同样支持 `offset`，返回 `total_chars` / `next_offset` /
  `unread_chars`，不再静默截断后当全文。最终交付仍读完整文件并独立校验。
- 事实与写入权限不变：中间成果不写正文，登记不能代替 `submit_document`；正文仍由界面线程
  在用户可见的 Pi 交付流程里写入，并保留整次撤销。

旧流程长期保留历史兼容：卡片可看 / 可导出，「复制为新 Pi 任务」只搬要求、材料、引用与已确认
信息到统一入口，旧选区与旧检查点不迁移、也不当作 Pi 检查点。

- 同一轮工作区同时只能有一个 host：host 启动时在运行目录的 `host-owner.sqlite` 上开
  `BEGIN EXCLUSIVE`（Node 内置 `node:sqlite`），由 SQLite / 操作系统持有文件锁；第二个
  host 拿到 `SQLITE_BUSY`，返回可读的「另一个窗口正在用」错误，不动数据库。进程死亡
  由 OS 释放锁，没有永不回收的 `exists` 标记。
- Rust 在准备运行、等待 host 及写最终状态的整个期间还持有 `rust-owner.lock` 文件锁，
  防止第二个窗口在 host 退出到状态写回的间隙抢同一轮任务。
- 「接着写」必须启动 host，由 host 在锁内重放交付回执；Rust 不绕过锁直读回执。
- 「从头写」不校验旧正文 / 旧模型 / 旧 Durable 库，只安全读取旧材料（路径 + 哈希 + 体积），
  以当前正文与当前 Pi 模型建立新 run；过期锁定选区明确丢弃并提示。
- 每次工具返回与用户回答都会在交给 host 之前把真实事实来源写入运行目录的
  `sources/<SHA256>.md`，再原子更新 `run.json` 中的引用；旧版本保留，中断不覆盖旧来源；
  恢复时重灌，避免崩溃后闸门把真实依据当成无来源。用户回答另存纯答案值（`answers`），
  「问题：回答」的显示字符串不当事实。- 工作区不自动清理：中断 / 停止 / 删除会话后，`配置目录/pi/workspaces/` 下的运行目录保留，
  便于人工查看与恢复。本期不做自动回收，也不把工作区放进 GWX 同步包；需要清理时由用户
  先停止所有窗口的 Pi 任务并退出应用，再手动清理 `pi/workspaces`。未保存到稿件库的新稿只在本次会话里保留运行引用，
  关掉应用后无法「接着写」——界面上有这句说明，不显示假入口。

### 成品写入正文与撤销（2026-11-02）

- 宿主交回的正文成品由界面线程**直接写进当前编辑正文**（普通流程不弹人工校订窗、
  不勾事实、不接受提案）；报告型产物只进侧栏。事实来源疑点只作提示。
- 每次写入保留精确的修改前后文本与运行标识（`ai_panel::pi_write::PiWrite`，随会话存盘）：
  卡片上有「撤销本次写入」，并经 `replace_with_undo` 记一个撤销点。编辑器状态按稿件
  隔离；后台标签写入后切回来，也能一次 Ctrl+Z 撤回该稿件的 Pi 写入。
- 写入前先确认修改前基线在盘上：工作区的 `input/current.md` 在交付时重新核对（
  `pi_agent::recovery::check_baseline_snapshot`），有库稿件还要先把撤销记录写进会话；
  任一失败就不换正文，成品留在轮次里可显式取回。
- 写入成功后运行记录标成 `Accepted`（旧提案接受用的也是它，语义都是「成品已进正文」）；
  这一轮不再可恢复，重启后重放回执也不会自动再覆盖一次。
- 运行期间用户改过正文时**不自动覆盖**：AI 成品留在这一轮（`TurnState::Conflict`），
  可显式取回（同样留撤销记录）或按当前正文重做。写入后用户又编辑过，卡片撤销禁用并
  说明可用编辑器 Ctrl+Z，不会整篇冲掉新编辑。
- 已入库稿件的前后文本随 AI 会话保存，重载会话仍可回看与卡片撤销。未入库稿件的基线和成品
  保存在任务工作区；编辑器 Ctrl+Z 栈是进程内状态，不承诺未保存稿件重启后还能恢复同一撤销栈。

## 七、干净目标机上的安装 / 升级

脚本模拟不等于干净机真验。实际发布前按下面步骤在一台没有开发环境的机器上做一次：

**Windows x64**

1. 只拷贝 `gongwen-assistant-<version>-win-x64-setup.exe`（及其 `.sha256`）到目标机，
   校验 SHA-256；机器上不装 Node / npm / Python / 全局 Pi。
2. 双击安装（默认装到 `%LOCALAPPDATA%\Programs\GongwenAssistant`），确认
   `runtime\pi\node.exe --version` 输出 `v22.22.3`。
3. 启动应用 →「AI 管理 → 模型服务」选一个内网 OpenAI 兼容模型 → 起草页侧栏写一句要求 →
   「智能写作（Pi）」跑一轮，确认成品直接写进正文、卡片显示「已写入正文」且能撤销；断网后重复一次，确认没有公网访问、错误可读。
4. 升级：用新的完整包或增量包覆盖安装；确认 `SHA256SUMS.txt` 里 `runtime/pi/**` 与安装目录
   一致，旧配置、稿件与输入法词表都还在。

**Linux ARM64（deb）/ Arch x64（pacman）**

1. 在干净机器上 `sudo dpkg -i gongwen-assistant_<version>_arm64.deb`（或
   `sudo pacman -U gongwen-assistant-<version>-1-x86_64.pkg.tar.zst`）。
2. deb 的安装根是 `/opt/gongwen-assistant`，Arch 的安装根是 `/usr/lib/gongwen-assistant`。
   对应目录下 `ldd runtime/pi/bin/node` 无 `not found`，`runtime/pi/bin/node --version`
   为 `v22.22.3`。GLIBC / GLIBCXX / 解释器审计在构建阶段执行，目标机不需要项目脚本。
3. 启动应用跑一轮 Pi（同上），确认没有 npm / 全局 Pi 依赖；Arch 上确认 portal 文件框与
   主线程不卡（Wayland）。
4. 升级：`dpkg -i` / `pacman -U` 新包；确认 `runtime/pi/manifest.json` 版本变化、
   用户数据未动。

## 八、Pi 标准技能管理（4C）

AI 管理 → 技能管理实际提供给 Pi 的标准 SKILL.md，不再编辑旧 flow 流程。
用户技能位于配置目录 `pi/skills/<name>/`，停用清单为 `pi/skills-disabled.json`。
内置技能是只读默认，复制为同名用户副本后可修改，也可恢复内置；旧 `skills/`
数据保留，不自动转换或执行。公共 `gongwen-markdown` 语法包由程序提供，不能被用户技能覆盖。

### 内置技能与参考文件（4D）

八份内置技能：起草 `gongwen-draft`、修改 `gongwen-revise`、审核 `gongwen-review`、
仿写 `gongwen-imitate`、研究报告 `gongwen-research`、复函 `gongwen-reply`、
要点提炼 `gongwen-extract`、政策依据核对 `gongwen-policy-check`。
润色 / 精简 / 语气 / 规范化是修改技能按任务选择的 `references/`，材料成文归起草，
事实核对归审核；旧 15 份技能的映射、取舍与未覆盖能力见 `docs/pi-skill-migration.md`。

`assets/pi-skills/` 下的每个文件都通过 `FILES` 表 `include_str!` 编进二进制：
新建工作区、复制为用户副本、导出技能包都带上完整 references，不依赖源码目录或
安装目录里另有一份。磁盘资源与注册表由测试核对；公共语法包仍只读且不可覆盖。

技能按需读 `skills/gongwen-markdown/` 的语法与文种规则、材料角色、选区与已填要素；
只有会让整篇方向写错的事才 `ask_user`，授权要素不建议、不出题，也没有「先确认清单
才能生成」或「事实勾选才能进正文」的闸门。交付分流：只审核 / 提炼 / 风格分析用
`finish_report`；生成或修改正文才用 `submit_document` 交完整 Markdown。

### 只读风格工具（4D）

Pi 增加两个只读领域工具：`style_list`（按当前文种列出启用且有适用范围限定的风格档案）
与 `style_read`（按 id 取回写法描述与范例）。它们直接复用
`ai_services::style::{StyleBook, render}`，不调第二个模型、不写风格库、不记使用次数、
不执行 shell；列表最多 20 份、返回文本有 token 与字符上限。未知 / 停用 / 不适用的
档案给结构化错误（`available:false` + `reason`），不悄悄换成另一份。风格内容与范例
按 `style_sample` 对待：只提供写法，不登记为事实来源，单位、人员、日期、数字不得
搬进正文。AI 自动学风格并保存进风格库仍未实现，模型不得声称已经保存。

导入/导出保留完整的 references、assets、scripts 等文件；脚本不会因此执行。
同名导入报冲突，不覆盖现有包。标准 frontmatter 用 YAML 解析，步骤写在 Markdown 正文中；
旧 flow 执行字段会被拒绝。编辑失败保留缓冲，切换文件/技能也不丢未保存文本。

侧栏下拉和 `/` 使用相同的启用目录。开跑时后台复制整包到任务工作区，并生成
`input/skills.json`（技能说明、路径、字节数与 SHA-256）；request.md 和 host 都读取本轮快照。
全局技能后续保存/停用不改变正在运行或恢复的任务，“从头重来”才取当前目录。
单包上限 32 MiB、256 文件，单文件 8 MiB、路径最多 8 段；每轮技能包合计最多 128 MiB，
索引最多 512 KiB。超限会明确失败，不静默截断技能列表。

## 九、已知限制

- **第五阶段策略恢复**：短步骤在同一个 Durable Harness 的空闲边界 reset。宿主登记保存 active cursor、输入/成果哈希、依赖、用户回答与实际 kb/ms 只读证据快照；恢复先核验并还原阶段，再恢复 Harness。长单行可用字符范围续读，知识/稿件工具的单次结果同时受工作预算限制。最终交付仍完整读取并检查文件，不用片段读取冒充全文。
- **技术反馈与事实提示**：显式提交时会反馈缺少题名、标题手写编号、研究报告脚注和附件标记等格式错误，模型可修改文件后重新提交；不自动清洗原稿。该检查不能证明模型完成了业务要求或查全依据，Qwen 的漏查与无据推断仍见第五阶段主审记录。

- **体积**：Windows 组装后 `runtime/pi` 约 162 MB（Node 87 MB + 生产依赖约 75 MB，
  1.3 万个文件）；完整安装包会明显变大，增量包只带变化的文件。
- **Linux 未在本机验证**：Node 的 GLIBC / GLIBCXX 与解释器要求由 CI 在目标架构检查；
  本机只真验 Windows x64。
- **manifest 较大**（约 2.6 MB）：启动时解析一次，不在每帧或每个工具调用重复读。
- **不自动清理孤立工作区**：见第六节。
