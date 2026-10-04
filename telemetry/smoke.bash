#!/usr/bin/env bash
# Exercise a running collector over HTTP: what it accepts, what it refuses, and that a
# report sent twice is kept once.
#
#   telemetry/smoke.bash URL ORIGIN
#
# URL is the collector (`http://127.0.0.1:8787` under `wrangler dev`, or
# `https://t.drawbar.app`); ORIGIN is the origin it accepts (`DEV_ORIGIN` locally,
# `https://drawbar.app` in production). Rows and reports carry version `0.0.0-test`, and
# it prints the report ids, so both can be found and deleted afterward.
#
# Set SMOKE_GAP=4 against production: the edge's rate limit on /report answers 429 to
# more than three reports in ten seconds from one address.
#
# nix-deps: curl
set -euo pipefail

url=${1:?collector URL}
origin=${2:?accepted origin}
failed=0
gap=${SMOKE_GAP:-0}

# Expect status $1 from a request with the rest of the arguments, described by $2.
expect() {
  local want=$1 what=$2
  shift 2
  local got
  got=$(curl -s -o /dev/null -w '%{http_code}' "$@")
  if [[ $got == "$want" ]]; then
    echo "ok   $got  $what"
  else
    echo "FAIL $got  $what (wanted $want)"
    failed=1
  fi
}

post() {
  local path=$1 body=$2
  shift 2
  if [[ $path == /report ]]; then
    sleep "$gap"
  fi
  curl_args=(-X POST "$url$path" -H 'content-type: text/plain' --data-binary "$body" "$@")
}

visit='{"event":"visit","version":"0.0.0-test","navigation":"navigate","first_day":true,"first_month":true,"webusb":true,"fits":true,"referrer":"","language":"en"}'
op='{"event":"op","version":"0.0.0-test","op":"put","class":"1","outcome":"device-status 0x15","took":"<1s","model":"Nord Electro 5D","firmware":"2.04"}'

post /e "[$visit,$op]" -H "origin: $origin"
expect 204 "a beacon of two rows" "${curl_args[@]}"
post /e "[$visit]" -H 'origin: https://fork.example'
expect 403 "a beacon from another origin" "${curl_args[@]}"
post /e "[$visit]"
expect 403 "a beacon naming no origin" "${curl_args[@]}"
expect 405 "a GET" "$url/e" -H "origin: $origin"
post /nowhere "[]" -H "origin: $origin"
expect 404 "an unknown path" "${curl_args[@]}"
expect 204 "a preflight" -X OPTIONS "$url/report" -H "origin: $origin"

alphabet=23456789abcdefghjkmnpqrstuvwxyz
fresh() {
  local made=
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    made+=${alphabet:RANDOM%${#alphabet}:1}
  done
  echo "$made"
}
id=$(fresh)
desktop=$(fresh)
# A report with text $1, under id $2 (default: $id).
report() {
  printf '{"id":"%s","kind":"problem","text":"%s","contact":"","version":"0.0.0-test","model":"","firmware":"","faults":"","build":"","log":""}' "${2:-$id}" "$1"
}

post /report "$(report 'smoke test')" -H "origin: $origin"
expect 204 "a report" "${curl_args[@]}"
post /report "$(report 'smoke test')" -H "origin: $origin"
expect 204 "the same report again" "${curl_args[@]}"
post /report "$(report '')" -H "origin: $origin"
expect 400 "a report with no text" "${curl_args[@]}"
post /report '{"id":"x"}' -H "origin: $origin"
expect 400 "a report missing fields" "${curl_args[@]}"
post /report "$(report 'smoke test')" -H 'origin: https://fork.example'
expect 403 "a report from another origin" "${curl_args[@]}"
post /report "$(report 'smoke test' "$desktop")"
expect 204 "a report naming no origin, as the desktop app sends it" "${curl_args[@]}"

echo "report ids: $id $desktop"
exit "$failed"
