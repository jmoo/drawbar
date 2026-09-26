#!/usr/bin/env bash
# End-to-end installed binary checks: surface, USB replay, and file edits.
# NORD_RUNNER supports foreign binaries; POC_* select the fixtures.
set -euo pipefail

[ $# -ge 1 ] || {
  echo "usage: $0 path/to/nord" >&2
  exit 2
}
bin=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
here=$(cd "$(dirname "$0")" && pwd)
: "${POC_SCRIPT:=$here/../../nord-usb/tests/scripts/device/inventory.script}"
export POC_SCRIPT
: "${POC_PROJECT:=$here/../../nord-format/tests/fixtures/nsmpproj/one-zone.nsmpproj}"
export POC_PROJECT

run() { ${NORD_RUNNER:-} "$bin" "$@"; }

# Work in a scratch directory so the caller's directory is left alone.
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
cd "$scratch"

# Emulators need a writable HOME, and Wine refuses a prefix it does not own, so
# both point into the scratch directory. A native binary ignores them.
export HOME=$PWD/home
export WINEPREFIX=$HOME/.wine
export WINEDEBUG=-all
# The Nix sandbox has no network, so keep wineboot from fetching Gecko and Mono.
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
  echo "unexpected output: wanted 'Usage: $name'"
  exit 1
}

# A replayed inventory exercises every layer from the transport up to the CLI.
echo
echo "== $name device status --replay =="
run device status --replay "$POC_SCRIPT" >poc.txt 2>err.txt || {
  echo "device status failed:"
  cat err.txt
  exit 1
}
cat poc.txt

# The last two pin the two `STATUS` unit families: a slot class counts bytes and
# divides into slots, a library class counts blocks and reports a dirty pool.
for want in pianos samples programs 'set lists' '380 / 400 slots' '141 bytes each' \
  '1936 / 2048 blocks' '111 (64 dirty)'; do
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
