#!/usr/bin/env bash
# nix-deps: cargo jq
# Run from crates/, including in a Nix build whose source is the workspace.
set -euo pipefail

# The Nix build supplies the list its dependency build used.
features=${NORD_TEST_FEATURES-$(cargo metadata --locked --no-deps --format-version 1 | jq -r '
  [.packages[] | .name as $crate | .metadata.nix.testFeatures[]? | "\($crate)/\(.)"]
  | sort | join(",")')}
cargo test --locked --workspace --profile release-tests --features "$features"
bin=$(cargo build --locked --workspace --profile release-tests --features "$features" --bin nord \
  --message-format json | jq -r 'select(.reason == "compiler-artifact" and .target.name == "nord")
  | .executable // empty')
if command -v cygpath >/dev/null; then bin=$(cygpath -u "$bin"); fi
bash nord-cli/checks/check.sh "$bin"
