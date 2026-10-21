#!/usr/bin/env bash
# Wraps the macOS executable in Photo-AIrt.app with its icon, then signs the
# bundle ad hoc (no developer identity; see docs/releases.md).
#   packaging/macos/bundle.sh <photo-airt executable> <version> <output dir>
set -euo pipefail
exe=$1 version=$2 out=$3
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
app="$out/Photo-AIrt.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$exe" "$app/Contents/MacOS/photo-airt"
chmod 755 "$app/Contents/MacOS/photo-airt"
cp "$root/assets/icon/photo-airt.icns" "$app/Contents/Resources/photo-airt.icns"
sed "s/@VERSION@/$version/g" "$here/Info.plist" > "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"
if command -v plutil >/dev/null; then plutil -lint "$app/Contents/Info.plist"; fi
if command -v codesign >/dev/null; then
  codesign --force --deep --sign - "$app"
  codesign --verify --strict --verbose=2 "$app"
fi
echo "$app"
