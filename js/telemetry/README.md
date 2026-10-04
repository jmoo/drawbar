# telemetry

The collector behind drawbar.app's anonymous usage and error counts and its report
form, as a Cloudflare Worker on `t.drawbar.app`. [Privacy](../../docs/src/privacy.md)
says what it takes and keeps, in words for users.

- `POST /e` takes a JSON array of rows. Each row must match a kind in
  [`crates/drawbar/telemetry.json`](../../crates/drawbar/telemetry.json) exactly; anything
  else is dropped. Rows are written to the Analytics Engine dataset `drawbar_events`
  with the kind as `index1` and the fields as `blob1…` in the schema's order, followed
  by the kind's `edge` fields.
- `POST /report` takes one report into the D1 database `drawbar-reports`
  ([`migrations/`](migrations)): at most 1000 reports and about 4.5 MB of them in any
  24 hours (then `429`), so 90 days of reports stay inside the 500 MB a free-plan D1
  database holds. A daily trigger deletes reports older than 90 days. A Cloudflare
  rate-limiting rule on the zone limits each address on this path as well.

The collector keeps no request logs and stores no address or user agent.
[`queries/`](queries) holds the queries used to read both stores.

`node --test test/check.test.js` runs the tests; `nix flake check` runs them too. CI
deploys from `master`.

## Testing against a running collector

A build served anywhere but drawbar.app sends nothing unless it is pointed at a
collector, and the collector refuses every origin but drawbar.app's unless it is told
one more. From this directory, in the development shell (`nix develop`), with
`WRANGLER_SEND_METRICS=false`:

1. `wrangler d1 migrations apply drawbar-reports --local`
2. `wrangler dev --port 8787 --var DEV_ORIGIN:http://127.0.0.1:8090 --test-scheduled`
   runs the collector on this machine against a local D1. Analytics Engine writes go
   nowhere locally. With `--remote` it runs on Cloudflare against the real
   `drawbar-reports` and `drawbar_events` instead, which needs `wrangler login`.
3. `./smoke.bash http://127.0.0.1:8787 http://127.0.0.1:8090` checks what it accepts and
   refuses. `curl 'http://127.0.0.1:8787/__scheduled?cron=17+4+*+*+*'` runs the daily
   delete.
4. Serve a site build on that origin (`nix build .#site`, then any static server on port
   8090), and in its console run
   `localStorage.setItem("drawbar.telemetry.collector", "http://127.0.0.1:8787")`.
   Reload: the page reports there, and the report form sends there. drawbar.app ignores
   that key.

Test rows and reports carry version `0.0.0-test` or the id `smoke.bash` prints, for
deleting afterward.
