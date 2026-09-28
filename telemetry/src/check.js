// What the collector accepts. Anything that does not match exactly is dropped, since
// anyone can post to a public endpoint.

// Whether a request's `Origin` header allows it. Only drawbar.app sends; a request
// with no `Origin` is let through, since not every browser sends one with a beacon.
export function allowed(origin) {
  return origin === null || origin === "https://drawbar.app";
}

// Rows in one beacon.
export const MOST_ROWS = 50;

// Characters in one text field.
const LONGEST_TEXT = 160;

// What a text field may hold: codes, versions, source locations, product names, hosts.
const CODE = /^[A-Za-z0-9 _.:/+<>()-]*$/;

// Rows from a beacon's body, as Analytics Engine data points. `edge` holds what the
// collector adds itself: `browser`, `os` and `country`.
export function rows(body, schema, edge) {
  let parsed;
  try {
    parsed = JSON.parse(body);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed) || parsed.length > MOST_ROWS) {
    return [];
  }
  return parsed.map((row) => point(row, schema, edge)).filter(Boolean);
}

function point(row, schema, edge) {
  if (row === null || typeof row !== "object" || Array.isArray(row)) {
    return null;
  }
  const kind = Object.hasOwn(schema, row.event) ? schema[row.event] : null;
  if (!kind) {
    return null;
  }
  const names = Object.keys(kind.fields);
  const sent = Object.keys(row).filter((name) => name !== "event");
  if (sent.length !== names.length || !sent.every((name) => Object.hasOwn(kind.fields, name))) {
    return null;
  }
  const blobs = [];
  for (const name of names) {
    const blob = value(row[name], kind.fields[name]);
    if (blob === null) {
      return null;
    }
    blobs.push(blob);
  }
  for (const name of kind.edge) {
    blobs.push(edge[name] ?? "");
  }
  return { indexes: [row.event], blobs };
}

function value(sent, type) {
  switch (type) {
    case "flag":
      return typeof sent === "boolean" ? (sent ? "1" : "0") : null;
    case "text":
      return typeof sent === "string" && sent.length <= LONGEST_TEXT && CODE.test(sent)
        ? sent
        : null;
    default:
      return null;
  }
}

// The limits of a report's fields, in characters.
const REPORT = {
  id: 10,
  kind: 8,
  text: 5_000,
  contact: 200,
  version: 32,
  model: 80,
  firmware: 16,
  faults: 2_000,
  build: 4_000,
  log: 100_000,
};

const ID = /^[2-9a-hjkmnp-z]{10}$/;

// A report's fields, or `null` if it is not one. `edge` supplies `browser` and `os`.
export function report(body, edge) {
  let parsed;
  try {
    parsed = JSON.parse(body);
  } catch {
    return null;
  }
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    return null;
  }
  const names = Object.keys(REPORT);
  const sent = Object.keys(parsed);
  if (sent.length !== names.length || !sent.every((name) => Object.hasOwn(REPORT, name))) {
    return null;
  }
  for (const name of names) {
    const field = parsed[name];
    if (typeof field !== "string" || field.length > REPORT[name]) {
      return null;
    }
  }
  if (!ID.test(parsed.id) || !["problem", "feedback"].includes(parsed.kind) || !parsed.text) {
    return null;
  }
  return { ...parsed, browser: edge.browser ?? "", os: edge.os ?? "" };
}

// The browser family and major version, and the operating system family, from a user
// agent. The user agent itself is not kept.
export function agent(ua) {
  return { browser: browser(ua), os: os(ua) };
}

function browser(ua) {
  const found = [
    ["edge", /Edg\/(\d+)/],
    ["opera", /OPR\/(\d+)/],
    ["samsung", /SamsungBrowser\/(\d+)/],
    ["chrome", /Chrome\/(\d+)/],
    ["firefox", /Firefox\/(\d+)/],
    ["safari", /Version\/(\d+)[^ ]* (?:Mobile\/\S+ )?Safari\//],
  ].find(([, pattern]) => pattern.test(ua));
  if (!found) {
    return "other";
  }
  const [name, pattern] = found;
  return `${name} ${ua.match(pattern)[1]}`;
}

function os(ua) {
  const found = [
    ["windows", /Windows/],
    ["android", /Android/],
    ["ios", /iPhone|iPad|iPod/],
    ["chromeos", /CrOS/],
    ["macos", /Mac OS X|Macintosh/],
    ["linux", /Linux/],
  ].find(([, pattern]) => pattern.test(ua));
  return found ? found[0] : "other";
}
