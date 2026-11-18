#!/usr/bin/env bash
# Usage: check.bash APPIMAGE [LAUNCHER...]
#
# Extracts APPIMAGE, as a host without FUSE does, and runs what it holds on a virtual
# display with no GPU until drawbar draws its window. LAUNCHER is the command that
# starts it on a host with no loader at the standard path. Needs Xvfb.
set -euo pipefail

appimage=$(readlink -f "$1")
shift
work=$(mktemp -d)
cd "$work"

"$appimage" --appimage-extract >/dev/null

mkdir framebuffer home
Xvfb -displayfd 3 -screen 0 1280x800x24 -fbdir framebuffer -br -nolisten tcp \
  3>display 2>xvfb.log &
xvfb=$!
app=
trap 'kill "$xvfb" ${app:+"$app"} 2>/dev/null || true' EXIT
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
  "$@" "$work/squashfs-root/AppRun" >app.log 2>&1 &
app=$!

drawn=0
for _ in $(seq 240); do
  sleep 0.5
  if ! kill -0 "$app" 2>/dev/null; then
    wait "$app" && status=0 || status=$?
    echo "drawbar exited with status $status before drawing its window:" >&2
    cat app.log >&2
    exit 1
  fi
  drawn=$(tr -d '\0' <"$screen" | wc -c)
  ((drawn >= drawn_enough)) && break
done
if ((drawn < drawn_enough)); then
  echo "drawbar drew $drawn of the $drawn_enough non-black bytes its window needs in 120 s:" >&2
  cat app.log >&2
  exit 1
fi
echo "drawbar drew its window"
