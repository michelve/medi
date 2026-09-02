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

## Note

The image is Linux/glibc and compiles the Rust backend, so it cannot be built on
this Windows dev box — build it in CI (the canonical path) or on the Unraid/Linux
host. (The Windows box *can* still `cargo test`/`cargo build` the backend for
verification; it just can't produce the Debian runtime image.) The Dockerfile is
authored against the confirmed backend contract (binary name `medi`, env defaults
`/media` · `/config` · `0.0.0.0:8096`, `/api/health` → `ok`, self-migrating DB).

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
