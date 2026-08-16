---
name: release
description: 发布 gongwen 新版本到 GitHub Releases：bump 版本、推送 main、打 annotated tag 触发 CI/CD、监控 workflow、填写版本更新说明并验证。当用户要求「发布 / 发版 / 出新版本 / 打 tag / release」时使用。
---

# 发布 gongwen 新版本 — Agent Skill Guide

把当前 `main` 上的改动发布为 GitHub 新版本。**硬性要求：release 必须带有手写的版本更新说明，不能只推 tag 或依赖 `--generate-notes` 的占位说明**（用户明确要求）。

## 仓库事实（勿重新探索）

- 远程：`git@github.com:billowsand/gongwen.git`，发布分支 `main`。
- 版本号只维护在 `Cargo.toml`（根包 `version`）+ `Cargo.lock`（根包 `gongwen-assistant` 的 `version`）。
- 工作流：`ci.yml` 在 push main 时触发（fmt / clippy / test 三平台）；`release.yml` 在 push `v*` tag 时触发（构建 Windows setup.exe、Linux ARM64 deb、macOS ARM64 DMG + 各自 `.sha256`，共 6 个资产，最后 `release` job 用 `--generate-notes` 创建占位 release）。
- 本机无 `pwsh`：不能跑 `scripts/bump-version.ps1`，版本号用手动编辑。
- 历史惯例：tag 为 **annotated tag**（消息为中文概述）；release 标题为「公文助手 vX.Y.Z」；版本号按 **patch** 递增（`v0.3.x` 系列）；发布前有一个 `chore: release vX.Y.Z` 提交。
- Release workflow 全程约 20–30 分钟（三平台构建 + 打包）。

## 工作流

### 1. 前置检查

```bash
cd /Users/ours/Development/gongwen
git status -sb                 # 必须干净；确认所在分支为 main
git log origin/main..HEAD --oneline   # 本地领先的待发布提交
gh auth status                 # 确认已登录（billowsand，有 repo scope）
```

### 2. 确定版本号与发布内容

```bash
git tag --sort=-v:refname | head -3     # 最新 tag，如 v0.3.21
git log v<上次版本>..HEAD --oneline      # 本版本包含的提交
```

- 新版本号 = 最新 tag 的 patch + 1（如 `v0.3.21` → `v0.3.22`）；若含大量新功能且用户有暗示，可 bump minor，否则默认 patch。
- 用 `git log` 提炼用户可见变更（`feat`/`fix`，可结合 `git show <commit> --stat` 看具体内容与名称），作为版本更新说明素材；`refactor`/`docs` 归入「工程与维护」。

### 3. 本地质量门禁（关键，勿跳过）

CI 的 `Check formatting` 步骤对格式零容忍，历史上因 refactor 提交未跑 `cargo fmt` 导致发布流程返工。**发布前必须确认**：

```bash
cargo fmt --all -- --check     # 必须无输出（exit 0）
```

- 若不通过：`cargo fmt --all` 修复 → `cargo check --all-targets` 确认编译 → 单独提交 `style: cargo fmt 统一代码格式`（这会改变发布提交集，见步骤 5 的 tag 移动）。
- 若 `cargo fmt` 通过，可选跑 `cargo check --all-targets`（增量，通常 10s 内）确认基线。

### 4. bump 版本并推送

手动编辑（无 pwsh，等价于 `bump-version.ps1 -Part patch`）：

- `Cargo.toml`：`version = "0.3.21"` → `"0.3.22"`。
- `Cargo.lock`：根包 `name = "gongwen-assistant"` 下的 `version` 同步修改。
- 用 `grep -rn '0\.3\.21' Cargo.toml Cargo.lock` 确认无残留。

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: release v0.3.22"
git push origin main          # 触发 ci.yml
```

### 5. 创建 tag 并推送，触发 release.yml

```bash
git tag -a v0.3.22 -m "本版本<一句话概述，如：界面主题扩充至十二套，公文纸面支持深色显示。>"
git push origin v0.3.22
git ls-remote --tags origin v0.3.22   # 确认远程 tag 存在
```

**若步骤 3 新增了 fmt 修复提交（或任何后续提交）**，tag 必须指向最终发布提交：先删除本地+远程 tag，重打后 `git push origin v0.3.22`（新 tag 推送不需要 `--force`，因为远程 tag 已删）。

### 6. 监控 CI/CD

```bash
gh run list --limit 6         # 找到本次 CI（main push）与 Release（v0.3.22）两个 run
gh run watch <run-id> --exit-status   # 可后台运行，两路并行
gh run view <run-id> --json jobs --jq '.jobs[] | {name, status, conclusion}'
```

- 期望终态：CI 三平台 job 全 success；Release 的 5 个 job（Build Windows / Build Linux ARM64 / Build macOS / Package Linux ARM64 / Publish GitHub release）全 success。
- **CI 失败时**：先 `gh run cancel` 取消所有本次相关 run（CI + Release，避免浪费与旧 release 干扰）→ 本地修复（多数是 fmt）→ 提交 → 移动 tag（步骤 5 的做法）→ 重新 `git push origin main` + `git push origin v0.3.22`。

### 7. 填写版本更新说明（必须，勿跳过）

Release workflow 的 `release` job 创建的是 `--generate-notes` 占位 release（标题为 `vX.Y.Z`、正文只有 changelog 链接）。等它完成后：

1. 编写正式中文说明到临时文件（模板见下节），参考上次 release 正文风格。
2. 用 `gh release edit` 替换标题与正文：

```bash
gh release edit v0.3.22 --title "公文助手 v0.3.22" --notes-file /tmp/release-notes-v0.3.22.md
```

### 8. 最终验证

```bash
gh run view <ci-run-id> --json status,conclusion
gh run view <release-run-id> --json status,conclusion
gh api repos/billowsand/gongwen/releases/tags/v0.3.22 --jq '{name, tag_name, draft, prerelease, html_url}'
gh release view v0.3.22 --json assets --jq '.assets[] | .name'   # 应为 6 个资产
git status -sb                # 与 origin/main 同步、工作区干净
```

- release 必须：`draft=false`、`prerelease=false`、标题「公文助手 vX.Y.Z」、正文为手写说明、6 个资产齐全。
- 清理临时文件：`rm -f /tmp/release-notes-v0.3.22.md`。

### 9. 收尾汇报

向用户报告：版本号、release 链接（`https://github.com/billowsand/gongwen/releases/tag/vX.Y.Z`）、CI 与 Release 两个 workflow 结果、发布说明要点、过程中遇到的问题与处理。

## 版本更新说明模板

参考 `v0.3.20`、`v0.3.21` 的 release 正文风格：

```markdown
## 公文助手 v0.3.22

本版本<一句话概述主题>。

### <功能域一，如：主题与纸面>

- <feat 要点，带关键细节/名称>
- <fix 要点>

### <功能域二，如：编辑与排版>

- ...

### 工程与维护

- <refactor/docs 等内部变化，注明对用户无影响>

### 下载

- Windows x64 安装程序
- Linux ARM64 Debian 软件包（兼容 GLIBC 2.28）
- macOS ARM64 DMG

各安装包均附带 SHA-256 校验文件。
```

## 常见故障

| 故障 | 处理 |
|---|---|
| CI `Check formatting` 失败 | `cargo fmt --all` 修复 → 独立提交 → 移动 tag 重新发布（步骤 3/5/6） |
| Release run 需重启 | `gh run cancel` 旧 run → 重新 push tag（若 tag 未变，可 `gh workflow run release.yml` 不便时删 tag 重推） |
| 发布后又加了提交 | tag 移到新提交：`git tag -d vX.Y.Z && git push origin :refs/tags/vX.Y.Z && git tag -a vX.Y.Z -m "..." <new-commit> && git push origin vX.Y.Z` |
| release 标题/正文不对 | `gh release edit vX.Y.Z --title "..." --notes-file ...`（幂等，可重复执行） |
