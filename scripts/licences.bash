#!/usr/bin/env bash
# nix-deps: cargo cargo-about git jq nix
# Regenerate the licence notices drawbar's About box shows for the Rust crates in it.
set -euo pipefail

repo="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
drawbar="$repo/crates/drawbar"
licences="$drawbar/licences"
out="$drawbar/src/about/crates.rs"

about="$(cargo about generate --format json --fail --locked \
  --manifest-path "$drawbar/Cargo.toml" --config "$drawbar/about.toml")"

# licences.tsv supplies the text of a crate whose package carries none, fetched from
# the upstream file its `source` column names.
vendored="$(tail -n +2 "$repo/scripts/licences.tsv" |
  while IFS=$'\t' read -r name version licence file _; do
    jq -n --arg crate "$name $version" --arg licence "$licence" \
      --rawfile text "$licences/$file" '{$crate, $licence, $text}'
  done | jq -s 'INDEX(.crate)')"

# Apache-2.0.txt is https://www.apache.org/licenses/LICENSE-2.0.txt verbatim.
entries="$(jq --argjson vendored "$vendored" --rawfile apache "$licences/Apache-2.0.txt" '
  # Licences that ask nothing of a binary copy.
  def unconditional: [
    "0BSD",      # grants use "for any purpose with or without fee" and sets no condition
    "BSL-1.0",   # its notice may be left out of "machine-executable object code"
    "CC0-1.0",   # section 2 waives copyright and related rights outright
    "Unlicense", # dedicates the work to the public domain
    "Zlib"       # clause 1: acknowledgment "is not required"; clause 3 binds source only
  ];
  # epaint_default_fonts declares these for faces drawbar does not ship. Hack, the
  # face it does ship, has a hand-written About row.
  def fonts: ["OFL-1.1", "Ubuntu-font-1.0"];
  # The SPDX template of a notice, whose copyright line names no one.
  def unfilled: ascii_downcase | test("<year>|<owner>|<copyright holder");
  def offers_apache: [splits("\\s+OR\\s+|/") | gsub("^[(\\s]+|[)\\s]+$"; "")] | index("Apache-2.0");

  [
    .licenses[] as $licence
    | $licence.used_by[].crate
    # Path crates are the workspace, which the drawbar row covers.
    | select(.source != null)
    | select($licence.id | IN(unconditional[]) | not)
    | select(.name != "epaint_default_fonts" or ($licence.id | IN(fonts[]) | not))
    | {
        crate: "\(.name) \(.version)",
        name,
        version,
        offers_apache: (.license // "" | offers_apache != null),
        dir: (.manifest_path | rtrimstr("/Cargo.toml")),
        licence: $licence.id,
        text: $licence.text
      }
  ]
  | (map(.crate) | unique) as $shipped
  | ($vendored | keys - $shipped) as $stale
  | if $stale != [] then error("licences.tsv names crates drawbar does not ship: \($stale)") end
  | map(
      if $vendored[.crate] == null then .
      elif $vendored[.crate].licence == .licence then .text = $vendored[.crate].text
      else error("\(.crate) is under \(.licence), not the \($vendored[.crate].licence) licences.tsv gives")
      end
    )
  # Apache-2.0 needs no copyright line, so its text is complete where the other licence
  # a crate offers is only a template.
  | map(if (.text | unfilled) and .offers_apache then .licence = "Apache-2.0" | .text = $apache end)
  # rustc reads CRLF in a raw string as LF, and rejects a lone CR.
  | map(.text |= gsub("\r\n"; "\n"))
  | (map(select(.text | unfilled) | .crate) | unique) as $unfilled
  | if $unfilled != [] then error("these crates have only a licence template: \($unfilled)") end
  | if any(.[].text; test("\r|\"#")) then error("a licence text holds a CR or \"#") end
' <<<"$about")"

# Apache-2.0 §4(d) requires a NOTICE file's contents to travel with the binary.
notices="$(jq -r '.[] | select(.licence == "Apache-2.0") | .dir' <<<"$entries" | sort -u |
  xargs -I{} find {} -maxdepth 1 -iname 'NOTICE*')"
if [[ -n $notices ]]; then
  printf 'the About box does not carry these NOTICE files:\n%s\n' "$notices" >&2
  exit 1
fi

# licences/<id>.txt holds the one text the About box shows for a licence id. A new id
# needs a new file, and reading it here is what says so.
canon="$(jq -r '[.[].licence] | unique[]' <<<"$entries" |
  while read -r id; do
    jq -n --arg id "$id" --rawfile text "$licences/$id.txt" '{($id): $text}'
  done | jq -s 'add')"

locked="$(awk -F' = ' '
  $0 == "[[package]]" { name = version = "" }
  $1 == "name" { name = $2 }
  $1 == "version" { version = $2 }
  $1 == "source" && $2 ~ /^"registry\+/ { print "(" name ", " version ")," }
' "$repo/crates/Cargo.lock")"

rust="$(jq -r --argjson canon "$canon" --arg locked "$locked" '
  def version_key: split(".") | map(tonumber? // .);
  def list($name; $indent; $items):
    if $items == [] then "\($indent)\($name): &[],"
    else "\($indent)\($name): &[",
      ($items[] | "\($indent)    \(tojson),"),
      "\($indent)],"
    end;
  def crates: map([.name, .version]) | unique | sort_by(.[0], (.[1] | version_key))
    | map("\(.[0]) \(.[1])");

  # A copyright line opens a notice: the marker starts the line and a holder follows it.
  # This keeps the heading "COPYRIGHT AND PERMISSION NOTICE" out of the holder list.
  def opens: test("^(?i:copyright|\\(c\\)|©)[^A-Za-z0-9]*(?=.*[a-z0-9])");
  def indent: length - (sub("^[ \t]*"; "") | length);

  # A licence file split into the copyright notices it carries and the terms around them.
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
      # Apache-2.0 asks a binary to pass on the licence, not a copyright line.
      if .licence == "Apache-2.0" then {notice: "", body: .text} else .text | notices end
    )
  )
  | map(.variant = ($canon[.licence] as $whole | .body | covered($whole) | not))
  | group_by(.licence)
  | map(
      (map(select(.variant | not))) as $shared
      | {
          licence: .[0].licence,
          holders: (
            $shared
            | map(select(.notice != ""))
            | group_by(.notice)
            | map({notice: .[0].notice, crates: crates})
            | sort_by(.notice)
          ),
          unattributed: ($shared | map(select(.notice == "")) | length),
          variants: (
            map(select(.variant))
            | group_by(.text)
            | map({crates: crates, text: .[0].text})
            | sort_by(.crates[0])
          )
        }
    )
  | sort_by(.licence)
  | "// Generated by scripts/licences.bash; do not edit by hand.",
    "",
    "use super::{Group, Holder, Text};",
    "",
    "// A copyright notice can outrun rustfmt, which then leaves the whole table as it",
    "// finds it, so the table is written already laid out.",
    "#[rustfmt::skip]",
    "pub(super) const GROUPS: &[Group] = &[",
    (.[] |
      "    Group {",
      "        licence: \(.licence | tojson),",
      "        text: include_str!(\"../../licences/\(.licence).txt\"),",
      (if .holders == [] then "        holders: &[],"
       else "        holders: &[",
         (.holders[] |
           "            Holder {",
           "                notice: \(.notice | tojson),",
           list("crates"; "                "; .crates),
           "            },"),
         "        ],"
       end),
      "        unattributed: \(.unattributed),",
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
' <<<"$entries")"

printf '%s\n' "$rust" >"$out"
nix fmt -- "$out"
