#!/usr/bin/env bash
# nix-deps: cargo git jq
# Bump crates from Conventional Commits and propagate dependency releases.

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

crates=()
while IFS= read -r crate; do crates+=("$crate"); done < <(crates_in_publish_order)

declare -A tag_of released
for crate in "${crates[@]}"; do
  tag_of[$crate]="$(latest_tag "$crate")"
  released[$crate]="${tag_of[$crate]#"$crate"-v}"
done

# What the squashed PR title will call for, on the crates the PR touches. It is
# extra evidence about commits that are not on the base branch yet; everything
# below reads the same way with or without it.
declare -A title_level
merge_base=""
if [[ -n $title ]]; then
  breaking=0 type=other
  if [[ $title =~ $conventional ]]; then
    type=${BASH_REMATCH[1]}
    [[ -n ${BASH_REMATCH[4]} ]] && breaking=1
  fi
  level="$(printf -- '-\t%s\t%s\t-\t-\n' "$type" "$breaking" | bump_level)"
  merge_base="$(git -C "$repo" merge-base "$base" HEAD)"
  # Ignore existing bump commits; their manifest edits would promote every
  # dependent to the title's level on a re-run.
  changed="$(git -C "$repo" log --invert-grep --grep='^chore(release): ' --format= --name-only "$merge_base..HEAD" | sort -u)"
  for crate in "${crates[@]}"; do
    grep -q "^$(crate_dir "$crate")/" <<<"$changed" && title_level[$crate]=$level
  done
fi

# The version each released crate's own history calls for. In title mode the
# commits are read up to the merge-base, because the title speaks for the rest.
declare -A level wanted reason
for crate in "${crates[@]}"; do
  tag=${tag_of[$crate]}
  if [[ -z $tag ]]; then
    echo "$crate: no release yet, first release is $(crate_version "$crate") as written"
    continue
  fi
  own="$(commits_for "$crate" "$tag" "${merge_base:-HEAD}" | bump_level)"
  from_title="${title_level[$crate]:-none}"
  level[$crate]="$(max_level "$own" "$from_title")"
  wanted[$crate]="$(next_version "${released[$crate]}" "${level[$crate]}")"
  if (($(level_rank "$from_title") > $(level_rank "$own"))); then
    reason[$crate]="${level[$crate]}: the PR title, over $own since $tag"
  else
    reason[$crate]="${level[$crate]}: commits since $tag"
  fi
done

# A crate whose release is still unpublished carries its dependents with it,
# transitively, whether or not this run is the one that bumps it.
grew=1
while ((grew)); do
  grew=0
  for crate in "${crates[@]}"; do
    [[ -n ${wanted[$crate]:-} && ${wanted[$crate]} != "${released[$crate]}" ]] || continue
    while IFS= read -r dependent; do
      [[ -n $dependent && -n ${wanted[$dependent]:-} ]] || continue
      [[ ${wanted[$dependent]} == "${released[$dependent]}" ]] || continue
      level[$dependent]="$(max_level "${level[$dependent]}" patch)"
      wanted[$dependent]="$(next_version "${released[$dependent]}" "${level[$dependent]}")"
      reason[$dependent]="${level[$dependent]}: depends on $crate"
      grew=1
    done < <(dependents_of "$crate")
  done
done

# A manifest already at or past what its commits call for is left alone: the
# version it holds is the release, and its dependents point at that.
declare -A resolved
bumped=()
for crate in "${crates[@]}"; do
  [[ -n ${wanted[$crate]:-} ]] || continue
  current="$(crate_version "$crate")"
  if version_at_least "$current" "${wanted[$crate]}"; then
    resolved[$crate]=$current
  else
    resolved[$crate]="${wanted[$crate]}"
    bumped+=("$crate")
  fi
done

if ((${#bumped[@]} == 0)); then
  echo "nothing to bump"
  exit 0
fi

for crate in "${bumped[@]}"; do
  printf '%-18s %s -> %s  (%s)\n' "$crate" "$(crate_version "$crate")" "${resolved[$crate]}" "${reason[$crate]}"
done

((dry_run)) && exit 0

for crate in "${crates[@]}"; do
  [[ -n ${resolved[$crate]:-} ]] || continue
  manifest="$repo/$(crate_dir "$crate")/Cargo.toml"
  sed -i.bak "/^\[package\]/,/^\[/ s/^version = \"[^\"]*\"/version = \"${resolved[$crate]}\"/" "$manifest"
  rm "$manifest.bak"
  # ⚠️ The requirement must be on the dependency's first line; a `version` on a
  # continuation line is not rewritten.
  while IFS= read -r dependent; do
    [[ -n $dependent ]] || continue
    dep_manifest="$repo/$(crate_dir "$dependent")/Cargo.toml"
    sed -i.bak -E "/^$crate = \{/ s/version = \"(=?)[^\"]*\"/version = \"\1${resolved[$crate]}\"/" "$dep_manifest"
    rm "$dep_manifest.bak"
  done < <(dependents_of "$crate")
done

cargo update --manifest-path "$workspace/Cargo.toml" --workspace --quiet

summary=""
for crate in "${bumped[@]}"; do
  summary="$summary${summary:+, }$crate ${resolved[$crate]}"
done
echo
echo "suggested commit: chore(release): $summary"
