# Pi 退役审查收尾与验收记录（2026-11-05）

状态：F1–F5 代码/文档收尾、主审与本机软件回归完成；真实模型和目标机验收仍待条件，5D/5E 未整体通过。当前产品状态以 [Pi 现状索引](../pi-current-status.md) 为入口。

## 基线与分工

- 仓库 D:\gongwen，main，HEAD f882735d2feac49805bc8a1b17b4b103f68609d9。
- 已有未跟踪 docs/ai-agent-retirement-audit-2026-11-05.md；基线和完整 TASK 保存在 tmp/agent-retirement-2026-11-05/。保留原审查内容，追加处置记录。
- 用户授权按审查建议实施；一个 gpt-6-luna / high 实现子代理修改限定代码，主代理维护文档、审查真实 diff 并验收。没有提交、推送、发布或安装授权动作。
- 不删除旧历史执行入口；5D 仍是后续删除门槛。

## 本轮验收范围

1. 常规 60 / 分阶段硬上限 84；常规转分阶段允许按阶段追加，84/85 与跨语言边界对齐。
2. 已选 Pi 的新辅助调用不借隐藏旧起草备用；未选 Pi 的旧配置兼容保留。
3. 删除未编译旧技能管理器；数字解析与通用提问校验提取共享层，旧序列化/权限保持。
4. 权威现状索引、历史文档状态、评测矩阵与 CI 回归入口同步。
5. 本机能执行的格式、编译、严格 lint、Rust/Node 回归和离线运行时验收。

## 真实模型与平台前置检查

本轮检查本机历史测试端口 127.0.0.1:12345、127.0.0.1:1234 的 /v1/models，以及 Ollama 默认 127.0.0.1:11434/api/tags。沙箱外只读复检均为目标计算机拒绝连接。未读取真实配置、凭据或稿件，未调用外部模型。真实模型效果、20 万汉字质量、真实资料链、干净机与跨平台 GUI 不据此标记通过。

## 实施与验证结果

实现子代理已停止写入，主代理检查了实际 tracked/untracked 文件与删除清单，修复范围未扩展到旧历史执行链。

### 代码与主审

- F1：领域工具常规 60，分阶段绝对上限 84，拒绝的调用不计入已发出次数。主审发现初稿仍以闭包捕获常规 60，已改为同一额度状态的 promoteToPhased，再按阶段追加；不重置本轮已用次数。行为测试实际发送 84 次，第 85 次拒绝且不入队。
- host 补丁为 0.3.6 / 协议 3。主审发现初稿漏改实际握手常量，已同步并用动态断言核对 Rust、host、package、lock 与运行时配置，不把补丁号写死在测试中。既有恢复保护会拒绝用新 host 续接旧 0.3.5 中间任务，成品查看/取回/从头写仍保留。
- F2：LmBackend 的 Draft 主/备用都按一次性写作用途解析；有 Pi 时不加载旧起草备用，没有 Pi 才兼容旧备用配置。测试覆盖有效/失效旧备用，以及主模型失败时旧备用监听端口没有连接。
- F3：精确删除旧 src/app/skill_manager/ 七个未挂载文件，约 2151 行；没有递归清理其它旧代码或用户技能。
- F4：纯数字解析移入 ai_services::text；通用题面/选项校验集中于 ai_services::questions，Pi 不再引用旧 clarify。旧 Question/Reply/Target 序列化保留，旧流程只做适配；授权要素、文种、选项、长度与示例回显防护保持。
- F5：建立现状索引并链接 AGENTS、方案和交接；明确历史写作页只读与旧侧栏仍可恢复的区别，标记被 Pi 替代的旧内核待办；两处评测目录表同步实际 11 例和 style 交付。Windows/macOS CI 新增离线评测判据回归，不连接真实模型。

### 已执行验证

- 最终 Node 全量：`node --test pi-host/test/*.test.mjs scripts/pi-writing-eval.test.mjs`，121 通过 / 0 失败 / 1 跳过（总计 122）；包括评测判据 22 项。随后仅调整动态版本断言，该项 1/1 通过。
- 评测 `--selftest` 通过；CI YAML 能解析，两份目录表均覆盖当前 11 个案例，现状索引链接有效。
- 最终 `cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`、`git diff --check` 通过。
- 最终 `cargo test --locked --all-targets`：**2896 通过 / 0 失败 / 89 忽略**；随后 `cargo test --locked shipped_pi_runtime_manifest -- --ignored`：**1/1 通过**，验证当前仓库 runtime/pi。
- 20 万汉字自造材料文件链路：UTF-8 600332 字节，窗口 262144，2 个材料块 / 2 页读取，重组 SHA-256 与原文一致。脚本与结果在 tmp/agent-retirement-2026-11-05/long-material-{smoke.mjs,result.json}；这不是模型压力或信息保持质量验收。
- 用锁定官方 Node 22.22.3 归档（SHA-256 与 packaging/pi-runtime.json 一致）和本机缓存离线 npm ci（禁安装脚本）组装 win-x64 / host 0.3.6，完整 manifest 校验 12800 文件通过。
- 无公网运行时验收通过：常规假模型调用 4 次、分阶段 9 次、已有回执重放 0 次新调用；用的是简化技能夹具，不冒充真实模型/技能业务效果。
- 13 个 host 源文件逐一核对 SHA-256 后，更新仓库 runtime/pi；旧运行时完整保留于 tmp/agent-retirement-2026-11-05/runtime-before-0.3.6。没有安装覆盖用户应用。
- 后续模型评测用的基线与源码/技能哈希保存在 tmp/agent-retirement-2026-11-05/source-snapshot.json；验证日志同目录（clippy-final.log、rust-full-final.log、rust-runtime-manifest.log、node-full-final.log、offline-final.log）。最终业务源码与该快照对齐。

### 环境失败与边界

首轮 Node 在沙箱中 116 通过 / 4 失败 / 2 跳过，失败为子进程回环连接和恢复等待；最终在允许回环环境跑同一断言全通过。子代理首轮 Rust ai_services 定向测试 33 通过 / 6 失败 / 1 忽略，失败是既有套接字权限错误 10013，未削弱断言。首轮运行时组装遇沙箱 npm 缓存 ENOTCACHED，使用本机完整缓存离线重建后通过。官方构建归档下载首次受沙箱网络限制，允许下载后 SHA-256 校验通过；目标运行不依赖联网安装。

主代理严格 Clippy 首轮在 vendored OpenSSL 首次本地编译后发现新增测试触发 field_reassign_with_default；主代理已将其改为结构体初始化，没有添加 allow 或放宽 lint，随后重新执行严格检查与全量测试。

主代理首轮完整 Rust：2879 通过 / 17 失败 / 89 忽略。17 个失败均由本轮升级 host 补丁后，两处 Rust 内嵌 Node 夹具仍写死 0.3.5 握手引起；这是需要同步的测试实现遗漏，不记为环境失败。主代理已让通用夹具和工具参数夹具从 HOST_VERSION 生成握手，保留生产版本不匹配拒绝规则，再跑全量。首轮失败日志保留在 tmp/agent-retirement-2026-11-05/rust-full.log。

最终测试构建仍输出第三方 OpenSSL 静态库缺少 ossl_static.pdb 的 LNK4099 调试符号提示，链接与测试成功；严格 Clippy 通过。未为消除此提示扩改 OpenSSL 构建或屏蔽警告。

未调用真实/外部模型，未访问用户稿件/配置/密钥，未安装、发布、提交或推送。5D/5E、真实资料链和多平台 GUI 不因本轮软件回归自动通过，旧恢复/检查点/提案/字段建议继续保留原权限。
