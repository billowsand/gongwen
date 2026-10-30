#!/usr/bin/env bash
# 在 Arch 系环境里把 Linux 便携目录打成 pacman 包（.pkg.tar.zst）。
#
# Usage: package-arch-pkg.sh <version> <staging-dir> <output-dir> [amd64|arm64]
#
#   staging-dir  scripts/stage-linux-portable.sh 组装出来的便携目录
#   output-dir   .pkg.tar.zst 与 .sha256 的输出位置
#
# 只能在有 pacman / makepkg 的环境里跑（CI 用 archlinux:base 容器，本机可用
# Arch / Omarchy 机器）。依赖列表不是手抄的：ldd 出来的动态库用 pacman -Qoq
# 反查归属包，再补上 winit 运行时 dlopen 的那几个（ldd 看不到）与桌面集成需要
# 的基础包，最后逐个用 pacman -Si 核对名字确实存在——写错一个包名，pacman -U
# 在用户机器上会直接拒装。
set -euo pipefail

VERSION="${1:?usage: package-arch-pkg.sh <version> <staging-dir> <output-dir> [amd64|arm64]}"
STAGING="${2:?missing staging directory}"
OUTPUT_DIR="${3:?missing output directory}"
ARCH="${4:-amd64}"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

case "$ARCH" in
    amd64) PKG_ARCH="x86_64" ;;
    arm64) PKG_ARCH="aarch64" ;;
    *)
        echo "error: unsupported architecture: $ARCH (expected amd64 or arm64)" >&2
        exit 1
        ;;
esac

for tool in makepkg pacman ldd; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "error: $tool not found; run this inside Arch Linux" >&2
        exit 1
    fi
done

if [ ! -d "$STAGING" ]; then
    echo "error: staging directory not found: $STAGING" >&2
    exit 1
fi
STAGING="$(cd "$STAGING" && pwd)"
OUTPUT_DIR="$(cd "$OUTPUT_DIR" 2>/dev/null && pwd || echo "$OUTPUT_DIR")"

if [ ! -x "$STAGING/gongwen-assistant" ]; then
    echo "error: staging binary not found or not executable: $STAGING/gongwen-assistant" >&2
    exit 1
fi
if [ ! -f "$STAGING/runtime/fonts/GWFangSong.ttf" ]; then
    echo "error: staging runtime fonts not found: $STAGING/runtime/fonts" >&2
    exit 1
fi

# ---------------------------------------------------------------- 依赖列表
# 1) ldd 里出现的动态库：反查 pacman 包名。
if ldd "$STAGING/gongwen-assistant" 2>/dev/null | grep -q 'not found'; then
    echo "error: the binary has unresolved shared libraries in this environment:" >&2
    ldd "$STAGING/gongwen-assistant" 2>/dev/null | grep 'not found' >&2
    exit 1
fi

DEPS_RAW=""
add_dep() {
    DEPS_RAW+="$1"$'\n'
}

while read -r lib_path; do
    [ -e "$lib_path" ] || continue
    real_path="$(readlink -f "$lib_path")"
    owner="$(pacman -Qoq "$real_path" 2>/dev/null || true)"
    [ -n "$owner" ] || continue
    add_dep "$owner"
done < <(ldd "$STAGING/gongwen-assistant" 2>/dev/null \
    | awk '{ for (i = 1; i <= NF; i++) if ($i ~ /^\//) { print $i; break } }')

# 2) ldd 看不到、但运行时确实要用的：
#    - winit 的 X11 后端用 x11-dl 动态加载 libX11 / libXcursor / libXrandr / Xi /
#      Xinerama / XKBcommon-x11，缺了会在启动那一刻才 panic（Wayland 会话下
#      XWayland 兜底也要）；
#    - 文件选择走 XDG Desktop Portal（D-Bus），桌面集成要 xdg-utils。
for dep in \
    dbus \
    fontconfig \
    libx11 \
    libxcb \
    libxcursor \
    libxinerama \
    libxi \
    libxkbcommon \
    libxkbcommon-x11 \
    libxrandr \
    xdg-desktop-portal \
    xdg-utils \
    zlib
do
    add_dep "$dep"
done

DEPS=()
while IFS= read -r dep; do
    [ -n "$dep" ] || continue
    if ! pacman -Si "$dep" >/dev/null 2>&1 && ! pacman -Qi "$dep" >/dev/null 2>&1; then
        echo "error: dependency '$dep' does not exist in the Arch repositories" >&2
        exit 1
    fi
    DEPS+=("$dep")
done < <(printf '%s' "$DEPS_RAW" | sed '/^$/d' | sort -u)

echo "== Depends:"
printf '   %s\n' "${DEPS[@]}"

# ---------------------------------------------------------------- PKGBUILD
BUILD_DIR="$(mktemp -d)"
trap 'rm -rf "$BUILD_DIR"' EXIT

DEPS_BLOCK_FILE="$BUILD_DIR/deps.list"
for dep in "${DEPS[@]}"; do
    printf "  '%s'\n" "$dep" >> "$DEPS_BLOCK_FILE"
done

# 用 awk 替换占位符：依赖列表是多行的，sed 的替换里带裸换行不可移植。
awk \
    -v ver="$VERSION" \
    -v arch="$PKG_ARCH" \
    -v depsfile="$DEPS_BLOCK_FILE" \
    -v staging="$STAGING" \
    -v root="$PROJECT_ROOT" '
BEGIN {
    while ((getline dep < depsfile) > 0) {
        deps = deps dep "\n"
    }
    close(depsfile)
}
{
    line = $0
    gsub(/@PKGVER@/, ver, line)
    gsub(/@PKGARCH@/, arch, line)
    gsub(/@PROJECT_ROOT@/, root, line)
    gsub(/@STAGING@/, staging, line)
    if (line == "@DEPS@") {
        printf "%s", deps
        next
    }
    print line
}
' "$PROJECT_ROOT/packaging/arch/PKGBUILD.in" > "$BUILD_DIR/PKGBUILD"

# 安装脚本（刷新 MIME 数据库与图标缓存）与 PKGBUILD 放在一起。
cp "$PROJECT_ROOT/packaging/arch/gongwen-assistant.install" "$BUILD_DIR/gongwen-assistant.install"

# 生成出来的 PKGBUILD 是 bash 脚本，先自己检查一遍语法，别把语法错误带进 makepkg。
bash -n "$BUILD_DIR/PKGBUILD"

# ---------------------------------------------------------------- 打包
# makepkg 不许 root 运行；CI 容器里是 root，就切到普通用户再打。fakeroot 负责
# 把文件属主伪造成 root:root（没有它，包里会记着打包用户的 uid，装到系统上
# 文件属主就不对了）。
BUILD_USER="${GONGWEN_PKG_BUILD_USER:-gongwen-builder}"
if [ "$(id -u)" -eq 0 ]; then
    if ! id "$BUILD_USER" >/dev/null 2>&1; then
        useradd -m -s /bin/bash "$BUILD_USER"
    fi
    BUILD_HOME="$(getent passwd "$BUILD_USER" | cut -d: -f6)"
    BUILD_HOME="${BUILD_HOME:-/home/$BUILD_USER}"
    chown -R "$BUILD_USER" "$BUILD_DIR"
    if command -v runuser >/dev/null 2>&1; then
        runuser -u "$BUILD_USER" -- env HOME="$BUILD_HOME" \
            bash -c "cd '$BUILD_DIR' && makepkg --noconfirm --nodeps --force"
    else
        su "$BUILD_USER" -c "cd '$BUILD_DIR' && makepkg --noconfirm --nodeps --force"
    fi
else
    (cd "$BUILD_DIR" && makepkg --noconfirm --nodeps --force)
fi

PKG_FILE="$(find "$BUILD_DIR" -maxdepth 1 -name 'gongwen-assistant-*.pkg.tar.zst' -print -quit)"
if [ -z "$PKG_FILE" ]; then
    echo "error: makepkg did not produce a package in $BUILD_DIR" >&2
    exit 1
fi

mkdir -p "$OUTPUT_DIR"
PKG_NAME="$(basename "$PKG_FILE")"
cp "$PKG_FILE" "$OUTPUT_DIR/$PKG_NAME"
(
    cd "$OUTPUT_DIR"
    sha256sum "$PKG_NAME" | awk -v name="$PKG_NAME" '{ printf "%s  %s\n", $1, name }' > "$PKG_NAME.sha256"
)

echo "Created: $OUTPUT_DIR/$PKG_NAME"
