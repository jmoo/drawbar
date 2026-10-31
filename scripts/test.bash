#!/usr/bin/env bash
# nix-deps: cargo jq
# Run from crates/, including in a Nix build whose source is the workspace.
set -euo pipefail

features=$(cargo metadata --locked --no-deps --format-version 1 | jq -r '
  [.packages[] | .name as $crate | .metadata.nix.testFeatures[]? | "\($crate)/\(.)"]
  | sort | join(",")')
cargo test --locked --workspace --profile release-tests --features "$features"
cargo build --locked --profile release-tests -p nord-cli
bin=${CARGO_TARGET_DIR:-target}/release-tests/nord
if [[ -f $bin.exe ]]; then bin+=.exe; fi
bash nord-cli/checks/check.sh "$bin"
