// The collector. `POST /e` takes a beacon of anonymous rows into Analytics Engine;
// `POST /report` emails one report an operator sent to reports@drawbar.app, exactly as
// sent. Neither keeps the client's address or user agent.

import { admitted, agent, allowed, report, rows } from "./check.js";
import { message } from "./mail.js";
import schema from "../../../crates/drawbar/telemetry.json";

const ORIGIN = "https://drawbar.app";

// The largest body read, in bytes: a report with its whole log attached fits.
const LARGEST = 256 * 1024;

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
    const path = new URL(request.url).pathname;
    if (!admitted(path, origin, env.DEV_ORIGIN)) {
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
    switch (path) {
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
        const { success } = await env.REPORT_RATE.limit({ key: "report" });
        if (!success) {
          return reply(429);
        }
        try {
          await env.REPORTS.send(message(sent));
        } catch {
          // The app sends a report again after any 5xx.
          return reply(503);
        }
        return reply(204);
      }
      default:
        return reply(404);
    }
  },
};
