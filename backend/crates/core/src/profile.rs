//! Media profile types shared schema → ingest → transcode → api.
//!
//! Field semantics mirror the `media_files` columns in `docs/.tasks/01-db-schema.md`.
//! The Dolby Vision mapping mirrors `docs/.tasks/10-phase1-foundation-data.md`.

use serde::{Deserialize, Serialize};

/// Video codec of the primary video stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
    // NEW (`docs/.tasks/90`) — recognized sources no TV client natively decodes; always
    // transcoded to H.264. VP9 is the one where the host may still HW-*decode* it.
    Vc1,
    Mpeg2,
    Mpeg4,
    Vp9,
    /// Genuinely unknown (RealMedia, etc.) — still yields a transcode, never a crash.
    Other,
}

impl VideoCodec {
    /// Map an ffprobe `codec_name` to a typed codec (`docs/.tasks/90` §1). The single
    /// source of truth — used by both `ingest` (normalize before persist) and
    /// `db::MediaFile::profile()` (read back) so the mapping never drifts between them.
    pub fn from_ffprobe(name: &str) -> Self {
        match name {
            "h264" => VideoCodec::H264,
            "hevc" => VideoCodec::Hevc,
            "av1" => VideoCodec::Av1,
            "vc1" => VideoCodec::Vc1,
            "mpeg2video" => VideoCodec::Mpeg2,
            // DivX/Xvid + the MS-MPEG4 variants old rips carry.
            "mpeg4" | "msmpeg4v2" | "msmpeg4v3" => VideoCodec::Mpeg4,
            "vp9" => VideoCodec::Vp9,
            _ => VideoCodec::Other,
        }
    }
}

/// HDR classification. `hdr_type` column in `media_files`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HdrType {
    None,
    Hdr10,
    Hdr10Plus,
    Hlg,
    DolbyVision,
}

/// Dolby Vision profile. Drives the transcode decision downstream (Phase 2).
///
/// - `P5` — proprietary IPTPQc2, no fallback layer → always transcode for SDR displays.
/// - `P7` — BL(HDR10)+EL, common in 4K Blu-ray rips.
/// - `P8` — carries a base-layer compatibility id (see [`DvProfile::P8`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "profile")]
pub enum DvProfile {
    P5,
    P7,
    /// `bl_compatible_id`: 1 = HDR10 fallback, 4 = SDR fallback (others reserved).
    P8 { bl_compatible_id: u8 },
}

impl DvProfile {
    /// The raw profile number (5, 7, 8) as stored in `media_files.dv_profile`.
    pub fn number(self) -> u8 {
        match self {
            DvProfile::P5 => 5,
            DvProfile::P7 => 7,
            DvProfile::P8 { .. } => 8,
        }
    }
}

/// Audio codecs the pipeline recognizes (`docs/.tasks/70`). Shared across `ingest`,
/// `transcode`, and `api`; `transcode` re-exports it to avoid a churny rename.
///
/// DTS is split into the lossy core (`Dts`) and the lossless family (`DtsHd` = DTS-HD MA
/// / High-Res) because only the split lets the passthrough decision be correct on Shield
/// (which bitstreams DTS-HD MA losslessly, while Apple TV must re-encode it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioCodec {
    Aac,
    Ac3,
    /// Dolby Digital Plus (E-AC-3), also the carrier for Atmos (JOC).
    Eac3,
    /// DTS lossy core.
    Dts,
    /// DTS-HD MA / High-Res — a lossless bitstream format.
    DtsHd,
    TrueHd,
    Flac,
    Opus,
    Pcm,
    // NEW (`docs/.tasks/90`) — mainstream codecs that previously collapsed to `Other`.
    // MP3 is universally decodable (a client default); the rest transcode by default but
    // the enum lets a detected `AudioCapabilities` payload opt a device in.
    Mp3,
    Vorbis,
    Wma,
    /// Apple Lossless — lossless but always *decoded*, never HDMI-bitstreamed.
    Alac,
    Other,
}

impl AudioCodec {
    /// Lossless bitstream formats that only a passthrough-capable sink plays as-is.
    /// Apple TV cannot bitstream these; NVIDIA Shield can. ALAC is lossless but is
    /// always decoded (never bitstreamed), so it is deliberately excluded here.
    pub fn is_lossless_bitstream(self) -> bool {
        matches!(self, AudioCodec::TrueHd | AudioCodec::DtsHd)
    }
}

/// Immersive-audio marker parsed from the ffprobe stream `profile` string
/// (`docs/.tasks/70`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImmersiveAudio {
    None,
    DolbyAtmos,
    DtsX,
}

/// How a subtitle track can be served (`docs/.tasks/90` §5). Drives the passthrough-vtt
/// vs burn-in split: **text** subtitles convert to WebVTT and ride as a sidecar without a
/// video transcode; **image** subtitles (PGS / VobSub) can only be burned into the video.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubtitleFormat {
    Text,
    Image,
}

impl SubtitleFormat {
    /// Classify an ffprobe subtitle `codec_name`. Text formats convert to WebVTT; image
    /// formats must be burned in. An unknown codec defaults to **text** — a WebVTT
    /// passthrough attempt is cheap and non-destructive, unlike a forced burn-in.
    pub fn from_ffprobe(name: &str) -> Self {
        match name {
            "hdmv_pgs_subtitle" | "dvd_subtitle" | "dvdsub" | "dvb_subtitle" | "xsub" => {
                SubtitleFormat::Image
            }
            _ => SubtitleFormat::Text,
        }
    }

    /// The `subtitle_streams.format` string persisted in the DB.
    pub fn as_str(self) -> &'static str {
        match self {
            SubtitleFormat::Text => "text",
            SubtitleFormat::Image => "image",
        }
    }
}

/// "Best available quality" control (`docs/.tasks/70`). Default is `Auto`; the client's
/// default *setting* is `Original`.
///
/// - `Original` biases toward copy / passthrough and suppresses any bitrate-driven
///   video re-encode.
/// - `Capped` + a `max_bitrate` forces a transcode when the source exceeds the ceiling.
/// - `Auto` preserves today's behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QualityProfile {
    Original,
    Auto,
    Capped,
}

impl Default for QualityProfile {
    fn default() -> Self {
        QualityProfile::Auto
    }
}

/// The client platform, selecting a static per-device capability default
/// (`docs/.tasks/70`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    AppleTv,
    Shield,
    AndroidTv,
    /// A desktop/mobile web browser (the SPA). Conservative codec support — see
    /// `ClientProfile::web` — so undecodable sources transcode instead of black-screening.
    Web,
    Unknown,
}

/// Detected (or defaulted) capabilities the client sends to `/api/stream`
/// (`docs/.tasks/70`). On Android these come from ExoPlayer/media3 `AudioCapabilities`
/// (the HDMI `ACTION_HDMI_AUDIO_PLUG` intent → `EXTRA_ENCODINGS` / `EXTRA_MAX_CHANNEL_COUNT`);
/// on Apple TV a fixed profile is authoritative.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientCapabilities {
    pub platform: Platform,
    pub video_codecs: Vec<VideoCodec>,
    pub bit_depth_10: bool,
    pub hdr_display: bool,
    pub dolby_vision: bool,
    /// Audio codecs the client can decode OR bitstream (passthrough).
    pub audio_codecs: Vec<AudioCodec>,
    /// ExoPlayer `ENCODING_E_AC3_JOC` present — lossy Atmos passthrough.
    pub atmos_passthrough: bool,
    /// `EXTRA_MAX_CHANNEL_COUNT`; 2 if unknown.
    pub max_channels: u8,
    pub containers: Vec<String>,
    pub quality: QualityProfile,
    /// bits/sec; `None` = uncapped.
    pub max_bitrate: Option<u64>,
}

/// The normalized description of a physical media file's video characteristics.
///
/// This is the in-memory shape shared across crates; the persisted form is the
/// `media_files` row (`docs/.tasks/01-db-schema.md`).
// Not `Eq`: `frame_rate` is an `f64`. Nothing compares `MediaProfile` for equality or uses it
// as a hash key, so `PartialEq` alone is sufficient (the decision output `TranscodeTarget` stays
// `Eq` — it carries the integer `gop_frames`, not the float).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaProfile {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub hdr: HdrType,
    /// Present only when `hdr == DolbyVision`.
    pub dv: Option<DvProfile>,
    /// True for formats hardware decoders cannot handle (e.g. H.264 High 10),
    /// forcing a software-decode transcode path. See Phase 2.
    pub hw_decode_unsupported: bool,
    /// Source video/overall bitrate in bits/sec, if known. Drives the
    /// `QualityProfile::Capped` decision (`docs/.tasks/70`). `None` when unprobed.
    #[serde(default)]
    pub bitrate: Option<u64>,
    /// Source video frame rate (e.g. 23.976), if probed (`media_files.frame_rate`, V13). Drives
    /// the HLS keyframe GOP so segments cut at every `SEGMENT_SECONDS` boundary (`docs/.tasks/101`).
    /// `None` when unprobed → the decision falls back to a safe default fps.
    #[serde(default)]
    pub frame_rate: Option<f64>,
}
