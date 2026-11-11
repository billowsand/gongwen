#!/usr/bin/env bash
# Install a built .deb into a clean Debian/Ubuntu system and verify that every
# dependency resolves and the installed binaries find all their shared libraries.
#
# Usage: verify-deb-install.sh <path-to-deb>
#
# Meant to run as root inside a throwaway container (see release.yml
# verify-deb). It catches wrong or renamed package names in DEBIAN/control —
# 例如 libgdk-pixbuf-2.0-0 在 Ubuntu 20.04 / 麒麟 V10 上叫 libgdk-pixbuf2.0-0，
# 只写新名字时 dpkg 在用户机器上直接拒装。
set -euo pipefail

DEB="${1:?usage: verify-deb-install.sh <path-to-deb>}"
DEB="$(cd "$(dirname "$DEB")" && pwd)/$(basename "$DEB")"
export DEBIAN_FRONTEND=noninteractive

. /etc/os-release
echo "== Target: ${PRETTY_NAME:-unknown} ($(dpkg --print-architecture))"

# buster 已归档，源要改到 archive.debian.org。
if [ "${VERSION_CODENAME:-}" = "buster" ]; then
    sed -i \
        -e 's|deb.debian.org/debian|archive.debian.org/debian|g' \
        -e 's|security.debian.org/debian-security|archive.debian.org/debian-security|g' \
        -e '/buster-updates/d' \
        /etc/apt/sources.list
fi

apt-get -o Acquire::Check-Valid-Until=false update

echo "== Depends:"
dpkg-deb -f "$DEB" Depends | tr ',' '\n' | sed 's/^ */   /'

PACKAGE="$(dpkg-deb -f "$DEB" Package)"
case "$PACKAGE" in
    gongwen-assistant) EDITION=ai; DESKTOP_ID=cn.localtools.GongwenAssistant ;;
    gongwen-assistant-no-ai) EDITION=no-ai; DESKTOP_ID=cn.localtools.GongwenAssistantNoAi ;;
    *) echo "error: unexpected Debian package: $PACKAGE" >&2; exit 1 ;;
esac
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$PROJECT_ROOT/scripts/package-edition.sh"
APP_DIR="/opt/$PACKAGE"

# 只装 Depends，不装 Recommends：验证的是"没有 Recommends 也能装上并跑起来"。
apt-get install -y --no-install-recommends "$DEB"

status="$(dpkg-query -W -f='${Status}' "$PACKAGE")"
echo "== dpkg status: $status"
test "$status" = "install ok installed"

# 随包 Pi 运行时：Node / host / manifest 必须在，且 Node 能在这个 glibc 基线上跑。
check_package_binary "$APP_DIR/gongwen-assistant"
check_package_resources "$APP_DIR"
binaries=("$APP_DIR/gongwen-assistant")
if [ "$EDITION" = ai ]; then
    PI_DIR="$APP_DIR/runtime/pi"
    test -x "$PI_DIR/bin/node"
    test -f "$PI_DIR/host/main.mjs"
    test -f "$PI_DIR/manifest.json"
    test "$("$PI_DIR/bin/node" --version)" = "v22.22.3"
    binaries+=("$PI_DIR/bin/node")
fi

# 装进来的 Depends 必须覆盖二进制实际链接的所有库。
failed=0
for bin in "${binaries[@]}"; do
    echo "== ldd $bin"
    # 静态链接的 ldd 返回非零并打印 "not a dynamic executable"，没有要找的库，放行。
    out="$(ldd "$bin" 2>&1)" || true
    echo "$out"
    if echo "$out" | grep -qF 'not a dynamic executable'; then
        continue
    fi
    if echo "$out" | grep -F 'not found'; then
        echo "error: $bin has unresolved shared libraries on ${PRETTY_NAME:-this system}" >&2
        failed=1
    fi
done
test -L "/usr/bin/$PACKAGE"
test -f "$APP_DIR/runtime/fonts/GWFangSong.ttf"
test -f "$APP_DIR/runtime/ime/base.txt"
test -f "/usr/share/applications/$DESKTOP_ID.desktop"

# 稿件包 .gwx 的登记：MIME 定义、desktop 声明、文档图标都要装到位。
# 只装 Depends 的容器里未必有 shared-mime-info，装了才顺带核对数据库里的解析结果。
if [ "$EDITION" = ai ]; then
    test -f /usr/share/mime/packages/cn.localtools.GongwenAssistant.xml
    grep -qx 'MimeType=application/x-gongwen-package;'     /usr/share/applications/cn.localtools.GongwenAssistant.desktop
    test -f /usr/share/icons/hicolor/48x48/mimetypes/application-x-gongwen-package.png
    if command -v update-mime-database >/dev/null 2>&1; then
        update-mime-database /usr/share/mime
        test -f /usr/share/mime/application/x-gongwen-package.xml
    fi
fi
exit "$failed"
