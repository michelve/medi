# 20 — Phase 2: Hardware Acceleration & Transcoding Pipeline

> Maps to README §Development Roadmap → Phase 2. Depends on Phase 1 (`10`) and the
> `media_files` metadata from `01-db-schema.md`. Lives in `backend/crates/transcode`.

## Purpose

Transcode heavy formats (4K Dolby Vision, high-bitrate HEVC, AV1) on the fly for clients
that can't direct-play, using GPU hardware acceleration. Dynamically assemble the
`jellyfin-ffmpeg` command line per detected hardware and per source profile, including
Dolby Vision → SDR tone mapping via OpenCL/CUDA.

## Requirements

- Use `jellyfin-ffmpeg` (patched fork) — **not** vanilla FFmpeg (README §Transcoder Core).
- Support three vendor paths: **Intel QSV**, **NVIDIA NVENC/NVDEC**, **AMD AMF/VA-API**.
- Probe host capabilities and pick the path; fall back to software decode where HW can't.
- HDR10 → SDR via **Intel VPP** tone mapping; **DV P5/P8 IPTPQc2 → SDR via OpenCL (Intel/AMD)
  or CUDA (NVIDIA)** with `init_hw_device` chaining (VPP alone distorts DV → purple/green).
- Output HLS (playlist + segments) consumed by `/api/hls/...` (see `02-api-contract.md`).

## Packages / crates

- `tokio` (`process`), `jellyfin-ffmpeg` **binary** injected in the Docker runtime (Phase 5)
- `serde` (capability probe results), `tracing`
- No FFI crate — commands are assembled as strings and spawned as child processes.

## File structure

```
crates/transcode/src/
├── caps.rs        # probe: which GPUs / encoders / OpenCL / CUDA are available
├── decision.rs    # direct-play vs transcode; which vendor path; fallback rules
├── command.rs     # build the ffmpeg argv per (source profile, vendor, target)
├── session.rs     # HLS session lifecycle: spawn, segment dir under /config, teardown
└── lib.rs
```

## Playback decision table (implement in `decision.rs`)

> **This table is the VIDEO axis only.** The AUDIO axis (per-device passthrough vs
> downmix/re-encode, channel caps, immersive-audio handling) and the combined video × audio
> matrix live in `70-audio-quality-and-profiles.md`. `decide()` takes the default audio
> track's descriptor and `AudioTarget` carries a channel count; the two axes decide
> independently (a video full-transcode does not force an audio transcode, and vice versa).

| Source | Client/display | Decision |
|---|---|---|
| Codec+profile client supports, SDR or matching HDR | capable | **Direct play** (`/api/direct`) |
| H.264 **High 10** (8/10-bit 4:2:0 High-10) | any | **Software decode** → HW encode (HW decode universally unsupported) |
| HEVC/AV1 HDR10/HLG → SDR display | HW present | HW decode → **Intel VPP** (or vendor equiv) tone map → HW encode |
| Dolby Vision **P5** → SDR display | Intel/AMD | HW decode → **OpenCL** IPTPQc2→SDR tone map → QSV/VA-API encode |
| Dolby Vision **P5** → SDR display | NVIDIA | NVDEC → **CUDA** tone map → NVENC encode |
| Dolby Vision **P8.1** (HDR10 compat) → SDR | HW present | Treat BL as HDR10 → tone map path above |
| AV1 source, no AV1 HW decode | older host | **dav1d** software decode (bundled in jellyfin-ffmpeg) → HW encode |

## Vendor command sketches (assemble in `command.rs`)

**Intel QSV + VPP (HDR10 → SDR):**
```
ffmpeg -init_hw_device qsv=qs:/dev/dri/renderD128 -filter_hw_device qs \
  -hwaccel qsv -hwaccel_output_format qsv -i INPUT \
  -vf "vpp_qsv=tonemap=1:format=nv12" \
  -c:v h264_qsv -global_quality 23 -f hls ... OUTPUT
```

**Intel + OpenCL (Dolby Vision P5 → SDR):**
```
ffmpeg -init_hw_device vaapi=va:/dev/dri/renderD128 \
  -init_hw_device opencl@va=ocl -filter_hw_device ocl \
  -i INPUT \
  -vf "hwupload,tonemap_opencl=tonemap=bt2390:transfer=bt709:matrix=bt709:primaries=bt709:format=nv12,hwdownload,format=nv12,hwupload=derive_device=qsv" \
  -c:v h264_qsv ... -f hls OUTPUT
```

**NVIDIA + CUDA (Dolby Vision → SDR):**
```
ffmpeg -init_hw_device cuda=cu -filter_hw_device cu -hwaccel cuda \
  -i INPUT \
  -vf "hwupload_cuda,tonemap_cuda=tonemap=bt2390:transfer=bt709:matrix=bt709:primaries=bt709,scale_cuda=format=nv12" \
  -c:v h264_nvenc ... -f hls OUTPUT
```

> Exact filter names/flags depend on the jellyfin-ffmpeg build; validate against the
> installed binary during implementation. The **rule** is fixed: DV tone mapping must go
> through OpenCL/CUDA, not plain VPP.

## Sub-tasks

1. **`caps.rs`**: at boot, probe available devices (`/dev/dri` render nodes, `nvidia-smi`
   presence), and query `ffmpeg -hwaccels` / encoders; cache capability struct.
2. **`decision.rs`**: implement the table above using `media_files` fields + client hints
   from `/api/stream`. Return direct vs (vendor, filter chain, encoder).
3. **`command.rs`**: build argv per decision; chain `init_hw_device` for OpenCL/CUDA DV path.
4. **`session.rs`**: create a session id, spawn `jellyfin-ffmpeg` writing HLS to
   `/config/hls/<session>/`, expose via `/api/hls`; kill process + clean dir on teardown/idle timeout.
5. Wire `/api/stream/:file_id` (Phase 1 `api`) to call `decision.rs`.

## Scaling notes

- Cap concurrent transcode sessions by GPU capacity (README cites e.g. UHD 770 ≈ 4–7 4K
  streams, Arc A380 ≈ 8–12); make the cap configurable, reject with `409` past the cap.
- iGPU tone mapping is memory-bandwidth bound — document the dual-channel RAM recommendation.
- The `assets` worker (Phase 3) must yield the GPU to live sessions (GPU-idle guard).

## Verification

- One transcode per row of the decision table completes and plays via HLS.
- **DV P5 output shows no purple/green tint** (visual check) — proves OpenCL/CUDA path, not VPP.
- H.264 High-10 falls back to software decode without crashing.
- CPU stays nominal during a 4K transcode (HW path engaged, confirm with `intel_gpu_top`/`nvidia-smi`).
