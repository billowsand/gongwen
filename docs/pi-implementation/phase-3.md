# TASK：第三阶段——恢复、交付去重与离线分发

你是用户指定的本机 Pi 实现 worker。直接实现并验证，主代理负责阶段提示词、主审、修正、最终回归与实际打包验收。先读必要文件；不要只写设计。

## 基线、协作与范围

- cwd `D:\gongwen`，main；第二阶段提交 `338d6eb`，其后两个其它任务提交 `1d80a24`、`d7c236e` 要保留。开始前核对 HEAD/status，根代理快照在 `tmp/pi-implementation/phase-3-baseline/`。不要还原任何其它改动。
- 同一个 worker 顺序实现 3A 恢复、3B 随包运行时，不创建子代理、不提交/推送/tag、不改版本。完成 `docs/pi-implementation/phase-3-report.md` 后立即停止写入与自查，交给根代理。
- 根代理拥有本 TASK、`docs/pi-implementation-plan.md`、主审记录。你不修改这些文件。
- 必读 AGENTS.md、`docs/ai-architecture.md` 三红线、`docs/pi-writing-product-design.md` 恢复/分发章节、`docs/pi-implementation/phase-2-review.md`、`docs/security-hardening.md`。改会话/导入/同步前读相应文档；本期不更改同步包格式。
- 禁止读取或打印 auth.json、用户 config.json、`.env`、模型测试密钥；用自造文稿/假模型/回环接口。真实凭据联机验收由主代理负责。
- 允许：`src/pi_agent*`，`pi-host/`（Pi版本保持1.4.0），`src/ai_panel.rs` 与 ai_panel 的 Pi/会话/UI 模块、`src/app/{pi_jobs,jobs}.rs`，必要的 `src/draft_page.rs` 及构造字段适配；必要的纯元数据 `src/ai_panel/{history,session_ui,workspace_ui}.rs`、`src/app/ai_proposal.rs`（仅Pi来源/交付元数据，不改原有接受规则）。打包文件允许 `scripts/` 内 Pi专用新脚本/配置及 `package-portable.ps1`、`stage-linux-portable.sh`、`package-deb.sh`、`package-arch-pkg.sh`、`verify-deb-install.sh`、`verify-arch-pkg-install.sh`、`smoke-launch-linux.sh`、`package-arch-local.sh`；`.github/workflows/{release,ci}.yml` 仅Pi运行时构建/验证接入；`THIRD_PARTY_NOTICES.md`；新增 `docs/pi-runtime.md`；本阶段报告。
- 不改旧 agent 编排、小模型路径、Typst/IME、版式、Cargo依赖/版本、旧技能语法包，不做第四阶段删除与默认入口切换。不要引入异步框架或通用任务引擎。需要清单外修改先报告并等主代理授权。新代码按功能拆模块，避免 process.rs/ui.rs 再堆上千行。
- 主代理补充批准：`packaging/pi-runtime.json` 保存固定运行时版本与官方校验值；`.gitignore` 仅增加生成的 `/runtime/pi/` 忽略项，防止随包二进制/生产依赖误入库。

## 3A 必须交付：可以恢复的真实写作任务

1. **运行记录与身份**：每轮有稳定 runId、workspace相对引用、协议/host版本、正式基线及哈希、候选基准、要求/技能/选区/材料角色与哈希、模型引用（禁止保存密钥；地址也无需进工作区）、本轮证据/已确认回答、交付回执与状态。与 AiTurn/SavedTurn 关联；旧会话缺Pi字段照常读取。工作区仍在配置目录，模型看不到宿主状态/凭据；元数据有大小/数量上限，读取坏记录返回中文问题，不崩窗口。
2. **明确恢复入口**：Pi 中断/停止/失败轮次可以“接着写”或“从头写”；沿用当前会话/正式正文的过期保护，已归档/发布只读拒绝。恢复必须校验工作区路径、快照完整性、host协议/版本及当前基线；不匹配明确拒绝，不能静默覆盖旧输入或偷偷从头运行。恢复沿用原工作区、Durable SQLite、runId/input requestId。新一轮追问仍是新的工作区与输入快照，不能混成恢复。
3. **真实 Durable 恢复**：研究已安装1.4.0 API（README与源码）。重开同一个Harness/root、用相同requestId找回提交，不用复制 transcript 冒充恢复，不把任务图搬成Rust Board。默认停用自动无限重启；若任务被parked应显示原因并给出明确人工动作。崩溃的 write/edit 不声明 replay-safe：恢复时中断调用让模型读当前文件确认，不盲目重插。文件写入要原子替换，已有 WorkspaceEnv 的安全路径/配额不能退步。
4. **停在问答**：旧进程的 callId/输入管道不能沿用。恢复后重新出必要问题/或由模型从已确认回答继续，待回答只影响上下文与事实台账；授权类问题仍拒绝。避免没有UI题卡却一直等待失效Promise。等待预算与取消仍有效。
5. **成品与清单去重**：`submit_document` 时复制内容为宿主不可变快照（不在模型可写work），哈希/提交调用标识/摘要落回执；`finish_report` 同样可重放。进程在“快照落盘、事件发送、应用收到/安装提案”任意点崩溃后，用回执返回同一产物，不重复生成/接受正文、不从可变工作文件重读旧提案。回执与文件必须绑定run，哈希不符拒绝。取消、超时、失败不冒充成功；恢复成功回执仍重新过本地闸门。
6. **多窗口所有权**：同一Durable库同时只能一个host占有，必须用真正的OS/SQLite锁，不能只靠前端busy或永不回收的exists标志。锁随进程死亡释放；错误应可读，不能破坏数据库。可使用独立owner SQLite事务锁（Node内置sqlite）或Rust标准库文件锁，但须覆盖host仍存活而Rust崩溃的情况。工作目录生成用UUID，避免多个窗口doc_key/job_seq相同。
7. **生命周期与来源**：会话重载后仍能继续修改未采纳提案，上一轮真实原始依据与其正式基线/提案绑定保存；不能把候选正文整体当事实来源。部分接受/人工改正文以后依据与基准按当前真实正文处理。保存/切会话时清理临时pi_ask/pi_bridge/pi_report等显示引用，不把上个会话卡片带过来。删除会话不危险递归删目录，先保留孤立运行目录并在文档说明；本期不做自动清理或GWX同步工作区。未入稿件库的新稿至少明确说明其重启恢复条件，不能显示假恢复入口；有库稿件开跑前强制保存运行引用。

## 3B 必须交付：目标机无需开发环境的固定运行时

1. `runtime/pi/node.exe`（Windows）或 `runtime/pi/bin/node`（Linux）、`host/main.mjs` 与其它host源码、`host/package.json` 和完整生产 `host/node_modules`、依赖许可证、manifest/校验清单。不能使用junction指向源码或用户Pi安装目录。没有 npm/Python/全局Pi、PATH里没有node也能由Rust绝对路径启动；用户数据/SQLite不写安装目录。
2. 构建期在线安装固定Pi1.4.0与lockfile传递依赖，建议 `npm ci --omit=dev --ignore-scripts --no-audit --no-fund`，如有运行资源生成脚本必须明确审查，不能静默遗漏WASM/worker/JSON。不要生产环境npm install。构建脚本不得把keys/.npmrc/.env、测试工作区、缓存、npm或开发工具装箱。
3. Node先固定 **22.22.3**（与本机已验版本一致，并非宣称最新）；官方下载SHA256已由主代理核对：
   - `node-v22.22.3-win-x64.zip`：`6c8d54f635feff4df76c2ca80f45332eb2ff57d25226edce36592e51a177ee33`
   - `node-v22.22.3-linux-arm64.tar.xz`：`1c4a9933a5e45bc88f54f70b5f91232c127ec49f1a5989d23fb85824c7adf9b7`
   - `node-v22.22.3-linux-x64.tar.xz`：`2e5d13569282d016861fae7c8f935e741693c269101a5bebcf761a5376d1f99f`
   来源 `https://nodejs.org/download/release/v22.22.3/SHASUMS256.txt`。把固定配置写为受版本控制文件，下载只在构建脚本发生，先校验再解包。不得运行未校验的下载内容。允许传本地已缓存压缩包离线组装，但也必须验同一校验值。升级版本以后明确改配置/重验，禁止latest自动跟随。
4. 平台Windows x64、Linux ARM64 deb（GLIBC2.28约束）、Linux x64 Arch。分别在目标架构组装依赖，不能Windows node_modules直接打给Linux而不审计平台包。Windows本地只真验Windows，Linux通过CI加真实架构检查，未运行不得声称通过。需要核查Node的ldd/GLIBC/GLIBCXX和解释器要求；不能把Rust兼容2.28等同于Node一定兼容。
5. 接入PowerShell staging与Linux staging、Windows完整包/增量包现有清单、release workflow三平台。缺Pi运行时或manifest不匹配时打包明确失败，不能发出去一个只有按钮没有运行时的包；不要替目标机下载补齐。PowerShell5.1中文脚本有UTF8 BOM；安全删除只允许验证后的本任务生成目录，不跨shell拼删除命令。
6. 运行时manifest至少声明平台架构、Node/Pi/host/协议版本、lockfile哈希、文件列表与SHA256。Rust定位随包运行时需要校验入口/manifest兼容性、缺文件/资源给中文错误，开发显式env模式可保留但不能冒充生产已验。pack阶段完整验证，启动阶段按成本选关键文件与元数据校验，不在每帧遍历node_modules。
7. 许可证：随包Node LICENSE与完整传递依赖license信息/通知，更新THIRD_PARTY_NOTICES。项目GPL不变。manifest与SHA256SUMS纳入原增量升级差分；移除旧文件仍按现有InstallDelete，不碰用户数据。
8. **无公网验收脚本**：对真实组装runtime启动host，用本地假OpenAI服务跑“读技能→读材料→编辑文件→submit_document”并验交付。白名单只允许loopback网络，检测意外公网连接，清空用户Pi配置/模型目录环境并限制PATH，不依赖开发node_modules；至少验证内置sqlite打开和恢复回执。脚本必须从打包目录运行，而不是import源码冒充。另给干净目标机的离线安装/升级步骤，不能把脚本模拟当干净机真验。

## 验证与停止点

- 自造文稿+假模型；host测试覆盖强制杀进程后的恢复、写后杀/回执后杀、重开数据库、锁竞争、坏快照/哈希/路径、重复提交/接收、问答中断、取消；测试不要靠长sleep碰运气。
- Rust测试覆盖SavedTurn旧新兼容、Pi运行引用/来源重载、过期正文拒恢复、接受后不重复恢复、会话隔离与本地闸门。新字段构造需要其它测试文件适配时仅补默认字段，不改测试判据，报告路径。
- fmt/check/clippy与相关Rust测试、host node --test。全量Rust回归、联网真实模型、GUI、真实Windows运行时装箱/安装验收留根代理；你可以先做自造runtime装箱脚本测试，网络/权限障碍明确报告，不越权读凭据或改系统网络/防火墙。
- 报告写清真实改动、测试命令/数字、未跑事项、安装体积、恢复语义与Linux限制，不能把“已写脚本”说成“已离线部署验证”。写完立即停止工具调用；不反复跑已通过检查，不提交。
