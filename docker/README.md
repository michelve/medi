# docker

Multi-stage, Debian-based image (Phase 5, `docs/.tasks/50-phase5-playback-packaging.md`).

- **`Dockerfile`** — `rust:1-bookworm` build stage compiles the `medi` release
  binary; `debian:bookworm-slim` runtime stage injects the proprietary user-mode
  GPU drivers (`intel-media-va-driver-non-free` + `intel-opencl-icd`,
  `mesa-va-drivers`) and **jellyfin-ffmpeg** (symlinked onto `PATH` as
  `ffmpeg`/`ffprobe`, which `medi-transcode` calls). NVIDIA is **not** baked in —
  the host's `--runtime=nvidia` injects it.
- **`entrypoint.sh`** — prepares the writable `/config` layout, logs the GPU
  passthrough the host actually exposed, then `exec`s `medi` as PID 1. The binary
  migrates SQLite itself on boot (refinery), so there is no separate migrate step.

## Build & run

```sh
# Build context is the repo root.
docker build -f docker/Dockerfile -t medi:dev .

# Intel/AMD (VA-API/QSV) — pass the render node:
docker run --rm -p 8096:8096 \
  --device /dev/dri:/dev/dri \
  -v /mnt/user/appdata/medi:/config \
  -v /mnt/user/movies:/media:ro \
  medi:dev

# NVIDIA — use the NVIDIA container runtime instead of --device:
docker run --rm -p 8096:8096 \
  --runtime=nvidia -e NVIDIA_VISIBLE_DEVICES=all \
  -v /mnt/user/appdata/medi:/config \
  -v /mnt/user/movies:/media:ro \
  medi:dev
```

## Verify (per the task)

- `curl -f http://localhost:8096/api/health` → `200 ok`.
- `docker exec <ctr> vainfo` lists VA profiles (Intel/AMD) — passthrough works.
- A 4K transcode uses the GPU: watch `intel_gpu_top` (Intel) / `nvidia-smi`
  (NVIDIA) while `/api/stream` drives a transcode; host CPU stays nominal.

## Local dev on Windows (fast loop)

`docker/compose.dev.yml` + `docker/Dockerfile.dev` are the **developer inner loop** on a
Windows box with Docker Desktop (WSL2 backend) — build once, bind-mount media/appdata, and
hit **real NVENC** transcode on an NVIDIA RTX GPU, with a debug build + cargo cache mounts so
Rust edits rebuild in seconds. See **`docs/.tasks/92-windows-dev-and-native-gpu.md`** for the
full setup (NVIDIA Container Toolkit prereq, software fallback for AMD/Intel).

```sh
# Prereq: verify GPU passthrough works at all (task 92 §1)
docker run --rm --gpus all nvidia/cuda:12.4.0-base-ubuntu22.04 nvidia-smi

# Then run the dev loop (edit Rust -> re-run; only the changed crate recompiles):
docker compose -f docker/compose.dev.yml up --build
```

## Metadata & artwork env vars

Enrichment (posters, overview, cast, and title logos) is driven by a few optional env vars —
pass them with `-e` on `docker run`, or set them in the Unraid template (each has a `<Config>`
entry there). All are **optional**: with none set, titles still scan and play, just without
online metadata.

| Env var             | What it does                                                              | Default  |
| ------------------- | ------------------------------------------------------------------------ | -------- |
| `TMDB_API_KEY`      | Posters, backdrops, overview, cast/crew. Free from themoviedb.org (Settings → API). Used when `METADATA_PROVIDER=tmdb` (the default). | *(unset)* |
| `OMDB_API_KEY`      | Alternative provider, used only when `METADATA_PROVIDER=omdb`.            | *(unset)* |
| `METADATA_PROVIDER` | `tmdb` (default) or `omdb`.                                               | `tmdb`   |
| `METADATA_LANGUAGE` | Preferred metadata language (BCP-47), e.g. `en-US`, `fr-FR`.              | `en-US`  |
| `FANARTTV_API_KEY`  | **Movie title logos + background wallpapers** on detail pages — the transparent-PNG wordmark shown over the backdrop instead of the text title, and a curated wallpaper shown on the hero in place of the TMDB backdrop. Free from fanart.tv (Account → API). **Independent** of the TMDB/OMDb keys; leave blank to disable. Movies only. | *(unset)* |
| `BACKFILL_INTERVAL_HOURS` | How often a background pass re-checks already-matched titles for newer metadata/artwork (genres, collections, fanart logos + wallpapers) — e.g. picking up fanart art added after a title was matched. `0` disables it (the "Refresh metadata & artwork" button in **Settings → Libraries** still works on demand). | `24` |

Logos and wallpapers are downloaded and served locally (`/config/images/movies/<id>/logo.png`
and `wallpaper.jpg`) exactly like posters — fanart.tv is never hotlinked from the client, and
both art types come from a single request per movie. A movie without a logo shows its text
title; without a wallpaper it shows the TMDB backdrop, unchanged.

## Note

The **release** image (`docker/Dockerfile`) is Linux/glibc and is built in **CI** (the
canonical path) or on the Unraid/Linux host — that is where the published, Unraid-pinned
artifact comes from. Docker **Desktop on Windows can still build and run the Linux image
locally** for development (that is exactly what the dev loop above does); the Windows box just
shouldn't be the source of a *published release*. (The Windows box can also `cargo
test`/`cargo build` the backend directly for verification.) The Dockerfile is authored against
the confirmed backend contract (binary name `medi`, env defaults `/media` · `/config` ·
`0.0.0.0:8096`, `/api/health` → `ok`, self-migrating DB).

## CI, releases & Unraid updates

`.github/workflows/docker-publish.yml` is the canonical build. It gates every push
and PR on the CI job (`cargo test --workspace` + web typecheck/build), then:

| Trigger                     | Image tags published                          | Moves `:latest`? |
| --------------------------- | --------------------------------------------- | ---------------- |
| push to `main`              | `edge`, `sha-<short>`                          | **no**           |
| push a tag `vX.Y.Z`         | `latest`, `X.Y.Z`, `X.Y`, `X`, `sha-<short>`  | **yes**          |
| pull request                | *(builds only, nothing pushed)*               | no               |

So `main` stays continuously built and tested, but the tag Unraid watches
(`:latest`) only advances on a real release — an update prompt on Unraid always
means a version you cut on purpose.

### Cut a release

```sh
git tag v0.2.0
git push origin v0.2.0     # → the workflow builds, tests, and publishes :latest + v0.2.0
```

Use semver. The first `v*` build is also the moment to set the GHCR package
**Public** (one-time), so Unraid can pull without auth:
<https://github.com/michelve/medi/pkgs/container/medi>.

### Updating on Unraid

The template pins `ghcr.io/michelve/medi:latest`, so:

- **Manual:** Docker tab → medi → **Check for Updates** (or *Force Update*). Unraid
  compares the local image digest against `:latest` on GHCR and shows
  "update ready" once a new release is published; **Apply Update** pulls and
  recreates the container. `/config` and `/media` are volumes, so data survives.
- **Automatic:** install **CA Auto Update Applications** (Community Applications →
  search "Auto Update"). Enable auto-update for `medi` and pick a schedule; it
  watches the `:latest` digest and updates the container hands-off. Because
  `:latest` only moves on tagged releases, this won't churn on every dev commit.
- **Bleeding edge (opt-in):** to track `main` instead, edit the container's
  *Repository* to `ghcr.io/michelve/medi:edge`. Then every green push to `main`
  is an available update. Not recommended for a stable box.
- **Pin a version:** set *Repository* to a specific tag, e.g.
  `ghcr.io/michelve/medi:0.2.0`, to freeze updates entirely.
