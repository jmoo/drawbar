#!/usr/bin/env bash
set -euo pipefail

scripts=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/scripts" "$scratch/crates/core/src" "$scratch/crates/app/src"
cp "$scripts/bump.bash" "$scripts/lib.bash" "$scratch/scripts/"
cd "$scratch"
git init -q -b master
git config user.name 'Release test'
git config user.email 'release-test@example.invalid'
cat >crates/Cargo.toml <<'EOF'
[workspace]
members = ["core", "app"]
resolver = "2"
EOF
for crate in core app; do
  cat >"crates/$crate/Cargo.toml" <<EOF
[package]
name = "$crate"
version = "0.1.0"
edition = "2021"
EOF
  echo 'pub fn value() -> u8 { 1 }' >"crates/$crate/src/lib.rs"
done
cat >>crates/app/Cargo.toml <<'EOF'
[dependencies]
core = { path = "../core", version = "0.1.0" }
EOF
cargo generate-lockfile --manifest-path crates/Cargo.toml --offline
git add .
git commit -qm 'feat: initial release'
git tag core-v0.1.0
git tag app-v0.1.0
echo 'pub fn value() -> u8 { 2 }' >crates/core/src/lib.rs
git add .
git commit -qm 'fix(core): return the corrected value'

version_is() {
  local crate=$1 expected=$2 actual
  actual=$(cargo metadata --manifest-path crates/Cargo.toml --no-deps --format-version 1 |
    jq -r --arg name "$crate" '.packages[] | select(.name == $name) | .version')
  if [[ $actual != "$expected" ]]; then
    echo "$crate: expected $expected, got $actual" >&2
    exit 1
  fi
}

bash scripts/bump.bash
version_is core 0.1.1
version_is app 0.1.1
grep -q 'core = .*version = "0.1.1"' crates/app/Cargo.toml || {
  echo 'the dependent must require the patched core version' >&2
  exit 1
}
git diff >first.diff
bash scripts/bump.bash
git diff >second.diff
cmp first.diff second.diff || {
  echo 'repeating an uncommitted catch-up bump changed the release' >&2
  exit 1
}
git add crates
git commit -qm 'chore(release): prepare versions'
bash scripts/bump.bash
git diff --exit-code -- crates

echo 'pub fn extra() {}' >>crates/core/src/lib.rs
git add crates
git commit -qm 'feat(core): expose another operation'
bash scripts/bump.bash
version_is core 0.2.0
version_is app 0.1.1
grep -q 'core = .*version = "0.2.0"' crates/app/Cargo.toml || {
  echo 'the dependent must require the newly bumped core version' >&2
  exit 1
}
git add crates
git commit -qm 'chore(release): prepare the larger release'
bash scripts/bump.bash
git diff --exit-code -- crates

echo 'ok: catch-up bumps converge and propagate dependency requirements'
