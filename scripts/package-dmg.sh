#!/usr/bin/env bash
# Package the portable macOS layout (binary + TeX runtime) as an app bundle
# and a drag-to-Applications DMG.
#
# Usage: package-dmg.sh <version> <staging-dir> <output-dir>
#
# The staging directory is produced by scripts/package-portable.ps1 with
# -ArchiveFormat none; it contains `gongwen-assistant`, `runtime/`, and the
# README/LICENSE documents. This script lays that tree out as
# GongwenAssistant.app, generates the .icns icon from the PNG assets with
# iconutil, ad-hoc signs every binary, then builds the DMG with hdiutil.
set -euo pipefail

VERSION="${1:?usage: package-dmg.sh <version> <staging-dir> <output-dir>}"
STAGING="${2:?missing staging directory}"
OUTPUT_DIR="${3:?missing output directory}"

BUNDLE_ID="com.billowsand.gongwen"
APP_NAME="公文助手"
BUNDLE_NAME="GongwenAssistant.app"
DMG_NAME="gongwen-assistant-${VERSION}-macos-arm64.dmg"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [ ! -x "$STAGING/gongwen-assistant" ]; then
    echo "error: staging binary not found or not executable: $STAGING/gongwen-assistant" >&2
    exit 1
fi
if [ ! -x "$STAGING/runtime/tectonic/tectonic" ]; then
    echo "error: staging runtime tectonic not found or not executable" >&2
    exit 1
fi
if [ ! -f "$PROJECT_ROOT/assets/app-icon/app-icon-512.png" ]; then
    echo "error: icon source not found: $PROJECT_ROOT/assets/app-icon/app-icon-512.png" >&2
    exit 1
fi

BUILD_ROOT="$(mktemp -d)"
trap 'rm -rf "$BUILD_ROOT"' EXIT

MACOS_DIR="$BUILD_ROOT/$BUNDLE_NAME/Contents/MacOS"
RESOURCES_DIR="$BUILD_ROOT/$BUNDLE_NAME/Contents/Resources"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

# Executable next to its runtime/, matching the lookup in
# src/portable_runtime.rs (current_exe().parent()/runtime).
cp "$STAGING/gongwen-assistant" "$MACOS_DIR/gongwen-assistant"
cp -a "$STAGING/runtime" "$MACOS_DIR/runtime"
chmod 755 "$MACOS_DIR/gongwen-assistant" "$MACOS_DIR/runtime/tectonic/tectonic"

# Documentation travels inside the bundle so the DMG stays a plain
# drag-to-Applications drop.
for doc in README.md THIRD_PARTY_NOTICES.md LICENSE; do
    if [ -f "$STAGING/$doc" ]; then
        cp "$STAGING/$doc" "$RESOURCES_DIR/$doc"
    fi
done

# Generate the .icns from the pre-rendered PNGs. iconutil requires the full
# fixed-size iconset; sips guarantees exact pixel dimensions.
ICONSET="$BUILD_ROOT/AppIcon.iconset"
mkdir -p "$ICONSET"
render() {
    local size=$1
    local name=$2
    sips -z "$size" "$size" \
        "$PROJECT_ROOT/assets/app-icon/app-icon-512.png" \
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

cat > "$BUILD_ROOT/$BUNDLE_NAME/Contents/Info.plist" <<EOF
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

# No Developer ID is available, so sign ad hoc (identity "-"). This keeps the
# app runnable on Apple Silicon and covers the bundled Tectonic binary.
codesign --force --deep --sign - "$BUILD_ROOT/$BUNDLE_NAME"
codesign --verify --deep --strict "$BUILD_ROOT/$BUNDLE_NAME"

# Drag-to-Applications DMG: the bundle plus a symlink to /Applications.
DMG_ROOT="$BUILD_ROOT/dmg"
mkdir -p "$DMG_ROOT"
cp -R "$BUILD_ROOT/$BUNDLE_NAME" "$DMG_ROOT/"
ln -s /Applications "$DMG_ROOT/Applications"

mkdir -p "$OUTPUT_DIR"
hdiutil create \
    -volname "$APP_NAME" \
    -srcfolder "$DMG_ROOT" \
    -ov \
    -format UDZO \
    "$OUTPUT_DIR/$DMG_NAME" >/dev/null
hdiutil verify "$OUTPUT_DIR/$DMG_NAME" >/dev/null

shasum -a 256 "$OUTPUT_DIR/$DMG_NAME" |
    awk '{ printf "%s  %s\n", $1, $2 }' > "$OUTPUT_DIR/$DMG_NAME.sha256"

echo "Created: $OUTPUT_DIR/$DMG_NAME"
