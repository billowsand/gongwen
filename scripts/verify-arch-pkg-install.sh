#!/usr/bin/env bash
# 在干净的 Arch 环境里真装一次 pacman 包，确认它能装上、文件落在该落的地方。
#
# Usage: verify-arch-pkg-install.sh <path-to-pkg.tar.zst>
#
# 以 root 在一次性容器里跑（见 release.yml 的 verify-arch-pkg）。装得上不等于
# 打得开，所以这个脚本只做"静态"检查：依赖能不能解析、文件在不在、属主对不对、
# 动态库有没有缺的。真正起一次窗口的是 scripts/smoke-launch-linux.sh。
set -euo pipefail

PKG="${1:?usage: verify-arch-pkg-install.sh <path-to-pkg.tar.zst>}"
PKG="$(cd "$(dirname "$PKG")" && pwd)/$(basename "$PKG")"

if [ ! -f "$PKG" ]; then
    echo "error: package not found: $PKG" >&2
    exit 1
fi

echo "== Target: Arch Linux ($(uname -m)), $(pacman --version | head -1)"

# 依赖解析：pacman -U 时如果缺依赖会从仓库补装；仓库里没有的依赖名会在这里
# 直接暴露出来（写错包名时 pacman -U 在用户机器上会拒装）。
pacman -Sy --noconfirm
pacman -U --noconfirm "$PKG"

status="$(pacman -Q gongwen-assistant 2>/dev/null || true)"
echo "== pacman -Q: $status"
case "$status" in
    "gongwen-assistant "*) ;;
    *)
        echo "error: the package is not registered with pacman" >&2
        exit 1
        ;;
esac

# 安装位置
APP_DIR="/usr/lib/gongwen-assistant"
test -x "$APP_DIR/gongwen-assistant"
test -L /usr/bin/gongwen-assistant
test -f "$APP_DIR/runtime/fonts/GWFangSong.ttf"
test -f "$APP_DIR/runtime/fonts/FZShuSong.ttf"
test -f "$APP_DIR/runtime/ime/base.txt"

# 桌面集成
test -f /usr/share/applications/cn.localtools.GongwenAssistant.desktop
grep -qx 'MimeType=application/x-gongwen-package;' \
    /usr/share/applications/cn.localtools.GongwenAssistant.desktop
test -f /usr/share/mime/packages/cn.localtools.GongwenAssistant.xml
test -f /usr/share/icons/hicolor/48x48/apps/cn.localtools.GongwenAssistant.png
test -f /usr/share/icons/hicolor/48x48/mimetypes/application-x-gongwen-package.png
test -f /usr/share/licenses/gongwen-assistant/LICENSE

# 属主必须是 root:root：makepkg 不用 fakeroot 时会记成打包用户的 uid。
owner="$(stat -c '%U:%G' "$APP_DIR/gongwen-assistant")"
echo "== owner: $owner"
if [ "$owner" != "root:root" ]; then
    echo "error: installed files are not owned by root ($owner)" >&2
    exit 1
fi

# 依赖的动态库都要在
echo "== ldd $APP_DIR/gongwen-assistant"
out="$(ldd "$APP_DIR/gongwen-assistant" 2>&1)" || true
echo "$out"
if echo "$out" | grep -qF 'not found'; then
    echo "error: the installed binary has unresolved shared libraries" >&2
    exit 1
fi

# 声明的依赖要真的都装上了（pacman -U 装不上就会失败，这里再确认一遍）
echo "== Depends:"
pacman -Qi gongwen-assistant | sed -n '/^Depends On/p'

exit 0
