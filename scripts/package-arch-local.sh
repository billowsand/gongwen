#!/usr/bin/env bash
# 在本机（Arch / Omarchy）把项目打成 pacman 包并就地安装、验证。
#
# Usage: scripts/package-arch-local.sh [options]
#
#   --edition ai|no-ai 选择版本，默认 ai
#   --skip-build     使用已编译的对应版本
#   --no-install     只出包，不 pacman -U 安装（只做包内容自检）
#   --keep-staging   保留中间的便携目录（默认也保留，方便排查）
#   -h | --help      显示本帮助
#
# 流程与 CI 的 release.yml 一致，只是把「容器里做」搬到本机：
#   1) cargo build --release --locked --no-default-features
#      --features ai,linux-portal-dialogs（Arch 变体走 XDG Desktop Portal，不依赖 GTK3）
#   2) scripts/stage-linux-portable.sh 组装便携目录（二进制 + runtime 字体 + 文档 + 技能包）
#   3) scripts/package-arch-pkg.sh 用 makepkg 打成 .pkg.tar.zst
#   4) sudo pacman -U 安装
#   5) scripts/verify-arch-pkg-install.sh --no-install 做静态校验
#      （文件落位、属主 root:root、动态库齐全、依赖可解析）
#
# 依赖名不是手抄的：ldd 出来的库用 pacman -Qoq 反查归属包，见 package-arch-pkg.sh。
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$PROJECT_ROOT"

SKIP_BUILD=0
EDITION=ai
DO_INSTALL=1
KEEP_STAGING=1

while [ $# -gt 0 ]; do
    case "$1" in
        --edition)
            [ "$#" -ge 2 ] || { echo "error: --edition needs an argument" >&2; exit 1; }
            EDITION="$2"
            shift
            ;;
        --skip-build) SKIP_BUILD=1 ;;
        --no-install) DO_INSTALL=0 ;;
        --keep-staging) KEEP_STAGING=1 ;;
        -h|--help) sed -n '2,28p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) echo "error: unknown option: $1" >&2; exit 1 ;;
    esac
    shift
done

source "$PROJECT_ROOT/scripts/package-edition.sh"
PACKAGE="gongwen-assistant${EDITION_SUFFIX}"

for tool in cargo makepkg pacman zstd sha256sum; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "error: $tool not found; this script needs Arch Linux (pacman/makepkg)" >&2
        exit 1
    fi
done

# 版本号取自 Cargo.toml 的根包，与发布版本保持一致。
VERSION="$(awk '/^version = "/ { gsub(/^version = "|"/, ""); print; exit }' Cargo.toml)"
if [ -z "$VERSION" ]; then
    echo "error: cannot read version from Cargo.toml" >&2
    exit 1
fi

# 便携目录用的 runtime（字体、许可证、SHA256SUMS.txt）。仓库里带清单，字体是
# 本机开发副本（.gitignore 忽略）。缺字体时给出手工补齐的办法。
RUNTIME_DIR="${GONGWEN_RUNTIME_DIR:-$PROJECT_ROOT/runtime}"
if [ ! -f "$RUNTIME_DIR/SHA256SUMS.txt" ]; then
    echo "error: runtime manifest not found: $RUNTIME_DIR/SHA256SUMS.txt" >&2
    exit 1
fi
if [ ! -f "$RUNTIME_DIR/fonts/GWFangSong.ttf" ]; then
    echo "error: runtime fonts missing: $RUNTIME_DIR/fonts/GWFangSong.ttf" >&2
    echo "       从 gongwen-runtime v0.8.2 的 runtime.zip 解出 fonts/ 到该目录即可" >&2
    exit 1
fi

BINARY="$PROJECT_ROOT/target/release/gongwen-assistant"

# ---------------------------------------------------------------- 1) 编译
if [ "$SKIP_BUILD" -eq 0 ]; then
    echo "== Building release binary (linux-portal-dialogs)"
    if [ "$EDITION" = ai ]; then
        cargo build --release --locked --no-default-features --features ai,linux-portal-dialogs
    else
        cargo build --release --locked --target-dir target/no-ai-package --no-default-features --features task-assistant,linux-portal-dialogs
        BINARY="$PROJECT_ROOT/target/no-ai-package/release/gongwen-assistant"
    fi
elif [ "$EDITION" = no-ai ]; then
    BINARY="$PROJECT_ROOT/dist/editions/gongwen-assistant-no-ai"
fi
if [ ! -x "$BINARY" ]; then
    echo "error: binary not found: $BINARY" >&2
    exit 1
fi

# ---------------------------------------------------------------- 2) 便携目录
STAGING_DIR="$PROJECT_ROOT/dist/staging-linux-x86_64$EDITION_SUFFIX"
echo "== Staging portable directory: $STAGING_DIR"
rm -rf "$STAGING_DIR"
bash "$PROJECT_ROOT/scripts/stage-linux-portable.sh" \
    "$BINARY" "$RUNTIME_DIR" "$STAGING_DIR" "$EDITION"

# ---------------------------------------------------------------- 3) 打包
PKG_OUTPUT_DIR="$PROJECT_ROOT/dist"

# 本机已经装过同版本时，pacman -U 会因版本相同而跳过（up to date -- skipping）。
# 这里把 pkgrel 自动加一，让它成为一次真正的升级。
PKGREL="${GONGWEN_PKGREL:-1}"
installed="$(pacman -Q "$PACKAGE" 2>/dev/null || true)"
if [ -n "$installed" ]; then
    inst_ver="${installed#"$PACKAGE "}"
    inst_ver="${inst_ver%-*}"
    inst_rel="${installed##*-}"
    if [ "$inst_ver" = "$VERSION" ] && [ "${GONGWEN_PKGREL:-}" = "" ]; then
        PKGREL="$((inst_rel + 1))"
        echo "== Installed version is $installed; building with pkgrel=$PKGREL"
    fi
fi

echo "== Building pacman package (makepkg)"
GONGWEN_PKGREL="$PKGREL" bash "$PROJECT_ROOT/scripts/package-arch-pkg.sh" \
    "$VERSION" "$STAGING_DIR" "$PKG_OUTPUT_DIR" amd64 "$EDITION"

PKG_FILE="$PKG_OUTPUT_DIR/$PACKAGE-$VERSION-$PKGREL-x86_64.pkg.tar.zst"
if [ ! -f "$PKG_FILE" ]; then
    echo "error: no .pkg.tar.zst produced" >&2
    exit 1
fi
echo "== Package: $PKG_FILE"
sha256sum "$PKG_FILE"

# ---------------------------------------------------------------- 4) 安装
if [ "$DO_INSTALL" -eq 1 ]; then
    echo "== Installing with pacman -U (需要管理员口令)"
    # 提权方式：有免密 sudo 就用它；否则退到 pkexec（图形会话里弹口令框）。
    if [ "$(id -u)" -eq 0 ]; then
        pacman -U --noconfirm --needed "$PKG_FILE"
        echo "== Verifying installation"
        bash "$PROJECT_ROOT/scripts/verify-arch-pkg-install.sh" --no-install "$PKG_FILE"
    elif sudo -n true 2>/dev/null; then
        sudo pacman -U --noconfirm --needed "$PKG_FILE"
        echo "== Verifying installation"
        sudo bash "$PROJECT_ROOT/scripts/verify-arch-pkg-install.sh" --no-install "$PKG_FILE"
    elif command -v pkexec >/dev/null 2>&1; then
        pkexec pacman -U --noconfirm --needed "$PKG_FILE"
        echo "== Verifying installation"
        pkexec bash "$PROJECT_ROOT/scripts/verify-arch-pkg-install.sh" --no-install "$PKG_FILE"
    else
        echo "error: need root to install; no passwordless sudo and no pkexec" >&2
        echo "       请手动执行: sudo pacman -U --noconfirm --needed $PKG_FILE" >&2
        exit 1
    fi
else
    echo "== Skipping install (--no-install); 包内容自检："
    tar --zstd -tf "$PKG_FILE" | sed -n '1,20p'
    echo "== 共 $(tar --zstd -tf "$PKG_FILE" | wc -l) 个文件"
fi

if [ "$KEEP_STAGING" -eq 0 ]; then
    rm -rf "$STAGING_DIR"
fi

echo "== Done: $PKG_FILE"
