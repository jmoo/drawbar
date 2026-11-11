#!/usr/bin/env bash
# nix-deps: cargo coreutils curl gh git jq nix
# Publish untagged manifest versions in dependency order, then create releases
# and attach their Windows builds.

usage() {
  echo "usage: $0 [--dry-run]" >&2
  exit 2
}

dry_run=0
for arg in "$@"; do
  case $arg in
  --dry-run) dry_run=1 ;;
  *) usage ;;
  esac
done

# shellcheck source=scripts/lib.bash
source "$(dirname "${BASH_SOURCE[0]}")/lib.bash"

if ((!dry_run)) && [[ -n "$(git -C "$repo" status --porcelain)" ]]; then
  echo "working tree is dirty; release from a clean checkout" >&2
  exit 1
fi

# crates.io's sparse index: crate names of 4+ chars live at <ab>/<cd>/<name>.
# 404 is a crate with no release at all, not a failed lookup.
published() {
  local crate=$1 version=$2 index code
  index="https://index.crates.io/${crate:0:2}/${crate:2:2}/$crate"
  code="$(curl -s -o "$tmp/index" -w '%{http_code}' "$index")"
  case $code in
  200) jq -e --arg v "$version" 'select(.vers == $v)' "$tmp/index" >/dev/null ;;
  404) return 1 ;;
  *)
    echo "$index returned HTTP $code" >&2
    exit 1
    ;;
  esac
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

sha="$(git -C "$repo" rev-parse HEAD)"
git -C "$repo" fetch --quiet --tags origin

# ⚠️ Publishing is irreversible. Validate every version against its commits
# before publishing any crate, so the run touches all or none.
under_bumped=()
for crate in "${publish_order[@]}"; do
  version="$(crate_version "$crate")"
  if git -C "$repo" rev-parse -q --verify "refs/tags/$crate-v$version" >/dev/null; then continue; fi
  previous="$(latest_tag "$crate")"
  [[ -z $previous ]] && continue # first release: the manifest is the whole claim
  from="${previous#"$crate"-v}"
  wanted="$(next_version "$from" "$(commits_for "$crate" "$previous" | bump_level)")"
  version_at_least "$version" "$wanted" ||
    under_bumped+=("$crate is $version, but its commits since $previous call for $wanted")
done

if ((${#under_bumped[@]})); then
  echo "refusing to publish a version that understates its own changes:" >&2
  printf '  %s\n' "${under_bumped[@]}" >&2
  echo "run scripts/bump.bash on master, which bumps from the full history since each release tag, then re-run" >&2
  exit 1
fi

# A release this run makes, or one made at this commit by an earlier run of it.
# Anything older keeps the assets it has; this commit did not build them.
released_here() {
  local ref="refs/tags/$1"
  ! git -C "$repo" rev-parse -q --verify "$ref" >/dev/null ||
    [[ "$(git -C "$repo" rev-parse "$ref^{commit}")" == "$sha" ]]
}

# The Windows build a crate's release carries: a cross package and its executable.
declare -A windows_build=(["drawbar"]=drawbar-windows/drawbar.exe ["nord-cli"]=nord-cli-windows/nord.exe)

# Built before anything is published, so a build that fails releases nothing.
declare -A windows_asset=()
for crate in "${publish_order[@]}"; do
  [[ -v "windows_build[$crate]" ]] || continue
  version="$(crate_version "$crate")"
  released_here "$crate-v$version" || continue
  build=${windows_build[$crate]}
  asset="$tmp/$(basename "$build" .exe)-$version-x86_64-windows.exe"
  windows_asset[$crate]=$asset
  ((dry_run)) && continue
  out="$(nix build --no-link --print-out-paths "$repo#${build%%/*}")"
  cp "$out/${build#*/}" "$asset"
done

# Uploads only an asset the release lacks or holds different bytes for, so a
# rerun converges.
attach_windows_build() {
  local crate=$1 tag=$2 asset name digest
  [[ -v "windows_asset[$crate]" ]] || return 0
  asset=${windows_asset[$crate]}
  name="$(basename "$asset")"
  if ((dry_run)); then
    echo "$tag: would attach $name"
    return
  fi
  digest="sha256:$(sha256sum "$asset" | cut -d' ' -f1)"
  if [[ "$(gh release view "$tag" --repo "$repo_url" --json assets \
    --jq ".assets[] | select(.name == \"$name\") | .digest")" == "$digest" ]]; then
    echo "$tag: already carries $name"
  else
    gh release upload "$tag" "$asset" --repo "$repo_url" --clobber
  fi
}

for crate in "${publish_order[@]}"; do
  version="$(crate_version "$crate")"
  tag="$crate-v$version"
  if git -C "$repo" rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
    echo "$tag: already released"
    attach_windows_build "$crate" "$tag"
    continue
  fi
  previous="$(latest_tag "$crate")"
  commits_for "$crate" "$previous" | notes "$tag" "$previous" >"$tmp/notes.md"

  if published "$crate" "$version"; then
    echo "$crate $version: already on crates.io"
  elif ((dry_run)); then
    echo "$crate $version: would publish to crates.io"
  else
    cargo publish --manifest-path "$workspace/Cargo.toml" -p "$crate"
  fi

  if ((dry_run)); then
    echo "$tag: would create the GitHub release at ${sha:0:7} with notes:"
    sed 's/^/    /' "$tmp/notes.md"
  else
    gh release create "$tag" --repo "$repo_url" --target "$sha" --title "$tag" --notes-file "$tmp/notes.md"
  fi
  attach_windows_build "$crate" "$tag"
done
