#!/usr/bin/env bash
# The command tree, asserted rather than described: every noun's verb list is
# pinned exactly against surface.txt.
#
#   checks/surface.sh path/to/nord
#
# NORD_RUNNER prefixes every invocation when the binary is foreign.
set -euo pipefail

[ $# -ge 1 ] || {
  echo "usage: $0 path/to/nord" >&2
  exit 2
}
bin=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
here=$(cd "$(dirname "$0")" && pwd)

run() { ${NORD_RUNNER:-} "$bin" "$@"; }

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
cd "$scratch"

echo
echo "== the command surface =="
# Clap commands have exactly two leading spaces; continuation lines have more.
# Use POSIX BRE: macOS BSD sed treats `\+` as a literal and silently finds nothing.
commands() { sed -n 's/^  \([a-z][a-z-]*\)  .*/\1/p' "$1" | tr '\n' ' '; }

while IFS=: read -r noun want; do
  [ -n "$want" ] || continue
  want=${want# }
  out=surface-${noun:-top}.txt
  if [ -n "$noun" ]; then
    run "$noun" --help >"$out" 2>err.txt || {
      echo "nord $noun --help failed:"
      cat err.txt
      exit 1
    }
  else
    run --help >"$out" 2>err.txt || {
      echo "nord --help failed:"
      cat err.txt
      exit 1
    }
  fi
  got=$(commands "$out")
  [ "$got" = "$want " ] || {
    echo "nord $noun: command list drifted"
    echo "  want: $want"
    echo "  got:  $got"
    exit 1
  }
done < <(grep -v '^#' "$here/surface.txt")

# The raw escape hatch must remain reachable but absent from advertised commands.
if grep -q ' raw ' surface-top.txt; then
  echo "nord raw is meant to be hidden from the top-level help"
  exit 1
fi
echo "ok: every noun's verb list matches"
