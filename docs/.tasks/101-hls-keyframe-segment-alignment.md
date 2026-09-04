# 101 — Transcode: force keyframes at HLS segment boundaries (fix `fragParsingError`)

> **Status: IMPLEMENTED** (Parts A + B + unit tests). Bug-fix task. Depends on the HWA transcode
> command builder (`20-phase2-hwa-transcode.md`: `transcode/src/command.rs`, `decision.rs`), the
> web player's HLS path (`97-web-player-shell-and-controls.md`), the probed frame rate
> (`99-subtitles-and-chapters.md`: `media_files.frame_rate`, V13), and task 100 — the pixfmt fix
> that unmasked this. No new crates, no DB change, no API change.
>
> **Gap this closes.** After task 100 made ffmpeg finally *produce* segments for a 10-bit source,
> playback still failed: the segments come out **~10 s long** while the server's playlist promises
> **4 s** segments, so hls.js can't align a fragment to the timeline and dies with
> `fragParsingError` ("Found no media in msn N"). This is **general** — it hits any source whose
> native GOP is longer than `SEGMENT_SECONDS`, on **every** encoder — not just the 10-bit file.

## Symptom (as reported)

```
/play/1563
hls/LEVEL_LOADED {totalduration:8120, fragments:2031}
hls/FRAG_LOADED #0 {sn:0, duration:4}
hls/recovered mediaError: fragParsingError  "Found no media in msn 0 ... seg00000.m4s"
… repeats per fragment …
hls/FATAL mediaError: fragParsingError #6   → never recovers, black screen at 0:00 / --:--
```

## Root cause (confirmed on disk in the running dev container)

The server synthesizes a VOD playlist (`command::build_vod_playlist`) that declares fixed
`SEGMENT_SECONDS` (**4 s**) segments — segment `N` = `[N·4, (N+1)·4)`. The whole seek/segment model
keys off this: `-ss = start_segment·4`, `-start_number = start_segment`.

But the transcode's only keyframe control was:

```
-force_key_frames expr:gte(t,n_forced*4)      # command.rs
```

and **no `-g` / `-forced-idr` / `-keyint_min` / `-sc_threshold`**. On the failing NVENC job the
emitted segments were **~10.4 s** each (`ffmpeg.m3u8` → `#EXTINF:10.427083`; `ffprobe` on
`init.mp4`+`seg00000.m4s` → `duration=10.449083` with a **single keyframe at t=0.125 s**, none at 4 s
or 8 s).

Why: `h264_nvenc` **silently ignores `-force_key_frames`** unless `-forced-idr 1` is set (NVENC's
default IDR mode `-1` does not propagate forced-keyframe requests). And per the standard HLS recipe,
`-force_key_frames` alone is insufficient even on other encoders — the encoder must also have its GOP
pinned (`-g`) with scene-cut disabled so a keyframe can't drift off the segment grid. So the encoder
kept its native ~10 s GOP; the fMP4 HLS muxer can only cut on a keyframe → ~10 s segments →
timeline desync → `fragParsingError`.

### References

- FFmpeg/NVENC forced-keyframe behavior (needs `-forced-idr 1`):
  <https://patchwork.ffmpeg.org/project/ffmpeg/patch/58A097A9.5010702@email.cz/> ·
  <https://forums.developer.nvidia.com/t/idr-frames-and-gop-size-configuration-in-nvenc-h264/294758>
- HLS GOP discipline (`-g` + closed GOP + `sc_threshold=0`; HW encoders use `-g`, not x264opts):
  <https://www.mpegflow.com/recipes/keyframe-interval-tuning-for-hls>
- hls.js "Found no media in msn" = fragment/timeline misalignment:
  <https://github.com/video-dev/hls.js/issues/6436>

## Scope

Targeted fix in the ffmpeg command builder only. Keep `SEGMENT_SECONDS`, the synthesized playlist,
the seek model, the api layer, and the decision table unchanged. Keep
`-force_key_frames expr:gte(t,n_forced*SEGMENT_SECONDS)` as the exact-boundary driver; add the
per-encoder GOP/IDR flags that make the encoder honor it.

## Part A — thread source frame rate → a GOP length into the command builder

The frame rate is already probed and stored (`media_files.frame_rate REAL`, V13). Thread it exactly
like task 100's `source_bit_depth`:

1. `MediaProfile` (`core/src/profile.rs`): add `frame_rate: Option<f64>` (`#[serde(default)]`).
   `MediaProfile` drops `Eq` (an `f64` isn't `Eq`) — nothing compares it for equality or hashes it,
   so `PartialEq` suffices, and the decision output `TranscodeTarget` stays `Eq` (it carries the
   integer `gop_frames`, not the float). Same pattern `MediaFile` already uses.
2. `MediaFile::profile()` (`db/src/models.rs`): set `frame_rate: self.frame_rate`.
3. `TranscodeTarget` (`transcode/src/decision.rs`): add `gop_frames: u32`, populated in
   `transcode_target(...)` via `gop_frames(profile.frame_rate)` = `round(fps · SEGMENT_SECONDS)`,
   with a safe fallback (default 24 fps) and clamp (`SEGMENT_SECONDS..=480`) for missing/absurd fps.

## Part B — emit per-encoder keyframe/GOP flags in `command.rs`

New `HwPlan::gop_args(&self, t)`, emitted right after the encoder + `quality_args` in `build_argv`,
using `t.gop_frames` (`N`). `-force_key_frames` (unchanged) forces the exact boundary; `-g N` caps
the encoder's max spacing so it can't drift past one.

| `HwPlan` | flags | why |
|---|---|---|
| `Nvidia` | `-forced-idr 1 -g N -no-scenecut 1` | `-forced-idr 1` is **the fix** (NVENC honors forced keyframes); `-g`+no-scenecut pin the grid. |
| `Intel { .. }` (QSV) | `-g N -forced_idr 1` | pin GOP; force IDR at boundaries. |
| `Amd { .. }` (VA-API) | `-g N` | honors `force_key_frames` with a capped GOP. |
| `Software` (libx264/265) | `-g N -keyint_min N -sc_threshold 0` | closed GOP, no scene-cut drift (canonical libx264 HLS recipe). |

**Interactions (no extra code):** the tone-map and burn-in paths keep their filters; `gop_args`
appends after the encoder regardless. 8-bit and HDR paths get the same alignment (correct — they had
the same latent bug).

## Verification

1. **Unit** (`cargo test -p medi-transcode`, dev container per `medi-no-rust-toolchain`): each plan
   emits `-g <gop_frames>`; NVENC also `-forced-idr 1`/`-no-scenecut 1`; software `-keyint_min` +
   `-sc_threshold 0`; `-force_key_frames` still present. Decision: 23.976 fps → `gop_frames=96`;
   None/NaN/≤0 → clamped fallback (96). Existing + task-100 tests stay green. (75 pass.)
2. **End-to-end** in `docker-medi-1` (NVENC; SMB media via
   `-f docker/compose.dev.yml -f docker/compose.dev.override.yml up -d`):
   - `curl .../api/stream/1563?platform=web` → hls; take the session id.
   - Fetch `seg00000.m4s`, concat with `init.mp4`, `ffprobe` → **duration ≈ 4 s** with a keyframe at
     t≈0 (was ~10.4 s, one keyframe); `ffmpeg.m3u8` shows `#EXTINF:4.0…`.
   - Browser `http://localhost:5174/play/1563`: **no `fragParsingError`**; video renders, duration
     shows the real runtime, seeking lands cleanly.
3. **No regression:** an 8-bit H.264 transcode and an HDR10 tone-map both segment at 4 s and play.

## Cross-references

- **`20-phase2-hwa-transcode.md`** — command-builder notes: the transcode encoder pins its GOP to
  `≈ fps × SEGMENT_SECONDS` (`-g`, plus `-forced-idr` on NVENC) so segments cut on a keyframe at every
  segment boundary (task 101).
- **`100-transcode-pixfmt-10bit-to-h264.md`** — the pixfmt fix that made segments get produced at all,
  unmasking this segmentation bug.
