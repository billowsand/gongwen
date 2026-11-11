#!/usr/bin/env bash
# 一键打包 macOS DMG：构建二进制 → 按 runtime 清单组装便携布局 → 校验并放入 Pi
# 运行时 → .app bundle → 签名（可选 Developer ID + 公证）→ 带「应用程序」拖放链接的 DMG。
#
# 用法：
#   scripts/package-dmg.sh [选项]                                 # 一键完整打包
#   scripts/package-dmg.sh <version> <staging-dir> <output-dir>   # 旧接口：只把
#       现成便携目录打成 DMG（staging 由 scripts/package-portable.ps1
#       -ArchiveFormat none 生成，含 gongwen-assistant、runtime/ 与 README/LICENSE）
#
# 选项：
#   --version X.Y.Z   版本号；缺省从 Cargo.toml 读取。
#   --output DIR      产物目录；缺省 <仓库>/dist/macos。
#   --staging DIR     复用现成便携目录，跳过构建与组装。
#   --skip-build      不重新 cargo build，直接用 target/release/gongwen-assistant。
#   --require-pi      必须包含 Pi 运行时（runtime/pi），缺失就失败；发布流水线用。
#   -h, --help        显示帮助。
#
# 产物：dist/macos/gongwen-assistant-<版本>-macos-<架构>.dmg（附 .sha256）。
#
# 签名 / 公证（可选，发布必备）：
#   GONGWEN_MACOS_SIGN_IDENTITY    Developer ID Application 证书名。非空时先单独签
#       Contents/MacOS/runtime/pi/bin/node，再签整个 .app，带 hardened runtime 与
#       packaging/macos/entitlements.plist；为空时只做 ad-hoc 签名（仅本机测试）。
#       ！注意！没有 identity 时不要用 --deep 去「掩盖」未签名；ad-hoc 产出的包
#       换机器会被 Gatekeeper 拦。
#   GONGWEN_MACOS_NOTARY_PROFILE   notarytool keychain profile。非空时在 DMG 生成后
#       submit --wait 并 staple；失败非零退出但保留 DMG 便于查日志。
#
# 一键模式的 staging 组装与 scripts/package-portable.ps1 保持一致：
#   - 校验 runtime/SHA256SUMS.<平台>.txt 里每个资产的 SHA-256；
#   - 资产放进可执行文件旁的 runtime/（程序按它查找，见 src/portable_runtime.rs），
#     旧清单里的 Tectonic / TeX bundle 不随包；
#   - Pi 运行时（runtime/pi）先按 darwin-* manifest 全量校验，再复制进
#     staging/runtime/pi；没有时默认警告（智能写作不可用），--require-pi 则失败；
#   - 附 README / THIRD_PARTY_NOTICES / LICENSE / config.example.json；
#   - 把 skills/gongwen-markdown/ 打成 skills/gongwen-markdown.skill。
#
# Info.plist 故意不注册 .gwx 的 CFBundleDocumentTypes / UTI：应用还没有处理 macOS
# 的 open-file Apple Event，注册后双击只会「打开应用但不导入」，反而更糟。.gwx 用
# 拖放或应用内导入；见 packaging/macos/README.md。
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
BUNDLE_ID="com.billowsand.gongwen"
APP_NAME="公文助手"
BUNDLE_NAME="GongwenAssistant.app"
ENTITLEMENTS="$PROJECT_ROOT/packaging/macos/entitlements.plist"

die() {
    echo "error: $*" >&2
    exit 1
}

usage() {
    awk 'NR > 1 { if ($0 !~ /^#/) exit; sub(/^# ?/, ""); print }' "${BASH_SOURCE[0]}"
}

VERSION=""
STAGING=""
OUTPUT_DIR=""
SKIP_BUILD=0
REQUIRE_PI=0

# 旧三段式接口：<version> <staging-dir> <output-dir>（首参数不以 - 开头）。
if [ $# -eq 3 ] && [ "${1#-}" = "$1" ]; then
    VERSION="$1"
    STAGING="$2"
    OUTPUT_DIR="$3"
else
    while [ $# -gt 0 ]; do
        case "$1" in
            --version)
                [ $# -ge 2 ] || die "--version 缺少参数"
                VERSION="$2"
                shift 2
                ;;
            --output)
                [ $# -ge 2 ] || die "--output 缺少参数"
                OUTPUT_DIR="$2"
                shift 2
                ;;
            --staging)
                [ $# -ge 2 ] || die "--staging 缺少参数"
                STAGING="$2"
                shift 2
                ;;
            --skip-build)
                SKIP_BUILD=1
                shift
                ;;
            --require-pi)
                REQUIRE_PI=1
                shift
                ;;
            -h | --help)
                usage
                exit 0
                ;;
            *)
                die "未知选项：$1（旧接口需三个参数：<version> <staging-dir> <output-dir>）"
                ;;
        esac
    done
fi

case "$(uname -s)" in
    Darwin) ;;
    *) die "package-dmg.sh 只能在 macOS 上运行（需要 hdiutil / codesign）" ;;
esac

# Pi 运行时的平台名与 packaging/pi-runtime.json 的 platforms 键一致；macOS 用
# .tar.gz 归档，node 在 bin/node（与 Linux 相同）。
case "$(uname -m)" in
    arm64|aarch64) PI_PLATFORM="darwin-arm64" ;;
    x86_64|amd64) PI_PLATFORM="darwin-x64" ;;
    *) die "不支持的 macOS 架构：$(uname -m)（只有 darwin-arm64 / darwin-x64）" ;;
esac

# 校验一份组装好的 Pi 运行时：manifest 平台 / 哈希 / 可执行位；失败直接中止。
verify_pi_runtime() {
    local dir="$1"
    [ -f "$dir/manifest.json" ] || die "Pi 运行时缺 manifest：$dir/manifest.json"
    [ -f "$dir/bin/node" ] || die "Pi 运行时缺 node：$dir/bin/node"
    [ -x "$dir/bin/node" ] || die "Pi 运行时 node 没有可执行位：$dir/bin/node（chmod 755 后重试）"
    "$dir/bin/node" "$PROJECT_ROOT/scripts/pi-runtime-verify.mjs" "$dir" "$PI_PLATFORM" ||
        die "Pi 运行时校验失败（要求平台 ${PI_PLATFORM}）：$dir"
}

# 版本号只维护在 Cargo.toml，与 bump-version.ps1 保持一致。
if [ -z "$VERSION" ]; then
    VERSION="$(
        grep -m1 '^version[[:space:]]*=' "$PROJECT_ROOT/Cargo.toml" |
            sed -E 's/.*"([0-9]+\.[0-9]+\.[0-9]+)".*/\1/'
    )"
fi
case "$VERSION" in
    [0-9]*\.[0-9]*\.[0-9]*) ;;
    *) die "无法确定 x.y.z 版本号（得到：${VERSION}）" ;;
esac

ARCH="$(uname -m)"
case "$ARCH" in
    arm64) SUFFIX="darwin-arm64" ;;
    x86_64) SUFFIX="darwin-amd64" ;;
    *) SUFFIX="darwin-$ARCH" ;;
esac

ICNS_FILE="$PROJECT_ROOT/assets/app-icon/app-icon.icns"
if [ ! -f "$ICNS_FILE" ]; then
    die "应用图标不存在：$ICNS_FILE（由 scripts/generate-brand-icons.py 生成）"
fi

BUILD_ROOT="$(mktemp -d)"
trap 'rm -rf "$BUILD_ROOT"' EXIT

STAGE="$BUILD_ROOT/staging"
if [ -n "$STAGING" ]; then
    STAGE="$(cd "$STAGING" && pwd -P)"
else
    mkdir -p "$STAGE"

    if [ "$SKIP_BUILD" -eq 0 ]; then
        echo "==> cargo build --release --locked"
        (cd "$PROJECT_ROOT" && cargo build --release --locked)
    fi
    BIN="$PROJECT_ROOT/target/release/gongwen-assistant"
    [ -f "$BIN" ] || die "找不到二进制：${BIN}（去掉 --skip-build 或先构建）"
    cp "$BIN" "$STAGE/gongwen-assistant"

    # 按 runtime 清单校验并组装；缺平台清单时回退 SHA256SUMS.txt（与
    # package-portable.ps1 的回退一致）。
    MANIFEST="$PROJECT_ROOT/runtime/SHA256SUMS.$SUFFIX.txt"
    if [ ! -f "$MANIFEST" ]; then
        MANIFEST="$PROJECT_ROOT/runtime/SHA256SUMS.txt"
    fi
    [ -f "$MANIFEST" ] || die "缺少 runtime 校验清单：runtime/SHA256SUMS.$SUFFIX.txt"
    echo "==> 校验并组装 runtime（$(basename "$MANIFEST")）"
    while IFS= read -r line || [ -n "$line" ]; do
        case "$line" in '' | \#*) continue ;; esac
        read -r sum rel <<<"$line"
        src="$PROJECT_ROOT/runtime/$rel"
        [ -f "$src" ] || die "runtime 清单中的资产不存在：$src"
        actual="$(shasum -a 256 "$src" | awk '{print toupper($1)}')"
        expected="$(printf '%s' "$sum" | tr '[:lower:]' '[:upper:]')"
        [ "$actual" = "$expected" ] || die "SHA-256 不匹配：${src}（期望 ${expected}，实际 ${actual}）"
        case "$rel" in
            ime/base.txt) dest="runtime/$rel" ;;
            # PDF 由 Typst 在进程内排版，旧 runtime 清单里的 TeX 资产不随包。
            ime/* | tectonic/* | texbundle/* | licenses/LICENSE.CTAN | licenses/LICENSE.TL | licenses/TECTONIC-LICENSE.txt) continue ;;
            *) dest="runtime/$rel" ;;
        esac
        mkdir -p "$STAGE/$(dirname "$dest")"
        cp "$src" "$STAGE/$dest"
    done <"$MANIFEST"
    [ -f "$STAGE/runtime/fonts/GWFangSong.ttf" ] ||
        die "runtime 清单未提供字体，PDF 排版将不可用（${SUFFIX}）"

    # Pi 运行时：与字体 runtime 分开组装（SHA256SUMS 里没有它），复制进 staging。
    # 缺 manifest 说明没跑 stage-pi-runtime.sh；这一轮默认只警告，--require-pi 必须失败。
    PI_SOURCE="$PROJECT_ROOT/runtime/pi"
    if [ -f "$PI_SOURCE/manifest.json" ]; then
        verify_pi_runtime "$PI_SOURCE"
        rm -rf "$STAGE/runtime/pi"
        cp -a "$PI_SOURCE" "$STAGE/runtime/pi"
        chmod 755 "$STAGE/runtime/pi/bin/node"
    elif [ "$REQUIRE_PI" -eq 1 ]; then
        die "没有 Pi 运行时（${PI_SOURCE}/manifest.json）。先运行 scripts/stage-pi-runtime.sh --platform ${PI_PLATFORM} --force，或去掉 --require-pi"
    else
        echo "警告：未包含 Pi 运行时，智能写作将不可用（缺 ${PI_SOURCE}/manifest.json）" >&2
    fi

    for doc in README.md THIRD_PARTY_NOTICES.md LICENSE config.example.json; do
        [ -f "$PROJECT_ROOT/$doc" ] && cp "$PROJECT_ROOT/$doc" "$STAGE/"
    done

    SKILL_SRC="$PROJECT_ROOT/skills/gongwen-markdown"
    [ -f "$SKILL_SRC/SKILL.md" ] || die "技能包源目录不存在：$SKILL_SRC"
    mkdir -p "$STAGE/skills"
    command -v zip >/dev/null 2>&1 || die "找不到 zip 命令（打包技能包需要）"
    (cd "$PROJECT_ROOT/skills" &&
        zip -Xrq "$STAGE/skills/gongwen-markdown.skill" gongwen-markdown \
            -x '*__pycache__*' -x '*.DS_Store')

    chmod 755 "$STAGE/gongwen-assistant"
fi

# 与 Windows 便携包一致：项目自维护的 quan.txt 独立随包，不嵌入二进制；
# mabiao.txt 只作旧机后备。
# 使用现成 staging 时保留其中的 base.txt，不能因排除旧输入法而一并丢弃。
BASE_TABLE="$PROJECT_ROOT/assets/fuma/quan.txt"
if [ ! -f "$BASE_TABLE" ]; then
    BASE_TABLE="$PROJECT_ROOT/assets/fuma/mabiao.txt"
fi
if [ -f "$BASE_TABLE" ]; then
    mkdir -p "$STAGE/runtime/ime"
    cp "$BASE_TABLE" "$STAGE/runtime/ime/base.txt"
elif [ ! -f "$STAGE/runtime/ime/base.txt" ]; then
    echo "警告：未找到基础词表；安装后需在「输入法词表」页导入，内置输入法才能接管" >&2
fi

if [ ! -x "$STAGE/gongwen-assistant" ]; then
    die "staging 里没有可执行的二进制：$STAGE/gongwen-assistant"
fi
if [ ! -f "$STAGE/runtime/fonts/GWFangSong.ttf" ]; then
    die "staging 里没有 runtime 字体：$STAGE/runtime/fonts"
fi

# 没有 Developer ID 时，ad-hoc 签名必须在**放进 .app 之前**对 staging 里的
# 可执行文件做：codesign 一旦看到 .app 内的路径，会把它当成 bundle 的
# 主可执行文件并尝试签整个 bundle，随包字体 / 许可证资源会被当成嵌套代码，
# 报 “code object is not signed at all In subcomponent”。
# 注意：node 不能在这里签——它随后还要按 manifest 校验 SHA-256；签名会改写文件，
# 校验就过不去。随包 node 保留官方 Developer ID 签名（见下方 PI_DIR 段）。
if [ -z "${GONGWEN_MACOS_SIGN_IDENTITY:-}" ]; then
    codesign --force --sign - "$STAGE/gongwen-assistant"
    codesign --verify --strict "$STAGE/gongwen-assistant"
fi

MACOS_DIR="$BUILD_ROOT/$BUNDLE_NAME/Contents/MacOS"
RESOURCES_DIR="$BUILD_ROOT/$BUNDLE_NAME/Contents/Resources"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

# 可执行文件与 runtime/ 同级，对应 src/portable_runtime.rs 的查找逻辑
# （current_exe().parent()/runtime）。
cp "$STAGE/gongwen-assistant" "$MACOS_DIR/gongwen-assistant"
cp -a "$STAGE/runtime" "$MACOS_DIR/runtime"
chmod 755 "$MACOS_DIR/gongwen-assistant"

# Pi 运行时显式再落一次：校验 manifest / 哈希 / 可执行位后再复制并 chmod，
# 避免某些复制方式丢可执行位，运行期才以 EACCES 暴露。
#
# 这里**不要**重签 node。Node 官方包自带 Developer ID 签名（hardened runtime + JIT
# 权限，codesign --verify 通过），重签只会改写文件；而随包 manifest.json 记录的是官方
# 二进制的体积与 SHA-256，应用启动时 src/pi_agent/runtime.rs 的 verify_bundled 会按
# manifest 逐项比哈希，文件一变就报「随包 Pi 运行时文件校验失败（bin/node）。请重装。」
# 智能写作整条链随即不可用。需要 Developer ID 签名时在下面 .app 签名段处理 node，
# 并在签完后回写 manifest。
PI_DIR="$STAGE/runtime/pi"
if [ -f "$PI_DIR/manifest.json" ]; then
    verify_pi_runtime "$PI_DIR"
    rm -rf "$MACOS_DIR/runtime/pi"
    cp -a "$PI_DIR" "$MACOS_DIR/runtime/pi"
    chmod 755 "$MACOS_DIR/runtime/pi/bin/node"
else
    if [ "$REQUIRE_PI" -eq 1 ]; then
        die "staging 里没有 Pi 运行时（${PI_DIR}/manifest.json），但传了 --require-pi"
    fi
    echo "警告：未包含 Pi 运行时，智能写作将不可用（缺 ${PI_DIR}/manifest.json）" >&2
fi

# 随包 runtime 里的许可证、字体说明等普通文件从 zip 解出来可能带可执行位；
# codesign 会把 bundle 内带执行位的文件当成嵌套代码，要求逐个签名并以
# “code object is not signed at all In subcomponent” 失败。先统一去掉普通文件
# 的执行位，再恢复真正的可执行文件。
find "$MACOS_DIR/runtime" -type f -exec chmod a-x {} + 2>/dev/null || true
[ -f "$MACOS_DIR/runtime/pi/bin/node" ] && chmod 755 "$MACOS_DIR/runtime/pi/bin/node"

# 应用依赖的许可证由本仓库维护，外部 runtime 清单还没列它；Windows 的
# scripts/package-portable.ps1 与 Linux 的 scripts/stage-linux-portable.sh 都显式补拷，
# macOS 这里照做——THIRD_PARTY_NOTICES.md 承诺它在 runtime/licenses/NUCLEO-MPL-2.0.txt。
NUCLEO_LICENSE="$PROJECT_ROOT/runtime/licenses/NUCLEO-MPL-2.0.txt"
if [ -f "$NUCLEO_LICENSE" ]; then
    mkdir -p "$MACOS_DIR/runtime/licenses"
    install -m 644 "$NUCLEO_LICENSE" "$MACOS_DIR/runtime/licenses/"
else
    echo "警告：缺少 runtime/licenses/NUCLEO-MPL-2.0.txt，随包许可证不完整" >&2
fi

# 文档随包进 bundle，DMG 保持纯拖放布局。
for doc in README.md THIRD_PARTY_NOTICES.md LICENSE; do
    if [ -f "$STAGE/$doc" ]; then
        cp "$STAGE/$doc" "$RESOURCES_DIR/$doc"
    fi
done
# AI 技能包（skills/gongwen-markdown.skill，由 package-portable.ps1 或一键模式生成）。
if [ -d "$STAGE/skills" ]; then
    cp -a "$STAGE/skills" "$RESOURCES_DIR/skills"
fi

# 直接使用与其他平台一起生成的多尺寸图标。
cp "$ICNS_FILE" "$RESOURCES_DIR/AppIcon.icns"

# Info.plist 刻意不含 CFBundleDocumentTypes / UTExportedTypeDeclarations：
# .gwx 双击走 macOS open-file Apple Event，当前 winit/eframe 外壳还没接，
# 注册了只会「打开应用但不导入」。导入请用拖放 / 应用内导入。
cat >"$BUILD_ROOT/$BUNDLE_NAME/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>zh_CN</string>
	<key>CFBundleExecutable</key>
	<string>gongwen-assistant</string>
	<key>CFBundleIdentifier</key>
	<string>$BUNDLE_ID</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>$APP_NAME</string>
	<key>CFBundleDisplayName</key>
	<string>$APP_NAME</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>$VERSION</string>
	<key>CFBundleVersion</key>
	<string>$VERSION</string>
	<key>CFBundleIconFile</key>
	<string>AppIcon</string>
	<key>LSMinimumSystemVersion</key>
	<string>12.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSPrincipalClass</key>
	<string>NSApplication</string>
</dict>
</plist>
EOF

SIGN_IDENTITY="${GONGWEN_MACOS_SIGN_IDENTITY:-}"
NOTARY_PROFILE="${GONGWEN_MACOS_NOTARY_PROFILE:-}"
APP_BUNDLE="$BUILD_ROOT/$BUNDLE_NAME"
# 从 zip 解出来的运行时可能带 resource fork / quarantine 扩展属性，codesign 会以
# 「resource fork, Finder information, or similar detritus not allowed」失败。
xattr -cr "$APP_BUNDLE" 2>/dev/null || true

if [ -n "$SIGN_IDENTITY" ]; then
    [ -f "$ENTITLEMENTS" ] || die "缺少 entitlements：$ENTITLEMENTS"
    PI_NODE="$MACOS_DIR/runtime/pi/bin/node"
    if [ -f "$PI_NODE" ]; then
        # 先签 bundle 内的 Node（V8 JIT 权限必须挂在 node 自己身上），再签整个 .app。
        # 不用 --deep：--deep 不会给嵌套可执行文件单独带 entitlements，也不该用来签
        # node_modules 之类的资源。
        echo "==> codesign（Developer ID + hardened runtime）：$SIGN_IDENTITY"
        codesign --force --options runtime --timestamp \
            --entitlements "$ENTITLEMENTS" --sign "$SIGN_IDENTITY" "$PI_NODE"
        codesign --verify --strict "$PI_NODE"
        # 签名改写了 node，必须把新的体积与 SHA-256 回写进随包 manifest：应用启动时
        # src/pi_agent/runtime.rs 的 verify_bundled 按 manifest 逐项比哈希，不更新就报
        # 「随包 Pi 运行时文件校验失败（bin/node）。请重装。」智能写作即不可用。
        # ad-hoc 路径不重签 node（用官方 Developer ID 签名），因此不需要这一步。
        "$PI_NODE" -e '
const fs = require("fs");
const crypto = require("crypto");
const dir = process.argv[1];
const bytes = fs.readFileSync(dir + "/bin/node");
const manifestPath = dir + "/manifest.json";
const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
const entry = (manifest.files || []).find((file) => file.path === "bin/node");
if (!entry) throw new Error("manifest 缺 bin/node");
entry.bytes = bytes.length;
entry.sha256 = crypto.createHash("sha256").update(bytes).digest("hex");
fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2) + "\n");
' "$MACOS_DIR/runtime/pi" || die "签名后回写 Pi 运行时 manifest 失败"
        verify_pi_runtime "$MACOS_DIR/runtime/pi"
    else
        echo "警告：没有随包 Pi 运行时，跳过 node 签名与 manifest 回写" >&2
    fi
    codesign --force --options runtime --timestamp \
        --entitlements "$ENTITLEMENTS" --sign "$SIGN_IDENTITY" \
        "$APP_BUNDLE"
    # 不要 --deep：随包 runtime 里有些资产（如许可证 txt）带可执行位，
    # --deep 会把它们当嵌套代码要求签名并以 “In subcomponent” 失败。
    codesign --verify --strict --verbose=2 "$APP_BUNDLE"
else
    if [ -n "$NOTARY_PROFILE" ]; then
        die "配置了 GONGWEN_MACOS_NOTARY_PROFILE 但没有 GONGWEN_MACOS_SIGN_IDENTITY；公证必须有 Developer ID 签名"
    fi
    # 没有 Developer ID：可执行文件已在放进 .app 之前做过 ad-hoc 签名（见上方
    # staging 段），这里只提示产物未签名/未公证，不再对 .app 内路径调用 codesign。
    echo "警告：未配置 GONGWEN_MACOS_SIGN_IDENTITY，仅 ad-hoc 签名——未签名/未公证，仅本机测试；" >&2
    echo "      换机器下载会被 Gatekeeper 拦，不要作为发布产物分发。" >&2
fi

# 拖放到「应用程序」的 DMG：bundle 加 /Applications 符号链接。
DMG_ROOT="$BUILD_ROOT/dmg"
mkdir -p "$DMG_ROOT"
cp -R "$APP_BUNDLE" "$DMG_ROOT/"
ln -s /Applications "$DMG_ROOT/Applications"

[ -n "$OUTPUT_DIR" ] || OUTPUT_DIR="$PROJECT_ROOT/dist/macos"
case "$OUTPUT_DIR" in
    /*) ;;
    *) OUTPUT_DIR="$PROJECT_ROOT/$OUTPUT_DIR" ;;
esac
mkdir -p "$OUTPUT_DIR"

DMG_NAME="gongwen-assistant-${VERSION}-macos-${ARCH}.dmg"
DMG_PATH="$OUTPUT_DIR/$DMG_NAME"
echo "==> 生成磁盘映像：$DMG_PATH"
hdiutil create \
    -volname "$APP_NAME" \
    -srcfolder "$DMG_ROOT" \
    -ov \
    -format UDZO \
    "$DMG_PATH" >/dev/null
hdiutil verify "$DMG_PATH" >/dev/null

# 公证必须在 DMG 生成之后、写 .sha256 之前：staple 会改写 DMG，先算哈希就作废。
if [ -n "$NOTARY_PROFILE" ]; then
    echo "==> 提交公证（notarytool profile: $NOTARY_PROFILE），等待结果……"
    if ! xcrun notarytool submit "$DMG_PATH" --keychain-profile "$NOTARY_PROFILE" --wait; then
        echo "error: 公证失败，DMG 已保留：$DMG_PATH" >&2
        echo "       用 xcrun notarytool log <submission-id> --keychain-profile $NOTARY_PROFILE 查原因" >&2
        exit 1
    fi
    if ! xcrun stapler staple "$DMG_PATH"; then
        echo "error: staple 失败，DMG 已保留：$DMG_PATH" >&2
        exit 1
    fi
    xcrun stapler validate "$DMG_PATH" >/dev/null ||
        { echo "error: staple 后校验失败：$DMG_PATH" >&2; exit 1; }
fi

# 校验文件里只写文件名（与 scripts/package-arch-pkg.sh 一致）：写绝对路径的话，
# 别人下载 .dmg 与 .sha256 后 `shasum -a 256 -c` 会因为路径对不上而失败。
shasum -a 256 "$DMG_PATH" |
    awk -v name="$(basename "$DMG_PATH")" '{ printf "%s  %s\n", $1, name }' >"$DMG_PATH.sha256"

echo "Created: $DMG_PATH"
