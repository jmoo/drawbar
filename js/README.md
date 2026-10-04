# js

The JavaScript in this repository: code that runs somewhere Rust does not reach
cheaply. Each directory is its own small project with no npm dependencies, run with the
Node.js and `wrangler` that nixpkgs ships.

- [`telemetry/`](telemetry) — the Cloudflare Worker on `t.drawbar.app` that collects
  drawbar.app's anonymous usage counts and the reports people send from the app.

Tests run in `nix flake check`, or by hand:

```sh
nix shell --inputs-from .. nixpkgs#nodejs -c node --test telemetry/test/check.test.js
```
