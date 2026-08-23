#!/usr/bin/env bash
# nix-deps: cargo git jq
#
# Bump crate versions from the Conventional Commits since each crate's last
# release tag, then rewrite the workspace dependency requirements that point at
# a bumped crate and refresh Cargo.lock. Prints the plan; `--dry-run` stops
# there. Commit the result as `chore(release): …` and merge it; release.yml
# publishes whatever is new.
#
#   breaking change (`!` or BREAKING CHANGE:) → major   (minor while 0.x)
#   feat                                       → minor
#   fix / perf / revert                        → patch
#   anything else                              → no bump
#
# A crate that depends on a bumped crate gets at least a patch bump so the new
# requirement ships. A crate with no release tag is left alone: its Cargo.toml
# already says what the first release is.

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

declare -A new_version reason
crates=()
while IFS= read -r crate; do crates+=("$crate"); done < <(crates_in_publish_order)

for crate in "${crates[@]}"; do
  tag="$(latest_tag "$crate")"
  if [[ -z $tag ]]; then
    echo "$crate: no release yet, first release is $(crate_version "$crate") as written"
    continue
  fi
  commits="$(commits_for "$crate" "$tag")"
  level="$(bump_level <<<"$commits")"
  [[ $level == none ]] && continue
  new_version[$crate]="$(next_version "$(crate_version "$crate")" "$level")"
  reason[$crate]="$level: $(wc -l <<<"$commits" | tr -d ' ') commit(s) since $tag"
done

# Dependents of a bumped crate ride along, transitively.
changed=1
while ((changed)); do
  changed=0
  for crate in "${crates[@]}"; do
    [[ -n ${new_version[$crate]:-} ]] || continue
    while IFS= read -r dependent; do
      [[ -z $dependent || -n ${new_version[$dependent]:-} ]] && continue
      new_version[$dependent]="$(next_version "$(crate_version "$dependent")" patch)"
      reason[$dependent]="patch: depends on $crate"
      changed=1
    done < <(dependents_of "$crate")
  done
done

if ((${#new_version[@]} == 0)); then
  echo "nothing to bump"
  exit 0
fi

for crate in "${crates[@]}"; do
  [[ -n ${new_version[$crate]:-} ]] || continue
  printf '%-18s %s -> %s  (%s)\n' "$crate" "$(crate_version "$crate")" "${new_version[$crate]}" "${reason[$crate]}"
done

((dry_run)) && exit 0

for crate in "${crates[@]}"; do
  [[ -n ${new_version[$crate]:-} ]] || continue
  manifest="$repo/$(crate_dir "$crate")/Cargo.toml"
  sed -i.bak "/^\[package\]/,/^\[/ s/^version = \"[^\"]*\"/version = \"${new_version[$crate]}\"/" "$manifest"
  rm "$manifest.bak"
  # ⚠️ The requirement must sit on the dependency's first line
  # (`nord-usb = { path = …, version = "…", … }`); a `version` on a
  # continuation line is not rewritten.
  while IFS= read -r dependent; do
    [[ -n $dependent ]] || continue
    dep_manifest="$repo/$(crate_dir "$dependent")/Cargo.toml"
    sed -i.bak -E "/^$crate = \{/ s/version = \"(=?)[^\"]*\"/version = \"\1${new_version[$crate]}\"/" "$dep_manifest"
    rm "$dep_manifest.bak"
  done < <(dependents_of "$crate")
done

cargo update --manifest-path "$workspace/Cargo.toml" --workspace --offline --quiet

summary=""
for crate in "${crates[@]}"; do
  [[ -n ${new_version[$crate]:-} ]] || continue
  summary="$summary${summary:+, }$crate ${new_version[$crate]}"
done
echo
echo "suggested commit: chore(release): $summary"
