# 100 — Transcode: normalize 10-bit sources to 8-bit for H.264 (fix "video never loads")

> **Status: IMPLEMENTED** (Parts A + B + unit tests). Bug-fix task. Depends on the HWA transcode command
> builder (`20-phase2-hwa-transcode.md`: `transcode/src/command.rs`, `transcode/src/decision.rs`)
> and the web player's HLS path (`97-web-player-shell-and-controls.md`,
> `82-web-ui-player-admin.md`). No new crates, no DB change, no API change.
>
> **Gap this closes.** Playing **any 10-bit HEVC** title in the web player fails: the HLS
> manifest loads (full runtime, all segments) but the first fragment `init.mp4` never arrives,
> hls.js retries `fragLoadTimeOut` every 10 s and goes fatal at 50 s, and playback never starts.
> The transcode session's ffmpeg is silently dying on the very first frame.

## Symptom (as reported)

```
/play/1563
decision/resolved: hls  reason="codec_unsupported"  url="/api/hls/<sid>/index.m3u8"
hls/MANIFEST_PARSED {levels:1}  LEVEL_LOADED {totalduration:8120, fragments:2031}
10063ms hls/recovered networkError: fragLoadTimeOut  url=…/init.mp4
… (10s retries) …
50108ms hls/FATAL networkError: fragLoadTimeOut  url=…/init.mp4  → never recovers
```

The playlist is the server-synthesized VOD playlist (correct, complete), so it always loads;
`init.mp4` and the segments are produced **on demand** by ffmpeg and never appear.

## Root cause (confirmed in the running dev container)

File 1563 = `Downsizing (2017) … [Bluray-2160p]…[x265]-YTS.mkv`, a **4K 10-bit HEVC (Main10),
SDR** source. For `platform=web`, the decision is `hls` / `codec_unsupported` (a browser can't
do HEVC): the target is **H.264**, `tone_map=false`, `software_decode=false`. On the NVIDIA
host the emitted ffmpeg argv has **no `-vf` at all**:

```
-hwaccel cuda -hwaccel_output_format cuda -i <10bit HEVC>
-force_key_frames … -c:v h264_nvenc -rc vbr -cq 23 -c:a aac …   # NO video filter
```

NVDEC decodes Main10 into **CUDA p010 (10-bit)** frames that flow straight into `h264_nvenc`,
which is **8-bit only**. ffmpeg dies immediately (container logs):

```
[h264_nvenc] 10 bit encode not supported
[h264_nvenc] Provided device doesn't support required NVENC features
[out#0/hls] Nothing was written into output file, because at least one of its streams received no packets.
```

No `init.mp4`/segments are ever written → the client times out. **This is general, not
NVIDIA-specific:** `command.rs::HwPlan::filter_graph` returns `None` whenever `tone_map == false`,
so a **10-bit SDR source → H.264 with no tone-map** gets no pixel-format down-conversion on
*any* HW path — `h264_nvenc`, `h264_qsv`, and `h264_vaapi` are all 8-bit-only H.264 encoders
and would fail the same way. The tone-map path already outputs nv12 and is unaffected; only the
no-tone-map 10-bit case is broken. (An HDR source hides the bug because `tone_map=true` inserts
a chain that outputs nv12.)

**Reproduced fix:** adding `-vf scale_cuda=format=nv12` to the exact failing command makes
ffmpeg write `init.mp4` + `seg00000.m4s` with zero errors.

## How Jellyfin handles this (reference: `E:\GitHub\jellyfin-web`)

Jellyfin's browser device profile (`src/scripts/browserDeviceProfile.js`) constrains the H.264
codec profile by `VideoProfile` (`high|main|baseline`), `VideoRangeType` (`SDR`), and
`VideoLevel` — it sets **no** `VideoBitDepth` condition for H.264, and treats HEVC `main 10` as
the 10-bit marker. Pixel-format normalization is the **server's** job: jellyfin-server's ffmpeg
command builder always inserts the encoder-appropriate down-convert (`scale_cuda=format=nv12`
for NVENC, `scale_vaapi`/`scale_qsv=format=nv12` for VA-API/QSV, `format=yuv420p` for libx264)
when the target encoder is 8-bit and the source is 10-bit.

medi's **decision** layer already does the client-side half correctly (web profile = H.264,
`bit_depth_10:false`, so a 10-bit HEVC transcodes to H.264). The missing half is the
**server-side pixel-format conversion in the ffmpeg command builder** — exactly this task. The
NVENC fix is the canonical one (upstream: 10-bit → H.264 on NVENC needs an explicit `format=nv12`;
Pascal-and-later H.264 NVENC is 8-bit-only). `scale_cuda=format=nv12` does the convert on-GPU
(no CPU roundtrip); medi already uses that same filter in its DV tone-map path (`command.rs`).

## Scope

**Targeted fix only** (agreed): emit an 8-bit (nv12 / yuv420p) conversion **iff** the target
codec is H.264 **and** the source is 10-bit **and** we're not already tone-mapping. Do **not**
broaden to other pixel formats (4:2:2/4:4:4) or add a session health-check in this task.

## Part A — thread source bit depth into the command builder

`TranscodeTarget` (`backend/crates/transcode/src/decision.rs`) does not carry the source bit
depth. Add a field:

```rust
/// Source luma bit depth (8 or 10). When 10 and the (8-bit-only) H.264 encoder is used
/// without a tone-map, the filter graph must down-convert to nv12/yuv420p or the HW encoder
/// rejects the frames ("10 bit encode not supported").
pub source_bit_depth: u8,
```

Populate it in `transcode_target(...)` from `profile.bit_depth` (already on `MediaProfile`,
`backend/crates/core/src/profile.rs`). It's constructed in exactly one place. Update the
`TranscodeTarget { … }` literals in the `decision.rs` test module and the `command.rs` test
`target(...)` helper to set `source_bit_depth` (default the helpers to 8).

## Part B — emit the 8-bit conversion in `HwPlan::filter_graph`

In `backend/crates/transcode/src/command.rs`, `filter_graph(&self, t)` early-returns `None`
when `!t.tone_map`. Change that no-tone-map branch: when
`t.video_codec == VideoCodec::H264 && t.source_bit_depth >= 10`, return the vendor-appropriate
down-convert instead of `None`:

| `HwPlan` | filter |
|---|---|
| `Nvidia` | `scale_cuda=format=nv12` (frames are already CUDA — verified working) |
| `Intel { .. }` (non-DV HW path) | `vpp_qsv=format=nv12` (mirrors the existing `vpp_qsv=tonemap=1:format=nv12`, minus tonemap) |
| `Amd { .. }` (VA-API) | `scale_vaapi=format=nv12` |
| `Software` | `format=yuv420p` (CPU frames; forces libx264 to 8-bit High → broadly browser-decodable) |

Leave the tone-map branch unchanged (it already yields nv12/yuv420p).

**Interactions (no extra code needed):**
- **Burn-in.** `build_argv` uses `hw.filter_graph(target)` as the `base` for
  `build_burn_in_filter`, so returning a conversion here also makes burn-in over a 10-bit source
  correct.
- **`software_decode`.** `HwPlan::for_target` already returns `HwPlan::Software` when
  `software_decode` is true, so the Software arm's `format=yuv420p` covers CPU-frame cases (e.g.
  a future AV1-10bit→H264). The match on `self` routes correctly.
- **8-bit sources.** Unchanged — still no `-vf` (must not regress `sdr_transcode_has_no_filter`).

## Verification

1. **Unit tests** (`cargo test -p medi-transcode`; runs in the dev container per
   `medi-no-rust-toolchain`, or in CI):
   - New: 10-bit SDR HEVC → H.264 on Nvidia emits `scale_cuda=format=nv12` + `h264_nvenc`; on
     Intel emits `vpp_qsv=format=nv12`; software emits `format=yuv420p`.
   - New: an **8-bit** source → H.264 with no tone-map emits **no** `-vf` (guards the regression).
   - Existing tone-map / DV / burn-in tests stay green.
2. **End-to-end** in `docker-medi-1` (NVENC via WSL2):
   - Rebuild: `docker compose -f docker/compose.dev.yml up --build -d`.
   - `curl "http://localhost:8096/api/stream/1563?platform=web"` → `mode:"hls"`; take the session
     id from the URL.
   - `curl -f "http://localhost:8096/api/hls/<id>/init.mp4"` → **200 with bytes** (was a timeout),
     and `docker exec docker-medi-1 ls /config/hls/<id>/` shows `init.mp4` + `seg00000.m4s`.
   - Browser `http://localhost:5174/play/1563`: playback starts, no `fragLoadTimeOut` fatal.
   - `docker logs docker-medi-1` shows **no** `10 bit encode not supported` /
     `Nothing was written into output file` for the new session.
3. **No regression:** an 8-bit H.264 file still direct-plays; an HDR10 source still tone-maps
   (nv12) and plays.

## Cross-references

- **`20-phase2-hwa-transcode.md`** — one line under the command-builder notes: 10-bit sources
  targeting the 8-bit H.264 encoder get an nv12/yuv420p down-convert in `filter_graph` (task 100).
