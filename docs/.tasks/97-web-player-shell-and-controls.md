# 97 — Web Player Shell & Controls

> **Status: SPEC (not started).** New web-player phase, peer to
> `82` (browse+player+admin) and `90` (format coverage). Depends on `02-api-contract.md`
> (`/api/stream`), `70-audio-quality-and-profiles.md` (the audio axis, `audio_streams`,
> `default_audio_track`), `20-phase2-hwa-transcode.md` (`command.rs`, the session layer), and
> the **seekable-VOD HLS** infrastructure now in `transcode/session.rs`
> (`build_vod_playlist` / `ensure_segment`) + `api::routes::hls_asset`.
>
> **Gap this closes.** The web player today is a **boxed** `<video>` (~1100px wide, inside the
> `App` nav shell) with only play/pause + a scrub bar + a clock. There is no fullscreen, no
> volume/mute, no audio-track selection, and no in-player caption menu. Real playback needs a
> viewport-filling player and a full control bar. Audio-track switching in particular is
> **impossible end-to-end today**: `/api/stream` and the ffmpeg command are hardcoded to the
> default/first audio track.

## Goal

1. **Full-viewport player** — `/play/:fileId` fills the whole window (no nav chrome, no
   max-width box); the only UI is the video + an auto-hiding control overlay, from which the
   user can enter **native fullscreen**.
2. **Complete control bar** — play/pause, ±10s skip, scrub (with the existing trickplay
   hover thumb), current-time/duration clock, **volume slider + mute**, **fullscreen**, and
   **audio-track** + **subtitles** menu buttons.
3. **Audio-track switching (end-to-end)** — select any of a file's `audio_streams` and have
   the server transcode that track, re-attaching playback at the same position.

## Background — what already exists (reuse, don't rebuild)

- **Layout**: `router.tsx` nests `/play/:fileId` under `App` (`NavBar` + `<main>` max-width
  padded box, ~108px top). `PlayerPage.tsx` adds a `← Back` button and a `maxWidth:1100` container.
  `VideoPlayer.tsx` renders the `<video>` at `aspectRatio:16/9`, `width:100%` inside that box.
- **Transport reducer**: `client/packages/player/src/usePlayerControls.ts` (shared with the TV
  app, imported in web via `@medi/player/usePlayerControls`) holds `isPlaying`,
  `overlayVisible` (+ auto-hide timer), `scrubTargetMs`, `positionMs`, `durationMs`. It has
  **no** volume/mute/fullscreen/track state.
- **Controls**: `client/apps/web/src/components/PlayerControls.tsx` — play/pause + clock +
  `ScrubBar`. `ScrubBar.tsx` already does the trickplay hover thumbnail.
- **Audio data**: `audio_streams` table (`V4`), `MediaFile.audio_streams`
  (`db/src/models.rs`), one row per track (`stream_index`, `codec`, `channels`, `language`,
  `title`, `immersive`, `is_default`). Client type mirrors it.
- **Stream decision**: `routes.rs::stream_decision` picks the track via
  `default_audio_track(&file.audio_streams)` (`is_default` else lowest `stream_index`).
  `StreamQuery` has **no** audio-track param. ffmpeg maps `0:a:0?` (burn-in path,
  `command.rs`) / relies on ffmpeg's default-first-audio otherwise. `TranscodeTarget` /
  `AudioTarget` carry **no** stream index.
- **Session dedup**: `session_key = (input, target, audio)` (`session.rs`). A different audio
  target therefore yields a **distinct** session (new `/api/hls/<id>/…`) — exactly what an
  audio switch needs.

## Part A — Full-viewport route

Make the player escape the `App` chrome. Recommended: a **sibling top-level route** in
`router.tsx` so the player has no `App` shell at all:

```
{ path: '/', element: <App/>, children: [ …browse/detail/settings… ] },
{ path: '/play/:fileId', element: <PlayerPage/> },   // NOT a child of App
```

(Alternative if shared providers are needed: keep it nested but have `App` render only the
`<Outlet/>` — no `NavBar`, no padding, `height:100dvh` — when `location.pathname` starts with
`/play/`. Pick one and note it.)

- `PlayerPage` root becomes `position:fixed; inset:0; width:100vw; height:100dvh; background:#000`.
- The `<video>` fills the container (`width:100%; height:100%; object-fit:contain`) — drop the
  `aspectRatio:16/9` box in `VideoPlayer.tsx` (or make it a prop so the boxed detail-page use,
  if any, is unaffected).
- **Back affordance**: a small top-left back button that shares the overlay's auto-hide
  (visible only while `overlayVisible`). `Esc` also navigates back (and exits fullscreen first
  if in fullscreen).
- Keep the existing diagnostics panel available but **collapsed/off by default** in
  full-viewport (it must not occupy the frame).

## Part B — Control bar (extend `PlayerControls.tsx`)

Reuse the `usePlayerControls` overlay/auto-hide and existing play/pause + `ScrubBar`. Add:

- **±10s skip** buttons (reuse the reducer's seek/`onSeek`).
- **Volume slider + mute toggle** — bind to `video.volume` / `video.muted`; persist the last
  volume + muted flag in `localStorage` (`try/catch`; restore on mount). **Design note:** keep
  volume/mute/fullscreen as **web-local component state in `PlayerControls`** (they are
  browser-only concepts), NOT in the shared cross-platform `usePlayerControls` reducer.
- **Fullscreen** button — `container.requestFullscreen()` / `document.exitFullscreen()`;
  reflect state from the `fullscreenchange` event; swap the icon. This is the real Fullscreen
  API, distinct from the Part A CSS full-viewport layout.
- **Audio-track menu** button (Part C) and **Subtitles menu** button (defined in `99`; leave a
  placeholder button + slot here so both specs converge on the same bar).
- Accessibility: `aria-label` on every button; keyboard already drives transport via the page.

## Part C — Audio-track switching (end-to-end)

**C1 — shared per-file endpoint (foundation, reused by `99`).**
Add `GET /api/files/:file_id` → the file's tracks, so a **deep link** to `/play/:id` (no
router state) can populate menus. Reuse `queries::get_media_file` (already returns
`audio_streams` + `subtitle_streams`). New DTO, e.g.:

```
GET /api/files/:file_id → {
  file_id,
  audio:   [{ stream_index, codec, channels, channel_layout, language, title, is_default }],
  subtitles:[{ id, stream_index?, external?, format, language, title, is_default, is_forced }]
}
```

Add the row to `02-api-contract.md`. (If `99` lands first it may define this endpoint; either
way it is defined **once** and both specs consume it.)

**C2 — select the track through the pipeline.**
- `StreamQuery` (`routes.rs`): add `audio_track: Option<i64>` (an ffprobe `stream_index`).
- `stream_decision`: when `audio_track` is set, resolve that specific `audio_streams` row
  (fall back to `default_audio_track` when absent/invalid) and use it for the `AudioTrack`
  descriptor + `audio_plan`.
- Carry the selected `stream_index` into `AudioTarget` / `TranscodeTarget` so the session key
  differs per track, and emit `-map 0:a:<n>` in `command.rs` (both the burn-in `-filter_complex`
  map and the normal path — replace the hardcoded `0:a:0` / default-first mapping). Keep the
  audio codec/downmix decision (`audio_plan`) unchanged; only the **source track** changes.
- Because `session_key` includes the audio target, a switch spawns its own transcode session
  and its own seekable VOD playlist automatically — no session-layer change needed.

**C3 — client switch UX.**
- On mount, `VideoPlayer`/`PlayerPage` fetches `GET /api/files/:id` (fixes deep-link) and
  renders an audio menu from `audio.streams` — label = `title || language || "Track N"` +
  channel layout (e.g. `English · 5.1`), mark the active one.
- On switch: capture `video.currentTime`, re-request `/api/stream?...&audio_track=<idx>`, tear
  down the current hls.js instance, attach the new decision's playlist, and **seek back to the
  captured position** (the VOD playlist makes the target immediately seekable). Show a brief
  "switching audio…" state.
- **Direct-play caveat**: a browser `<video>` can't reliably switch an embedded audio track,
  so selecting a non-default track must force the transcode path — pair `audio_track` with
  `force_transcode=1` (already supported) when the base decision was `direct`.

## Verification

- Multi-audio title: menu lists all tracks; switching changes the audio and **preserves
  position**; a distinct `/api/hls/<id>/` session appears per track.
- Fullscreen button enters/exits real fullscreen; `Esc` exits.
- Volume/mute persist across reloads (localStorage).
- Deep-link `/play/:id` (no nav state) still shows audio + subtitle menus (via `/api/files/:id`).
- Player fills the viewport; no nav bar / max-width box; Back + `Esc` return to the previous page.
- Backend: `cargo test -p medi-api -p medi-transcode` green (add a `stream_decision` test that
  `audio_track=N` selects that track and yields a distinct session key + `-map 0:a:N`).
