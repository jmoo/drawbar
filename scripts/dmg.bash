#!/usr/bin/env bash
# nix-deps: coreutils git nix
# Pack drawbar.app into the disk image a release carries, and check the copy in it.
# macOS only: hdiutil, codesign and plutil are the system's.
set -euo pipefail

usage() {
  echo "usage: $0 [--from <installable>] [--out <path>]" >&2
  exit 2
}

repo="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
from="$repo#nord.drawbar-app" out=""
while (($#)); do
  case $1 in
  --from)
    [[ $# -ge 2 ]] || usage
    from=$2
    shift
    ;;
  --out)
    [[ $# -ge 2 ]] || usage
    out=$2
    shift
    ;;
  *) usage ;;
  esac
  shift
done

fail() {
  echo "$*" >&2
  exit 1
}

tmp="$(mktemp -d)"
mounted=""
cleanup() {
  [[ -z $mounted ]] || hdiutil detach -quiet "$mounted" || hdiutil detach -force -quiet "$mounted"
  rm -rf "$tmp"
}
trap cleanup EXIT

built="$(nix build --no-link --print-out-paths "$from")"
[[ -d "$built/drawbar.app" ]] || fail "$from holds no drawbar.app"
version="$(plutil -extract CFBundleShortVersionString raw "$built/drawbar.app/Contents/Info.plist")"
out="${out:-drawbar-$version-aarch64-macos.dmg}"

mkdir "$tmp/image"
cp -R "$built/drawbar.app" "$tmp/image/"
chmod -R u+w "$tmp/image"
# The linker signs the binary alone, and a bundle verifies only once a signature
# seals its Info.plist and resources too. Ad hoc: no identity, so Gatekeeper still asks.
codesign --force --sign - "$tmp/image/drawbar.app"
ln -s /Applications "$tmp/image/Applications"
hdiutil create -quiet -ov -volname drawbar -srcfolder "$tmp/image" -format ULMO "$out"

mkdir "$tmp/mount"
hdiutil attach -quiet -readonly -nobrowse -noautoopen -mountpoint "$tmp/mount" "$out"
mounted="$tmp/mount"
[[ -x "$mounted/drawbar.app/Contents/MacOS/drawbar" ]] || fail "$out holds no drawbar.app"

# The checks run on a copy taken out of the image, as a user's would be.
ditto "$mounted/drawbar.app" "$tmp/drawbar.app"
app="$tmp/drawbar.app"

plutil -lint -s "$app/Contents/Info.plist" || fail "$out: drawbar.app's Info.plist is malformed"

# ⚠️ An arm64 binary whose signature is broken is killed at launch, with no dialog.
codesign --verify --deep --strict "$app" ||
  fail "$out: drawbar.app's signature does not verify, so macOS would kill it at launch"

# A missing library or a rejected signature ends the process before it opens a window.
# HOME keeps the app's settings and library away from the caller's.
mkdir "$tmp/home"
HOME="$tmp/home" "$app/Contents/MacOS/drawbar" >"$tmp/launch.log" 2>&1 &
pid=$!
sleep 5
if ! kill -0 "$pid" 2>/dev/null; then
  status=0
  wait "$pid" || status=$?
  cat "$tmp/launch.log" >&2
  fail "$out: drawbar exited with status $status within 5 s of launch"
fi
kill "$pid"
wait "$pid" 2>/dev/null || true

echo "$out: drawbar $version, $(du -h "$out" | cut -f1)"
