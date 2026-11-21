#!/usr/bin/env bash
# nix-deps: coreutils curl git jq nix
# Draw the guide's screenshots from this checkout's browser build, once in the light
# theme and once in the dark, into docs/src/assets as <name>-light.png and
# <name>-dark.png. Headless Firefox runs each theme in a fresh profile, so nothing of
# this computer's appears; the demo sounds and the practice take are the only files.
#
# The scene below clicks by position in a fixed viewport. A change to drawbar's layout
# can move a target, so look at every image before committing it.
set -euo pipefail

usage() {
  echo "usage: $0 [--out <dir>]" >&2
  exit 2
}

repo="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
out="$repo/docs/src/assets"
while (($#)); do
  case $1 in
  --out)
    [[ $# -ge 2 ]] || usage
    out=$2
    shift
    ;;
  *) usage ;;
  esac
  shift
done

fail() {
  echo "$*" >&2
  exit 1
}

# The viewport in CSS pixels, and the device pixels each one is drawn with.
width=1280 height=720 scale=2
# Where the pointer rests between steps: blank status strip, hovering nothing.
rest_x=700 rest_y=700

# One step per line: `click x y`, `double x y`, `drop <file served beside the app>`,
# or `shot <name> left top right bottom`, in CSS pixels. Each comment names the target.
scene() {
  cat <<'EOF'
click  868 357                     # Start with a demo, on the welcome sheet
click  36  131                     # Demo sounds' disclosure, under This computer
double 470 260                     # drawbar-tine, in the library
click  980 64                      # the inspector's toggle
shot   demo-piano    0   0   1280 290
shot   piano-key-map 268 140 1254 448
drop   tine-take.wav
double 95  239                     # tine-take, under This computer
click  305 139                     # Detect
click  670 320                     # E3, in the take
shot   take-editor   260 245 1272 636
EOF
}

system="$(nix eval --raw --impure --expr builtins.currentSystem)"
mapfile -t built < <(nix build --no-link --print-out-paths \
  "$repo#drawbar-web" \
  "$repo#legacyPackages.$system.firefox" \
  "$repo#legacyPackages.$system.geckodriver" \
  "$repo#legacyPackages.$system.miniserve")
web=${built[0]} firefox=${built[1]}
geckodriver=${built[2]}/bin/geckodriver miniserve=${built[3]}/bin/miniserve
# nixpkgs' Darwin Firefox is an app bundle with no bin/.
for firefox in "$firefox/bin/firefox" "$firefox/Applications/Firefox.app/Contents/MacOS/firefox"; do
  [[ -x $firefox ]] && break
done
[[ -x $firefox ]] || fail "no firefox executable in ${built[1]}"

tmp="$(mktemp -d)"
pids=()
cleanup() {
  ((${#pids[@]} == 0)) || kill "${pids[@]}" 2>/dev/null || true
  rm -rf "$tmp"
}
trap cleanup EXIT

mkdir "$tmp/www"
ln -s "$web"/* "$tmp/www/"
ln -s "$repo/docs/src/assets/tine-take.wav" "$tmp/www/"

free_port() {
  local port
  for _ in {1..20}; do
    port=$((20000 + RANDOM % 20000))
    curl -s -o /dev/null "127.0.0.1:$port" || {
      echo "$port"
      return
    }
  done
  fail "no free port"
}
site_port="$(free_port)"
driver_port="$(free_port)"
"$miniserve" --index index.html --interfaces 127.0.0.1 --port "$site_port" "$tmp/www" \
  >"$tmp/miniserve.log" 2>&1 &
pids+=($!)
"$geckodriver" --host 127.0.0.1 --port "$driver_port" >"$tmp/geckodriver.log" 2>&1 &
pids+=($!)

session=""
wd() {
  local method=$1 path=$2 body=${3-}
  local args=(-sS -m 120 -X "$method" -H 'content-type: application/json')
  [[ -z $body ]] || args+=(-d "$body")
  local answer
  answer="$(curl "${args[@]}" "127.0.0.1:$driver_port/session$session$path")"
  jq -e 'has("value") and ((.value | type) != "object" or (.value | has("error") | not))' \
    <<<"$answer" >/dev/null || fail "webdriver $method $path: $answer"
  jq -c .value <<<"$answer"
}
run_js() {
  wd POST /execute/async "$(jq -nc --arg script "$1" '{script: $script, args: []}')"
}

for port in "$site_port" "$driver_port"; do
  for _ in {1..50}; do
    curl -s -o /dev/null "127.0.0.1:$port" && break
    sleep 0.2
  done
done

pointer() {
  wd POST /actions "$(jq -nc --argjson steps "$1" \
    '{actions: [{type: "pointer", id: "mouse", parameters: {pointerType: "mouse"}, actions: $steps}]}')" \
    >/dev/null
}
move() { jq -nc --argjson x "$1" --argjson y "$2" '{type: "pointerMove", x: $x, y: $y, origin: "viewport"}'; }
press='{"type":"pointerDown","button":0},{"type":"pointerUp","button":0}'

# Screenshots until two in a row match: egui draws nothing new once it is still.
settle() {
  local before="" now
  for _ in {1..120}; do
    sleep 0.5
    now="$(wd GET /screenshot)"
    [[ $now == "$before" ]] && return
    before=$now
  done
  fail "drawbar never came to rest"
}

step() {
  local verb=$1
  shift
  case $verb in
  click) pointer "[$(move "$1" "$2"),$press,$(move "$rest_x" "$rest_y")]" ;;
  double) pointer "[$(move "$1" "$2"),$press,$press,$(move "$rest_x" "$rest_y")]" ;;
  drop)
    run_js "const done = arguments[0];
      fetch('$1').then((r) => r.blob()).then((blob) => {
        const files = new DataTransfer();
        files.items.add(new File([blob], '$1'));
        for (const type of ['dragenter', 'dragover', 'drop']) {
          document.getElementById('drawbar').dispatchEvent(new DragEvent(type, {
            bubbles: true, cancelable: true, clientX: $rest_x, clientY: $rest_y, dataTransfer: files,
          }));
        }
        done(true);
      }, (e) => done(String(e)));" | grep -qx true || fail "could not drop $1"
    ;;
  shot) shoot "$@" ;;
  *) fail "unknown step: $verb" ;;
  esac
  settle
}

# A transparent box over the canvas frames the crop; its element screenshot is what lies
# beneath it.
shoot() {
  local name=$1 left=$2 top=$3 right=$4 bottom=$5
  run_js "const frame = document.createElement('div');
    frame.id = 'shot';
    frame.style.cssText = 'position:fixed;pointer-events:none;left:${left}px;top:${top}px;width:$((right - left))px;height:$((bottom - top))px';
    document.body.append(frame);
    arguments[0](true);" >/dev/null
  local element
  element="$(wd POST /element '{"using":"css selector","value":"#shot"}' | jq -r 'to_entries[0].value')"
  wd GET "/element/$element/screenshot" | jq -r . | base64 -d >"$out/$name-$theme.png"
  run_js "document.getElementById('shot').remove(); arguments[0](true);" >/dev/null
  echo "$out/$name-$theme.png"
}

# Firefox's override of the page's prefers-color-scheme: 0 is dark, 1 is light.
for theme in light dark; do
  case $theme in
  light) scheme=1 ;;
  dark) scheme=0 ;;
  esac
  capabilities="$(jq -nc --arg binary "$firefox" --argjson scheme "$scheme" --arg scale "$scale" '{
    capabilities: {alwaysMatch: {browserName: "firefox", "moz:firefoxOptions": {
      binary: $binary,
      args: ["-headless"],
      prefs: {
        "layout.css.prefers-color-scheme.content-override": $scheme,
        "layout.css.devPixelsPerPx": $scale
      }
    }}}
  }')"
  session=""
  session="/$(wd POST "" "$capabilities" | jq -r .sessionId)"

  wd POST /window/rect "$(jq -nc --argjson w "$width" --argjson h "$height" '{width: $w, height: $h}')" >/dev/null
  inner="$(run_js 'arguments[0]([innerWidth, innerHeight, outerWidth, outerHeight])')"
  wd POST /window/rect "$(jq -c --argjson w "$width" --argjson h "$height" \
    '{width: ($w + .[2] - .[0]), height: ($h + .[3] - .[1])}' <<<"$inner")" >/dev/null
  inner="$(run_js 'arguments[0]([innerWidth, innerHeight, devicePixelRatio])')"
  [[ $inner == "[$width,$height,$scale]" ]] || fail "viewport is $inner, not [$width,$height,$scale]"

  wd POST /url "$(jq -nc --arg url "http://127.0.0.1:$site_port/" '{url: $url}')" >/dev/null
  for _ in {1..240}; do
    started="$(run_js "const splash = document.getElementById('splash');
      arguments[0](splash === null ? 'started' : splash.classList.contains('failed') ? 'failed' : 'loading');")"
    [[ $started == '"loading"' ]] || break
    sleep 0.5
  done
  [[ $started == '"started"' ]] || fail "drawbar did not start: $started"
  pointer "[$(move "$rest_x" "$rest_y")]"
  settle

  while read -r line; do
    read -ra words <<<"${line%%#*}"
    step "${words[@]}"
  done < <(scene)

  wd DELETE "" >/dev/null
  session=""
done
