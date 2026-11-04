# 公文助手 · macOS 打包与签名

本目录是 macOS 打包的辅助资产：`entitlements.plist` 给 hardened runtime 用；
本文档说明如何在本机或 CI 上把「二进制 + 字体 runtime + Pi 运行时」组装成可安装的
`.app` / `.dmg`，以及签名、公证、验收与已知限制。

> 状态：macOS 产物目前只做 **arm64**（`macos-14` / Apple Silicon）。x64 的 Intel Mac
> 可以按本文手动出包（见「arm64 / x64 双包策略」），CI 与 release 暂不覆盖。
> 应用内导入 `.gwx` 走拖放 / 应用内导入，**不支持双击 .gwx 打开应用**（见下文）。

## 1. 前置条件

- macOS 12+（打包机建议 macOS 14）。
- Xcode Command Line Tools：`xcode-select --install`，提供 `codesign`、`xcrun`、
  `hdiutil`、`ditto`、`sips`、`iconutil`、`shasum`。
- Rust stable（`rustup`），仓库依赖已 `cargo` 拉齐。
- `runtime/`：中文字体等运行时资产来自独立的 `gongwen-runtime` 仓库的 `runtime.zip`
  （CI 里 `RUNTIME_RELEASE_TAG` 下载后解到 `runtime/`）。打包脚本要求其中有
  `runtime/fonts/GWFangSong.ttf`，否则 PDF 排版不可用、直接失败。
- 不需要系统安装 Node / npm：随包 Node 由 `scripts/stage-pi-runtime.sh` 下载并校验，
  `npm ci` 用的是随包 Node 自带的 npm。

## 2. 组装步骤

在仓库根目录执行：

```bash
# 0) 取字体 runtime（发布流水线这一步是下载 runtime.zip 解压到 runtime/）
#    本机已有 runtime/ 就跳过。

# 1) 组装随包 Pi 运行时（Node 22.22.3 + host + 生产依赖）
#    Apple Silicon → darwin-arm64；Intel → darwin-x64。脚本自动按 uname 推导，
#    但显式传平台更稳（CI 也显式传）。
bash scripts/stage-pi-runtime.sh --platform darwin-arm64 --force
# Intel：bash scripts/stage-pi-runtime.sh --platform darwin-x64 --force

# 2) 构建 release 二进制
cargo build --release --locked

# 3) 打成 .app + .dmg（--require-pi：没有 Pi 运行时直接失败，绝不发出残缺包）
bash scripts/package-dmg.sh --skip-build --require-pi --output dist/macos
```

产物：

```
dist/macos/gongwen-assistant-<version>-macos-arm64.dmg
dist/macos/gongwen-assistant-<version>-macos-arm64.dmg.sha256
```

`package-macos.sh` 是只组装 `.app`（不出 DMG）的旧脚本，需要在本地查看界面时用：

```bash
bash scripts/package-macos.sh --skip-build --output dist/macos
open dist/macos/GongwenAssistant.app
```

它同样会把 `runtime/pi` 复制进 `Contents/MacOS/runtime/pi` 并校验、`chmod 755`。

### 运行时放进 bundle 的位置

应用按「可执行文件所在目录 / runtime / pi」定位 Pi 运行时（`src/pi_agent/runtime.rs`
用 `current_exe().parent()`），在 `.app` 里就是：

```
GongwenAssistant.app/Contents/MacOS/gongwen-assistant
GongwenAssistant.app/Contents/MacOS/runtime/pi/bin/node
GongwenAssistant.app/Contents/MacOS/runtime/pi/host/main.mjs
GongwenAssistant.app/Contents/MacOS/runtime/pi/manifest.json
GongwenAssistant.app/Contents/MacOS/runtime/fonts/...
```

`package-dmg.sh` / `package-macos.sh` 在复制前会调用
`node scripts/pi-runtime-verify.mjs <dir> darwin-<arch>` 校验 manifest 平台、Node /
host / 协议版本与全部文件哈希，并检查 `bin/node` 的可执行位；平台不符或哈希不符
直接失败。省略 `--require-pi` 时缺运行时会打印明确警告「未包含 Pi 运行时，智能写作
将不可用」但仍出包（只适合界面预览，不要拿去发布）。

## 3. 签名与公证（可选，发布必备）

未配置签名身份时脚本做 ad-hoc 签名（`codesign --force --deep --sign -`），只能本机
测试：换一台机器下载 DMG 会被 Gatekeeper 拦。正式发布需要 Apple Developer Program
账号（Team ID）、一张 **Developer ID Application** 证书，以及一个 notarytool
keychain profile。

### 3.1 证书 / keychain profile

1. 在 Apple Developer 后台创建 **Developer ID Application** 证书，导入打包机的
   `login` keychain（`security find-identity -v -p codesigning` 能看到它）。
2. 用 App 专用密码创建公证 profile（只需在打包机做一次）：

```bash
xcrun notarytool store-credentials gongwen-notary \
  --apple-id "you@example.com" \
  --team-id "ABCDE12345" \
  --password "app-specific-password"
```

3. 设置环境变量后重新打包：`package-dmg.sh` 检测到 `GONGWEN_MACOS_SIGN_IDENTITY`
   非空时按「先签 bundle 内的 node，再签整个 .app」的顺序做 hardened runtime 签名
   （`--options runtime --timestamp --entitlements packaging/macos/entitlements.plist`），
   不使用 `--deep` 掩盖嵌套代码；检测到 `GONGWEN_MACOS_NOTARY_PROFILE` 非空时，
   生成 DMG 后跑 `xcrun notarytool submit ... --wait`，成功再 `xcrun stapler staple`。
   公证失败脚本非零退出，但保留已生成的 DMG 便于查日志。

```bash
export GONGWEN_MACOS_SIGN_IDENTITY="Developer ID Application: Your Name (ABCDE12345)"
export GONGWEN_MACOS_NOTARY_PROFILE="gongwen-notary"
bash scripts/package-dmg.sh --skip-build --require-pi
```

entitlements（`packaging/macos/entitlements.plist`）固定三项：
`com.apple.security.cs.allow-jit`、`allow-unsigned-executable-memory`（Node/V8 的 JIT
必需）与 `disable-library-validation`（bundle 内 node_modules 的 dylib 与主程序不是
同一签名，需要关掉库验证）。移除任何一项都会让随包 Node 起不来。

### 3.2 检查命令

```bash
APP="dist/macos/GongwenAssistant.app"      # 或挂载 DMG 后的 .app
DMG="dist/macos/gongwen-assistant-<version>-macos-arm64.dmg"

# 签名信息：TeamIdentifier、Authority、Runtime 版本、entitlements
codesign -dv --verbose=4 "$APP"
codesign -dv --verbose=4 "$APP/Contents/MacOS/runtime/pi/bin/node"
codesign -d --entitlements :- "$APP/Contents/MacOS/runtime/pi/bin/node"

# 校验签名完整性与 Gatekeeper 判定
codesign --verify --deep --strict --verbose=2 "$APP"
spctl -a -vv "$APP"
spctl -a -t open --context context:primary-signature -v "$DMG"

# DMG 与公证票据
hdiutil verify "$DMG"
xcrun stapler validate "$DMG"
xcrun notarytool log <submission-id> --keychain-profile gongwen-notary   # 公证失败时看原因
```

也可以装到 `/Applications` 后真机跑一次：打开应用 → 设置里看 Pi 是否可用（不是
「运行时未安装」）→ 用真实模型起草一小段 → 导出 PDF。

验证 Pi 运行时本身（不进 bundle）：

```bash
./runtime/pi/bin/node scripts/pi-runtime-verify.mjs runtime/pi darwin-arm64
./runtime/pi/bin/node scripts/pi-offline-acceptance.mjs --runtime ./runtime/pi
```

离线验收只用 127.0.0.1 上的假模型服务，不访问公网；它只能证明「运行时装配正确、
无公网访问」，不能替代干净目标机上的真实安装验收。

## 4. `.gwx` 双击：不支持

`Info.plist` **故意没有** `.gwx` 的 `CFBundleDocumentTypes` /
`UTExportedTypeDeclarations`。原因：macOS 双击文件走 LaunchServices 的 Apple Event
`open-file`，而当前 winit / eframe 外壳没有处理这个事件（只读 argv 里的路径），
注册了 UTI 只会出现「应用被拉起但没有导入任何东西」的假成功。所以：

- 支持：把 `.gwx` **拖到应用窗口/图标上**，或用应用内「导入」；
- 不支持：Finder 里双击 `.gwx` 打开应用。

要做双击支持，需要先接 `open-file` Apple Event 并复用现有导入入口，再在
`Info.plist` 补 UTI——这一轮没有做，不要在 release notes 里声称支持。

## 5. 自动更新与卸载残留

- 当前**没有自动更新**：升级方式是下载新 DMG → 把 `.app` 拖进「应用程序」覆盖。
  `runtime/` 随 bundle 走，不需要单独更新。
- 用户数据在 `~/Library/Application Support/cn.LocalTools.GongwenAssistant/`
  （`directories` crate 的 qualifier/organization/application 组合），里面有配置、
  稿件库、词表，以及 Pi 的 `pi/workspaces/` 工作区。卸载（删掉 `.app`）不会清理它；
  需要彻底清干净时手动删除该目录。系统可能另建
  `~/Library/Saved Application State/cn.localtools.GongwenAssistant.savedState`，
  一并删除即可。
- 未签名 / 未公证的本地测试包首次启动可能被拦：右键「打开」，或
  `xattr -dr com.apple.quarantine /Applications/GongwenAssistant.app`。

## 6. arm64 / x64 双包策略

不做 universal binary：随包 Node 是单架构官方二进制，一个 universal 包里塞两份
`runtime/pi` 需要改运行时定位逻辑（按 `cfg!(target_arch)` 选目录），这一轮不引入。
现状是「一个架构一个包」：

| 架构 | 平台名 | 状态 |
| --- | --- | --- |
| Apple Silicon | `darwin-arm64` | CI（`macos-14`）与 release 产出 DMG |
| Intel | `darwin-x64` | 可在 Intel Mac 上按本文手动出包，CI 未覆盖 |

`package-dmg.sh` 按 `uname -m` 自动取 `darwin-arm64` / `darwin-x64`；DMG 文件名保持
`gongwen-assistant-<version>-macos-<arch>.dmg`（`arch` 是 `uname -m` 的值）。

## 7. CI secrets

`.github/workflows/release.yml` 的 `build-macos-arm64` 作业从仓库 secrets 注入：

| Secret | 用途 | 为空时 |
| --- | --- | --- |
| `MACOS_SIGN_IDENTITY` | `GONGWEN_MACOS_SIGN_IDENTITY`，Developer ID Application 证书的完整名称 | 走 ad-hoc 签名，不公证，仅供本机测试 |
| `MACOS_NOTARY_PROFILE` | `GONGWEN_MACOS_NOTARY_PROFILE`，notarytool keychain profile 名 | 跳过公证 |

注意：GitHub 的 `macos-14` runner 是临时机器，**证书本身不会自己出现**。要真正签名，
还需要在作业里从 secrets 导入 `.p12` 到临时 keychain（以及写入 notary profile）；
当前 workflow 只注入上面两个变量、没有证书导入步骤——配置 secrets 之前请先补导入
步骤，或在证书已预装的 self-hosted runner 上跑。CI 产物在未配置 secrets 时是未签名 /
未公证的 DMG，不要直接发给用户。
