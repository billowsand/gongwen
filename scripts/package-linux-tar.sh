#!/usr/bin/env bash
# Build a portable Linux archive used by the Arch/Omarchy binary package.
#
# Usage: package-linux-tar.sh <version> <staging-dir> <output-dir> <amd64|arm64>
set -euo pipefail

VERSION="${1:?usage: package-linux-tar.sh <version> <staging-dir> <output-dir> <amd64|arm64>}"
STAGING="${2:?missing staging directory}"
OUTPUT_DIR="${3:?missing output directory}"
ARCH="${4:?missing architecture}"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

case "$ARCH" in
    amd64|arm64) ;;
    *)
        echo "error: unsupported architecture: $ARCH (expected amd64 or arm64)" >&2
        exit 1
        ;;
esac

if [ ! -x "$STAGING/gongwen-assistant" ]; then
    echo "error: staging binary not found or not executable: $STAGING/gongwen-assistant" >&2
    exit 1
fi
if [ ! -x "$STAGING/runtime/tectonic/tectonic" ]; then
    echo "error: staging runtime tectonic not found or not executable" >&2
    exit 1
fi

BUILD_ROOT="$(mktemp -d)"
trap 'rm -rf "$BUILD_ROOT"' EXIT
ARCHIVE_ROOT="$BUILD_ROOT/gongwen-assistant-$VERSION"
APP_ROOT="$ARCHIVE_ROOT/app"
SHARE_ROOT="$ARCHIVE_ROOT/share"

mkdir -p "$APP_ROOT" "$SHARE_ROOT/applications"
cp -a "$STAGING"/. "$APP_ROOT"/
chmod 755 "$APP_ROOT/gongwen-assistant" "$APP_ROOT/runtime/tectonic/tectonic"
cp "$PROJECT_ROOT/assets/linux/cn.localtools.GongwenAssistant.desktop" \
    "$SHARE_ROOT/applications/"

for size in 16 24 32 48 64 128 256 512; do
    src="$PROJECT_ROOT/assets/app-icon/app-icon-$size.png"
    if [ -f "$src" ]; then
        dest="$SHARE_ROOT/icons/hicolor/${size}x${size}/apps"
        mkdir -p "$dest"
        cp "$src" "$dest/cn.localtools.GongwenAssistant.png"
    fi
done

mkdir -p "$OUTPUT_DIR"
ARCHIVE_NAME="gongwen-assistant-$VERSION-linux-$ARCH-omarchy.tar.gz"
tar -C "$BUILD_ROOT" -czf "$OUTPUT_DIR/$ARCHIVE_NAME" "gongwen-assistant-$VERSION"
sha256sum "$OUTPUT_DIR/$ARCHIVE_NAME" \
    | awk -v name="$ARCHIVE_NAME" '{ printf "%s  %s\n", $1, name }' \
    > "$OUTPUT_DIR/$ARCHIVE_NAME.sha256"

echo "Created: $OUTPUT_DIR/$ARCHIVE_NAME"
