#!/usr/bin/env bash
# Usage: check.bash APPIMAGE
#
# Checks that the loader inside APPIMAGE finds every library it needs inside it, then
# runs it the way a host without FUSE does, on a virtual display with no GPU, until
# drawbar draws its window, and checks that every library drawbar loaded came from
# the AppImage, Mesa among them. Needs Xvfb and pgrep.
set -euo pipefail

appimage=$(readlink -f "$1")
work=$(mktemp -d)
cd "$work"

"$appimage" --appimage-extract >/dev/null
root=$work/squashfs-root
loader=$(echo "$root"/usr/lib/ld-linux*)

for file in "$root"/usr/bin/* "$root"/usr/lib/*; do
  [[ $file == "$loader" ]] && continue
  if ! listing=$("$loader" --library-path "$root/usr/lib" --list "$file" 2>&1); then
    echo "${file#"$root"/} needs a library the AppImage lacks: $listing" >&2
    exit 1
  fi
  if outside=$(grep -o '=> /[^ ]*' <<<"$listing" | grep -v "=> $root/"); then
    echo "${file#"$root"/} resolves a library outside the AppImage: $outside" >&2
    exit 1
  fi
done

mkdir framebuffer home
Xvfb -displayfd 3 -screen 0 1280x800x24 -fbdir framebuffer -br -nolisten tcp \
  3>display 2>xvfb.log &
xvfb=$!
trap 'kill "$xvfb" 2>/dev/null || true' EXIT
until [[ -s display ]]; do
  if ! kill -0 "$xvfb" 2>/dev/null; then
    echo "Xvfb did not start:" >&2
    cat xvfb.log >&2
    exit 1
  fi
  sleep 0.1
done

# drawbar's window fills most of the screen, and the root window is black.
screen=framebuffer/Xvfb_screen0
drawn_enough=$(($(stat -c %s "$screen") / 4))

env -u WAYLAND_DISPLAY -u XDG_RUNTIME_DIR DISPLAY=":$(<display)" HOME="$work/home" \
  "$appimage" --appimage-extract-and-run >app.log 2>&1 &
app=$!

pid=
drawn=0
for _ in $(seq 240); do
  sleep 0.5
  if ! kill -0 "$app" 2>/dev/null; then
    wait "$app" && status=0 || status=$?
    echo "drawbar exited with status $status before drawing its window:" >&2
    cat app.log >&2
    exit 1
  fi
  pid=$(pgrep -n -f '/usr/bin/drawbar$') || continue
  drawn=$(tr -d '\0' <"$screen" | wc -c)
  ((drawn >= drawn_enough)) && break
done
if ((drawn < drawn_enough)); then
  echo "drawbar drew $drawn of the $drawn_enough non-black bytes its window needs in 120 s:" >&2
  cat app.log >&2
  exit 1
fi

extracted=$(dirname "$(dirname "$(dirname "$(readlink "/proc/$pid/exe")")")")
mapped=$(awk '$6 ~ /\.so/ { print $6 }' "/proc/$pid/maps" | sort -u)
kill "$pid"
wait "$app" || true

if outside=$(grep -v "^$extracted/" <<<"$mapped"); then
  echo "drawbar loaded libraries from outside the AppImage:" >&2
  echo "$outside" >&2
  exit 1
fi
if ! grep -q '/libgallium-' <<<"$mapped"; then
  echo "drawbar drew its window without loading the AppImage's Mesa" >&2
  exit 1
fi
echo "drawbar drew its window from the AppImage's own libraries"
