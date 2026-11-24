#!/bin/sh
# Builds Sisyphus.app around a universal binary (made with lipo from the
# Apple silicon and Intel builds), then:
# - Sisyphus-macos-universal.dmg: the download, a disk image with the app
#   and a shortcut to Applications to drag it onto
# - Sisyphus-macos-universal.app.tar.gz: the same app for updates (the app
#   unpacks it with /usr/bin/tar, see crates/client/src/update.rs)
#
#   packaging/macos/app.sh <universal binary> <version> <output dir>
#
# Runs on macOS (sips, iconutil, codesign and hdiutil come with it). The
# app is signed ad hoc only (no Apple Developer ID yet), so the first open
# needs System Settings > Privacy & Security > "Open Anyway".
set -eu

binary=$1
version=$2
out=$3
here=$(cd "$(dirname "$0")" && pwd)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

app="$work/Sisyphus.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/sisyphus"
chmod 755 "$app/Contents/MacOS/sisyphus"
sed "s/@VERSION@/$version/g" "$here/Info.plist" > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"

# The icon, in every size macOS asks for.
iconset="$work/Sisyphus.iconset"
mkdir "$iconset"
for size in 16 32 128 256 512; do
  sips -z $size $size "$here/icon-1024.png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z $double $double "$here/icon-1024.png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/Sisyphus.icns"

codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"

mkdir -p "$out"
tar -czf "$out/Sisyphus-macos-universal.app.tar.gz" -C "$work" Sisyphus.app

stage="$work/dmg"
mkdir "$stage"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
hdiutil create -volname Sisyphus -srcfolder "$stage" -ov -format UDZO \
  "$out/Sisyphus-macos-universal.dmg"
