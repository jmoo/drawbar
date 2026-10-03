// The collector. `POST /e` takes a beacon of anonymous rows into Analytics Engine;
// `POST /report` takes one report an operator sent into D1. Neither keeps the client's
// address or user agent.

import { agent, allowed, report, rows } from "./check.js";
import schema from "../../crates/drawbar/telemetry.json";

const ORIGIN = "https://drawbar.app";

// The largest body read, in bytes: a report with its whole log attached fits.
const LARGEST = 256 * 1024;

// How long a report is kept.
const KEEP_SECONDS = 90 * 24 * 60 * 60;

const HEADERS = {
  "access-control-allow-origin": ORIGIN,
  "access-control-allow-methods": "POST",
  "access-control-allow-headers": "content-type",
};

function answer(status) {
  return new Response(null, { status, headers: HEADERS });
}

export default {
  async fetch(request, env) {
    if (request.method === "OPTIONS") {
      return answer(204);
    }
    if (request.method !== "POST") {
      return answer(405);
    }
    if (!allowed(request.headers.get("origin"))) {
      return answer(403);
    }
    const declared = Number(request.headers.get("content-length") ?? 0);
    if (declared > LARGEST) {
      return answer(413);
    }
    const body = await request.text();
    if (body.length > LARGEST) {
      return answer(413);
    }
    const edge = {
      ...agent(request.headers.get("user-agent") ?? ""),
      country: request.cf?.country ?? "",
    };
    switch (new URL(request.url).pathname) {
      case "/e":
        for (const point of rows(body, schema, edge)) {
          env.EVENTS.writeDataPoint(point);
        }
        return answer(204);
      case "/report": {
        const sent = report(body, edge);
        if (!sent) {
          return answer(400);
        }
        await env.REPORTS.prepare(
          `INSERT OR IGNORE INTO reports
             (id, kind, text, contact, version, model, firmware, browser, os, faults, build, log)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
        )
          .bind(
            sent.id,
            sent.kind,
            sent.text,
            sent.contact,
            sent.version,
            sent.model,
            sent.firmware,
            sent.browser,
            sent.os,
            sent.faults,
            sent.build,
            sent.log,
          )
          .run();
        return answer(204);
      }
      default:
        return answer(404);
    }
  },

  async scheduled(_controller, env) {
    await env.REPORTS.prepare("DELETE FROM reports WHERE created < unixepoch() - ?")
      .bind(KEEP_SECONDS)
      .run();
  },
};
