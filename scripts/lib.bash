# Shared by bump.bash and release.bash. Source it; do not run it.
#
# Every function reads the workspace through `cargo metadata`, so the set of
# crates, their versions and the dependency graph need no wiring here.

set -euo pipefail

repo="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
workspace="$repo/crates"

meta="$(cargo metadata --manifest-path "$workspace/Cargo.toml" --no-deps --format-version 1)"
# shellcheck disable=SC2034 # release.bash
repo_url="$(jq -r '.packages[0].repository // empty' <<<"$meta")"

# Publishable crates, dependencies before dependents.
crates_in_publish_order() {
  jq -r '
    [.packages[] | select(.publish == null or (.publish | length) > 0)] as $ws
    | ($ws | map(.name)) as $names
    | ($ws | map({key: .name, value: [.dependencies[] | select(.path != null) | .name]}) | from_entries) as $deps
    | reduce range(0; $names | length) as $_ ([];
        . as $done
        | . + [$names[] | select(. as $n | ($done | index($n) | not) and all($deps[$n][]; . as $d | $done | index($d)))]
      )
    | .[]' <<<"$meta"
}

crate_version() { jq -r --arg n "$1" '.packages[] | select(.name == $n) | .version' <<<"$meta"; }

# Directory of the crate, relative to the repository root.
crate_dir() {
  jq -r --arg n "$1" --arg repo "$repo/" \
    '.packages[] | select(.name == $n) | .manifest_path | sub("/Cargo.toml$"; "") | ltrimstr($repo)' <<<"$meta"
}

# Workspace crates that depend on `$1`.
dependents_of() {
  jq -r --arg n "$1" '.packages[] | select(any(.dependencies[]; .path != null and .name == $n)) | .name' <<<"$meta" | sort -u
}

# Newest `<crate>-v*` tag, or nothing when the crate has never been released.
latest_tag() { git -C "$repo" tag --list "$1-v*" --sort=-v:refname | head -n1; }

# Conventional commits touching `$1` since `$2` through `$3` (HEAD by default).
# Output: sha, type, breaking flag, scope, description; unmatched subjects are `other`.
conventional='^([a-z]+)(\(([^)]+)\))?(!)?:[[:space:]]+(.*)$'
commits_for() {
  local crate=$1 since=${2:-} until=${3:-HEAD} range
  range="${since:+$since..}$until"
  git -C "$repo" log --format='%H%x1f%s%x1f%b%x1e' "$range" -- "$(crate_dir "$crate")" |
    while IFS=$'\x1f' read -r -d $'\x1e' sha subject body; do
      sha=${sha//$'\n'/} # git log ends each record with a newline
      local type=other scope="" breaking=0 description=$subject
      if [[ $subject =~ $conventional ]]; then
        type=${BASH_REMATCH[1]}
        scope=${BASH_REMATCH[3]}
        [[ -n ${BASH_REMATCH[4]} ]] && breaking=1
        description=${BASH_REMATCH[5]}
      fi
      grep -Eq '^BREAKING[ -]CHANGE:' <<<"$body" && breaking=1
      printf '%s\t%s\t%s\t%s\t%s\n' "$sha" "$type" "$breaking" "$scope" "$description"
    done
}

# major | minor | patch | none, from commits_for output on stdin.
bump_level() {
  awk -F'\t' '
    $3 == 1 { level = 3 }
    $2 == "feat" && level < 2 { level = 2 }
    ($2 == "fix" || $2 == "perf" || $2 == "revert") && level < 1 { level = 1 }
    END { print (level == 3 ? "major" : level == 2 ? "minor" : level == 1 ? "patch" : "none") }'
}

level_rank() {
  case $1 in
  major) echo 3 ;;
  minor) echo 2 ;;
  patch) echo 1 ;;
  *) echo 0 ;;
  esac
}

# The higher of two levels.
max_level() {
  if (($(level_rank "$1") >= $(level_rank "$2"))); then echo "$1"; else echo "$2"; fi
}

# Before 1.0 a breaking change bumps the minor: 0.x is the pre-release line and
# cargo already treats 0.MINOR as the compatibility boundary.
next_version() {
  local version=$1 level=$2 major minor patch
  IFS=. read -r major minor patch <<<"$version"
  [[ $level == major && $major == 0 ]] && level=minor
  case $level in
  major) echo "$((major + 1)).0.0" ;;
  minor) echo "$major.$((minor + 1)).0" ;;
  patch) echo "$major.$minor.$((patch + 1))" ;;
  none) echo "$version" ;;
  esac
}

# True when dotted version `$1` is at least dotted version `$2`.
version_at_least() {
  local -a have want
  IFS=. read -r -a have <<<"$1"
  IFS=. read -r -a want <<<"$2"
  local i
  for i in 0 1 2; do
    if ((${have[i]:-0} > ${want[i]:-0})); then return 0; fi
    if ((${have[i]:-0} < ${want[i]:-0})); then return 1; fi
  done
  return 0
}

# The `[package] version` of the manifest at `$2/Cargo.toml` as of ref `$1`;
# empty when the crate does not exist there.
manifest_version_at() {
  git -C "$repo" show "$1:$2/Cargo.toml" 2>/dev/null |
    awk -F'"' '/^\[/ { in_package = ($0 == "[package]") }
               in_package && /^version = / { print $2; exit }'
}
