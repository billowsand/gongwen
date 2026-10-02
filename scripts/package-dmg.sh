#!/usr/bin/env bash
# 一键打包 macOS DMG：构建二进制 → 按 runtime 清单组装便携布局 → .app bundle
# → ad-hoc 签名 → 带「应用程序」拖放链接的 DMG。供本机使用（发布流水线只产
# Windows / Linux ARM64，不产 macOS 包）。
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
#   -h, --help        显示帮助。
#
# 产物：dist/macos/gongwen-assistant-<版本>-macos-<架构>.dmg（附 .sha256）。
# 打开 DMG 把「公文助手」拖进「应用程序」即可。未做 Developer ID 签名，
# 仅 ad-hoc 签名（identity "-"），本机可直接运行。
#
# 一键模式的 staging 组装与 scripts/package-portable.ps1 保持一致：
#   - 校验 runtime/SHA256SUMS.<平台>.txt 里每个资产的 SHA-256；
#   - 资产放进可执行文件旁的 runtime/（程序按它查找，见 src/portable_runtime.rs），
#     旧清单里的 Tectonic / TeX bundle 不随包；
#   - 附 README / THIRD_PARTY_NOTICES / LICENSE / config.example.json；
#   - 把 skills/gongwen-markdown/ 打成 skills/gongwen-markdown.skill（zip，
#     顶层一个 gongwen-markdown/ 文件夹，条目名正斜杠）。
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
BUNDLE_ID="com.billowsand.gongwen"
APP_NAME="公文助手"
BUNDLE_NAME="GongwenAssistant.app"

die() {
    echo "error: $*" >&2
    exit 1
}

usage() {
    sed -n '2,27p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

VERSION=""
STAGING=""
OUTPUT_DIR=""
SKIP_BUILD=0

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
    *) die "package-dmg.sh 只能在 macOS 上运行（需要 hdiutil / codesign / iconutil）" ;;
esac

# 版本号只维护在 Cargo.toml，与 bump-version.ps1 保持一致。
if [ -z "$VERSION" ]; then
    VERSION="$(
        grep -m1 '^version[[:space:]]*=' "$PROJECT_ROOT/Cargo.toml" |
            sed -E 's/.*"([0-9]+\.[0-9]+\.[0-9]+)".*/\1/'
    )"
fi
case "$VERSION" in
    [0-9]*\.[0-9]*\.[0-9]*) ;;
    *) die "无法确定 x.y.z 版本号（得到：$VERSION）" ;;
esac

ARCH="$(uname -m)"
case "$ARCH" in
    arm64) SUFFIX="darwin-arm64" ;;
    x86_64) SUFFIX="darwin-amd64" ;;
    *) SUFFIX="darwin-$ARCH" ;;
esac

if [ ! -f "$PROJECT_ROOT/assets/app-icon/app-icon-1024.png" ]; then
    die "图标源图不存在：$PROJECT_ROOT/assets/app-icon/app-icon-1024.png"
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
    [ -f "$BIN" ] || die "找不到二进制：$BIN（去掉 --skip-build 或先构建）"
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
        [ "$actual" = "$expected" ] || die "SHA-256 不匹配：$src（期望 $expected，实际 $actual）"
        case "$rel" in
            # PDF 由 Typst 在进程内排版，旧 runtime 清单里的 TeX 资产不随包。
            ime/* | tectonic/* | texbundle/* | licenses/LICENSE.CTAN | licenses/LICENSE.TL | licenses/TECTONIC-LICENSE.txt) continue ;;
            *) dest="runtime/$rel" ;;
        esac
        mkdir -p "$STAGE/$(dirname "$dest")"
        cp "$src" "$STAGE/$dest"
    done <"$MANIFEST"
    [ -f "$STAGE/runtime/fonts/FZFangSong.ttf" ] ||
        die "runtime 清单未提供字体，PDF 排版将不可用（$SUFFIX）"

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

if [ ! -x "$STAGE/gongwen-assistant" ]; then
    die "staging 里没有可执行的二进制：$STAGE/gongwen-assistant"
fi
if [ ! -f "$STAGE/runtime/fonts/FZFangSong.ttf" ]; then
    die "staging 里没有 runtime 字体：$STAGE/runtime/fonts"
fi

MACOS_DIR="$BUILD_ROOT/$BUNDLE_NAME/Contents/MacOS"
RESOURCES_DIR="$BUILD_ROOT/$BUNDLE_NAME/Contents/Resources"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

# 可执行文件与 runtime/ 同级，对应 src/portable_runtime.rs 的查找逻辑
# （current_exe().parent()/runtime）。
cp "$STAGE/gongwen-assistant" "$MACOS_DIR/gongwen-assistant"
cp -a "$STAGE/runtime" "$MACOS_DIR/runtime"
chmod 755 "$MACOS_DIR/gongwen-assistant"

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

# 由预渲染 PNG 生成 .icns。iconutil 需要完整固定尺寸 iconset，sips 保证像素精确。
ICONSET="$BUILD_ROOT/AppIcon.iconset"
mkdir -p "$ICONSET"
render() {
    local size=$1
    local name=$2
    sips -z "$size" "$size" \
        "$PROJECT_ROOT/assets/app-icon/app-icon-1024.png" \
        --out "$ICONSET/$name" >/dev/null
}
render 16 icon_16x16.png
render 32 icon_16x16@2x.png
render 32 icon_32x32.png
render 64 icon_32x32@2x.png
render 128 icon_128x128.png
render 256 icon_128x128@2x.png
render 256 icon_256x256.png
render 512 icon_256x256@2x.png
render 512 icon_512x512.png
render 1024 icon_512x512@2x.png
iconutil -c icns "$ICONSET" -o "$RESOURCES_DIR/AppIcon.icns"

cat >"$BUILD_ROOT/$BUNDLE_NAME/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>zh_CN</string>
	<key>CFBundleExecutable</key>
	<string>gongwen-assistant</string>
	<key>CFBundleIconFile</key>
	<string>AppIcon</string>
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
	<key>LSMinimumSystemVersion</key>
	<string>12.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSPrincipalClass</key>
	<string>NSApplication</string>
</dict>
</plist>
EOF

# 没有 Developer ID，做 ad-hoc 签名（identity "-"），保证 Apple Silicon 上可直接运行。
codesign --force --deep --sign - "$BUILD_ROOT/$BUNDLE_NAME"
codesign --verify --deep --strict "$BUILD_ROOT/$BUNDLE_NAME"

# 拖放到「应用程序」的 DMG：bundle 加 /Applications 符号链接。
DMG_ROOT="$BUILD_ROOT/dmg"
mkdir -p "$DMG_ROOT"
cp -R "$BUILD_ROOT/$BUNDLE_NAME" "$DMG_ROOT/"
ln -s /Applications "$DMG_ROOT/Applications"

[ -n "$OUTPUT_DIR" ] || OUTPUT_DIR="$PROJECT_ROOT/dist/macos"
case "$OUTPUT_DIR" in
    /*) ;;
    *) OUTPUT_DIR="$PROJECT_ROOT/$OUTPUT_DIR" ;;
esac
mkdir -p "$OUTPUT_DIR"

DMG_NAME="gongwen-assistant-${VERSION}-macos-${ARCH}.dmg"
echo "==> 生成磁盘映像：$OUTPUT_DIR/$DMG_NAME"
hdiutil create \
    -volname "$APP_NAME" \
    -srcfolder "$DMG_ROOT" \
    -ov \
    -format UDZO \
    "$OUTPUT_DIR/$DMG_NAME" >/dev/null
hdiutil verify "$OUTPUT_DIR/$DMG_NAME" >/dev/null

shasum -a 256 "$OUTPUT_DIR/$DMG_NAME" |
    awk '{ printf "%s  %s\n", $1, $2 }' >"$OUTPUT_DIR/$DMG_NAME.sha256"

echo "Created: $OUTPUT_DIR/$DMG_NAME"
