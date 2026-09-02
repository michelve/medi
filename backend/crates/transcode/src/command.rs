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
/// playlist churn for a long movie.
const SEGMENT_SECONDS: u32 = 4;

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
) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let push = |a: &mut Vec<String>, s: &str| a.push(s.to_string());

    push(&mut a, "-hide_banner");
    push(&mut a, "-loglevel");
    push(&mut a, "warning");
    // Never let a hung transcode block forever; the session layer also enforces idle
    // teardown, but this bounds a single stalled read.
    push(&mut a, "-nostdin");

    // --- Hardware device init + decode. --------------------------------------
    let node = render_node.unwrap_or("/dev/dri/renderD128");
    let hw = HwPlan::for_target(target);
    hw.emit_init_and_decode(&mut a, node);

    // --- Input. --------------------------------------------------------------
    push(&mut a, "-i");
    a.push(input.to_string_lossy().into_owned());

    // --- Video filter graph + encoder. ---------------------------------------
    if let Some(vf) = hw.filter_graph(target) {
        push(&mut a, "-vf");
        a.push(vf);
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
        AudioTarget::Copy => {
            push(&mut a, "-c:a");
            push(&mut a, "copy");
        }
        AudioTarget::Transcode { codec, channels } => {
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
    emit_hls_muxer(&mut a, out_dir);

    a
}

/// Whether the transcode copies or re-encodes audio, and to how many channels (resolved
/// by the caller from the source track + client profile via `decision::audio_plan`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioTarget {
    Copy,
    Transcode { codec: AudioCodec, channels: u8 },
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
                // CUDA tone-map for both HDR10 and Dolby Vision.
                "hwupload_cuda,\
                 tonemap_cuda=tonemap=bt2390:transfer=bt709:matrix=bt709:primaries=bt709,\
                 scale_cuda=format=nv12"
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

/// Append the fragmented-MP4 HLS muxer flags + output playlist path.
fn emit_hls_muxer(a: &mut Vec<String>, out_dir: &Path) {
    let p = |a: &mut Vec<String>, s: &str| a.push(s.to_string());
    p(a, "-f");
    p(a, "hls");
    p(a, "-hls_time");
    a.push(SEGMENT_SECONDS.to_string());
    p(a, "-hls_playlist_type");
    p(a, "vod");
    // Fragmented-MP4 (CMAF) segments — required by tvOS AVPlayer for HEVC/HDR.
    p(a, "-hls_segment_type");
    p(a, "fmp4");
    p(a, "-hls_fmp4_init_filename");
    p(a, INIT_NAME);
    p(a, "-hls_segment_filename");
    a.push(out_dir.join(SEGMENT_TEMPLATE).to_string_lossy().into_owned());
    // Keep the whole playlist (VOD) so the client can seek anywhere.
    p(a, "-hls_list_size");
    p(a, "0");
    a.push(out_dir.join(PLAYLIST_NAME).to_string_lossy().into_owned());
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
        }
    }

    fn argv(t: &TranscodeTarget, audio: AudioTarget) -> Vec<String> {
        build_argv(
            t,
            audio,
            &PathBuf::from("/media/in.mkv"),
            &PathBuf::from("/config/hls/abc"),
            Some("/dev/dri/renderD128"),
        )
    }

    fn joined(v: &[String]) -> String {
        v.join(" ")
    }

    #[test]
    fn every_output_is_fmp4_hls() {
        let a = argv(&target(Some(Vendor::Intel), false, false, false), AudioTarget::Copy);
        let s = joined(&a);
        assert!(s.contains("-f hls"));
        assert!(s.contains("-hls_segment_type fmp4"), "Apple TV needs fMP4, got: {s}");
        assert!(s.contains("init.mp4"));
        assert!(s.ends_with("index.m3u8"));
    }

    #[test]
    fn intel_hdr10_uses_vpp_tonemap() {
        let a = argv(&target(Some(Vendor::Intel), true, false, false), AudioTarget::Copy);
        let s = joined(&a);
        assert!(s.contains("vpp_qsv=tonemap=1"), "HDR10 uses VPP: {s}");
        assert!(s.contains("h264_qsv"));
        assert!(!s.contains("tonemap_opencl"));
    }

    #[test]
    fn intel_dv_uses_opencl_not_vpp() {
        let a = argv(&target(Some(Vendor::Intel), true, true, false), AudioTarget::Copy);
        let s = joined(&a);
        assert!(s.contains("tonemap_opencl"), "DV must use OpenCL, not VPP: {s}");
        assert!(!s.contains("vpp_qsv=tonemap"));
        // The OpenCL device is chained off the VA-API device.
        assert!(s.contains("opencl@va=ocl"));
        assert!(s.contains("-filter_hw_device ocl"));
    }

    #[test]
    fn nvidia_dv_uses_cuda_tonemap() {
        let a = argv(&target(Some(Vendor::Nvidia), true, true, false), AudioTarget::Copy);
        let s = joined(&a);
        assert!(s.contains("tonemap_cuda"));
        assert!(s.contains("h264_nvenc"));
        assert!(s.contains("-hwaccel cuda"));
    }

    #[test]
    fn software_fallback_tone_map_uses_zscale() {
        let a = argv(&target(None, true, true, true), AudioTarget::Copy);
        let s = joined(&a);
        assert!(s.contains("libx264"));
        assert!(s.contains("tonemap=tonemap=hable") || s.contains("zscale"), "sw tonemap: {s}");
        assert!(!s.contains("-hwaccel"));
    }

    #[test]
    fn sdr_transcode_has_no_filter() {
        let a = argv(&target(Some(Vendor::Intel), false, false, false), AudioTarget::Copy);
        let s = joined(&a);
        assert!(!s.contains("-vf"), "plain SDR transcode needs no filter: {s}");
    }

    #[test]
    fn tone_mapped_output_is_tagged_bt709() {
        let a = argv(&target(Some(Vendor::Intel), true, false, false), AudioTarget::Copy);
        let s = joined(&a);
        assert!(s.contains("-color_trc bt709"));
    }

    #[test]
    fn audio_transcode_to_eac3_surround() {
        let a = argv(
            &target(Some(Vendor::Intel), false, false, false),
            AudioTarget::Transcode { codec: AudioCodec::Eac3, channels: 6 },
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
            AudioTarget::Transcode { codec: AudioCodec::Aac, channels: 2 },
        );
        let s = joined(&a);
        assert!(s.contains("-c:a aac"));
        assert!(s.contains("-ac 2"), "AAC stereo fallback: {s}");
    }

    #[test]
    fn capped_emits_maxrate_and_bufsize() {
        let mut t = target(Some(Vendor::Intel), false, false, false);
        t.max_bitrate = Some(8_000_000);
        let a = argv(&t, AudioTarget::Copy);
        let s = joined(&a);
        assert!(s.contains("-maxrate 8000000"), "capped sets -maxrate: {s}");
        assert!(s.contains("-bufsize 16000000"));
    }
}
