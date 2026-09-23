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

# 只装 Depends，不装 Recommends：验证的是"没有 Recommends 也能装上并跑起来"。
apt-get install -y --no-install-recommends "$DEB"

status="$(dpkg-query -W -f='${Status}' gongwen-assistant)"
echo "== dpkg status: $status"
test "$status" = "install ok installed"

# 装进来的 Depends 必须覆盖二进制实际链接的所有库。
failed=0
for bin in /opt/gongwen-assistant/gongwen-assistant \
           /opt/gongwen-assistant/runtime/tectonic/tectonic; do
    echo "== ldd $bin"
    out="$(ldd "$bin")"
    echo "$out"
    if echo "$out" | grep -F 'not found'; then
        echo "error: $bin has unresolved shared libraries on ${PRETTY_NAME:-this system}" >&2
        failed=1
    fi
done
test -L /usr/bin/gongwen-assistant
exit "$failed"
