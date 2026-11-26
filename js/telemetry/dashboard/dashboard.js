// A private dashboard of `drawbar_events`, served on this machine. Each load reads what
// Analytics Engine holds and makes the page from it.
//
// ⚠️ Nothing is stored here. The privacy page says rows are kept for three months, and
// the dataset's own retention is what keeps that promise.

import { execFileSync, spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { createServer } from "node:http";

const API = "https://api.cloudflare.com/client/v4";
const PORT = Number(process.env.PORT ?? 8788);
// The names a browser on this machine calls the page by.
const HOSTS = [`127.0.0.1:${PORT}`, `localhost:${PORT}`];

// Analytics Engine gives a row twenty blobs.
const BLOBS = Array.from({ length: 20 }, (_, i) => `blob${i + 1}`);

// Rows read for one page. A dataset holding more is an error, not a short read.
const MOST_ROWS = 100_000;

// What rows from a test against the real dataset carry as their version.
const TEST_VERSION = "0.0.0-test";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");

// A stored row as the collector took it: the fields a browser sent, under their names
// and types, then the edge fields the collector added. `null` for a kind the schema does
// not have.
export function named(stored, schema) {
  const kind = Object.hasOwn(schema, stored.index1) ? schema[stored.index1] : null;
  if (!kind) {
    return null;
  }
  const types = [...Object.entries(kind.fields), ...kind.edge.map((name) => [name, "text"])];
  const fields = types.map(([name, type], i) => {
    const blob = stored[BLOBS[i]];
    return [name, type === "flag" ? blob === "1" : blob];
  });
  return {
    at: stored.timestamp,
    weight: Number(stored._sample_interval),
    row: { event: stored.index1, ...Object.fromEntries(fields) },
  };
}

async function cloudflare(path, token, init) {
  const response = await fetch(`${API}${path}`, {
    ...init,
    headers: { authorization: `Bearer ${token}` },
  });
  if (!response.ok) {
    throw new Error(`Cloudflare answered ${response.status}: ${await response.text()}`);
  }
  return response.json();
}

// The token of `wrangler login`, unless the environment gives one.
function token() {
  if (process.env.CLOUDFLARE_API_TOKEN) {
    return process.env.CLOUDFLARE_API_TOKEN;
  }
  const out = execFileSync("wrangler", ["auth", "token", "--json"], {
    encoding: "utf8",
    env: { ...process.env, WRANGLER_SEND_METRICS: "false" },
  });
  return JSON.parse(out).token;
}

async function account(token) {
  if (process.env.CLOUDFLARE_ACCOUNT_ID) {
    return process.env.CLOUDFLARE_ACCOUNT_ID;
  }
  const { result } = await cloudflare("/accounts", token);
  if (result.length !== 1) {
    throw new Error(`the token reads ${result.length} accounts: set CLOUDFLARE_ACCOUNT_ID`);
  }
  return result[0].id;
}

// Every row the dataset holds, oldest first.
async function stored() {
  const bearer = token();
  const sql = `SELECT timestamp, index1, _sample_interval, ${BLOBS.join(", ")}
    FROM drawbar_events ORDER BY timestamp LIMIT ${MOST_ROWS} FORMAT JSON`;
  const answer = await cloudflare(
    `/accounts/${await account(bearer)}/analytics_engine/sql`,
    bearer,
    {
      method: "POST",
      body: sql,
    },
  );
  if (answer.rows_before_limit_at_least > answer.data.length) {
    throw new Error(`the dataset holds more than ${MOST_ROWS} rows`);
  }
  return answer.data;
}

async function page() {
  const schema = JSON.parse(read("../../../crates/drawbar/telemetry.json"));
  const events = (await stored())
    .map((row) => named(row, schema))
    .filter((event) => event && event.row.version !== TEST_VERSION);
  // `<` is escaped so no stored text can close the script element it is embedded in.
  const embedded = JSON.stringify({ pulled: new Date().toISOString(), events }).replaceAll(
    "<",
    "\\u003c",
  );
  return read("./dashboard.html").replace("__DATA__", () => embedded);
}

function serve() {
  const server = createServer(async (request, response) => {
    // ⚠️ The `Host` check is what stops another site, rebound to this address, from
    // reading the page.
    if (!HOSTS.includes(request.headers.host) || request.url !== "/") {
      response.writeHead(404).end();
      return;
    }
    try {
      const html = await page();
      response
        .writeHead(200, { "cache-control": "no-store", "content-type": "text/html; charset=utf-8" })
        .end(html);
    } catch (error) {
      response.writeHead(502, { "content-type": "text/plain; charset=utf-8" }).end(String(error));
    }
  });
  server.listen(PORT, "127.0.0.1", () => {
    const url = `http://${HOSTS[0]}/`;
    console.log(url);
    const opener = process.platform === "darwin" ? "open" : "xdg-open";
    spawn(opener, [url], { stdio: "ignore" }).on("error", () => {});
  });
}

if (import.meta.main) {
  serve();
}
