//! Direct-play vs transcode decision (`docs/.tasks/20` §Playback decision table),
//! specialized for the Apple TV / Android TV clients this server targets.
//!
//! `/api/stream/:file_id` calls [`decide`] with the source [`MediaProfile`] (from the
//! `media_files` row), the host [`HwCaps`], and a [`ClientProfile`] describing what the
//! client and its connected display can natively play. The result is either
//! [`Decision::DirectPlay`] (client fetches `/api/direct`, possibly a remux) or
//! [`Decision::Transcode`] carrying the chosen vendor path, tone-map need, and target
//! codecs that `command.rs` turns into a jellyfin-ffmpeg argv.
//!
//! ## Why the client profile matters for Apple TV
//!
//! Apple TV 4K (AVPlayer) natively decodes a lot: H.264 High, HEVC Main/Main10
//! including **HDR10, HLG, and Dolby Vision Profile 5 & 8**, and — on A15/A17 models —
//! AV1. So a 4K DV file should **direct-play to a DV-capable Apple TV on an HDR/DV
//! display**, not be tone-mapped; transcoding is only forced when the *display* is SDR,
//! the client can't decode the codec, or the audio needs conversion. Always
//! transcoding DV would waste the GPU and throw away the DV presentation the TV could
//! show. (This is the "Direct-play DV to capable ATV" policy chosen for Phase 2.)
//!
//! AVPlayer also can't open a Matroska (`mkv`) container or decode DTS/TrueHD audio, so
//! those force at least a **remux**/audio-transcode even when the video codec is fine —
//! handled by [`ClientProfile::supports_container`] / audio checks.

use serde::{Deserialize, Serialize};

use medi_core::{
    AudioCodec, ClientCapabilities, DvProfile, HdrType, ImmersiveAudio, MediaProfile, Platform,
    QualityProfile, VideoCodec,
};

use crate::caps::HwCaps;

/// The GPU vendor family whose HWA path a transcode uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Vendor {
    Intel,
    Nvidia,
    Amd,
}

/// The source audio track's decision-relevant descriptor: what `decide` / [`audio_plan`]
/// read (`docs/.tasks/70`). The `api` layer maps a `medi_db::models::AudioStream` (the
/// default track) into this so `transcode` stays free of a `db` dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioTrack {
    pub codec: AudioCodec,
    /// Source channel count (2, 6, 8). 0/unknown is treated as within any cap.
    pub channels: u8,
    pub immersive: ImmersiveAudio,
}

impl AudioTrack {
    /// The AAC-safe default for an un-probed file (`docs/.tasks/70` §Backward compat):
    /// a client-supported stereo track that never forces a needless remux.
    pub fn unknown_safe() -> Self {
        Self {
            codec: AudioCodec::Aac,
            channels: 2,
            immersive: ImmersiveAudio::None,
        }
    }
}

/// What the client (and its connected display) can natively play. Sent by the client
/// as hints on `GET /api/stream`; [`ClientProfile::apple_tv_4k`] is the built-in
/// baseline used when a request omits hints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientProfile {
    /// Video codecs the client can decode.
    pub video_codecs: Vec<VideoCodec>,
    /// Whether the client can decode 10-bit (Main 10 / High 10) video.
    pub bit_depth_10: bool,
    /// Whether the connected display is HDR-capable (any of HDR10/HLG/DV). An SDR
    /// display forces tone-mapping of HDR sources.
    pub hdr_display: bool,
    /// Whether the client+display can present Dolby Vision (Apple TV 4K on a DV TV).
    pub dolby_vision: bool,
    /// Audio codecs the client can decode / passthrough.
    pub audio_codecs: Vec<AudioCodec>,
    /// Max audio channels the sink accepts (`EXTRA_MAX_CHANNEL_COUNT`). A source with
    /// more channels is downmixed to this (`docs/.tasks/70`).
    pub max_channels: u8,
    /// Whether the sink passes lossy Atmos (E-AC-3 JOC) through (`ENCODING_E_AC3_JOC`).
    pub atmos_passthrough: bool,
    /// Containers the client can open directly (for the direct-play remux decision).
    pub containers: Vec<String>,
    /// Whether the client can itself remux a direct byte stream — i.e. cope when only the
    /// **container** or **audio codec** differs while the video is copyable (`/api/direct`
    /// serves the raw file untouched). Native players (Apple TV / Android TV / Shield) do this
    /// internally; a browser `<video>` element cannot, so a browser must instead be handed a
    /// server-side transcode. `false` promotes a would-be `DirectPlay { remux: true }` to a
    /// real transcode in [`decide`]. Defaults to `true` for the TV profiles.
    #[serde(default = "yes")]
    pub can_remux_direct: bool,
}

/// serde default for [`ClientProfile::can_remux_direct`] on payloads that predate the field.
fn yes() -> bool {
    true
}

impl ClientProfile {
    /// The Apple TV 4K baseline (used when a `/api/stream` request sends no hints).
    ///
    /// Reflects AVPlayer on tvOS: H.264 + HEVC (incl. 10-bit, HDR10, HLG, Dolby Vision
    /// P5/P8) and AV1 (A15/A17 models); AAC / AC-3 / E-AC-3 (Atmos) audio; MP4/MOV and
    /// fragmented-MP4 HLS containers. DTS/TrueHD are **not** listed — they force an
    /// audio transcode. Apple TV passes **lossy** E-AC-3 JOC Atmos through but never
    /// bitstreams the lossless formats (`docs/.tasks/70`).
    pub fn apple_tv_4k() -> Self {
        Self {
            video_codecs: vec![VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Av1],
            bit_depth_10: true,
            hdr_display: true,
            dolby_vision: true,
            // MP3 is universally decodable (`docs/.tasks/90` §4).
            audio_codecs: vec![
                AudioCodec::Aac,
                AudioCodec::Ac3,
                AudioCodec::Eac3,
                AudioCodec::Mp3,
            ],
            max_channels: 8,
            atmos_passthrough: true,
            containers: vec!["mp4".into(), "mov".into(), "m4v".into(), "hls".into()],
            can_remux_direct: true,
        }
    }

    /// The NVIDIA Shield default (`docs/.tasks/70`): everything Apple TV does **plus**
    /// the lossless bitstream formats (DTS, DTS-HD MA, TrueHD) and the MKV container —
    /// Shield bitstreams TrueHD/DTS:X/DTS-HD MA losslessly over HDMI.
    pub fn nvidia_shield() -> Self {
        Self {
            video_codecs: vec![VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Av1],
            bit_depth_10: true,
            hdr_display: true,
            dolby_vision: true,
            audio_codecs: vec![
                AudioCodec::Aac,
                AudioCodec::Ac3,
                AudioCodec::Eac3,
                AudioCodec::Dts,
                AudioCodec::DtsHd,
                AudioCodec::TrueHd,
                AudioCodec::Mp3,
            ],
            max_channels: 8,
            atmos_passthrough: true,
            // `ts` (MPEG-TS): ExoPlayer opens it directly (`docs/.tasks/90` §2).
            containers: vec![
                "mp4".into(),
                "mov".into(),
                "m4v".into(),
                "mkv".into(),
                "ts".into(),
                "hls".into(),
            ],
            can_remux_direct: true,
        }
    }

    /// The generic Android TV default (`docs/.tasks/70`): deliberately **pessimistic**
    /// (stereo, no passthrough) because capabilities vary by the attached AVR. The
    /// client is expected to upgrade it with a detected `AudioCapabilities` payload.
    pub fn generic_android_tv() -> Self {
        Self {
            video_codecs: vec![VideoCodec::H264, VideoCodec::Hevc],
            bit_depth_10: true,
            hdr_display: false,
            dolby_vision: false,
            audio_codecs: vec![
                AudioCodec::Aac,
                AudioCodec::Ac3,
                AudioCodec::Eac3,
                AudioCodec::Mp3,
            ],
            max_channels: 2,
            atmos_passthrough: false,
            containers: vec!["mp4".into(), "mkv".into(), "hls".into()],
            can_remux_direct: true,
        }
    }

    /// A conservative SDR-only profile: H.264 8-bit, AAC, on an SDR display. Useful as
    /// a "least common denominator" and for testing the tone-map / transcode paths.
    pub fn sdr_baseline() -> Self {
        Self {
            video_codecs: vec![VideoCodec::H264],
            bit_depth_10: false,
            hdr_display: false,
            dolby_vision: false,
            audio_codecs: vec![AudioCodec::Aac],
            max_channels: 2,
            atmos_passthrough: false,
            containers: vec!["mp4".into(), "hls".into()],
            can_remux_direct: true,
        }
    }

    /// A web-browser baseline (the SPA, `platform=web`). Deliberately conservative to what a
    /// desktop/mobile browser can reliably decode via `<video>` / hls.js: **H.264 only**
    /// (HEVC/AV1 support is patchy and licence-gated, so treat them as needing a transcode),
    /// 8-bit SDR, and **AAC / MP3 / Opus / FLAC** audio — **no AC-3 / E-AC-3 / DTS / TrueHD**
    /// (browsers can't decode those). Containers `mp4`/`m4v` direct-play; everything else
    /// transcodes to H.264+AAC HLS, which hls.js plays anywhere. This stops the server handing
    /// a browser a direct stream it can only render as a black screen.
    pub fn web() -> Self {
        Self {
            video_codecs: vec![VideoCodec::H264],
            bit_depth_10: false,
            hdr_display: false,
            dolby_vision: false,
            audio_codecs: vec![
                AudioCodec::Aac,
                AudioCodec::Mp3,
                AudioCodec::Opus,
                AudioCodec::Flac,
            ],
            max_channels: 2,
            atmos_passthrough: false,
            containers: vec!["mp4".into(), "m4v".into(), "hls".into()],
            // A browser <video> can't remux a raw /api/direct stream itself — a container or
            // audio-codec mismatch must be fixed by a server transcode.
            can_remux_direct: false,
        }
    }

    /// Select the static per-platform default (`docs/.tasks/70` §capability defaults).
    pub fn for_platform(platform: Platform) -> Self {
        match platform {
            Platform::AppleTv => Self::apple_tv_4k(),
            Platform::Shield => Self::nvidia_shield(),
            Platform::AndroidTv => Self::generic_android_tv(),
            Platform::Web => Self::web(),
            // Unknown: fall back to the Apple TV baseline (its authoritative, safe set).
            Platform::Unknown => Self::apple_tv_4k(),
        }
    }

    fn supports_video(&self, codec: VideoCodec) -> bool {
        self.video_codecs.contains(&codec)
    }

    /// Does the client accept this source container directly (case-insensitive)?
    pub fn supports_container(&self, container: &str) -> bool {
        let c = container.to_ascii_lowercase();
        self.containers.iter().any(|x| x.eq_ignore_ascii_case(&c))
    }

    /// Does the client's codec list include this audio codec?
    pub fn supports_audio(&self, codec: AudioCodec) -> bool {
        self.audio_codecs.contains(&codec)
    }

    /// Can the client bitstream this lossless format as-is? Only a sink that lists the
    /// lossless codec (Shield lists TrueHd/DtsHd; Apple TV does not).
    fn can_bitstream(&self, codec: AudioCodec) -> bool {
        !codec.is_lossless_bitstream() || self.supports_audio(codec)
    }
}

impl From<ClientCapabilities> for ClientProfile {
    /// Build the decision-input profile from a detected/defaulted capability payload
    /// (`docs/.tasks/70`). The `QualityProfile` and `max_bitrate` are carried separately
    /// into `decide` and are not part of the static profile.
    fn from(c: ClientCapabilities) -> Self {
        Self {
            video_codecs: c.video_codecs,
            bit_depth_10: c.bit_depth_10,
            hdr_display: c.hdr_display,
            dolby_vision: c.dolby_vision,
            audio_codecs: c.audio_codecs,
            max_channels: c.max_channels.max(2),
            atmos_passthrough: c.atmos_passthrough,
            containers: c.containers,
            // A detected-capability payload comes from a native client that can remux.
            can_remux_direct: true,
        }
    }
}

/// The target codecs + processing a transcode must apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscodeTarget {
    /// HWA vendor path, or `None` for a fully software pipeline.
    pub vendor: Option<Vendor>,
    /// Decode on the CPU (HW decode impossible/unsupported for this source).
    pub software_decode: bool,
    /// Tone-map HDR/DV down to SDR (BT.2020/PQ → BT.709). Implies the OpenCL/CUDA path
    /// for Dolby Vision sources.
    pub tone_map: bool,
    /// The tone-mapping must go through OpenCL/CUDA (a DV source), not plain VPP.
    pub dv_tone_map: bool,
    /// Target video codec (`h264`/`hevc`), chosen for broad client compatibility.
    pub video_codec: VideoCodec,
    /// Target audio: `Some(codec)` to transcode audio, `None` to copy it through.
    pub audio_transcode_to: Option<AudioCodec>,
    /// `QualityProfile::Capped` bitrate ceiling (bits/sec) applied to video via
    /// `-maxrate`/`-bufsize` in `command.rs`. `None` = uncapped (`docs/.tasks/70`).
    pub max_bitrate: Option<u64>,
    /// An image subtitle (`docs/.tasks/90` §5) to burn into the video: `Some(n)` is the
    /// ffprobe subtitle stream index overlaid onto the frame **after** any tone-map.
    /// `None` = no burn-in. Set only via [`Decision::with_burn_in`]; a text subtitle never
    /// sets this (it rides as a client WebVTT sidecar).
    #[serde(default)]
    pub subtitle_burn_in: Option<i64>,
}

/// The audio plan for a track + client: copy through (passthrough / within cap) or
/// re-encode and/or downmix (`docs/.tasks/70`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioPlan {
    /// Passthrough — supported codec, bitstreamable, channels ≤ cap.
    Copy,
    /// Re-encode and/or downmix to `codec` at `channels`.
    Transcode { codec: AudioCodec, channels: u8 },
}

/// Decide how to serve one audio track to a client (`docs/.tasks/70` §decision matrix).
///
/// Copies when the client supports the codec, can bitstream it (Shield vs Apple TV on
/// TrueHD/DTS-HD), passes its immersive form, and the channel count is within the cap.
/// Otherwise re-encodes to E-AC-3 (surround-preserving) — or AAC stereo when the client
/// lacks E-AC-3 — downmixing to the channel cap.
pub fn audio_plan(track: AudioTrack, client: &ClientProfile) -> AudioPlan {
    let bitstreamable = client.can_bitstream(track.codec);
    // Lossy Atmos (E-AC-3 JOC) passes only where the sink advertises it.
    let atmos_ok = track.immersive != ImmersiveAudio::DolbyAtmos || client.atmos_passthrough;
    let supported = client.supports_audio(track.codec) && bitstreamable && atmos_ok;
    // A 0/unknown channel count is treated as within any cap (never a needless downmix).
    let over_cap = track.channels != 0 && track.channels > client.max_channels;

    if supported && !over_cap {
        return AudioPlan::Copy;
    }

    let codec = if client.supports_audio(AudioCodec::Eac3) {
        AudioCodec::Eac3
    } else {
        AudioCodec::Aac
    };
    // AAC is a stereo fallback; E-AC-3 keeps surround, downmixed to the cap.
    let target_ch = if codec == AudioCodec::Aac {
        2
    } else {
        let src = if track.channels == 0 { 6 } else { track.channels };
        src.min(client.max_channels.max(2))
    };
    AudioPlan::Transcode { codec, channels: target_ch }
}

/// The playback decision for one file + client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    /// Serve the source bytes directly (`/api/direct`), optionally remuxing the
    /// container / transcoding only audio. `remux` is true when the video can be
    /// copied but the container or an audio track must change.
    DirectPlay { remux: bool },
    /// Transcode via HLS (`/api/hls/...`).
    Transcode {
        target: TranscodeTarget,
        /// Stable slug explaining *why* (for the `reason` field + logs).
        reason: &'static str,
    },
}

impl Decision {
    /// `"direct"` or `"hls"` — the `mode` string in the `/api/stream` response.
    pub fn mode(&self) -> &'static str {
        match self {
            Decision::DirectPlay { .. } => "direct",
            Decision::Transcode { .. } => "hls",
        }
    }

    /// The stable reason slug for the `/api/stream` response + logs.
    pub fn reason(&self) -> &'static str {
        match self {
            Decision::DirectPlay { remux: false } => "direct_play",
            Decision::DirectPlay { remux: true } => "remux_container_or_audio",
            Decision::Transcode { reason, .. } => reason,
        }
    }

    /// Force this decision to burn an **image** subtitle into the video (`docs/.tasks/90`
    /// §5). Applied by the api layer when the client selects an image (PGS / VobSub) track
    /// via `sub_burn=1`: an image subtitle cannot become a text sidecar, so it must be
    /// overlaid onto the frame, which forces a **video transcode** even when the video
    /// would otherwise direct-play.
    ///
    /// - A [`Decision::DirectPlay`] is promoted to a [`Decision::Transcode`] carrying a
    ///   fresh [`TranscodeTarget`] (built for this host / source) with the burn-in index.
    /// - An existing [`Decision::Transcode`] keeps its vendor path / tone-map and just
    ///   records the burn-in index so `command.rs` adds the overlay after any tone-map.
    ///
    /// A **text** subtitle never calls this — it rides as a react-native-video `textTracks`
    /// sidecar and the video can still direct-play.
    pub fn with_burn_in(
        self,
        stream_index: i64,
        profile: &MediaProfile,
        client: &ClientProfile,
        quality: Quality,
        caps: &HwCaps,
    ) -> Decision {
        match self {
            Decision::Transcode { mut target, reason } => {
                target.subtitle_burn_in = Some(stream_index);
                Decision::Transcode { target, reason }
            }
            Decision::DirectPlay { .. } => {
                let tone_map = needs_tonemap(profile, client);
                let mut target = transcode_target(caps, false, tone_map, profile, quality);
                target.subtitle_burn_in = Some(stream_index);
                Decision::Transcode {
                    target,
                    reason: "subtitle_burn_in",
                }
            }
        }
    }

    /// Force a would-be direct-play into a real HLS transcode (`force_transcode=1`).
    ///
    /// A browser `<video>` that finds a `DirectPlay` stream unplayable (a `direct` decision
    /// the server guessed wrong, or an exotic profile the element rejects) re-requests the
    /// stream with this flag to demand a server-side H.264+AAC HLS transcode it can always
    /// play. A [`Decision::Transcode`] is returned unchanged (already the safe path); a
    /// [`Decision::DirectPlay`] is promoted to a fresh [`TranscodeTarget`] built for this
    /// host + source, tone-mapping only when the display needs it.
    pub fn force_transcode(
        self,
        profile: &MediaProfile,
        client: &ClientProfile,
        quality: Quality,
        caps: &HwCaps,
    ) -> Decision {
        match self {
            Decision::Transcode { .. } => self,
            Decision::DirectPlay { .. } => {
                let tone_map = needs_tonemap(profile, client);
                let target = transcode_target(caps, /*software_decode=*/ false, tone_map, profile, quality);
                Decision::Transcode {
                    target,
                    reason: "forced_transcode",
                }
            }
        }
    }
}

/// The subtitle handling a stream request resolves to (`docs/.tasks/90` §5). Built by the
/// api layer from the selected `sub` / `sub_burn` params against the file's
/// `subtitle_streams`; only [`SubtitlePlan::BurnIn`] affects the transcode decision (text
/// subtitles ride as a client WebVTT sidecar and never force a transcode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitlePlan {
    /// No subtitle selected, or the client renders a text sidecar itself.
    None,
    /// An image subtitle (PGS / VobSub) to overlay onto the video — forces a transcode.
    BurnIn { stream_index: i64 },
}

/// The "best available quality" control + bitrate ceiling carried alongside the client
/// profile into [`decide`] (`docs/.tasks/70` §QualityProfile). Built by the api layer
/// from the request's `quality` / `max_bitrate` params.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quality {
    pub profile: QualityProfile,
    /// bits/sec; `None` = uncapped. Honored only for `QualityProfile::Capped`.
    pub max_bitrate: Option<u64>,
}

impl Default for Quality {
    fn default() -> Self {
        Self {
            profile: QualityProfile::Auto,
            max_bitrate: None,
        }
    }
}

/// Decide how to serve `profile` (a source file) to `client`, given host `caps`.
///
/// Implements `docs/.tasks/20` §Playback decision table (video axis) plus the audio axis
/// from `docs/.tasks/70`. Pure and deterministic — unit-tested against every table row.
///
/// `container` and `audio` come from the `media_files` row + its default `audio_streams`
/// track. Pass [`AudioTrack::unknown_safe`] and a client-supported container when
/// audio/container are unknown, to avoid forcing a needless remux.
pub fn decide(
    profile: &MediaProfile,
    audio: AudioTrack,
    container: &str,
    client: &ClientProfile,
    quality: Quality,
    caps: &HwCaps,
) -> Decision {
    // --- Forced software-decode case: H.264 High 10 (10-bit AVC). ------------
    // HW decode is universally unsupported (`docs/.tasks/20` row 2 / README §HWA);
    // this always transcodes with a software decoder feeding a HW (or SW) encoder.
    if profile.hw_decode_unsupported {
        return Decision::Transcode {
            target: transcode_target(caps, /*software_decode=*/ true, /*tone_map=*/ needs_tonemap(profile, client), profile, quality),
            reason: "h264_high10_sw_decode",
        };
    }

    // --- Can the client present the *video* as-is? ---------------------------
    // Dolby Vision Profile 7 is never directly presentable (no TV client shows its
    // dual-layer BL+EL). It always transcodes — dropping the EL and keeping/tone-mapping
    // the HDR10 base layer — so it can never satisfy the direct-play branch below, even
    // when the client lists HEVC (`docs/.tasks/90` §3).
    let dv_p7 = profile.hdr == HdrType::DolbyVision && matches!(profile.dv, Some(DvProfile::P7));
    let video_ok = !dv_p7
        && client.supports_video(profile.codec)
        && (profile.bit_depth <= 8 || client.bit_depth_10);

    // --- HDR / Dolby Vision vs the display. ----------------------------------
    let tone_map = needs_tonemap(profile, client);

    // --- QualityProfile::Capped can force a full transcode. ------------------
    // A bitrate ceiling below the source video bitrate forces a re-encode even when the
    // codec would direct-play (`docs/.tasks/70`). `Original` explicitly suppresses this.
    let bitrate_forces = quality.profile == QualityProfile::Capped
        && matches!((quality.max_bitrate, profile_bitrate(profile)), (Some(cap), Some(src)) if src > cap);

    if video_ok && !tone_map && !bitrate_forces {
        // Video is directly presentable (incl. DV P5/P8 to a DV-capable Apple TV on an
        // HDR/DV display). The only remaining question is container + audio.
        let audio_ok = audio_plan(audio, client) == AudioPlan::Copy;
        let container_ok = client.supports_container(container);
        if audio_ok && container_ok {
            return Decision::DirectPlay { remux: false };
        }
        // Video can be copied; only the container or audio needs work. A client that can remux
        // a direct byte stream itself (native TV players) gets a cheap `remux` served over
        // `/api/direct`. A client that cannot (a browser `<video>`) must instead be handed a
        // real server transcode — copying the video, fixing the audio/container into HLS — or
        // it would black-screen on the raw file.
        if client.can_remux_direct {
            return Decision::DirectPlay { remux: true };
        }
        return Decision::Transcode {
            target: transcode_target(caps, /*software_decode=*/ false, /*tone_map=*/ false, profile, quality),
            reason: "web_remux_to_transcode",
        };
    }

    // --- Otherwise transcode. Pick the reason for logs/debugging. ------------
    let reason = if bitrate_forces && video_ok && !tone_map {
        "bitrate_capped"
    } else {
        transcode_reason(profile, client, tone_map, video_ok)
    };
    Decision::Transcode {
        target: transcode_target(caps, /*software_decode=*/ false, tone_map, profile, quality),
        reason,
    }
}

/// The source bitrate for the `QualityProfile::Capped` check (`media_files.bitrate`).
fn profile_bitrate(profile: &MediaProfile) -> Option<u64> {
    profile.bitrate
}

/// Does this source need tone-mapping for the client's display? True when the source
/// is HDR/DV **and** the display cannot present that format.
fn needs_tonemap(profile: &MediaProfile, client: &ClientProfile) -> bool {
    match profile.hdr {
        HdrType::None => false,
        HdrType::DolbyVision => {
            // Profile 7 (BL(HDR10)+EL) is never directly presentable — no TV client shows
            // dual-layer DV (`docs/.tasks/90` §3). It always transcodes (see the P7 gate in
            // `decide`); the *tone-map* question is only about its HDR10 base layer vs the
            // display: keep HDR10 on an HDR display (no tone-map), tone-map on SDR.
            if matches!(profile.dv, Some(DvProfile::P7)) {
                return !client.hdr_display;
            }
            // A DV-capable client on a DV display presents P5/P8 directly; otherwise, if
            // the source is P8 with an HDR10 base layer and the display is plain HDR10,
            // it can still show as HDR10 (no tone-map). Everything else → tone-map.
            if client.dolby_vision {
                false
            } else {
                !(is_p8_hdr10_compatible(profile) && client.hdr_display)
            }
        }
        // HDR10 / HDR10+ / HLG present directly on an HDR display; tone-map on SDR.
        HdrType::Hdr10 | HdrType::Hdr10Plus | HdrType::Hlg => !client.hdr_display,
    }
}

/// Is this a Dolby Vision Profile 8 source whose base layer is HDR10-compatible
/// (`bl_compatible_id == 1`)? Such a file plays as HDR10 on a non-DV HDR display.
fn is_p8_hdr10_compatible(profile: &MediaProfile) -> bool {
    matches!(
        profile.dv,
        Some(medi_core::DvProfile::P8 { bl_compatible_id: 1 })
    )
}

/// Pick the stable reason slug for a transcode.
fn transcode_reason(
    profile: &MediaProfile,
    client: &ClientProfile,
    tone_map: bool,
    video_ok: bool,
) -> &'static str {
    // Dolby Vision Profile 7 (`docs/.tasks/90` §3): always a transcode. On an HDR display
    // the EL is dropped and the HDR10 base layer is kept (no tone-map); on SDR the base is
    // tone-mapped via the normal VPP/CUDA path (not the P5-only OpenCL path).
    if matches!(profile.dv, Some(DvProfile::P7)) {
        return if tone_map {
            "dv_p7_sdr_display"
        } else {
            "dv_p7_hdr10_display"
        };
    }
    if tone_map {
        return match profile.hdr {
            HdrType::DolbyVision => {
                if matches!(profile.dv, Some(DvProfile::P5)) {
                    "dv_p5_sdr_display"
                } else {
                    "dv_p8_sdr_display"
                }
            }
            _ => "hdr_sdr_display",
        };
    }
    if !video_ok {
        if !client.supports_video(profile.codec) {
            return "codec_unsupported";
        }
        return "bit_depth_unsupported";
    }
    "transcode"
}

/// Build the [`TranscodeTarget`] for a decision: choose the vendor path, target video
/// codec (H.264 for maximum client compatibility), and whether audio must convert.
///
/// AV1 with no AV1 HW decoder falls back to dav1d software decode (bundled in
/// jellyfin-ffmpeg) → HW encode (`docs/.tasks/20` row 7).
fn transcode_target(
    caps: &HwCaps,
    mut software_decode: bool,
    tone_map: bool,
    profile: &MediaProfile,
    quality: Quality,
) -> TranscodeTarget {
    let vendor = caps.vendor;

    // Per-codec HW-decode fallback (`docs/.tasks/90` §per-codec HW-decode): a source the
    // host cannot hardware-decode (AV1 without an AV1 hwaccel, VP9/VC-1/MPEG-2 on a host
    // lacking that accel, or any `Other`) falls back to a software decoder feeding the HW
    // (or SW) encoder. AV1→dav1d is now one arm of this general rule.
    if !caps.can_hw_decode(profile.codec) {
        software_decode = true;
    }

    // DV tone-map requires OpenCL/CUDA, and is reserved for **Profile 5** (proprietary
    // IPTPQc2). A P7 source tone-maps its HDR10 base layer via the normal VPP/CUDA path,
    // so it never sets `dv_tone_map` (`docs/.tasks/90` §3). If the host can't run the
    // OpenCL/CUDA path for a P5 source, fall back to software decode + software tonemap.
    let dv_tone_map =
        tone_map && profile.hdr == HdrType::DolbyVision && matches!(profile.dv, Some(DvProfile::P5));
    if dv_tone_map && !caps.can_tonemap_dv() {
        software_decode = true;
    }

    // Target H.264 High — the universally direct-playable codec for both TV clients.
    let video_codec = VideoCodec::H264;

    // `Capped` threads its ceiling through to `-maxrate`/`-bufsize` in `command.rs`.
    let max_bitrate = if quality.profile == QualityProfile::Capped {
        quality.max_bitrate
    } else {
        None
    };

    TranscodeTarget {
        vendor,
        software_decode,
        tone_map,
        dv_tone_map,
        video_codec,
        // Audio target is decided by the caller against the client via `audio_plan`.
        audio_transcode_to: None,
        max_bitrate,
        // Burn-in is layered on by the api via `Decision::with_burn_in` only when the
        // client selects an image subtitle.
        subtitle_burn_in: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medi_core::DvProfile;

    fn hw_intel() -> HwCaps {
        let mut c = HwCaps::software_only();
        c.vendor = Some(Vendor::Intel);
        c.render_node = Some("/dev/dri/renderD128".into());
        c.opencl = true;
        c.encoders = vec!["h264_qsv".into()];
        c
    }

    fn prof(codec: VideoCodec, bit_depth: u8, hdr: HdrType, dv: Option<DvProfile>) -> MediaProfile {
        MediaProfile {
            codec,
            width: 3840,
            height: 2160,
            bit_depth,
            hdr,
            dv,
            hw_decode_unsupported: false,
            bitrate: None,
        }
    }

    /// A source audio track with a codec, channel count, and immersive marker.
    fn aud(codec: AudioCodec, channels: u8, immersive: ImmersiveAudio) -> AudioTrack {
        AudioTrack { codec, channels, immersive }
    }

    /// A supported stereo AAC track — the audio never blocks a direct-play here.
    fn aac() -> AudioTrack {
        AudioTrack::unknown_safe()
    }

    fn q() -> Quality {
        Quality::default()
    }

    #[test]
    fn h264_sdr_direct_plays_to_apple_tv() {
        let p = prof(VideoCodec::H264, 8, HdrType::None, None);
        let d = decide(&p, aac(), "mp4", &ClientProfile::apple_tv_4k(), q(), &hw_intel());
        assert_eq!(d, Decision::DirectPlay { remux: false });
        assert_eq!(d.mode(), "direct");
    }

    #[test]
    fn h264_aac_mp4_direct_plays_to_web() {
        // The common browser-friendly case: H.264 + AAC in mp4 direct-plays for `platform=web`.
        let p = prof(VideoCodec::H264, 8, HdrType::None, None);
        let d = decide(&p, aac(), "mp4", &ClientProfile::web(), q(), &hw_intel());
        assert_eq!(d, Decision::DirectPlay { remux: false });
    }

    #[test]
    fn force_transcode_promotes_web_direct_play_to_hls() {
        // A browser that finds a `direct` stream unplayable re-requests with force_transcode=1;
        // even the normally direct-playing H.264/AAC/mp4 case must then become an HLS transcode.
        let p = prof(VideoCodec::H264, 8, HdrType::None, None);
        let web = ClientProfile::web();
        let base = decide(&p, aac(), "mp4", &web, q(), &hw_intel());
        assert_eq!(base, Decision::DirectPlay { remux: false });

        let forced = base.force_transcode(&p, &web, q(), &hw_intel());
        assert_eq!(forced.mode(), "hls");
        assert_eq!(forced.reason(), "forced_transcode");
        match forced {
            Decision::Transcode { target, .. } => {
                assert_eq!(target.video_codec, VideoCodec::H264);
                assert!(!target.tone_map, "SDR source needs no tone-map");
            }
            _ => panic!("expected transcode"),
        }
    }

    #[test]
    fn force_transcode_leaves_existing_transcode_unchanged() {
        // An already-transcoding decision is the safe path; force_transcode is a no-op on it.
        let p = prof(VideoCodec::Hevc, 10, HdrType::Hdr10, None);
        let web = ClientProfile::web();
        let base = decide(&p, aac(), "mp4", &web, q(), &hw_intel());
        assert_eq!(base.mode(), "hls");
        let forced = base.clone().force_transcode(&p, &web, q(), &hw_intel());
        assert_eq!(forced, base, "no-op on an existing transcode");
    }

    #[test]
    fn hevc_transcodes_for_web() {
        // A browser can't reliably decode HEVC → the web profile forces a transcode instead of
        // handing back a direct stream that would black-screen.
        let p = prof(VideoCodec::Hevc, 10, HdrType::Hdr10, None);
        let d = decide(&p, aac(), "mp4", &ClientProfile::web(), q(), &hw_intel());
        assert_eq!(d.mode(), "hls", "HEVC must transcode for a browser");
    }

    #[test]
    fn ac3_audio_transcodes_for_web() {
        // AC-3 audio is undecodable in most browsers. Since a browser can't remux a raw direct
        // stream itself, a would-be `remux` is promoted to a real transcode (H.264 copy + audio
        // fixed into HLS) rather than served as an unplayable `/api/direct` file.
        let p = prof(VideoCodec::H264, 8, HdrType::None, None);
        let d = decide(&p, aud(AudioCodec::Ac3, 6, ImmersiveAudio::None), "mp4", &ClientProfile::web(), q(), &hw_intel());
        assert_eq!(d.mode(), "hls", "AC-3 audio must transcode for a browser");
        assert_eq!(d.reason(), "web_remux_to_transcode");
    }

    #[test]
    fn mkv_transcodes_for_web_but_remuxes_for_shield() {
        // An MKV with browser-OK codecs: Shield opens MKV directly; a browser can't, and can't
        // remux a direct stream, so it gets a real transcode rather than a raw `/api/direct`.
        let p = prof(VideoCodec::H264, 8, HdrType::None, None);
        let shield = decide(&p, aac(), "mkv", &ClientProfile::nvidia_shield(), q(), &hw_intel());
        assert_eq!(shield, Decision::DirectPlay { remux: false }, "Shield opens MKV directly");
        let web = decide(&p, aac(), "mkv", &ClientProfile::web(), q(), &hw_intel());
        assert_eq!(web.mode(), "hls", "browser can't open MKV → transcode");
    }

    #[test]
    fn hevc_hdr10_direct_plays_on_hdr_display() {
        // Apple TV 4K on an HDR display presents HDR10 directly — no transcode.
        let p = prof(VideoCodec::Hevc, 10, HdrType::Hdr10, None);
        let d = decide(&p, aud(AudioCodec::Eac3, 6, ImmersiveAudio::None), "mp4", &ClientProfile::apple_tv_4k(), q(), &hw_intel());
        assert_eq!(d, Decision::DirectPlay { remux: false });
    }

    #[test]
    fn hevc_hdr10_tone_maps_on_sdr_display() {
        let p = prof(VideoCodec::Hevc, 10, HdrType::Hdr10, None);
        let d = decide(&p, aac(), "mp4", &ClientProfile::sdr_baseline(), q(), &hw_intel());
        match d {
            Decision::Transcode { target, reason } => {
                assert!(target.tone_map);
                assert!(!target.dv_tone_map);
                assert_eq!(reason, "hdr_sdr_display");
            }
            _ => panic!("expected transcode"),
        }
    }

    #[test]
    fn dv_p5_direct_plays_to_dv_apple_tv() {
        // The chosen policy: DV-capable Apple TV on a DV display plays P5 directly.
        let p = prof(VideoCodec::Hevc, 10, HdrType::DolbyVision, Some(DvProfile::P5));
        let d = decide(&p, aud(AudioCodec::Eac3, 6, ImmersiveAudio::None), "mp4", &ClientProfile::apple_tv_4k(), q(), &hw_intel());
        assert_eq!(d, Decision::DirectPlay { remux: false });
    }

    #[test]
    fn dv_p5_tone_maps_on_sdr_display_via_opencl() {
        let p = prof(VideoCodec::Hevc, 10, HdrType::DolbyVision, Some(DvProfile::P5));
        let d = decide(&p, aac(), "mp4", &ClientProfile::sdr_baseline(), q(), &hw_intel());
        match d {
            Decision::Transcode { target, reason } => {
                assert!(target.dv_tone_map, "DV must use the OpenCL/CUDA tone-map path");
                assert_eq!(target.vendor, Some(Vendor::Intel));
                assert!(!target.software_decode, "Intel+OpenCL keeps HW decode");
                assert_eq!(reason, "dv_p5_sdr_display");
            }
            _ => panic!("expected transcode"),
        }
    }

    #[test]
    fn dv_p8_hdr10_compat_plays_as_hdr10_on_hdr10_display() {
        // Non-DV client, HDR10 display: P8.1 shows as HDR10 → no tone-map.
        let mut client = ClientProfile::apple_tv_4k();
        client.dolby_vision = false; // e.g. Android TV without DV
        let p = prof(
            VideoCodec::Hevc,
            10,
            HdrType::DolbyVision,
            Some(DvProfile::P8 { bl_compatible_id: 1 }),
        );
        let d = decide(&p, aud(AudioCodec::Eac3, 6, ImmersiveAudio::None), "mp4", &client, q(), &hw_intel());
        assert_eq!(d, Decision::DirectPlay { remux: false });
    }

    #[test]
    fn dv_p5_on_sw_host_falls_back_to_software() {
        let p = prof(VideoCodec::Hevc, 10, HdrType::DolbyVision, Some(DvProfile::P5));
        let d = decide(&p, aac(), "mp4", &ClientProfile::sdr_baseline(), q(), &HwCaps::software_only());
        match d {
            Decision::Transcode { target, .. } => {
                assert!(target.dv_tone_map);
                assert!(target.software_decode, "no OpenCL/CUDA → software tone-map");
                assert_eq!(target.vendor, None);
            }
            _ => panic!("expected transcode"),
        }
    }

    #[test]
    fn h264_high10_forces_software_decode() {
        let mut p = prof(VideoCodec::H264, 10, HdrType::None, None);
        p.hw_decode_unsupported = true;
        let d = decide(&p, aac(), "mp4", &ClientProfile::apple_tv_4k(), q(), &hw_intel());
        match d {
            Decision::Transcode { target, reason } => {
                assert!(target.software_decode);
                assert_eq!(reason, "h264_high10_sw_decode");
            }
            _ => panic!("expected transcode"),
        }
    }

    #[test]
    fn av1_without_hw_decode_uses_software() {
        // Apple TV lists AV1 as decodable, but here the display is SDR so we transcode;
        // the host has no AV1 hwaccel → software (dav1d) decode.
        let p = prof(VideoCodec::Av1, 10, HdrType::Hdr10, None);
        let d = decide(&p, aac(), "mp4", &ClientProfile::sdr_baseline(), q(), &hw_intel());
        match d {
            Decision::Transcode { target, .. } => {
                assert!(target.software_decode, "no AV1 hwaccel → dav1d software decode");
            }
            _ => panic!("expected transcode"),
        }
    }

    // --- widened codecs + DV P7 + subtitle burn-in (`docs/.tasks/90`) --------

    #[test]
    fn legacy_codecs_transcode_to_h264() {
        // VC-1 / MPEG-2 / MPEG-4 are not in any client's video list → always transcode,
        // and the host here has no matching hwaccel → software decode.
        for codec in [VideoCodec::Vc1, VideoCodec::Mpeg2, VideoCodec::Mpeg4] {
            let p = prof(codec, 8, HdrType::None, None);
            let d = decide(&p, aac(), "mp4", &ClientProfile::apple_tv_4k(), q(), &hw_intel());
            match d {
                Decision::Transcode { target, reason } => {
                    assert_eq!(target.video_codec, VideoCodec::H264);
                    assert!(target.software_decode, "{codec:?} has no hwaccel here → sw decode");
                    assert_eq!(reason, "codec_unsupported");
                }
                _ => panic!("expected transcode for {codec:?}"),
            }
        }
    }

    #[test]
    fn vp9_hw_decode_gated_on_hwaccel() {
        let p = prof(VideoCodec::Vp9, 8, HdrType::None, None);
        // No VP9 hwaccel → software decode.
        let d = decide(&p, aac(), "mp4", &ClientProfile::apple_tv_4k(), q(), &hw_intel());
        match d {
            Decision::Transcode { target, .. } => assert!(target.software_decode),
            _ => panic!("expected transcode"),
        }
        // With a VP9 hwaccel advertised → HW decode.
        let mut caps = hw_intel();
        caps.hwaccels = vec!["vp9".into(), "qsv".into()];
        let d = decide(&p, aac(), "mp4", &ClientProfile::apple_tv_4k(), q(), &caps);
        match d {
            Decision::Transcode { target, .. } => {
                assert!(!target.software_decode, "VP9 hwaccel → HW decode");
                assert_eq!(target.video_codec, VideoCodec::H264);
            }
            _ => panic!("expected transcode"),
        }
    }

    #[test]
    fn dv_p7_on_hdr_display_keeps_hdr10_no_tonemap() {
        // P7 never direct-plays; on an HDR display drop the EL, keep the HDR10 base (no
        // tone-map), and never use the P5 OpenCL path.
        let p = prof(VideoCodec::Hevc, 10, HdrType::DolbyVision, Some(DvProfile::P7));
        let d = decide(
            &p,
            aud(AudioCodec::Eac3, 6, ImmersiveAudio::None),
            "mp4",
            &ClientProfile::apple_tv_4k(),
            q(),
            &hw_intel(),
        );
        match d {
            Decision::Transcode { target, reason } => {
                assert_eq!(reason, "dv_p7_hdr10_display");
                assert!(!target.tone_map, "HDR display keeps HDR10 base");
                assert!(!target.dv_tone_map, "P7 never uses the P5 OpenCL path");
            }
            _ => panic!("P7 must transcode even on a DV-capable client"),
        }
    }

    #[test]
    fn dv_p7_on_sdr_display_tonemaps_via_vpp_not_opencl() {
        let p = prof(VideoCodec::Hevc, 10, HdrType::DolbyVision, Some(DvProfile::P7));
        let d = decide(&p, aac(), "mp4", &ClientProfile::sdr_baseline(), q(), &hw_intel());
        match d {
            Decision::Transcode { target, reason } => {
                assert_eq!(reason, "dv_p7_sdr_display");
                assert!(target.tone_map, "SDR display tone-maps the HDR10 base");
                assert!(!target.dv_tone_map, "P7 uses VPP/CUDA, not the OpenCL P5 path");
                assert!(!target.software_decode, "Intel HEVC decodes in HW");
            }
            _ => panic!("expected transcode"),
        }
    }

    #[test]
    fn image_subtitle_burn_in_forces_transcode() {
        // A text sub leaves a direct-play alone; an image sub burn-in promotes it to a
        // transcode carrying the burn-in index.
        let p = prof(VideoCodec::H264, 8, HdrType::None, None);
        let base = decide(&p, aac(), "mp4", &ClientProfile::apple_tv_4k(), q(), &hw_intel());
        assert_eq!(base, Decision::DirectPlay { remux: false });

        let burned = base.with_burn_in(0, &p, &ClientProfile::apple_tv_4k(), q(), &hw_intel());
        match burned {
            Decision::Transcode { target, reason } => {
                assert_eq!(reason, "subtitle_burn_in");
                assert_eq!(target.subtitle_burn_in, Some(0));
            }
            _ => panic!("image sub must force a transcode"),
        }
    }

    #[test]
    fn mp3_audio_copies_on_all_clients() {
        let mp3 = aud(AudioCodec::Mp3, 2, ImmersiveAudio::None);
        assert_eq!(audio_plan(mp3, &ClientProfile::apple_tv_4k()), AudioPlan::Copy);
        assert_eq!(audio_plan(mp3, &ClientProfile::nvidia_shield()), AudioPlan::Copy);
        assert_eq!(audio_plan(mp3, &ClientProfile::generic_android_tv()), AudioPlan::Copy);
    }

    #[test]
    fn vorbis_wma_alac_transcode_by_default() {
        for codec in [AudioCodec::Vorbis, AudioCodec::Wma, AudioCodec::Alac] {
            let t = aud(codec, 2, ImmersiveAudio::None);
            assert!(
                matches!(audio_plan(t, &ClientProfile::apple_tv_4k()), AudioPlan::Transcode { .. }),
                "{codec:?} transcodes by default on Apple TV"
            );
        }
    }

    #[test]
    fn ts_container_direct_plays_on_shield() {
        // MPEG-TS opens directly on ExoPlayer/Shield → no remux for an H.264/AAC .ts file.
        let p = prof(VideoCodec::H264, 8, HdrType::None, None);
        let d = decide(&p, aac(), "ts", &ClientProfile::nvidia_shield(), q(), &hw_intel());
        assert_eq!(d, Decision::DirectPlay { remux: false });
        // Apple TV can't open .ts → remux.
        let d2 = decide(&p, aac(), "ts", &ClientProfile::apple_tv_4k(), q(), &hw_intel());
        assert_eq!(d2, Decision::DirectPlay { remux: true });
    }

    #[test]
    fn mkv_container_forces_remux_even_when_codec_ok() {
        // HEVC SDR video the Apple TV can decode, but in an MKV it can't open → remux.
        let p = prof(VideoCodec::Hevc, 10, HdrType::None, None);
        let d = decide(&p, aud(AudioCodec::Eac3, 6, ImmersiveAudio::None), "mkv", &ClientProfile::apple_tv_4k(), q(), &hw_intel());
        assert_eq!(d, Decision::DirectPlay { remux: true });
    }

    // --- audio axis (`docs/.tasks/70` §combined matrix) ---------------------

    #[test]
    fn dts_audio_forces_remux_with_audio_transcode() {
        // Plain DTS core is unsupported on Apple TV → video copies, audio must re-encode.
        let p = prof(VideoCodec::H264, 8, HdrType::None, None);
        let d = decide(&p, aud(AudioCodec::Dts, 6, ImmersiveAudio::None), "mp4", &ClientProfile::apple_tv_4k(), q(), &hw_intel());
        assert_eq!(d, Decision::DirectPlay { remux: true });
        assert_eq!(
            audio_plan(aud(AudioCodec::Dts, 6, ImmersiveAudio::None), &ClientProfile::apple_tv_4k()),
            AudioPlan::Transcode { codec: AudioCodec::Eac3, channels: 6 }
        );
    }

    #[test]
    fn shield_copies_truehd_apple_tv_transcodes() {
        // The Shield ≠ Apple TV split on lossless bitstream: Shield passthrough, Apple TV
        // re-encode to E-AC-3 5.1.
        let truehd = aud(AudioCodec::TrueHd, 8, ImmersiveAudio::DolbyAtmos);
        assert_eq!(audio_plan(truehd, &ClientProfile::nvidia_shield()), AudioPlan::Copy);
        assert_eq!(
            audio_plan(truehd, &ClientProfile::apple_tv_4k()),
            AudioPlan::Transcode { codec: AudioCodec::Eac3, channels: 8 }
        );

        // On the full decision: MKV+TrueHD to a Shield remuxes only the container (audio
        // copies via passthrough); to Apple TV it also re-encodes audio.
        let p = prof(VideoCodec::Hevc, 10, HdrType::None, None);
        let shield = decide(&p, truehd, "mkv", &ClientProfile::nvidia_shield(), q(), &hw_intel());
        assert_eq!(shield, Decision::DirectPlay { remux: false }, "Shield opens MKV + bitstreams TrueHD");
    }

    #[test]
    fn seven_one_downmixes_over_a_five_one_cap() {
        // Any codec with channels > client cap downmixes to the cap.
        let mut client = ClientProfile::nvidia_shield();
        client.max_channels = 6; // a 5.1 AVR
        let plan = audio_plan(aud(AudioCodec::Eac3, 8, ImmersiveAudio::None), &client);
        assert_eq!(plan, AudioPlan::Transcode { codec: AudioCodec::Eac3, channels: 6 });
    }

    #[test]
    fn eac3_joc_atmos_copies_on_both() {
        // Lossy Atmos (E-AC-3 JOC) passes through on Apple TV and Shield.
        let joc = aud(AudioCodec::Eac3, 6, ImmersiveAudio::DolbyAtmos);
        assert_eq!(audio_plan(joc, &ClientProfile::apple_tv_4k()), AudioPlan::Copy);
        assert_eq!(audio_plan(joc, &ClientProfile::nvidia_shield()), AudioPlan::Copy);
        // But the pessimistic generic Android default has no atmos passthrough → re-encode.
        assert!(matches!(
            audio_plan(joc, &ClientProfile::generic_android_tv()),
            AudioPlan::Transcode { .. }
        ));
    }

    #[test]
    fn flac_transcodes_to_aac_on_apple_tv() {
        // Unsupported codec on Apple TV → re-encode. AAC because Apple TV has E-AC-3, so
        // actually it targets E-AC-3; use a client without E-AC-3 to hit the AAC branch.
        let flac = aud(AudioCodec::Flac, 2, ImmersiveAudio::None);
        assert_eq!(
            audio_plan(flac, &ClientProfile::apple_tv_4k()),
            AudioPlan::Transcode { codec: AudioCodec::Eac3, channels: 2 }
        );
        assert_eq!(
            audio_plan(flac, &ClientProfile::sdr_baseline()),
            AudioPlan::Transcode { codec: AudioCodec::Aac, channels: 2 }
        );
    }

    #[test]
    fn audio_copies_through_when_supported() {
        assert_eq!(audio_plan(aud(AudioCodec::Eac3, 6, ImmersiveAudio::None), &ClientProfile::apple_tv_4k()), AudioPlan::Copy);
        assert_eq!(audio_plan(aac(), &ClientProfile::apple_tv_4k()), AudioPlan::Copy);
    }

    #[test]
    fn quality_original_copies_where_auto_would_cap() {
        // A high-bitrate H.264 file that would be forced to transcode under Capped stays
        // direct under Original / Auto.
        let mut p = prof(VideoCodec::H264, 8, HdrType::None, None);
        p.bitrate = Some(40_000_000);
        let capped = Quality { profile: QualityProfile::Capped, max_bitrate: Some(8_000_000) };
        let d = decide(&p, aac(), "mp4", &ClientProfile::apple_tv_4k(), capped, &hw_intel());
        match d {
            Decision::Transcode { reason, target } => {
                assert_eq!(reason, "bitrate_capped");
                assert_eq!(target.max_bitrate, Some(8_000_000));
            }
            _ => panic!("expected a capped transcode"),
        }
        // Original suppresses the bitrate re-encode.
        let orig = Quality { profile: QualityProfile::Original, max_bitrate: Some(8_000_000) };
        let d = decide(&p, aac(), "mp4", &ClientProfile::apple_tv_4k(), orig, &hw_intel());
        assert_eq!(d, Decision::DirectPlay { remux: false });
    }

    #[test]
    fn from_capabilities_builds_profile() {
        let caps = ClientCapabilities {
            platform: Platform::Shield,
            video_codecs: vec![VideoCodec::Hevc],
            bit_depth_10: true,
            hdr_display: true,
            dolby_vision: true,
            audio_codecs: vec![AudioCodec::Eac3, AudioCodec::TrueHd],
            atmos_passthrough: true,
            max_channels: 6,
            containers: vec!["mkv".into()],
            quality: QualityProfile::Auto,
            max_bitrate: None,
        };
        let p: ClientProfile = caps.into();
        assert_eq!(p.max_channels, 6);
        assert!(p.atmos_passthrough);
        assert!(p.supports_audio(AudioCodec::TrueHd));
    }
}
