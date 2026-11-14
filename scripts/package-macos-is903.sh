#!/usr/bin/env bash
# Build the optional root-owned IS903 helper installer. Inputs must be trusted
# release artifacts for the same architecture as the installed application.
set -euo pipefail

binary="${1:?Usage: package-macos-is903.sh <gongwen-assistant> <libusb-1.0.0.dylib> <output.pkg>}"
libusb="${2:?Missing libusb 1.0.30+ dylib}"
output="${3:?Missing output .pkg}"
sign_identity="${GONGWEN_MACOS_SIGN_IDENTITY:?Set the Developer ID Application signing identity}"
installer_identity="${GONGWEN_MACOS_INSTALLER_IDENTITY:?Set the Developer ID Installer signing identity}"
notary_profile="${GONGWEN_MACOS_NOTARY_PROFILE:?Set the notarytool keychain profile}"
project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
version="$(awk -F '"' '/^version = / {print $2; exit}' "$project_root/Cargo.toml")"

temp_dir="$(mktemp -d)"
trap 'rm -rf "$temp_dir"' EXIT
payload="$temp_dir/payload"
destination="$payload/Library/PrivilegedHelperTools/org.gongwen.is903"
install -d -m 755 "$destination"
install -m 755 "$binary" "$destination/gongwen-is903"
install -m 644 "$libusb" "$destination/libusb-1.0.0.dylib"
for file in "$destination/gongwen-is903" "$destination/libusb-1.0.0.dylib"; do
    codesign --force --options runtime --timestamp --sign "$sign_identity" "$file"
    codesign --verify --strict "$file"
done
pkgbuild --root "$payload" --install-location / --ownership recommended \
    --identifier com.billowsand.gongwen.is903-helper --version "$version" \
    --sign "$installer_identity" "$output"
xcrun notarytool submit "$output" --keychain-profile "$notary_profile" --wait
xcrun stapler staple "$output"
