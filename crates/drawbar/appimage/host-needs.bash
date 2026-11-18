#!/usr/bin/env bash
# Usage: host-needs.bash PROGRAM LOADER GLIBC LIBRARY...
#
# Fails unless PROGRAM asks no more of the distribution it runs on than this: LOADER
# starts it, it searches no RUNPATH, it links no library but a LIBRARY, and it requires
# no glibc symbol version newer than GLIBC. Needs patchelf and readelf.
set -euo pipefail

program=$1
loader=$2
glibc=$3
shift 3

fail() {
  echo "$program $*" >&2
  exit 1
}

interpreter=$(patchelf --print-interpreter "$program")
[[ $interpreter == "$loader" ]] || fail "is started by $interpreter, not $loader"

runpath=$(patchelf --print-rpath "$program")
[[ -z $runpath ]] || fail "searches $runpath for its libraries"

for needed in $(patchelf --print-needed "$program"); do
  [[ " $* " == *" $needed "* ]] || fail "links $needed, which is none of: $*"
done

# ⚠️ The loader refuses a program over a version its libc lacks, even when every
# symbol at that version is a weak reference.
newest=$(readelf --version-info --wide "$program" |
  sed -n 's/.*Name: GLIBC_\([0-9][0-9.]*\) .*/\1/p' | sort -V | tail -n1)
[[ -n $newest ]] || fail "names no glibc symbol version"
[[ $(printf '%s\n' "$glibc" "$newest" | sort -V | tail -n1) == "$glibc" ]] ||
  fail "requires glibc $newest, newer than $glibc"
