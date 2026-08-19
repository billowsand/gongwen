#!/usr/bin/env bash
# Package the portable Linux layout (binary + TeX runtime) as a Debian package.
#
# Usage: package-deb.sh <version> <staging-dir> <output-dir> [architecture]
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

PACKAGE="gongwen-assistant"
ARCH="${4:-arm64}"
INSTALL_ROOT="/opt/gongwen-assistant"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

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

# Runtime dependencies for eframe/glow + rfd (GTK3) on X11/Wayland. Kept as a
# deterministic list instead of dpkg-shlibdeps so the package does not depend
# on which libraries happen to be installed on the packaging host.
DEPENDS="libc6 (>= 2.28), libgcc-s1, libglib2.0-0, libgtk-3-0, \
libgdk-pixbuf-2.0-0, libpango-1.0-0, libcairo2, libatk1.0-0, libx11-6, \
libx11-xcb1, libxcb1, libxcb-render0, libxcb-shape0, libxcb-xfixes0, \
libxkbcommon0, libxkbcommon-x11-0, libxrender1, libxrandr2, libxcursor1, \
libxinerama1, libxext6, libxi6, libxfixes3, libxdamage1, libxcomposite1, \
libwayland-client0, libwayland-cursor0, libwayland-egl1, libegl1, libgl1, \
libglx0, libfontconfig1, libfreetype6, libdbus-1-3, zlib1g"

# 打印走 CUPS 的 lp。列为 Recommends 而不是 Depends：apt 默认会装上，但缺了
# 也只是打印按钮置灰，不该拦住整个应用的安装。
RECOMMENDS="cups-client"

if [ ! -x "$STAGING/gongwen-assistant" ]; then
    echo "error: staging binary not found or not executable: $STAGING/gongwen-assistant" >&2
    exit 1
fi
if [ ! -x "$STAGING/runtime/tectonic/tectonic" ]; then
    echo "error: staging runtime tectonic not found or not executable" >&2
    exit 1
fi

PKG_ROOT="$(mktemp -d)"
trap 'rm -rf "$PKG_ROOT"' EXIT

APP_DIR="$PKG_ROOT$INSTALL_ROOT"
mkdir -p "$APP_DIR"

# Copy the full portable tree into the install root.
cp -a "$STAGING"/. "$APP_DIR"/
chmod 755 "$APP_DIR/gongwen-assistant" "$APP_DIR/runtime/tectonic/tectonic"

# Command-line entry point.
mkdir -p "$PKG_ROOT/usr/bin"
ln -s "$INSTALL_ROOT/gongwen-assistant" "$PKG_ROOT/usr/bin/gongwen-assistant"

# Desktop entry. Its filename must match the Wayland app_id used by eframe.
mkdir -p "$PKG_ROOT/usr/share/applications"
cp "$PROJECT_ROOT/assets/linux/cn.localtools.GongwenAssistant.desktop" \
    "$PKG_ROOT/usr/share/applications/cn.localtools.GongwenAssistant.desktop"

# Icons: install the pre-rendered PNG sizes into the hicolor theme.
for size in 16 24 32 48 64 128 256 512; do
    src="$PROJECT_ROOT/assets/app-icon/app-icon-$size.png"
    if [ -f "$src" ]; then
        dest="$PKG_ROOT/usr/share/icons/hicolor/${size}x${size}/apps"
        mkdir -p "$dest"
        cp "$src" "$dest/cn.localtools.GongwenAssistant.png"
    fi
done
if [ -f "$PROJECT_ROOT/assets/app-icon/app-icon.png" ]; then
    mkdir -p "$PKG_ROOT/usr/share/pixmaps"
    cp "$PROJECT_ROOT/assets/app-icon/app-icon.png" \
        "$PKG_ROOT/usr/share/pixmaps/cn.localtools.GongwenAssistant.png"
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
 公文助手是一个基于 Rust、egui 与本地模型服务（LM Studio / Ollama 等）的
 离线公文写作桌面应用。内置便携式 TeX 运行时（Tectonic + 离线 bundle + 字体），
 可导出 Markdown、DOCX、TeX 并编译 PDF。
 .
 此包针对 $PLATFORM_DESCRIPTION（glibc >= 2.28），安装在 /opt/gongwen-assistant。
EOF

mkdir -p "$OUTPUT_DIR"
DEB_NAME="${PACKAGE}_${VERSION}_${ARCH}.deb"
dpkg-deb --build --root-owner-group "$PKG_ROOT" "$OUTPUT_DIR/$DEB_NAME"

echo "Created: $OUTPUT_DIR/$DEB_NAME"
