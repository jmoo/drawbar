import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { MOST_ROWS, agent, allowed, report, rows } from "../src/check.js";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const schema = JSON.parse(read("../../crates/drawbar/telemetry.json"));
const edge = { browser: "chrome 141", os: "macos", country: "DE" };

const visit = {
  event: "visit",
  version: "0.9.1",
  navigation: "navigate",
  first_day: true,
  first_month: false,
  webusb: true,
  fits: true,
  referrer: "example.com",
  language: "de",
};

const panic = {
  event: "panic",
  version: "0.9.1",
  location: "crates/drawbar/src/app.rs:10:5",
  model: "Nord Electro 5D",
  firmware: "2.04",
};

const beacon = (...sent) => JSON.stringify(sent);

test("a visit is stored in field order, then its edge fields", () => {
  assert.deepEqual(rows(beacon(visit), schema, edge), [
    {
      indexes: ["visit"],
      blobs: ["0.9.1", "navigate", "1", "0", "1", "1", "example.com", "de", "chrome 141", "macos", "DE"],
    },
  ]);
});

test("a fault row carries no country", () => {
  const [point] = rows(beacon(panic), schema, edge);
  assert.deepEqual(point.blobs.slice(-2), ["chrome 141", "macos"]);
  assert.ok(!point.blobs.includes("DE"));
});

test("a row with a field its kind does not declare is dropped", () => {
  assert.deepEqual(rows(beacon({ ...panic, ip: "1.2.3.4" }), schema, edge), []);
});

test("a row missing a field is dropped", () => {
  const { firmware: _, ...short } = panic;
  assert.deepEqual(rows(beacon(short), schema, edge), []);
});

test("free text is dropped", () => {
  assert.deepEqual(rows(beacon({ ...panic, location: "my song \"Blue\"" }), schema, edge), []);
  assert.deepEqual(rows(beacon({ ...panic, model: "x".repeat(161) }), schema, edge), []);
});

test("a flag must be a boolean", () => {
  assert.deepEqual(rows(beacon({ ...visit, fits: "yes" }), schema, edge), []);
});

test("an unknown kind is dropped and the rest of the beacon kept", () => {
  const kept = rows(beacon({ event: "__proto__" }, { event: "click" }, panic), schema, edge);
  assert.equal(kept.length, 1);
  assert.deepEqual(kept[0].indexes, ["panic"]);
});

test("an oversized or malformed beacon stores nothing", () => {
  const many = Array.from({ length: MOST_ROWS + 1 }, () => panic);
  assert.deepEqual(rows(JSON.stringify(many), schema, edge), []);
  assert.deepEqual(rows("not json", schema, edge), []);
  assert.deepEqual(rows(JSON.stringify(panic), schema, edge), []);
});

const sent = {
  id: "abcdefgh23",
  kind: "problem",
  text: "The piano editor froze.",
  contact: "",
  version: "0.9.1",
  model: "",
  firmware: "",
  faults: "usb: lost",
  build: "",
  log: "",
};

test("a report keeps its fields and gains the browser and os", () => {
  assert.deepEqual(report(JSON.stringify(sent), edge), { ...sent, browser: "chrome 141", os: "macos" });
});

test("a report is refused without text, with a bad id, or with an extra field", () => {
  assert.equal(report(JSON.stringify({ ...sent, text: "" }), edge), null);
  assert.equal(report(JSON.stringify({ ...sent, id: "ABCDEFGH23" }), edge), null);
  assert.equal(report(JSON.stringify({ ...sent, id: "abcdefghi1" }), edge), null);
  assert.equal(report(JSON.stringify({ ...sent, ip: "1.2.3.4" }), edge), null);
  assert.equal(report(JSON.stringify({ ...sent, text: "x".repeat(5_001) }), edge), null);
});

test("the user agent is reduced to a family, a major version and an os", () => {
  const cases = [
    [
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36",
      { browser: "chrome 141", os: "macos" },
    ],
    [
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 Edg/141.0.0.0",
      { browser: "edge 141", os: "windows" },
    ],
    [
      "Mozilla/5.0 (X11; Linux x86_64; rv:143.0) Gecko/20100101 Firefox/143.0",
      { browser: "firefox 143", os: "linux" },
    ],
    [
      "Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.6 Mobile/15E148 Safari/604.1",
      { browser: "safari 18", os: "ios" },
    ],
    [
      "Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Mobile Safari/537.36",
      { browser: "chrome 141", os: "android" },
    ],
    ["", { browser: "other", os: "other" }],
  ];
  for (const [ua, expected] of cases) {
    assert.deepEqual(agent(ua), expected, ua);
  }
});

test("the privacy page names every row and field", () => {
  const page = read("../../docs/src/privacy.md");
  for (const [kind, { fields, edge: added }] of Object.entries(schema)) {
    for (const name of [kind, ...Object.keys(fields), ...added]) {
      assert.ok(page.includes(`\`${name}\``), `the privacy page does not name \`${name}\` of \`${kind}\``);
    }
  }
});

test("only drawbar.app, or a request that names no origin, is accepted", () => {
  assert.ok(allowed("https://drawbar.app"));
  assert.ok(allowed(null));
  assert.ok(!allowed("http://127.0.0.1:8765"));
  assert.ok(!allowed("https://fork.example"));
});
