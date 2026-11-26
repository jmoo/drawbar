import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { named } from "../dashboard/dashboard.js";
import { rows } from "../src/check.js";

const schema = JSON.parse(
  readFileSync(new URL("../../../crates/drawbar/telemetry.json", import.meta.url), "utf8"),
);
const edge = { browser: "chrome 141", os: "macos", country: "DE" };

// A beacon row as Analytics Engine hands it back after the collector wrote it.
function stored(sent) {
  const [point] = rows(JSON.stringify([sent]), schema, edge);
  const blobs = Object.fromEntries(point.blobs.map((blob, i) => [`blob${i + 1}`, blob]));
  return {
    timestamp: "2026-01-02 03:04:05",
    index1: point.indexes[0],
    _sample_interval: "2",
    ...blobs,
  };
}

test("the dashboard reads a row back as it was sent, with the edge fields of its kind", () => {
  const visit = {
    event: "visit",
    version: "0.9.1",
    navigation: "navigate",
    first_day: true,
    first_month: false,
    webusb: true,
    fits: false,
    referrer: "example.com",
    language: "de",
  };
  const error = {
    event: "error",
    version: "0.9.1",
    domain: "usb",
    kind: "lost",
    model: "",
    firmware: "2.04",
  };

  assert.deepEqual(named(stored(visit), schema), {
    at: "2026-01-02 03:04:05",
    weight: 2,
    row: { ...visit, ...edge },
  });
  assert.deepEqual(named(stored(error), schema).row, {
    ...error,
    browser: "chrome 141",
    os: "macos",
  });
});

test("the dashboard leaves out a row of a kind the schema does not have", () => {
  assert.equal(
    named({ timestamp: "2026-01-02 03:04:05", index1: "retired", _sample_interval: 1 }, schema),
    null,
  );
});
