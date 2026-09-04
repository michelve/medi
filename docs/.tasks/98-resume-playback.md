# 98 — Resume Playback & Continue Watching

> **Status: BUILT (session 2026-09-04).** Parts A–D shipped. Backend `cargo test -p medi-db
> -p medi-api` green (new `upsert_progress` finished-threshold + `list_continue_watching`
> ordering/exclusion/cascade db tests, and a `catalog.rs` test over all three routes incl. the
> `POST` sendBeacon path); web + api-client typecheck and web `vite build` green (hls.js still
> code-split). Single-user, no auth (LAN appliance) — progress is global, keyed by
> `media_file_id`, not per-user.
>
> **Chosen resume UX:** auto-seek after `loadedmetadata` (seeded via a new `VideoPlayer`
> `initialResumeMs` prop that reuses the audio-switch resume-seek path) **plus** a small
> non-blocking `ResumeChip` ("Resuming from mm:ss / Start over") that auto-dismisses after ~8s;
> ignoring it just keeps the resumed playback. **Web only** — the TV app's demo "Continue
> Watching" label was left as-is (the optional TV swap is deferred).
>
> **Gap this closed.** There was **no playback-progress persistence anywhere**: no table, no
> route, nothing. The player started at 0 every time; closing the tab lost your place. The TV
> app's "Continue Watching" row is a **hardcoded label** over a slice of `/api/library`, not
> real resume data. This task makes the web player remember where you left off and surfaces a
> real Continue-Watching row on the web landing page.

## Goal

1. Persist playback position as the user watches (throttled), and on pause / tab-hide / unmount.
2. On opening a title, **resume** from the saved position (with a Start-over option).
3. A real **Continue Watching** row on the landing page, driven by actual progress.

## Background — what exists

- **Nothing to reuse for persistence.** Confirmed: no `progress` / `playback_position` /
  `watched` table or column across `V1`–`V10`; no progress route in `routes.rs`.
- **In-memory position only**: `usePlayerControls.ts` holds `positionMs` / `durationMs`, fed by
  the `<video>` `timeupdate` event (`PlayerPage.tsx` wires `loadedmetadata`/`timeupdate`).
  Never persisted or sent to the server. These are the exact values to hook into.
- **TV "Continue Watching"** (`client/apps/tv/.../HomeScreen.tsx`) is a demo label, not data —
  replace its source (or leave TV as-is and add the row on **web** `LibraryPage` first).

## Part A — DB (migration V11)

`V11__playback_progress.sql` — additive DDL, refinery-idempotent, following the `V4`/`V5`
header conventions (no PRAGMAs). Reserve **V11** ([[medi-migration-numbering]] — V1–V10 used).

```sql
CREATE TABLE playback_progress (
    media_file_id INTEGER PRIMARY KEY REFERENCES media_files(id) ON DELETE CASCADE,
    position_ms   INTEGER NOT NULL,
    duration_ms   INTEGER NOT NULL,      -- snapshot at write time (for the % calc)
    updated_at    INTEGER NOT NULL,      -- unix seconds (match existing epoch columns)
    finished      INTEGER NOT NULL DEFAULT 0  -- set past ~95%; drops it from Continue Watching
);
CREATE INDEX idx_playback_progress_updated ON playback_progress(updated_at DESC);
```

Single-user, so `media_file_id` as PK (one progress row per file). `ON DELETE CASCADE` so a
removed file's progress goes with it (matches `audio_streams`/`subtitle_streams`).

## Part B — DB layer

- `writes::upsert_progress(conn, media_file_id, position_ms, duration_ms)` — upsert (PK
  conflict → overwrite `position_ms`/`duration_ms`/`updated_at`); set `finished=1` when
  `position_ms >= 0.95*duration_ms`, else `0`.
- `queries::get_progress(conn, media_file_id) -> Option<Progress>`.
- `queries::list_continue_watching(conn, limit) -> Vec<ContinueItem>` — rows where
  `finished=0` AND `position_ms` is meaningfully into the film (e.g. `> 30_000` ms and
  `< 0.95*duration_ms`), ordered by `updated_at DESC`. Join to the owning movie/episode so the
  row can render a poster + title (reuse the library projection shape where practical).

## Part C — API (`02-api-contract.md` rows)

| Method | Path | Body / Query | Returns |
|---|---|---|---|
| `GET`  | `/api/progress/:file_id` | — | `{ position_ms, duration_ms, updated_at, finished }` or `204`/empty when none |
| `PUT`  | `/api/progress/:file_id` | `{ position_ms, duration_ms }` | `204` |
| `GET`  | `/api/continue-watching` | `?limit=` | `[{ file_id, title, kind, poster…, position_ms, duration_ms }]` |

Not ETag-cached (progress is live). `PUT` is the throttled write target.

## Part D — Client

**Persist (in `PlayerPage`, reusing the reducer's `positionMs`/`durationMs`):**
- Throttled `PUT /api/progress/:id` every ~10–15 s of playback (a `useRef` timer, not on every
  `timeupdate`).
- **Flush** on `pause`, on `visibilitychange` (tab hidden), and on unmount — use
  `navigator.sendBeacon` (or a keepalive fetch) for the unload/hide flush so it isn't dropped.
- Don't write while scrubbing/seeking is mid-flight (write the committed position).

**Resume (on player mount):**
- `GET /api/progress/:id`; if a resumable position exists (not finished, > ~30 s), either
  auto-seek after `loadedmetadata` **or** show a "Resume from mm:ss / Start over" chip
  (recommended: a small non-blocking chip that auto-dismisses; auto-resume if dismissed).
  Spec the chosen behavior explicitly in the implementation.
- Seek by setting `video.currentTime` after metadata; for an HLS/VOD stream the synthesized
  playlist makes the target immediately seekable.

**Continue Watching row (`LibraryPage`):**
- Fetch `/api/continue-watching` and render a real row at the top (reuse the existing
  poster-row/`PosterCard` components). Each card links to `/play/:file_id` (which then resumes).
- Optional: replace the TV app's demo label source with the same endpoint.

## Verification

- Play a title ~5 min in, close the tab; reopen `/play/:id` → resumes at ~5 min (chip or
  auto-seek per the chosen UX).
- `PUT` fires on a throttle during playback and once more on pause/hide/unmount (network tab).
- Continue-Watching row lists in-progress titles newest-first; a title watched past ~95% is
  marked `finished` and drops off the row.
- Backend: `cargo test -p medi-db -p medi-api` green — add db tests for `upsert_progress`
  (upsert + finished threshold) and `list_continue_watching` (ordering + exclusions), and an
  api test for the three routes.
