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
this Windows dev box (no Rust toolchain here — see the repo memory). Build it in
CI or on the Unraid/Linux host. The Dockerfile is authored correct-by-construction
against the confirmed backend contract (binary name `medi`, env defaults
`/media` · `/config` · `0.0.0.0:8096`, `/api/health` → `ok`, self-migrating DB).
