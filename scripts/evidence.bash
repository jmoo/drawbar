#!/usr/bin/env bash
# nix-deps: coreutils gnugrep nix procps
# Runs the evidence for a change, its independent parts at once, and prints each
# part's wall time. The full tier gates a merge; `--fast` is the tier for every
# change. TLC's model check, `nix build .#nord.toshokan-spec`, runs apart. Every
# crate's tests run in its Nix build, with its declared test features.
# shellcheck disable=SC2329 # `part` runs the functions it names
set -euo pipefail

usage() {
  echo "usage: $0 [--fast] [--out <dir>]" >&2
  exit 2
}

tier=full out=""
while (($#)); do
  case $1 in
  --fast) tier=fast ;;
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
out="${out:-$(mktemp -d)}"
mkdir -p "$out"
out="$(realpath "$out")"
system="$(nix eval --raw --impure --expr builtins.currentSystem)"
export CARGO_INCREMENTAL=0 CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo/crates/target}"

# Peak resident memory on an 8-core, 31 GB host: a power-loss VM 1.4 GB, a
# cargo build at -j 8 2.3 GB per rustc, the sweep 230 MB. TLC, when it runs, keeps
# 10 GB.
if [[ -r /proc/meminfo ]]; then
  mem_gb=$(($(grep MemTotal /proc/meminfo | grep -o '[0-9]*') >> 20))
else
  mem_gb=$(($(sysctl -n hw.memsize) >> 30))
fi
pgrep -x java >/dev/null && mem_gb=$((mem_gb - 10))
if ((mem_gb >= 24)); then nix_jobs=4 cargo_jobs=8; else nix_jobs=2 cargo_jobs=4; fi
nix=(nice -n 19 nix --max-jobs "$nix_jobs" --cores 4)
cargo() { nice -n 19 nix develop "$repo" -c cargo "$@"; }

# Runs `$2…` as part `$1` in the background, logged to `$out/$1.log`.
part() {
  local name=$1
  shift
  (
    start=$(date +%s)
    status=0
    "$@" >"$out/$name.log" 2>&1 </dev/null || status=$?
    echo "$status $(($(date +%s) - start))" >"$out/$name.status"
  ) &
}

nix_build() { "${nix[@]}" build --no-link -L --keep-going "$@"; }

toshokan() {
  cd "$repo/crates"
  cargo test -p toshokan --profile release-tests -j "$cargo_jobs"
  cargo clippy -p toshokan --all-targets --features web -- --deny warnings
}

# The value-level sweep's binary; the sweep takes longest, so it is built first.
sweep_binary() {
  local built
  built=$(cd "$repo/crates" && cargo test -p toshokan --profile release-tests -j "$cargo_jobs" \
    --test retention_sweep --no-run 2>&1) || {
    echo "$built" >&2
    return 1
  }
  # Cargo names it relative to crates/ when the target directory is under it.
  built=$(grep -o '[^ (]*/retention_sweep-[0-9a-f]*' <<<"$built")
  (cd "$repo/crates" && realpath "$built")
}

# The browser suites run on Linux, and the power-loss VMs need x86_64 Linux.
linux=$([[ $system == *-linux ]] && echo 1 || true)
linux_x86=$([[ $system == x86_64-linux ]] && echo 1 || true)
started=$(date +%s)
case $tier in
fast)
  part toshokan toshokan
  [[ -n $linux_x86 ]] && part power-loss nix_build "$repo#toshokan-power-loss.ext4"
  ;;
full)
  mapfile -t checks < <(nix eval --raw "$repo#checks.$system" --apply \
    'checks: builtins.concatStringsSep "\n" (builtins.filter (name: name != "toshokan-spec") (builtins.attrNames checks))')
  part nix nix_build "${checks[@]/#/$repo#checks.$system.}" "$repo#nord.all" \
    ${linux:+"$repo#drawbar-web-tests" "$repo#toshokan-web" "$repo#toshokan-browser-crash"} ${linux_x86:+"$repo#toshokan-power-loss"}
  sweep=$(sweep_binary) || {
    wait
    exit 1
  }
  part sweep nice -n 19 "$sweep" --ignored --test-threads 1 --nocapture
  ;;
esac
wait
[[ $tier == full ]] && part flake nix flake check --no-build "$repo" && wait

failed=0
for status in "$out"/*.status; do
  read -r code took <"$status"
  name=$(basename "$status" .status)
  printf '%-12s %4d s  %s\n' "$name" "$took" "$([[ $code == 0 ]] && echo ok || echo "FAILED: $out/$name.log")"
  ((code == 0)) || failed=1
done
echo "$tier tier: $(($(date +%s) - started)) s; logs in $out"
exit "$failed"
