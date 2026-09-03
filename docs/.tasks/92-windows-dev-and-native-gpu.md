# 92 — Windows Docker Dev Loop & Native GPU

> **Status (2026-09-02): SPEC — not yet implemented.** Cross-cutting dev-tooling task,
> peer to `90-format-coverage-and-subtitles.md` and `91-genres-and-people-discovery.md`.
> Depends on the shipped Docker image (`50-phase5-playback-packaging.md`), the HWA probe +
> command builder (`20-phase2-hwa-transcode.md`: `transcode/src/caps.rs`,
> `transcode/src/command.rs`), and the `/config` vs `/media` contract
> (`00-architecture.md`).
>
> **Gap this closes.** `medi` builds and deploys as a **Linux/glibc Debian container for
> Unraid**: the caps probe reads `/dev/dri/renderD*` + `nvidia-smi`, the Dockerfile bakes
> Intel/AMD VA-API drivers, and `docker/README.md` states the image "cannot be built on this
> Windows dev box." That is true for producing a **release artifact**, but it leaves the
> developer — who works on **Windows with an NVIDIA RTX GPU** — with **no documented fast
> local loop**: no dev compose file, no software fallback, and no guidance for getting the
> RTX GPU into a Docker Desktop container for real **NVENC** transcode testing. Today the
> only tested inner loop is `cargo test` on the host (no ffmpeg, no GPU, no end-to-end
> stream), so any change to the decision engine, the `/api/stream` handler, HLS packaging,
> or the web player can only be exercised against real hardware in CI or on the Unraid box.
>
> This task adds the **developer inner loop on Windows**: a dev image + compose that mounts
> the repo's media/appdata, passes the RTX GPU through Docker Desktop's WSL2 backend for
> genuine NVENC transcode, and falls back to software transcode where GPU passthrough isn't
> possible — all without changing the production deploy path.

## Purpose

Give a Windows developer a **build-once, iterate-fast** loop that exercises the whole stack
end-to-end (scan → browse → transcode → play) against **real GPU hardware**:

1. **NVIDIA NVENC in Docker Desktop (primary).** Run the same Linux container the Unraid box
   runs, with the RTX GPU passed through via the **NVIDIA Container Toolkit** on the WSL2
   backend. The existing probe lights up the NVENC path with **no code change**.
2. **Fast rebuilds.** A **dev image target** does a *debug* backend build with cargo
   registry/target caches mounted, so a Rust edit rebuilds in seconds instead of a full
   `--release` compile.
3. **Honest fallback.** AMD/Intel GPUs cannot be passed through Docker Desktop's WSL2 kernel
   today (no `/dev/dri`); those hosts (and any no-GPU box) fall back to **software transcode**
   (libx264) via the existing `HwCaps::software_only()` — correct output, slower, zero host
   deps. Real AMD/Intel HW-encode validation stays on the Linux/Unraid host or in CI.

**Key property: this is a dev-tooling + docs task.** The NVIDIA path already works
(`caps::probe()` → `/dev/nvidia0` / `nvidia-smi`, `command.rs` emits `*_nvenc`), and the
software fallback already exists. The spec's job is to make the loop **discoverable and
repeatable**, not to add a production target.

## Requirements

- **Same image family as prod.** The dev image is the same Debian + jellyfin-ffmpeg + GPU
  driver runtime as `docker/Dockerfile` — only the backend build differs (debug + caches).
  A transcode that works in dev must be the same code path that runs on Unraid; do not fork
  the transcode pipeline for dev.
- **No change to the production deploy path.** `docker/Dockerfile`, the CI release build
  (`.github/workflows/docker-publish.yml`), and the Unraid template are untouched. CI stays
  the canonical release build.
- **No change to the `/config` vs `/media` contract** (`00-architecture.md`). Dev bind-mounts
  a Windows media folder → `/media:ro` and a local appdata dir → `/config` (read-write). The
  container still writes only under `/config`.
- **NVENC works with zero backend code change.** The dev loop must light up `Vendor::Nvidia`
  purely from container config (`--gpus all` / compose device reservation + the toolkit).
- **Software fallback works with zero code change.** `MEDI_GPU_VENDOR=none` (or a host with no
  passthrough) yields a valid fMP4 HLS transcode via libx264, so decision/API/UI iteration
  never blocks on GPU availability.
- **Backward compatible + no auth.** LAN-first, no auth, no new production surface. Any code
  tweak (see §5) is OPTIONAL and must not change existing behavior for the Unraid path.

## Packages / crates

**No new crates, and no *required* code change.** The task ships **docker artifacts + docs**:
a dev compose file, a dev image target, `docker/README.md` edits, and cross-ref lines in
`00`/`20`/`50`. The two OPTIONAL robustness tweaks (§5) touch only
`backend/crates/transcode/src/caps.rs` and are backward-compatible.

## File structure (where to save)

```
docker/
├── compose.dev.yml         # NEW: dev service — dev image target + GPU passthrough + bind mounts
├── Dockerfile.dev          # NEW: dev image (same runtime as prod; debug backend build + caches)
│                           #      (alternative: a `dev` build stage inside docker/Dockerfile)
└── README.md               # EDIT: scope the "can't build on Windows" note to release artifacts;
                            #       add the Windows dev-loop pointer to this task + compose.dev.yml
docs/.tasks/
├── 00-architecture.md      # EDIT (1 line): local dev on Windows sits beside the Unraid deploy shape
├── 20-phase2-hwa-transcode.md  # EDIT (1 line): NVENC dev path via WSL2 + NVIDIA Container Toolkit
└── 50-phase5-playback-packaging.md # EDIT (1 line): dev image (debug + caches) vs release image
backend/crates/transcode/src/
└── caps.rs                 # OPTIONAL (§5): short-circuit MEDI_GPU_VENDOR=none to software_only()
```

## 1 — Prerequisites (Windows host)

The developer sets these up once. The spec documents them; nothing in the repo enforces them.

- **Docker Desktop with the WSL2 backend** (Settings → General → *Use the WSL 2 based
  engine*). The Hyper-V backend cannot pass GPUs through.
- **A current Windows NVIDIA driver** (the normal GeForce/Studio driver — it carries the
  WSL2 CUDA/NVENC passthrough libraries). No separate "CUDA on WSL" driver is needed.
- **NVIDIA Container Toolkit inside the WSL2 distro** Docker Desktop uses (installed once in
  the distro; Docker Desktop wires `--gpus` to it). This is what injects `/dev/nvidia*` +
  `nvidia-smi` into a container.
- **Verify the toolkit before touching `medi`:**
  ```sh
  docker run --rm --gpus all nvidia/cuda:12.4.0-base-ubuntu22.04 nvidia-smi
  ```
  Seeing the RTX card listed here is the precondition for everything below. If this fails,
  fix the toolkit first — `medi` can do nothing the base CUDA image can't.

## 2 — Dev image + compose (the fast loop)

### `docker/Dockerfile.dev` (or a `dev` stage in `docker/Dockerfile`)

Same **runtime** as production — the Debian base, GPU user-mode drivers, and jellyfin-ffmpeg
symlinked onto `PATH` — so the transcode code path is identical to Unraid. The **only**
differences from prod:

- **Debug backend build** (`cargo build --bin medi`, no `--release`) so compiles are fast.
- **Cargo registry + target cache mounts** kept warm across rebuilds
  (`--mount=type=cache,target=/usr/local/cargo/registry` and `.../target`), so an edit → run
  cycle rebuilds only the changed crate, in seconds.
- The **web SPA stage is reused unchanged** (a debug web build adds nothing for backend work;
  keep the same `@medi/web` build the prod image uses, or mount a prebuilt `dist/`).

Keep the prod `ENV` defaults (`MEDIA_DIR=/media`, `CONFIG_DIR=/config`, `WEB_DIR`,
`BIND_ADDR=0.0.0.0:8096`, `NVIDIA_DRIVER_CAPABILITIES=compute,video,utility`) and the same
`entrypoint.sh` — its GPU-visibility log lines are exactly what makes a misconfigured
passthrough obvious in `docker compose logs`.

> **On rebuild speed.** The very first `up --build` still pays for the base image + apt layers
> + a cold cargo build. It's the *subsequent* rebuilds (after a Rust edit) that the cache
> mounts make fast. For an even tighter loop a developer may instead mount the host cargo
> target and run `cargo run` inside the container, but the cache-mounted dev image is the
> documented default (no host Rust toolchain required, matching the `medi-no-rust-toolchain`
> reality of this box).

### `docker/compose.dev.yml`

One `medi` service that:

- **Builds** `docker/Dockerfile.dev` (context = repo root, same as prod).
- **Reserves the NVIDIA GPU** via the compose device reservation (equivalent to `--gpus all`):
  ```yaml
  services:
    medi:
      build:
        context: ..
        dockerfile: docker/Dockerfile.dev
      ports: ["8096:8096"]
      volumes:
        - type: bind
          source: ${MEDI_MEDIA:-./devdata/media}   # a Windows folder of sample clips
          target: /media
          read_only: true
        - type: bind
          source: ${MEDI_CONFIG:-./devdata/config}  # local appdata (db, hls, subs, previews)
          target: /config
      environment:
        RUST_LOG: ${RUST_LOG:-info,medi_transcode=debug}
        NVIDIA_VISIBLE_DEVICES: all
        NVIDIA_DRIVER_CAPABILITIES: compute,video,utility
        # MEDI_GPU_VENDOR: nvidia   # optional explicit override; probe already auto-detects
      deploy:
        resources:
          reservations:
            devices:
              - driver: nvidia
                count: all
                capabilities: [gpu, compute, video]
  ```
- Documents the **AMD/Intel-software fallback** as commented lines: drop the `deploy.devices`
  block and set `MEDI_GPU_VENDOR: none` (see §4).

> Bind-mount sources use `${MEDI_MEDIA}` / `${MEDI_CONFIG}` with `./devdata/...` defaults so a
> fresh clone runs with zero edits — the developer drops a couple of sample clips into
> `devdata/media` (git-ignored) and goes. Windows paths work as env overrides
> (`MEDI_MEDIA=E:/Movies`).

Run it:
```sh
docker compose -f docker/compose.dev.yml up --build
# edit Rust → re-run the same command; only the changed crate recompiles
```

## 3 — NVIDIA NVENC path (primary; already wired)

Nothing in the transcode code changes. The chain the dev loop exercises:

1. **Passthrough** — the toolkit injects `/dev/nvidia0` + `nvidia-smi` into the container.
2. **Probe** — `transcode/src/caps.rs::probe()` calls `nvidia_present()`, which returns true
   from `/dev/nvidia0` (or `nvidia-smi -L`), so `vendor = Some(Vendor::Nvidia)` and
   `cuda = true`. (`MEDI_GPU_VENDOR=nvidia` forces this explicitly if ever needed.)
3. **Decision** — `decision.rs` picks the NVIDIA path.
4. **Command** — `command.rs::HwPlan::Nvidia` emits `-init_hw_device cuda`, `-hwaccel cuda`,
   and `h264_nvenc` / `hevc_nvenc`, with `tonemap_cuda` for HDR/DV → SDR.

**Verify NVENC is really engaged** (not silently falling back to software):
```sh
docker compose -f docker/compose.dev.yml exec medi nvidia-smi   # RTX card listed
curl -f http://localhost:8096/api/health                        # → ok
# drive a transcode (a 4K HEVC/HDR source the client can't direct-play):
curl "http://localhost:8096/api/stream/<file_id>?..."           # → mode:"hls"
docker compose -f docker/compose.dev.yml exec medi nvidia-smi   # Encoder util > 0, a medi/ffmpeg PID
```
Host CPU should stay nominal during the transcode; a busy CPU + zero encoder util means the
probe didn't see the GPU — recheck §1.

## 4 — AMD / Intel on Windows (limitation + software fallback)

**State this plainly in the spec:** Docker Desktop's WSL2 kernel exposes **no `/dev/dri`**
render node, so Intel **QSV/VA-API** and AMD **AMF/VA-API** **cannot be passed through** on
Windows Docker today — regardless of the host GPU. This is a WSL2/Docker-Desktop limitation,
not a `medi` bug.

The dev loop still runs, via the existing software path:

- With no `/dev/dri` and no NVIDIA, `caps::probe()` already returns `vendor = None` →
  `HwPlan::Software` → **libx264** encode with zscale/tonemap for HDR. Output is valid fMP4
  HLS; only slower.
- Force it explicitly (e.g. to bypass a partial probe, or to test the software path on an
  NVIDIA box) with `MEDI_GPU_VENDOR=none` in `compose.dev.yml`.
- This is enough to iterate on **everything except NVENC-specific behavior**: the decision
  engine, `/api/stream`, HLS packaging, subtitles/burn-in, the web player, metadata/browse.

**Real Intel/AMD HW-encode validation** happens on the **Linux/Unraid host** (with
`--device /dev/dri`) or in CI — the same as today (`docker/README.md` verify steps). This
task does **not** try to make QSV/VA-API work on Windows.

## 5 — OPTIONAL small robustness tweaks (`caps.rs`)

Nice-to-haves only; the loop works without them. Keep backward-compatible.

- **Short-circuit `MEDI_GPU_VENDOR=none`.** Today `none` yields `vendor = None` but still runs
  the `/dev/dri` + `nvidia-smi` + `ffmpeg -hwaccels` subprocess probes. Have `probe()` return
  `HwCaps::software_only()` immediately when the override is `none`, so a dev container that
  wants software never misfires on a stray host GPU signal and starts a hair faster. (Pure
  early-return; every other override and the auto-detect path are unchanged. Add a unit test
  asserting `none` ⇒ `software_only()`.)
- **A one-line resolved-vendor boot log** naming the final vendor + whether an NVENC encoder
  was found, so `docker compose logs` shows at a glance whether passthrough worked. Most of
  this already exists in `entrypoint.sh`'s GPU lines and `probe()`'s `tracing::info!`; this
  just makes the *resolved* decision (post-probe) explicit next to them.

If implemented, these ship behind the existing `cargo test -p medi-transcode` suite; they add
no new dependency and do not alter the Unraid runtime behavior.

## 6 — Media fixtures for a fresh clone

So `up --build` yields a browsable, transcodable library without a real 10k library:

- Document a **`devdata/media/`** dir (git-ignored) the developer fills with a few short
  clips — ideally one **4K HEVC/HDR** and one **DV** sample so the NVENC tone-map path is
  exercised, plus one plain **H.264** for the direct-play/remux path.
- `devdata/config/` (git-ignored) holds the dev `library.db`, `hls/`, `subs/`, `previews/` —
  wiped freely to reset state.
- Reuse any existing fixture guidance (`backend/tests/` sample-media metadata) for what a
  representative source looks like; the point is a couple of real files, not a fixture crate.

## Verification

> Note: this dev machine has no host Rust toolchain (see the `medi-no-rust-toolchain` note).
> The dev **container** builds the backend itself; the OPTIONAL `caps.rs` unit test (§5) runs
> on a machine with Rust installed / in CI.

- **Toolkit precondition:** `docker run --rm --gpus all nvidia/cuda:12.4.0-base-ubuntu22.04
  nvidia-smi` lists the RTX card.
- **Dev loop up:** `docker compose -f docker/compose.dev.yml up --build` starts; logs show the
  `entrypoint.sh` NVIDIA line and a resolved `Vendor::Nvidia`; `curl /api/health` → `ok`; the
  web UI loads at `/`.
- **NVENC engaged:** a 4K HDR `/api/stream` request → `mode:"hls"`; `nvidia-smi` inside the
  container shows encoder utilization + an ffmpeg PID; host CPU stays nominal.
- **Fast rebuild:** edit a Rust file → re-run `up --build`; only the changed crate recompiles
  (seconds), not a full `--release` build.
- **Software fallback:** set `MEDI_GPU_VENDOR=none` (or run on a no-GPU box) → the same
  `/api/stream` request still returns valid fMP4 HLS via libx264; no `-hwaccel` in the argv.
- **No regression:** `docker/Dockerfile` (prod), the CI release workflow, and the Unraid
  template are unchanged; `cargo test --workspace` is green (including the OPTIONAL §5 test if
  added).

## Cross-references (edits required in lockstep)

- **`00-architecture.md`** — one line beside the Unraid deploy shape: local development on
  Windows uses `docker/compose.dev.yml` (NVENC via Docker Desktop WSL2 + NVIDIA Container
  Toolkit; software fallback otherwise) — see task 92. The `/config` vs `/media` contract is
  unchanged (dev bind-mounts a Windows media folder → `/media:ro` and a local dir → `/config`).
- **`20-phase2-hwa-transcode.md`** — one line: the NVENC vendor path is exercised locally on
  Windows via task 92's dev image; the probe (`caps.rs`) lights up `Vendor::Nvidia` unchanged
  from the toolkit-injected `/dev/nvidia0` / `nvidia-smi`. Intel/AMD HW-encode is **not**
  passthrough-able on Windows Docker (no `/dev/dri`) → software fallback there.
- **`50-phase5-playback-packaging.md`** — one line under Part B (Docker image): the **release**
  image (`docker/Dockerfile`, built in CI) is distinct from the **dev** image
  (`docker/Dockerfile.dev`, debug + cache mounts) introduced by task 92 for the Windows inner
  loop; only the release image is published/pinned by the Unraid template.
