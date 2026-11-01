#!/usr/bin/env bash
# 核查随包 Node 的 glibc / libstdc++ 与解释器要求。
#
# Rust 二进制按 GLIBC 2.28 构建，不等于 Node 也兼容 2.28：官方 Node 压缩包
# 的构建基线随平台不同。这个脚本在 Linux staging 后跑一次，读 ELF 符号版本与
# 解释器，要求最高 GLIBC 符号版本不超过 2.28（deb 的 GLIBC2.28 约束）。
#
# 用法：scripts/check-node-glibc.sh <node-binary> [max-glibc]
set -euo pipefail

NODE_BIN="${1:?usage: check-node-glibc.sh <node-binary> [max-glibc]}"
MAX_GLIBC="${2:-2.28}"
MAX_GLIBCXX="${3:-3.4.25}"

if [ ! -f "$NODE_BIN" ]; then
    echo "error: node binary not found: $NODE_BIN" >&2
    exit 1
fi
for tool in objdump readelf; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "error: $tool not found (binutils required to audit Node glibc)" >&2
        exit 1
    fi
done

highest="$(objdump -T "$NODE_BIN" 2>/dev/null | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/GLIBC_//' | sort -Vu | tail -n 1)"
if [ -z "$highest" ]; then
    echo "error: cannot read GLIBC symbol versions from $NODE_BIN" >&2
    exit 1
fi
# sort -V 比较：最高版本不能高于允许值。
if [ "$(printf '%s\n%s\n' "$highest" "$MAX_GLIBC" | sort -Vu | tail -n 1)" != "$MAX_GLIBC" ] && [ "$highest" != "$MAX_GLIBC" ]; then
    echo "error: bundled Node requires GLIBC $highest, but the package targets $MAX_GLIBC" >&2
    exit 1
fi

glibcxx="$(objdump -T "$NODE_BIN" 2>/dev/null | grep -o 'GLIBCXX_[0-9][0-9.]*' | sed 's/GLIBCXX_//' | sort -Vu | tail -n 1 || true)"
if [ -n "$glibcxx" ]; then
    if [ "$(printf '%s\n%s\n' "$glibcxx" "$MAX_GLIBCXX" | sort -Vu | tail -n 1)" != "$MAX_GLIBCXX" ]; then
        echo "error: bundled Node requires GLIBCXX_$glibcxx, target supports $MAX_GLIBCXX" >&2
        exit 1
    fi
fi
interpreter="$(readelf -l "$NODE_BIN" 2>/dev/null | sed -n 's/.*interpreter: \(.*\)\]/\1/p' | head -n 1)"
if [ -z "$interpreter" ] || [ ! -e "$interpreter" ]; then
    echo "error: bundled Node interpreter is unavailable: $interpreter" >&2
    exit 1
fi
if ldd "$NODE_BIN" 2>&1 | grep -q 'not found'; then
    echo "error: bundled Node has unresolved shared libraries" >&2
    exit 1
fi
echo "Node glibc check: highest GLIBC_$highest <= $MAX_GLIBC"
