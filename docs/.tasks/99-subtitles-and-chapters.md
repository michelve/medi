# 99 — Subtitles (in-player) & Chapters

> **Status: IN PROGRESS.** Revised for production readiness after studying jellyfin-web:
> subtitles render **client-side** (libass-wasm for ASS/SSA, libbitsub for PGS/VobSub) with
> server **burn-in as a fallback** — this costs zero transcode sessions, switches instantly,
> and renders at native resolution (burn-in doesn't scale — every styled/image sub would eat a
> GPU session). Plain SRT/VTT stays native `<track>`. All four enhancements are in scope
> (persist+auto-select, appearance, chapter nav+keyboard, sync/offset).
>
> **Shipped:** chapters end-to-end (ffprobe→V12→`/api/files/:id`, scrub ticks + hover names +
> prev/next + PageUp/PageDown); caption menu (Off/text/ASS/image, Forced/Default/SDH badges)
> with programmatic `<track>` selection; persist + cross-episode auto-select; raw subtitle +
> font endpoints (`/api/subtitles/:id/:index/raw`, `/api/files/:id/fonts[/:name]`); libass-wasm
> ASS rendering; subtitle sync (`g`/`h` + indicator) for both render paths.
> **Remaining:** libbitsub image rendering + a working burn-in fallback re-request (Phase 5);
> subtitle appearance panel (deferred). See the tracking checklist in the plan.
>
> Original spec below (kept for reference; A1 deep-link loading was already done in `97`).
>
> New web-player phase. Depends on
> `90-format-coverage-and-subtitles.md` (the subtitle subsystem: `subtitle_streams` V5,
> `/api/subtitles/...`, image-sub burn-in), `97-web-player-shell-and-controls.md` (the control
> bar + `GET /api/files/:id`), and `20`/ingest for the ffprobe pass.
>
> **Gap this closes.** Subtitle *infrastructure* exists (discovery, DB, WebVTT serving,
> `<track>` rendering) but the web player can't use it well: subtitles are passed only via
> router `location.state`, so a **deep link to `/play/:id` has no captions**, there is **no
> in-player caption menu** (native controls are off, so the browser's own menu is hidden), and
> **image subtitles** (PGS/VobSub) are never offered. Separately, **real chapter markers do
> not exist at all** (no ffprobe `-show_chapters`, no table, no UI).
>
> Per the product decision, subtitle **discovery stays same-folder only** (already implemented
> in `scanner.rs::discover_sidecars`) — this task does **not** add subfolder scanning.

## Goal

1. Subtitles work on a **deep link**, with an in-player caption **menu** (off / per-track /
   forced), and **image subtitles** selectable via the existing burn-in path.
2. **Real chapter markers** — probe embedded chapters, store them, and show chapter ticks +
   titles on the scrub bar.

## Background — what exists

- **Subtitles**: `scanner.rs::discover_sidecars` finds same-folder sidecars
  (`srt/ass/ssa/vtt/sub`, stem-prefix match, `.<lang>`/`.forced` tokens). `subtitle_streams`
  (V5) stores embedded (`stream_index`) XOR external (`external_path`) tracks with
  `format` (`text`|`image`), `language`, `is_default`, `is_forced`. `/api/subtitles/:file_id/:index`
  serves text tracks as WebVTT (embedded/external converted + cached; `.vtt` served directly; image
  → `415`). `stream_decision` already burns in an image sub when `sub=<idx>&sub_burn=1`
  (`image_subtitle_relative_index`). The web player renders `<track>` children
  (`VideoPlayer.tsx`) from `PlayerPage`'s `textTracks`, but that list comes from
  `location.state` only.
- **Chapters**: **nothing** — grep for `chapter` across the repo returns zero. The single
  ffprobe pass (`ingest/ffprobe.rs`, `-show_format -show_streams`) does not request chapters.
- **Per-file endpoint**: `97` adds `GET /api/files/:id` (audio + subtitle tracks). This spec
  extends it with chapters. If `99` lands before `97`, define `GET /api/files/:id` here.

## Part A — Subtitles in the player

**A1 — deep-link loading.** Source the player's subtitle list from `GET /api/files/:id`
(shared with `97`) instead of `location.state`, so `/play/:id` deep links get captions. Keep
`api.subtitleUrl(fileId, index)` for the `<track src>`.

**A2 — caption menu (in `PlayerControls`).** A subtitles button opens a menu: **Off**, each
**text** track (label = `title || language`, mark forced/default), and each **image** track
(A3). Selecting a text track toggles that `<track>`'s `mode` to `showing` and the rest to
`disabled` (the native menu is unavailable because native `controls` are off). Persist the
last choice per session (optional).

**A3 — image subtitles (PGS/VobSub).** Offer image tracks in the same menu. Selecting one
re-requests `/api/stream?...&sub=<stream_index>&sub_burn=1` (burn-in already wired in
`stream_decision`), tears down + re-attaches hls.js, and re-seeks to the current position
(same switch mechanics as audio-track switching in `97`). Selecting **Off** or a text track
re-requests without burn-in. Note: burn-in forces a transcode session distinct from the plain
one (different `TranscodeTarget.subtitle_burn_in` → different `session_key`).

## Part B — Chapters (greenfield)

**B1 — ingest.** Add `-show_chapters` to the existing single ffprobe invocation
(`ingest/ffprobe.rs`, alongside `-show_format -show_streams` — one pass, no extra process).
Parse `chapters[]`: `start_time` / `end_time` (seconds, float) → ms, `tags.title`.

**B2 — DB (migration V12).** `V12__chapters.sql` — additive, refinery-idempotent, `V4`/`V5`
header conventions. Reserve **V12** ([[medi-migration-numbering]]).

```sql
CREATE TABLE chapters (
    id            INTEGER PRIMARY KEY,
    media_file_id INTEGER NOT NULL REFERENCES media_files(id) ON DELETE CASCADE,
    ordinal       INTEGER NOT NULL,     -- 0-based chapter order
    start_ms      INTEGER NOT NULL,
    end_ms        INTEGER,              -- nullable; next chapter's start bounds it otherwise
    title         TEXT,                 -- ffprobe tags.title, may be NULL
    UNIQUE(media_file_id, ordinal)
);
CREATE INDEX idx_chapters_file ON chapters(media_file_id);
```

Repopulated on re-probe like `audio_streams`/`subtitle_streams` (existing rows simply have no
`chapters` children until re-probed via the `scan_state` path).

**B3 — DB layer.** `writes::replace_chapters(conn, media_file_id, &[ChapterWrite])`
(delete-then-insert, keep ordinal — mirror `replace_credits`), `queries::chapters_for(conn,
media_file_id) -> Vec<Chapter>`. Wire `replace_chapters` into the ingest write path where
audio/subtitle streams are written.

**B4 — API.** Include `chapters: [{ ordinal, start_ms, end_ms?, title? }]` in
`GET /api/files/:id` (preferred — one round trip for the player), OR a dedicated
`GET /api/chapters/:file_id`. Add the row to `02-api-contract.md`.

**B5 — client (`ScrubBar` + controls).**
- Render **chapter ticks** on the scrub bar at each `start_ms` (small marks over the track).
- On hover near a tick, show the chapter **title** in the tooltip **alongside** the existing
  trickplay thumbnail (reuse the current hover math in `ScrubBar.tsx`).
- Optional: a chapter list in a menu, and a "next chapter" skip that seeks to the next
  `start_ms`.

## Reuse notes

- Subtitle serving + burn-in decision are **done** (`90`) — this task only wires the player UI
  and deep-link loading; do not re-implement WebVTT conversion or the burn-in branch.
- The audio/sub/burn-in **switch mechanics** (capture position → re-request `/api/stream` →
  re-attach hls.js → re-seek) are the same as `97` Part C — share one helper.
- Trickplay hover (`ScrubBar`) is done; chapter ticks/titles layer onto the same component.

## Verification

- Deep-link `/play/:id` for a title with a same-folder `.srt` → captions load and the menu
  lists the track; toggling Off/On works.
- A PGS/VobSub title → selecting the image track burns it in (new transcode session) and
  playback resumes at position; switching back removes it.
- A title with embedded chapters → ticks appear on the scrub bar and titles show on hover;
  next-chapter skip (if built) jumps correctly.
- Backend: `cargo test -p medi-ingest -p medi-db -p medi-api` green — add an ffprobe parse
  test for `-show_chapters` (a fixture with chapters), a `replace_chapters`/`chapters_for`
  db test, and an api test that `GET /api/files/:id` includes chapters.
