-- V10__probe_failures.sql — persisted ffprobe failures for enrichment observability
-- (Task 96 Part C). The next free refinery version after V9__fanart_wallpapers.sql.
-- Additive DDL only (no PRAGMAs — see migrations/README.md).
--
-- The ingest worker skips a file whose ffprobe errors (bad container, truncated download,
-- unsupported codec). That was log-only, so a silently-missing title was unexplainable
-- without reading container logs. This table records each such failure keyed by path so
-- `GET /api/status/probe-failures` can list them, and a subsequent successful re-probe of
-- the same path clears its row (the worker deletes it on success).

CREATE TABLE probe_failures (
    path            TEXT PRIMARY KEY,   -- absolute media path that failed ffprobe
    error           TEXT NOT NULL,      -- the ffprobe error text (for the operator to diagnose)
    last_attempt_at INTEGER NOT NULL    -- unix seconds of the most recent failed attempt
);

-- List newest failures first in the status UI.
CREATE INDEX idx_probe_failures_time ON probe_failures(last_attempt_at DESC);
