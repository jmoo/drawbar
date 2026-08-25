#!/usr/bin/env bash
# nix-deps: cargo git jq
#
# Bump crate versions from Conventional Commits, rewrite the workspace
# dependency requirements that point at a bumped crate, and refresh Cargo.lock.
# Prints the plan and a suggested `chore(release): …` commit title; `--dry-run`
# stops at the plan. ci.yml's release job publishes whatever the merged result is new.
#
#   breaking change (`!` or BREAKING CHANGE:) → major   (minor while 0.x)
#   feat                                       → minor
#   fix / perf / revert                        → patch
#   anything else                              → no bump
#
# Two ways to pick what bumps:
#
#   bump.bash                  each crate, from its commits since its last
#                              release tag (catch-up mode, run on master)
#   bump.bash --title <t>      the crates touched by the diff against --base
#                              (default origin/master), at the level the one
#                              title says — the PR flow, where the squashed
#                              title is the only commit that will land.
#                              Targets are computed from the versions at the
#                              merge-base, so re-running after more commits or
#                              a retitle converges instead of double-bumping.
#
# ⚠️ In title mode a crate takes the higher of the title's level and the level
# its already-unreleased commits call for. A PR that merges without the `bump`
# label leaves its commits on the base branch at the last released version, and
# the title alone would then publish, say, a feat under a patch.
#
# Either way a crate that depends on a bumped crate gets at least a patch bump
# so the new requirement ships. A crate with no release tag yet is never
# bumped: its Cargo.toml already says what the first release is.

usage() {
  echo "usage: $0 [--dry-run] [--title <pr-title> [--base <ref>]]" >&2
  exit 2
}

dry_run=0 title="" base=origin/master
while (($#)); do
  case $1 in
  --dry-run) dry_run=1 ;;
  --title)
    [[ $# -ge 2 ]] || usage
    title=$2
    shift
    ;;
  --base)
    [[ $# -ge 2 ]] || usage
    base=$2
    shift
    ;;
  *) usage ;;
  esac
  shift
done

# shellcheck source=scripts/lib.bash
source "$(dirname "${BASH_SOURCE[0]}")/lib.bash"

declare -A new_version reason
bumped=0
merge_base=""

# The level called for by commits that are on the base branch and unreleased —
# what PRs that merged without the `bump` label left behind.
unreleased_level() {
  local tag
  tag="$(latest_tag "$1")"
  if [[ -z $tag ]]; then
    echo none
    return
  fi
  commits_for "$1" "$tag" "${merge_base:-HEAD}" | bump_level
}

crates=()
while IFS= read -r crate; do crates+=("$crate"); done < <(crates_in_publish_order)

if [[ -n $title ]]; then
  breaking=0 type=other
  if [[ $title =~ $conventional ]]; then
    type=${BASH_REMATCH[1]}
    [[ -n ${BASH_REMATCH[4]} ]] && breaking=1
  fi
  level="$(printf -- '-\t%s\t%s\t-\t-\n' "$type" "$breaking" | bump_level)"
  if [[ $level == none ]]; then
    echo "'$title' releases nothing"
    exit 0
  fi
  merge_base="$(git -C "$repo" merge-base "$base" HEAD)"
  # What the PR touches, not counting bump commits already on the branch —
  # their manifest edits would otherwise promote every dependent to the
  # title's level on a re-run.
  changed="$(git -C "$repo" log --invert-grep --grep='^chore(release): ' --format= --name-only "$merge_base..HEAD" | sort -u)"
  for crate in "${crates[@]}"; do
    dir="$(crate_dir "$crate")"
    grep -q "^$dir/" <<<"$changed" || continue
    if [[ -z "$(latest_tag "$crate")" ]]; then
      echo "$crate: no release yet, first release is $(crate_version "$crate") as written"
      continue
    fi
    from="$(manifest_version_at "$merge_base" "$dir")"
    pending="$(unreleased_level "$crate")"
    crate_level="$(max_level "$level" "$pending")"
    new_version[$crate]="$(next_version "$from" "$crate_level")"
    if [[ $crate_level == "$level" ]]; then
      reason[$crate]="$crate_level: PR title, from $from at the merge-base"
    else
      reason[$crate]="$crate_level: unreleased $pending outranks the title's $level, from $from"
    fi
    bumped=$((bumped + 1))
  done
else
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
    bumped=$((bumped + 1))
  done
fi

# The version a dependent bumps from: in title mode the merge-base, so the run
# converges; in catch-up mode the working tree.
base_version() {
  if [[ -n $merge_base ]]; then
    manifest_version_at "$merge_base" "$(crate_dir "$1")"
  else
    crate_version "$1"
  fi
}

# Dependents of a bumped crate ride along, transitively.
grew=1
while ((grew)); do
  grew=0
  for crate in "${crates[@]}"; do
    [[ -n ${new_version[$crate]:-} ]] || continue
    while IFS= read -r dependent; do
      [[ -z $dependent || -n ${new_version[$dependent]:-} ]] && continue
      [[ -z "$(latest_tag "$dependent")" ]] && continue
      dependent_level="$(max_level patch "$(unreleased_level "$dependent")")"
      new_version[$dependent]="$(next_version "$(base_version "$dependent")" "$dependent_level")"
      reason[$dependent]="$dependent_level: depends on $crate"
      [[ $dependent_level == patch ]] ||
        reason[$dependent]="$dependent_level: depends on $crate, and has unreleased $dependent_level commits"
      bumped=$((bumped + 1))
      grew=1
    done < <(dependents_of "$crate")
  done
done

if ((bumped == 0)); then
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

cargo update --manifest-path "$workspace/Cargo.toml" --workspace --quiet

summary=""
for crate in "${crates[@]}"; do
  [[ -n ${new_version[$crate]:-} ]] || continue
  summary="$summary${summary:+, }$crate ${new_version[$crate]}"
done
echo
echo "suggested commit: chore(release): $summary"
