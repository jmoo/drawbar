#!/usr/bin/env bash
# nix-deps: cargo coreutils gh git jq nix
# Attach to each crate's newest release, or to the releases named, the files
# scripts/assets.tsv lists for this host's Nix system, each built from its tag's flake.
# A file the release already carries is left alone, so any host may rerun this.
#
# A row is: crate, the system that builds it, a flake package, a file in that package,
# `make`, and the asset name. With `make` at `-` the file is attached as it is;
# otherwise `make <file> <asset>` writes the asset, run from this checkout. `@version@`
# in a name is the tag's version.

usage() {
  echo "usage: $0 [--dry-run] [--tag <crate>-v<version>]..." >&2
  exit 2
}

dry_run=0 tags=()
while (($#)); do
  case $1 in
  --dry-run) dry_run=1 ;;
  --tag)
    [[ $# -ge 2 ]] || usage
    tags+=("$2")
    shift
    ;;
  *) usage ;;
  esac
  shift
done

# shellcheck source=scripts/lib.bash
source "$(dirname "${BASH_SOURCE[0]}")/lib.bash"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

system="$(nix eval --impure --raw --expr builtins.currentSystem)"
tail -n +2 "$repo/scripts/assets.tsv" | awk -F'\t' -v host="$system" '$2 == host' >"$tmp/rows"

git -C "$repo" fetch --quiet --tags origin

if ((${#tags[@]} == 0)); then
  while read -r crate; do
    tag="$(latest_tag "$crate")"
    if [[ -n $tag ]]; then tags+=("$tag"); else echo "$crate: no release yet"; fi
  done < <(cut -f1 "$tmp/rows" | sort -u)
fi

missing=()
for tag in "${tags[@]}"; do
  if ! git -C "$repo" rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
    echo "$tag is not a tag in $repo" >&2
    exit 1
  fi
  crate=${tag%-v*} version=${tag##*-v}
  flake="git+file://$repo?ref=refs/tags/$tag"
  awk -F'\t' -v crate="$crate" '$1 == crate' "$tmp/rows" >"$tmp/mine"
  if [[ ! -s "$tmp/mine" ]]; then
    echo "$tag: nothing to attach from $system"
    continue
  fi

  # A failed lookup is an error, not a release without the file.
  if ! gh release view "$tag" --repo "$repo_url" --json assets >"$tmp/assets"; then
    echo "$tag: could not list the release's files" >&2
    exit 1
  fi
  if ! nix eval --json "$flake#packages.$system" --apply builtins.attrNames >"$tmp/packages"; then
    echo "$tag: could not list its flake's packages for $system" >&2
    exit 1
  fi

  while IFS=$'\t' read -r _ _ package file make asset; do
    file=${file//@version@/$version} name=${asset//@version@/$version}
    if jq -e --arg name "$name" 'any(.assets[]; .name == $name)' "$tmp/assets" >/dev/null; then
      echo "$tag: already carries $name"
    elif ! jq -e --arg package "$package" 'index($package) != null' "$tmp/packages" >/dev/null; then
      # A release cut before the package existed has nothing to build it from.
      echo "$tag: its flake has no $package; skipped $name"
    elif ((dry_run)); then
      echo "$tag: would build $package and attach $name"
    elif ! out="$(nix build --no-link --print-out-paths "$flake#$package")" ||
      ! if [[ $make == - ]]; then cp "$out/$file" "$tmp/$name"; else "$repo/$make" "$out/$file" "$tmp/$name"; fi ||
      ! gh release upload "$tag" "$tmp/$name" --repo "$repo_url"; then
      missing+=("$tag: $name")
    fi
  done <"$tmp/mine"
done

if ((${#missing[@]})); then
  echo "these files are not attached; a rerun attaches them:" >&2
  printf '  %s\n' "${missing[@]}" >&2
  exit 1
fi
