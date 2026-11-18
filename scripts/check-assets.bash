#!/usr/bin/env bash
# nix-deps: cargo git jq
# scripts/assets.bash against a throwaway repository, with `nix` and `gh` replaced by
# stand-ins that answer from files and record what they were asked to do.
set -euo pipefail

scripts=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT

fail() {
  echo "$*" >&2
  exit 1
}

stubs="$root/stubs"
mkdir -p "$root/bin" "$stubs/releases" "$stubs/flakes" "$stubs/store"
export STUBS=$stubs PATH="$root/bin:$PATH"

# `packages/<tag>` lists a tag's flake packages; `store/<tag>/<package>` is a build,
# and `broken` names packages whose build fails.
cat >"$root/bin/nix" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
tag_of() { sed -E 's/.*refs\/tags\/([^#]+)#.*/\1/' <<<"$1"; }
case "$1 $2" in
"eval --impure") echo -n "$STUB_SYSTEM" ;;
"eval --json") cat "$STUBS/flakes/$(tag_of "$3")" ;;
"build --no-link")
  package=${4#*#}
  echo "$(tag_of "$4") $package" >>"$STUBS/builds"
  if grep -qxF "$package" "$STUBS/broken" 2>/dev/null; then exit 1; fi
  echo "$STUBS/store/$(tag_of "$4")/$package"
  ;;
*) exit 1 ;;
esac
EOF

cat >"$root/bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "$1 $2" in
"release view") cat "$STUBS/releases/$3.json" ;;
"release upload")
  name=$(basename "$4")
  echo "$3 $name $(cat "$4")" >>"$STUBS/uploads"
  jq --arg name "$name" '.assets += [{name: $name}]' "$STUBS/releases/$3.json" >"$STUBS/next"
  mv "$STUBS/next" "$STUBS/releases/$3.json"
  ;;
*) exit 1 ;;
esac
EOF
chmod +x "$root/bin/nix" "$root/bin/gh"

repo="$root/repo"
mkdir -p "$repo/scripts" "$repo/crates/app/src"
cp "$scripts/assets.bash" "$scripts/lib.bash" "$repo/scripts/"
cd "$repo"
git init -q -b master
git config user.name 'Release test'
git config user.email 'release-test@example.invalid'
cat >crates/Cargo.toml <<'EOF'
[workspace]
members = ["app"]
resolver = "2"
EOF
cat >crates/app/Cargo.toml <<'EOF'
[package]
name = "app"
version = "0.2.0"
edition = "2021"
EOF
touch crates/app/src/lib.rs
cargo generate-lockfile --manifest-path crates/Cargo.toml --offline
printf '%s\t%s\t%s\t%s\t%s\t%s\n' \
  crate system package file make asset \
  app x86_64-linux app-windows app.exe - app-@version@-x86_64-windows.exe \
  app x86_64-linux app-tarball app-@version@.tar.gz - app-@version@-x86_64-linux.tar.gz \
  app aarch64-darwin app-bundle app.app scripts/pack.bash app-@version@-aarch64-macos.dmg \
  >scripts/assets.tsv
cat >scripts/pack.bash <<'EOF'
#!/usr/bin/env bash
echo "packed $(cat "$1")" >"$2"
EOF
chmod +x scripts/pack.bash
git add .
git commit -qm 'feat: initial release'
git remote add origin "$repo"
git tag app-v0.1.0
git tag app-v0.2.0

# app-v0.1.0 predates the tarball and the bundle, and already carries its executable.
echo '["app-windows"]' >"$stubs/flakes/app-v0.1.0"
echo '{"assets": [{"name": "app-0.1.0-x86_64-windows.exe"}]}' >"$stubs/releases/app-v0.1.0.json"
echo '["app-bundle", "app-tarball", "app-windows"]' >"$stubs/flakes/app-v0.2.0"
echo '{"assets": []}' >"$stubs/releases/app-v0.2.0.json"
for package in app-tarball app-windows app-bundle; do mkdir -p "$stubs/store/app-v0.2.0/$package"; done
echo exe >"$stubs/store/app-v0.2.0/app-windows/app.exe"
echo tarball >"$stubs/store/app-v0.2.0/app-tarball/app-0.2.0.tar.gz"
echo bundle >"$stubs/store/app-v0.2.0/app-bundle/app.app"

uploads() { cat "$stubs/uploads" 2>/dev/null || true; }
builds() { cat "$stubs/builds" 2>/dev/null || true; }

export STUB_SYSTEM=x86_64-linux

bash scripts/assets.bash --tag app-v0.1.0 >old.txt ||
  fail "a release older than its packages must not fail: $(cat old.txt)"
grep -qxF 'app-v0.1.0: its flake has no app-tarball; skipped app-0.1.0-x86_64-linux.tar.gz' old.txt ||
  fail "the skip must name the package the tag lacks: $(cat old.txt)"
grep -qxF 'app-v0.1.0: already carries app-0.1.0-x86_64-windows.exe' old.txt ||
  fail "an attached file must be reported as attached: $(cat old.txt)"
[[ -z $(builds)$(uploads) ]] || fail "nothing was missing, yet: $(builds) $(uploads)"

bash scripts/assets.bash --dry-run >dry.txt
grep -qxF 'app-v0.2.0: would build app-tarball and attach app-0.2.0-x86_64-linux.tar.gz' dry.txt ||
  fail "a dry run names what it would attach: $(cat dry.txt)"
[[ -z $(builds)$(uploads) ]] || fail "a dry run built or uploaded: $(builds) $(uploads)"

echo app-tarball >"$stubs/broken"
if bash scripts/assets.bash 2>broken.txt >/dev/null; then
  fail 'a failed build must fail the run'
fi
grep -qxF '  app-v0.2.0: app-0.2.0-x86_64-linux.tar.gz' broken.txt ||
  fail "the failure must name the file left unattached: $(cat broken.txt)"
[[ $(uploads) == 'app-v0.2.0 app-0.2.0-x86_64-windows.exe exe' ]] ||
  fail "one failed build must not hold back another file: $(uploads)"

rm "$stubs/broken"
bash scripts/assets.bash >/dev/null
bash scripts/assets.bash >again.txt
[[ $(uploads) == 'app-v0.2.0 app-0.2.0-x86_64-windows.exe exe
app-v0.2.0 app-0.2.0-x86_64-linux.tar.gz tarball' ]] ||
  fail "each file is attached once, and only this host's: $(uploads)"
grep -q 'app-0.2.0-aarch64-macos.dmg' again.txt && fail "another host's file was considered: $(cat again.txt)"

STUB_SYSTEM=aarch64-darwin bash scripts/assets.bash >/dev/null
[[ $(uploads | tail -n1) == 'app-v0.2.0 app-0.2.0-aarch64-macos.dmg packed bundle' ]] ||
  fail "a row with a make command attaches what that command wrote: $(uploads)"

echo 'ok: assets attach once per host, skip a package a tag predates, fail on a failed build, and make files by command'
