#!/usr/bin/env bash
# 在 macOS 上把便携目录（二进制 + runtime 字体）组装成可双击运行的 .app bundle。
#
# 对应 Windows 的 scripts/package-installer.ps1（Inno Setup 安装程序）：这里用
# macOS 标准 .app 结构（Contents/MacOS + Contents/Resources + Info.plist）产出
# GongwenAssistant.app，拖入「应用程序」即可使用，便于在 macOS 上查看/验证程序。
#
# 用法：
#   ./scripts/package-macos.sh [--version X.Y.Z] [--staging DIR] \
#       [--output DIR] [--force] [--skip-build] [--dmg]
#
#   --version    版本号；缺省从 Cargo.toml 读取。
#   --staging    复用 package-portable.ps1 -ArchiveFormat none 生成的便携目录；
#                缺省时自动 cargo build --release 并组装（--skip-build 则用现有
#                target/release/gongwen-assistant）。
#   --output     产物目录，缺省 <仓库>/dist/macos。
#   --force      覆盖已存在的 .app（否则报错退出）。
#   --dmg        额外生成 .dmg 磁盘映像（需要 macOS 自带的 hdiutil 与 shasum）。
#
# 说明：gongwen-runtime 目前不发布 macOS 运行时资产；本地 runtime/ 目录若有字体
# 则一并打进 .app（PDF 可用），否则只打包应用本体——程序仍可启动查看界面，
# 仅 PDF 导出需要后续补充 runtime。
set -euo pipefail

VERSION=""
STAGING=""
OUTPUT_DIR=""
FORCE=0
SKIP_BUILD=0
BUILD_DMG=0

usage() {
    sed -n '2,17p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

die() {
    echo "error: $*" >&2
    exit 1
}

warn() {
    echo "warning: $*" >&2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version)
            [ $# -ge 2 ] || die "--version requires an argument"
            VERSION="$2"
            shift 2
            ;;
        --staging)
            [ $# -ge 2 ] || die "--staging requires an argument"
            STAGING="$2"
            shift 2
            ;;
        --output)
            [ $# -ge 2 ] || die "--output requires an argument"
            OUTPUT_DIR="$2"
            shift 2
            ;;
        --force)
            FORCE=1
            shift
            ;;
        --skip-build)
            SKIP_BUILD=1
            shift
            ;;
        --dmg)
            BUILD_DMG=1
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            die "unknown option: $1"
            ;;
    esac
done

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"

# 版本号只维护在 Cargo.toml，与 bump-version.ps1 保持一致。
if [ -z "$VERSION" ]; then
    VERSION="$(
        grep -m1 '^version[[:space:]]*=' "$PROJECT_ROOT/Cargo.toml" |
            sed -E 's/.*"([0-9]+\.[0-9]+\.[0-9]+)".*/\1/'
    )"
fi
case "$VERSION" in
    [0-9]*\.[0-9]*\.[0-9]*) ;;
    *) die "cannot determine a x.y.z version (got: $VERSION)" ;;
esac

# 组装临时 staging 目录；与 package-installer.ps1 的 -SourceDir 对应。
WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT
STAGE="$WORK_DIR/staging"

if [ -n "$STAGING" ]; then
    STAGE="$(cd "$STAGING" && pwd -P)"
    [ -f "$STAGE/gongwen-assistant" ] ||
        die "staging directory has no gongwen-assistant binary: $STAGE"
else
    if [ "$SKIP_BUILD" -eq 0 ]; then
        echo "==> cargo build --release"
        (cd "$PROJECT_ROOT" && cargo build --release)
    fi
    BIN="$PROJECT_ROOT/target/release/gongwen-assistant"
    [ -f "$BIN" ] || die "built binary not found: $BIN (drop --skip-build or build first)"
    mkdir -p "$STAGE"
    cp "$BIN" "$STAGE/gongwen-assistant"
    if [ -d "$PROJECT_ROOT/runtime" ]; then
        cp -a "$PROJECT_ROOT/runtime" "$STAGE/runtime"
    fi
    for doc in README.md THIRD_PARTY_NOTICES.md LICENSE config.example.json; do
        [ -f "$PROJECT_ROOT/$doc" ] && cp "$PROJECT_ROOT/$doc" "$STAGE/"
    done
fi

# runtime 是可选的：没有也能启动查看界面，PDF 排版需要其中的字体。
RUNTIME_DIR="$STAGE/runtime"
if [ -d "$RUNTIME_DIR" ]; then
    # PDF 由 Typst 在进程内排版，旧 runtime 里的 TeX 资产不随包。
    rm -rf "$RUNTIME_DIR/tectonic" "$RUNTIME_DIR/texbundle" "$RUNTIME_DIR/ime"
else
    warn "no runtime/ directory; app bundle will not include the bundled fonts (PDF export unavailable)"
fi

# 生成 .icns（sips + iconutil 为 macOS 自带）；不可用则跳过图标。
ICNS_FILE=""
if command -v sips >/dev/null 2>&1 && command -v iconutil >/dev/null 2>&1; then
    ICON_SRC="$PROJECT_ROOT/assets/app-icon/app-icon.png"
    if [ -f "$ICON_SRC" ]; then
        ICONSET="$WORK_DIR/app-icon.iconset"
        mkdir -p "$ICONSET"
        # 512 源图缩放出 iconset 各尺寸；512@2x 用放大到 1024 的占位。
        for spec in \
            "16 icon_16x16" "32 icon_32x32" "32 icon_32x32@2x" \
            "128 icon_128x128" "256 icon_128x128@2x" "256 icon_256x256" \
            "512 icon_256x256@2x" "512 icon_512x512" "1024 icon_512x512@2x"; do
            set -- $spec
            sips -z "$1" "$1" "$ICON_SRC" --out "$ICONSET/$2.png" >/dev/null
        done
        ICNS_FILE="$WORK_DIR/app-icon.icns"
        iconutil -c icns "$ICONSET" -o "$ICNS_FILE"
    fi
fi

[ -n "$OUTPUT_DIR" ] || OUTPUT_DIR="$PROJECT_ROOT/dist/macos"
case "$OUTPUT_DIR" in
    /*) ;;
    *) OUTPUT_DIR="$PROJECT_ROOT/$OUTPUT_DIR" ;;
esac
mkdir -p "$OUTPUT_DIR"

APP_NAME="GongwenAssistant.app"
APP_DIR="$OUTPUT_DIR/$APP_NAME"
if [ -e "$APP_DIR" ]; then
    if [ "$FORCE" -eq 1 ]; then
        rm -rf "$APP_DIR"
    else
        die "app bundle already exists: $APP_DIR (pass --force to overwrite)"
    fi
fi

MACOS_DIR="$APP_DIR/Contents/MacOS"
RESOURCES_DIR="$APP_DIR/Contents/Resources"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

# 程序按「可执行文件旁 runtime/」定位运行时（见 src/portable_runtime.rs），
# 因此 runtime 与二进制同放 Contents/MacOS，无需改动代码。
cp "$STAGE/gongwen-assistant" "$MACOS_DIR/gongwen-assistant"
chmod +x "$MACOS_DIR/gongwen-assistant"
if [ -d "$STAGE/runtime" ]; then
    cp -a "$STAGE/runtime" "$MACOS_DIR/runtime"
fi
for doc in README.md THIRD_PARTY_NOTICES.md LICENSE config.example.json; do
    [ -f "$STAGE/$doc" ] && cp "$STAGE/$doc" "$RESOURCES_DIR/"
done
if [ -n "$ICNS_FILE" ]; then
    cp "$ICNS_FILE" "$RESOURCES_DIR/app-icon.icns"
fi

cat > "$APP_DIR/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>zh_CN</string>
	<key>CFBundleDisplayName</key>
	<string>公文助手</string>
	<key>CFBundleExecutable</key>
	<string>gongwen-assistant</string>
	<key>CFBundleIdentifier</key>
	<string>cn.gongwen.GongwenAssistant</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>Gongwen Assistant</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>$VERSION</string>
	<key>CFBundleVersion</key>
	<string>$VERSION</string>
EOF
if [ -n "$ICNS_FILE" ]; then
    cat >> "$APP_DIR/Contents/Info.plist" <<'EOF'
	<key>CFBundleIconFile</key>
	<string>app-icon</string>
EOF
fi
cat >> "$APP_DIR/Contents/Info.plist" <<'EOF'
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.productivity</string>
	<key>LSMinimumSystemVersion</key>
	<string>11.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
EOF

echo "App bundle created: $APP_DIR"
if [ -f "$MACOS_DIR/runtime/fonts/FZFangSong.ttf" ]; then
    echo "Runtime fonts included: runtime/fonts"
else
    echo "Runtime fonts NOT included: app starts for preview, but PDF export needs a runtime"
fi

if [ "$BUILD_DMG" -eq 1 ]; then
    if command -v hdiutil >/dev/null 2>&1; then
        DMG_NAME="gongwen-assistant-$VERSION-macos-$(uname -m).dmg"
        DMG_PATH="$OUTPUT_DIR/$DMG_NAME"
        echo "==> Creating disk image: $DMG_PATH"
        hdiutil create -volname "公文助手" -srcfolder "$APP_DIR" -ov -format UDZO "$DMG_PATH" >/dev/null
        if command -v shasum >/dev/null 2>&1; then
            shasum -a 256 "$DMG_PATH" | awk '{ print $1 "  " $2 }' > "$DMG_PATH.sha256"
        elif command -v sha256sum >/dev/null 2>&1; then
            sha256sum "$DMG_PATH" | awk '{ print $1 "  " $2 }' > "$DMG_PATH.sha256"
        fi
        echo "Disk image created: $DMG_PATH"
    else
        warn "hdiutil not found; skipping .dmg (app bundle is still usable)"
    fi
fi

echo
echo "Done. Double-click to run: open \"$APP_DIR\""
