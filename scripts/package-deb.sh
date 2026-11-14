#!/usr/bin/env bash
# Package the portable Linux layout (binary + TeX runtime) as a Debian package.
#
# Usage: package-deb.sh <version> <staging-dir> <output-dir> [architecture] [ai|no-ai]
#
# The staging directory is produced by scripts/package-portable.ps1 with
# -ArchiveFormat none; it contains `gongwen-assistant`, `runtime/`, and the
# README/LICENSE documents. This script lays that tree out under
# /opt/gongwen-assistant and adds a /usr/bin symlink, a .desktop entry, and
# icon assets, then builds the .deb with dpkg-deb.
set -euo pipefail

VERSION="${1:?usage: package-deb.sh <version> <staging-dir> <output-dir>}"
STAGING="${2:?missing staging directory}"
OUTPUT_DIR="${3:?missing output directory}"

ARCH="${4:-arm64}"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EDITION="${5:-ai}"
source "$PROJECT_ROOT/scripts/package-edition.sh"
PACKAGE="gongwen-assistant${EDITION_SUFFIX}"
DESKTOP_ID="cn.localtools.GongwenAssistant"
if [ "$EDITION" = no-ai ]; then DESKTOP_ID+="NoAi"; fi


INSTALL_ROOT="/opt/$PACKAGE"

case "$ARCH" in
    arm64)
        PLATFORM_DESCRIPTION="Linux ARM64"
        ;;
    amd64)
        PLATFORM_DESCRIPTION="Linux AMD64"
        ;;
    *)
        echo "error: unsupported Debian architecture: $ARCH (expected arm64 or amd64)" >&2
        exit 1
        ;;
esac

# shellcheck source=../packaging/debian/depends.sh
. "$PROJECT_ROOT/packaging/debian/depends.sh"

if [ ! -x "$STAGING/gongwen-assistant" ]; then
    echo "error: staging binary not found or not executable: $STAGING/gongwen-assistant" >&2
    exit 1
fi
if [ ! -f "$STAGING/runtime/fonts/GWFangSong.ttf" ]; then
    echo "error: staging runtime fonts not found: $STAGING/runtime/fonts" >&2
    exit 1
fi

check_package_binary "$STAGING/gongwen-assistant"
check_package_resources "$STAGING"

# 随包 Pi 运行时：缺 Node / host / manifest 或校验不符就不能出包。
if [ "$EDITION" = ai ]; then
    case "$ARCH" in
        arm64) pi_platform="linux-arm64" ;;
        amd64) pi_platform="linux-x64" ;;
    esac
    if [ ! -x "$STAGING/runtime/pi/bin/node" ]; then
        echo "error: staging Pi runtime missing: $STAGING/runtime/pi/bin/node" >&2
        exit 1
    fi
    "$STAGING/runtime/pi/bin/node" "$PROJECT_ROOT/scripts/pi-runtime-verify.mjs" "$STAGING/runtime/pi" "$pi_platform" >/dev/null

fi

PKG_ROOT="$(mktemp -d)"
trap 'rm -rf "$PKG_ROOT"' EXIT

APP_DIR="$PKG_ROOT$INSTALL_ROOT"
mkdir -p "$APP_DIR"

# Copy the full portable tree into the install root.
cp -a "$STAGING"/. "$APP_DIR"/
chmod 755 "$APP_DIR/gongwen-assistant"
install -Dm755 "$APP_DIR/gongwen-assistant" "$PKG_ROOT/usr/libexec/$PACKAGE/gongwen-is903"

# Command-line entry point.
mkdir -p "$PKG_ROOT/usr/bin"
ln -s "$INSTALL_ROOT/gongwen-assistant" "$PKG_ROOT/usr/bin/$PACKAGE"

# Desktop entry. Its filename must match the Wayland app_id used by eframe.
mkdir -p "$PKG_ROOT/usr/share/applications"
cp "$PROJECT_ROOT/assets/linux/$DESKTOP_ID.desktop" \
    "$PKG_ROOT/usr/share/applications/$DESKTOP_ID.desktop"

# Icons: install the pre-rendered PNG sizes into the hicolor theme.
for size in 16 24 32 48 64 128 256 512; do
    src="$PROJECT_ROOT/assets/app-icon/app-icon-$size.png"
    if [ -f "$src" ]; then
        dest="$PKG_ROOT/usr/share/icons/hicolor/${size}x${size}/apps"
        mkdir -p "$dest"
        cp "$src" "$dest/$DESKTOP_ID.png"
    fi
done
# 稿件包 .gwx：MIME 类型定义与文档图标。update-mime-database、gtk-update-icon-cache
# 由 shared-mime-info、hicolor-icon-theme 的 dpkg 触发器自动跑，不必写 postinst。
if [ "$EDITION" = ai ]; then
    mkdir -p "$PKG_ROOT/usr/share/mime/packages"
    cp "$PROJECT_ROOT/assets/linux/cn.localtools.GongwenAssistant.xml"     "$PKG_ROOT/usr/share/mime/packages/cn.localtools.GongwenAssistant.xml"
    for size in 16 24 32 48 64 128 256 512; do
        dest="$PKG_ROOT/usr/share/icons/hicolor/${size}x${size}/mimetypes"
        mkdir -p "$dest"
        cp "$PROJECT_ROOT/assets/doc-icon/gwx-document-$size.png"         "$dest/application-x-gongwen-package.png"
    done
fi

if [ -f "$PROJECT_ROOT/assets/app-icon/app-icon.png" ]; then
    mkdir -p "$PKG_ROOT/usr/share/pixmaps"
    cp "$PROJECT_ROOT/assets/app-icon/app-icon.png" \
        "$PKG_ROOT/usr/share/pixmaps/$DESKTOP_ID.png"
fi

INSTALLED_SIZE="$(du -sk "$APP_DIR" | cut -f1)"

mkdir -p "$PKG_ROOT/DEBIAN"
cat > "$PKG_ROOT/DEBIAN/control" <<EOF
Package: $PACKAGE
Version: $VERSION
Section: utils
Priority: optional
Architecture: $ARCH
Maintainer: billowsand <billowsand@users.noreply.github.com>
Depends: $DEPENDS
Recommends: $RECOMMENDS
Installed-Size: $INSTALLED_SIZE
Description: 离线公文写作助手 (offline official-document writing assistant)
 公文助手是一个基于 Rust 与 egui 的
 离线公文写作桌面应用。内置 Typst 排版引擎与字体，
 可导出 Markdown、DOCX 与 PDF。
 .
 此包针对 ${PLATFORM_DESCRIPTION}（glibc >= 2.28），安装在 $INSTALL_ROOT。
EOF

mkdir -p "$OUTPUT_DIR"
DEB_NAME="${PACKAGE}_${VERSION}_${ARCH}.deb"
# 固定用 xz：新版 dpkg-deb 默认 zstd，Debian buster 等老系统的 dpkg 不认。
dpkg-deb -Zxz --build --root-owner-group "$PKG_ROOT" "$OUTPUT_DIR/$DEB_NAME"

echo "Created: $OUTPUT_DIR/$DEB_NAME"
