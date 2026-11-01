#!/usr/bin/env bash
# 组装随包 Pi 运行时（Linux ARM64 / x64）：固定 Node 22.22.3 + Pi 1.4.0 host + 生产依赖。
#
# 用法：
#   scripts/stage-pi-runtime.sh --platform linux-arm64|linux-x64 \
#       [--node-archive <tar.xz>] [--output runtime/pi] [--force]
#
# 离线：把官方压缩包放进 tmp/pi-implementation/downloads/ 或传 --node-archive，
# 仍然按 packaging/pi-runtime.json 的固定 SHA256 校验。生产依赖只允许用随包 Node
# 自带的 npm 跑 `npm ci --omit=dev --ignore-scripts --no-audit --no-fund`
# （GONGWEN_PI_NPM_OFFLINE=1 时加 --offline，用完整 npm 缓存离线装）。
# 读配置需要 jq / python3 / node 之一。
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG="$PROJECT_ROOT/packaging/pi-runtime.json"
PLATFORM=""
NODE_ARCHIVE=""
OUTPUT=""
FORCE=0

while [ $# -gt 0 ]; do
    case "$1" in
        --platform) PLATFORM="${2:?}"; shift 2 ;;
        --node-archive) NODE_ARCHIVE="${2:?}"; shift 2 ;;
        --output) OUTPUT="${2:?}"; shift 2 ;;
        --force) FORCE=1; shift ;;
        *) echo "error: 不认识的参数：$1" >&2; exit 2 ;;
    esac
done

if [ -z "$PLATFORM" ]; then
    case "$(uname -m)" in
        aarch64|arm64) PLATFORM="linux-arm64" ;;
        x86_64|amd64) PLATFORM="linux-x64" ;;
        *) echo "error: 认不出当前架构，请传 --platform" >&2; exit 2 ;;
    esac
fi
case "$PLATFORM" in linux-arm64|linux-x64) ;; *) echo "error: Linux staging 只支持 linux-arm64 / linux-x64" >&2; exit 2 ;; esac

json_get() {
    local expr="$1"
    if command -v jq >/dev/null 2>&1; then
        jq -er --arg key "$expr" 'getpath($key | split("."))' "$CONFIG"
    elif command -v python3 >/dev/null 2>&1; then
        python3 - "$CONFIG" "$expr" <<'PY'
import json, sys
data = json.load(open(sys.argv[1], encoding="utf-8"))
for part in sys.argv[2].split("."):
    data = data[part]
print(data)
PY
    elif command -v node >/dev/null 2>&1; then
        node -e 'const fs=require("fs");let v=JSON.parse(fs.readFileSync(process.argv[1],"utf8"));for(const p of process.argv[2].split("."))v=v[p];process.stdout.write(String(v)+"\n")' "$CONFIG" "$expr"
    else
        echo "error: 读 $CONFIG 需要 jq / python3 / node 之一" >&2
        exit 1
    fi
}

json_pi_version() {
    local pkg="$1"
    if command -v jq >/dev/null 2>&1; then
        jq -er --arg k "$pkg" '.pi[$k]' "$CONFIG"
    elif command -v python3 >/dev/null 2>&1; then
        python3 - "$CONFIG" "$pkg" <<'PY'
import json, sys
data = json.load(open(sys.argv[1], encoding="utf-8"))
print(data["pi"][sys.argv[2]])
PY
    elif command -v node >/dev/null 2>&1; then
        node -e 'const fs=require("fs");const c=JSON.parse(fs.readFileSync(process.argv[1],"utf8"));process.stdout.write(String(c.pi[process.argv[2]])+"\n")' "$CONFIG" "$pkg"
    else
        echo "error: 读 $CONFIG 需要 jq / python3 / node 之一" >&2
        exit 1
    fi
}

NODE_VERSION="$(json_get node)"
DOWNLOAD_BASE="$(json_get downloadBase)"
NODE_FILE="$(json_get "platforms.$PLATFORM.file")"
NODE_SHA="$(json_get "platforms.$PLATFORM.sha256")"
NODE_BINARY="$(json_get "platforms.$PLATFORM.binary")"

if [ -z "$OUTPUT" ]; then OUTPUT="$PROJECT_ROOT/runtime/pi"; fi
mkdir -p "$(dirname "$OUTPUT")"
OUTPUT="$(cd "$(dirname "$OUTPUT")" && pwd)/$(basename "$OUTPUT")"
RUNTIME_ROOT="$PROJECT_ROOT/runtime"
TMP_ROOT="$PROJECT_ROOT/tmp"

# 严格子目录 + 拒绝符号链接祖先：不能等于 runtime / tmp 本身，也不能是 runtime-evil。
case "$OUTPUT" in
    "$RUNTIME_ROOT"/*|"$TMP_ROOT"/*) ;;
    *) echo "error: 拒绝在 runtime/ 或 tmp/ 之外组装：$OUTPUT" >&2; exit 1 ;;
esac
[ "$OUTPUT" != "$RUNTIME_ROOT" ] && [ "$OUTPUT" != "$TMP_ROOT" ] || { echo "error: 拒绝使用 runtime / tmp 根目录" >&2; exit 1; }
cursor="$OUTPUT"
while [ -n "$cursor" ] && [ "$cursor" != "/" ]; do
    if [ -L "$cursor" ]; then
        echo "error: 路径包含符号链接，拒绝组装或删除：$cursor" >&2
        exit 1
    fi
    cursor="$(dirname "$cursor")"
done

if [ -e "$OUTPUT" ]; then
    [ -f "$OUTPUT/manifest.json" ] || { echo "error: 拒绝替换不是本脚本生成的目录：$OUTPUT" >&2; exit 1; }
    [ "$FORCE" -eq 1 ] || { echo "error: $OUTPUT 已存在，加 --force 覆盖" >&2; exit 1; }
    rm -rf -- "$OUTPUT"
fi
mkdir -p "$OUTPUT"

if [ -z "$NODE_ARCHIVE" ]; then
    for dir in "${GONGWEN_PI_NODE_CACHE:-}" "$PROJECT_ROOT/tmp/pi-implementation/downloads" "$PROJECT_ROOT/tmp/pi-runtime-cache"; do
        [ -n "$dir" ] || continue
        if [ -f "$dir/$NODE_FILE" ]; then NODE_ARCHIVE="$dir/$NODE_FILE"; break; fi
    done
fi
if [ -z "$NODE_ARCHIVE" ]; then
    if [ "${GONGWEN_PI_OFFLINE:-0}" = "1" ]; then
        echo "error: 离线模式缺少 $NODE_FILE" >&2
        exit 1
    fi
    mkdir -p "$PROJECT_ROOT/tmp/pi-runtime-cache"
    NODE_ARCHIVE="$PROJECT_ROOT/tmp/pi-runtime-cache/$NODE_FILE"
    echo "Downloading $DOWNLOAD_BASE$NODE_FILE"
    curl -fsSL "$DOWNLOAD_BASE$NODE_FILE" -o "$NODE_ARCHIVE"
fi
actual="$(sha256sum "$NODE_ARCHIVE" | awk '{print $1}')"
if [ "$actual" != "$NODE_SHA" ]; then
    echo "error: Node 压缩包 SHA256 不符：$NODE_ARCHIVE" >&2
    exit 1
fi

TEMP="$(mktemp -d)"
cleanup() { rm -rf -- "$TEMP"; }
trap cleanup EXIT

tar -xJf "$NODE_ARCHIVE" -C "$TEMP"
NODE_ROOT="$(find "$TEMP" -mindepth 1 -maxdepth 1 -type d | head -n 1)"
[ -n "$NODE_ROOT" ] || { echo "error: Node 压缩包没有顶层目录" >&2; exit 1; }
NODE_BIN="$NODE_ROOT/$NODE_BINARY"
[ -x "$NODE_BIN" ] || { echo "error: 缺 $NODE_BINARY" >&2; exit 1; }
version="$("$NODE_BIN" --version)"
[ "$version" = "v$NODE_VERSION" ] || { echo "error: Node 报告 $version，期望 v$NODE_VERSION" >&2; exit 1; }

mkdir -p "$OUTPUT/bin"
install -m 755 "$NODE_BIN" "$OUTPUT/bin/node"
mkdir -p "$OUTPUT/licenses"
[ -f "$NODE_ROOT/LICENSE" ] || { echo "error: 官方压缩包缺 LICENSE" >&2; exit 1; }
install -m 644 "$NODE_ROOT/LICENSE" "$OUTPUT/licenses/LICENSE.node"

HOST_OUT="$OUTPUT/host"
mkdir -p "$HOST_OUT"
HOST_BUILD="$TEMP/hostbuild"
mkdir -p "$HOST_BUILD"
cp "$PROJECT_ROOT/pi-host/package.json" "$PROJECT_ROOT/pi-host/package-lock.json" "$HOST_BUILD/"
NPM_CLI="$NODE_ROOT/lib/node_modules/npm/bin/npm-cli.js"
[ -f "$NPM_CLI" ] || { echo "error: 随包 npm 不存在：$NPM_CLI" >&2; exit 1; }
NPM_ARGS=(ci --omit=dev --ignore-scripts --no-audit --no-fund)
if [ "${GONGWEN_PI_NPM_OFFLINE:-0}" = "1" ]; then NPM_ARGS+=(--offline); fi
(cd "$HOST_BUILD" && "$NODE_BIN" "$NPM_CLI" "${NPM_ARGS[@]}")
cp -R "$HOST_BUILD/node_modules" "$HOST_OUT/node_modules"

# 生产依赖版本与 lockfile 对应；不接受开发目录直接复制。
for pkg in "@earendil-works/chord" "@earendil-works/pi-ai" "@earendil-works/pi-durable"; do
    want="$(json_pi_version "$pkg")"
    file="$HOST_OUT/node_modules/$pkg/package.json"
    [ -f "$file" ] || { echo "error: 缺生产依赖 $pkg" >&2; exit 1; }
    got="$("$NODE_BIN" -e 'process.stdout.write(JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).version)' "$file")"
    [ "$got" = "$want" ] || { echo "error: $pkg 是 $got，需要 $want" >&2; exit 1; }
done

cp "$PROJECT_ROOT"/pi-host/src/*.mjs "$HOST_OUT/"
"$NODE_BIN" -e '
const fs = require("fs");
const src = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
const out = { name: src.name, version: src.version, private: true, type: "module", main: "main.mjs", engines: src.engines, dependencies: src.dependencies };
fs.writeFileSync(process.argv[2], JSON.stringify(out, null, 2) + "\n");
' "$PROJECT_ROOT/pi-host/package.json" "$HOST_OUT/package.json"

"$NODE_BIN" "$PROJECT_ROOT/scripts/pi-runtime-licenses.mjs" "$OUTPUT"
"$NODE_BIN" "$PROJECT_ROOT/scripts/pi-runtime-manifest.mjs" "$OUTPUT" "$PLATFORM" "$CONFIG"
"$NODE_BIN" "$PROJECT_ROOT/scripts/pi-runtime-verify.mjs" "$OUTPUT" "$PLATFORM"
bash "$PROJECT_ROOT/scripts/check-node-glibc.sh" "$OUTPUT/bin/node"

echo "Pi runtime staged at $OUTPUT"
