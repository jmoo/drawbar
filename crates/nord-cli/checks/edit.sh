#!/usr/bin/env bash
# End-to-end edit contracts against fresh defaults and an editor-produced fixture.
# NORD_RUNNER prefixes invocations of a foreign binary.
set -euo pipefail

[ $# -ge 1 ] || {
  echo "usage: $0 path/to/nord" >&2
  exit 2
}
bin=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
here=$(cd "$(dirname "$0")" && pwd)
# A real editor-written project for the file-verb section; POC_PROJECT
# overrides it where the checkout is not beside the script (the nix build).
: "${POC_PROJECT:=$here/../../nord-format/tests/fixtures/nsmpproj/one-zone.nsmpproj}"

run() { ${NORD_RUNNER:-} "$bin" "$@"; }

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
cd "$scratch"

echo
echo "== nord program edit =="
run program edit --set center_panel.gain=64 -o base.ne5p >/dev/null 2>err.txt || {
  echo "writing a default program failed:"
  cat err.txt
  exit 1
}
# ⚠️ Under Wine, piping directly to grep can miss present output. Capture first;
# the cause is unknown and the file path is reliable.
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

# Transpose owns bytes 0x30..=0x31; any body edit also moves CRC 0x18..=0x1b.
# `cmp -l` counts from one and exits nonzero when it finds the expected differences.
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

# Non-TTY output must omit escapes so Wine/Linux comparison reflects the decode,
# not their consoles.
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

echo
echo "== nord edit: the file verb reaches the registry =="
run edit base.ne5p --set center_panel.gain=100 -o viafile.ne5p >viafile.txt 2>err.txt || {
  echo "nord edit failed on a program file:"
  cat err.txt
  exit 1
}
run verify viafile.ne5p >verified.txt 2>err.txt || {
  echo "a file-verb edit did not round-trip:"
  cat verified.txt err.txt
  exit 1
}
run inspect viafile.ne5p >decoded.txt 2>err.txt
grep -q 'gain: *100' decoded.txt || {
  echo "the file-verb edit did not land:"
  cat decoded.txt
  exit 1
}
echo "ok: nord edit drives the same registry the noun does"

echo
echo "== nord setlist edit =="
run setlist edit --set slot1=2:5 --set slot4=8:50 -o set.ne5t >set.txt 2>err.txt || {
  echo "writing an edited default set list failed:"
  cat err.txt
  exit 1
}
run verify set.ne5t >verified.txt 2>err.txt || {
  echo "a written set list did not round-trip:"
  cat verified.txt err.txt
  exit 1
}
run inspect set.ne5t >decoded.txt 2>err.txt
for want in 'bank 2 slot 5' 'bank 8 slot 50'; do
  grep -q "$want" decoded.txt || {
    echo "the set list edit did not land in the decode (missing '$want'):"
    cat decoded.txt
    exit 1
  }
done
run setlist edit set.ne5t --fields >fields.txt 2>err.txt || {
  echo "setlist --fields failed:"
  cat err.txt
  exit 1
}
grep -q '^slot1 ' fields.txt || {
  echo "setlist --fields does not list the slots:"
  cat fields.txt
  exit 1
}
echo "ok: a set list's four slots edit and round-trip"

echo
echo "== nord edit: a Sample Editor project =="
cp "$POC_PROJECT" proj.nsmpproj
run verify proj.nsmpproj >verified.txt 2>err.txt || {
  echo "the project fixture did not round-trip:"
  cat verified.txt err.txt
  exit 1
}
run edit proj.nsmpproj --fields >fields.txt 2>err.txt || {
  echo "project --fields failed:"
  cat err.txt
  exit 1
}
grep -q '^name ' fields.txt || {
  echo "project --fields does not list the name:"
  cat fields.txt
  exit 1
}
run edit proj.nsmpproj --set name=Renamed -o renamed.nsmpproj >edited.txt 2>err.txt || {
  echo "project edit failed:"
  cat err.txt
  exit 1
}
run verify renamed.nsmpproj >verified.txt 2>err.txt || {
  echo "an edited project did not round-trip:"
  cat verified.txt err.txt
  exit 1
}
run inspect renamed.nsmpproj >decoded.txt 2>err.txt
grep -q 'Renamed' decoded.txt || {
  echo "the project rename did not come back out of the decode:"
  cat decoded.txt
  exit 1
}
echo "ok: a Sample Editor project lists, edits and round-trips"
