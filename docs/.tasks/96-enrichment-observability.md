# 96 — Enrichment Visibility & Auto-Run Reliability

> **Status: BUILT (session 2026-09-03).** Parts A–D + the Part C store shipped; full backend
> `cargo test --workspace` green (18 new/existing db tests + 2 status tests), web typecheck +
> `vite build` green. Part E (setting the fanart key on the *Unraid* box) is operator config,
> not code — the new `/api/status` `fanart.configured=false` chip surfaces it. Fixes the
> reported symptom *"cron/scheduler jobs don't seem to run and there's no visibility; no
> titles/logos download automatically — I have to match manually."* This task is **mostly
> observability + a couple of small reliability gaps**, not a rewrite: the enrichment pipeline
> was already wired and *is* running (see §Diagnosis). The real defect was that the system was
> a black box, so correct-but-invisible behavior looked broken.
>
> **Delivered:** `GET /api/status` (counts + provider chips incl. fanart, last scan/enrichment,
> watcher liveness), `GET /api/status/unmatched`, `GET /api/status/probe-failures`,
> `POST /api/metadata/enrich`; boot-time pending sweep + periodic pending-drain in `main.rs`
> (Part B gaps #5/#6); `V10__probe_failures.sql` + record/clear in the worker (Part C); a shared
> `medi_ingest::EnrichmentStatus` handle the worker updates and the API reads; a web
> `/settings/status` page (counts, provider chips, "Run enrichment/backfill now", unmatched list
> with per-row Fix-match reusing `MatchDialog`, probe-failure list) + a header nav link +
> api-client types/methods.

## Diagnosis (what's actually happening — verified live on the Unraid box, 2026-09-03)

Evidence gathered from the running `ghcr.io/michelve/medi:latest` container over SSH:

1. **Auto-enrichment IS running.** Logs show every scan that writes ≥1 row kicks
   `run_enrichment`, which pulls a batch of 200 `pending` movies and matches them. ~1118
   `poster.jpg` files are on disk — so ~1100 movies enriched **automatically**, no manual step.
   The pipeline is alive.

2. **Logos are 0 because `FANARTTV_API_KEY` is NOT set on the box.** The key exists only in the
   git-ignored dev `docker/compose.dev.override.yml`, never on the Unraid container's env. With
   no key, `build_fanart()` returns `None` and the logo/wallpaper fetch is inert **by design**
   (graceful degradation). `find … -name logo.png` → **0**. This is *configuration*, not a bug —
   but nothing tells the operator "fanart is off because the key is missing."

3. **A large block of front-of-alphabet titles is genuinely `unmatched`.** Of the first 200
   library items, only 11 have a poster. Logs show why: `no candidate cleared match threshold →
   unmatched` for titles like `Culpa Mia`, `Ghandi` (misspelled), `veto-shang chi the legend of
   the ten rings`, `Assassins Bullet 1` — junk/misspelled/foreign folder names TMDB can't match.
   This is *correct* (the match threshold is doing its job), but the operator has no list of
   "these N are unmatched, here's why" to act on (fix the folder name, or pin a match).

4. **Recurring `ffprobe failed; skipping`** on a handful of `.mp4` files (James Bond
   collection, Harry Potter, a few YTS rips) every scan. Those files never get a `media_files`
   row, so their movie stays empty — and the failure scrolls past in logs with no durable
   record.

5. **Auto-enrichment only fires when a scan writes ≥1 new row** (`worker.rs`: the
   `if written > 0 { run_enrichment }` gate). On a restart where every file is already in
   `scan_state`, the initial scan writes 0 → the `pending` backlog is **not** swept at boot.
   It only advances when a *new* file lands (which then processes a batch of 200 including old
   pending rows). So a backlog can sit untouched for a long time with no new files.

6. **The periodic backfill is `matched`-only and first-runs after a full interval.** It fills
   *new fields on already-matched titles* (fanart art, genres…) — it does **not** enrich
   `pending`/`unmatched` titles. Default `BACKFILL_INTERVAL_HOURS=24`, and the immediate first
   tick is consumed, so its first run is ~24h after boot.

7. **There is no status surface at all.** The only endpoint is `/api/health → "ok"`. No count
   of pending/matched/unmatched/failed, no "last scan / last enrichment ran at", no unmatched
   list, no ffprobe-failure list, no "fanart: disabled (no key)" signal. Everything is in
   `tracing` logs the operator never reads. **This is the core of the complaint.**

**Conclusion:** the scheduler/worker do run; manual match works because it bypasses the
`written > 0` gate. The fixes are (A) **make it observable**, (B) **close the two auto-run gaps**
(boot-time pending sweep; periodic pass covers pending too), and (C) **surface config/ffprobe
problems** so "why is this title empty" is answerable without SSH.

## Goals

- An operator (and the web UI) can **see enrichment state at a glance**: how many titles are
  matched / pending / unmatched / failed, whether a provider + fanart are configured, and when
  the last scan and last enrichment pass ran.
- **Nothing needing metadata sits forever.** A boot-time and periodic pass drains the
  `pending`/`failed` backlog even when no new files land.
- **Unmatched and ffprobe-failed titles are listable**, so the operator can fix a folder name,
  pin a match, or re-probe — the actionable follow-through the logs hide today.
- **Zero behavior change when everything is already working** — this is additive.

## Non-goals

- Rewriting the scheduler or the enrichment pipeline (they work).
- A full job-queue / task-runner abstraction. The existing spawned tasks + single-flight
  backfill guard are enough; this task observes and lightly extends them.
- Auto-fixing bad folder names or auto-pinning fuzzy matches (still a human decision; we just
  make the list visible).
- Series logos / fanart TV (separate deferred task).

---

## Part A — Status & observability API (the headline fix)

Add read-only status endpoints, cached briefly (or uncached — they're cheap counts) and
surfaced in the admin UI.

| Method | Path | Returns |
|--------|------|---------|
| `GET` | `/api/status` | System + enrichment status envelope (below). |
| `GET` | `/api/status/unmatched?kind=movie&cursor=&limit=` | Keyset page of `unmatched`/`failed` titles: `{ id, kind, title, year, state, path }` so the operator can see *what* didn't match and where it lives on disk. |
| `GET` | `/api/status/probe-failures?cursor=&limit=` | Files whose last ffprobe errored (see Part C for the store): `{ path, error, last_attempt_at }`. |

`GET /api/status` envelope (shape is the contract; keep names snake_case to match the wire):

```jsonc
{
  "version": "0.2.0",
  "media_dir_present": true,
  "counts": {
    "movies": { "total": 1300, "matched": 1100, "pending": 5, "unmatched": 189, "failed": 6 },
    "series": { "total": 40,  "matched": 38,  "pending": 0, "unmatched": 2,  "failed": 0 }
  },
  "providers": {
    "metadata": { "name": "tmdb", "configured": true },   // active provider + whether its key is set
    "fanart":   { "configured": false }                    // ← surfaces the missing FANARTTV_API_KEY
  },
  "last_scan":       { "started_at": 1725370000, "finished_at": 1725370005, "written": 1, "probe_failures": 7 },
  "last_enrichment": { "finished_at": 1725370010, "matched": 3, "unmatched": 12, "failed": 0 },
  "workers": { "watcher_alive": true, "backfill_interval_hours": 24 }
}
```

**Where the data comes from:**

- **counts** — one grouped SQL per kind: `SELECT metadata_state, COUNT(*) FROM movies GROUP BY
  metadata_state` (add `list_metadata_state_counts(conn, kind)` in `queries.rs`). `total` is the
  sum. Fast even on 10k titles (indexed by `idx_*_meta_state`).
- **providers** — from `AppConfig`: `metadata_provider` + `active_metadata_key().is_some()`,
  and `fanart_enabled()` (already exists per task 93). **This one line is what tells the
  operator "logos are off because you never set the fanart key."**
- **last_scan / last_enrichment / watcher_alive** — a small in-memory `EnrichmentStatus` struct
  behind an `Arc<RwLock<…>>` (or `Arc<Mutex<…>>`) on `AppState`, updated by the worker:
  `run_scan` records started/finished/written/probe-failure count; `run_enrichment` records its
  tallies; the watch loop sets `watcher_alive = true` when it starts and the initial-scan task
  can flip it. No DB table needed — status is ephemeral and cheap; it resets on restart, which
  is fine (the counts, the durable part, come from the DB).

**Wiring:** thread the shared status handle into `WorkerConfig` / `run_scan` / `run_enrichment`
(an `Option<StatusSink>` so tests and non-API callers can pass `None`), mirroring how the
`Invalidator` callback is already threaded. Add the routes in `router()` next to `/api/health`.

## Part B — Close the auto-run gaps

Two small, surgical changes so the `pending` backlog can't stall:

1. **Boot-time pending sweep.** After the initial `run_scan` completes in `main.rs`, if a
   provider is configured, run **one** `run_enrichment` pass unconditionally (not gated on
   `written > 0`). Today a restart with no new files never touches the backlog. This is a couple
   of lines in the existing `tokio::spawn` block in `main.rs` (call `run_enrichment` once before
   entering `watch`). Idempotent — matched/unmatched rows are skipped, so it's cheap when the
   library is already enriched.

2. **Periodic pass also drains `pending`, and first-runs sooner.** Extend the periodic task in
   `main.rs` (currently backfill-only) to *also* call `run_enrichment` each tick, so
   `pending`/`failed` titles get retried on a schedule even with no filesystem activity.
   Consider a **shorter default for this pending-retry cadence** than the 24h backfill (e.g. a
   separate `ENRICH_RETRY_INTERVAL_HOURS`, default 6) — or simply don't consume the immediate
   first tick for the enrichment half, so a fresh boot retries pending within the first interval
   instead of 24h later. Keep it behind the same single-flight guard so passes never overlap.

> Both changes reuse `run_enrichment` verbatim — no new enrichment logic, just *when* it's
> invoked. The `written > 0` auto-enrich inside `run_scan` stays (it's the fast path for a newly
> dropped file); B just adds two backstops so nothing waits on a new file to be processed.

## Part C — Record ffprobe failures (make "why is this empty" answerable)

Today `ffprobe failed; skipping` only logs. Persist it so `/api/status/probe-failures` can list
it and a re-scan can retry:

- Add a tiny store for probe failures. Simplest: a `probe_failures` table
  (`path TEXT PRIMARY KEY, error TEXT, last_attempt_at INTEGER`) written when
  `ffprobe::probe` errors in the worker, and **cleared on a subsequent successful probe** of the
  same path. Migration **V10** (next after V9 — see [[medi-migration-numbering]]).
  - Alternative if a table feels heavy: reuse `scan_state` with a nullable `probe_error` column.
    A dedicated table is cleaner and keeps the hot `scan_state` row narrow — prefer the table.
- The worker's probe-error branch (currently just `tracing::warn!`) also upserts the failure row
  and bumps the `last_scan.probe_failures` status counter.
- This turns "7 files silently missing" into a visible, actionable list (bad container, truncated
  download, unsupported codec → the operator can delete/replace the file).

## Part D — Web admin: a Status page

Surface Part A in the existing admin area (the Libraries settings live at `/settings/libraries`
per [[medi-web-client]]). Add `/settings/status` (or fold a panel into the libraries page):

- **Enrichment summary**: the counts as a small bar/table per kind (matched / pending /
  unmatched / failed), and "last scan / last enrichment" timestamps ("2 min ago").
- **Provider chips**: `TMDB ✓  fanart ✗ (no key)` — the single most useful line for this exact
  complaint. When fanart is unconfigured, show a hint: *"Set FANARTTV_API_KEY to enable movie
  title logos."*
- **Unmatched list** (from `/api/status/unmatched`): title + on-disk path + a **"Fix match"**
  button that opens the **existing `MatchDialog`** (already built, task 82) so the operator can
  pin a match right there. This directly closes the loop the user hits today ("I have to match
  manually") — now they can *find* what needs matching instead of hunting.
- **Probe failures list** (from `/api/status/probe-failures`): path + error, so bad files are
  visible.
- A **"Run enrichment now"** / **"Backfill now"** button wired to the existing
  `POST /api/metadata/backfill` (and, if added, a `POST /api/metadata/enrich` that triggers a
  `run_enrichment` pass) so the operator can kick a pass on demand instead of waiting for the
  schedule — with the response toasting the resulting counts.

New api-client methods + types for the three status endpoints (extend
`packages/api-client/src/types.ts`, the single source of truth — task 40).

## Part E — Ops: actually set the fanart key on the box

Independent of the code, the running container is missing `FANARTTV_API_KEY` (see Diagnosis #2).
As part of shipping this:

- Set `FANARTTV_API_KEY` on the Unraid container (the operator's real key) so logos download.
  Then a `POST /api/metadata/backfill` fills logos for the already-matched ~1100 movies.
- Confirm the Unraid **template** exposes `FANARTTV_API_KEY` (task 94 §Ops covers the template +
  README wiring) so a fresh install prompts for it. The `/api/status` `fanart.configured: false`
  chip is the durable guardrail that makes a missing key obvious next time.

## File structure (where to save)

```
backend/
├── migrations/
│   └── V10__probe_failures.sql        # NEW — Part C (next free version after V9)
└── crates/
    ├── core/src/config.rs             # (maybe) ENRICH_RETRY_INTERVAL_HOURS; expose provider/fanart status helpers
    ├── db/src/
    │   ├── queries.rs                 # list_metadata_state_counts; list_unmatched (keyset); probe-failure reads
    │   ├── writes.rs                  # upsert_probe_failure / clear_probe_failure
    │   └── models.rs                  # MetadataStateCounts, UnmatchedTitle, ProbeFailure
    ├── ingest/src/
    │   ├── worker.rs                  # record scan/probe status; write probe_failures; status sink
    │   └── enrich.rs                  # record enrichment tallies into the status sink
    └── api/src/
        ├── state.rs                   # Arc<RwLock<EnrichmentStatus>> on AppState
        ├── status.rs                  # NEW — EnrichmentStatus struct + /api/status* handlers
        ├── routes.rs                  # wire /api/status, /api/status/unmatched, /api/status/probe-failures; maybe /api/metadata/enrich
        └── main.rs                    # boot-time pending sweep; periodic pass drains pending; pass status handle to worker

client/
├── packages/api-client/src/
│   ├── types.ts                       # SystemStatus, UnmatchedTitle, ProbeFailure
│   └── client.ts                      # status(), unmatched(), probeFailures() methods
└── apps/web/src/
    ├── router.tsx                     # + /settings/status
    ├── pages/StatusPage.tsx           # NEW — counts, provider chips, unmatched + probe lists, run-now buttons
    └── (reuse MatchDialog from task 82 for the per-row Fix match)

docs/.tasks/96-enrichment-observability.md   # this file
```

## Testing

- **db**: `list_metadata_state_counts` returns correct per-state counts on a seeded mix;
  `list_unmatched` keyset-paginates and returns only `unmatched`/`failed`;
  `upsert_probe_failure` then `clear_probe_failure` round-trips (a successful re-probe clears it).
- **ingest**: `run_scan` with an injected status sink records started/finished/written and a
  probe-failure count; a probe error writes a `probe_failures` row and a subsequent success
  clears it. `run_enrichment` records matched/unmatched/failed tallies.
- **api**: `GET /api/status` returns the documented envelope with correct counts and
  `fanart.configured=false` when the key is unset (and `true` when set);
  `/api/status/unmatched` first-page + cursor; `/api/status/probe-failures` lists a seeded
  failure. Boot-time sweep: with a provider configured and pending rows present but `written==0`,
  the initial pass still enriches them (integration-style test around the `main.rs` wiring, or a
  unit test of the extracted "sweep once" helper).
- **web**: typecheck + `vite build` green (no vitest harness yet — keep it type-safe; manual
  smoke documented).
- **manual smoke (on the box)**: after setting `FANARTTV_API_KEY` and hitting backfill, logos
  appear; `/api/status` shows `fanart.configured=true` and a shrinking `pending` count; the
  Status page lists the ~189 unmatched titles with working Fix-match.

## Rollout notes

- **Additive + backward-compatible.** New endpoints, one new migration (V10), two small `main.rs`
  wiring additions, one new web page. No change to the match threshold or the enrichment
  algorithm — a fully-enriched library sees only the new visibility.
- **Do Part A + Part B first** (the visibility + the auto-run backstops) — they resolve the
  reported symptom. Parts C/D/E are the actionable follow-through and can land incrementally.
- Reserve migration **V10** in [[medi-migration-numbering]] when Part C lands.
