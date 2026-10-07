---
name: release
description: 发布 gongwen 新版本到 GitHub Releases：核对 README 与使用帮助文档、写版本更新说明、bump 版本、推送 main、打 annotated tag 触发 CI/CD、监控 workflow 并验证。当用户要求「发布 / 发版 / 出新版本 / 打 tag / release」时使用。
---

# 发布 gongwen 新版本 — Agent Skill Guide

把当前 `main` 上的改动发布为 GitHub 新版本。**硬性要求：release 必须带有手写的版本更新说明，不能只推 tag 或依赖 `--generate-notes` 的占位说明**（用户明确要求）。说明随源码提交在 `docs/release-notes/vX.Y.Z.md`，`release.yml` 发布时直接取用。

**硬性要求二：每次发版都要核对 README 与使用帮助是否跟上了本版功能**（用户明确要求）——功能改了而文档没改，等于发布一个说不清自己的版本。流程见步骤 3，机械检查命令已写在那一节，不要靠印象判断。

## 仓库事实（勿重新探索）

- 远程：`git@github.com:billowsand/gongwen.git`，发布分支 `main`。
- 版本号只维护在 `Cargo.toml`（根包 `version`）+ `Cargo.lock`（根包 `gongwen-assistant` 的 `version`）。**注意 `Cargo.lock` 里另有若干同名版本的依赖**（`block2`、`type-map`、`windows-strings`、`zune-core` 等），只改 `name = "gongwen-assistant"` 那一条。
- 工作流：`ci.yml` 在 push main 时触发（fmt / clippy / test：Windows x64 + Linux ARM64 两平台）；`release.yml` 在 push `v*` tag 时触发，共 5 个 job（build-windows、build-linux-arm64、package-linux-arm64、verify-deb、release）。
- **版本更新说明由工作流取仓库文件**：`release.yml` 的 `release` job 读 `docs/release-notes/${GITHUB_REF_NAME}.md`，有则 `gh release create --title "公文助手 vX.Y.Z" --notes-file <该文件>`；**缺文件才回退 `--generate-notes` 并打一条 `::warning::`**。所以说明必须在打 tag 前提交进仓库，不是发完再补。
- **用户文档有三份，发版时都可能过期**（发版前必查，见步骤 3）：
  - `README.md`（根目录）：面向读者的宣发版，含成堆**会随版本漂移的硬数字**（文种 7 种、导入 24 种格式、校对词表 155 条、文档级规则 21 条、约 21.2 万行 Rust、1852 项自动化测试——括号里是 v0.9.1 的值，**每版都要重新数**，见步骤 3）。打包脚本会把它复制进发布包（`scripts/package-portable.ps1`、`scripts/package-dmg.sh`），所以它是**随包发行**的文档，不是仓库自留物。
  - 使用帮助：应用内嵌正文 `assets/help/*.md`（`include_str!` 打进二进制）＋ 手工副本 `docs/help/*.md`；章节元数据在 `src/help/content.rs` 的 `CHAPTERS`（25 条 = 正文 22 章 + 附录 3 篇）。**改章节要三处同步**，`docs/help/diagrams/` 是可编辑图源，只在 docs 侧。
  - `docs/install.md`（安装指南）与 `docs/manual.md`（手册目录页，分部表格列出全部章号章名）。
- **Release 资产是 4 个**（v0.6.6 之后的口径；macOS、Linux AMD64、Arch/Omarchy 与 PKGBUILD 不再发布）：
  - `gongwen-assistant-X.Y.Z-win-x64-setup.exe` + `.sha256`
  - `gongwen-assistant_X.Y.Z_arm64.deb` + `.sha256`（Debian buster 容器构建，GLIBC 锁死 2.28，兼容 Ubuntu 20.04 / 麒麟 V10）
- 本机无 `pwsh`：不能跑 `scripts/bump-version.ps1`，版本号用手动编辑。
- **提交预分诊（可选加速，用 eval 的 `judge_batch`）**：本机 omp 已接入 TypeSafe
  （judge 模型角色 = `typesafe/jev-latest`），一条 `judge_batch` 调用就能把本区间每条提交的
  类别、所属功能域、是否需要动 README 与使用帮助一次性判完，把「逐条 `git show` 读提交」
  这段重复劳动换成一张表。Jev 是 System One 模型，只出结构化判断、不生成文字，
  所以它替代的是读提交，不是写说明。
  **它不替代任何硬性要求**：版本更新说明仍要手写，README / 使用帮助仍要人工过目。
  没配 TypeSafe 凭据就跳过，流程完全不变。
- 历史惯例：tag 为 **annotated tag**（消息为中文概述）；release 标题为「公文助手 vX.Y.Z」；版本号按 **patch** 递增（`v0.5.x` 系列）；发布前有一个 `chore: release vX.Y.Z` 提交（可同时带上说明文件）。
- Release workflow 全程约 20–30 分钟（Windows + Linux ARM64 构建 + 打包），`gh run watch` 一轮就能等完。

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
- 若 main 最近一次 CI 是 failure：先查失败原因（见步骤 4 与「常见故障」），修好随本版一起发，否则本版 CI 也是红的。

### 2. 确定版本号与发布内容

```bash
git tag --sort=-v:refname | head -3     # 最新 tag，如 v0.5.1
git log v<上次版本>..HEAD --oneline      # 本版本包含的提交
git show --stat --format='%s%n%n%b' <commit>   # 逐个看正文与改动面
```

- 新版本号 = 最新 tag 的 patch + 1（如 `v0.5.1` → `v0.5.2`）；若含大量新功能且用户有暗示，可 bump minor，否则默认 patch。
- 用提交正文（本仓库的 `feat`/`fix` 提交信息写得很详细）提炼用户可见变更，作为版本更新说明素材；`refactor`/`docs` 归入「工程与维护」或「文档」。

### 2.5 提交预分诊（可选，用 judge 加速读提交）

本仓库一个版本常有几十条提交，逐条 `git show` 判断「属于哪类、落哪个功能域、要不要动文档」
是纯重复判断。**用 eval 的 `judge_batch` 把这段交给 Jev**（omp 已接入 TypeSafe，
judge 角色 = `typesafe/jev-latest`）：把每条提交当成一个 state，三个问题一次批量跑完。
**它只做判断、不写文字**，所以替代的是读提交，不是写说明。

关键 API 形状（在 eval 单元里跑，`language="py"`）：

- `commits` 是 `{短 sha: {"subject", "body", "stat"}}`，用一次 `git log` 拿到
  （`--format=%x01%H%x02%s%x02%b%x03` 配 `--shortstat --no-merges` 解析，别逐条调 git）。
- `questions` 三条：`kind`（choice：user_feature / user_fix / internal_refactor / docs /
  test_build / unsure）、`section`（choice：排版导出 / AI 智能体 / 数据接口 / 稿件库与版本 /
  送批材料 / 输入法 / 校对词表 / 平台打包 / other）、`docs`（bool：要不要动 README 与使用帮助）。
  **criteria 要写足**，官方记录 Jev 偏「字面理解」，判据含糊它就跟着含糊。
- `b = judge_batch(commits, QUESTIONS)`，然后 **`await b.drain(timeout=30)` 循环取**
  （`drain` 是协程，得 await；不 await 会静默不消费）。取完 `b.status()` / `b.results()` /
  `b.failed()` 都在 **`b.close()` 之前**读——close 之后作业引用失效，再读会抛
  `unknown judge_batch`。judge_batch 是宿主托管的异步作业，会话重置后可用
  `judge_batch.attach(b.id)` 重新拿回。
- **不要写成 `for` 循环里逐条 `judge()`**——两个以上 state 一律 `judge_batch`，
  循环单发既慢又贵。
- `chore: release vX.Y.Z`、`docs: 同步 README`、`style:` 这类自身不带用户可见变更的提交
  可以先从 state 里剔掉：分诊标出的「需人工确认」里混着它们会虚高，直接跳过更省事。

- **实测（v0.8.1..HEAD，57 条提交、171 个问题）**：57/57 成功，`typesafe/jev-1.13.0`，
  **约 1.1–1.8 秒，$0.0028**，0 失败。对照人工逐条读 57 条提交，是数量级上的差距。
- **置信度分流是安全阀，不要跳过**：Choice 的 `confidence` 低于 0.60、
  bool 概率落在 0.35–0.65 的含糊区间、或模型自认 `unsure` 的提交，一律交回人工。
  实测 57 条里会标出 20 条上下，占比不低——**这说明它是筛选器不是判官**。
- 仓库提交与说明都是中文，Jev 的训练以英文为主、中文能用但精度不等同：
  实测把「数据接口识别业务失败」判成 AI 智能体、把一批 UI 改动归到「其他」。
  **照它的「要改文档」清单防漏，别照它的功能域归类照抄**，写说明前人工重排。
- **凭据**：没配 TypeSafe 时 `judge` 会退到 tiny/smol 对话模型，答案不可信。
  开跑前先确认 `b.status()["model"]` 是 `typesafe/*`，不是就别用这份结果。
  凭据用 `/login typesafe` 或环境变量 `TYPESAFE_API_KEY`，**不要写进仓库任何文件**。
- **没凭据就照原流程人工读提交，不要卡住发版。**

### 3. 文档同步检查（README 与使用帮助，最容易跳过）

**必须在写版本更新说明、bump 之前做完**，改完的文档要进本版提交集，并在说明的「### 文档」小节里交代。已发生过 README 里硬数字随版本漂移、功能上线而手册没跟上的情况，所以这一步不许省。

先看本版改了什么，逐条问「文档里有没有对应的说法，还是旧说法」：

```bash
git log v<上次版本>..HEAD --oneline          # 本版提交
git show --stat --format='%s' <commit>       # 逐个看改到了哪些界面 / 流程 / 格式
```

若步骤 2.5 跑过分诊，先拿它的「文档 = 要改」清单当检查表：那条提交对应到
README 哪一节、手册哪一章、要不要重截截图。它能保证不漏，但不能保证说得准，
每一条仍要自己看一眼；标「需人工确认」的优先看。分诊没跑或没有 TypeSafe 凭据就按上面的
`git log` 逐条来，本节的检查要求与分诊无关。

三道机械检查（能自动发现的不靠人眼）：

```bash
# ① 手册两处副本必须逐字一致（docs/help/diagrams 是可编辑图源，只在 docs 侧，故排除）
diff -rq assets/help docs/help --exclude=diagrams && echo "手册副本一致"

# ② 章节口径：CHAPTERS 25 条 = 正文 22 章 + 附录 3 篇，
#    与 README「22 章 + 3 附录」、docs/manual.md 的分部表格一致
#    （v0.7.0 增补「22 送批材料」一章后即为 25/22/3；若将来又加章，三处同步改这里）
grep -c '^ *chapter!' src/help/content.rs     # 25
ls assets/help/[0-9]*.md | wc -l              # 22
ls assets/help/[a-c]-*.md | wc -l             # 3

# ③ README「文种、导出与平台」表的硬数字，逐条重新数一遍
#    （下面是 v0.9.1 的实际值，仅作对照——每版都要重新数，别直接抄）
grep -v '^#' proofread-lexicon.tsv | tail -n +2 | wc -l   # 校对词表条数（v0.9.1 时 155）
find src -name '*.rs' | xargs wc -l | tail -1              # 代码行数（v0.9.1 时约 21.2 万行）
# 测试数：复用步骤 4 那次 cargo test 的输出汇总（v0.9.1 时 1852）
cargo test --locked --all-targets --no-fail-fast 2>&1 | grep -oE '[0-9]+ passed' | awk '{s+=$1} END{print s}'
```

人眼要过的一遍：

- **新功能**：本版新增的界面、面板、快捷键、导出格式，在 `assets/help/` 里有没有对应章节或段落？没有就补；补完同步 `docs/help/` 副本（用上面 ① 验）。若新增**一整章**，还要在 `src/help/content.rs` 的 `CHAPTERS` 里加一条、把配图同时放进 `assets/help/images/` 与 `docs/help/images/`。
- **改动的功能**：README 的「为什么是公文助手」「核稿保障」「排版」「开箱即用」「文稿管理」「AI」「文种、导出与平台」各节，以及手册对应章节的描述、示例、截图，是否还是现在这个样子。
- **界面截图**：界面有可见变化时，`docs/images/readme/`（README 用）与 `docs/help/images/`（手册用）里的过期截图要重截，别挂着旧界面发版。
- **版本相关表述**：`docs/install.md` 的平台 / 产物清单、README 末尾的 CI 与发布说明，是否仍与 `release.yml` 的实际行为一致。

有改动就提交（可单独 `docs:` 提交，也可并入本版 `chore: release vX.Y.Z`）：

```bash
git add README.md docs assets/help src/help/content.rs
git commit -m "docs: 同步 README 与使用帮助至 vX.Y.Z"
```

确认无需改动时，在收尾汇报里明确写一句「README 与使用帮助已核对，本版无需更新」，别默默跳过。

### 4. 本地质量门禁（关键，勿跳过）

CI 有三道门，历史上因未跑 `cargo fmt` 导致发布返工，也出现过**测试在 CI 的 Linux 上失败而本地 Windows 通过**（环境假设类断言）。**发布前必须跑**：

```bash
cargo fmt --all -- --check                                    # 必须无输出（exit 0）
cargo clippy --locked --all-targets -- -D warnings             # 必须干净
cargo test --locked --all-targets --no-fail-fast               # 看失败名单
```

- 格式不通过：`cargo fmt --all` 修复 → 单独提交 `style: cargo fmt 统一代码格式`（改变发布提交集，见步骤 6 的 tag 移动）。
- 测试失败要**分清是环境抖动还是真失败**：
  - 已知沙箱抖动：`highlight::tests::swapping_light_and_dark_relayouts_against_the_rebuilt_font_atlas`（字体图集重建，首跑偶发失败、重跑即过），以及 AGENTS.md 记录的约 5 个沙箱相关失败。**不要为环境相关失败改测试。**
  - 若断言依赖「本机装了什么」（字体、代理、路径），要问的是「CI 上成不成立」，多为 CI 独有失败，按下面处理。
- 平台差异的排查入口：

```bash
gh run view <run-id> --log-failed | awk -F'\t' '{print $3}' | grep -E "panicked|test result:|##\[error\]"
gh run view --job <job-id> --log | grep -E "<测试名>|test result:"   # 确认某用例在特定平台的结果
```

### 5. 写版本更新说明并 bump 版本

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

### 6. 创建 tag 并推送，触发 release.yml

```bash
git tag -a vX.Y.Z -m "本版本<一句话中文概述，如：生僻字兜底字体落地，研究报告的编号与导航对回纸面。>"
git push origin vX.Y.Z
git ls-remote --tags origin vX.Y.Z   # 确认远程 tag 存在
git rev-parse vX.Y.Z^{commit}        # 必须等于 git rev-parse HEAD
```

**tag 必须指向最终发布提交**：若之后又加了任何提交（fmt 修复、补丁），先删本地+远程 tag，重打后 `git push origin vX.Y.Z`（远程 tag 已删，新 tag 推送不需要 `--force`）。

### 7. 监控 CI/CD

```bash
gh run list --limit 4         # 找到本次 CI（main push）与 Release（vX.Y.Z）两个 run
gh run watch <run-id> --interval 60 --exit-status   # 可后台运行，两路并行
gh run view <run-id> --json status,conclusion,jobs --jq '"\(.status)/\(.conclusion)", (.jobs[] | "  \(.name): \(.conclusion)")'
```

- 期望终态：CI 全部 job success；Release 的 5 个 job（Build Windows x64 / Build Linux ARM64 / Package Linux ARM64 / Verify deb install / Publish GitHub release）全 success。
- `gh run view --json jobs` 里 `Build …` 的 job 名带平台后缀（如 `Build Linux ARM64 / GLIBC 2.28`），按包含关键字筛选：
  `gh run view <id> --json jobs --jq '.jobs[] | select(.name | test("ARM64")) | .databaseId'`。
- **CI 失败时**：先 `gh run cancel` 取消所有本次相关 run（CI + Release，避免浪费与旧 release 干扰）→ 本地修复（多数是 fmt 或环境假设类断言）→ 提交 → 移动 tag（步骤 6 的做法）→ 重新 `git push origin main` + `git push origin vX.Y.Z`。

### 8. 校验版本更新说明是否被工作流取用（必须，勿跳过）

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

（幂等可重复执行；更干净的做法是把文件补提交后按步骤 6 移动 tag 重跑，让工作流自己也走一遍取文件的分支。）

### 9. 最终验证

```bash
gh run view <ci-run-id> --json status,conclusion
gh run view <release-run-id> --json status,conclusion
gh api repos/billowsand/gongwen/releases/tags/vX.Y.Z \
  --jq '{name, tag_name, draft, prerelease, html_url}'
gh release list --limit 3                                            # 本版应为 Latest
gh release view vX.Y.Z --json assets --jq '.assets[] | .name'         # 应为 4 个资产
git rev-parse vX.Y.Z^{commit}; git rev-parse HEAD                     # 两者相等
git status -sb                                                       # 与 origin/main 同步、工作区干净
```

- release 必须：`draft=false`、`prerelease=false`、标题「公文助手 vX.Y.Z」、正文为手写说明（与仓库文件一致）、**4 个资产齐全**且各安装包都有对应 `.sha256`。
- 抽查一个校验和文件真的对得上（资产从同一轮 run 上传，抽查 deb 一个即可）：

```bash
cd /tmp && mkdir -p kw && cd kw && gh release download vX.Y.Z --pattern '*_arm64.deb*' --dir . --clobber
sha256sum -c gongwen-assistant_X.Y.Z_arm64.deb.sha256     # 必须 OK
cd - && rm -rf /tmp/kw
```

### 10. 收尾汇报

向用户报告：版本号、release 链接（`https://github.com/billowsand/gongwen/releases/tag/vX.Y.Z`）、CI 与 Release 两个 workflow 结果、发布说明要点、资产口径、过程中遇到的问题与处理（尤其是未提交改动如何处置、CI 失败根因）、**文档核对结论**（README / 使用帮助 / install.md 是本版更新了哪几处，还是确认无需改动）。跑过分诊的话，附一句它标出多少条「需人工确认」以及这些是怎么处理的——别让分诊悄悄决定文档核对结论。

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

- <README / 使用帮助 / 安装指南的改动，来自步骤 3 的核对结果；确实没有用户可见的文档改动就省略本节>

### 质量检查

- 两平台 CI 门禁（格式、clippy、<N> 余项自动化测试）全部通过。
- <本版新增的关键测试与它为什么这么断>

### 下载

- Windows x64 安装程序
- Linux ARM64 Debian 软件包（兼容 GLIBC 2.28 / Ubuntu 20.04）

各安装包均附带 SHA-256 校验文件。
```

## 常见故障

| 故障 | 处理 |
| --- | --- |
| CI `Check formatting` 失败 | `cargo fmt --all` 修复 → 独立提交 → 移动 tag 重新发布（步骤 4/6/7） |
| CI 在 Linux 上测试失败、Windows 通过 | 多为「本机装了什么」的环境假设（如字体、系统路径）。用 `--log-failed` 取 `panicked` 那一行确认，把断言收窄到条件成立时才断（而不是删断言或改产品行为），修好随本版提交 |
| main 上 CI 本来就是红的 | 先修红再发：本版 bump 提交会再触发一次 CI，红着发出去的版本「CI 全绿」这句话就不成立 |
| Release run 需重启 | `gh run cancel` 旧 run → 重新 push tag（若 tag 未变，删 tag 重推比 `gh workflow run release.yml` 省事） |
| 发布后又加了提交 | tag 移到新提交：`git tag -d vX.Y.Z && git push origin :refs/tags/vX.Y.Z && git tag -a vX.Y.Z -m "..." <new-commit> && git push origin vX.Y.Z` |
| release 正文是 generate-notes 占位 | 缺 `docs/release-notes/vX.Y.Z.md`。先记下文件内容，`gh release edit vX.Y.Z --title "公文助手 vX.Y.Z" --notes-file …` 补正；同时把文件补提交，下次同 tag 重跑就不会再回退 |
| release 标题/正文不对 | `gh release edit vX.Y.Z --title "..." --notes-file ...`（幂等，可重复执行） |
| 发版后才发现 README / 使用帮助没跟上 | 别指望下版补：优先按步骤 6 移动 tag 重跑（代价是两平台构建再来一轮）；若只是文字补充、不值得重跑，就单独提 `docs:` 提交并用 `gh release edit vX.Y.Z --notes-file …` 把文档改动补进正文，**同时把步骤 3 的检查固化到下一次发版** |
| 资产数不是 4 | `gh release view vX.Y.Z --json assets --jq '.assets[].name'` 对比上面清单；少平台包看对应 build/package job 的日志 |
| `judge_batch` 的 `drain()` 取不到东西 | `drain` 是协程，必须 `await`；不 await 会静默不消费、循环立刻空转退出 |
| 报 `unknown judge_batch "<id>"` | `close()` 之后作业引用失效，状态 / 结果 / 失败项必须在 close 前读完；真丢了结果用 `judge_batch.attach(<id>)` 拿回 |
| 分诊结果模型不是 `typesafe/*` | 没配 TypeSafe 凭据时 judge 会退到 tiny/smol 对话模型，判定不可信。`/login typesafe` 或设 `TYPESAFE_API_KEY`，或直接跳过分诊走人工 |
| 分诊结论明显不对（功能域张冠李戴） | 属正常误差：Jev 只出判断不写字、中文精度不等同。照它的「要改文档」清单防漏，别照功能域归类照抄，写说明前人工重排 |
