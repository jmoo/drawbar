#!/usr/bin/env bash
# nix-deps: cargo cargo-about git jq nix
# Regenerate the license notices that travel with drawbar and nord: drawbar's About box
# table and nord's notices file.
set -euo pipefail

repo="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
drawbar="$repo/crates/drawbar"
nord="$repo/crates/nord-cli"
licenses="$drawbar/licenses"
out="$drawbar/src/about/crates.rs"
notices_out="$nord/THIRD-PARTY-NOTICES.txt"

defs='
  # Licenses that ask nothing of a binary copy.
  def unconditional: [
    "0BSD",      # grants use "for any purpose with or without fee" and sets no condition
    "BSL-1.0",   # its notice may be left out of "machine-executable object code"
    "CC0-1.0",   # section 2 waives copyright and related rights outright
    "Unlicense", # dedicates the work to the public domain
    "Zlib"       # clause 1: acknowledgment "is not required"; clause 3 binds source only
  ];
'

# cargo-about's report on one binary's crates, for the triples that binary ships on.
about() {
  local manifest=$1 targets=() target
  shift
  for target in "$@"; do targets+=(--target "$target"); done
  cargo about generate --format json --fail --locked \
    --manifest-path "$manifest" --config "$drawbar/about.toml" "${targets[@]}"
}

drawbar_about="$(about "$drawbar/Cargo.toml")"
nord_about="$(about "$nord/Cargo.toml" x86_64-unknown-linux-musl aarch64-unknown-linux-musl)"

# licenses.tsv supplies the text of a crate whose package carries none, fetched from
# the upstream file its `source` column names.
vendored="$(tail -n +2 "$repo/scripts/licenses.tsv" |
  while IFS=$'\t' read -r name version license file _; do
    jq -n --arg crate "$name $version" --arg license "$license" \
      --rawfile text "$licenses/$file" '{$crate, $license, $text}'
  done | jq -s 'INDEX(.crate)')"

# Apache-2.0.txt is https://www.apache.org/licenses/LICENSE-2.0.txt verbatim.
entries() {
  jq --argjson vendored "$vendored" --rawfile apache "$licenses/Apache-2.0.txt" "$defs"'
  # The SPDX template of a notice, whose copyright line names no one.
  def unfilled: ascii_downcase | test("<year>|<owner>|<copyright holder");
  def offers_apache: [splits("\\s+OR\\s+|/") | gsub("^[(\\s]+|[)\\s]+$"; "")] | index("Apache-2.0");

  [
    .licenses[] as $license
    | $license.used_by[].crate
    # Path crates are the workspace, which the project row covers.
    | select(.source != null)
    | select($license.id | IN(unconditional[]) | not)
    | {
        crate: "\(.name) \(.version)",
        name,
        version,
        offers_apache: (.license // "" | offers_apache != null),
        dir: (.manifest_path | rtrimstr("/Cargo.toml")),
        license: $license.id,
        text: $license.text
      }
  ]
  | map(
      if $vendored[.crate] == null then .
      elif $vendored[.crate].license == .license then .text = $vendored[.crate].text
      else error("\(.crate) is under \(.license), not the \($vendored[.crate].license) licenses.tsv gives")
      end
    )
  # Apache-2.0 needs no copyright line, so its text is complete where the other license
  # a crate offers is only a template.
  | map(if (.text | unfilled) and .offers_apache then .license = "Apache-2.0" | .text = $apache end)
  # rustc reads CRLF in a raw string as LF, and rejects a lone CR.
  | map(.text |= gsub("\r\n"; "\n"))
  | (map(select(.text | unfilled) | .crate) | unique) as $unfilled
  | if $unfilled != [] then error("these crates have only a license template: \($unfilled)") end
  | if any(.[].text; test("\r|\"#")) then error("a license text holds a CR or \"#") end
'
}

drawbar_entries="$(entries <<<"$drawbar_about")"
nord_entries="$(entries <<<"$nord_about")"

jq -n --argjson vendored "$vendored" --argjson drawbar "$drawbar_entries" --argjson nord "$nord_entries" '
  ($vendored | keys - ($drawbar + $nord | map(.crate) | unique)) as $stale
  | if $stale != [] then error("licenses.tsv names crates neither drawbar nor nord ships: \($stale)") end
' >/dev/null

# Apache-2.0 §4(d) requires a NOTICE file's contents to travel with the binary.
notices="$(jq -r '.[] | select(.license == "Apache-2.0") | .dir' <<<"$drawbar_entries $nord_entries" |
  sort -u | xargs -I{} find {} -maxdepth 1 -iname 'NOTICE*')"
if [[ -n $notices ]]; then
  printf 'neither notices file carries these NOTICE files:\n%s\n' "$notices" >&2
  exit 1
fi

# licenses/<id>.txt holds the one text shown for a license id. A new id needs a new
# file, and reading it here is what says so.
canon="$(jq -rn --argjson drawbar "$drawbar_entries" --argjson nord "$nord_entries" \
  '$drawbar + $nord | map(.license) | unique[]' |
  while read -r id; do
    jq -n --arg id "$id" --rawfile text "$licenses/$id.txt" '{($id): $text}'
  done | jq -s 'add')"

# Each license with its text once, the copyright notices under it and the crates that
# carry each, and the crates whose own text says more.
groups() {
  jq --argjson canon "$canon" '
  def version_key: split(".") | map(tonumber? // .);
  def crates: map([.name, .version]) | unique | sort_by(.[0], (.[1] | version_key))
    | map("\(.[0]) \(.[1])");

  # A copyright line opens a notice: the marker starts the line and a holder follows it.
  # This keeps the heading "COPYRIGHT AND PERMISSION NOTICE" out of the holder list.
  def opens: test("^(?i:copyright|\\(c\\)|©)[^A-Za-z0-9]*(?=.*[a-z0-9])");
  def indent: length - (sub("^[ \t]*"; "") | length);

  # A license file split into the copyright notices it carries and the terms around them.
  # A notice runs on while the lines under it are indented, continue with "and", or are
  # the "All rights reserved." that BSD-2-Clause wraps onto its own line.
  def notices:
    reduce (split("\n")[]) as $raw (
      {open: null, base: 0, notice: [], body: []};
      ($raw | rtrim) as $line
      | ($line | trim) as $held
      | if ($held | opens) then
          if .open == null then .base = ($line | indent) | .open = [$held]
          else .open += [$held]
          end
        elif .open != null and $held != ""
          and (($line | indent) > .base
            or ($held | test("(?i)^and "))
            or ($held | ascii_downcase) == "all rights reserved.")
        then .open += [$held]
        else (if .open == null then . else .notice += .open | .open = null end)
          | .body += [$line]
        end
    )
    | (if .open == null then . else .notice += .open end)
    | {notice: (.notice | join("\n")), body: (.body | join("\n"))};

  # The appendix is boilerplate for a licensee to fill in, not terms: crates ship it
  # blank, filled in with their own name, or not at all.
  def terms: split("APPENDIX: How to apply the Apache License")[0];
  def compare: terms | ascii_downcase | gsub("[^a-z0-9]+"; " ") | trim;

  # A crate keeps the group text when its own says nothing that text does not. Its first
  # line may be a title the group text spells differently, so try again without it.
  def covered($whole):
    ($whole | compare) as $all
    | [compare, (split("\n")[1:] | join("\n") | compare)]
    | any(.[]; . as $part | $part != "" and ($all | contains($part)));

  map(
    . + (
      # Apache-2.0 asks a binary to pass on the license, not a copyright line.
      if .license == "Apache-2.0" then {notice: "", body: .text} else .text | notices end
    )
  )
  | map(.variant = ($canon[.license] as $whole | .body | covered($whole) | not))
  | group_by(.license)
  | map(
      (map(select(.variant | not))) as $shared
      | {
          license: .[0].license,
          holders: (
            $shared
            | map(select(.notice != ""))
            | group_by(.notice)
            | map({notice: .[0].notice, crates: crates})
            | sort_by(.notice)
          ),
          unattributed: ($shared | map(select(.notice == "")) | crates),
          variants: (
            map(select(.variant))
            | group_by(.text)
            | map({crates: crates, text: .[0].text})
            | sort_by(.crates[0])
          )
        }
    )
  | sort_by(.license)
'
}

# The registry packages drawbar reaches in Cargo.lock, so a lock change that only
# another crate sees does not ask for this script.
locked="$(awk -F' = ' '
  $0 == "[[package]]" { n++; listing = 0; next }
  listing && $0 == "]" { listing = 0; next }
  listing { gsub(/^ *"|",?$/, ""); deps[n] = deps[n] "\n" $0; next }
  $1 == "name" { name[n] = $2; gsub(/"/, "", $2); bare[n] = $2 }
  $1 == "version" { version[n] = $2; gsub(/"/, "", $2); plain[n] = $2 }
  $1 == "source" && $2 ~ /^"registry\+/ { registry[n] = 1 }
  $0 == "dependencies = [" { listing = 1 }
  END {
    for (i = 1; i <= n; i++) if (bare[i] == "drawbar") queue[++tail] = i
    for (head = 1; head <= tail; head++) {
      at = queue[head]
      if (seen[at]++) continue
      count = split(substr(deps[at], 2), wanted, "\n")
      for (d = 1; d <= count; d++) {
        split(wanted[d], word, " ")
        for (i = 1; i <= n; i++)
          if (bare[i] == word[1] && (word[2] == "" || plain[i] == word[2])) queue[++tail] = i
      }
    }
    for (i = 1; i <= n; i++) if (seen[i] && registry[i]) print "(" name[i] ", " version[i] "),"
  }
' "$repo/crates/Cargo.lock")"

rust="$(groups <<<"$drawbar_entries" | jq -r --arg locked "$locked" '
  def list($name; $indent; $items):
    if $items == [] then "\($indent)\($name): &[],"
    else "\($indent)\($name): &[",
      ($items[] | "\($indent)    \(tojson),"),
      "\($indent)],"
    end;

  "// Generated by scripts/licenses.bash; do not edit by hand.",
    "",
    "use super::{Group, Holder, Text};",
    "",
    "// A copyright notice can outrun rustfmt, which then leaves the whole table as it",
    "// finds it, so the table is written already laid out.",
    "#[rustfmt::skip]",
    "pub(super) const GROUPS: &[Group] = &[",
    (.[] |
      "    Group {",
      "        license: \(.license | tojson),",
      "        text: include_str!(\"../../licenses/\(.license).txt\"),",
      (if .holders == [] then "        holders: &[],"
       else "        holders: &[",
         (.holders[] |
           "            Holder {",
           "                notice: \(.notice | tojson),",
           list("crates"; "                "; .crates),
           "            },"),
         "        ],"
       end),
      list("unattributed"; "        "; .unattributed),
      (if .variants == [] then "        variants: &[],"
       else "        variants: &[",
         (.variants[] |
           "            Text {",
           list("crates"; "                "; .crates),
           "                text: r#\"\(.text)\"#,",
           "            },"),
         "        ],"
       end),
      "    },"),
    "];",
    "",
    "#[cfg(test)]",
    "pub(super) const LOCKED: &[(&str, &str)] = &[",
    $locked,
    "];"
')"

printf '%s\n' "$rust" >"$out"
nix fmt -- "$out"

# The published field maps nord-format's Stage decoding is derived from, as drawbar's
# About box credits them.
derived="$(jq -n \
  --rawfile florentz "$licenses/nord-documentation.txt" \
  --rawfile randy "$licenses/ns4decode.txt" '[
  {covers: "Stage 2 and 3 field maps", page: "https://github.com/Chris55/nord-documentation", license: "BSD-3-Clause", text: $florentz},
  {covers: "Stage 4 field tables", page: "https://ns4decode.netlify.app", license: "MIT", text: $randy}
]')"

# A crate is listed as `- name version` on a line of its own; the tarball's build finds
# every crate it links that way.
groups <<<"$nord_entries" | jq -r --argjson derived "$derived" --argjson canon "$canon" \
  --argjson free "$(jq "$defs"'
    [.licenses[] | select(.id | IN(unconditional[])) | .used_by[].crate | select(.source != null)
      | "\(.name) \(.version)"] | unique' <<<"$nord_about")" '
  def rule: "", ("=" * 78), "";
  def listed: .[] | "- \(.)";

  "Generated by scripts/licenses.bash; do not edit by hand.",
  "",
  "nord is under the BSD 3-Clause License in LICENSE. It carries the work below,",
  "under the licenses that follow it.",
  ($derived[] | rule, "\(.covers) (\(.license))", .page, "", .text),
  (.[] |
    rule,
    "Rust crates under \(.license)",
    (.holders[] | "", .notice, (.crates | listed)),
    (if .unattributed == [] then empty else "", "No copyright line in the license file:", (.unattributed | listed) end),
    "", $canon[.license],
    (.variants[] | "", "These carry their own text:", (.crates | listed), "", .text)),
  rule,
  "Rust crates under licenses that ask for no notice in a binary",
  "",
  ($free | listed)
' >"$notices_out"
