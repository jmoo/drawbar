#!/usr/bin/env bash
# nix-deps: cargo curl gh git jq
#
# Release every crate whose Cargo.toml version has no `<crate>-v<version>` tag
# yet: publish it to crates.io (skipped when that version is already there)
# and create the tag as a GitHub release whose notes are the crate's
# Conventional Commits since its previous tag, grouped by kind. Dependencies go
# before dependents. Every step is idempotent, so a failed run is re-run as is.
# `--dry-run` prints what would happen, notes included, and touches nothing.
#
# Needs a crates.io token in CARGO_REGISTRY_TOKEN (or `cargo login`) and
# `gh auth`; ci.yml's release job provides both.

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

# Release notes for the crate, from commits_for output on stdin.
notes() {
  local crate=$1 tag=$2 previous=$3
  awk -F'\t' -v url="$repo_url" '
    function item(line,    s) {
      s = "- " ($4 != "" ? "**" $4 ":** " : "") $5
      if (url != "") s = s " ([" substr($1, 1, 7) "](" url "/commit/" $1 "))"
      return s
    }
    $3 == 1 { breaking = breaking item($0) "\n"; next }
    $2 == "feat" { feat = feat item($0) "\n"; next }
    $2 == "fix" { fix = fix item($0) "\n"; next }
    $2 == "perf" { perf = perf item($0) "\n"; next }
    { other = other item($0) "\n" }
    END {
      if (breaking != "") print "### ⚠️ Breaking changes\n" breaking
      if (feat != "") print "### Features\n" feat
      if (fix != "") print "### Bug fixes\n" fix
      if (perf != "") print "### Performance\n" perf
      if (other != "") print "### Other changes\n" other
    }'
  if [[ -n $repo_url ]]; then
    if [[ -n $previous ]]; then
      echo "**Full changelog**: $repo_url/compare/$previous...$tag"
    else
      echo "**Full changelog**: $repo_url/commits/$tag"
    fi
  fi
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

sha="$(git -C "$repo" rev-parse HEAD)"
git -C "$repo" fetch --quiet --tags origin

while IFS= read -r crate; do
  version="$(crate_version "$crate")"
  tag="$crate-v$version"
  if git -C "$repo" rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
    echo "$tag: already released"
    continue
  fi
  previous="$(latest_tag "$crate")"
  commits_for "$crate" "$previous" | notes "$crate" "$tag" "$previous" >"$tmp/notes.md"

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
done < <(crates_in_publish_order)
