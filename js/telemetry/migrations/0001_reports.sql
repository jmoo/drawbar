-- One row per report an operator sent. Deleted three months after `created`.
CREATE TABLE reports (
  id TEXT PRIMARY KEY,
  created INTEGER NOT NULL DEFAULT (unixepoch()),
  kind TEXT NOT NULL,
  text TEXT NOT NULL,
  contact TEXT NOT NULL,
  version TEXT NOT NULL,
  model TEXT NOT NULL,
  firmware TEXT NOT NULL,
  faults TEXT NOT NULL,
  build TEXT NOT NULL,
  log TEXT NOT NULL
);

CREATE INDEX reports_created ON reports (created);
