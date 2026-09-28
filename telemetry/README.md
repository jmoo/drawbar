# telemetry

The collector behind drawbar.app's anonymous usage and error counts and its report
form, as a Cloudflare Worker on `t.drawbar.app`. [Privacy](../docs/src/privacy.md)
says what it takes and keeps, in words for users.

- `POST /e` takes a JSON array of rows. Each row must match a kind in
  [`crates/drawbar/telemetry.json`](../crates/drawbar/telemetry.json) exactly; anything
  else is dropped. Rows are written to the Analytics Engine dataset `drawbar_events`
  with the kind as `index1` and the fields as `blob1…` in the schema's order, followed
  by the kind's `edge` fields.
- `POST /report` takes one report into the D1 database `drawbar-reports`
  ([`migrations/`](migrations)). A daily trigger deletes reports older than 90 days.

The collector keeps no request logs and stores no address or user agent.
[`queries/`](queries) holds the queries used to read both stores.

`node --test test/check.test.js` runs the tests; `nix flake check` runs them too. CI
deploys from `master`.
