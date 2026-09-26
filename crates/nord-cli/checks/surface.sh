#!/usr/bin/env bash
# Checks every noun's verb list against surface.txt.
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
# Use POSIX BRE: BSD sed on macOS treats `\+` as a literal and silently matches nothing.
commands() { sed -n 's/^  \([a-z][a-z-]*\)  .*/\1/p' "$1" | tr '\n' ' '; }

while IFS=: read -r noun want; do
  [ -n "$want" ] || continue
  want=${want# }
  # A noun may be nested (`sample project`), so split it into words.
  read -r -a path <<<"$noun"
  out=surface-${noun:-top}.txt
  out=${out// /-}
  run ${path[@]+"${path[@]}"} --help >"$out" 2>err.txt || {
    echo "nord ${noun:+$noun }--help failed:"
    cat err.txt
    exit 1
  }
  got=$(commands "$out")
  [ "$got" = "$want " ] || {
    echo "nord $noun: command list drifted"
    echo "  want: $want"
    echo "  got:  $got"
    exit 1
  }
done < <(grep -v '^#' "$here/surface.txt")

# `nord raw` stays callable but must not appear in the top-level help.
if grep -q ' raw ' surface-top.txt; then
  echo "nord raw is meant to be hidden from the top-level help"
  exit 1
fi
echo "ok: every noun's verb list matches"
