#!/usr/bin/env bash
# `edit` needs no instrument and no corpus: a fresh default program is a legal
# `.ne5p`, so the whole contract — one field changes, its bytes and the
# checksum move, nothing else does, and the decode reports the new value —
# runs anywhere the binary does.
#
#   checks/edit.sh path/to/nord
#
# NORD_RUNNER prefixes every invocation when the binary is foreign.
set -euo pipefail

bin=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")

run() { ${NORD_RUNNER:-} "$bin" "$@"; }

cd "$(mktemp -d)"

echo
echo "== nord program edit =="
run program edit --set center_panel.gain=64 -o base.ne5p >/dev/null 2>err.txt || {
  echo "writing a default program failed:"
  cat err.txt
  exit 1
}
# ⚠️ Capture to a file and grep the file — never pipe an emulated binary
# straight into `grep`. Under Wine that pipeline reports no match for a line
# the captured output plainly contains, and the failure branch cannot show it
# to you either. Cause not established; the file is the reliable form and
# costs nothing.
run verify base.ne5p >verified.txt 2>err.txt || {
  echo "a written program did not round-trip:"
  cat verified.txt err.txt
  exit 1
}
cat verified.txt

run program edit base.ne5p \
  --set center_panel.transpose=-5 \
  --set center_panel.transpose_enabled=true \
  -o edited.ne5p >edited.txt 2>err.txt || {
  echo "edit failed:"
  cat err.txt
  exit 1
}
cat edited.txt

# `transpose_enabled` is bit 23 and `transpose` bits 24..=27 of a panel
# starting at 0x2e, so bytes 0x30 and 0x31 — plus the body CRC at 0x18..=0x1b,
# which any body change moves. `cmp -l` counts from one.
# `cmp` reports a difference by exiting non-zero, which is the expected
# outcome here, so its status must not end the script.
moved=$( (cmp -l base.ne5p edited.ne5p || true) | awk '{print $1}' | tr '\n' ' ')
[ "$moved" = "25 26 27 28 49 50 " ] || {
  echo "edit touched the wrong bytes"
  echo "  want: 25 26 27 28 49 50   (crc32, then transpose and its enable)"
  echo "  got:  $moved"
  exit 1
}

# The decode has to agree with the edit, or the write went somewhere unrelated.
run inspect edited.ne5p >decoded.txt 2>err.txt || {
  echo "inspect failed on the edited file:"
  cat err.txt
  exit 1
}
grep -q 'transpose: -5  (on)' decoded.txt || {
  echo "the edited value did not come back out of the decode:"
  cat decoded.txt
  exit 1
}

# Presentation is gated on a TTY, and there is none here. This is the check
# the byte-identical Wine/Linux result rests on: an escape sequence surviving
# a pipe would make that comparison depend on the console, not on the decode.
run program edit base.ne5p --set center_panel.gain=1 --dry-run >plain.txt 2>&1
run --color=always program edit base.ne5p --set center_panel.gain=1 --dry-run \
  >colored.txt 2>&1
esc=$(printf '\033')
if grep -q "$esc" plain.txt; then
  echo "piped output carried ANSI escapes:"
  cat -v plain.txt
  exit 1
fi
if ! grep -q "$esc" colored.txt; then
  echo "--color=always emitted no escapes, so the check above proves nothing"
  exit 1
fi

run program edit --fields >fields.txt 2>err.txt || {
  echo "--fields failed:"
  cat err.txt
  exit 1
}
grep -q '^center_panel.transpose ' fields.txt || {
  echo "--fields does not list the field --set just wrote"
  exit 1
}
echo "ok: edit moved exactly the bytes it named, and nothing else"
