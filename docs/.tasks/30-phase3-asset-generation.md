# 30 — Phase 3: Background Asset Generation

> Maps to README §Development Roadmap → Phase 3. Depends on Phase 1 (`10`) and Phase 2
> (`20`, reuses the ffmpeg HWA command builder). Lives in `backend/crates/assets`.

## Purpose

Pre-generate lightweight preview assets so the client's Netflix-style hover-to-play and
timeline scrubbing load instantly, without live-transcoding the massive 4K source. A
dedicated background worker runs during off-peak hours and writes assets under `/config`.

## Requirements

- Extract 10–15 second silent preview clips, HW-downscaled to **720p H.264**, audio stripped.
- Generate **trickplay sprites** — Roku-style **BIF** files or tiled JPG matrices — for scrubbing.
- Write assets to `/config` (never `/media`): `/config/previews/<file_id>.mp4`,
  `/config/trickplay/<file_id>.{bif,jpg}`; record rows in `preview_clips` / `trickplay_assets`.
- Run only inside a **configurable off-peak window** and **yield the GPU to live transcodes**
  (GPU-idle guard) so preview generation never competes with user-requested streaming.
- Idempotent: skip files already done (via `scan_state.preview_done_at` / `trickplay_done_at`).

## Packages / crates

- `tokio` (`process`, timers), reuse `transcode::command`/`caps` for HW-accelerated downscale
- `serde`, `tracing`
- `jellyfin-ffmpeg` binary (Docker runtime); trickplay BIF packing done in-process or via ffmpeg tiling

## File structure

```
crates/assets/src/
├── scheduler.rs   # off-peak window check + GPU-idle guard + throttle/semaphore
├── preview.rs     # extract 10-15s clip → 720p h264, strip audio → /config/previews
├── trickplay.rs   # sample frames at interval → BIF or tiled-JPG → /config/trickplay
├── worker.rs      # main loop: pick next un-done file, generate, record, update scan_state
└── lib.rs
```

## Off-peak scheduling & GPU guard (implement in `scheduler.rs`)

- `AppConfig` exposes `ASSET_WINDOW` (e.g. `"02:00-06:00"`) and `ASSET_MAX_CONCURRENCY`.
- Before starting each asset job: (a) check current time is in the window; (b) check the
  live transcode session count from Phase 2 is **zero** (or below a threshold) — if not,
  sleep and re-check. This is the GPU-idle guard.
- Use a `Semaphore` to bound concurrent ffmpeg jobs; low priority relative to live streams.

## Preview clip command sketch (`preview.rs`)

```
ffmpeg -ss <mid-point> -t 15 -hwaccel <vendor> -i INPUT \
  -an -vf "scale_<vendor>=w=-2:h=720" -c:v h264_<vendor> -profile:v high \
  -movflags +faststart /config/previews/<file_id>.mp4
```
- `-an` strips audio (silent UI preview). Reuse the vendor selection from `transcode::caps`.
- For DV/HDR sources, tone-map to SDR (reuse Phase 2 filter chain) so previews look correct.

## Trickplay sprites (`trickplay.rs`)

- Sample one frame every N seconds (`interval_ms`, e.g. 10000), scale small (e.g. 320px wide).
- Pack into either:
  - **BIF** (Roku Base Index Frames): header + index + concatenated JPEGs; or
  - **tiled JPG**: a grid (`cols`×`rows`) of thumbnails + metadata (`tile_w/h`, grid dims).
- Record kind, path, interval, grid dims in `trickplay_assets`.

## Sub-tasks

1. `scheduler.rs`: window parsing, GPU-idle guard querying live-session count, semaphore.
2. `preview.rs`: mid-point 15s extraction, HW downscale to 720p, audio strip; write + DB row.
3. `trickplay.rs`: interval frame sampling; BIF packer (choose one format as default — BIF)
   with tiled-JPG as an option; write + DB row.
4. `worker.rs`: main loop selecting the next file lacking assets (join `media_files` ↔
   `scan_state`), generate both asset types, stamp `scan_state.*_done_at`.
5. Serve assets via `/api/preview/:file_id` and `/api/trickplay/:file_id` (Phase 1 `api`).

## Scaling notes

- Off-peak + GPU-idle guard keeps the primary GPU free for live streaming at peak times.
- First-run backfill of 10,000 files is large — process oldest/newest-added first, persist
  progress in `scan_state` so restarts resume rather than restart.
- Assets are small and cheap to serve; they can be `ServeDir`'d and cached at the edge (client).

## Verification

- With `ASSET_WINDOW` set to "now", the worker generates `/config/previews/<id>.mp4`
  (720p, no audio: `ffprobe` shows 0 audio streams, height 720) and a trickplay file.
- `/api/preview/:file_id` and `/api/trickplay/:file_id` serve the generated assets.
- Start a live transcode → the worker pauses (GPU-idle guard) until the session ends.
- Restart mid-backfill → resumes from `scan_state`, no duplicate regeneration.
