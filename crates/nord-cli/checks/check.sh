#!/usr/bin/env bash
# The whole end-to-end run against a built `nord`: it answers --help, replays
# the inventory sweep, holds its command surface still (surface.sh) and edits a
# program byte-exactly (edit.sh).
#
#   checks/check.sh path/to/nord
#
# NORD_RUNNER prefixes every invocation when the binary is foreign — wine,
# qemu-aarch64 — and POC_SCRIPT names the replay fixture, defaulting to the
# checkout's own copy.
set -euo pipefail

[ $# -ge 1 ] || {
  echo "usage: $0 path/to/nord" >&2
  exit 2
}
bin=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
here=$(cd "$(dirname "$0")" && pwd)
: "${POC_SCRIPT:=$here/../../nord-usb/tests/scripts/device/inventory.script}"
export POC_SCRIPT

run() { ${NORD_RUNNER:-} "$bin" "$@"; }

# The scratch files are the check's own; the directory it was invoked from is
# not.
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
cd "$scratch"

# Emulators want a writable HOME; wine additionally refuses a prefix it does
# not own, so point both at the scratch space. Harmless for a native binary.
export HOME=$PWD/home
export WINEPREFIX=$HOME/.wine
export WINEDEBUG=-all
# No network in the nix sandbox — stop wineboot reaching for gecko/mono.
export WINEDLLOVERRIDES="mscoree,mshtml="
mkdir -p "$HOME"

name=$(basename "$bin")

echo "== $name --help =="
run --help >help.txt 2>err.txt || {
  echo "failed to run:"
  cat err.txt
  exit 1
}
cat help.txt
grep -q "Usage: $name" help.txt || {
  echo "unexpected output — wanted 'Usage: $name'"
  exit 1
}

# The POC itself: a full read-only inventory sweep over a replayed exchange,
# exercising transport → wire → session → op → CLI without a device. This is
# the proof that the protocol stack works on this target, not merely that the
# binary starts.
echo
echo "== $name device status --replay =="
run device status --replay "$POC_SCRIPT" >poc.txt 2>err.txt || {
  echo "device status failed:"
  cat err.txt
  exit 1
}
cat poc.txt

for want in pianos samples programs 'set lists' '380 / 400 slots' '141 blocks each'; do
  grep -q "$want" poc.txt || {
    echo "POC output missing '$want'"
    cat poc.txt
    exit 1
  }
done

bash "$here/surface.sh" "$bin"
bash "$here/edit.sh" "$bin"

echo
echo "ok: $name completed the read-only inventory sweep"
