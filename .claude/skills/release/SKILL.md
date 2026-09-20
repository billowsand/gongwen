---
name: release
description: 发布 gongwen 新版本到 GitHub Releases：写版本更新说明、bump 版本、推送 main、打 annotated tag 触发 CI/CD、监控 workflow 并验证。当用户要求「发布 / 发版 / 出新版本 / 打 tag / release」时使用。
---

# 发布 gongwen 新版本 — Agent Skill Guide

把当前 `main` 上的改动发布为 GitHub 新版本。**硬性要求：release 必须带有手写的版本更新说明，不能只推 tag 或依赖 `--generate-notes` 的占位说明**（用户明确要求）。说明随源码提交在 `docs/release-notes/vX.Y.Z.md`，`release.yml` 发布时直接取用。

## 仓库事实（勿重新探索）

- 远程：`git@github.com:billowsand/gongwen.git`，发布分支 `main`。
- 版本号只维护在 `Cargo.toml`（根包 `version`）+ `Cargo.lock`（根包 `gongwen-assistant` 的 `version`）。**注意 `Cargo.lock` 里另有若干同名版本的依赖**（`block2`、`type-map`、`windows-strings`、`zune-core` 等），只改 `name = "gongwen-assistant"` 那一条。
- 工作流：`ci.yml` 在 push main 时触发（fmt / clippy / test 三平台）；`release.yml` 在 push `v*` tag 时触发，共 7 个 job。
- **版本更新说明由工作流取仓库文件**：`release.yml` 的 `release` job 读 `docs/release-notes/${GITHUB_REF_NAME}.md`，有则 `gh release create --title "公文助手 vX.Y.Z" --notes-file <该文件>`；**缺文件才回退 `--generate-notes` 并打一条 `::warning::`**。所以说明必须在打 tag 前提交进仓库，不是发完再补。
- **Release 资产是 14 个**（`v0.5.1` 起实测口径，不是 8 个）：
  - `gongwen-assistant-X.Y.Z-win-x64-setup.exe` + `.sha256`
  - `gongwen-assistant_X.Y.Z_arm64.deb` + `.sha256`、`gongwen-assistant_X.Y.Z_amd64.deb` + `.sha256`
  - `gongwen-assistant-X.Y.Z-macos-arm64.dmg` + `.sha256`
  - `gongwen-assistant-X.Y.Z-linux-{arm64,amd64}-omarchy.tar.gz` + 各自 `.sha256`（Arch/Omarchy 便携包）
  - `PKGBUILD` + `PKGBUILD.sha256`（由 `packaging/arch/PKGBUILD.template` 现场生成）
- 本机无 `pwsh`：不能跑 `scripts/bump-version.ps1`，版本号用手动编辑。
- 历史惯例：tag 为 **annotated tag**（消息为中文概述）；release 标题为「公文助手 vX.Y.Z」；版本号按 **patch** 递增（`v0.5.x` 系列）；发布前有一个 `chore: release vX.Y.Z` 提交（可同时带上说明文件）。
- Release workflow 全程约 20–30 分钟（三平台构建 + 打包），`gh run watch` 一轮就能等完。

## 工作流

### 1. 前置检查

```bash
git status -sb                  # 确认在 main；工作区应干净
git log origin/main..HEAD --oneline    # 本地领先的待发布提交（可能为空，改动已在远程）
git log v<最新 tag>..HEAD --oneline    # 本版本实际包含的提交
gh auth status                  # 确认已登录（billowsand，有 repo scope）
gh run list --limit 3           # main 上最近一次 CI 是否 success
```

- 工作区有未提交改动时：**问用户是否纳入本版，不要替用户决定**（可能是有意的未完成工作）。纳入则以独立提交（按改动性质 `fix:` / `feat:`）先提交，再做 bump。
- 若 main 最近一次 CI 是 failure：先查失败原因（见步骤 3 与「常见故障」），修好随本版一起发，否则本版 CI 也是红的。

### 2. 确定版本号与发布内容

```bash
git tag --sort=-v:refname | head -3     # 最新 tag，如 v0.5.1
git log v<上次版本>..HEAD --oneline      # 本版本包含的提交
git show --stat --format='%s%n%n%b' <commit>   # 逐个看正文与改动面
```

- 新版本号 = 最新 tag 的 patch + 1（如 `v0.5.1` → `v0.5.2`）；若含大量新功能且用户有暗示，可 bump minor，否则默认 patch。
- 用提交正文（本仓库的 `feat`/`fix` 提交信息写得很详细）提炼用户可见变更，作为版本更新说明素材；`refactor`/`docs` 归入「工程与维护」或「文档」。

### 3. 本地质量门禁（关键，勿跳过）

CI 有三道门，历史上因未跑 `cargo fmt` 导致发布返工，也出现过**测试在 CI 的 macOS / Linux 上失败而本地 Windows 通过**（环境假设类断言）。**发布前必须跑**：

```bash
cargo fmt --all -- --check                                    # 必须无输出（exit 0）
cargo clippy --locked --all-targets -- -D warnings             # 必须干净
cargo test --locked --all-targets --no-fail-fast               # 看失败名单
```

- 格式不通过：`cargo fmt --all` 修复 → 单独提交 `style: cargo fmt 统一代码格式`（改变发布提交集，见步骤 5 的 tag 移动）。
- 测试失败要**分清是环境抖动还是真失败**：
  - 已知沙箱抖动：`highlight::tests::swapping_light_and_dark_relayouts_against_the_rebuilt_font_atlas`（字体图集重建，首跑偶发失败、重跑即过），以及 AGENTS.md 记录的约 5 个沙箱相关失败。**不要为环境相关失败改测试。**
  - 若断言依赖「本机装了什么」（字体、代理、路径），要问的是「CI 上成不成立」，多为 CI 独有失败，按下面处理。
- 三平台差异的排查入口：

```bash
gh run view <run-id> --log-failed | awk -F'\t' '{print $3}' | grep -E "panicked|test result:|##\[error\]"
gh run view --job <job-id> --log | grep -E "<测试名>|test result:"   # 确认某用例在特定平台的结果
```

### 4. 写版本更新说明并 bump 版本

先写说明（模板见下节），文件名必须与 tag 同名：`docs/release-notes/vX.Y.Z.md`。参考上一版 `gh release view v<上一版> --json body --jq .body` 的风格。

再手动编辑版本号（无 pwsh，等价于 `bump-version.ps1 -Part patch`）：

- `Cargo.toml`：`version = "0.5.1"` → `"0.5.2"`。
- `Cargo.lock`：根包 `name = "gongwen-assistant"` 下的 `version` 同步修改（只改这一处）。
- 用 `grep -n '"0\.5\.1"' Cargo.toml` 确认无残留。

```bash
git add Cargo.toml Cargo.lock docs/release-notes/vX.Y.Z.md
git commit -m "chore: release vX.Y.Z"
git push origin main          # 触发 ci.yml
```

### 5. 创建 tag 并推送，触发 release.yml

```bash
git tag -a vX.Y.Z -m "本版本<一句话中文概述，如：生僻字兜底字体落地，研究报告的编号与导航对回纸面。>"
git push origin vX.Y.Z
git ls-remote --tags origin vX.Y.Z   # 确认远程 tag 存在
git rev-parse vX.Y.Z^{commit}        # 必须等于 git rev-parse HEAD
```

**tag 必须指向最终发布提交**：若之后又加了任何提交（fmt 修复、补丁），先删本地+远程 tag，重打后 `git push origin vX.Y.Z`（远程 tag 已删，新 tag 推送不需要 `--force`）。

### 6. 监控 CI/CD

```bash
gh run list --limit 4         # 找到本次 CI（main push）与 Release（vX.Y.Z）两个 run
gh run watch <run-id> --interval 60 --exit-status   # 可后台运行，两路并行
gh run view <run-id> --json status,conclusion,jobs --jq '"\(.status)/\(.conclusion)", (.jobs[] | "  \(.name): \(.conclusion)")'
```

- 期望终态：CI 三平台 job 全 success；Release 的 7 个 job（Build Windows x64 / Build Linux ARM64 / Package Linux ARM64 / Build Linux AMD64 / Package Linux AMD64 / Build macOS ARM64 / Publish GitHub release）全 success。
- `gh run view --json jobs` 里 `Build …` 的 job 名带平台后缀（如 `Build Linux ARM64 / GLIBC 2.28`），按包含关键字筛选：
  `gh run view <id> --json jobs --jq '.jobs[] | select(.name | test("macOS")) | .databaseId'`。
- **CI 失败时**：先 `gh run cancel` 取消所有本次相关 run（CI + Release，避免浪费与旧 release 干扰）→ 本地修复（多数是 fmt 或环境假设类断言）→ 提交 → 移动 tag（步骤 5 的做法）→ 重新 `git push origin main` + `git push origin vX.Y.Z`。

### 7. 校验版本更新说明是否被工作流取用（必须，勿跳过）

说明文件已随源码提交，正常情况下无需手动改正文。要确认工作流真的取到了：

```bash
# 日志里不应出现「缺少 docs/release-notes/...，本次回退到自动生成的说明」这条告警
jid=$(gh run view <release-run-id> --json jobs --jq '.jobs[] | select(.name | test("Publish")) | .databaseId')
gh run view --job "$jid" --log | grep -iE "缺少 docs/release-notes|generate-notes" | head

# 正文与仓库文件逐字对比（GitHub 会在末尾多加一个空行，先剥掉再比）
diff <(gh release view vX.Y.Z --json body --jq .body | sed -e :a -e '/^\n*$/{$d;N;};/\n$/ba') docs/release-notes/vX.Y.Z.md \
  && echo "正文与仓库说明一致"
```

**兜底（只有漏提交说明文件时才需要）**：等占位 release 创建后手写替换，

```bash
gh release edit vX.Y.Z --title "公文助手 vX.Y.Z" --notes-file docs/release-notes/vX.Y.Z.md
```

（幂等可重复执行；更干净的做法是把文件补提交后按步骤 5 移动 tag 重跑，让工作流自己也走一遍取文件的分支。）

### 8. 最终验证

```bash
gh run view <ci-run-id> --json status,conclusion
gh run view <release-run-id> --json status,conclusion
gh api repos/billowsand/gongwen/releases/tags/vX.Y.Z \
  --jq '{name, tag_name, draft, prerelease, html_url}'
gh release list --limit 3                                            # 本版应为 Latest
gh release view vX.Y.Z --json assets --jq '.assets[] | .name'         # 应为 14 个资产
git rev-parse vX.Y.Z^{commit}; git rev-parse HEAD                     # 两者相等
git status -sb                                                       # 与 origin/main 同步、工作区干净
```

- release 必须：`draft=false`、`prerelease=false`、标题「公文助手 vX.Y.Z」、正文为手写说明（与仓库文件一致）、**14 个资产齐全**且各安装包都有对应 `.sha256`。
- 抽查一个校验和文件真的对得上（资产从同一轮 run 上传，抽查即可，不必下载 100MB 的安装包）：

```bash
cd /tmp && mkdir -p kw && cd kw && gh release download vX.Y.Z --pattern 'PKGBUILD*' --dir . --clobber
cat PKGBUILD.sha256 && sha256sum PKGBUILD     # 两个哈希必须一致
cd - && rm -rf /tmp/kw
```

### 9. 收尾汇报

向用户报告：版本号、release 链接（`https://github.com/billowsand/gongwen/releases/tag/vX.Y.Z`）、CI 与 Release 两个 workflow 结果、发布说明要点、资产口径、过程中遇到的问题与处理（尤其是未提交改动如何处置、CI 失败根因）。

## 版本更新说明模板

参考 `docs/release-notes/v0.4.3.md`、`v0.5.2.md` 的正文风格：一句话总述 + 按功能域分节 + 质量检查 + 下载。

```markdown
## 公文助手 vX.Y.Z

本版本<一句话概述主题>。

### <功能域一，如：字体：生僻字兜底>

- **<feat 要点，首句加粗给出结论>**。<补充机制、边界与用户可见后果>
- <fix 要点>

### <功能域二，如：研究报告：编号与导航>

- ...

### 文档

- <README / 手册类改动>

### 质量检查

- 三平台 CI 门禁（格式、clippy、<N> 余项自动化测试）全部通过。
- <本版新增的关键测试与它为什么这么断>

### 下载

- Windows x64 安装程序
- Linux ARM64 Debian 软件包（兼容 GLIBC 2.28）
- Linux AMD64 Debian 软件包（兼容 GLIBC 2.28）
- macOS ARM64 DMG
- Linux ARM64 / AMD64 Omarchy 便携包与 PKGBUILD

各安装包均附带 SHA-256 校验文件。
```

## 常见故障

| 故障 | 处理 |
| --- | --- |
| CI `Check formatting` 失败 | `cargo fmt --all` 修复 → 独立提交 → 移动 tag 重新发布（步骤 3/5/6） |
| CI 在 macOS / Linux 上测试失败、Windows 通过 | 多为「本机装了什么」的环境假设（如字体、系统路径）。用 `--log-failed` 取 `panicked` 那一行确认，把断言收窄到条件成立时才断（而不是删断言或改产品行为），修好随本版提交 |
| main 上 CI 本来就是红的 | 先修红再发：本版 bump 提交会再触发一次 CI，红着发出去的版本「CI 全绿」这句话就不成立 |
| Release run 需重启 | `gh run cancel` 旧 run → 重新 push tag（若 tag 未变，删 tag 重推比 `gh workflow run release.yml` 省事） |
| 发布后又加了提交 | tag 移到新提交：`git tag -d vX.Y.Z && git push origin :refs/tags/vX.Y.Z && git tag -a vX.Y.Z -m "..." <new-commit> && git push origin vX.Y.Z` |
| release 正文是 generate-notes 占位 | 缺 `docs/release-notes/vX.Y.Z.md`。先记下文件内容，`gh release edit vX.Y.Z --title "公文助手 vX.Y.Z" --notes-file …` 补正；同时把文件补提交，下次同 tag 重跑就不会再回退 |
| release 标题/正文不对 | `gh release edit vX.Y.Z --title "..." --notes-file ...`（幂等，可重复执行） |
| 资产数不是 14 | `gh release view vX.Y.Z --json assets --jq '.assets[].name'` 对比上面清单；少 `PKGBUILD*` 看 release job 的「Generate Arch PKGBUILD」步骤，少平台包看对应 build/package job 的日志 |
