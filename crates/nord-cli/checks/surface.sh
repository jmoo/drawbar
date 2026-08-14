#!/usr/bin/env bash
# The command tree, asserted rather than described: every noun's verb list is
# pinned exactly against surface.txt.
#
#   checks/surface.sh path/to/nord
#
# NORD_RUNNER prefixes every invocation when the binary is foreign.
set -euo pipefail

bin=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
here=$(cd "$(dirname "$0")" && pwd)

run() { ${NORD_RUNNER:-} "$bin" "$@"; }

cd "$(mktemp -d)"

echo
echo "== the command surface =="
# Clap lists each command as two spaces, the name, then its description.
# Continuation lines are indented further, so they do not match.
# POSIX BRE only — this also runs under macOS's BSD sed, where `\+` is a
# literal and silently matches nothing.
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

# The escape hatch is reachable but unadvertised: hidden is not deprecated, it
# is the escape hatch class-generalisation earns, and it has to stay tested to
# stay usable.
if grep -q ' raw ' surface-top.txt; then
  echo "nord raw is meant to be hidden from the top-level help"
  exit 1
fi
echo "ok: every noun's verb list matches"
