# 随包 Pi 运行时（Node + host + 生产依赖）

当前进度、历史执行与退役门槛见 [Pi 现状索引](pi-current-status.md)。本文各阶段版本说明保留当时状态，实际发布版本以 `packaging/pi-runtime.json` 为准。

2026-11-06 安全存储收尾：host 升至 **0.4.0 / 协议 4**。加密工作区中，Rust 与 Node 的 Pi 材料、工作文件、来源、接口缓存、进度、交付和 Durable 会话统一使用 GWA2；Durable 生产状态由 `session.sqlite` 改为 `pi/session/` 下的加密 JSONL。DEK 只经 stdin 的 init 传递。旧协议/旧 host 的中间任务不能续跑，已有交付回执仍可重放。详细边界见 [加密资源全生命周期](encrypted-resource-lifecycle.md)。

2026-11-10 上游升级：`@earendil-works/chord` / `pi-ai` / `pi-durable` 由 1.4.0 升至 **1.5.0**（三包锁定同升），host 升至 **0.4.2 / 协议 4**（协议不变）。`watchEvents()` 上游移除，host 改挂 `Conversation.watch()` 会话视图；`onReport` 改单 report 参数，`task_parked` 报告被移除（restart-limited 任务改为明确终结，映射见下文升级节）。TypeBox 不再由 pi-ai 导出，host 直依赖 `typebox@1.3.27`。升级记录与破坏性变更清单见下文「Pi 1.5 升级」一节。

2026-11-05 退役审查收尾：host 补丁升至 0.3.6（协议仍为 3），常规领域工具额度 60，分阶段按阶段追加到绝对上限 84；常规任务转分阶段后也可追加，拒绝的调用不占用未发出的额度。Rust/host 与实际握手版本由回归对账。旧 0.3.5 中间任务按既有版本保护不能在原工作区接着写，已有成品仍可查看/取回或从头写。验证与本机运行时状态见 [收尾记录](pi-implementation/retirement-fixes-2026-11-05.md)。

> **2026-11-04 用户确认的 Pi 写作取舍（优先于下文历史方案）。** 新任务按 256K 及以上窗口作为主要使用场景，预算采用实际部署容量，取消未知模型 96K 封顶、材料份数与旧短步骤偏好触发的自动细分；材料优先整篇阅读，超限按大章节/大段切分。每次运行提供当前日期、星期、时区及相关组织背景，`current_date` 可重读实时日期，`vocab_search` 支持职能/简介与所属单位。Pi 不执行事实核查、不生成事实疑点或额外 AI 审校清单；全面审校和政策核查技能退出新任务目录。现有文字复核、格式检查、引用保护、授权表单权限、正文基线及精确撤销保留。旧提案和历史记录权限不变。实现与验收见 [写作上下文首版](pi-implementation/writing-context-v1.md)。

> **2026-11-04 审查后修复（本轮已改，待真机验收）。** 单轮执行上限改为 **30 分钟**（等待用户
> 回答的时间不计），卡片显示「执行上限 30 分钟」；工具调用常规 60 次，分阶段由宿主按阶段
> 追加额度、硬上限 **84 次**；事实来源改为「单项 + 索引」落盘（旧格式仍可读），落盘失败不再
> 中止本轮；运行目录新增自动清理，AI 管理页显示占用并可「清理旧工作区」；生产 host 改为
> `env_clear` 后最小环境白名单并透传代理设置；macOS 的 `darwin-arm64` / `darwin-x64` 运行时、
> 打包、签名公证（可选）与 CI 骨架已补齐，macOS 真机与发布验证仍未做。各条详见下文对应节。

本文说明智能写作（Pi）在目标机上的固定运行时：怎么组装、manifest 是什么、打包怎么接入、
怎么验收，以及恢复与所有权的边界。设计与协议见 `docs/pi-implementation/phase-3.md`、
`docs/pi-implementation/phase-3-report.md`。

## 一、目录与固定版本

运行时放在安装目录（或便携目录）的 `runtime/pi/`，**不写用户数据**：

```text
runtime/pi/
  manifest.json              平台、Node / Pi / host / 协议版本、lockfile 哈希、文件清单
  node.exe                   Windows（Linux / macOS 是 bin/node）
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
| 平台 | `win-x64`、`linux-arm64`、`linux-x64`、`darwin-arm64`、`darwin-x64`（macOS 用官方 `.tar.gz`） |
| Node | `22.22.3`（与本机已验版本一致，不跟随 latest） |
| `@earendil-works/chord` / `pi-ai` / `pi-durable` | `1.5.1` |
| host | `0.5.2`，协议 `4` |
| lockfile | `pi-host/package-lock.json` 的 SHA-256 写进 manifest |

官方压缩包与 SHA-256（来源 `https://nodejs.org/download/release/v22.22.3/SHASUMS256.txt`）：

- `node-v22.22.3-win-x64.zip`：`6c8d54f635feff4df76c2ca80f45332eb2ff57d25226edce36592e51a177ee33`
- `node-v22.22.3-linux-arm64.tar.xz`：`1c4a9933a5e45bc88f54f70b5f91232c127ec49f1a5989d23fb85824c7adf9b7`
- `node-v22.22.3-linux-x64.tar.xz`：`2e5d13569282d016861fae7c8f935e741693c269101a5bebcf761a5376d1f99f`
- `node-v22.22.3-darwin-arm64.tar.gz` / `node-v22.22.3-darwin-x64.tar.gz`：官方归档是 **gzip**
  （`.tar.gz`，不是 Linux 的 `.tar.xz`）；SHA-256 记在 `packaging/pi-runtime.json`，组装时按清单
  校验，并与官方 `SHASUMS256.txt` 核对。

**升级步骤**：同时改 `packaging/pi-runtime.json`（版本 + 五个平台归档的 SHA-256）、host 的
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

macOS（arm64 / x64，同样必须在目标架构上组装）：

```bash
bash scripts/stage-pi-runtime.sh --platform darwin-arm64 --force
bash scripts/stage-pi-runtime.sh --platform darwin-x64 --force
```

macOS 走 BSD 工具：校验和用 `shasum -a 256`，解包用 `tar -xzf`（官方 darwin 归档是 gzip），
不要求 GNU coreutils；脚本已按平台分支，不要拿 Linux 归档或 `sha256sum` 硬套。

组装脚本只做这些事：

1. 找官方压缩包（缓存目录 / `-NodeArchive` / 构建期下载），**先校验 SHA-256 再解包**；
2. 只取 `node` / `node.exe` 与 `LICENSE`，不把 npm、头文件、文档塞进安装包；Unix（Linux /
   macOS）上放 `bin/node`，打包脚本负责补可执行位；
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
# macOS
./runtime/pi/bin/node scripts/pi-offline-acceptance.mjs --runtime ./runtime/pi
```

脚本从**安装目录**启动 `host/main.mjs`，不 import 源码、不依赖开发 node_modules：

- 本地假 OpenAI 服务只监听 `127.0.0.1`；host 以 `node --import scripts/pi-offline-guard.mjs` 启动，
  守卫在导入 host **之前**接管 `net.connect` / `tls.connect` / `dns.lookup` / `dns.resolve*` /
  `fetch`：只放 `127.0.0.1` / `::1` / `localhost`，其它目标写进审计 JSONL 并直接报错；
  运行期间另用 `netstat -ano`（Windows）/ `ss -tnp`（Linux）/ `lsof -nP -iTCP -sTCP:ESTABLISHED -a -p <pid>`
  （macOS，系统没有 `ss`）采样 host 子进程的 ESTABLISHED 连接作补充（采样只能观察，不能单独
  证明无公网）；守卫没生效或工具缺失都按失败处理；
- 子进程环境是最小白名单：`PATH` 只含随包 Node 与系统目录，`HOME` / `USERPROFILE` /
  `XDG_CONFIG_HOME` / `APPDATA` / `TEMP` 指向临时目录；不继承 `NODE_PATH`、
  `NODE_OPTIONS`、`LD_LIBRARY_PATH` 与任何模型 / 云凭据，不依赖用户缓存；
- 走完「读技能 → 读材料 → 写文件 → `submit_document`」，校验 `submitted` / `completed`、
  回执绑定与哈希、`final.md` 快照、Durable `session/` JSONL 目录已落盘且非空；
- 再用 `recovery: { mode: "resume" }` 启动一次：必须交回同一份成品，且**不再调用模型**。

注意：验收脚本工作区里的技能是**简化夹具**（只验证文件工具与交付链），7 份内置技能
（6 份写作 + 1 份风格学习 `gongwen-style-learn`）由应用内嵌资源（`cargo test --locked skill_pack`）
与界面集成验收覆盖，本脚本不冒充真实技能验收。

本机已对 Windows x64 组装运行时实测通过（4 次模型调用、回执重放 0 次新调用）。Linux 骨架已进
CI（buster 作业组装 `linux-arm64` 运行时 + 离线验收 + Pi 的 Rust 进程 / 锁 / 交付测试），
**Linux 真机整链仍待验**；macOS 的组装与离线验收已在本机 darwin-arm64 实际跑通（数字见
「Pi 1.5 升级」节），但装到干净 Mac 上的整链与发布环境仍未验证。本机结果不能代替真机验收。

## 四、打包接入

- `scripts/package-portable.ps1`（Windows 完整包 / 增量包、Linux ARM64 的 pwsh 路径）：
  出包前调用 `scripts/verify-pi-runtime.ps1` 校验 manifest、平台、Node / host / 协议版本
  与全部文件哈希，再按清单复制到 `runtime/pi/`；缺运行时、平台不符或哈希不符直接失败。
- `scripts/stage-linux-portable.sh`、`package-deb.sh`、`package-arch-pkg.sh`：同样先调用
  `scripts/pi-runtime-verify.mjs`，再复制 / 出包；`check-node-glibc.sh` 核查随包 Node 的
  GLIBC 符号版本不超过 2.28、并打印解释器与 GLIBCXX 需求。
- macOS：`package-dmg.sh` / `package-macos.sh` 出包前同样校验 manifest 与全部文件哈希，并把
  `runtime/pi` 复制进 `Contents/MacOS/runtime/pi`（不再只搬字体）；`packaging/macos/entitlements.plist`
  给随包 Node 授予 JIT 所需权限。签名与公证是**可选**步骤：配置了
  `GONGWEN_MACOS_SIGN_IDENTITY` / `GONGWEN_MACOS_NOTARY_PROFILE` 才执行 hardened runtime
  签名、`notarytool` 与 `staple`，未配置时出未签名包（本机可用，别人机器上 Gatekeeper 会拦）。
  证书与 Apple 账号的准备见 `packaging/macos/README.md`。
- `release.yml` 各平台作业都先组装运行时、跑无公网验收，再进入原有打包流程；新增 macOS
  arm64 的 `.dmg`（先只出 arm64，x64 可按同一办法出包）。`ci.yml` 在 Windows、Ubuntu 20.04
  ARM64 与 `macos-15` 上跑同样的组装 + 验收；Debian buster 作业会组装 `linux-arm64` 运行时
  并把 `runtime/pi/bin` 加进 PATH，Pi 的进程 / 锁 / 交付 Rust 测试因此真正执行；Linux 与
  Windows 作业另跑 `cargo test --locked --release shipped_pi_runtime_manifest -- --ignored`
  做随包运行时 manifest 的端到端校验。
- 最终 `SHA256SUMS.txt` 与增量升级差分自然包含 `runtime/pi/**`：Node 或依赖变了就进升级包，
  没变就不带。卸载 / 升级只按现有 `InstallDelete` 处理，不碰用户配置与稿件。

## 五、启动校验与开发模式

Rust 侧 `PiRuntime::locate()` 生产路径只认 exe 邻接 `runtime/pi`：

- 平台在**运行期**判定（`std::env::consts::OS` / `ARCH`），不再用编译期 `cfg!` 把「非 Windows」
  一律当 `linux-*`：Windows x64 → `win-x64`，Linux arm64 / x64 → `linux-arm64` / `linux-x64`，
  macOS arm64 / x64 → `darwin-arm64` / `darwin-x64`（manifest 的平台名也用 `darwin-*`）；
  未知平台明确报「不支持的系统」，不再误接受一份别的平台的运行时；
- 读 `manifest.json`，要求平台、Node `22.22.3`、host `0.5.2`、协议 `4` 与配置一致；
- 在执行 Node 前核对 Node 本身的 SHA-256，并检查依赖锁文件哈希；抽查关键入口
  （`host/main.mjs`、`host/package.json`、三个 Pi 包的 `package.json`）的
  SHA-256；不会每帧遍历 `node_modules`（manifest 只在每次开跑定位时读一次）；
  定位与哈希校验的结果按运行时目录在进程内缓存，同一进程后续开跑复用，不重复整读
  80 MB 级的 Node；开发运行时（环境变量模式）不读 manifest，也不走这份缓存；
- 缺文件 / 版本不符 / 哈希不符都给中文错误并指向「重装」。

开发时仍可用 `GONGWEN_PI_RUNTIME_DIR` / `GONGWEN_PI_NODE` 指向源码目录：这条路径会明确
标注「开发运行时（未经安装包验证）」，不读 manifest，**不冒充生产已验**。它不会被安装包
采用，也不参与发布验收。

## 六、所有权、恢复与工作区

### 统一策略、分阶段与阶段成果（当前协议 4 / host 0.5.2；策略语义沿用协议 3）

模型能力档案由「测试写作能力」后台动作生成：四次同题自动 / 分阶段合成写作都经生产 Rust Pi 运行链，
工具源显式绑定到临时 SQLite 知识库 / 稿件库与空风格源。单轮限时 **300 秒**、四轮总计 **15 分钟**，
并分别记录「超时」与「能力不足」——工具链超时不算模型能力不足；校准仍**不产生**
`verifiedWorkBudget`。结果与独立原始证据按 SHA-256 关联；资料、模型成品、
工具覆盖和机器判据可回看。当前测试只验证 FTS/关键词与工具闭环，模拟 embedding 服务不可达，因此不代表真实向量
或 rerank 服务验收，也不验证长上下文工作预算。

匹配时检查规范化端点（去除 userinfo / query / fragment）、模型 ID、窗口数值及来源、思考设置、host/判据版本与技能快照摘要。
校准档案在 `配置目录/pi/capability.json`，不保存密钥或稿件；原始合成证据在 `pi/calibration-evidence/<sha256>.json`，
按单文件 256 KiB、单次四轮和单篇成品 16 KiB 限额约束。超限明确报错，不静默截断成品。新运行在后台匹配档案后把能力摘要写进 Init；恢复沿用运行开始时
保存的能力快照，不受以后校准影响。旧证据不匹配、缺失或损坏时按实际窗口预算继续，不阻断写作。

新任务统一从「智能写作」入口发起，不再由用户选择小模型版 / 大模型版。init 增加可选
`strategy: { preference: "auto" | "phased", capability: { verifiedWorkBudget?, needsShortSteps? } }`：
用户只选「自动 / 倾向分阶段」，host 按**实际窗口**（`model.contextWindow`）与本轮任务规模
（要求、正文、材料字节、报告型任务）算出**工作预算**，两个字段分开上报在 `strategy` 事件里
（`window` / `workBudget` / `materialsBudget` / `reasons`）。未验证模型 / 未验证工作预算时，
`workBudget` = **实际服务窗口**（不会伪造一个更小的「保守值」），并在策略事件里标注「未验证」；
结构上不产生 `verifiedWorkBudget`（校准只做短任务观察，不验证长上下文能力），
不按模型名、参数量或 256K 阈值猜能力，也没有输出长度设置。

分阶段与常规共用同一个 Durable Harness 和同一套交付权限：

- `record_progress({ phase, unit, path, summary, sources?, uses?, sections? })` 登记中间成果。
  host 只认 `work/` 下真实存在、哈希对得上、引文能在 `input/` 原文里定位的成果；材料角色以
  本轮索引为准，样稿 / 范文不能登记成事实摘录。登记幂等，重复同一份成果不产生第二条记录。
- 阶段依赖由**宿主补齐**，不再要求模型手填 `uses`：提纲继承全部材料单元，章节继承提纲与
  全部材料，合稿继承提纲与全部章节。非提炼 / 样稿研究阶段再填 `sources` 会被直接拒绝，
  并回一条带可用单元编号与建议路径的提示；依赖编号写错时同样提示当前已登记编号。
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
- 工具调用额度：常规 **60 次**；分阶段由宿主按阶段追加额度，硬上限 **84 次**（Rust 侧也按
  84 判定，超过即终止并说明原因，旧文档的「60 + 16」说法作废）。`read` 改为有界分页，
  返回「已读第 x–y 行 / 未读第 y+1–N 行，继续用 offset」，单次结果受本次工作预算限制。
  知识库 / 稿件库的全文本工具同样支持 `offset`，返回 `total_chars` / `next_offset` /
  `unread_chars`，不再静默截断后当全文。最终交付仍读完整文件并独立校验。
- 读工具按本次预算声明 `outputLimits`，避免 Durable Harness 再按默认上限二次截断
  （长材料曾被截成约 50 KiB / 2000 行）；知识库与稿件库全文工具同样按字符预算声明上限。
- 单轮执行上限 **30 分钟（1800 秒）**，等待用户回答的时间不计入；卡片显示「执行上限 30 分钟」。
  超时按可续写失败收尾并保留工作区，已登记的阶段成果可「接着写」，不重付整轮成本。
- 事实与写入权限不变：中间成果不写正文，登记不能代替 `submit_document`；正文仍由界面线程
  在用户可见的 Pi 交付流程里写入，并保留整次撤销。

旧流程长期保留历史兼容：卡片可看 / 可导出，「复制为新 Pi 任务」只搬要求、材料、引用与已确认
信息到统一入口，旧选区与旧检查点不迁移、也不当作 Pi 检查点。

- 加密工作区使用 GWA2（AES-256-GCM）保护 Pi 持久文件，AAD 逐字节为
  `gongwen-resource-v2\0<slot>\0pi\0<逻辑路径>`；真实/诱饵槽分别绑定 `0`/`1`，逻辑路径
  相对配置目录并统一用 `/`。Rust 与 Node 共用固定向量回归，跨槽、跨用途或跨路径替换密文会
  认证失败。DEK 只在 host 启动时随 stdin 的 init 传递，不进 argv、环境变量或日志。
- Durable 状态是运行目录 `pi/session/` 下的 JSONL；加密追加必须先解密旧内容、在内存拼接后
  整份原子重写，不能向密文末尾追加明文。旧 `session.sqlite` 只作密封归档，不自动迁移或续跑；
  旧协议/旧 host 记录仅在有既有交付回执时允许重放，没有回执的中间任务须「从头写」。
- 同一轮工作区同时只能有一个 host：host 启动时在运行目录的 `host-owner.lock` 上开
  `BEGIN EXCLUSIVE`（Node 内置 `node:sqlite`），由 SQLite / 操作系统持有文件锁；锁文件只有
  无行业务无关锁表，不写 runId、pid 或正文。第二个 host 拿到 `SQLITE_BUSY`，返回可读的
  「另一个窗口正在用」错误，不动数据库。进程死亡由 OS 释放锁，没有永不回收的 `exists` 标记。
- Rust 在准备运行、等待 host 及写最终状态的整个期间还持有 `rust-owner.lock` 文件锁，
  防止第二个窗口在 host 退出到状态写回的间隙抢同一轮任务。
- 「接着写」必须启动 host，由 host 在锁内重放交付回执；Rust 不绕过锁直读回执。
- 「从头写」不校验旧正文 / 旧模型 / 旧 Durable 库，只安全读取旧材料（路径 + 哈希 + 体积），
  以当前正文与当前 Pi 模型建立新 run；过期锁定选区明确丢弃并提示。
- 事实来源存储（本轮已改）：每次工具返回与用户回答后，把**单项**写入运行目录的
  `sources/items/<SHA256>.md`，再写 `sources/index-<hash>.json` 索引并原子更新 `run.json`
  的引用。单项上限 2 MiB、索引上限 512 KiB、恢复读取总量上限 8 MiB；旧格式
  `sources/<hash>.md` 仍可读，兼容历史运行。索引清理在运行记录指针切换成功之后做：
  保留最近 2 个版本以及当前指针指向的那一份（最多 3 个），未被任何现存索引引用的项文件
  超过 24 小时后清理。**来源落盘失败不再中止本轮**，只在过程记录里留一行
  提示（Pi 交付本身不做事实核查，这层数据只服务历史记录与恢复），恢复时重灌。用户回答另存
  纯答案值（`answers`），「问题：回答」的显示字符串不当事实。
- 工作区自动清理（本轮已改）：每个稿件保留最近 3 轮；只自动删除**已写入正文（Accepted）**
  或**没有 `run.json` 的孤儿目录（超过 1 小时）**、且最后修改超过 7 天的运行目录；
  Running / Interrupted / Failed / Parked / Delivered 的目录不自动删（它们支撑「接着写」）。
  AI 管理页显示 `Pi 工作区占用 X MB（N 轮）` 并提供「清理旧工作区」按钮，按同一规则清理。
  运行目录仍不放进 GWX 同步包；需要手动清理时，先停止所有窗口的 Pi 任务并退出应用，再删
  `pi/workspaces`。未保存到稿件库的新稿只在本次会话里保留运行引用，关掉应用后无法
  「接着写」——界面上有这句说明，不显示假入口。
- 生产 host 启动环境（本轮已改）：不再继承完整环境，而是 `env_clear` 后只注入最小白名单
  （`PATH`、`SystemRoot`、`TEMP` / `TMP`、`HOME` / `USERPROFILE` / `APPDATA` / `LOCALAPPDATA`、
  `XDG_*`、`LANG` / `LC_ALL`、`TZ` 等），**不继承** `NODE_OPTIONS` / `NODE_PATH` /
  `LD_LIBRARY_PATH`。应用内代理设置会以 `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` /
  `NO_PROXY`（含小写形式与 `NODE_USE_ENV_PROXY=1`）传给 host；直连模式设 `NO_PROXY=*`，
  跟随系统模式原样传递系统代理变量。

### 成品写入正文与撤销（2026-11-02）

- 宿主交回的正文成品由界面线程**直接写进当前编辑正文**（普通流程不弹人工校订窗、
  不勾事实、不接受提案）；报告型产物只进侧栏。Pi 交付检查**不做事实核查**，不产生事实疑点
  清单（`DeliveryReport.advisories` 仅为兼容历史记录保留、新交付恒空）。
- 交付通道按任务类型分三类：正文 `document`（`submit_document`）、清单 `report`
  （`finish_report`）、独立风格文件 `style`（`submit_style`，只进设置页编辑器，不能写正文）；
  技能目录按任务类型隔离，互不串用。
- 通道与内置技能**双向对齐**：五个正文技能（起草 / 修改 / 仿写 / 研究报告 / 复函）只能
  `submit_document`，提炼技能只能 `finish_report`，风格学习只能 `submit_style`。Rust 侧按
  技能校验回执类型，正文技能收到清单回执（或反之）一律拒绝；host 侧同样拒绝用错通道。
  恢复回放只认启动前已存在、类型与摘要一致的历史回执，不能凭伪造的 `recovered` 标记绕过。
- 每次写入保留精确的修改前后文本与运行标识（`ai_panel::pi_write::PiWrite`，随会话存盘）：
  卡片上有「撤销本次写入」，并经 `replace_with_undo` 记一个撤销点。编辑器状态按稿件
  隔离；后台标签写入后切回来，也能一次 Ctrl+Z 撤回该稿件的 Pi 写入。
- **写入前自动版本（2026-11-05）**：正文非空且稿件已入库时，写正文前先自动提交一个
  「AI 写入前 MM-DD HH:MM」版本（`commit_manuscript_version`），AI 成品再直接覆盖正文；
  版本历史 / 版本对照负责找回与看差异。正文为空、未入库或与最新提交版相同则不提交；
  自动版本失败就不写正文，成品留在轮次里。Pi 不再生成「AI 工作稿」预览：候选只报进度，
  写完后也不保留工作稿副本，旧会话里 Pi 轮次的工作稿状态在载入时收敛。
- 写入前先确认修改前基线在盘上：工作区的 `input/current.md` 在交付时重新核对（
  `pi_agent::recovery::check_baseline_snapshot`），有库稿件还要先把撤销记录写进会话；
  任一失败就不换正文，成品留在轮次里可显式取回。
- 被硬闸门拦下（空稿、路径 / 体积 / 哈希、基线与选区、公文引用、只读）的成品不丢弃，
  保留在这一轮里；只能由用户显式点「**取回 AI 成品**」写入正文——取回路径不重跑闸门，
  但保留精确撤销记录（2026-11-02 / 11-04 确认的产品口径）。红线 3 说的「照旧拦下」指
  自动写入路径。
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

**macOS arm64（dmg，待真机验收）**

1. 打开 dmg，把应用拖进「应用程序」，确认 `Contents/MacOS/runtime/pi/bin/node --version`
   为 `v22.22.3`；未签名的开发包会被 Gatekeeper 拦，需要右键「打开」或先移除隔离属性。
2. 首次启动后跑一轮 Pi（同上），确认随包 Node 能 exec、卡片能写入/撤销；`.gwx` 的**双击
   打开在 macOS 不支持**（未接 Apple Event open-file），用拖放或应用内「导入」。
3. 签名、公证与证书/Apple 账号的准备见 `packaging/macos/README.md`；CI 未配 secrets 时
   发布的是未签名包。卸载时注意 `~/Library/Application Support/` 下配置目录（含
   `pi/workspaces`）需要手动清理。

## 八、Pi 标准技能管理（4C）

AI 管理 → 技能管理实际提供给 Pi 的标准 SKILL.md，不再编辑旧 flow 流程。
用户技能位于配置目录 `pi/skills/<name>/`，停用清单为 `pi/skills-disabled.json`。
内置技能是只读默认，复制为同名用户副本后可修改，也可恢复内置；旧 `skills/`
数据保留，不自动转换或执行。公共 `gongwen-markdown` 语法包由程序提供，不能被用户技能覆盖。

### 领域工具（2026-11-05 更新）

常规写作任务可用的域工具：`kb_search` / `kb_read`（知识库）、`ms_search` / `ms_read`（稿件库）、
`vocab_search`（标准词库）、`current_date`、`check_document`、`doc_elements`、
`style_list` / `style_read`，以及 **`api_search` / `api_call` / `api_read`**（内网只读数据接口：
先搜接口、再按参数签名调用，返回条目进证据；长结果用 `api_read` 按 `result_id` 续读，
缓存会落盘到运行目录，恢复后仍可读）。`api_search` 只列只读且开放给 AI 的接口，
`api_call` 调用前按 JSON Schema 校验；会改数据的接口一律拒绝。风格任务不注册这些域工具。

### 内置技能与参考文件（4D）

内置 **7 个**技能：6 个写作技能——起草 `gongwen-draft`、修改 `gongwen-revise`、仿写
`gongwen-imitate`、研究报告 `gongwen-research`、复函 `gongwen-reply`、要点提炼
`gongwen-extract`——加 1 个风格学习 `gongwen-style-learn`（设置页入口，不写正文，见文末）。
全面审校 `gongwen-review` 与政策核查 `gongwen-policy-check` 已退役：只保留历史文件，
不再进新任务技能目录（旧 15 份技能的映射见 `docs/pi-skill-migration.md`）。
润色 / 精简 / 语气 / 规范化是修改技能按任务选择的 `references/`，材料成文归起草，
旧 15 份技能的历史映射、取舍与未覆盖能力见 `docs/pi-skill-migration.md`。

`assets/pi-skills/` 下的每个文件都通过 `FILES` 表 `include_str!` 编进二进制：
新建工作区、复制为用户副本、导出技能包都带上完整 references，不依赖源码目录或
安装目录里另有一份。磁盘资源与注册表由测试核对；公共语法包仍只读且不可覆盖。

技能按需读 `skills/gongwen-markdown/` 的语法与文种规则、材料角色、选区与已填要素；
只有会让整篇方向写错的事才 `ask_user`，授权要素不建议、不出题，也没有「先确认清单
才能生成」或「事实勾选才能进正文」的闸门。交付通道按任务类型分三类：正文用
`submit_document` 交完整 Markdown；只提炼 / 清单 / 报告型产物用 `finish_report`；
风格学习用 `submit_style` 交独立 JSON 草稿（只进设置页编辑器，不能写正文）。技能目录
按任务类型隔离：常规写作快照不含风格学习技能，风格任务快照只含它。

### 只读风格工具（4D）

Pi 增加两个只读领域工具：`style_list`（按当前文种列出启用且有适用范围限定的风格档案）
与 `style_read`（按 id 取回写法描述与范例）。它们直接复用
`ai_services::style::{StyleBook, render}`，不调第二个模型、不写风格库、不记使用次数、
不执行 shell；列表最多 20 份、返回文本有 token 与字符上限。未知 / 停用 / 不适用的
档案给结构化错误（`available:false` + `reason`），不悄悄换成另一份。风格内容与范例
按 `style_sample` 对待：只提供写法，不登记为事实来源，单位、人员、日期、数字不得
搬进正文。设置页风格学习已由 Pi 生成 JSON 草稿，用户查看编辑并保存后才更新风格库；模型直接自动入库不提供，模型不得声称已经保存。

导入/导出保留完整的 references、assets、scripts 等文件；脚本不会因此执行。
同名导入报冲突，不覆盖现有包。标准 frontmatter 用 YAML 解析，步骤写在 Markdown 正文中；
旧 flow 执行字段会被拒绝。编辑失败保留缓冲，切换文件/技能也不丢未保存文本。

侧栏下拉和 `/` 使用相同的启用目录。开跑时后台复制整包到任务工作区，并生成
`input/skills.json`（技能说明、路径、字节数与 SHA-256）；request.md 和 host 都读取本轮快照。
全局技能后续保存/停用不改变正在运行或恢复的任务，“从头重来”才取当前目录。
单包上限 32 MiB、256 文件，单文件 8 MiB、路径最多 8 段；每轮技能包合计最多 128 MiB，
索引最多 512 KiB。超限会明确失败，不静默截断技能列表。

## 九、已知限制

- **能力校准**：AI 管理 → 模型服务 →「测试写作能力」在后台做两组自动/分阶段合成通知对照；最多四轮，每轮 **300 秒**、总计 **15 分钟**，可停止；「超时」与「能力不足」分开记录（工具链超时不算能力不足），校准仍不产生 `verifiedWorkBudget`。仅使用内置技能、空风格库、临时SQLite资料，不读取用户稿件/知识库。档案在配置目录 `pi/capability.json`，原始合成输入/成品与机器判据在 `pi/calibration-evidence/<SHA-256>.json`。窗口数值或来源、模型、服务、思考参数、host/判据、技能内容变化后重新匹配；服务器不暴露的权重/默认参数变化仍需要用户重测。
- **校准结论边界**：当前短任务测试只支持工具调用和成对策略观察；不验证长输入预算，不因一次网络/超时失败设置短步骤偏好。没有充分证据时保持未知：`workBudget` 按实际服务窗口规划并标注「未验证」，经验证的更小工作预算 `verifiedWorkBudget` 当前结构上不可得。恢复任务使用原快照；「从头重来」才读取新证据。

- **第五阶段策略恢复**：短步骤在同一个 Durable Harness 的空闲边界 reset。宿主登记保存 active cursor、输入/成果哈希、依赖、用户回答与实际 kb/ms 只读证据快照；恢复先核验并还原阶段，再恢复 Harness。长单行可用字符范围续读，知识/稿件工具的单次结果同时受工作预算限制。最终交付仍完整读取并检查文件，不用片段读取冒充全文。
- **技术反馈（Pi 不做事实核查）**：显式提交时会反馈缺少题名、标题手写编号、研究报告脚注和附件标记等格式错误，模型可修改文件后重新提交；不自动清洗原稿。Pi 交付检查不做事实核查、不生成事实疑点清单，因此它不能证明模型完成了业务要求或查全依据（Qwen 的漏查与无据推断仍见第五阶段主审记录）。

- **体积**：Windows 组装后 `runtime/pi` 约 162 MB（Node 87 MB + 生产依赖约 75 MB，
  1.3 万个文件）；完整安装包会明显变大，增量包只带变化的文件。
- **Linux / macOS 真机未验收**：Linux 骨架已进 CI（buster 作业组装 `linux-arm64` 运行时 +
  离线验收 + Pi 的 Rust 进程 / 锁 / 交付测试，GLIBC / GLIBCXX 与解释器也在目标架构检查），
  **真机整链仍待验**；macOS 的运行时组装、离线验收（`lsof`）、打包与 `macos-15` CI 已补，
  但尚未在真机和发布环境跑通，签名 / 公证需要证书与 Apple 账号（见
  [`packaging/macos/README.md`](../packaging/macos/README.md)）。本机只真验 Windows x64。
- **Windows 干净机整链未验收**：安装包在无开发环境机器上的安装 → 启动 → 内网模型 →
  写入 / 撤销 → 重启「接着写」全链，仍按第七节清单待真机执行。
- **5D / 5E 仍未通过**：较强模型同题对照、干净机安装、Linux / macOS 真机验收均未完成，
  旧代码退役（旧会话只读 → 旧检查点 → `skill_job` → `engine/router/ops`）未开始；文档同步
  不代表验收通过。
- **manifest 较大**（约 2.6 MB）：启动时解析一次，不在每帧或每个工具调用重复读。
- **工作区自动清理规则见第六节**：只删已写入正文或超期孤儿目录；其余目录保留以支撑「接着写」，不会无提示整目录回收。

## 风格学习的独立文件交付（当前 host 0.4.2）

设置页通过同一 `PiJob` / `run` / Durable Harness 执行 `gongwen-style-learn`，任务文种标识为“风格档案”。
风格任务只注册文件、阶段登记及 `submit_style`，不注册正文提交、表单、知识库或写作模型工具。
程序提供的样稿角色为 `style_sample`，候选范例和统计保存在只读 `input/style-guide.json`。

`submit_style` 校验64 KiB内的JSON、字段上限、候选范例编号和路径，再使用现有不可变文件回执。
回执 `kind=style`、成品快照 `final.json`；JSONL仍用submitted/completed传输，由Rust核对任务与回执类型后返回独立 `PiOutcome::Style`。
普通正文任务拒收style回执，风格任务拒收document/report回执；恢复时类型不一致同样拒绝。
已有style回执可通过同一回执重放链返回同一文件，不重跑模型。设置页当前提供停止、重新学习，不提供单独的“接着学习”恢复按钮。

模型只生成风格内容，程序填身份、来源、统计和范例原文；结果进入设置页编辑器，只有用户保存才更新风格库。
上传样稿留在工作区；档案里的 `learning_workspace` 是本机相对引用，用于重新学习上传样稿，读取前校验材料哈希。
详情见 `pi-implementation/style-learning-v1.md`。

## host 稳定性加固（2026-11-06 第七轮，host 0.4.0 / 协议 4）

加密工作区把 host 的运行目录、Durable JSONL 与交付回执都改成 GWA2 密文后，本轮补了一轮
崩溃/并发/长会话稳定性修复（协议与 host 版本号不变，随包 `runtime/pi` 需按新源码重建）：

- **崩溃残留**：`.<pid>-<uuid>.tmp` 在 SIGKILL/断电后残留会让 Durable 恢复的目录列举
  整目录失败、`work/` 里也会让模型写失败。host 启动与每轮开始会递归清理该模式的普通
  文件（限深度/条目），目录列举与配额扫描跳过它们；`info()` 对 GWA1/GWA2 文件按
  `stat.size - 36` 算明文长度，不再为列目录整份解密。
- **上限**：加密追加单文件明文上限 64 MiB（超限报 `EFBIG` 并提示清理旧会话，旧内容
  保持完整），读取默认上限 256 MiB；原子写 rename 后尽力 fsync 父目录（Unix）。
- **占用锁自愈**：`host-owner.lock` 是 SQLite 锁载体。若它被旧版本迁移误封成 GWA2 或
  损坏，host 只对已知锁名做一次受控自愈（删除后重建）；`SQLITE_BUSY` 仍按「另一个窗口
  在用」处理，绝不删锁。
- **会话完整性**：`progress.json` 有记录而 `pi/session/main.jsonl` 缺失/0 字节时，
  「接着写」明确报 `resume_session_missing`，不再静默当空会话重跑；Rust 侧同样在恢复
  前校验主文件，并把「Running 但没有活动宿主」的轮次纠正为 Interrupted。
- **路径口径**：资源 AAD 的逻辑路径在 Rust 侧按 realpath 规范化，host 同时接受字面与
  realpath 两种根（配置目录含符号链接/junction、大小写差异时仍能读懂同一批密文）。
- **已知边界**：pinned `pi-durable` 1.5.0 对「marker 引用的 sidecar 记录缺失」仍会
  硬失败（1.5.0 未修）；JSONL 长会话暂无压缩（现靠 64 MiB 上限明确报错）；Windows 绝对路径 +
  符号链接的 `resolveInside` 语义未改。

## Pi 1.5 升级（2026-11-10，host 0.4.2 / 协议 4）

`@earendil-works/chord` / `pi-ai` / `pi-durable` 1.4.0 → 1.5.0（2026-11-02 发布），三包锁定同升
（pi-durable 1.5.0 依赖 chord 1.5.0 + pi-ai 1.5.0 + pi-schema 1.5.0）。host 0.4.1 → 0.4.2，
协议仍为 4。动机与收益：崩溃后模型请求不再无限重发（run 以 `interrupted` 明确终态）、
流式工具参数解析 O(n²) 修复、流式提交内存与长会话 CPU 下降、OpenRouter/智谱等网关兼容修复。

host 侧适配的破坏性变更（按上游 1.5.0 CHANGELOG）：

- `watchEvents()` 移除：`agent.mjs` 改挂 `root.watch(context, { min: 200, max: 400 })`，
  每次投递 `ConversationView`（`pi.live` + 条目窗口）；`activity.mjs` 重写为对连续快照 diff，
  事件→状态对应：工具槽位 `live.tools`（参数倒查助理条目）、重试看 `live.generation.retry`、
  失败看 `live.runFailure`、模型状态行看 `live.run` / `live.generation`。
- `onReport(error, info)` 改为 `onReport(report)`（`report.kind` 分派）；
  `task_parked` 报告移除：restart-limited 任务改为明确终结——内置任务报
  `task_interrupted(reason: "restart_limit")`，run 以 `runFailure reason: "restart_limit"` 收尾、
  inputs 结算 `unanswered`（reason 同上）。host 在恢复路径与 `runDelivery`/`runPhase` 的
  unanswered 分支把这两种 reason 映射回旧 `task_parked` 错误码，Rust 侧 `Parked` 语义不变。
- `Type`/`Static` 不再从 pi-ai 导出：host 直依赖 `typebox@1.3.27` 并改从 `typebox` 导入
  （TypeBox schema 仍是合法 JSON Schema，pi-schema 照常校验）。
- watch 监听签名统一为单 call 对象（`call.kind === "value"` 取 `call.value`）。
- `Storage.commit()` 改批签名 `commit(batches, receive, context)`：host 不实现自定义 Storage，
  仅两处加密测试改为 `commit([[...]], () => {}, ctx)`。
- `pi.live` 存储升到 v3（加 `run.turns`），1.5 写的 JSONL 会话 1.4 runtime 读不了：
  随包运行时整体更换，不允许新旧 host 混跑同一工作区（host 版本有 Rust 侧强校验）。

验证（2026-11-10，本机 macOS darwin-arm64 实测）：

- **Node 测试**：升级提交时 `cd pi-host && npm test` 为 **123 项 / 122 通过 / 0 失败 / 1 跳过**
  （跳过的是需要公网的真实模型用例）；本次 1.5 升级一共新增 **15 条用例**——1 条「host 版本与
  协议在 `protocol.mjs`、打包配置、`package.json`/lock 与 Rust 四处一致」的版本对账用例
  （`pi-host/test/protocol.test.mjs`）+ 14 条会话视图与报告映射回归，合计 **138 项 / 137 通过 /
  0 失败 / 1 跳过**。版本对账用例保证：改 host 版本或协议时漏改 `packaging/pi-runtime.json`、
  `pi-host/package.json`、lockfile 或 Rust 常量会直接失败；已用「把配置改回 0.4.1」「把协议改成
  5」两种人为漂移验证过它确实能拦住。
- **构建期护栏**：`scripts/pi-runtime-manifest.mjs` 不再直接抄 `packaging/pi-runtime.json` 的
  版本写 manifest，而是读取组装出来的 `host/package.json` 与 `host/protocol.mjs` 常量再核对；
  这样「配置改了、host 源码漏改」会在**组装阶段**直接失败，而不是等到运行期握手才报错
  （已用 host 0.4.1 / 协议 5 两种人为漂移验证）。
- **无公网验收**：`./runtime/pi/bin/node scripts/pi-offline-acceptance.mjs --runtime ./runtime/pi`
  PASS——Node 22.22.3 · host 0.4.2 · 13796 文件 · 常规模型调用 4 次 · 回执重放 0 次新调用 ·
  分阶段模型调用 9 次（重建修复后的 host 源码后重跑，见「运行时重建」）。
- **依赖安全**：`cd pi-host && npm audit --omit=dev` 为 **0 漏洞**（依赖树 94 个节点；无 high /
  critical / moderate / low）。运行时 `host/node_modules` 实测：`typebox@1.3.27`、
  `@earendil-works/pi-schema@1.5.0`、`ws@8.21.0`。
- **ws / esbuild 说明**：chord 1.5.0 把 `ws@8.21.0`、`esbuild@0.28.2` 声明为**精确的可选
  peerDependency**（`peerDependenciesMeta` 两者都是 optional）。ws 仍被 `@google/genai`
  （依赖 `^8.18.0`）和 `openai`（peer `^8.21.0`）需要，lockfile 从 8.22.1 收敛到 8.21.0 是满足
  chord 精确 peer 的结果，不是异常降级；esbuild 没有其它依赖需要，未安装。两者只在 chord 的
  `dist/next-node/` 里被静态 import，而 `dist/next-node/` 不在任何导出入口的导入链上；host 只
  import `@earendil-works/chord/context`、`pi-ai` 的 `models` / `api/*`、`pi-durable` 根与
  `env` / `storage/jsonl` / `tools` 子路径、`typebox`。升级核查时用 ESM resolve 记录 host
  导入链共 **551 个模块，无 ws / esbuild**，chord 与 pi-durable 根入口均可正常加载。
- **Node 归档**：五个平台的归档文件名与 SHA-256 均与官方 `SHASUMS256.txt`
  （`https://nodejs.org/download/release/v22.22.3/SHASUMS256.txt`）核对一致；本次未改 Node
  版本，哈希沿用。
- **许可证**：`licenses/DEPENDENCIES.json` 覆盖全部生产依赖（94 条按 name@version 去重后
  93 条，无重复），`licenses/deps/` 收录各包随附的许可原文。本次给收集脚本补上对 `LICENSES/`
  子目录的处理；**重建运行时后产物已含 `licenses/deps/earendil-works__pi-schema@1.5.0/
  typebox-LICENSE`**（1094 字节，来自 pi-schema 的 `LICENSES/typebox-LICENSE`）。上游
  `@earendil-works` 各包自身没有随包提供 LICENSE 原文文件，对应 deps 目录只能留空，仅
  `DEPENDENCIES.json` 记录 MIT（上游打包所致，不伪造原文）。
- **运行时重建**：2026-11-10 08:14（Asia/Shanghai）按**修复后的 host 源码**（`activity.mjs` /
  `agent.mjs`，含会话视图与报告映射修复）重建 darwin-arm64 运行时：16 个
  `pi-host/src/*.mjs` 覆盖进 `runtime/pi/host/`（`host/package.json` 与 staging 产物结构一致：
  version 0.4.2、dependencies 三包 1.5.0 + typebox 1.3.27、engines 不变），重跑许可证收集与
  manifest 生成。manifest 文件数 **13795 → 13796**（新增 pi-schema 许可原文一条），
  `node scripts/pi-runtime-verify.mjs runtime/pi darwin-arm64` **PASS**（host 0.4.2 / 协议 4，
  13796 文件全哈希），离线验收 **PASS**（常规 4 次 / 分阶段 9 次模型调用、回执重放 0 次）。
  `host/activity.mjs`、`host/agent.mjs` 与 `pi-host/src/` 同文件逐字节一致，manifest 里的
  sha256 / bytes 与磁盘一致。其余平台（win-x64 / linux-arm64 / linux-x64 / darwin-x64）的运行时
  必须在各自平台发布前从 `pi-host` 源码重建——**这次重建只覆盖本机 darwin-arm64**。CI / release
  各平台作业都先跑组装 + 无公网验收，再进入打包，不会被旧 `runtime` 顶替。
- 仍待真机验收：真实模型长会话、崩溃恢复与目标机门槛。

## Pi 1.5.1 复核（2026-11-10，host 0.5.1 / 协议 4）

三个 Pi 包锁定至 1.5.1，修复并行工具结束时偶发丢失调度通知、整轮无法结算的问题。
会话视图按 1.5 的带标签帧消费，存储回归用上游 `commitWrites` 测试入口。
沿用运行记录的版本规则：旧 host 的中间轮次从头运行，已有成品仍可按回执取回。
[上游更新记录](https://github.com/earendil-works/pi/blob/main/packages/durable/CHANGELOG.md#151---2026-11-09)

## 任务助手作业（自 host 0.5.0，协议 4）

任务助手的材料整理与单任务填写自 2026-11-10 起走 Pi，不再是一次性补全调用。

- 两个内置技能：`gongwen-task-extract`（材料 → 任务候选）、`gongwen-task-fill`（单任务表单
  → 六栏）。它们不进侧栏写作下拉（`Catalog::writing_enabled` 排除），由任务助手页面直接指定；
  技能管理页仍可停用或覆盖。作业入口 `src/pi_agent/task_jobs.rs`，与风格学习同构：
  `PiWorkspace::prepare` → `run` → 交付 JSON → Rust 复核。
- 工具面收窄：只挂 `read` / `write` / `edit` / `record_progress` / `submit_task` 与领域工具
  `task_search` / `task_read` / `vocab_search` / `current_date`。知识库、稿件库、内网接口、
  `check_document`、`doc_elements`、风格工具与 `ask_user` 一律不挂；Rust 侧 `PiToolContext`
  另有一道 `task_only` 白名单。`task_search` / `task_read` 每次调用新开一个只读任务库连接。
- 交付：`submit_task({path, summary})`，路径在 `work/` 下、`.json`、≤ 2 MiB；`mode` 必须与技能
  一致；host 写 `final.json` + `receipt.json`（`kind = "task"`），`submitted` 事件与正文交付同一条；
  分阶段时交付阶段要求先 `record_progress` 登记 `final`（path/sha 与提交一致）。
- 复核与门禁：`parse_task_delivery` 按 `mode` 复用原有校验——`evidence` 必须逐字出现在原文
  （只忽略空白与全半角标点）、期限须有原文支撑、候选类型白名单、目标 ID 必须存在于任务库；
  期限按本轮开始时的日期基准复核，任务 ID 按交付时的任务库复核。候选只交任务页，用户采纳才更新正式任务。
- 关闭填写表单会停止该轮填写。锁屏与关窗会取消在跑的任务助手作业并丢弃迟到结果。锁屏同时清除
  待自动整理的队列，知识库材料仍可手工发起整理。
- 整理与填写使用各自保留的工作区目录键。AI 管理页清理时各保留最近三轮，已结束且超过七天的
  作业可清理。旧版写在稿件目录里的任务作业按技能识别。
- 放开的一次性门禁：不再只注入最近 50 项任务（改用 `task_search`）、材料按 512 KiB 分块
  （每轮最多 20 份、合计 4 MiB，单份原文上限 4 MiB）、候选上限 100 条、任务作业不占
  `begin_writing` 写作门禁，可与 Pi 写作并行。
- 依赖：任务作业需要已选 Pi 写作模型与随包 `runtime/pi`（当前 host 0.5.2）；缺任一项直接报错，
  不回落到旧起草模型。各平台发布前按锁文件重新组装随包运行时。


## 维基作业宿主接线（2026-11-11，host 0.5.2 / 协议 4）

- 归纳、整编、更新各自只开放所需的只读域工具，均走 `submit_task` 交 JSON。宿主按技能检查 `themes` 或 `pages` 的外层结构，Rust 负责核对引用和写入权限。维基作业要求一次读完当前目标页，容量不足时按目标页缩小批次。
- 任务助手开放四个维基只读工具。维基内容用于理解背景，任务候选与表单字段的连续原文证据仍取本轮材料。
- 维基工具独立限 12 次，宿主和 Rust 分别计数。维基正文和来源原文分页读取时遵循宿主的读取预算。升级时同步重建随包运行时。
