# 10 — Phase 1: Foundation & Data Layer

> Maps to README §Development Roadmap → Phase 1. Depends on `00-architecture.md`,
> `01-db-schema.md`, `02-api-contract.md`.

## Purpose

Stand up the Rust backend skeleton, the NVMe-optimized SQLite database, and the
asynchronous ingestion worker that scans `/media`, runs `ffprobe`, and populates the
catalog — including exact Dolby Vision profile and HDR transfer characteristics.

## Requirements

- `cargo` workspace under `backend/` with crates `core`, `db`, `ingest`, `api` (see `00`).
- SQLite configured per `01-db-schema.md` (64 KB pages, WAL, mmap, cache).
- `ffprobe` runs asynchronously via subprocess, never blocking the axum runtime.
- Ingestion is idempotent using `scan_state` (mtime + size); re-scans skip unchanged files.
- Catalog served through the moka LRU cache with the endpoints in `02-api-contract.md`.

## Packages / crates

`axum`, `tokio` (full), `tower`, `tower-http`, `rusqlite` (bundled), `r2d2`, `r2d2_sqlite`,
`refinery`, `serde`, `serde_json`, `tracing`, `tracing-subscriber`, `moka`, `notify`
(filesystem change watching), `anyhow`/`thiserror`, `figment`/`config` (env-driven config).

## File structure (where to save)

```
backend/
├── Cargo.toml                 # [workspace] members = ["crates/*"]
├── migrations/V1__init.sql    # from 01-db-schema.md
└── crates/
    ├── core/src/lib.rs        # DvProfile, HdrType, MediaProfile, AppConfig, error types
    ├── db/src/                # pool.rs (r2d2 + PRAGMA customizer), models.rs, queries.rs, migrate.rs
    ├── ingest/src/            # scanner.rs (walk /media, diff scan_state), ffprobe.rs (subprocess+parse), worker.rs
    └── api/src/               # main.rs (bootstrap), routes/*.rs, cache.rs, dto.rs
```

## Sub-tasks

1. **Workspace + config**: create the cargo workspace; `AppConfig` reads `MEDIA_DIR`
   (`/media`), `CONFIG_DIR` (`/config`), `BIND_ADDR`, off-peak window, pool size from env.
2. **DB bring-up** (`db`): implement pool with PRAGMA customizer + fresh-DB `page_size`
   ordering; run refinery migrations at boot; expose typed query fns.
3. **Filesystem scanner** (`ingest/scanner.rs`): recursively walk `/media`, classify
   movie vs series/episode by path/naming, diff against `scan_state`, enqueue new/changed files.
4. **ffprobe worker** (`ingest/ffprobe.rs`): spawn `ffprobe -v quiet -print_format json
   -show_format -show_streams -show_frames -read_intervals '%+#1' <file>` via `tokio::process`;
   parse video codec, profile, bit depth, `color_transfer`, `color_space`, and the Dolby
   Vision side-data (`dv_profile`, `dv_bl_signal_compatibility_id`). Set `hw_decode_unsupported`
   for H.264 High 10 (see `20`). Write rows via `db`, update `scan_state.probed_at`.
   > The same `-show_streams` output also carries every **audio** stream. `70-audio-quality-and-profiles.md`
   > widens this parser (no new invocation) to persist all audio tracks into the `audio_streams`
   > table; this task remains video-only.
5. **Watch mode** (`notify`): watch `/media` for changes to trigger incremental re-scans.
6. **API skeleton** (`api`): implement `/api/health`, `/api/library`, `/api/movies/:id`,
   `/api/series/:id` from `02-api-contract.md`, backed by `db` + moka; run all DB calls under
   `spawn_blocking`. Invalidate cache on ingest write.
7. **Observability**: `tracing` structured logs; log each probe with detected profile.

## Dolby Vision extraction detail

`ffprobe` exposes DV as stream side_data (`DOVI configuration record`). Map:
- `dv_profile` 5 → proprietary IPTPQc2, no fallback → always transcode for SDR.
- `dv_profile` 7 → BL(HDR10)+EL, common in 4K Blu-ray rips.
- `dv_profile` 8 → `dv_bl_compatible_id` 1 = HDR10 fallback, 4 = SDR fallback.
Store all three fields; `transcode` (Phase 2) reads them.

## Scaling notes

- Bound the ffprobe worker's concurrency (e.g. a `tokio::sync::Semaphore`) so a first-run
  scan of 10,000 files doesn't spawn thousands of processes.
- Keep the write path single-threaded (WAL = one writer); readers stay concurrent.

## Verification

- Point `MEDIA_DIR` at a fixtures folder with one file per case (H.264 SDR, HEVC HDR10,
  DV P5, DV P8.1, AV1, H.264 High-10). Boot → `scan_state` + `media_files` populated with
  correct `dv_profile`/`hdr_type`/`hw_decode_unsupported`.
- `curl /api/library` returns the ingested catalog; `curl /api/movies/:id` returns file metadata.
- Restart → no re-probe of unchanged files (idempotent).
