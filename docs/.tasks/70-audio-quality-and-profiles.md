# 70 — Audio Handling, Quality Profiles & Client Capability Negotiation

> New cross-cutting phase, peer to `60-metadata-and-libraries.md`. Depends on
> `01-db-schema.md` (`media_files`), `02-api-contract.md` (`/api/stream`), and
> `20-phase2-hwa-transcode.md` (the video-only decision table + `command.rs` builder).
> **Gap this closes:** the `transcode` crate already ships an `AudioCodec` enum, a
> `ClientProfile` carrying `audio_codecs`/`containers`, a combined `decide()` that emits
> `Decision::DirectPlay { remux }` vs `Decision::Transcode`, and an `audio_target()` that
> maps DTS/TrueHD → E-AC-3 → AAC — **but the audio half of that decision is fed a lie.**
> `crates/api/src/routes.rs` hard-codes `let audio = AudioCodec::Aac;` (a standing TODO)
> because **no audio metadata is ever probed or persisted**. On top of that, `AudioCodec`
> has no notion of **channel count**, **channel layout**, or **lossless-vs-lossy**, so the
> server cannot downmix 7.1 → 5.1 or tell *"NVIDIA Shield bitstreams TrueHD"* from
> *"Apple TV must re-encode it"*; the client sends only three booleans (`hdr`/`dv`/`sdr`),
> never its detected audio-sink capabilities or a bitrate cap; and there is no
> "best available quality" / quality-profile concept at all. This task makes the audio half
> of the playback decision **real** and adds per-device capability negotiation.

## Purpose

Give `medi` the two audio capabilities every Plex/Jellyfin-style server has and this one
does not yet, mirroring Jellyfin's `DeviceProfile` model (DirectPlayProfiles +
TranscodingProfiles + CodecProfiles, with audio conditions and a `MaxAudioChannels` ceiling)
but scoped to `medi`'s two clients:

1. **Audio metadata + audio-aware playback decisions** — probe and persist *every* audio
   track's codec, channels, layout, bitrate, language, and immersive markers (Dolby Atmos /
   DTS:X); feed the **default track's** real descriptor into the existing `decide()` so the
   common *"video direct-plays, audio must be remuxed / downmixed / re-encoded"* case is
   expressed correctly, and per-device passthrough is honored.
2. **Client capability negotiation + quality profiles** — let the TV send a detected
   capability payload (ExoPlayer/media3 `AudioCapabilities` on Android; a fixed profile on
   Apple TV), fall back to per-platform static defaults when it doesn't, and cap the
   streaming bitrate via a `QualityProfile`.

**The device asymmetry that makes this necessary.** NVIDIA Shield bitstreams **TrueHD /
DTS:X / DTS-HD MA** losslessly over HDMI. Apple TV 4K **never** bitstreams TrueHD or DTS:X —
it decodes internally to LPCM / Dolby MAT, and even tvOS 26 "passthrough" carries only
**lossy** Atmos (E-AC-3 JOC), never the lossless formats. Generic Android TV / Sony varies by
the attached AVR and must self-detect. "Best available quality" playback therefore *requires*
a per-device capability profile plus a combined video × audio decision — a single global
audio target (today's hard-coded AAC) is wrong on every device.

## Requirements

- Every audio track is probed and persisted; a file may have **multiple** audio tracks
  (director's commentary, foreign dub, a lossless + lossy pair) — the model holds all of
  them, not just the first.
- The decision must support: **full direct-play**; **video direct + audio remux / downmix /
  re-encode** (the common case, already representable via `Decision::DirectPlay { remux }`
  and the copy-video HLS path); **full transcode** (per `20`).
- Per-device audio passthrough facts live in **capability profiles**, not hard-coded in
  `decide()`: **Shield** bitstreams TrueHD/DTS:X/DTS-HD MA losslessly; **Apple TV 4K** never
  bitstreams the lossless formats (decode → LPCM/Dolby MAT), passing only **lossy** Atmos
  (E-AC-3 JOC); **generic Android TV/Sony** varies and must self-detect.
- The client **SHOULD** send detected capabilities; a static per-platform default is the
  fallback when it doesn't (graceful degradation, mirroring `60`'s "no API key →
  filename-only" posture).
- **Backward compatible:** an un-probed row (audio unknown, no `audio_streams` children) must
  still yield a valid decision and **never force a needless remux** — exactly today's
  `AudioCodec::Aac` safety default, but now reached explicitly.
- No auth; LAN-only; the capability payload changes no security posture (`00-architecture.md`).

## Packages / crates

No new crates. The audio types are **promoted from `transcode` into `medi-core`** (as
`core/src/profile.rs` already hosts `VideoCodec`/`HdrType`/`DvProfile`) so `ingest`,
`transcode`, and `api` share one definition, then re-exported from `transcode` to avoid a
churny rename. Touches `core`, `db` (V4 migration + write/read helpers), `ingest` (ffprobe
audio parse), `transcode` (decision + command audio branches), `api` (stream params), and
`client/packages/{api-client,player}`. Existing workspace deps suffice (`serde`, `rusqlite`,
`refinery`, `tokio`).

> **Numbering note.** On disk only `V1__init.sql` exists, but `60-metadata-and-libraries.md`
> *reserves* `V2__metadata.sql` and `V3__libraries.sql` (not yet built). refinery versions are
> globally sequential and single-valued, so **this task's migration is `V4__audio_streams.sql`**.
> Whichever of `60`/`70` ships later must keep the version numbers gapless and monotonic —
> the ordering constraint is refinery's; the numbers themselves are not load-bearing.

## File structure (where to save)

```
backend/
├── migrations/
│   └── V4__audio_streams.sql        # NEW: one row per audio track of a media file
└── crates/
    ├── core/src/profile.rs          # +AudioCodec (moved+extended), ImmersiveAudio,
    │                                 #  ClientCapabilities, QualityProfile, Platform,
    │                                 #  ClientProfile field additions + platform defaults
    ├── db/src/{writes,queries,models}.rs   # AudioStreamWrite / replace_audio_streams /
    │                                        #  get_audio_streams / AudioStream read model
    ├── ingest/src/ffprobe.rs        # parse ALL audio streams; immersive/lossless classify
    ├── transcode/src/{decision,command}.rs # audio branch, channel cap, passthrough, QualityProfile
    └── api/src/routes.rs            # StreamQuery + client_profile() extension; drop the TODO
```

Client (lockstep, type-sync owned by `40-phase4-tv-client-ui.md`):
`client/packages/api-client/src/{types,client}.ts`, `client/packages/player/`.

## New shared types (`core/src/profile.rs`)

```rust
/// Audio codecs the pipeline recognizes. DTS is split into the lossy core (`Dts`) and the
/// lossless family (`DtsHd` = DTS-HD MA / High-Res) because only the split lets the
/// passthrough decision be correct on Shield.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioCodec {
    Aac, Ac3, Eac3,
    Dts, DtsHd,
    TrueHd, Flac, Opus, Pcm, Other,
    // Added by `90-format-coverage-and-subtitles.md`: Mp3 (decodable default on all
    // clients) plus Vorbis/Wma/Alac (transcode by default). The audio decision is
    // otherwise unchanged; ALAC is lossless but always decoded, never bitstreamed.
    // Mp3, Vorbis, Wma, Alac,
}

impl AudioCodec {
    /// Lossless bitstream formats that only a passthrough-capable sink plays as-is.
    /// Apple TV cannot bitstream these; NVIDIA Shield can.
    pub fn is_lossless_bitstream(self) -> bool {
        matches!(self, AudioCodec::TrueHd | AudioCodec::DtsHd)
    }
}

/// Immersive-audio marker parsed from the ffprobe stream `profile` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImmersiveAudio { None, DolbyAtmos, DtsX }

/// "Best available quality" control. Default is `Auto`; the client's default *setting* is
/// `Original` (see §QualityProfile).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QualityProfile { Original, Auto, Capped }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform { AppleTv, Shield, AndroidTv, Unknown }

/// Detected (or defaulted) capabilities the client sends to `/api/stream`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientCapabilities {
    pub platform: Platform,
    pub video_codecs: Vec<VideoCodec>,
    pub bit_depth_10: bool,
    pub hdr_display: bool,
    pub dolby_vision: bool,
    pub audio_codecs: Vec<AudioCodec>,   // decoded OR passthrough-capable
    pub atmos_passthrough: bool,         // ExoPlayer ENCODING_E_AC3_JOC present
    pub max_channels: u8,                // EXTRA_MAX_CHANNEL_COUNT; 2 if unknown
    pub containers: Vec<String>,
    pub quality: QualityProfile,
    pub max_bitrate: Option<u64>,        // bits/sec; None = uncapped
}
```

`ClientProfile` (the existing `decide()` input) gains `max_channels: u8` and
`atmos_passthrough: bool`, plus `impl From<ClientCapabilities> for ClientProfile`.

### Static per-platform capability defaults (the fallback)

Three constructors alongside the existing `apple_tv_4k()` / `sdr_baseline()`:

| Default | Video | Audio codecs | Atmos passthrough | Max ch | Containers |
|---|---|---|---|---|---|
| `apple_tv_4k()` | h264, hevc, av1; 10-bit; HDR+DV | aac, ac3, eac3 | **true** (lossy E-AC-3 JOC only) | 8 | mp4, mov, m4v, hls |
| `nvidia_shield()` | h264, hevc, av1; 10-bit; HDR, DV | aac, ac3, eac3, **dts, dtshd, truehd** | true | 8 | + **mkv** |
| `generic_android_tv()` | h264, hevc | aac, ac3, eac3 | **false** | **2** | mp4, mkv, hls |

> The Apple TV default is authoritative (fixed hardware) — client detection is optional
> there. The generic Android default is deliberately **pessimistic** (stereo, no
> passthrough) because capabilities vary by AVR; the client is **expected** to upgrade it via
> a detected `AudioCapabilities` payload.

## DB migration — `V4__audio_streams.sql`

A **child table keyed by `media_file_id`**, not additive columns on `media_files`. Rationale:
a file is 1:N in audio tracks (commentary, dubs, lossless+lossy), so flat columns could hold
only one track and would force a lossy "pick the first" choice at probe time, breaking
`selectedAudioTrack`. `media_files` stays the 1:1 home for the single primary **video**
stream; audio joins the same normalization discipline `01-db-schema.md` already uses for
`credits`/`seasons`/`episodes`. Each track gets a row id and a `stream_index` matching
ffprobe's ordering, which is exactly what react-native-video's `selectedAudioTrack` selects by.

```sql
-- V4__audio_streams.sql
-- One row per audio track of a media file. A file has 1..N audio tracks; media_files
-- remains the 1:1 home for the single primary VIDEO stream.
CREATE TABLE audio_streams (
    id             INTEGER PRIMARY KEY,
    media_file_id  INTEGER NOT NULL REFERENCES media_files(id) ON DELETE CASCADE,
    stream_index   INTEGER NOT NULL,        -- ffprobe stream index (selectedAudioTrack)
    codec          TEXT,                    -- aac,ac3,eac3,dts,dtshd,truehd,flac,opus,pcm
    profile        TEXT,                    -- raw ffprobe profile, e.g. "DTS-HD MA"
    channels       INTEGER,                 -- 2, 6, 8
    channel_layout TEXT,                    -- "stereo", "5.1", "7.1", "5.1(side)"
    bitrate        INTEGER,                 -- bits/sec, NULL if lossless / unknown
    sample_rate    INTEGER,                 -- Hz
    language       TEXT,                    -- ISO-639-2 tag, e.g. "eng"
    title          TEXT,                    -- stream tag title, e.g. "Commentary"
    immersive      TEXT NOT NULL DEFAULT 'none',  -- none | dolby_atmos | dts_x
    is_default     INTEGER NOT NULL DEFAULT 0,    -- ffprobe DISPOSITION:default
    UNIQUE(media_file_id, stream_index)
);
CREATE INDEX idx_audio_streams_file ON audio_streams(media_file_id);
```

Additive DDL only (no PRAGMAs — see `migrations/README.md`); idempotent via refinery version
records. Existing `media_files` rows simply have no `audio_streams` children until re-probed;
the existing `scan_state`-driven re-probe path repopulates them.

### `db` write / query additions

- `writes.rs`: `pub struct AudioStreamWrite { stream_index, codec, profile, channels,
  channel_layout, bitrate, sample_rate, language, title, immersive, is_default }` and
  `pub fn replace_audio_streams(conn, media_file_id, streams: &[AudioStreamWrite])` —
  delete-then-insert **inside the same transaction** as `upsert_media_file`, so a re-probe
  overwrites cleanly (mirrors the overwrite-in-place contract already in `upsert_media_file`).
- `queries.rs`: `pub fn get_audio_streams(conn, media_file_id) -> Vec<AudioStream>`; join into
  the `MediaFile` aggregate returned by `get_movie`/`get_series` details.
- `models.rs`: `pub struct AudioStream { … }` read model; add `audio_streams: Vec<AudioStream>`
  to the `MediaFile` read model so the client can render a track list.

## ffprobe audio parsing (`ingest/src/ffprobe.rs`)

The existing invocation already emits every stream (`-show_streams`). Widen `map_output` to
collect **all** `codec_type == "audio"` streams (today it only `find`s the single video
stream) and classify each:

```rust
let audio_streams: Vec<AudioStreamWrite> = out.streams.iter().enumerate()
    .filter(|(_, s)| s.codec_type.as_deref() == Some("audio"))
    .map(|(idx, s)| AudioStreamWrite {
        stream_index: idx as i64,
        codec: normalize_audio_codec(s.codec_name.as_deref(), s.profile.as_deref()),
        profile: s.profile.clone(),
        channels: s.channels,
        channel_layout: s.channel_layout.clone(),
        bitrate: s.bit_rate.as_deref().and_then(|b| b.parse().ok()),
        sample_rate: s.sample_rate.as_deref().and_then(|r| r.parse().ok()),
        language: s.tags.as_ref().and_then(|t| t.language.clone()),
        title: s.tags.as_ref().and_then(|t| t.title.clone()),
        immersive: classify_immersive(s.codec_name.as_deref(), s.profile.as_deref()),
        is_default: s.disposition.as_ref().map(|d| d.default == 1).unwrap_or(false),
    }).collect();
```

Classification rules (`normalize_audio_codec` / `classify_immersive`):

- `codec_name == "truehd"` → `TrueHd`; profile containing `"Atmos"` → `immersive =
  dolby_atmos`.
- `codec_name == "eac3"` + profile/tag mentioning `"Atmos"`/`"JOC"` → `immersive =
  dolby_atmos` (this is the **lossy** Atmos Apple TV *can* pass through).
- `codec_name == "dts"` + profile `"DTS-HD MA"` / `"DTS-HD High"` → `DtsHd`; profile
  `"DTS:X"` / `"DTS-X"` → `immersive = dts_x`.
- plain `codec_name == "dts"` core → `Dts`.

`probe()` returns `(MediaFileWrite, Vec<AudioStreamWrite>)`; the worker persists both in the
file's single write transaction (single-writer/bounded-fan-out pattern from `10`).

## `/api/stream` capability params (`api/src/routes.rs`)

Keep the endpoint a **GET** (cacheable, matches `02`); extend `StreamQuery`, keeping the
existing `hdr`/`dv`/`sdr` for back-compat:

```
GET /api/stream/:file_id
  ?platform=appletv|shield|androidtv          # selects the static default profile
  &hdr=0|1 &dv=0|1 &sdr=0|1                    # existing (video display)
  &max_channels=8                             # EXTRA_MAX_CHANNEL_COUNT
  &audio=eac3,ac3,aac,truehd,dtshd,eac3_joc   # ExoPlayer EXTRA_ENCODINGS; eac3_joc ⇒ Atmos
  &max_bitrate=20000000                       # MaxStreamingBitrate, bits/sec (0/absent = uncapped)
  &quality=original|auto|capped
```

`client_profile()` starts from the `platform` default (above), then **overlays** any
explicitly-sent params — so a Shield reporting `max_channels=6` (5.1 AVR) overrides the Shield
8-channel default. The handler then reads the file's `audio_streams`, picks the **default
track** (`is_default`, else `stream_index` 0), and feeds its real `(codec, channels,
immersive)` into `decide()` — **removing the `let audio = AudioCodec::Aac;` TODO**. A file
with no `audio_streams` children falls back to the current AAC-safe default (no needless
remux).

## Combined playback decision matrix (extends `20`'s video-only table)

The video axis is unchanged from `20`; **audio is the new axis**. `decide()` gains an audio
branch and a channel-cap check; `audio_target()` is upgraded (below).

| Source audio | Client | Audio decision |
|---|---|---|
| Supported codec, channels ≤ client cap | any | **copy** |
| AC-3 / E-AC-3, MKV container | Apple TV | **copy** (audio fine; container drives the remux) |
| **TrueHD / DTS-HD MA** | **Shield** | **copy (bitstream passthrough)** |
| **TrueHD / DTS-HD MA** | **Apple TV** | **transcode → E-AC-3 5.1** (AAC 2.0 if no E-AC-3) |
| **DTS core / DTS:X** | Apple TV | **transcode → E-AC-3** |
| Any codec, **channels > client `max_channels`** | any | **downmix** to cap (`-ac 6` / `-ac 2`) |
| **Dolby Atmos (E-AC-3 JOC)** | Apple TV / Shield (`atmos_passthrough`) | **copy** (lossy Atmos passes on both) |
| Unsupported codec (e.g. FLAC on Apple TV) | any | **transcode → AAC / E-AC-3** |
| — video needs full transcode (tone-map / unsupported / cap) | any | copy if supported, else transcode / downmix alongside |

The Shield ≠ Apple TV split on TrueHD/DTS-HD is the whole point: **Shield passthrough, Apple
TV re-encode.** Note the two decisions are independent — a video full-transcode does not force
an audio transcode, and vice versa (`DirectPlay { remux: true }` carries a video-copy + audio
fix without HLS re-encoding the video).

`audio_target()` is replaced with a channel-aware plan:

```rust
pub enum AudioPlan {
    Copy,                                           // passthrough (supported, channels ≤ cap)
    Transcode { codec: AudioCodec, channels: u8 },  // re-encode and/or downmix
}

pub fn audio_plan(track: &AudioStream, client: &ClientProfile) -> AudioPlan {
    let bitstreamable = !track.codec.is_lossless_bitstream()
        || client_can_bitstream(client, track.codec);      // Shield yes, Apple TV no
    let atmos_ok = track.immersive != ImmersiveAudio::DolbyAtmos || client.atmos_passthrough;
    let supported = client.supports_audio(track.codec) && bitstreamable && atmos_ok;
    let over_cap = track.channels > client.max_channels as i64;
    if supported && !over_cap { return AudioPlan::Copy; }
    let target_ch = track.channels.min(client.max_channels as i64) as u8;
    let codec = if client.supports_audio(AudioCodec::Eac3) { AudioCodec::Eac3 } else { AudioCodec::Aac };
    AudioPlan::Transcode { codec, channels: if codec == AudioCodec::Aac { 2 } else { target_ch } }
}
```

### QualityProfile interaction

- **`Original`** = the "best available quality" setting. It biases toward `Copy` /
  passthrough and **suppresses any bitrate-driven video re-encode**: a Shield with TrueHD
  support copies TrueHD (`-c:a copy`); on Apple TV, `Original` cannot conjure lossless
  passthrough (hardware can't) but picks the richest allowed target (E-AC-3/Atmos over AAC).
- **`Capped`** + `max_bitrate` sets the ceiling (Jellyfin's `MaxStreamingBitrate`): if source
  video bitrate exceeds `max_bitrate`, **force a transcode** even when the codec would
  direct-play, and pass `-maxrate`/`-bufsize` into `command.rs`.
- **`Auto`** (default) preserves today's behavior.

## ffmpeg audio argument sketches (`command.rs`)

Extend the existing `AudioTarget` (`Copy | Transcode(AudioCodec)`) to carry channels:
`Transcode { codec, channels }`. The current code hard-codes `-ac 2` / `-ac 6`; replace with
the resolved channel count from `AudioPlan`.

```
# Passthrough (Shield TrueHD, or any supported codec within the channel cap):
-c:a copy

# Downmix 7.1 → 5.1, keep E-AC-3:
-c:a eac3 -b:a 768k -ac 6

# TrueHD / DTS-HD → E-AC-3 5.1 (Apple TV, no lossless bitstream):
-c:a eac3 -b:a 640k -ac 6

# Anything → AAC stereo (least-common-denominator / generic Android default):
-c:a aac -b:a 256k -ac 2

# QualityProfile::Capped also constrains video:
-maxrate 20000000 -bufsize 40000000
```

**Multi-track scope.** For HLS output `medi` emits a single program. This task transcodes the
**selected / default** audio track only; the remaining tracks stay available for direct-play
`selectedAudioTrack`. Per-language HLS audio groups (`-map 0:v -map 0:a` + `#EXT-X-MEDIA`) are
**out of scope** here and left as a future add.

## Client changes (`api-client` + `player`)

- `api-client/src/types.ts`: extend `StreamHints` with `platform`, `maxChannels`, `audio`
  (`string[]`), `atmos`, `maxBitrate`, `quality`; add an `AudioStream` interface mirroring the
  read model; add `audio_streams: AudioStream[]` to `MediaFile`. (Hand-written types remain
  source of truth per `40`.)
- `api-client/src/client.ts`: `stream()` serializes the new hints into the query, reusing the
  existing `hdr`/`dv`/`sdr` param-building pattern.
- `player` (Phase 5 stub today): on **Android**, call ExoPlayer/media3
  `AudioCapabilities.getCapabilities()` (HDMI `ACTION_HDMI_AUDIO_PLUG` intent →
  `EXTRA_ENCODINGS`, incl. `ENCODING_E_AC3_JOC` = Atmos, and `EXTRA_MAX_CHANNEL_COUNT`), map
  it to the payload, and send it with the stream request; on **tvOS**, send the static
  `platform=appletv` default. Drive react-native-video `selectedAudioTrack` from the
  `audio_streams` list.

## Sub-tasks

1. **`core`**: add `AudioCodec` (moved from `transcode`, re-exported) with `Dts`/`DtsHd`/`Pcm`
   + `is_lossless_bitstream()`; add `ImmersiveAudio`, `QualityProfile`, `Platform`,
   `ClientCapabilities`; add `max_channels`/`atmos_passthrough` to `ClientProfile` +
   `From<ClientCapabilities>`; add the three platform capability defaults.
2. **`db`**: `V4__audio_streams.sql` + `AudioStreamWrite` / `replace_audio_streams` +
   `get_audio_streams` + `AudioStream` read model + join into details.
3. **`ingest`**: parse all audio streams in `map_output`; `normalize_audio_codec` /
   `classify_immersive`; `probe()` returns audio; worker persists in the file transaction.
4. **`transcode/decision.rs`**: feed the real default-track descriptor; add the channel-cap +
   passthrough logic; replace `audio_target` with `audio_plan`; add `QualityProfile` handling.
5. **`transcode/command.rs`**: `AudioTarget::Transcode { codec, channels }`; emit `-ac <n>`;
   emit `-maxrate`/`-bufsize` for `Capped`.
6. **`api`**: extend `StreamQuery`; build the profile from platform default + overlays; feed
   real audio into `decide`; **remove the `AudioCodec::Aac` TODO**.
7. **`client`**: api-client types + `stream()` params; player capability detection +
   `selectedAudioTrack`.
8. **Cross-ref edits** to `01` / `02` / `20` / `60` / `10` (below).

## Scaling notes

- Audio probing adds **no** ffprobe passes — the existing single invocation already returns
  audio streams; only the parser widens. No new semaphore pressure on ingest.
- **Audio-only transcode (video copied) is CPU-trivial** — an E-AC-3 downmix is far cheaper
  than a video transcode and should **not** count against the GPU transcode-session cap in
  `20`. Prefer the `DirectPlay { remux }` path (served over `/api/direct`, or a copy-video
  HLS) over a full HLS transcode wherever the video codec is presentable; give any
  audio-remux HLS session a separate, higher limit than the GPU cap.
- Preserve the existing bias: `DirectPlay { remux }` over `Transcode` whenever the video can
  be copied.

## Verification

> Note: this dev machine has no Rust toolchain (see the `medi-no-rust-toolchain` note), so the
> crate tests below run on a machine with Rust installed; they cannot be `cargo test`ed where
> this doc was authored.

- **Migration** (`cargo test -p medi-db`): a fresh DB applies V4 once; restart is a no-op
  (refinery version records); a file with 3 audio tracks yields 3 `audio_streams` rows with
  the correct `stream_index` values.
- **Probe** (`cargo test -p medi-ingest`): fixture JSON with TrueHD+Atmos →
  `codec=truehd, immersive=dolby_atmos`; DTS-HD MA → `codec=dtshd`; 7.1 → `channels=8,
  channel_layout="7.1"`; language / title tags captured.
- **Decision** (`cargo test -p medi-transcode`): one assertion per matrix row — Shield copies
  TrueHD; Apple TV transcodes TrueHD → E-AC-3 5.1; 7.1 over a `max_channels=6` cap downmixes;
  E-AC-3 JOC copies on both; FLAC → AAC on Apple TV; `QualityProfile::Original` copies where
  `Auto` would; `Capped` + a low `max_bitrate` forces a transcode.
- **Command** (`cargo test -p medi-transcode`): copy emits `-c:a copy`; downmix emits
  `-ac 6`; capped emits `-maxrate`.
- **API** (`curl`): `curl "/api/stream/<truehd_file>?platform=shield"` → `mode:"direct"`;
  `?platform=appletv` on the same file → an audio-transcode reason; `?platform=androidtv&max_channels=2`
  → downmix; `?quality=capped&max_bitrate=8000000` on a 40 Mbps file → `mode:"hls"`.
- **Backward compatibility**: a row with no `audio_streams` children still returns a valid
  decision and never forces a needless remux.

## Cross-references (edits required in lockstep)

- **`01-db-schema.md`** — note under `media_files` that audio lives in the child
  `audio_streams` table introduced by `V4` (task 70), with the 1:N rationale. Do **not** add
  audio columns to `media_files`.
- **`02-api-contract.md`** — update the `GET /api/stream/:file_id` row and the client-hints
  text to list the new params (`platform`, `max_channels`, `audio`, `atmos`, `max_bitrate`,
  `quality`); note the `MediaFile` detail now carries `audio_streams`.
- **`20-phase2-hwa-transcode.md`** — annotate its "Playback decision table" as **video-only**
  and point to `70` for the audio companion / combined matrix; note `decide()` now takes audio
  track data and `AudioTarget` carries a channel count.
- **`60-metadata-and-libraries.md`** — one line: `60` reserves migration versions V2/V3 and
  `70` uses **V4**; whichever ships later keeps refinery versions gapless (see §Numbering).
- **`10-phase1-foundation-data.md`** — note ffprobe now also parses audio streams (the
  invocation is unchanged; the parser widens).
