# 82 — Web UI Client: In-Browser Playback, Library Management & Metadata Match

> Depends on `80-web-ui-client.md` (scaffold + serving) and `81-web-ui-browse.md` (grid,
> detail pages, and the stubbed Play buttons). Consumes `GET /api/stream`, `/api/direct`,
> `/api/hls`, `/api/trickplay`, and the libraries + metadata-match endpoints from
> `02-api-contract.md` / `60-metadata-and-libraries.md`. **Gap this closes:** after `81`
> the web UI can browse but not *do* anything — this task adds the three interactive flows
> that make it a real server UI: **play in the browser**, **manage libraries**, and **fix
> metadata matches**.

## Purpose

Complete the web UI's interactive surface:

1. **In-browser playback** — honor the server's stream decision: **direct-play** via a plain
   `<video>` + HTTP Range, or **HLS** via `hls.js` (with native `<video>` HLS fallback on
   Safari).
2. **Library management** — a Plex-style admin panel over the `/api/libraries` CRUD + scan
   endpoints.
3. **Metadata match** — a per-title "fix match" flow over the refresh/matches/match endpoints.

## Requirements

- Playback path is chosen **by the server**, never guessed: call `client.stream(fileId)` and
  branch on `decision.mode` (`"direct"` | `"hls"`), using `client.directUrl` / the returned
  HLS `url`. Do not hard-code container assumptions client-side.
- `hls.js` is a dependency of **`apps/web` only** — never added to the backend or shared
  packages.
- Reuse `@medi/player` `usePlayerControls.ts` (pure reducer: play/pause, seek, overlay
  auto-hide) and `trickplay.ts` (tile math). Feed the reducer **DOM** keyboard/pointer
  events instead of the RN remote events.
- Admin writes surface `ApiError`: a `409` from a scan (`ApiError.isBusy`) shows
  "scan already in progress"; a `404` shows a not-found state; other errors show the
  server's `error.message`.

## Packages / crates

- **Touched (all under `client/apps/web`):** `src/pages/*`, `src/components/*`. New dep:
  `hls.js` (web app only).

## File structure (where to save)

```
client/apps/web/src/
  pages/
    PlayerPage.tsx           # "/play/:fileId" (or a modal over detail)
    LibrariesPage.tsx        # "/settings/libraries"
  components/
    VideoPlayer.tsx          # <video> + hls.js attach/detach; direct vs hls
    PlayerControls.tsx       # DOM overlay driven by usePlayerControls.ts
    ScrubBar.tsx             # seek bar + optional trickplay thumbnail
    LibraryEditor.tsx        # create/rename/add-remove folders/delete/scan
    MatchDialog.tsx          # refresh + candidate list + pin match
```

## Sub-tasks

1. **`VideoPlayer` (decision-driven)** — on mount call `client.stream(fileId, hints)`.
   - `mode === 'direct'` → `<video src={client.directUrl(fileId)}>` (browser handles Range
     seeking).
   - `mode === 'hls'` → if `video.canPlayType('application/vnd.apple.mpegurl')` (Safari) set
     `video.src = decision.url`; else create an `Hls` instance, `loadSource(decision.url)`,
     `attachMedia(video)`, and **destroy it on unmount**. Surface fatal `hls.js` errors as a
     retry state.
2. **Transport controls** — wire `usePlayerControls.ts` from `@medi/player`; map DOM events
   to its events: Space/click → play-pause, ←/→ → seek (`SEEK_STEP_MS`), pointer-move →
   show overlay (auto-hides after `HIDE_AFTER_MS`). Render current time / duration.
3. **Trickplay scrub (nice-to-have)** — fetch `GET /api/trickplay/:fileId/meta`; on hover
   over `ScrubBar`, use `trickplay.ts` `tileForPosition` + `client.trickplayUrl(fileId,'jpg')`
   to show the sprite cell. If meta is `404` (BIF-only or none), fall back to a plain bar —
   no error surfaced.
4. **Library management** (`LibrariesPage` + `LibraryEditor`) — list with
   `client.libraries()`. Create via `client.createLibrary({ name, kind, folders })`;
   rename / add / remove folders via `client.patchLibrary(id, { name?, add_folders?,
   remove_folders? })`; delete via `client.deleteLibrary(id)`; trigger a rescan via
   `client.scanLibrary(id)`. Show `isBusy` (409) as "scan in progress"; refresh the list
   after each write.
5. **Metadata match** (`MatchDialog`, opened from a movie detail) — `client.refreshMovie(id)`
   to force re-enrich; `client.movieMatches(id, query?)` to list candidates (title, year,
   `score`); `client.matchMovie(id, providerId)` to pin a chosen candidate. Reflect the
   resulting `metadata_state` and re-fetch the movie so the new poster/overview show.

## Verification

> Note: no Rust/Node toolchain on this dev machine — build in Docker / on the host.

- **HLS playback** — a title the server decides to transcode plays in **Chrome/Firefox**
  (hls.js path) and in **Safari** (native HLS); seeking works.
- **Direct playback** — a browser-friendly file plays via `<video>` with working Range
  seeks (scrub back and forth).
- **Controls** — Space toggles play/pause, arrows seek, the overlay auto-hides and
  reappears on pointer move.
- **Libraries** — creating a library then `scanLibrary` repopulates the grid (`81`) with its
  titles; a second concurrent scan shows the busy state, not a crash.
- **Match** — opening the match dialog on a mismatched movie, picking a candidate, and
  refreshing updates the poster/overview on the detail page.

## Cross-references (edits required in lockstep)

- `81-web-ui-browse.md` — completes the **Play** buttons stubbed on movie/series detail
  (they now navigate to `PlayerPage` / open the player).
- `60-metadata-and-libraries.md` — this is the **web** counterpart to its deferred RN
  Settings→Libraries + match screens (sub-task 12); both consume the same endpoints.
