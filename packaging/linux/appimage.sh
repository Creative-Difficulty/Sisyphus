#!/bin/sh
# Builds Sisyphus-linux-<arch>.AppImage from a release binary:
#
#   packaging/linux/appimage.sh <binary> <x86_64|aarch64> <output dir>
#
# Runs on a machine of that architecture (the release workflow's runners).
# Only the binary goes in, no libraries (it links against glibc and loads
# the rest at runtime, like the plain archive). appimagetool and the
# AppImage runtime are pinned by version and SHA-256; the runtime is the
# static type2 runtime, so the AppImage needs no libfuse2.
set -eu

binary=$1
arch=$2
out=$3
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)

tool_version=1.9.1
runtime_version=20251108
case $arch in
  x86_64)
    tool_sha=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
    runtime_sha=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d ;;
  aarch64)
    tool_sha=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158
    runtime_sha=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444 ;;
  *) echo "unknown architecture: $arch" >&2; exit 1 ;;
esac

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fetch() { # url sha256 file
  curl -fsSL -o "$3" "$1"
  echo "$2  $3" | sha256sum -c - >/dev/null
}
fetch "https://github.com/AppImage/appimagetool/releases/download/$tool_version/appimagetool-$arch.AppImage" \
  "$tool_sha" "$work/appimagetool"
fetch "https://github.com/AppImage/type2-runtime/releases/download/$runtime_version/runtime-$arch" \
  "$runtime_sha" "$work/runtime"
chmod +x "$work/appimagetool"

app="$work/Sisyphus.AppDir"
mkdir -p "$app/usr/bin"
cp "$binary" "$app/usr/bin/sisyphus"
chmod 755 "$app/usr/bin/sisyphus"
ln -s usr/bin/sisyphus "$app/AppRun"
cp "$here/sisyphus.desktop" "$app/sisyphus.desktop"
cp "$repo/crates/client/assets/logo.svg" "$app/sisyphus.svg"
ln -s sisyphus.svg "$app/.DirIcon"

mkdir -p "$out"
# The tool is an AppImage itself; unpacking it avoids needing FUSE here.
ARCH=$arch APPIMAGE_EXTRACT_AND_RUN=1 "$work/appimagetool" --no-appstream \
  --runtime-file "$work/runtime" "$app" "$out/Sisyphus-linux-$arch.AppImage"
