// The collector. `POST /e` takes a beacon of anonymous rows into Analytics Engine;
// `POST /report` takes one report an operator sent into D1, exactly as sent. Neither
// keeps the client's address or user agent.

import { agent, allowed, report, rows } from "./check.js";
import schema from "../../crates/drawbar/telemetry.json";

const ORIGIN = "https://drawbar.app";

// The largest body read, in bytes: a report with its whole log attached fits.
const LARGEST = 256 * 1024;

// The most reports kept from any 24 hours, so a sender past the edge's per-address
// rate limit can fill a day rather than the database.
const MOST_A_DAY = 100;

// How long a report is kept.
const KEEP_SECONDS = 90 * 24 * 60 * 60;

// `origin` is the one the request may be read from: drawbar.app, or a local run's.
function answer(status, origin = ORIGIN) {
  return new Response(null, {
    status,
    headers: {
      "access-control-allow-origin": origin,
      "access-control-allow-methods": "POST",
      "access-control-allow-headers": "content-type",
    },
  });
}

export default {
  async fetch(request, env) {
    const origin = request.headers.get("origin");
    const reader = allowed(origin, env.DEV_ORIGIN) ? origin : ORIGIN;
    const reply = (status) => answer(status, reader);
    if (request.method === "OPTIONS") {
      return reply(204);
    }
    if (request.method !== "POST") {
      return reply(405);
    }
    if (!allowed(origin, env.DEV_ORIGIN)) {
      return reply(403);
    }
    const declared = Number(request.headers.get("content-length") ?? 0);
    if (declared > LARGEST) {
      return reply(413);
    }
    const body = await request.text();
    if (body.length > LARGEST) {
      return reply(413);
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
        return reply(204);
      case "/report": {
        const sent = report(body);
        if (!sent) {
          return reply(400);
        }
        // A retry of a report already kept is answered as kept, before the daily cap
        // can refuse it.
        const kept = await env.REPORTS.prepare("SELECT 1 AS kept FROM reports WHERE id = ?")
          .bind(sent.id)
          .first("kept");
        if (kept) {
          return reply(204);
        }
        const today = await env.REPORTS.prepare(
          "SELECT COUNT(*) AS n FROM reports WHERE created > unixepoch() - 86400",
        ).first("n");
        if (today >= MOST_A_DAY) {
          return reply(429);
        }
        await env.REPORTS.prepare(
          `INSERT OR IGNORE INTO reports
             (id, kind, text, contact, version, model, firmware, faults, build, log)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
        )
          .bind(
            sent.id,
            sent.kind,
            sent.text,
            sent.contact,
            sent.version,
            sent.model,
            sent.firmware,
            sent.faults,
            sent.build,
            sent.log,
          )
          .run();
        return reply(204);
      }
      default:
        return reply(404);
    }
  },

  async scheduled(_controller, env) {
    await env.REPORTS.prepare("DELETE FROM reports WHERE created < unixepoch() - ?")
      .bind(KEEP_SECONDS)
      .run();
  },
};
