//! Builds the concrete jellyfin-ffmpeg argument vector for a chosen pipeline
//! (`docs/.tasks/20` §Vendor command sketches, sub-task 3).
//!
//! Given a [`TranscodeTarget`] (from `decision.rs`), the input path, and the output
//! directory, [`build_argv`] assembles the full argv — `init_hw_device` chaining,
//! `-hwaccel`, the filter graph (VPP for HDR10→SDR, OpenCL/CUDA for Dolby Vision), the
//! encoder, audio handling, and the **fMP4/CMAF HLS** muxer.
//!
//! ## Apple TV output: fragmented-MP4 HLS (CMAF)
//!
//! Output is HLS with `-hls_segment_type fmp4` (an `init.mp4` + `.m4s` segments), not
//! legacy MPEG-TS. tvOS AVPlayer requires fMP4 to carry HEVC/HDR and gets cleaner
//! seeking and CMAF-compatibility from it; TS cannot reliably carry HEVC on tvOS. H.264
//! plays fine in fMP4 too, so we use one packaging for every client.
//!
//! The exact filter/flag names depend on the installed jellyfin-ffmpeg build; the fixed
//! *rules* (DV → OpenCL/CUDA not plain VPP; fMP4 output; `init_hw_device` chaining for
//! the DV path) are what this module encodes. Validate flags against the binary during
//! bring-up (`docs/.tasks/20` note).

use std::path::Path;

use medi_core::{AudioCodec, VideoCodec};

use crate::decision::{TranscodeTarget, Vendor};

/// Names of the generated HLS artifacts, shared with `session.rs` and the `/api/hls`
/// route so the playlist/segment/init filenames stay in one place.
pub const PLAYLIST_NAME: &str = "index.m3u8";
pub const INIT_NAME: &str = "init.mp4";
/// `%d`-templated segment name (fMP4 → `.m4s`).
pub const SEGMENT_TEMPLATE: &str = "seg%05d.m4s";

/// Target HLS segment length, seconds. ~4s balances tvOS start-up latency against
/// playlist churn for a long movie. Segment `N` covers `[N*SEGMENT_SECONDS, …)`; the server
/// synthesizes the VOD playlist from this + the media duration so the whole timeline is
/// seekable before any byte is transcoded.
pub const SEGMENT_SECONDS: u32 = 4;

/// Build the full jellyfin-ffmpeg argv for `target`, reading `input` and writing the
/// HLS playlist + fMP4 segments into `out_dir`.
///
/// The returned `Vec<String>` is the argument list **after** the program name; the
/// caller (`session.rs`) spawns `Command::new(ffmpeg_bin).args(argv)`.
pub fn build_argv(
    target: &TranscodeTarget,
    audio: AudioTarget,
    input: &Path,
    out_dir: &Path,
    render_node: Option<&str>,
    start_segment: u32,
) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let push = |a: &mut Vec<String>, s: &str| a.push(s.to_string());

    push(&mut a, "-hide_banner");
    push(&mut a, "-loglevel");
    push(&mut a, "warning");
    // Never let a hung transcode block forever; the session layer also enforces idle
    // teardown, but this bounds a single stalled read.
    push(&mut a, "-nostdin");

    // --- Seek to the start segment (VOD seek support). -----------------------
    // For a seek, start decoding at the segment's time so this ffmpeg produces segments from
    // there onward. `-ss` BEFORE `-i` is the fast, keyframe-accurate input seek. Segment N
    // starts at N*SEGMENT_SECONDS; `-start_number N` makes the emitted files `segN..`, so they
    // line up with the synthesized VOD playlist's URLs no matter where we started.
    let start_time = start_segment * SEGMENT_SECONDS;
    if start_time > 0 {
        push(&mut a, "-ss");
        a.push(start_time.to_string());
    }

    // --- Hardware device init + decode. --------------------------------------
    let node = render_node.unwrap_or("/dev/dri/renderD128");
    let hw = HwPlan::for_target(target);
    hw.emit_init_and_decode(&mut a, node);

    // --- Input. --------------------------------------------------------------
    push(&mut a, "-i");
    a.push(input.to_string_lossy().into_owned());

    // --- Keyframe-aligned segments (seekability). ----------------------------
    // Force a keyframe at every segment boundary so each fMP4 segment is independently
    // decodable and seeks land cleanly on a segment start. `n_forced` counts forced frames
    // from THIS process's start, and its `t` is relative to the post-`-ss` timeline, so the
    // expression is the same whether we started at 0 or mid-file.
    push(&mut a, "-force_key_frames");
    a.push(format!("expr:gte(t,n_forced*{SEGMENT_SECONDS})"));

    // --- Video filter graph + encoder. ---------------------------------------
    // Image-subtitle burn-in (`docs/.tasks/90` §5) needs a `-filter_complex` so the
    // subtitle can be a second input to `overlay`, and the overlay MUST run **after** any
    // HDR→SDR tone-map (else the burned subtitle would be tone-mapped/washed out). A plain
    // transcode (no burn-in) keeps the simpler `-vf` path.
    if let Some(sub_idx) = target.subtitle_burn_in {
        let base = hw.filter_graph(target);
        let fc = build_burn_in_filter(base.as_deref(), sub_idx);
        push(&mut a, "-filter_complex");
        a.push(fc);
        push(&mut a, "-map");
        push(&mut a, "[v]");
        // With an explicit video map, the audio stream must be mapped too: the selected
        // source track (`0:a:<n>`) when one was chosen, else the default first audio.
        push(&mut a, "-map");
        a.push(format!("0:a:{}?", audio.source().unwrap_or(0)));
    } else {
        if let Some(vf) = hw.filter_graph(target) {
            push(&mut a, "-vf");
            a.push(vf);
        }
        // On the normal (non-burn-in) path ffmpeg maps the default first audio stream on its
        // own. Emit an explicit `-map` only when a specific source track was selected (an
        // in-player audio switch): `0:v` keeps the video, `0:a:<n>` picks the chosen track.
        if let Some(src) = audio.source() {
            push(&mut a, "-map");
            push(&mut a, "0:v");
            push(&mut a, "-map");
            a.push(format!("0:a:{src}?"));
        }
    }
    push(&mut a, "-c:v");
    push(&mut a, hw.video_encoder(target));
    // Quality-targeted VBR: quality param name differs per encoder.
    for arg in hw.quality_args(target) {
        a.push(arg);
    }
    // QualityProfile::Capped ceiling (`docs/.tasks/70`): bound the output bitrate with a
    // VBV so the stream stays under the client's `MaxStreamingBitrate`. `-bufsize` is a
    // conventional 2× the ceiling.
    if let Some(cap) = target.max_bitrate {
        push(&mut a, "-maxrate");
        a.push(cap.to_string());
        push(&mut a, "-bufsize");
        a.push((cap.saturating_mul(2)).to_string());
    }
    // SDR output after tone-mapping should carry BT.709 tags so the client renders it
    // correctly rather than assuming BT.2020.
    if target.tone_map {
        for t in [
            "-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709",
        ] {
            push(&mut a, t);
        }
    }

    // --- Audio. --------------------------------------------------------------
    match audio {
        AudioTarget::Copy { .. } => {
            push(&mut a, "-c:a");
            push(&mut a, "copy");
        }
        AudioTarget::Transcode { codec, channels, .. } => {
            push(&mut a, "-c:a");
            push(&mut a, audio_encoder(codec));
            push(&mut a, "-b:a");
            push(&mut a, audio_bitrate(codec));
            // Channel count comes from the resolved `AudioPlan` (downmix-aware), not a
            // hard-coded 2/6 (`docs/.tasks/70`).
            push(&mut a, "-ac");
            a.push(channels.max(1).to_string());
        }
    }

    // --- fMP4/CMAF HLS muxer. -------------------------------------------------
    emit_hls_muxer(&mut a, out_dir, start_segment);

    a
}

/// How to handle the audio: whether to copy or re-encode (and to how many channels,
/// resolved by the caller from the source track + client profile via `decision::audio_plan`),
/// plus **which** source audio track to take (`docs/.tasks/97` Part C).
///
/// `source` is the *audio-relative* index (`0:a:<n>` — the track's position among the file's
/// audio streams, ordered by `stream_index`), or `None` to let ffmpeg pick its default (the
/// first audio stream). A non-`None` `source` is what an in-player audio-track switch sets so
/// the chosen track — not the default — is mapped into the output. It is part of the
/// session-key fingerprint (via `AudioTarget`'s `Debug`), so switching tracks spawns a
/// distinct transcode session automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioTarget {
    Copy { source: Option<u32> },
    Transcode { codec: AudioCodec, channels: u8, source: Option<u32> },
}

impl AudioTarget {
    /// The audio-relative source index (`0:a:<n>`) this target selects, if any.
    fn source(&self) -> Option<u32> {
        match self {
            AudioTarget::Copy { source } | AudioTarget::Transcode { source, .. } => *source,
        }
    }
}

/// The HW pipeline plan derived from a [`TranscodeTarget`]: which device to init, how
/// to decode, the tone-map filter chain, and the encoder. Keeps the vendor branching in
/// one place so `build_argv` reads top-to-bottom.
enum HwPlan {
    /// Fully software: no `-hwaccel`, libx264, software (zscale/tonemap) filters.
    Software,
    /// Intel QSV/VA-API. `opencl` = chain an OpenCL device for Dolby Vision tone-map.
    Intel { opencl: bool },
    /// NVIDIA NVDEC/NVENC, CUDA filters for tone-map.
    Nvidia,
    /// AMD VA-API + OpenCL for Dolby Vision tone-map.
    Amd { opencl: bool },
}

impl HwPlan {
    fn for_target(t: &TranscodeTarget) -> Self {
        if t.software_decode || t.vendor.is_none() {
            return HwPlan::Software;
        }
        match t.vendor {
            Some(Vendor::Intel) => HwPlan::Intel { opencl: t.dv_tone_map },
            Some(Vendor::Nvidia) => HwPlan::Nvidia,
            Some(Vendor::Amd) => HwPlan::Amd { opencl: t.dv_tone_map },
            None => HwPlan::Software,
        }
    }

    /// Emit the `-init_hw_device` / `-hwaccel` prefix (before `-i`).
    fn emit_init_and_decode(&self, a: &mut Vec<String>, node: &str) {
        let p = |a: &mut Vec<String>, s: &str| a.push(s.to_string());
        match self {
            HwPlan::Software => {
                // No HW decode; dav1d/libx264 handled by ffmpeg defaults.
            }
            HwPlan::Intel { opencl } => {
                if *opencl {
                    // DV path: VA-API device + an OpenCL device derived from it, so the
                    // OpenCL tone-map and the QSV encode share the same frames.
                    p(a, "-init_hw_device");
                    a.push(format!("vaapi=va:{node}"));
                    p(a, "-init_hw_device");
                    p(a, "opencl@va=ocl");
                    p(a, "-filter_hw_device");
                    p(a, "ocl");
                } else {
                    // HDR10→SDR / plain transcode: QSV device + QSV decode.
                    p(a, "-init_hw_device");
                    a.push(format!("qsv=qs:{node}"));
                    p(a, "-filter_hw_device");
                    p(a, "qs");
                    p(a, "-hwaccel");
                    p(a, "qsv");
                    p(a, "-hwaccel_output_format");
                    p(a, "qsv");
                }
            }
            HwPlan::Nvidia => {
                p(a, "-init_hw_device");
                p(a, "cuda=cu");
                p(a, "-filter_hw_device");
                p(a, "cu");
                p(a, "-hwaccel");
                p(a, "cuda");
                p(a, "-hwaccel_output_format");
                p(a, "cuda");
            }
            HwPlan::Amd { opencl } => {
                p(a, "-init_hw_device");
                a.push(format!("vaapi=va:{node}"));
                if *opencl {
                    p(a, "-init_hw_device");
                    p(a, "opencl@va=ocl");
                    p(a, "-filter_hw_device");
                    p(a, "ocl");
                } else {
                    p(a, "-filter_hw_device");
                    p(a, "va");
                    p(a, "-hwaccel");
                    p(a, "vaapi");
                    p(a, "-hwaccel_output_format");
                    p(a, "vaapi");
                }
            }
        }
    }

    /// The `-vf` filter graph, or `None` when no filtering is needed (plain HW
    /// transcode of an SDR source).
    fn filter_graph(&self, t: &TranscodeTarget) -> Option<String> {
        if !t.tone_map {
            // No tone-map. HW paths still need the frame on the right device for the
            // encoder; ffmpeg handles that implicitly for same-device decode→encode, so
            // no filter is required.
            return None;
        }

        Some(match self {
            HwPlan::Software => {
                // CPU tone-map via zscale + tonemap (works for DV too, just slow).
                "zscale=t=linear:npl=100,format=gbrpf32le,\
                 zscale=p=bt709,tonemap=tonemap=hable:desat=0,\
                 zscale=t=bt709:m=bt709:r=tv,format=yuv420p"
                    .to_string()
            }
            HwPlan::Intel { opencl } | HwPlan::Amd { opencl } => {
                if *opencl {
                    // Dolby Vision: upload to OpenCL, tone-map (bt2390) to BT.709,
                    // download, re-upload to QSV/VA-API for the encoder.
                    "hwupload,\
                     tonemap_opencl=tonemap=bt2390:transfer=bt709:matrix=bt709:primaries=bt709:format=nv12,\
                     hwdownload,format=nv12,hwupload=derive_device=qsv"
                        .to_string()
                } else {
                    // HDR10 → SDR via Intel VPP tone-map.
                    "vpp_qsv=tonemap=1:format=nv12".to_string()
                }
            }
            HwPlan::Nvidia => {
                // CUDA tone-map for both HDR10 and Dolby Vision. The source is decoded via
                // NVDEC with `-hwaccel_output_format cuda`, so frames are ALREADY CUDA frames
                // entering the graph — do NOT `hwupload_cuda` (uploading an already-uploaded
                // frame fails the graph with "Error reinitializing filters! ... -38 Function
                // not implemented"). `tonemap_cuda` outputs nv12 directly (`:format=nv12`) so
                // no extra `scale_cuda` is needed; nvenc consumes the CUDA nv12 frames.
                "tonemap_cuda=tonemap=bt2390:transfer=bt709:matrix=bt709:primaries=bt709:format=nv12"
                    .to_string()
            }
        })
    }

    /// The encoder name for the target video codec on this plan.
    fn video_encoder(&self, t: &TranscodeTarget) -> &'static str {
        match (self, t.video_codec) {
            (HwPlan::Software, VideoCodec::H264) => "libx264",
            (HwPlan::Software, _) => "libx265",
            (HwPlan::Intel { .. }, VideoCodec::H264) => "h264_qsv",
            (HwPlan::Intel { .. }, _) => "hevc_qsv",
            (HwPlan::Nvidia, VideoCodec::H264) => "h264_nvenc",
            (HwPlan::Nvidia, _) => "hevc_nvenc",
            (HwPlan::Amd { .. }, VideoCodec::H264) => "h264_vaapi",
            (HwPlan::Amd { .. }, _) => "hevc_vaapi",
        }
    }

    /// Quality-targeted rate-control args (the flag name is encoder-specific).
    fn quality_args(&self, _t: &TranscodeTarget) -> Vec<String> {
        let q = |k: &str, v: &str| vec![k.to_string(), v.to_string()];
        match self {
            HwPlan::Software => q("-crf", "21"),
            HwPlan::Intel { .. } => q("-global_quality", "23"),
            HwPlan::Nvidia => {
                let mut v = q("-rc", "vbr");
                v.extend(q("-cq", "23"));
                v
            }
            HwPlan::Amd { .. } => q("-qp", "23"),
        }
    }
}

/// Build the `-filter_complex` graph that burns image subtitle `sub_idx` (a *subtitle-
/// relative* index, i.e. `0:s:<sub_idx>`) into the video, after any base filter chain
/// (`docs/.tasks/90` §5).
///
/// - With a base chain (a tone-map): `[0:v]<base>[base];[base][0:s:N]overlay[v]` — the
///   overlay runs **after** the tone-map so the subtitle is not tone-mapped/washed out.
/// - Without one (plain transcode + burn-in): `[0:v][0:s:N]overlay[v]`.
fn build_burn_in_filter(base: Option<&str>, sub_idx: i64) -> String {
    match base {
        Some(chain) => {
            format!("[0:v]{chain}[base];[base][0:s:{sub_idx}]overlay[v]")
        }
        None => format!("[0:v][0:s:{sub_idx}]overlay[v]"),
    }
}

/// Append the fragmented-MP4 HLS muxer flags. `start_segment` makes ffmpeg number its
/// segment files from that index (so a seek-started process writes `segN..` that line up with
/// the server-synthesized VOD playlist). The playlist ffmpeg writes here is a throwaway —
/// the api layer serves its OWN complete `#EXT-X-PLAYLIST-TYPE:VOD` playlist (built from the
/// media duration) so the whole timeline is seekable before any segment is transcoded.
fn emit_hls_muxer(a: &mut Vec<String>, out_dir: &Path, start_segment: u32) {
    let p = |a: &mut Vec<String>, s: &str| a.push(s.to_string());
    p(a, "-f");
    p(a, "hls");
    p(a, "-hls_time");
    a.push(SEGMENT_SECONDS.to_string());
    // Number emitted segments from `start_segment` so they match the VOD playlist's URLs
    // regardless of where this ffmpeg started (a fresh start=0, a seek start=N).
    p(a, "-start_number");
    a.push(start_segment.to_string());
    // Write segments atomically (`temp_file`) so the client never fetches a half-written one,
    // and mark them independently decodable so a seek into any segment plays.
    p(a, "-hls_flags");
    p(a, "temp_file+independent_segments");
    // Fragmented-MP4 (CMAF) segments — required by tvOS AVPlayer for HEVC/HDR.
    p(a, "-hls_segment_type");
    p(a, "fmp4");
    p(a, "-hls_fmp4_init_filename");
    p(a, INIT_NAME);
    p(a, "-hls_segment_filename");
    a.push(out_dir.join(SEGMENT_TEMPLATE).to_string_lossy().into_owned());
    // No `-hls_list_size` cap: keep all segments. The written playlist is ignored by the api
    // layer (it serves a synthesized VOD one), so its name is a throwaway.
    p(a, "-hls_list_size");
    p(a, "0");
    a.push(out_dir.join("ffmpeg.m3u8").to_string_lossy().into_owned());
}

/// Synthesize the complete VOD HLS playlist for a title of `duration_ms`, listing every
/// segment up front so the client sees the full runtime and can seek anywhere immediately.
///
/// Segment `N` covers `[N*SEGMENT_SECONDS, …)`; all but the last are exactly `SEGMENT_SECONDS`
/// long, and the last carries the remainder. The server produces the actual `.m4s` bytes on
/// demand (starting or seeking a transcode when a listed-but-absent segment is requested), but
/// the playlist itself is complete and static — that is what makes the whole timeline
/// seekable without transcoding it all first.
pub fn build_vod_playlist(duration_ms: u64) -> String {
    let seg = SEGMENT_SECONDS as f64;
    let total = duration_ms as f64 / 1000.0;
    let full = (total / seg).floor() as u32;
    let remainder = total - (full as f64) * seg;

    let mut m = String::new();
    m.push_str("#EXTM3U\n");
    m.push_str("#EXT-X-VERSION:7\n");
    // TARGETDURATION must be >= the longest segment (ceil of SEGMENT_SECONDS).
    m.push_str(&format!("#EXT-X-TARGETDURATION:{}\n", SEGMENT_SECONDS));
    m.push_str("#EXT-X-MEDIA-SEQUENCE:0\n");
    m.push_str("#EXT-X-PLAYLIST-TYPE:VOD\n");
    m.push_str("#EXT-X-INDEPENDENT-SEGMENTS\n");
    m.push_str(&format!("#EXT-X-MAP:URI=\"{INIT_NAME}\"\n"));
    for n in 0..full {
        m.push_str(&format!("#EXTINF:{seg:.6},\n"));
        m.push_str(&format!("seg{n:05}.m4s\n"));
    }
    // A final short segment for the remainder (skip a negligible <100ms tail).
    if remainder > 0.1 {
        m.push_str(&format!("#EXTINF:{remainder:.6},\n"));
        m.push_str(&format!("seg{full:05}.m4s\n"));
    }
    m.push_str("#EXT-X-ENDLIST\n");
    m
}

/// The segment index a filename like `seg00042.m4s` refers to, or `None` if it isn't a
/// segment name. Used by the api layer to map a requested segment back to its time offset.
pub fn segment_index(file: &str) -> Option<u32> {
    let stem = file.strip_prefix("seg")?.strip_suffix(".m4s")?;
    stem.parse::<u32>().ok()
}

/// The ffmpeg encoder name for an audio target codec.
fn audio_encoder(codec: AudioCodec) -> &'static str {
    match codec {
        AudioCodec::Aac => "aac",
        AudioCodec::Ac3 => "ac3",
        AudioCodec::Eac3 => "eac3",
        // We never *target* these; fall back to AAC if somehow requested.
        _ => "aac",
    }
}

fn audio_bitrate(codec: AudioCodec) -> &'static str {
    match codec {
        AudioCodec::Aac => "256k",
        // Surround targets carry 6 channels.
        AudioCodec::Ac3 => "640k",
        AudioCodec::Eac3 => "768k",
        _ => "256k",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::Vendor;
    use medi_core::VideoCodec;
    use std::path::PathBuf;

    fn target(vendor: Option<Vendor>, tone_map: bool, dv: bool, sw: bool) -> TranscodeTarget {
        TranscodeTarget {
            vendor,
            software_decode: sw,
            tone_map,
            dv_tone_map: dv,
            video_codec: VideoCodec::H264,
            audio_transcode_to: None,
            max_bitrate: None,
            subtitle_burn_in: None,
        }
    }

    fn argv(t: &TranscodeTarget, audio: AudioTarget) -> Vec<String> {
        build_argv(
            t,
            audio,
            &PathBuf::from("/media/in.mkv"),
            &PathBuf::from("/config/hls/abc"),
            Some("/dev/dri/renderD128"),
            0,
        )
    }

    fn joined(v: &[String]) -> String {
        v.join(" ")
    }

    #[test]
    fn every_output_is_fmp4_hls() {
        let a = argv(&target(Some(Vendor::Intel), false, false, false), AudioTarget::Copy { source: None });
        let s = joined(&a);
        assert!(s.contains("-f hls"));
        assert!(s.contains("-hls_segment_type fmp4"), "Apple TV needs fMP4, got: {s}");
        assert!(s.contains("init.mp4"));
        // Keyframe-aligned segments for clean seeking; ffmpeg writes a throwaway playlist
        // (the api layer serves the synthesized VOD one).
        assert!(s.contains("-force_key_frames"), "segments must be keyframe-aligned: {s}");
        assert!(s.contains("-start_number 0"));
        assert!(s.ends_with("ffmpeg.m3u8"), "ffmpeg writes a throwaway playlist: {s}");
    }

    #[test]
    fn seek_start_segment_adds_ss_and_start_number() {
        // Starting at segment 100 (=400s in) seeks the input and numbers segments from 100.
        let a = build_argv(
            &target(Some(Vendor::Intel), false, false, false),
            AudioTarget::Copy { source: None },
            &PathBuf::from("/media/in.mkv"),
            &PathBuf::from("/config/hls/abc"),
            Some("/dev/dri/renderD128"),
            100,
        );
        let s = joined(&a);
        assert!(s.contains("-ss 400"), "seek to 100*4s: {s}");
        assert!(s.contains("-start_number 100"), "segments numbered from 100: {s}");
        // `-ss` must come before `-i` (fast input seek).
        let ss = a.iter().position(|x| x == "-ss").unwrap();
        let i = a.iter().position(|x| x == "-i").unwrap();
        assert!(ss < i, "-ss must precede -i");
    }

    #[test]
    fn vod_playlist_is_complete_and_seekable() {
        // A 30.5s title at 4s segments → 7 full + 1 remainder = 8 segments, VOD, ENDLIST.
        let m = build_vod_playlist(30_500);
        assert!(m.contains("#EXT-X-PLAYLIST-TYPE:VOD"), "must be VOD, not live: {m}");
        assert!(m.contains("#EXT-X-ENDLIST"), "complete playlist has ENDLIST");
        assert!(m.contains("#EXT-X-MAP:URI=\"init.mp4\""));
        assert!(m.contains("seg00000.m4s") && m.contains("seg00007.m4s"), "all segments listed: {m}");
        assert_eq!(m.matches(".m4s").count(), 8, "7 full + 1 remainder");
        // The remainder segment carries <4s.
        assert!(m.contains("#EXTINF:2.500000,"), "remainder EXTINF: {m}");
    }

    #[test]
    fn segment_index_parses_only_segment_names() {
        assert_eq!(segment_index("seg00042.m4s"), Some(42));
        assert_eq!(segment_index("seg00000.m4s"), Some(0));
        assert_eq!(segment_index("init.mp4"), None);
        assert_eq!(segment_index("index.m3u8"), None);
        assert_eq!(segment_index("../secret"), None);
    }

    #[test]
    fn intel_hdr10_uses_vpp_tonemap() {
        let a = argv(&target(Some(Vendor::Intel), true, false, false), AudioTarget::Copy { source: None });
        let s = joined(&a);
        assert!(s.contains("vpp_qsv=tonemap=1"), "HDR10 uses VPP: {s}");
        assert!(s.contains("h264_qsv"));
        assert!(!s.contains("tonemap_opencl"));
    }

    #[test]
    fn intel_dv_uses_opencl_not_vpp() {
        let a = argv(&target(Some(Vendor::Intel), true, true, false), AudioTarget::Copy { source: None });
        let s = joined(&a);
        assert!(s.contains("tonemap_opencl"), "DV must use OpenCL, not VPP: {s}");
        assert!(!s.contains("vpp_qsv=tonemap"));
        // The OpenCL device is chained off the VA-API device.
        assert!(s.contains("opencl@va=ocl"));
        assert!(s.contains("-filter_hw_device ocl"));
    }

    #[test]
    fn nvidia_dv_uses_cuda_tonemap() {
        let a = argv(&target(Some(Vendor::Nvidia), true, true, false), AudioTarget::Copy { source: None });
        let s = joined(&a);
        assert!(s.contains("tonemap_cuda"));
        assert!(s.contains("h264_nvenc"));
        assert!(s.contains("-hwaccel cuda"));
        // Frames arrive already on the GPU (`-hwaccel_output_format cuda`) — uploading them
        // again fails the filter graph, so the chain must NOT contain `hwupload_cuda`.
        assert!(!s.contains("hwupload_cuda"), "must not re-upload GPU frames: {s}");
        // `tonemap_cuda` outputs nv12 directly (no separate scale_cuda needed).
        assert!(s.contains("format=nv12"), "tonemap_cuda outputs nv12: {s}");
    }

    #[test]
    fn software_fallback_tone_map_uses_zscale() {
        let a = argv(&target(None, true, true, true), AudioTarget::Copy { source: None });
        let s = joined(&a);
        assert!(s.contains("libx264"));
        assert!(s.contains("tonemap=tonemap=hable") || s.contains("zscale"), "sw tonemap: {s}");
        assert!(!s.contains("-hwaccel"));
    }

    #[test]
    fn sdr_transcode_has_no_filter() {
        let a = argv(&target(Some(Vendor::Intel), false, false, false), AudioTarget::Copy { source: None });
        let s = joined(&a);
        assert!(!s.contains("-vf"), "plain SDR transcode needs no filter: {s}");
    }

    #[test]
    fn tone_mapped_output_is_tagged_bt709() {
        let a = argv(&target(Some(Vendor::Intel), true, false, false), AudioTarget::Copy { source: None });
        let s = joined(&a);
        assert!(s.contains("-color_trc bt709"));
    }

    #[test]
    fn audio_transcode_to_eac3_surround() {
        let a = argv(
            &target(Some(Vendor::Intel), false, false, false),
            AudioTarget::Transcode { codec: AudioCodec::Eac3, channels: 6, source: None },
        );
        let s = joined(&a);
        assert!(s.contains("-c:a eac3"));
        assert!(s.contains("-ac 6"));
    }

    #[test]
    fn audio_downmix_emits_resolved_channel_count() {
        // A 7.1 → 5.1 downmix carries the resolved channel count, not a hard-coded 6.
        let a = argv(
            &target(Some(Vendor::Intel), false, false, false),
            AudioTarget::Transcode { codec: AudioCodec::Aac, channels: 2, source: None },
        );
        let s = joined(&a);
        assert!(s.contains("-c:a aac"));
        assert!(s.contains("-ac 2"), "AAC stereo fallback: {s}");
    }

    #[test]
    fn default_audio_maps_nothing_explicit() {
        // With no source track selected, ffmpeg picks the default first audio itself — no
        // explicit `-map` is emitted on the plain (non-burn-in) path.
        let a = argv(&target(Some(Vendor::Intel), false, false, false), AudioTarget::Copy { source: None });
        let s = joined(&a);
        assert!(!s.contains("-map "), "no explicit audio map without a selected track: {s}");
    }

    #[test]
    fn selected_audio_track_maps_that_stream() {
        // Selecting audio-relative track 2 (an in-player audio switch) maps `0:v` + `0:a:2`.
        let a = argv(
            &target(Some(Vendor::Intel), false, false, false),
            AudioTarget::Copy { source: Some(2) },
        );
        let s = joined(&a);
        assert!(s.contains("-map 0:v"), "video kept: {s}");
        assert!(s.contains("-map 0:a:2?"), "selected audio track mapped: {s}");
    }

    #[test]
    fn selected_audio_track_maps_under_burn_in() {
        // The burn-in path (explicit `-map [v]`) maps the selected source audio track too,
        // not the hard-coded first track.
        let mut t = target(Some(Vendor::Intel), false, false, false);
        t.subtitle_burn_in = Some(0);
        let a = argv(&t, AudioTarget::Copy { source: Some(3) });
        let s = joined(&a);
        assert!(s.contains("-map [v]"));
        assert!(s.contains("-map 0:a:3?"), "burn-in path maps the selected audio track: {s}");
        assert!(!s.contains("-map 0:a:0?"), "no longer hard-codes the first track: {s}");
    }

    #[test]
    fn capped_emits_maxrate_and_bufsize() {
        let mut t = target(Some(Vendor::Intel), false, false, false);
        t.max_bitrate = Some(8_000_000);
        let a = argv(&t, AudioTarget::Copy { source: None });
        let s = joined(&a);
        assert!(s.contains("-maxrate 8000000"), "capped sets -maxrate: {s}");
        assert!(s.contains("-bufsize 16000000"));
    }

    #[test]
    fn burn_in_without_tonemap_overlays_video() {
        // A plain transcode that burns in an image subtitle: filter_complex overlay, no
        // tone-map chain, and an explicit `-map [v]`.
        let mut t = target(Some(Vendor::Intel), false, false, false);
        t.subtitle_burn_in = Some(0);
        let a = argv(&t, AudioTarget::Copy { source: None });
        let s = joined(&a);
        assert!(s.contains("-filter_complex"), "burn-in uses filter_complex: {s}");
        assert!(s.contains("[0:v][0:s:0]overlay[v]"), "overlay graph: {s}");
        assert!(s.contains("-map [v]"));
        assert!(!s.contains("-vf "), "no plain -vf when burning in: {s}");
    }

    #[test]
    fn burn_in_after_tonemap_orders_overlay_last() {
        // HDR10→SDR tone-map + burn-in: the overlay must chain AFTER the VPP tone-map so
        // the burned subtitle isn't washed out (`docs/.tasks/90` §5).
        let mut t = target(Some(Vendor::Intel), true, false, false);
        t.subtitle_burn_in = Some(1);
        let a = argv(&t, AudioTarget::Copy { source: None });
        let s = joined(&a);
        assert!(s.contains("-filter_complex"));
        // The tone-map filter appears in the base, before the overlay stage.
        let fc = a
            .iter()
            .position(|x| x == "-filter_complex")
            .map(|i| a[i + 1].clone())
            .unwrap();
        let tonemap_at = fc.find("vpp_qsv=tonemap").expect("tone-map present");
        let overlay_at = fc.find("overlay[v]").expect("overlay present");
        assert!(tonemap_at < overlay_at, "overlay must come after tone-map: {fc}");
        assert!(fc.contains("[0:v]"));
        assert!(fc.contains("[base][0:s:1]overlay[v]"));
    }
}
