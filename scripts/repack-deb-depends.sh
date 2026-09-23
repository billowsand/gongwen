#!/usr/bin/env bash
# 把已发布的 deb 按 packaging/debian/depends.sh 重写 Depends / Recommends 后重新打包，
# 包内文件（二进制、runtime、图标等）原样保留，不重新编译。
#
# Usage: repack-deb-depends.sh <input.deb> <output.deb>
#
# 用于修复已发版本的依赖声明而不动版本号与其他资产（见 .github/workflows/repack-deb.yml）。
set -euo pipefail

IN="${1:?usage: repack-deb-depends.sh <input.deb> <output.deb>}"
OUT="${2:?missing output path}"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# shellcheck source=../packaging/debian/depends.sh
. "$PROJECT_ROOT/packaging/debian/depends.sh"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

dpkg-deb -R "$IN" "$WORK/root"
control="$WORK/root/DEBIAN/control"

# Depends / Recommends 都是单行字段，逐行替换，其余字段不动。
awk -v deps="$DEPENDS" -v recs="$RECOMMENDS" '
    /^Depends:/    { print "Depends: " deps; next }
    /^Recommends:/ { print "Recommends: " recs; next }
    { print }
' "$control" > "$WORK/control.new"
mv "$WORK/control.new" "$control"
chmod 644 "$control"

grep -q "^Depends: $DEPENDS\$" "$control"

echo "== control diff"
diff <(dpkg-deb -f "$IN") <(sed -e '/^ *$/d' "$control") || true

mkdir -p "$(dirname "$OUT")"
# 固定用 xz：新版 dpkg-deb 默认 zstd，Debian buster 等老系统的 dpkg 不认。
dpkg-deb -Zxz --build --root-owner-group "$WORK/root" "$OUT"

# 包内文件必须与原包一致，只有 control 变化。
diff <(dpkg-deb -c "$IN" | awk '{ print $1, $3, $6, $7, $8 }') \
     <(dpkg-deb -c "$OUT" | awk '{ print $1, $3, $6, $7, $8 }')
echo "Repacked: $OUT"
