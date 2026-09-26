# code-review-graph 本地集成

本地优先的代码知识图(`tirth8205/code-review-graph` v2.3.9)，
为 Claude Code / OpenCode / Codex 三个 MCP 客户端提供
"读最小代码就能 review 改动"的能力。

## 安装

```bash
uv tool install code-review-graph
```

## 三平台 MCP 配置

```bash
code-review-graph install --no-instructions --no-hooks --no-skills -y --platform claude-code
code-review-graph install --no-instructions --no-hooks --no-skills -y --platform opencode
code-review-graph install --no-instructions --no-hooks --no-skills -y --platform codex
```

| 平台 | 配置文件 | 性质 |
| --- | --- | --- |
| Claude Code | `<repo>/.mcp.json` | 项目级,已入库 |
| OpenCode | `<repo>/opencode.jsonc` | 项目级,已入库 |
| Codex | `~/.codex/config.toml` | 用户级,仅本机 |

三个平台都用 `uvx code-review-graph serve` 启动 stdio MCP。
**禁止去掉 `--no-instructions`**：`AGENTS.md` 是给全仓库协作者
与 omp agent 看的工程规则（含三条红线 / `docs/ai-architecture.md` 约束），
不能被 install 注入的"用图做 review"提示污染。

## 构建与守护

一次性全量构建：

```bash
code-review-graph build    # ~20s, 索引 src/ 等 165 文件 → 4605 节点 71458 边
```

后台守护（增量更新）：

```bash
code-review-graph watch
```

会话内以 `name=crg-watch` 起的进程会一直跑，watch 到 `src/`、
`tests/`、`scripts/` 等 17 个路径的文件变更后增量重建。
可通过以下命令与之交互：

- `code-review-graph status`：看当前节点 / 边数 / 最近更新时间
- `proc://crg-watch/status`：读守护进程输出（最近的增量更新日志）
- `proc://crg-watch/kill`：停掉守护

**已知限制**：Windows 重启后 `crg-watch` 不会自启，需手动再跑一次
`code-review-graph watch`。暂未注册 Windows Task Scheduler 任务；
若以后需要开机自启，新增 `scripts/start-crg-watch.ps1` + 任务计划注册脚本。

## 排除规则（`.code-review-graphignore`）

不复用 `.gitignore`：图数据库需独立维护一份白名单/黑名单，
否则大量 `target/`、`vendor/`、`runtime/` 等被错误索引。

当前已排除：

- 构建产物：`target/`、`dist/`、`output/`、`tmp/`、`*.aux` 等
- 第三方代码：`vendor/qingjian/`（输入法内核，GPL，意义不大）、
  `vendor/mdx/`（独立 workspace）
- 用户/运行时资源：`config.json`、`.env`、`/font/`、
  `/runtime/{fonts,tectonic,texbundle,ime}/`
- AI 工具技能包（外部）：`skills/`
- 图自身：`.code-review-graph/`

`.gitignore` 里的 `.code-review-graph/` 保证图数据库本机使用、不入库；
`.mcp.json` 与 `opencode.jsonc` 已脱敏：只写 `uvx code-review-graph serve`，
**不写 `D:\gongwen` 这类本机硬路径**（server 从启动目录向上找 git 根自动
定位仓库，换机器/换克隆目录都能用），codex 配在用户目录 `~/.codex/config.toml`。

## 协作约定

- AI 产物落地前的确定性闸门（`ai_guard` / `validator` /
  `proofread` / `proofread_rules`，见 `AGENTS.md` 三条红线）**不依赖**
  本图。本图只给 review / 探索提供上下文，不参与公文写作流程。
- `code-review-graph` 的子命令（`query`、`impact`、`search`、
  `flows`、`dead-code`、`large-functions`、`refactor`）可由 MCP
  客户端直接调用，prompt 见各工具文档。
