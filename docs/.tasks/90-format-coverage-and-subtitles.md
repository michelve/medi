# 90 — Format Coverage & Subtitles

> New cross-cutting phase, peer to `60-metadata-and-libraries.md` and
> `70-audio-quality-and-profiles.md`. Depends on `01-db-schema.md` (`media_files`),
> `02-api-contract.md` (`/api/stream`), `20-phase2-hwa-transcode.md` (the video decision
> table + `command.rs` builder), and `70` (the audio axis + capability profiles).
>
> **Gap this closes:** `medi` today handles a **narrower** set of formats than a
> Plex/Jellyfin-class server, and three of the gaps make real-library files misplay or
> silently degrade:
>
> 1. **Video codecs.** Only `H264 / Hevc / Av1` are typed (`core/src/profile.rs::VideoCodec`).
>    Every other codec — **VC-1, MPEG-2, MPEG-4 (DivX/Xvid), VP9** — collapses to
>    `VideoCodec::Other` in `db/src/models.rs::profile()`, which no client lists as
>    supported, so it never direct-plays **and** has no described transcode source path.
>    These are common in DVD/Blu-ray rips and older libraries.
> 2. **Dolby Vision Profile 7.** `ingest` already detects and persists P7 (`dv_profile = 7`),
>    but `decision.rs` has **no P7 branch**: `transcode_reason` only names P5 and treats
>    everything else as `dv_p8_sdr_display`, and no code accounts for P7's dual-layer
>    (BL(HDR10)+EL) structure. A P7 file therefore gets the wrong reason slug and, worse, no
>    defined handling of its enhancement layer.
> 3. **Subtitles.** Not handled **at all** — no detection, no persistence, no passthrough,
>    no burn-in, no external-sidecar discovery. A file's subtitle tracks are invisible to
>    the client.
>
> Audio (`70`) is otherwise solid but misses **MP3, Vorbis, WMA, ALAC** — MP3 especially is
> ubiquitous and currently maps to `Other`, forcing a needless re-encode.
>
> This task widens the codec/container/audio surface, adds the DV P7 path, and introduces a
> complete subtitle subsystem — so `medi` direct-plays or transcodes essentially any file a
> real library holds.

## Purpose

Bring `medi`'s format coverage up to the level users expect from a media server:

1. **Codec & container widening** — recognize VC-1 / MPEG-2 / MPEG-4 / VP9 video and
   MP3 / Vorbis / WMA / ALAC audio as first-class (typed, not `Other`), ingest the
   remaining mainstream container extensions, and route every unsupported codec to a
   correct **transcode → H.264** decision (HW decode where the host offers it, software
   otherwise — reusing the AV1/dav1d fallback pattern).
2. **Dolby Vision Profile 7** — an explicit, correct decision branch: drop the
   enhancement layer, treat the HDR10 base layer as HDR10, and direct-play-as-HDR10 on an
   HDR display or tone-map to SDR — **without** the OpenCL/CUDA IPTPQc2 path P5 needs
   (P7's base layer is standard HDR10, so VPP/CUDA HDR10 tone-mapping suffices).
3. **Subtitles (full support)** — probe and persist every embedded subtitle track,
   discover external sidecars, serve **text** subtitles as WebVTT for direct-play, and
   **burn in image** subtitles (PGS / VobSub) via a forced video transcode.

**Scope (agreed).** Mainstream gaps only — **not** RealMedia (rm/rmvb), raw disc images
(.iso / VIDEO_TS), or AC-4. Those are a possible later task; the enum's `Other` catch-all
keeps them from crashing ingest in the meantime.

## Requirements

- Every codec `jellyfin-ffmpeg` can decode yields a **valid decision**: direct-play when a
  client lists it, else transcode → H.264. No source codec silently fails.
- The new video codecs are **not** added to any client's `video_codecs` list (Apple TV /
  Shield / generic Android TV don't natively decode VC-1/MPEG-2/MPEG-4, and VP9 is
  container-gated on TV): they always transcode. VP9 is the one exception where HW
  **decode** may be available on the host — reuse the per-codec HW-decode capability check.
- **Dolby Vision P7** never direct-plays (no TV client presents dual-layer DV); it is
  decoded to its HDR10 base layer and then handled exactly like an HDR10 source vs the
  display. The OpenCL/CUDA DV tone-map path stays reserved for **P5** (IPTPQc2 only).
- **Every** subtitle track (embedded + external sidecar) is probed and persisted; a file
  is 1:N in subtitles, so they live in a **child table**, not columns on `media_files`
  (same discipline as `audio_streams`).
- **Text** subtitles (SRT / ASS / SSA / mov_text / WebVTT) are served as **WebVTT** — the
  one format both react-native-video backends (AVPlayer, ExoPlayer) consume via
  `textTracks` — so a direct-played file still shows subtitles without a video transcode.
- **Image** subtitles (HDMV PGS / VobSub) cannot become text: selecting one triggers a
  **burn-in** transcode (overlay filter + forced video re-encode).
- **Backward compatible:** a file with no subtitle tracks, an H.264/AAC body, and a known
  codec/container decides **exactly as it does today** — no regression to the `70` audio
  decision, no needless remux.
- No auth; LAN-only; converted/cached subtitles are written under `/config`, never `/media`
  (`00-architecture.md`).

## Packages / crates

No new crates. Types are **added to `medi-core`** (`core/src/profile.rs` already hosts
`VideoCodec` / `AudioCodec` / `HdrType` / `DvProfile`) so `ingest`, `transcode`, and `api`
share one definition. Touches `core`, `db` (V5 migration + write/read helpers), `ingest`
(ffprobe subtitle parse + sidecar discovery + codec normalize), `transcode` (DV P7 branch,
per-codec HW-decode check, subtitle burn-in), `api` (subtitle route + stream params), and
`client/packages/{api-client,player}`. Existing workspace deps suffice (`serde`,
`rusqlite`, `refinery`, `tokio`, `tower-http`).

> **Numbering note.** On disk only `V1__init.sql` exists; `60-metadata-and-libraries.md`
> reserves `V2__metadata.sql` / `V3__libraries.sql`, and `70-audio-quality-and-profiles.md`
> uses `V4__audio_streams.sql`. refinery versions are globally sequential and single-valued,
> so **this task's migration is `V5__subtitle_streams.sql`**. Whichever of `60`/`70`/`90`
> ships later must keep the versions gapless and monotonic — the ordering is refinery's
> constraint; the numbers themselves are not load-bearing.

## File structure (where to save)

```
backend/
├── migrations/
│   └── V5__subtitle_streams.sql            # NEW: one row per subtitle track (embedded or sidecar)
└── crates/
    ├── core/src/profile.rs                 # +VideoCodec{Vc1,Mpeg2,Mpeg4,Vp9}, +AudioCodec{Mp3,Vorbis,Wma,Alac},
    │                                        #  VideoCodec::from_ffprobe, SubtitleFormat
    ├── db/src/{writes,queries,models}.rs    # SubtitleStreamWrite / replace_subtitle_streams /
    │                                        #  get_subtitle_streams / SubtitleStream read model
    ├── ingest/src/ffprobe.rs               # parse subtitle streams; widen video/audio codec normalize
    ├── ingest/src/scanner.rs               # + container extensions; external sidecar discovery
    ├── transcode/src/{decision,command}.rs # DV P7 branch, can_hw_decode(codec), SubtitlePlan::BurnIn
    └── api/src/routes.rs                   # GET /api/subtitles/:file_id/:index.vtt; stream sub params
```

Client (lockstep, type-sync owned by `40-phase4-tv-client-ui.md`):
`client/packages/api-client/src/{types,client}.ts`, `client/packages/player/`.

## 1 — Video codec widening (`core/src/profile.rs`, `db/src/models.rs`)

Extend the enum and centralize the ffprobe-name mapping so it is not duplicated between
`ingest` and `db`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    H264, Hevc, Av1,
    Vc1, Mpeg2, Mpeg4, Vp9,   // NEW — recognized sources, always transcoded to H.264
    Other,                    // genuinely unknown (RealMedia, etc.) — still yields a transcode
}

impl VideoCodec {
    /// Map an ffprobe `codec_name` to a typed codec. The single source of truth — used by
    /// both `ingest` (normalize before persist) and `db::MediaFile::profile()` (read back).
    pub fn from_ffprobe(name: &str) -> Self {
        match name {
            "h264" => VideoCodec::H264,
            "hevc" => VideoCodec::Hevc,
            "av1" => VideoCodec::Av1,
            "vc1" => VideoCodec::Vc1,
            "mpeg2video" => VideoCodec::Mpeg2,
            "mpeg4" | "msmpeg4v2" | "msmpeg4v3" => VideoCodec::Mpeg4, // DivX/Xvid + MS variants
            "vp9" => VideoCodec::Vp9,
            _ => VideoCodec::Other,
        }
    }
}
```

- `db/src/models.rs::profile()` (currently the `match self.video_codec.as_deref()` at
  ~line 188) becomes `VideoCodec::from_ffprobe(self.video_codec.as_deref().unwrap_or(""))`.
- **Direct-play policy is unchanged and correct**: no client lists these codecs in
  `video_codecs`, so `decide()`'s `client.supports_video(codec)` is false → transcode. The
  `command.rs` encoder table already targets **H.264** for every non-H.264 source, so no
  encoder-side change is needed. Decode is jellyfin-ffmpeg's job.

### Per-codec HW-decode capability (generalize the AV1 check)

`decision.rs::transcode_target` currently special-cases AV1:
`if profile.codec == Av1 && !caps.hwaccels.iter().any(|h| h == "av1") { software_decode = true; }`.
Generalize it to `caps.can_hw_decode(codec)` (in `caps.rs`) so VP9 / VC-1 / MPEG-2 follow
the same rule: HW decode when the host advertises the hwaccel (e.g. Intel QSV decodes
MPEG-2 / VC-1 / VP9; NVDEC decodes VP9), else software decode. `can_hw_decode` maps the
codec to its `-hwaccels` / decoder token and checks `caps`. This keeps the existing AV1 →
dav1d behavior as one arm of a general rule.

## 2 — Container widening (`ingest/src/scanner.rs`, `decision.rs`)

- `VIDEO_EXTENSIONS` (currently `mkv, mp4, m4v, mov, avi, ts, m2ts, webm, wmv, mpg, mpeg`)
  gains: **`flv, ogv, ogm, 3gp, 3g2, vob, mk3d`**. (No RealMedia/`.iso` — out of scope.)
- Client container lists in `decision.rs`: add **`ts`** to `nvidia_shield()` (ExoPlayer
  opens MPEG-TS directly). No other logic change — `ClientProfile::supports_container`
  already returns false for anything not listed, which correctly forces a **remux** (over
  `/api/direct` / copy-video HLS) for an unlistable container even when the video codec is
  fine. That is the existing, tested behavior (`mkv_container_forces_remux_even_when_codec_ok`).

## 3 — Dolby Vision Profile 7 (`decision.rs`)

P7 is dual-layer **BL(HDR10) + EL** (FEL/MEL), common in 4K Blu-ray remux MKVs. **No TV
client presents P7** (Apple TV does P5/P8, Android TV varies but not P7). So P7 always
transcodes; the base layer is standard **HDR10**, which means:

- **HDR display** → decode P7, drop the EL, present the **HDR10 base layer** — treat like
  an HDR10 source that needs no tone-map. (Still a transcode because the P7 stream itself
  isn't directly playable, but the target keeps HDR10.)
- **SDR display** → tone-map the HDR10 base layer to SDR via the **normal HDR10 path**
  (Intel VPP / NVIDIA CUDA / software zscale) — **not** the OpenCL/CUDA IPTPQc2 path, which
  is P5-only. `dv_tone_map` (the flag that pins OpenCL) stays `false` for P7.

Changes:

- In `needs_tonemap` / the DV arm: a P7 source on an HDR display needs no tone-map (present
  BL as HDR10) but **still forces a transcode** because the container/codec-level DV stream
  isn't directly presentable — add an explicit "P7 always transcodes" gate (mirrors "P5
  always transcodes on SDR", but P7 also transcodes on HDR to strip the EL).
- `transcode_target`: `dv_tone_map = tone_map && profile.hdr == DolbyVision && dv == P5`
  (restrict OpenCL to P5). P7 tone-map uses the HDR10 VPP/CUDA branch already in `command.rs`.
- `transcode_reason`: add `dv_p7_hdr10_display` (HDR display, EL dropped) and
  `dv_p7_sdr_display` (tone-mapped). Keep `dv_p5_sdr_display` / `dv_p8_sdr_display`.

## 4 — Audio codec widening (`core/src/profile.rs`, `ingest/src/ffprobe.rs`)

- `AudioCodec` gains `Mp3, Vorbis, Wma, Alac`. `is_lossless_bitstream()` is unchanged (none
  are lossless-bitstream; ALAC is lossless but **decoded**, never HDMI-bitstreamed).
- `normalize_audio_codec` (`ffprobe.rs`) maps: `mp3` / `mp2` → `mp3`; `vorbis` → `vorbis`;
  `wmav2` / `wmapro` / `wmavoice` → `wma`; `alac` → `alac`.
- Client profiles (`decision.rs`): add **`Mp3`** to `apple_tv_4k()`, `nvidia_shield()`, and
  `generic_android_tv()` (universally decodable). Leave `Vorbis` / `Wma` / `Alac` **out** of
  the default decodable sets → they transcode to E-AC-3/AAC via the existing `audio_plan`
  fallback (Apple/Android don't reliably decode them), but the enum lets a detected
  `AudioCapabilities` payload add them where a device does.

## 5 — Subtitle subsystem (the large new piece)

Mirrors the `70` audio pattern end-to-end: child table → probe widening → a plan enum →
command args → client wiring.

### DB migration — `V5__subtitle_streams.sql`

A **child table keyed by `media_file_id`** (1:N — a file has commentary/foreign/forced
tracks + sidecars), same normalization discipline as `audio_streams`. `media_files` stays
the 1:1 home for the single primary video stream — **no subtitle columns there**.

```sql
-- V5__subtitle_streams.sql
-- One row per subtitle track of a media file (embedded track OR external sidecar file).
CREATE TABLE subtitle_streams (
    id             INTEGER PRIMARY KEY,
    media_file_id  INTEGER NOT NULL REFERENCES media_files(id) ON DELETE CASCADE,
    stream_index   INTEGER,                 -- ffprobe stream index for embedded; NULL for external
    codec          TEXT,                    -- subrip, ass, ssa, mov_text, webvtt, hdmv_pgs_subtitle, dvd_subtitle
    format         TEXT NOT NULL,           -- 'text' | 'image'  (drives passthrough-vtt vs burn-in)
    language       TEXT,                    -- ISO-639-2, e.g. "eng"
    title          TEXT,                    -- stream tag title
    is_default     INTEGER NOT NULL DEFAULT 0,   -- ffprobe DISPOSITION:default
    is_forced      INTEGER NOT NULL DEFAULT 0,   -- ffprobe DISPOSITION:forced (or ".forced." sidecar)
    is_external    INTEGER NOT NULL DEFAULT 0,   -- 0 embedded, 1 sidecar file
    external_path  TEXT,                    -- absolute path under /media for a sidecar; NULL if embedded
    UNIQUE(media_file_id, stream_index, external_path)
);
CREATE INDEX idx_subtitle_streams_file ON subtitle_streams(media_file_id);
```

Additive DDL only (no PRAGMAs — see `migrations/README.md`); idempotent via refinery
version records. Existing `media_files` rows have no `subtitle_streams` children until
re-probed; the `scan_state`-driven re-probe path repopulates them.

`db` write / query additions:

- `writes.rs`: `SubtitleStreamWrite { stream_index, codec, format, language, title,
  is_default, is_forced, is_external, external_path }` + `replace_subtitle_streams(conn,
  media_file_id, &[..])` — delete-then-insert **inside the same file transaction** as
  `upsert_media_file` / `replace_audio_streams` (the overwrite-in-place contract from `10`).
- `queries.rs`: `get_subtitle_streams(conn, media_file_id) -> Vec<SubtitleStream>`; join
  into the `MediaFile` aggregate returned by `get_movie` / `get_series` details.
- `models.rs`: `SubtitleStream { .. }` read model + `subtitle_streams: Vec<SubtitleStream>`
  on the `MediaFile` read model (next to `audio_streams`), so the client renders a picker.

### Probe — `ingest/src/ffprobe.rs`

The existing single invocation already emits every stream. Widen `map_output` to also
collect `codec_type == "subtitle"` streams (alongside the audio pass added by `70`) and
classify text-vs-image + read the forced disposition:

```rust
let subtitle_streams: Vec<SubtitleStreamWrite> = out.streams.iter().enumerate()
    .filter(|(_, s)| s.codec_type.as_deref() == Some("subtitle"))
    .map(|(idx, s)| SubtitleStreamWrite {
        stream_index: Some(idx as i64),
        codec: s.codec_name.clone(),
        format: subtitle_format(s.codec_name.as_deref()).to_string(), // "text" | "image"
        language: s.tags.as_ref().and_then(|t| t.language.clone()),
        title: s.tags.as_ref().and_then(|t| t.title.clone()),
        is_default: s.disposition.as_ref().map(|d| d.default == 1).unwrap_or(false),
        is_forced:  s.disposition.as_ref().map(|d| d.forced  == 1).unwrap_or(false),
        is_external: false,
        external_path: None,
    }).collect();
```

`subtitle_format`: `subrip | ass | ssa | mov_text | webvtt | text` → `"text"`;
`hdmv_pgs_subtitle | dvd_subtitle | dvb_subtitle | xsub` → `"image"`; unknown → `"text"`
(safe: text passthrough is cheap and non-destructive vs a forced burn-in). Add a `forced`
field to the `Disposition` serde shape (`ffprobe.rs`).

`probe()` return becomes `(MediaFileWrite, Vec<AudioStreamWrite>, Vec<SubtitleStreamWrite>)`;
the worker persists all three in the file's single write transaction.

### External sidecar discovery — `ingest/src/scanner.rs`

When a video file is discovered, look for sibling subtitle files in the same directory
whose stem matches the video's stem (with an optional `.<lang>` and `.forced` suffix):

- Extensions: `.srt, .ass, .ssa, .vtt, .sub` (VobSub `.sub` pairs with a `.idx`).
- Naming: `Movie (2020).srt`, `Movie (2020).en.srt`, `Movie (2020).en.forced.srt` →
  parse the trailing `.<lang>` into `language`, and a `.forced` token into `is_forced`.
- Emit each as a `SubtitleStreamWrite { is_external: true, external_path: Some(abs), codec:
  <by ext>, format: <text for srt/ass/ssa/vtt, image for sub/idx> }` attached to that file.
- Sidecars live under `/media` (read-only) — **never written**. Discovery is filename-only
  (no ffprobe pass on the sidecar); a follow-up probe of the sidecar is optional.

### Serving text subtitles — `GET /api/subtitles/:file_id/:index.vtt` (`api/src/routes.rs`)

New route returning `text/vtt`. `:index` selects a row of the file's `subtitle_streams`
(embedded `stream_index`, or a synthetic index for externals):

- **Embedded text** → extract + convert with jellyfin-ffmpeg:
  `ffmpeg -i <src> -map 0:s:<n> -c:s webvtt -f webvtt <cache>` (SRT / ASS / SSA / mov_text
  all convert to WebVTT). Cache the result at `/config/subs/<file_id>.<index>.vtt` and serve
  it (subsequent requests hit the cache). Text extraction is CPU-trivial — it does **not**
  count against the GPU transcode-session cap in `20`.
- **External `.vtt`** → serve directly. **External `.srt/.ass/.ssa`** → convert to WebVTT
  the same way, caching under `/config/subs/`.
- **Image subtitles** are **not** served here (they can't become text) — the client
  requests burn-in instead (below). Return `409`/`415` if `:index` names an image track.

The `MediaFile` detail (`/api/movies/:id`, `/api/series/:id`) now carries
`subtitle_streams` so the client lists tracks; for a **direct-play** file the client
attaches the chosen text track as a react-native-video `textTracks` sidecar pointing at
`/api/subtitles/:file_id/:index.vtt`.

### Image-subtitle burn-in — `transcode` (`decision.rs`, `command.rs`)

Image subs (PGS / VobSub) must be **burned into the video** — which forces a video
transcode even when the video would otherwise direct-play:

```rust
pub enum SubtitlePlan {
    None,                                   // no subtitle, or client renders a text sidecar itself
    BurnIn { stream_index: i64 },           // image sub → overlay onto the video (forces transcode)
}
```

- `decide()` (or a thin wrapper the `api` layer calls) takes an optional selected-subtitle
  index; if that track's `format == "image"`, it returns `SubtitlePlan::BurnIn` and the
  decision is forced to `Transcode` (a text sub never forces this — it rides as a sidecar).
- `command.rs`: when burning in, add the subtitle overlay to the **video filter graph**:
  `-filter_complex "[0:v]<existing tone-map/scale>[base];[base][0:s:<n>]overlay[v]"`
  with `-map "[v]"`. **Ordering matters**: the overlay must run **after** any HDR→SDR
  tone-map so the burned subtitle isn't tone-mapped/washed out. Burn-in is a full re-encode
  and **counts against the GPU transcode cap** in `20` (unlike the audio-only remux, which
  does not).

### Stream params (`api/src/routes.rs`)

Extend `StreamQuery` (keeping every `70` param) with:

```
GET /api/stream/:file_id
  ...existing video + audio params (70)...
  &sub=<index>            # selected subtitle track (embedded stream_index or external synthetic id)
  &sub_burn=0|1           # 1 ⇒ burn the selected image sub in (client sends this only for image subs)
```

When `sub_burn=1` and the selected track is image-format, the handler forces a transcode
with `SubtitlePlan::BurnIn`. A text track is **not** sent as `sub_burn`; the client fetches
its `.vtt` sidecar and the video can still direct-play.

## Combined decision matrix (extends `20` video + `70` audio)

| Source axis | Client / display | Decision |
|---|---|---|
| **VC-1 / MPEG-2 / MPEG-4 (DivX/Xvid)** | any | **transcode → H.264** (HW decode where host offers it, else software) |
| **VP9** | any | **transcode → H.264**; HW decode if `can_hw_decode(vp9)`, else software |
| **DV P7** (BL+EL) → **HDR** display | any | **transcode**, drop EL, keep **HDR10** base (no tone-map) — `dv_p7_hdr10_display` |
| **DV P7** → **SDR** display | any | **transcode**, tone-map HDR10 base → SDR via **VPP/CUDA** (not OpenCL) — `dv_p7_sdr_display` |
| **MP3** audio | any | **copy** (universally decodable) |
| **Vorbis / WMA / ALAC** audio | Apple/Android default | **transcode → E-AC-3 / AAC** (enum lets a device opt in) |
| **Text sub** (SRT/ASS/mov_text/vtt), direct-play video | any | serve **WebVTT** sidecar via `/api/subtitles`; video still direct-plays |
| **Image sub** (PGS / VobSub), selected | any | **burn-in** → forces a video transcode (overlay after tone-map) |

## Sub-tasks

1. **`core`**: extend `VideoCodec` (`Vc1/Mpeg2/Mpeg4/Vp9`) + `VideoCodec::from_ffprobe`;
   extend `AudioCodec` (`Mp3/Vorbis/Wma/Alac`); add a `SubtitleFormat`/`text|image` helper.
2. **`db`**: `V5__subtitle_streams.sql` + `SubtitleStreamWrite` / `replace_subtitle_streams`
   + `get_subtitle_streams` + `SubtitleStream` read model + join into details.
3. **`ingest/ffprobe.rs`**: parse subtitle streams (text/image classify, forced disposition);
   route `video_codec` through `from_ffprobe`; widen `normalize_audio_codec`; `probe()`
   returns the third vector; worker persists all three in the file transaction.
4. **`ingest/scanner.rs`**: add container extensions; external sidecar discovery
   (stem match + `.<lang>` / `.forced` parsing).
5. **`transcode/decision.rs`**: add the DV **P7** branch (always transcode; OpenCL only for
   P5); generalize the AV1 HW-decode check to `can_hw_decode(codec)`; add `SubtitlePlan`.
6. **`transcode/command.rs`**: image-sub overlay in the filter graph **after** tone-map;
   `-filter_complex` + `-map "[v]"` for burn-in.
7. **`api`**: `GET /api/subtitles/:file_id/:index.vtt` (extract/convert + `/config/subs`
   cache); `sub` / `sub_burn` stream params; carry `subtitle_streams` in details.
8. **`client`**: `SubtitleStream` type + `subtitle_streams` on `MediaFile`; wire
   react-native-video `textTracks` for text subs, `sub_burn` request for image subs, and a
   subtitle-track selector in the player overlay.
9. **Cross-ref edits** to `01` / `02` / `10` / `20` / `60` / `70` (below).

## Scaling notes

- **No new ffprobe passes** — the single existing invocation already returns subtitle
  streams; only the parser widens. No new ingest semaphore pressure.
- **Text-subtitle extraction is CPU-trivial** (a stream copy/convert, no video decode) and
  is cached under `/config/subs/` — it must **not** count against the GPU transcode cap.
- **Image-subtitle burn-in is a full video re-encode** and **does** count against the GPU
  cap (`20`) — it is the one subtitle path that engages the transcoder.
- Preserve the existing bias: `DirectPlay { remux }` over `Transcode` whenever the video can
  be copied; a **text** subtitle never demotes a direct-play, only an image burn-in does.

## Verification

> Note: this dev machine has no Rust toolchain (see the `medi-no-rust-toolchain` note), so
> the crate tests below run on a machine with Rust installed.

- **Scanner**: a `.flv` / `.vob` file is discovered and ingested; a `Movie (2020).en.forced.srt`
  sibling yields an external subtitle row `language=eng, is_forced=1, is_external=1`.
- **Probe** (`cargo test -p medi-ingest`): fixture JSON with a `vc1` video → `VideoCodec::Vc1`;
  `mpeg2video` → `Mpeg2`; `mp3` audio → `codec=mp3`; a `hdmv_pgs_subtitle` stream →
  `format=image`; a `subrip` stream → `format=text` with `is_forced` read from disposition.
- **Decision** (`cargo test -p medi-transcode`): MPEG-2 / VC-1 / VP9 → transcode-to-H.264;
  a VP9 host with a VP9 hwaccel uses HW decode, without it software; DV **P7** on an HDR
  display → transcode, `dv_p7_hdr10_display`, `dv_tone_map == false`; DV P7 on SDR →
  `dv_p7_sdr_display` via the VPP/CUDA path (**not** the P5 OpenCL path); an image sub
  selected → `SubtitlePlan::BurnIn` forces `Transcode`; a text sub does not.
- **Command** (`cargo test -p medi-transcode`): burn-in emits `-filter_complex … overlay`
  with the overlay **after** the tone-map filter and `-map "[v]"`.
- **API** (`curl`): `/api/subtitles/<file>/2.vtt` on a file with an embedded SRT returns
  valid WebVTT (`WEBVTT` header); `/api/stream/<mpeg2_file>` → `mode:"hls"`;
  `/api/stream/<file>?sub=3&sub_burn=1` on a PGS track → `mode:"hls"`, burn-in reason.
- **Backward compatibility**: a file with no subtitle streams, an H.264/AAC body, and a
  known container still returns `mode:"direct"` exactly as today (no `70` regression).

## Cross-references (edits required in lockstep)

- **`01-db-schema.md`** — note under `media_files` that subtitles live in the child
  `subtitle_streams` table introduced by `V5` (task 90), with the 1:N rationale (embedded +
  external sidecars). Do **not** add subtitle columns to `media_files`.
- **`02-api-contract.md`** — add the `GET /api/subtitles/:file_id/:index.vtt` row; extend the
  `/api/stream/:file_id` client-hints text with `sub` / `sub_burn`; note the `MediaFile`
  detail now carries `subtitle_streams` alongside `audio_streams`.
- **`10-phase1-foundation-data.md`** — note ffprobe now also parses subtitle streams and the
  widened video/audio codec set (the invocation is unchanged; the parser widens).
- **`20-phase2-hwa-transcode.md`** — add decision-table rows for VC-1 / MPEG-2 / MPEG-4 / VP9
  (→ transcode to H.264, per-codec HW-decode fallback) and the **DV P7** rows (HDR display:
  drop EL, keep HDR10; SDR: tone-map via VPP/CUDA, **not** OpenCL — OpenCL stays P5-only).
- **`60-metadata-and-libraries.md`** — one line: `60` reserves V2/V3, `70` uses V4, and task
  **90 uses V5**; whichever ships later keeps refinery versions gapless (see §Numbering).
- **`70-audio-quality-and-profiles.md`** — one line: `AudioCodec` gains `Mp3` (decodable
  default on all clients) and `Vorbis/Wma/Alac` (transcode by default); the audio decision is
  otherwise unchanged.
