#!/usr/bin/env bash
# nix-deps: cargo git jq
# scripts/bump.bash against throwaway workspaces, one per scenario.
set -euo pipefail

scripts=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT

fail() {
  echo "$*" >&2
  exit 1
}

# `core` and `app`, both released at 0.1.0, with one `fix(core)` commit on top.
scenario() {
  local dir="$root/$1"
  mkdir -p "$dir/scripts" "$dir/crates/core/src" "$dir/crates/app/src"
  cp "$scripts/bump.bash" "$scripts/lib.bash" "$dir/scripts/"
  cd "$dir"
  git init -q -b master
  git config user.name 'Release test'
  git config user.email 'release-test@example.invalid'
  cat >crates/Cargo.toml <<'EOF'
[workspace]
members = ["core", "app"]
resolver = "2"
EOF
  local crate
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
}

version_is() {
  local crate=$1 expected=$2 actual
  actual=$(awk -F'"' '/^\[/ { in_package = ($0 == "[package]") }
                      in_package && /^version = / { print $2; exit }' "crates/$crate/Cargo.toml")
  [[ $actual == "$expected" ]] || fail "$crate: expected $expected, got $actual"
}

requires() {
  local crate=$1 dependency=$2 expected=$3
  grep -q "^$dependency = .*version = \"$expected\"" "crates/$crate/Cargo.toml" ||
    fail "$crate must require $dependency $expected: $(grep "^$dependency = " "crates/$crate/Cargo.toml")"
}

set_version() {
  sed -i.bak "/^\[package\]/,/^\[/ s/^version = \"[^\"]*\"/version = \"$2\"/" "crates/$1/Cargo.toml"
  rm "crates/$1/Cargo.toml.bak"
}

scenario converges
bash scripts/bump.bash
version_is core 0.1.1
version_is app 0.1.1
requires app core 0.1.1
git diff >first.diff
bash scripts/bump.bash
git diff >second.diff
cmp first.diff second.diff ||
  fail 'repeating an uncommitted catch-up bump changed the release'
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
requires app core 0.2.0
git add crates
git commit -qm 'chore(release): prepare the larger release'
bash scripts/bump.bash
git diff --exit-code -- crates

# A dependency bumped by hand still owes its dependents a release, and the
# requirement recorded against it is the version the manifest now holds.
scenario hand_edited_dependency
set_version core 0.1.1
git add crates
git commit -qm 'chore: set the version by hand'
bash scripts/bump.bash
version_is core 0.1.1
version_is app 0.1.1
requires app core 0.1.1

# The PR title says what the branch adds; it must not bump a second time over a
# release already waiting on the base branch.
scenario title_does_not_double_count
bash scripts/bump.bash
git add crates
git commit -qm 'chore(release): prepare versions'
echo 'pub fn extra() {}' >>crates/core/src/lib.rs
git add crates
git commit -qm 'feat(core): expose another operation'
bash scripts/bump.bash
version_is core 0.2.0
git add crates
git commit -qm 'chore(release): prepare the pending release'
git checkout -q -b pr
echo 'pub fn more() {}' >>crates/core/src/lib.rs
git add crates
git commit -qm 'wip: another operation'
bash scripts/bump.bash --title 'feat(core): expose one more operation' --base master
version_is core 0.2.0
version_is app 0.1.1
git diff --exit-code -- crates
bash scripts/bump.bash
version_is core 0.2.0
version_is app 0.1.1

# A version raised past what the commits call for is a deliberate claim.
scenario a_manual_version_is_preserved
set_version core 0.9.0
git add crates
git commit -qm 'chore: claim a larger version'
bash scripts/bump.bash
version_is core 0.9.0
version_is app 0.1.1
requires app core 0.9.0

echo 'ok: bumps converge, catch up hand edits, and propagate dependency requirements'
