//! Spawns `ffprobe` via `tokio::process` and parses its JSON into the fields the
//! `media_files` row needs — codec, profile, bit depth, color transfer/space, HDR
//! classification, and the Dolby Vision side-data. Phase 1, sub-task 4.
//!
//! No libav FFI: metadata comes only from the `ffprobe` binary
//! (`docs/.tasks/00-architecture.md`). The exact invocation is the one specified in
//! `docs/.tasks/10-phase1-foundation-data.md`:
//!
//! ```text
//! ffprobe -v quiet -print_format json -show_format -show_streams \
//!         -show_frames -read_intervals '%+#1' <file>
//! ```
//!
//! `-show_frames` with `-read_intervals '%+#1'` reads a single frame at the start so
//! HDR10+ dynamic-metadata side-data is visible without decoding the whole file.
//!
//! ## Dolby Vision (`docs/.tasks/10` §Dolby Vision extraction detail)
//!
//! DV is exposed as a stream `side_data` entry of type *"DOVI configuration record"*
//! carrying `dv_profile`, `dv_bl_signal_compatibility_id`, and `dv_level`. We store
//! all three; the transcode pipeline (Phase 2) reads them:
//! - profile 5 → proprietary IPTPQc2, no fallback → always transcode for SDR;
//! - profile 7 → BL(HDR10)+EL, common in 4K Blu-ray rips;
//! - profile 8 → `bl_compatible_id` 1 = HDR10 fallback, 4 = SDR fallback.

use std::path::Path;
use std::process::Stdio;

use serde::Deserialize;
use tokio::process::Command;

use medi_db::writes::{AudioStreamWrite, MediaFileWrite};

/// The ffprobe binary. `jellyfin-ffmpeg` ships it as `ffprobe` on `PATH` inside the
/// container image (`docs/.tasks/50-phase5`); overridable via `FFPROBE_BIN` for tests
/// and non-standard installs.
fn ffprobe_bin() -> String {
    std::env::var("FFPROBE_BIN").unwrap_or_else(|_| "ffprobe".to_string())
}

/// Errors from probing a single file.
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("failed to spawn ffprobe: {0}")]
    Spawn(#[source] std::io::Error),

    #[error("ffprobe exited with status {status}: {stderr}")]
    NonZeroExit { status: String, stderr: String },

    #[error("could not parse ffprobe JSON: {0}")]
    Parse(#[source] serde_json::Error),

    #[error("no video stream in {0}")]
    NoVideoStream(String),
}

/// Run `ffprobe` on `path` and parse its output into the persistable `media_files` row
/// plus every audio track (`docs/.tasks/70`). The worker writes both in one transaction
/// via `upsert_media_file` + `replace_audio_streams`.
///
/// Runs fully asynchronously on `tokio::process` so it never blocks the runtime; the
/// worker bounds how many run at once with a semaphore.
pub async fn probe(path: &Path) -> Result<(MediaFileWrite, Vec<AudioStreamWrite>), ProbeError> {
    let output = Command::new(ffprobe_bin())
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            "-show_frames",
            "-read_intervals",
            "%+#1",
        ])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(ProbeError::Spawn)?;

    if !output.status.success() {
        return Err(ProbeError::NonZeroExit {
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    let parsed: FfprobeOutput = serde_json::from_slice(&output.stdout).map_err(ProbeError::Parse)?;
    map_output(&parsed).ok_or_else(|| ProbeError::NoVideoStream(path.display().to_string()))
}

/// Convert the parsed ffprobe JSON into the persistable write struct plus its audio
/// tracks. Returns `None` if the file has no video stream (nothing meaningful to catalog).
///
/// Split out from [`probe`] and made `pub(crate)` so it can be unit-tested against
/// fixture JSON without a real `ffprobe` on the machine.
pub(crate) fn map_output(out: &FfprobeOutput) -> Option<(MediaFileWrite, Vec<AudioStreamWrite>)> {
    let video = out
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"))?;

    let container = out
        .format
        .as_ref()
        .and_then(|f| f.format_name.as_deref())
        // ffprobe reports comma-joined names ("mov,mp4,m4a,..."); take the first.
        .map(|n| n.split(',').next().unwrap_or(n).to_string());

    let size_bytes = out
        .format
        .as_ref()
        .and_then(|f| f.size.as_deref())
        .and_then(|s| s.parse::<i64>().ok());

    // Duration is on the format (whole file) in fractional seconds.
    let duration_ms = out
        .format
        .as_ref()
        .and_then(|f| f.duration.as_deref())
        .and_then(|s| s.parse::<f64>().ok())
        .map(|secs| (secs * 1000.0).round() as i64);

    // Bitrate: prefer the video stream's, fall back to the container's.
    let bitrate = video
        .bit_rate
        .as_deref()
        .or(out.format.as_ref().and_then(|f| f.bit_rate.as_deref()))
        .and_then(|s| s.parse::<i64>().ok());

    let video_codec = video.codec_name.clone();
    let video_profile = video.profile.clone();
    let width = video.width;
    let height = video.height;
    let bit_depth = derive_bit_depth(video);

    let transfer_characteristics = video.color_transfer.clone();
    let color_space = video.color_space.clone();

    let dovi = find_dovi(video);
    let hdr_type = classify_hdr(video, &dovi, has_hdr10plus(out));

    let hw_decode_unsupported = is_hw_decode_unsupported(
        video_codec.as_deref(),
        video_profile.as_deref(),
        bit_depth,
    );

    // Collect every audio track (`docs/.tasks/70`). `stream_index` is the absolute
    // ffprobe stream index — what react-native-video's `selectedAudioTrack` selects by.
    let audio_streams: Vec<AudioStreamWrite> = out
        .streams
        .iter()
        .enumerate()
        .filter(|(_, s)| s.codec_type.as_deref() == Some("audio"))
        .map(|(idx, s)| AudioStreamWrite {
            stream_index: idx as i64,
            codec: normalize_audio_codec(s.codec_name.as_deref(), s.profile.as_deref()),
            profile: s.profile.clone(),
            channels: s.channels.map(|c| c as i64),
            channel_layout: s.channel_layout.clone(),
            bitrate: s.bit_rate.as_deref().and_then(|b| b.parse().ok()),
            sample_rate: s.sample_rate.as_deref().and_then(|r| r.parse().ok()),
            language: s.tags.as_ref().and_then(|t| t.language.clone()),
            title: s.tags.as_ref().and_then(|t| t.title.clone()),
            immersive: classify_immersive(s.codec_name.as_deref(), s.profile.as_deref(), s.tags.as_ref())
                .to_string(),
            is_default: s.disposition.as_ref().map(|d| d.default == 1).unwrap_or(false),
        })
        .collect();

    let media = MediaFileWrite {
        container,
        size_bytes,
        duration_ms,
        video_codec,
        video_profile,
        width: width.map(|w| w as i64),
        height: height.map(|h| h as i64),
        bit_depth: bit_depth.map(|b| b as i64),
        bitrate,
        transfer_characteristics,
        color_space,
        hdr_type: Some(hdr_type.as_str().to_string()),
        dv_profile: dovi.as_ref().map(|d| d.dv_profile as i64),
        dv_bl_compatible_id: dovi.as_ref().map(|d| d.bl_compatible_id as i64),
        dv_level: dovi.as_ref().and_then(|d| d.dv_level.map(|l| l as i64)),
        hw_decode_unsupported,
    };

    Some((media, audio_streams))
}

/// Normalize the ffprobe `codec_name` (+ `profile`, which disambiguates DTS) to the
/// `audio_streams.codec` string the transcode decision reads back (`docs/.tasks/70`).
///
/// The DTS split is the load-bearing one: a plain `dts` core stays `dts` (lossy), but a
/// `DTS-HD MA` / `DTS-HD High` profile becomes `dtshd` (lossless) so only a
/// passthrough-capable sink (Shield) copies it while Apple TV re-encodes.
fn normalize_audio_codec(codec: Option<&str>, profile: Option<&str>) -> Option<String> {
    let codec = codec?;
    let prof = profile.unwrap_or("");
    let name = match codec {
        "aac" => "aac",
        "ac3" => "ac3",
        "eac3" => "eac3",
        "truehd" => "truehd",
        "flac" => "flac",
        "opus" => "opus",
        "pcm_s16le" | "pcm_s24le" | "pcm_s32le" | "pcm_f32le" | "pcm_bluray" | "pcm_dvd" => "pcm",
        "dts" => {
            if prof.contains("DTS-HD MA") || prof.contains("DTS-HD High") {
                "dtshd"
            } else {
                "dts"
            }
        }
        _ if codec.starts_with("pcm") => "pcm",
        _ => "other",
    };
    Some(name.to_string())
}

/// Classify the immersive-audio marker from `codec_name` + `profile` + stream tags
/// (`docs/.tasks/70`). The `profile`/tag substrings are what jellyfin-ffmpeg surfaces:
/// TrueHD Atmos and E-AC-3 JOC both mark `dolby_atmos`; DTS:X marks `dts_x`.
fn classify_immersive(
    codec: Option<&str>,
    profile: Option<&str>,
    tags: Option<&Tags>,
) -> ImmersiveLabel {
    let prof = profile.unwrap_or("");
    let title = tags.and_then(|t| t.title.as_deref()).unwrap_or("");
    let mentions = |needle: &str| {
        prof.to_ascii_lowercase().contains(&needle.to_ascii_lowercase())
            || title.to_ascii_lowercase().contains(&needle.to_ascii_lowercase())
    };
    match codec {
        // TrueHD or E-AC-3 carrying Atmos (JOC) → Dolby Atmos. Apple TV can pass the
        // *lossy* E-AC-3 JOC form through; the TrueHD form is lossless (Shield only).
        Some("truehd") | Some("eac3") if mentions("Atmos") || mentions("JOC") => {
            ImmersiveLabel::DolbyAtmos
        }
        Some("dts") if prof.contains("DTS:X") || prof.contains("DTS-X") => ImmersiveLabel::DtsX,
        _ => ImmersiveLabel::None,
    }
}

/// The `audio_streams.immersive` string values (kept as an enum so the classification is
/// exhaustive and the strings live in one place).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImmersiveLabel {
    None,
    DolbyAtmos,
    DtsX,
}

impl ImmersiveLabel {
    fn as_str(self) -> &'static str {
        match self {
            ImmersiveLabel::None => "none",
            ImmersiveLabel::DolbyAtmos => "dolby_atmos",
            ImmersiveLabel::DtsX => "dts_x",
        }
    }
}

impl std::fmt::Display for ImmersiveLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Bit depth: ffprobe exposes it directly as `bits_per_raw_sample`, but that is often
/// absent; fall back to inferring from the pixel format (`yuv420p10le` → 10).
fn derive_bit_depth(video: &Stream) -> Option<u8> {
    if let Some(bits) = video
        .bits_per_raw_sample
        .as_deref()
        .and_then(|s| s.parse::<u8>().ok())
    {
        return Some(bits);
    }
    let pix = video.pix_fmt.as_deref()?;
    if pix.contains("p10") {
        Some(10)
    } else if pix.contains("p12") {
        Some(12)
    } else {
        Some(8)
    }
}

/// The Dolby Vision configuration record, if present on the video stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Dovi {
    pub dv_profile: u8,
    pub bl_compatible_id: u8,
    pub dv_level: Option<u8>,
}

/// Locate the "DOVI configuration record" side-data on the video stream and read its
/// profile / base-layer compatibility id / level.
fn find_dovi(video: &Stream) -> Option<Dovi> {
    let sd = video.side_data_list.iter().find(|sd| {
        sd.side_data_type
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case("DOVI configuration record"))
            || sd.dv_profile.is_some()
    })?;
    let dv_profile = sd.dv_profile?;
    Some(Dovi {
        dv_profile,
        // `dv_bl_signal_compatibility_id` is the field name; default 0 when absent.
        bl_compatible_id: sd.dv_bl_signal_compatibility_id.unwrap_or(0),
        dv_level: sd.dv_level,
    })
}

/// Does any read frame carry HDR10+ dynamic metadata (SMPTE ST 2094-40)? ffprobe
/// surfaces it as frame side-data of that type.
fn has_hdr10plus(out: &FfprobeOutput) -> bool {
    out.frames.iter().any(|f| {
        f.side_data_list.iter().any(|sd| {
            sd.side_data_type
                .as_deref()
                .is_some_and(|t| t.contains("HDR Dynamic Metadata") || t.contains("2094"))
        })
    })
}

/// Classify the HDR type from color transfer + DV presence + HDR10+ metadata.
///
/// Priority mirrors the `LibraryCard` ranking: Dolby Vision beats HDR10+, which beats
/// HDR10, which beats HLG. A PQ (`smpte2084`) transfer with no dynamic metadata is
/// plain HDR10; `arib-std-b67` is HLG; anything else is SDR (`none`).
fn classify_hdr(video: &Stream, dovi: &Option<Dovi>, hdr10plus: bool) -> HdrLabel {
    if dovi.is_some() {
        return HdrLabel::DolbyVision;
    }
    let transfer = video.color_transfer.as_deref().unwrap_or("");
    match transfer {
        "smpte2084" if hdr10plus => HdrLabel::Hdr10Plus,
        "smpte2084" => HdrLabel::Hdr10,
        "arib-std-b67" => HdrLabel::Hlg,
        _ => HdrLabel::None,
    }
}

/// The `hdr_type` string values stored in `media_files` (kept as an enum so the
/// classification is exhaustive and the strings live in one place).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HdrLabel {
    None,
    Hdr10,
    Hdr10Plus,
    Hlg,
    DolbyVision,
}

impl HdrLabel {
    fn as_str(self) -> &'static str {
        match self {
            HdrLabel::None => "none",
            HdrLabel::Hdr10 => "hdr10",
            HdrLabel::Hdr10Plus => "hdr10plus",
            HdrLabel::Hlg => "hlg",
            HdrLabel::DolbyVision => "dolbyvision",
        }
    }
}

/// Whether hardware decoders cannot handle this format, forcing a software-decode
/// transcode path. Phase 1 flags the known case from `docs/.tasks/20`: **H.264 High
/// 10** (10-bit AVC) is unsupported by essentially every consumer HW decoder.
fn is_hw_decode_unsupported(
    codec: Option<&str>,
    profile: Option<&str>,
    bit_depth: Option<u8>,
) -> bool {
    let is_h264 = matches!(codec, Some("h264"));
    let is_high10 = profile.is_some_and(|p| p.eq_ignore_ascii_case("High 10"))
        || (is_h264 && bit_depth == Some(10));
    is_h264 && is_high10
}

// ---------------------------------------------------------------------------
// ffprobe JSON shapes — only the fields we consume are declared; serde ignores
// the rest. Numeric-looking fields ffprobe emits as JSON strings ("1920"?) are
// kept typed where ffprobe uses real numbers and as strings where it uses text.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct FfprobeOutput {
    #[serde(default)]
    pub streams: Vec<Stream>,
    #[serde(default)]
    pub frames: Vec<Frame>,
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Stream {
    pub codec_type: Option<String>,
    pub codec_name: Option<String>,
    pub profile: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub pix_fmt: Option<String>,
    /// ffprobe emits this as a string ("10").
    pub bits_per_raw_sample: Option<String>,
    pub color_transfer: Option<String>,
    pub color_space: Option<String>,
    /// ffprobe emits bit rates as strings ("8000000").
    pub bit_rate: Option<String>,
    // --- audio fields (Task 70) ---
    /// Channel count — ffprobe emits this as a real number.
    pub channels: Option<u32>,
    pub channel_layout: Option<String>,
    /// ffprobe emits sample rate as a string ("48000").
    pub sample_rate: Option<String>,
    pub tags: Option<Tags>,
    pub disposition: Option<Disposition>,
    #[serde(default)]
    pub side_data_list: Vec<SideData>,
}

/// Stream tags ffprobe emits (`-show_streams`). We read the language + title, used for
/// the audio track list and immersive classification.
#[derive(Debug, Deserialize)]
pub(crate) struct Tags {
    pub language: Option<String>,
    pub title: Option<String>,
}

/// Stream disposition flags. `default == 1` marks the default audio track the transcode
/// decision feeds into `decide()`.
#[derive(Debug, Deserialize)]
pub(crate) struct Disposition {
    #[serde(default)]
    pub default: i64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Frame {
    #[serde(default)]
    pub side_data_list: Vec<SideData>,
}

/// A side-data entry on a stream or frame. The DV record lives on the stream and
/// carries the `dv_*` fields; HDR10+ metadata lives on a frame and is identified by
/// `side_data_type` alone.
#[derive(Debug, Deserialize)]
pub(crate) struct SideData {
    pub side_data_type: Option<String>,
    pub dv_profile: Option<u8>,
    pub dv_bl_signal_compatibility_id: Option<u8>,
    pub dv_level: Option<u8>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Format {
    pub format_name: Option<String>,
    pub duration: Option<String>,
    pub size: Option<String>,
    pub bit_rate: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> MediaFileWrite {
        let out: FfprobeOutput = serde_json::from_str(json).unwrap();
        map_output(&out).expect("has a video stream").0
    }

    fn parse_audio(json: &str) -> Vec<AudioStreamWrite> {
        let out: FfprobeOutput = serde_json::from_str(json).unwrap();
        map_output(&out).expect("has a video stream").1
    }

    #[test]
    fn hevc_hdr10() {
        let w = parse(
            r#"{
              "streams": [{
                "codec_type": "video", "codec_name": "hevc", "profile": "Main 10",
                "width": 3840, "height": 2160, "pix_fmt": "yuv420p10le",
                "color_transfer": "smpte2084", "color_space": "bt2020nc"
              }],
              "format": { "format_name": "matroska,webm", "duration": "7200.5",
                          "size": "40000000000", "bit_rate": "44000000" }
            }"#,
        );
        assert_eq!(w.video_codec.as_deref(), Some("hevc"));
        assert_eq!(w.bit_depth, Some(10));
        assert_eq!(w.hdr_type.as_deref(), Some("hdr10"));
        assert_eq!(w.container.as_deref(), Some("matroska"));
        assert_eq!(w.duration_ms, Some(7_200_500));
        assert_eq!(w.dv_profile, None);
        assert!(!w.hw_decode_unsupported);
    }

    #[test]
    fn dolby_vision_profile_5() {
        let w = parse(
            r#"{
              "streams": [{
                "codec_type": "video", "codec_name": "hevc", "profile": "Main 10",
                "width": 3840, "height": 2160, "pix_fmt": "yuv420p10le",
                "color_transfer": "smpte2084", "color_space": "bt2020nc",
                "side_data_list": [{
                  "side_data_type": "DOVI configuration record",
                  "dv_profile": 5, "dv_bl_signal_compatibility_id": 0, "dv_level": 6
                }]
              }],
              "format": { "format_name": "matroska,webm" }
            }"#,
        );
        assert_eq!(w.hdr_type.as_deref(), Some("dolbyvision"));
        assert_eq!(w.dv_profile, Some(5));
        assert_eq!(w.dv_bl_compatible_id, Some(0));
        assert_eq!(w.dv_level, Some(6));
    }

    #[test]
    fn dolby_vision_profile_8_hdr10_fallback() {
        let w = parse(
            r#"{
              "streams": [{
                "codec_type": "video", "codec_name": "hevc", "profile": "Main 10",
                "width": 3840, "height": 2160,
                "color_transfer": "smpte2084",
                "side_data_list": [{
                  "side_data_type": "DOVI configuration record",
                  "dv_profile": 8, "dv_bl_signal_compatibility_id": 1, "dv_level": 9
                }]
              }],
              "format": {}
            }"#,
        );
        assert_eq!(w.dv_profile, Some(8));
        assert_eq!(w.dv_bl_compatible_id, Some(1));
    }

    #[test]
    fn hdr10plus_detected_from_frame_side_data() {
        let w = parse(
            r#"{
              "streams": [{
                "codec_type": "video", "codec_name": "hevc",
                "width": 3840, "height": 2160, "color_transfer": "smpte2084"
              }],
              "frames": [{
                "side_data_list": [{ "side_data_type": "HDR Dynamic Metadata SMPTE2094-40" }]
              }],
              "format": {}
            }"#,
        );
        assert_eq!(w.hdr_type.as_deref(), Some("hdr10plus"));
    }

    #[test]
    fn hlg_from_transfer() {
        let w = parse(
            r#"{
              "streams": [{
                "codec_type": "video", "codec_name": "hevc",
                "width": 1920, "height": 1080, "color_transfer": "arib-std-b67"
              }],
              "format": {}
            }"#,
        );
        assert_eq!(w.hdr_type.as_deref(), Some("hlg"));
    }

    #[test]
    fn h264_high10_flags_hw_decode_unsupported() {
        let w = parse(
            r#"{
              "streams": [{
                "codec_type": "video", "codec_name": "h264", "profile": "High 10",
                "width": 1920, "height": 1080, "pix_fmt": "yuv420p10le"
              }],
              "format": {}
            }"#,
        );
        assert!(w.hw_decode_unsupported);
        assert_eq!(w.bit_depth, Some(10));
        assert_eq!(w.hdr_type.as_deref(), Some("none"));
    }

    #[test]
    fn plain_h264_sdr_is_supported() {
        let w = parse(
            r#"{
              "streams": [{
                "codec_type": "video", "codec_name": "h264", "profile": "High",
                "width": 1920, "height": 1080, "pix_fmt": "yuv420p"
              }],
              "format": { "format_name": "mov,mp4,m4a,3gp,3g2,mj2" }
            }"#,
        );
        assert!(!w.hw_decode_unsupported);
        assert_eq!(w.bit_depth, Some(8));
        assert_eq!(w.container.as_deref(), Some("mov"));
    }

    #[test]
    fn av1_hdr10() {
        let w = parse(
            r#"{
              "streams": [{
                "codec_type": "video", "codec_name": "av1", "profile": "Main",
                "width": 3840, "height": 2160, "pix_fmt": "yuv420p10le",
                "color_transfer": "smpte2084"
              }],
              "format": {}
            }"#,
        );
        assert_eq!(w.video_codec.as_deref(), Some("av1"));
        assert_eq!(w.hdr_type.as_deref(), Some("hdr10"));
        assert!(!w.hw_decode_unsupported);
    }

    #[test]
    fn no_video_stream_is_none() {
        let out: FfprobeOutput = serde_json::from_str(
            r#"{ "streams": [{ "codec_type": "audio", "codec_name": "aac" }], "format": {} }"#,
        )
        .unwrap();
        assert!(map_output(&out).is_none());
    }

    // --- audio parsing (Task 70) --------------------------------------------

    #[test]
    fn truehd_atmos_and_dtshd_classified() {
        // A file with three audio tracks: TrueHD Atmos (default), DTS-HD MA, AAC stereo.
        let streams = parse_audio(
            r#"{
              "streams": [
                { "codec_type": "video", "codec_name": "hevc", "width": 3840, "height": 2160 },
                { "codec_type": "audio", "codec_name": "truehd", "profile": "Dolby TrueHD + Dolby Atmos",
                  "channels": 8, "channel_layout": "7.1", "sample_rate": "48000",
                  "tags": { "language": "eng", "title": "TrueHD Atmos" },
                  "disposition": { "default": 1 } },
                { "codec_type": "audio", "codec_name": "dts", "profile": "DTS-HD MA",
                  "channels": 6, "channel_layout": "5.1", "sample_rate": "48000",
                  "tags": { "language": "eng" }, "disposition": { "default": 0 } },
                { "codec_type": "audio", "codec_name": "aac", "profile": "LC",
                  "channels": 2, "channel_layout": "stereo", "bit_rate": "256000",
                  "tags": { "language": "eng", "title": "Commentary" } }
              ],
              "format": {}
            }"#,
        );
        assert_eq!(streams.len(), 3);

        // Track 0: TrueHD + Atmos, 7.1, default, at absolute stream_index 1.
        assert_eq!(streams[0].stream_index, 1);
        assert_eq!(streams[0].codec.as_deref(), Some("truehd"));
        assert_eq!(streams[0].channels, Some(8));
        assert_eq!(streams[0].channel_layout.as_deref(), Some("7.1"));
        assert_eq!(streams[0].immersive, "dolby_atmos");
        assert!(streams[0].is_default);
        assert_eq!(streams[0].language.as_deref(), Some("eng"));

        // Track 1: DTS-HD MA → dtshd (lossless), 5.1, not default.
        assert_eq!(streams[1].stream_index, 2);
        assert_eq!(streams[1].codec.as_deref(), Some("dtshd"));
        assert_eq!(streams[1].channels, Some(6));
        assert_eq!(streams[1].immersive, "none");
        assert!(!streams[1].is_default);

        // Track 2: AAC stereo, bitrate + title captured.
        assert_eq!(streams[2].codec.as_deref(), Some("aac"));
        assert_eq!(streams[2].bitrate, Some(256000));
        assert_eq!(streams[2].title.as_deref(), Some("Commentary"));
    }

    #[test]
    fn eac3_joc_is_lossy_atmos() {
        let streams = parse_audio(
            r#"{
              "streams": [
                { "codec_type": "video", "codec_name": "h264", "width": 1920, "height": 1080 },
                { "codec_type": "audio", "codec_name": "eac3", "profile": "Dolby Digital Plus + Dolby Atmos",
                  "channels": 6, "channel_layout": "5.1" }
              ],
              "format": {}
            }"#,
        );
        assert_eq!(streams[0].codec.as_deref(), Some("eac3"));
        assert_eq!(streams[0].immersive, "dolby_atmos");
    }

    #[test]
    fn dts_core_and_dtsx() {
        let streams = parse_audio(
            r#"{
              "streams": [
                { "codec_type": "video", "codec_name": "h264", "width": 1920, "height": 1080 },
                { "codec_type": "audio", "codec_name": "dts", "profile": "DTS", "channels": 6 },
                { "codec_type": "audio", "codec_name": "dts", "profile": "DTS-HD MA + DTS:X", "channels": 8 }
              ],
              "format": {}
            }"#,
        );
        // Plain DTS core stays lossy `dts`.
        assert_eq!(streams[0].codec.as_deref(), Some("dts"));
        assert_eq!(streams[0].immersive, "none");
        // DTS-HD MA base carrying DTS:X → dtshd + dts_x immersive marker.
        assert_eq!(streams[1].codec.as_deref(), Some("dtshd"));
        assert_eq!(streams[1].immersive, "dts_x");
    }

    #[test]
    fn no_audio_streams_is_empty() {
        let streams = parse_audio(
            r#"{
              "streams": [{ "codec_type": "video", "codec_name": "h264", "width": 1920, "height": 1080 }],
              "format": {}
            }"#,
        );
        assert!(streams.is_empty());
    }
}
