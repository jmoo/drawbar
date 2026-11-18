#!/usr/bin/env bash
# Usage: appdir.bash APPDIR NAME PROGRAM [LIBRARY...]
#
# Copies PROGRAM to APPDIR/usr/bin/NAME, and every shared library it or a LIBRARY
# needs, the loader included, flat into APPDIR/usr/lib under the names that ask for
# them, with their RUNPATHs removed. LIBRARY names a library the program `dlopen`s.
# Prints each copied library's name and the store path it came from.
set -euo pipefail

appdir=$1
program_name=$2
program=$3
shift 3

bin="$appdir/usr/bin"
lib="$appdir/usr/lib"
mkdir -p "$bin" "$lib"

# The library each name resolved to, and the names nothing resolved yet.
declare -A resolved=()
declare -A unresolved=()
queue=()

add() {
  local name=$1 path
  path=$(readlink -f "$2")
  if [[ -v resolved[$name] ]]; then
    if [[ ${resolved[$name]} != "$path" ]]; then
      echo "two libraries are named $name: ${resolved[$name]} and $path" >&2
      exit 1
    fi
    return
  fi
  resolved[$name]=$path
  queue+=("$path")
  install -m 755 "$path" "$lib/$name"
}

# The first directory of FILE's RUNPATH that holds NAME.
find_needed() {
  local file=$1 name=$2 dir dirs
  IFS=: read -ra dirs <<<"$(patchelf --print-rpath "$file")"
  for dir in "${dirs[@]}"; do
    dir=${dir//\$ORIGIN/$(dirname "$file")}
    if [[ -e $dir/$name ]]; then
      echo "$dir/$name"
      return
    fi
  done
}

interpreter=$(patchelf --print-interpreter "$program")
add "$(basename "$interpreter")" "$interpreter"
install -m 755 "$program" "$bin/$program_name"
queue+=("$program")
for library in "$@"; do
  add "$(patchelf --print-soname "$library")" "$library"
done

for ((i = 0; i < ${#queue[@]}; i++)); do
  file=${queue[i]}
  for name in $(patchelf --print-needed "$file"); do
    path=$(find_needed "$file" "$name")
    if [[ -n $path ]]; then
      add "$name" "$path"
    else
      unresolved[$name]=$file
    fi
  done
done

# glibc's own libraries carry no RUNPATH, and find each other by name.
for name in "${!unresolved[@]}"; do
  if [[ ! -v resolved[$name] ]]; then
    echo "${unresolved[$name]} needs $name, which its RUNPATH does not hold" >&2
    exit 1
  fi
done

for file in "$bin"/* "$lib"/*; do
  if [[ $file != "$lib/$(basename "$interpreter")" ]]; then
    patchelf --remove-rpath "$file"
  fi
done

for name in "${!resolved[@]}"; do
  printf '%s\t%s\n' "$name" "${resolved[$name]}"
done | sort
