#!/usr/bin/env bash
# 公文助手 · Pi host 本地开发运行时（Linux / macOS）
#
# 用法（在当前 shell 里 source，环境变量只影响这个 shell）：
#   source scripts/pi-dev.sh
#   cargo run -- --new-window
#
# 只做三件事：校验本机已有 host 依赖、把 host 以符号链接放进开发运行时目录
# （不复制 pi-host/src）、把 node 的绝对路径写进 GONGWEN_PI_NODE。不下载、不执行
# npm install、不写 PATH 或用户级环境变量。
#
# 布局：tmp/pi-runtime/host -> pi-host/src，因为 host 按真实路径解析模块，Node 能在
# pi-host/node_modules 找到生产依赖。node 用 GONGWEN_PI_NODE，否则取 PATH 里的 node。
#
# 兼容 macOS 自带 bash 3.2：不用关联数组、不用 ${var,,} / mapfile 这类 4.x 语法。
set -euo pipefail

SCRIPT_PATH="${BASH_SOURCE[0]}"
if [ "$SCRIPT_PATH" = "$0" ]; then
    PI_DEV_SOURCED=0
else
    PI_DEV_SOURCED=1
fi

fail() {
    echo "error: $*" >&2
    # 被 source 时不要把调用者的 shell 退出；直接执行时按普通脚本非零退出。
    if [ "$PI_DEV_SOURCED" -eq 1 ]; then
        return 1
    fi
    exit 1
}

PROJECT_ROOT="$(cd "$(dirname "$SCRIPT_PATH")/.." && pwd -P)"
HOST_SOURCE="$PROJECT_ROOT/pi-host/src"
HOST_ENTRY="$HOST_SOURCE/main.mjs"
HOST_MODULES="$PROJECT_ROOT/pi-host/node_modules"

[ -f "$HOST_ENTRY" ] || fail "找不到 host 入口：$HOST_ENTRY"
[ -d "$HOST_MODULES" ] || fail "缺少 pi-host/node_modules。请先在联网构建阶段到 pi-host 目录执行 npm ci（本脚本不会自动下载依赖）。"

RUNTIME_DIR="${GONGWEN_PI_RUNTIME_DIR:-$PROJECT_ROOT/tmp/pi-runtime}"
case "$RUNTIME_DIR" in
    /*) ;;
    *) RUNTIME_DIR="$PROJECT_ROOT/$RUNTIME_DIR" ;;
esac
mkdir -p "$RUNTIME_DIR"

# host 用符号链接指向源码目录：改 host 代码即时生效，也不用把依赖复制一份。
HOST_LINK="$RUNTIME_DIR/host"
if [ -L "$HOST_LINK" ]; then
    linked_target="$(cd "$HOST_LINK" 2>/dev/null && pwd -P || true)"
    if [ "$linked_target" != "$(cd "$HOST_SOURCE" && pwd -P)" ]; then
        rm -f "$HOST_LINK"
        ln -s "$HOST_SOURCE" "$HOST_LINK"
    fi
elif [ -e "$HOST_LINK" ]; then
    fail "$HOST_LINK 已存在且不是符号链接；host 不复制，请手动删除后重跑"
else
    ln -s "$HOST_SOURCE" "$HOST_LINK"
fi

NODE_BIN="${GONGWEN_PI_NODE:-}"
if [ -z "$NODE_BIN" ]; then
    NODE_BIN="$(command -v node || true)"
fi
[ -n "$NODE_BIN" ] || fail "找不到 Node。请安装 Node >= 22.19，或先设置 GONGWEN_PI_NODE 指向 node 可执行文件。"
case "$NODE_BIN" in
    /*) ;;
    *) NODE_BIN="$(cd "$(dirname "$NODE_BIN")" && pwd)/$(basename "$NODE_BIN")" ;;
esac
[ -x "$NODE_BIN" ] || fail "Node 不可执行：$NODE_BIN"

version="$("$NODE_BIN" --version 2>/dev/null || true)"
case "$version" in
    v[0-9]*) ;;
    *) fail "Node 版本检查失败：$NODE_BIN（输出：${version:-空}）" ;;
esac
version="${version#v}"
major="${version%%.*}"
rest="${version#*.}"
minor="${rest%%.*}"
case "$major" in ''|*[!0-9]*) fail "无法解析 Node 版本：$version" ;; esac
case "$minor" in ''|*[!0-9]*) fail "无法解析 Node 版本：$version" ;; esac
if [ "$major" -lt 22 ] || { [ "$major" -eq 22 ] && [ "$minor" -lt 19 ]; }; then
    fail "Pi host 需要 Node >= 22.19.0，当前是 $version。"
fi

export GONGWEN_PI_RUNTIME_DIR="$RUNTIME_DIR"
export GONGWEN_PI_NODE="$NODE_BIN"

echo "Pi 开发运行时已设置（仅当前 shell）："
echo "export GONGWEN_PI_RUNTIME_DIR=\"$GONGWEN_PI_RUNTIME_DIR\""
echo "export GONGWEN_PI_NODE=\"$GONGWEN_PI_NODE\""
echo ""
echo "host 入口：$RUNTIME_DIR/host/main.mjs"
echo "下一步：cargo run -- --new-window"
if [ "$PI_DEV_SOURCED" -eq 1 ]; then
    echo "以上变量已在当前 shell 生效；新开终端时重新运行：source scripts/pi-dev.sh"
else
    echo "提示：直接执行不会影响你的 shell；要在当前终端生效，请运行：source scripts/pi-dev.sh"
fi
