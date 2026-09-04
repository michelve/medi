# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

**medi** — a self-hosted media server: a Rust backend (`backend/`, a Cargo workspace producing the `medi` binary) that scans libraries, probes with ffprobe, enriches metadata, and streams via direct-play or HLS transcode; plus JS/TS clients (`client/`, a Yarn workspace) — a browser SPA and Apple TV / Android TV apps.

## Commands

### Backend (Rust)

**`cargo` is NOT on PATH on the dev box.** It lives at `%USERPROFILE%\.cargo\bin\cargo.exe`. From Git Bash, invoke it as `"$USERPROFILE/.cargo/bin/cargo.exe"`. All commands need `--manifest-path backend/Cargo.toml` (or run from `backend/`).

```bash
# build the whole workspace
"$USERPROFILE/.cargo/bin/cargo.exe" build --manifest-path backend/Cargo.toml
# test one crate (fast inner loop — e.g. the transcode decision/command builder)
"$USERPROFILE/.cargo/bin/cargo.exe" test -p medi-transcode --manifest-path backend/Cargo.toml
# a single test by name
"$USERPROFILE/.cargo/bin/cargo.exe" test -p medi-transcode --manifest-path backend/Cargo.toml sdr_transcode_has_no_filter
# whole suite
"$USERPROFILE/.cargo/bin/cargo.exe" test --manifest-path backend/Cargo.toml
```

The runnable binary is `medi` (in the `medi-api` crate); `medi-api`'s `src/lib.rs` exposes the router so integration tests drive it in-process via `tower::ServiceExt::oneshot` (no port bind).

### Client (Yarn workspaces, run from `client/`)

```bash
cd client
yarn web:dev        # Vite dev server, http://localhost:5173, proxies /api -> localhost:8096
yarn web:build      # tsc --noEmit + vite build -> dist/ (baked into the Docker web stage)
yarn web:typecheck
yarn typecheck      # every workspace
```

Point the web dev proxy at a non-local backend with `MEDI_DEV_API` (shell var or `client/apps/web/.env.local`, e.g. `MEDI_DEV_API=http://192.168.5.242:8096`).

### Docker (the primary run/dev loop — real GPU transcode)

```bash
# base compose (bind-mount media via MEDI_MEDIA / MEDI_CONFIG); binary serves at :8096
docker compose -f docker/compose.dev.yml up --build -d
# this dev box also has a git-ignored override that mounts the SMB media server:
docker compose -f docker/compose.dev.yml -f docker/compose.dev.override.yml up --build -d
```

`docker/Dockerfile.dev` is the fast debug-build inner loop (cargo cache mounts); `docker/Dockerfile` is the CI release image (`ghcr.io/michelve/medi:latest`) that Unraid runs. NVIDIA NVENC is passed through with `gpus: all` (needs the NVIDIA Container Toolkit in WSL2); Intel/AMD have no `/dev/dri` on Windows Docker, so set `MEDI_GPU_VENDOR: none` for a software (libx264) fallback.

## Architecture

### Backend crates (`backend/crates/`) — data flows one direction

`core` → `db` / `ingest` → `transcode` / `assets` / `metadata` → `api`.

- **`core`** — shared domain types (`MediaProfile`, `DvProfile`, codecs, `ClientCapabilities`, config). Defined **once** here and never duplicated per crate; every other crate reads/writes these.
- **`db`** — SQLite via r2d2 pool + rusqlite. Owns the connection PRAGMA customizer, fresh-DB `page_size` ordering, refinery migrations, and typed query/write functions. Models (`models.rs`) convert rows → `core` types (e.g. `MediaFile::profile()`).
- **`ingest`** — scans `/media`, runs `ffprobe` as a bounded-concurrency subprocess (no libav FFI), populates the catalog. Idempotent via `scan_state` (mtime + size); re-probes only changed files (or when `probed_at` is cleared).
- **`transcode`** — the direct-play-vs-transcode decision and ffmpeg command generation. Two key modules: `decision.rs` (`decide()` → `Decision::DirectPlay | Transcode`, driven by source `MediaProfile` + a `ClientProfile` + host `HwCaps`) and `command.rs` (`build_argv()` turns a `TranscodeTarget` into a jellyfin-ffmpeg argv — HW device init, filter graph, encoder, fMP4/CMAF HLS muxer). `session.rs` manages live HLS sessions. This crate is **pure and heavily unit-tested** — the decision table and every command variant have tests; prefer extending them over manual ffmpeg checks.
- **`assets`** — off-peak background worker: hover-preview clips, trickplay sprites, and per-chapter poster frames, written under `/config` only. Yields the GPU to live transcode sessions (GPU-idle guard); resumes via `scan_state.*_done_at` markers.
- **`metadata`** — pluggable `MetadataProvider` trait (`tmdb` default, `omdb`), plus fanart.tv artwork; fills `overview` / artwork / cast.
- **`api`** — the axum HTTP + HLS server. `medi` binary is a thin wrapper: load config → open DB → build `AppState` → serve `routes::router`. Endpoint contract lives in `docs/.tasks/02-api-contract.md`.

### The playback decision (the crux)

A `/api/stream` request resolves to **direct-play** (client fetches `/api/direct`, maybe a remux) or **HLS transcode** (`/api/hls/...`). The decision keys off what the *client and its display* can natively present (`ClientProfile` — Apple TV, Shield, Android TV, or `web`), not just the source. Notably `web` (browser) is conservative: H.264 8-bit / AAC only and **cannot remux**, so a container/codec/HDR/10-bit mismatch is promoted to a real server transcode instead of an unplayable direct stream. HLS output is always fMP4/CMAF with a server-synthesized VOD playlist (`build_vod_playlist`) so the whole timeline is seekable before any byte is transcoded; segments are produced on demand.

### Migrations

refinery SQL under `backend/migrations/`, embedded at compile time by `db`. Add `V<N>__<name>.sql` (sequential, double underscore, currently through V15). **Files are pure DDL/DML — never set PRAGMAs** (the `db` pool module owns file-level tuning + ordering). Clearing `scan_state.probed_at` is the standard way to force a one-time library-wide re-probe. See `backend/migrations/README.md`.

### Client (`client/`)

- `apps/web` — Vite + React DOM SPA, served by the `medi` binary at `/` in prod. Player uses hls.js; subtitles are **client-rendered** (libass-wasm for ASS, libbitsub for PGS/VobSub) so styled/image subs cost zero transcode sessions.
- `apps/tv` — Apple TV / Android TV app (React Native).
- `packages/api-client` — the **single source of truth** for the API contract (types + client), aliased straight to source in `vite.config.ts` (no build step). Shared player logic (`packages/player`) and theme (`packages/ui`) are consumed the same way.

## Conventions

- **Design record.** `docs/.tasks/NN-*.md` are numbered task specs — the authoritative design history. Bug-fix and feature work references them (e.g. task 100, 101); a code doc-comment citing `docs/.tasks/NN` points at the rationale. When changing behavior, check and update the relevant task spec.
- **Commit straight to `main`** (no feature branches / PRs — user preference). Releases are annotated tags `vX.Y.Z` + a `gh release create`.
- Types that cross crate boundaries go in `core`, never re-declared downstream.
