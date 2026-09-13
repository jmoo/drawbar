#!/usr/bin/env bash
# nix-deps: git nix
# Assemble the deployed Pages tree: a released app, this checkout's guide.
set -euo pipefail

usage() {
  echo "usage: $0 [--tag <drawbar-v*>] [--out <path>]" >&2
  exit 2
}

tag="" out=./result-site
while (($#)); do
  case $1 in
  --tag)
    [[ $# -ge 2 ]] || usage
    tag=$2
    shift
    ;;
  --out)
    [[ $# -ge 2 ]] || usage
    out=$2
    shift
    ;;
  *) usage ;;
  esac
  shift
done

repo="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"

if [[ -z $tag ]]; then
  git -C "$repo" fetch --quiet --tags origin
  tag="$(git -C "$repo" tag --list 'drawbar-v*' --sort=-v:refname | head -n1)"
  [[ -n $tag ]] || {
    echo "no drawbar-v* tag to publish; release drawbar before deploying the site" >&2
    exit 1
  }
elif ! git -C "$repo" rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  echo "$tag is not a tag in $repo" >&2
  exit 1
fi

# The layout lives in the overlay's `site`; the tag supplies the app it wraps,
# through the nixpkgs and toolchain that release pinned.
# shellcheck disable=SC2016 # `${system}` is Nix's interpolation, not the shell's
expr="$(printf '
  let
    system = builtins.currentSystem;
    here = builtins.getFlake "git+file://%s";
    release = builtins.getFlake "git+file://%s?ref=refs/tags/%s";
  in
  here.legacyPackages.${system}.nord.site.override {
    web = release.packages.${system}.drawbar-web;
  }' "$repo" "$repo" "$tag")"

echo "site: app from $tag, guide from $(git -C "$repo" rev-parse --short HEAD) -> $out"

# `builtins.currentSystem` and an unlocked tag ref both need `--impure`; the
# tag's own flake.lock still pins everything the app is built from.
nix build --impure --print-build-logs --out-link "$out" --expr "$expr"
