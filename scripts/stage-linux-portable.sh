#!/usr/bin/env bash
# 组装 Linux 便携目录：二进制 + runtime（字体、许可证）+ 文档 + AI 技能包。
#
# Usage: stage-linux-portable.sh <binary> <runtime-dir> <output-dir>
#
#   binary      编译好的 gongwen-assistant
#   runtime-dir 解开后的 runtime 目录（里面有 SHA256SUMS.txt）
#   output-dir  便携目录的输出位置（不存在就建）
#
# 与 scripts/package-portable.ps1 保持同一份口径：按 runtime/SHA256SUMS.txt
# 清单取文件并核对校验值，跳过已经下线的 tectonic/ texbundle/ 与旧拼音词库，
# 再补上本项目自带的初始词表、依赖许可证与 AI 技能包。Windows 走 PowerShell
# 那份，Linux（Arch 包 / deb）走这个纯 bash 版本——CI 容器里不必装 PowerShell。
set -euo pipefail

# sha256sum（GNU）与 shasum -a 256（BSD）都能用；两个平台上跑得通。
if command -v sha256sum >/dev/null 2>&1; then
    sha256_of() { sha256sum "$1" | awk '{print $1}'; }
elif command -v shasum >/dev/null 2>&1; then
    sha256_of() { shasum -a 256 "$1" | awk '{print $1}'; }
else
    echo "error: no sha256sum / shasum available" >&2
    exit 1
fi

BINARY="${1:?usage: stage-linux-portable.sh <binary> <runtime-dir> <output-dir>}"
RUNTIME_DIR="${2:?missing runtime directory}"
OUTPUT_DIR="${3:?missing output directory}"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# 打技能包时要 cd 到 skills/ 再引用输出目录，传进来的相对路径先转绝对；
# 相对路径在 `(cd ... && zip ...)` 里会相对新工作目录解析，报「No such file」。
RUNTIME_DIR="$(cd "$RUNTIME_DIR" && pwd)"
BINARY="$(cd "$(dirname "$BINARY")" && pwd)/$(basename "$BINARY")"
mkdir -p "$OUTPUT_DIR"
OUTPUT_DIR="$(cd "$OUTPUT_DIR" && pwd)"

if [ ! -x "$BINARY" ]; then
    echo "error: binary not found or not executable: $BINARY" >&2
    exit 1
fi
if [ ! -f "$RUNTIME_DIR/SHA256SUMS.txt" ]; then
    echo "error: runtime checksum manifest not found: $RUNTIME_DIR/SHA256SUMS.txt" >&2
    exit 1
fi

# PDF 由 Typst 在进程内排版，runtime 里旧有的 tectonic/、texbundle/ 不随包；
# ime/ 是旧拼音词库，也不随包（初始词表由本仓库的 quan.txt 提供）。
is_retired_runtime_asset() {
    case "$1" in
        tectonic/*|texbundle/*|ime/*) return 0 ;;
        licenses/LICENSE.CTAN|licenses/LICENSE.TL|licenses/TECTONIC-LICENSE.txt) return 0 ;;
        *) return 1 ;;
    esac
}

install -m 755 "$BINARY" "$OUTPUT_DIR/gongwen-assistant"

runtime_out="$OUTPUT_DIR/runtime"
mkdir -p "$runtime_out"

while read -r expected relative; do
    case "$expected" in \#*) continue ;; esac
    [ -n "${relative:-}" ] || continue
    relative="${relative#/}"
    if is_retired_runtime_asset "$relative"; then
        continue
    fi
    source_file="$RUNTIME_DIR/$relative"
    if [ ! -f "$source_file" ]; then
        echo "error: runtime asset listed in manifest but missing: $source_file" >&2
        exit 1
    fi
    actual="$(sha256_of "$source_file")"
    if [ "$(printf '%s' "$actual" | tr 'a-f' 'A-F')" != "$(printf '%s' "$expected" | tr 'a-f' 'A-F')" ]; then
        echo "error: SHA-256 mismatch for $source_file" >&2
        exit 1
    fi
    mkdir -p "$runtime_out/$(dirname "$relative")"
    install -m 644 "$source_file" "$runtime_out/$relative"
done < "$RUNTIME_DIR/SHA256SUMS.txt"

# 初始基础词表独立随包，便于更新：项目自维护的 quan.txt 为唯一正式来源，
# mabiao.txt 只作旧机后备；两者都没有时由使用者在设置页导入。
base_table="$PROJECT_ROOT/assets/fuma/quan.txt"
if [ ! -f "$base_table" ]; then
    base_table="$PROJECT_ROOT/assets/fuma/mabiao.txt"
fi
if [ -f "$base_table" ]; then
    mkdir -p "$runtime_out/ime"
    install -m 644 "$base_table" "$runtime_out/ime/base.txt"
fi

# 应用依赖的许可证由本仓库维护；外部 runtime 清单可能尚未包含它。
if [ -f "$RUNTIME_DIR/licenses/NUCLEO-MPL-2.0.txt" ]; then
    mkdir -p "$runtime_out/licenses"
    install -m 644 "$RUNTIME_DIR/licenses/NUCLEO-MPL-2.0.txt" "$runtime_out/licenses/"
fi

for doc in README.md THIRD_PARTY_NOTICES.md LICENSE config.example.json; do
    if [ -f "$PROJECT_ROOT/$doc" ]; then
        install -m 644 "$PROJECT_ROOT/$doc" "$OUTPUT_DIR/$doc"
    fi
done

# AI 技能包：把 skills/gongwen-markdown/ 打成 skills/gongwen-markdown.skill
# 随包分发（.skill 就是 zip）。没有 zip 命令就跳过——应用内的「导出 AI 技能包」
# 按钮用的是编进二进制的同一份文件（src/skill_pack.rs），不影响使用。
skill_name="gongwen-markdown"
skill_source="$PROJECT_ROOT/skills/$skill_name"
if [ -d "$skill_source" ] && command -v zip >/dev/null 2>&1; then
    mkdir -p "$OUTPUT_DIR/skills"
    rm -f "$OUTPUT_DIR/skills/$skill_name.skill"
    (cd "$PROJECT_ROOT/skills" && zip -q -r "$OUTPUT_DIR/skills/$skill_name.skill" "$skill_name" \
        -x '*/__pycache__/*' -x '*/.DS_Store')
fi

# 便携目录自己的校验清单，便于事后核对。
(
    cd "$OUTPUT_DIR"
    find . -type f ! -name SHA256SUMS.txt | LC_ALL=C sort | while read -r file; do
        printf '%s  %s\n' "$(sha256_of "$file")" "${file#./}"
    done > SHA256SUMS.txt
)

echo "Staged: $OUTPUT_DIR"
