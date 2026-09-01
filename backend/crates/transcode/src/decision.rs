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

use medi_core::{HdrType, MediaProfile, VideoCodec};

use crate::caps::HwCaps;

/// The GPU vendor family whose HWA path a transcode uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Vendor {
    Intel,
    Nvidia,
    Amd,
}

/// Audio codec of a source or target track (the subset AVPlayer/ExoPlayer care about).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioCodec {
    Aac,
    Ac3,
    /// Dolby Digital Plus (E-AC-3), also the carrier for Atmos (JOC).
    Eac3,
    /// Not decodable by AVPlayer — must be transcoded for Apple TV.
    Dts,
    TrueHd,
    Flac,
    Opus,
    Other,
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
    /// Containers the client can open directly (for the direct-play remux decision).
    pub containers: Vec<String>,
}

impl ClientProfile {
    /// The Apple TV 4K baseline (used when a `/api/stream` request sends no hints).
    ///
    /// Reflects AVPlayer on tvOS: H.264 + HEVC (incl. 10-bit, HDR10, HLG, Dolby Vision
    /// P5/P8) and AV1 (A15/A17 models); AAC / AC-3 / E-AC-3 (Atmos) audio; MP4/MOV and
    /// fragmented-MP4 HLS containers. DTS/TrueHD are **not** listed — they force an
    /// audio transcode.
    pub fn apple_tv_4k() -> Self {
        Self {
            video_codecs: vec![VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Av1],
            bit_depth_10: true,
            hdr_display: true,
            dolby_vision: true,
            audio_codecs: vec![AudioCodec::Aac, AudioCodec::Ac3, AudioCodec::Eac3],
            containers: vec!["mp4".into(), "mov".into(), "m4v".into(), "hls".into()],
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
            containers: vec!["mp4".into(), "hls".into()],
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

    fn supports_audio(&self, codec: AudioCodec) -> bool {
        self.audio_codecs.contains(&codec)
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
}

/// Decide how to serve `profile` (a source file) to `client`, given host `caps`.
///
/// Implements `docs/.tasks/20` §Playback decision table, extended with the container /
/// audio checks tvOS AVPlayer needs. Pure and deterministic — unit-tested against every
/// table row.
///
/// `container` and `audio` come from the `media_files` row (audio codec of the primary
/// track). Pass `AudioCodec::Aac` and a client-supported container when audio/container
/// are unknown to avoid forcing a needless remux.
pub fn decide(
    profile: &MediaProfile,
    audio: AudioCodec,
    container: &str,
    client: &ClientProfile,
    caps: &HwCaps,
) -> Decision {
    // --- Forced software-decode case: H.264 High 10 (10-bit AVC). ------------
    // HW decode is universally unsupported (`docs/.tasks/20` row 2 / README §HWA);
    // this always transcodes with a software decoder feeding a HW (or SW) encoder.
    if profile.hw_decode_unsupported {
        return Decision::Transcode {
            target: transcode_target(caps, /*software_decode=*/ true, /*tone_map=*/ needs_tonemap(profile, client), profile),
            reason: "h264_high10_sw_decode",
        };
    }

    // --- Can the client present the *video* as-is? ---------------------------
    let video_ok = client.supports_video(profile.codec)
        && (profile.bit_depth <= 8 || client.bit_depth_10);

    // --- HDR / Dolby Vision vs the display. ----------------------------------
    let tone_map = needs_tonemap(profile, client);

    if video_ok && !tone_map {
        // Video is directly presentable (incl. DV P5/P8 to a DV-capable Apple TV on an
        // HDR/DV display). The only remaining question is container + audio.
        let audio_ok = client.supports_audio(audio);
        let container_ok = client.supports_container(container);
        if audio_ok && container_ok {
            return Decision::DirectPlay { remux: false };
        }
        // Video can be copied; only the container or audio needs work. A remux (served
        // over `/api/direct`, or a copy-video HLS) is far cheaper than re-encoding.
        return Decision::DirectPlay { remux: true };
    }

    // --- Otherwise transcode. Pick the reason for logs/debugging. ------------
    let reason = transcode_reason(profile, client, tone_map, video_ok);
    Decision::Transcode {
        target: transcode_target(caps, /*software_decode=*/ false, tone_map, profile),
        reason,
    }
}

/// Does this source need tone-mapping for the client's display? True when the source
/// is HDR/DV **and** the display cannot present that format.
fn needs_tonemap(profile: &MediaProfile, client: &ClientProfile) -> bool {
    match profile.hdr {
        HdrType::None => false,
        HdrType::DolbyVision => {
            // A DV-capable client on a DV display presents it directly; otherwise, if
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
    if tone_map {
        return match profile.hdr {
            HdrType::DolbyVision => {
                if matches!(profile.dv, Some(medi_core::DvProfile::P5)) {
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
) -> TranscodeTarget {
    let vendor = caps.vendor;

    // AV1 source but no HW AV1 decode → software (dav1d) decode.
    if profile.codec == VideoCodec::Av1 && !caps.hwaccels.iter().any(|h| h == "av1") {
        software_decode = true;
    }

    // DV tone-map requires OpenCL/CUDA; if the host can't, fall back to software
    // decode + software tonemap so playback still works (just on the CPU).
    let dv_tone_map = tone_map && profile.hdr == HdrType::DolbyVision;
    if dv_tone_map && !caps.can_tonemap_dv() {
        software_decode = true;
    }

    // Target H.264 High — the universally direct-playable codec for both TV clients.
    let video_codec = VideoCodec::H264;

    TranscodeTarget {
        vendor,
        software_decode,
        tone_map,
        dv_tone_map,
        video_codec,
        // Audio target is decided by the caller against the client; default copy.
        audio_transcode_to: None,
    }
}

/// Decide the audio target for a transcode/remux: copy through if the client supports
/// the source codec, else convert to the best compatible target.
///
/// For Apple TV, E-AC-3 (Dolby Digital Plus, incl. Atmos JOC) is the richest widely
/// supported target; AAC is the safe stereo fallback. DTS/TrueHD are converted to
/// E-AC-3 to preserve surround where the client supports it, else AAC.
pub fn audio_target(source: AudioCodec, client: &ClientProfile) -> Option<AudioCodec> {
    if client.supports_audio(source) {
        return None; // copy through
    }
    if client.supports_audio(AudioCodec::Eac3) {
        Some(AudioCodec::Eac3)
    } else {
        Some(AudioCodec::Aac)
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
        }
    }

    #[test]
    fn h264_sdr_direct_plays_to_apple_tv() {
        let p = prof(VideoCodec::H264, 8, HdrType::None, None);
        let d = decide(&p, AudioCodec::Aac, "mp4", &ClientProfile::apple_tv_4k(), &hw_intel());
        assert_eq!(d, Decision::DirectPlay { remux: false });
        assert_eq!(d.mode(), "direct");
    }

    #[test]
    fn hevc_hdr10_direct_plays_on_hdr_display() {
        // Apple TV 4K on an HDR display presents HDR10 directly — no transcode.
        let p = prof(VideoCodec::Hevc, 10, HdrType::Hdr10, None);
        let d = decide(&p, AudioCodec::Eac3, "mp4", &ClientProfile::apple_tv_4k(), &hw_intel());
        assert_eq!(d, Decision::DirectPlay { remux: false });
    }

    #[test]
    fn hevc_hdr10_tone_maps_on_sdr_display() {
        let p = prof(VideoCodec::Hevc, 10, HdrType::Hdr10, None);
        let d = decide(&p, AudioCodec::Aac, "mp4", &ClientProfile::sdr_baseline(), &hw_intel());
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
        let d = decide(&p, AudioCodec::Eac3, "mp4", &ClientProfile::apple_tv_4k(), &hw_intel());
        assert_eq!(d, Decision::DirectPlay { remux: false });
    }

    #[test]
    fn dv_p5_tone_maps_on_sdr_display_via_opencl() {
        let p = prof(VideoCodec::Hevc, 10, HdrType::DolbyVision, Some(DvProfile::P5));
        let d = decide(&p, AudioCodec::Aac, "mp4", &ClientProfile::sdr_baseline(), &hw_intel());
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
        let d = decide(&p, AudioCodec::Eac3, "mp4", &client, &hw_intel());
        assert_eq!(d, Decision::DirectPlay { remux: false });
    }

    #[test]
    fn dv_p5_on_sw_host_falls_back_to_software() {
        let p = prof(VideoCodec::Hevc, 10, HdrType::DolbyVision, Some(DvProfile::P5));
        let d = decide(&p, AudioCodec::Aac, "mp4", &ClientProfile::sdr_baseline(), &HwCaps::software_only());
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
        let d = decide(&p, AudioCodec::Aac, "mp4", &ClientProfile::apple_tv_4k(), &hw_intel());
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
        let d = decide(&p, AudioCodec::Aac, "mp4", &ClientProfile::sdr_baseline(), &hw_intel());
        match d {
            Decision::Transcode { target, .. } => {
                assert!(target.software_decode, "no AV1 hwaccel → dav1d software decode");
            }
            _ => panic!("expected transcode"),
        }
    }

    #[test]
    fn mkv_container_forces_remux_even_when_codec_ok() {
        // HEVC SDR video the Apple TV can decode, but in an MKV it can't open → remux.
        let p = prof(VideoCodec::Hevc, 10, HdrType::None, None);
        let d = decide(&p, AudioCodec::Eac3, "mkv", &ClientProfile::apple_tv_4k(), &hw_intel());
        assert_eq!(d, Decision::DirectPlay { remux: true });
    }

    #[test]
    fn dts_audio_forces_remux_with_audio_transcode() {
        let p = prof(VideoCodec::H264, 8, HdrType::None, None);
        let d = decide(&p, AudioCodec::Dts, "mp4", &ClientProfile::apple_tv_4k(), &hw_intel());
        assert_eq!(d, Decision::DirectPlay { remux: true });
        // The audio target for DTS → E-AC-3 on Apple TV.
        assert_eq!(
            audio_target(AudioCodec::Dts, &ClientProfile::apple_tv_4k()),
            Some(AudioCodec::Eac3)
        );
    }

    #[test]
    fn audio_copies_through_when_supported() {
        assert_eq!(audio_target(AudioCodec::Eac3, &ClientProfile::apple_tv_4k()), None);
        assert_eq!(audio_target(AudioCodec::Aac, &ClientProfile::apple_tv_4k()), None);
    }
}
