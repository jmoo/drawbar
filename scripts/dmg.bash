#!/usr/bin/env bash
# nix-deps: coreutils git nix
# Pack drawbar.app into the disk image a release carries, and check the copy in it.
# With no arguments the app is this checkout's, and the image lands in the current
# folder under its release name.
#
# DRAWBAR_SIGN_IDENTITY names a Developer ID Application identity in the keychain.
# Without it the app is signed ad hoc, and Gatekeeper refuses the download.
# DRAWBAR_NOTARY_PROFILE names a notarytool keychain profile. With it Apple notarizes
# the image, and Gatekeeper opens the download without a warning.
#
# macOS only: hdiutil, codesign, plutil, ditto, spctl and xcrun are the system's.
set -euo pipefail

usage() {
  echo "usage: $0 [<drawbar.app> <dmg>]" >&2
  exit 2
}

case $# in
0) built="" out="" ;;
2) built=$1 out=$2 ;;
*) usage ;;
esac

fail() {
  echo "$*" >&2
  exit 1
}

# Nix's shell points xcrun at its own SDK, which holds no notarytool or stapler.
xcrun() {
  env -u DEVELOPER_DIR -u SDKROOT /usr/bin/xcrun "$@"
}

identity=${DRAWBAR_SIGN_IDENTITY:-}
profile=${DRAWBAR_NOTARY_PROFILE:-}
[[ -z $profile || -n $identity ]] ||
  fail "DRAWBAR_NOTARY_PROFILE needs DRAWBAR_SIGN_IDENTITY: Apple notarizes only what a Developer ID signed"

tmp="$(mktemp -d)"
mounted=""
cleanup() {
  [[ -z $mounted ]] || /usr/bin/hdiutil detach -quiet "$mounted" || /usr/bin/hdiutil detach -force -quiet "$mounted"
  rm -rf "$tmp"
}
trap cleanup EXIT

if [[ -z $built ]]; then
  repo="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
  built="$(nix build --no-link --print-out-paths "$repo#drawbar-app")/drawbar.app"
fi
[[ -x "$built/Contents/MacOS/drawbar" ]] || fail "$built is not drawbar.app"
version="$(/usr/bin/plutil -extract CFBundleShortVersionString raw "$built/Contents/Info.plist")"
out="${out:-drawbar-$version-aarch64-macos.dmg}"

mkdir "$tmp/image"
cp -R "$built" "$tmp/image/drawbar.app"
chmod -R u+w "$tmp/image"
# The linker signs the binary alone, and a bundle verifies only once a signature
# seals its Info.plist and resources too.
if [[ -n $identity ]]; then
  # The notary service refuses a signature without the hardened runtime or a timestamp.
  /usr/bin/codesign --force --options runtime --timestamp --sign "$identity" "$tmp/image/drawbar.app"
else
  /usr/bin/codesign --force --sign - "$tmp/image/drawbar.app"
fi
ln -s /Applications "$tmp/image/Applications"
/usr/bin/hdiutil create -quiet -ov -volname drawbar -srcfolder "$tmp/image" -format ULMO "$out"
[[ -z $identity ]] || /usr/bin/codesign --timestamp --sign "$identity" "$out"

if [[ -n $profile ]]; then
  # ⚠️ Reported, not measured here: notarytool exits 0 for an image the service rejected.
  xcrun notarytool submit "$out" --keychain-profile "$profile" --wait \
    --output-format json >"$tmp/notary.json"
  verdict="$(/usr/bin/plutil -extract status raw "$tmp/notary.json")"
  if [[ $verdict != Accepted ]]; then
    xcrun notarytool log --keychain-profile "$profile" \
      "$(/usr/bin/plutil -extract id raw "$tmp/notary.json")" >&2 || true
    fail "$out: Apple's notary service answered $verdict"
  fi
  xcrun stapler staple -q "$out"
fi

mkdir "$tmp/mount"
/usr/bin/hdiutil attach -quiet -readonly -nobrowse -noautoopen -mountpoint "$tmp/mount" "$out"
mounted="$tmp/mount"
[[ -x "$mounted/drawbar.app/Contents/MacOS/drawbar" ]] || fail "$out holds no drawbar.app"

# The checks run on a copy taken out of the image, as a user's would be.
/usr/bin/ditto "$mounted/drawbar.app" "$tmp/drawbar.app"
app="$tmp/drawbar.app"

/usr/bin/plutil -lint -s "$app/Contents/Info.plist" || fail "$out: drawbar.app's Info.plist is malformed"

# ⚠️ An arm64 binary whose signature is broken is killed at launch, with no dialog.
/usr/bin/codesign --verify --deep --strict "$app" ||
  fail "$out: drawbar.app's signature does not verify, so macOS would kill it at launch"

if [[ -n $profile ]]; then
  /usr/sbin/spctl --assess --type open --context context:primary-signature "$out" ||
    fail "$out: Gatekeeper would refuse the image"
  /usr/sbin/spctl --assess --type execute "$app" ||
    fail "$out: Gatekeeper would refuse drawbar.app"
fi

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

if [[ -n $profile ]]; then
  signed="notarized"
elif [[ -n $identity ]]; then
  signed="signed, not notarized"
else
  signed="signed ad hoc"
fi
echo "$out: drawbar $version, $(du -h "$out" | cut -f1), $signed"
