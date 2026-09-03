# 50 — Phase 5: Playback & Packaging

> Maps to README §Development Roadmap → Phase 5. Depends on Phase 4 (`40`) for the client
> and Phases 1–3 for the backend. Covers the video player, the Docker image, and the Unraid
> template.

## Purpose

Integrate full-length playback with custom overlay controls that don't fight the TV focus
engine, then package the backend as a multi-stage Debian Docker image with GPU driver
injection, and publish an Unraid Community Applications XML template for one-click install.

## Part A — Video playback & overlay (client, `client/packages/player`)

### Requirements
- Use **react-native-video** (AVPlayer on Apple TV, ExoPlayer on Android TV).
- Custom overlay controls (Play/Pause/Seek/Audio-track) must **not** use standard spatial
  navigation — on Android TV, absolutely-positioned controls over the video view break D-pad
  routing and cause render lag (README §Video Playback and Overlay Integration).
- Instead, globally intercept raw remote events with **`useTVEventHandler`** (from
  react-native-tvos): `eventType: 'select' | 'playPause' | 'left' | 'right' | 'up' | 'down'`
  → programmatically toggle overlay + drive player state.
- Apple TV may optionally fall back to native AVPlayer controls for a familiar scrubbing UX.
- Player consumes `/api/stream/:file_id` → direct-play URL or HLS playlist (Phases 1–2).
- Timeline scrubbing uses the trickplay assets from `/api/trickplay/:file_id` (Phase 3).

### Sub-tasks
1. `player/VideoScreen`: mount react-native-video with the URL from `/api/stream`.
2. Overlay controller driven entirely by `useTVEventHandler` (no Touchable/Pressable focus).
3. Scrub bar renders trickplay thumbnails while seeking.
4. Handle HLS vs direct-play uniformly; surface transcode `409` (session cap) gracefully.

## Part B — Docker image (`docker/`)

### Requirements (README §Containerization Strategy)
- **Multi-stage** Dockerfile, **Debian** base (Bookworm/Bullseye — better proprietary GPU
  driver + FFmpeg compatibility than Alpine).
- **Build stage**: compile the Rust backend release binary (`cargo build --release`).
- **Runtime stage**: minimal Debian; inject dependencies:
  - `jellyfin-ffmpeg` binaries
  - `intel-media-va-driver-non-free` + `intel-compute-runtime` (Intel QSV / OpenCL)
  - `mesa-va-drivers` (AMD VA-API)
  - NVIDIA support is injected by the **host** NVIDIA container runtime (not baked in).
- `entrypoint.sh`: apply migrations/PRAGMAs (via the binary), then launch the server.
- Expose the API port; declare a `HEALTHCHECK` hitting `/api/health`.
- Volumes: `/config` (RW appdata), `/media` (RO array) — per `00-architecture.md`.

> **Release vs dev image.** This **release** image (`docker/Dockerfile`, `cargo build
> --release`, built in CI) is the only artifact the Unraid template publishes/pins. A separate
> **dev** image (`docker/Dockerfile.dev` — same runtime, but a *debug* backend build with
> cargo cache mounts) drives the Windows fast-iteration loop and is never published — see
> `92-windows-dev-and-native-gpu.md`.

### Dockerfile skeleton
```dockerfile
# ---- build ----
FROM rust:1-bookworm AS build
WORKDIR /src
COPY backend/ ./backend/
RUN cd backend && cargo build --release

# ---- runtime ----
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates \
      intel-media-va-driver-non-free intel-opencl-icd \
      mesa-va-drivers \
    && rm -rf /var/lib/apt/lists/*
# install jellyfin-ffmpeg (from its apt repo or release deb)
COPY --from=build /src/backend/target/release/medi /usr/local/bin/medi
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
ENV MEDIA_DIR=/media CONFIG_DIR=/config BIND_ADDR=0.0.0.0:8096
VOLUME ["/config", "/media"]
EXPOSE 8096
HEALTHCHECK CMD curl -f http://localhost:8096/api/health || exit 1
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
```

## Part C — Unraid CA template (`unraid-templates/`)

### Requirements (README §Unraid XML Template Schema)
- A dedicated GitHub repo (e.g. `username/unraid-templates`) hosting `medi.xml` and a
  root `ca_profile.xml` (maintainer profile + GitHub issues / Discord support links).
- The Community Applications plugin scrapes the repo to list the app in its store.
- Pre-configure hardware passthrough so users never touch the terminal.

### Required XML tags / config
| Tag | Value |
|---|---|
| `<Name>` | `medi` |
| `<Repository>` | `ghcr.io/username/medi:latest` |
| `<Network>` | `bridge` (or `host`) |
| `<Overview>` | concise capability summary (replaces deprecated `<Description>`) |
| `<Category>` | `MediaApp:Video` |
| `<Config>` Port | container `8096` → host port |
| `<Config>` Volume | `/config` → `/mnt/user/appdata/medi` (RW, SQLite + assets) |
| `<Config>` Volume | `/media` → `/mnt/user/movies` **Read-Only** |
| `<Config>` Device | `/dev/dri` → `/dev/dri` (Intel/AMD QSV/VA-API) |
| `<ExtraParams>` | `--runtime=nvidia` (NVIDIA hosts) |
| `<Config>` Env | `NVIDIA_VISIBLE_DEVICES=all` (NVIDIA hosts) |

## Scaling notes
- Keep the runtime image slim (multi-stage, `--no-install-recommends`) for fast Unraid pulls.
- `/media` Read-Only protects the user's library from any bug in the scanner/asset worker.
- Publish images to GHCR with both `:latest` and version tags so Unraid users can pin.

## Verification
- `docker build -f docker/Dockerfile .` succeeds; image runs; `/api/health` returns 200.
- With `--device /dev/dri` (Intel/AMD) or `--runtime=nvidia` (NVIDIA), a 4K transcode uses
  the GPU (confirm nominal CPU via `intel_gpu_top`/`nvidia-smi`).
- Add the templates repo URL in Unraid CA → `medi` appears; "Install" → pick folders →
  container starts fully hardware-accelerated with `/media` mounted read-only.
- On the TV client, play a title end-to-end: overlay responds to remote via `useTVEventHandler`,
  scrubbing shows trickplay thumbnails.
