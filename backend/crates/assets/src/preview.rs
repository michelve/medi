//! Silent 720p hover-preview extraction (`docs/.tasks/30` §Preview clip, sub-task 2).
//!
//! Extracts a ~15-second clip from the middle of the source, HW-downscales it to
//! **720p H.264**, strips audio, and writes `/config/previews/<file_id>.mp4` with
//! `+faststart` so the client can start playback on the first bytes. Vendor selection
//! (Intel QSV / NVIDIA NVENC / AMD VA-API / software) is reused from the Phase 2 host
//! capability probe [`HwCaps`] so previews run on the same accelerator as live
//! transcodes — but at low priority, off-peak, behind the GPU-idle guard.
//!
//! ## Why mid-point, silent, 720p
//! - **Mid-point**: the opening of a title is often black/logos; the middle is
//!   representative for a hover thumbnail. We seek to `duration/2` (falling back to a
//!   fixed offset when duration is unknown).
//! - **Silent** (`-an`): the Netflix-style hover loop has no audio; dropping it halves
//!   the muxing work and the file size.
//! - **720p**: big enough to look crisp on a hover tile, small enough to serve instantly
//!   and cache at the client edge.
//!
//! ## HDR / Dolby Vision sources
//! A preview of an HDR/DV source must be tone-mapped to SDR or the thumbnail looks
//! washed out / too dark on an SDR-composited UI. When the source is HDR and the host
//! can tone-map, we apply the vendor tone-map filter; otherwise we fall back to a plain
//! scale (an HDR-tagged 720p preview is acceptable, if not ideal, and never fatal).

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

use medi_transcode::caps::{ffmpeg_bin, HwCaps};
use medi_transcode::Vendor;

/// Target preview clip length, seconds.
const CLIP_SECONDS: u32 = 15;
/// Target preview height (720p). Width is `-2` (even, keep aspect).
const TARGET_HEIGHT: u32 = 720;
/// Seek offset used when the source duration is unknown — far enough past intros.
const FALLBACK_SEEK_SECONDS: u64 = 120;

/// Errors from generating a preview clip.
#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
    #[error("failed to spawn ffmpeg: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("ffmpeg exited with status {status}: {stderr}")]
    NonZeroExit { status: String, stderr: String },
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

/// Where a title's preview is written: `/config/previews/<file_id>.mp4`. Stable across
/// regenerations so the `/api/preview/<file_id>.mp4` URL never changes.
pub fn preview_path(previews_dir: &Path, media_file_id: i64) -> PathBuf {
    previews_dir.join(format!("{media_file_id}.mp4"))
}

/// Generate the 720p silent hover preview for `input` and return the output path.
///
/// `hdr` marks an HDR/DV source that should be tone-mapped to SDR. `duration_ms` picks
/// the mid-point seek; `None` falls back to [`FALLBACK_SEEK_SECONDS`]. The output
/// directory is created if absent. Writes atomically-ish: ffmpeg writes the final path
/// directly (a single fast mux); a crash leaves a partial file that the next off-peak
/// pass overwrites, and the DB row is only stamped after a clean exit (see `worker.rs`).
pub async fn generate(
    caps: &HwCaps,
    input: &Path,
    previews_dir: &Path,
    media_file_id: i64,
    duration_ms: Option<i64>,
    hdr: bool,
) -> Result<PathBuf, PreviewError> {
    std::fs::create_dir_all(previews_dir)?;
    let out = preview_path(previews_dir, media_file_id);

    let seek = mid_point_seconds(duration_ms);
    let argv = build_argv(caps, input, &out, seek, hdr);
    tracing::info!(
        media_file_id,
        input = %input.display(),
        seek,
        hdr,
        "generating 720p hover preview",
    );
    tracing::debug!(argv = ?argv, "preview ffmpeg argv");

    let output = Command::new(ffmpeg_bin())
        .args(&argv)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(PreviewError::Spawn)?;

    if !output.status.success() {
        // Remove a partial output so a stale/half file never gets served.
        let _ = std::fs::remove_file(&out);
        return Err(PreviewError::NonZeroExit {
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(out)
}

/// Seek offset (seconds) to the middle of the clip, given the source duration.
fn mid_point_seconds(duration_ms: Option<i64>) -> u64 {
    match duration_ms {
        Some(ms) if ms > 0 => (ms as u64 / 1000) / 2,
        _ => FALLBACK_SEEK_SECONDS,
    }
}

/// Assemble the ffmpeg argv for the preview extract. Kept pure (no I/O) so it is unit
/// testable. Places `-ss` *before* `-i` for a fast keyframe seek.
fn build_argv(caps: &HwCaps, input: &Path, out: &Path, seek: u64, hdr: bool) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let push = |a: &mut Vec<String>, s: &str| a.push(s.to_string());

    push(&mut a, "-hide_banner");
    push(&mut a, "-loglevel");
    push(&mut a, "warning");
    push(&mut a, "-nostdin");
    // Overwrite any stale partial from a previous interrupted run.
    push(&mut a, "-y");

    // Fast input seek (keyframe-accurate is fine for a preview) before -i.
    push(&mut a, "-ss");
    a.push(seek.to_string());

    // HW device init + decode, mirroring the live transcode vendor path. For an HDR
    // source we only take the GPU path when this host can GPU tone-map (`can_tonemap_dv`
    // covers OpenCL/CUDA); otherwise we fall back to software, where the zscale/tonemap
    // filter always works. A preview is a short 15s clip, so software is cheap enough
    // and this keeps the preview path from ever failing on an under-equipped GPU.
    let vendor = if hdr && !caps.can_tonemap_dv() {
        None
    } else {
        caps.vendor
    };
    let node = caps.render_node.as_deref().unwrap_or("/dev/dri/renderD128");
    emit_hw_init_decode(&mut a, vendor, node);

    push(&mut a, "-i");
    a.push(input.to_string_lossy().into_owned());

    // Duration of the extracted clip (output-side, after seek).
    push(&mut a, "-t");
    a.push(CLIP_SECONDS.to_string());

    // Silent preview.
    push(&mut a, "-an");

    // Scale to 720p (+ tone-map for HDR). Vendor-specific filter graph.
    if let Some(vf) = filter_graph(vendor, hdr) {
        push(&mut a, "-vf");
        a.push(vf);
    }

    // H.264 encoder for the vendor (previews are always H.264 for universal playback).
    push(&mut a, "-c:v");
    push(&mut a, h264_encoder(vendor));
    for arg in quality_args(vendor) {
        a.push(arg);
    }
    // Tag tone-mapped output BT.709 so the SDR preview renders correctly.
    if hdr {
        for t in [
            "-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709",
        ] {
            push(&mut a, t);
        }
    }

    // Web-optimized single-file MP4: move the moov atom to the front.
    push(&mut a, "-movflags");
    push(&mut a, "+faststart");
    a.push(out.to_string_lossy().into_owned());
    a
}

/// Emit `-init_hw_device` / `-hwaccel` for the chosen vendor, or nothing for software.
/// A trimmed version of the live transcode's `HwPlan` — previews never need the OpenCL
/// DV chain because the preview tone-map path uses software when GPU DV is unavailable.
fn emit_hw_init_decode(a: &mut Vec<String>, vendor: Option<Vendor>, node: &str) {
    let p = |a: &mut Vec<String>, s: &str| a.push(s.to_string());
    match vendor {
        None => {} // software decode
        Some(Vendor::Intel) => {
            p(a, "-init_hw_device");
            a.push(format!("qsv=qs:{node}"));
            p(a, "-filter_hw_device");
            p(a, "qs");
            p(a, "-hwaccel");
            p(a, "qsv");
            p(a, "-hwaccel_output_format");
            p(a, "qsv");
        }
        Some(Vendor::Nvidia) => {
            p(a, "-init_hw_device");
            p(a, "cuda=cu");
            p(a, "-filter_hw_device");
            p(a, "cu");
            p(a, "-hwaccel");
            p(a, "cuda");
            p(a, "-hwaccel_output_format");
            p(a, "cuda");
        }
        Some(Vendor::Amd) => {
            p(a, "-init_hw_device");
            a.push(format!("vaapi=va:{node}"));
            p(a, "-filter_hw_device");
            p(a, "va");
            p(a, "-hwaccel");
            p(a, "vaapi");
            p(a, "-hwaccel_output_format");
            p(a, "vaapi");
        }
    }
}

/// The `-vf` filter graph: downscale to 720p, tone-mapping HDR→SDR when `hdr`.
fn filter_graph(vendor: Option<Vendor>, hdr: bool) -> Option<String> {
    Some(match (vendor, hdr) {
        // Software: zscale tone-map (if HDR) then a plain scale to 720p.
        (None, true) => format!(
            "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,\
             tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,format=yuv420p,\
             scale=-2:{TARGET_HEIGHT}"
        ),
        (None, false) => format!("scale=-2:{TARGET_HEIGHT}"),
        // Intel: VPP scales (and tone-maps when HDR) on the GPU. `vpp_qsv` only accepts `-1`
        // for auto-dimension (unlike software `scale`'s `-2`); `w=-2` errors with
        // "Size values less than -1 are not acceptable" and the encode writes nothing.
        (Some(Vendor::Intel), true) => {
            format!("vpp_qsv=tonemap=1:w=-1:h={TARGET_HEIGHT}:format=nv12")
        }
        (Some(Vendor::Intel), false) => format!("vpp_qsv=w=-1:h={TARGET_HEIGHT}"),
        // NVIDIA: CUDA tone-map then scale, or scale_cuda alone.
        (Some(Vendor::Nvidia), true) => format!(
            "tonemap_cuda=tonemap=bt2390:transfer=bt709:matrix=bt709:primaries=bt709,\
             scale_cuda=-2:{TARGET_HEIGHT}:format=nv12"
        ),
        (Some(Vendor::Nvidia), false) => format!("scale_cuda=-2:{TARGET_HEIGHT}"),
        // AMD VA-API: scale_vaapi handles the downscale; tone-map via VA-API when HDR.
        (Some(Vendor::Amd), true) => {
            format!("tonemap_vaapi=format=nv12,scale_vaapi=w=-2:h={TARGET_HEIGHT}")
        }
        (Some(Vendor::Amd), false) => format!("scale_vaapi=w=-2:h={TARGET_HEIGHT}"),
    })
}

/// The H.264 encoder name for the vendor (previews are always H.264).
fn h264_encoder(vendor: Option<Vendor>) -> &'static str {
    match vendor {
        None => "libx264",
        Some(Vendor::Intel) => "h264_qsv",
        Some(Vendor::Nvidia) => "h264_nvenc",
        Some(Vendor::Amd) => "h264_vaapi",
    }
}

/// Quality-targeted rate control for the preview encode. Previews favor small size over
/// fidelity (a hover thumbnail), so the quality target is looser than a live transcode.
fn quality_args(vendor: Option<Vendor>) -> Vec<String> {
    let q = |k: &str, v: &str| vec![k.to_string(), v.to_string()];
    match vendor {
        None => q("-crf", "26"),
        Some(Vendor::Intel) => q("-global_quality", "28"),
        Some(Vendor::Nvidia) => {
            let mut v = q("-rc", "vbr");
            v.extend(q("-cq", "28"));
            v
        }
        Some(Vendor::Amd) => q("-qp", "28"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn caps(vendor: Option<Vendor>) -> HwCaps {
        let mut c = HwCaps::software_only();
        c.vendor = vendor;
        if vendor.is_some() {
            c.render_node = Some("/dev/dri/renderD128".into());
        }
        c
    }

    fn argv(vendor: Option<Vendor>, hdr: bool) -> Vec<String> {
        build_argv(
            &caps(vendor),
            &PathBuf::from("/media/movie.mkv"),
            &PathBuf::from("/config/previews/7.mp4"),
            600,
            hdr,
        )
    }

    fn joined(v: &[String]) -> String {
        v.join(" ")
    }

    #[test]
    fn preview_path_is_stable_by_id() {
        let p = preview_path(&PathBuf::from("/config/previews"), 42);
        assert!(p.ends_with("42.mp4"));
    }

    #[test]
    fn mid_point_uses_half_duration() {
        assert_eq!(mid_point_seconds(Some(600_000)), 300);
        // Unknown duration falls back to the fixed offset.
        assert_eq!(mid_point_seconds(None), FALLBACK_SEEK_SECONDS);
        assert_eq!(mid_point_seconds(Some(0)), FALLBACK_SEEK_SECONDS);
    }

    #[test]
    fn preview_is_silent_and_720p_and_faststart() {
        let s = joined(&argv(Some(Vendor::Intel), false));
        assert!(s.contains("-an"), "audio must be stripped: {s}");
        assert!(s.contains("h=720") || s.contains(":720"), "720p target: {s}");
        assert!(s.contains("+faststart"));
        assert!(s.ends_with("7.mp4"));
    }

    #[test]
    fn seek_precedes_input() {
        let a = argv(Some(Vendor::Intel), false);
        let ss = a.iter().position(|x| x == "-ss").unwrap();
        let i = a.iter().position(|x| x == "-i").unwrap();
        assert!(ss < i, "fast seek requires -ss before -i");
    }

    #[test]
    fn hdr_preview_tone_maps_and_tags_bt709() {
        // Software path (no GPU DV) tone-maps via zscale/tonemap and tags BT.709.
        let s = joined(&argv(None, true));
        assert!(s.contains("tonemap") || s.contains("zscale"), "HDR tone-map: {s}");
        assert!(s.contains("-color_trc bt709"));
        assert!(s.contains("libx264"));
    }

    #[test]
    fn sdr_intel_uses_qsv_encoder_no_tonemap() {
        let s = joined(&argv(Some(Vendor::Intel), false));
        assert!(s.contains("h264_qsv"));
        assert!(!s.contains("tonemap"));
    }

    #[test]
    fn intel_vpp_uses_minus_one_auto_width_not_minus_two() {
        // `vpp_qsv` rejects `-2` ("Size values less than -1 are not acceptable"); it must be
        // `-1` for auto-width. Assert both the SDR and HDR Intel filter graphs use `w=-1`.
        for hdr in [false, true] {
            let g = filter_graph(Some(Vendor::Intel), hdr).unwrap();
            assert!(g.contains("vpp_qsv"), "intel uses vpp_qsv: {g}");
            assert!(g.contains("w=-1"), "vpp_qsv auto-width must be -1: {g}");
            assert!(!g.contains("w=-2"), "vpp_qsv must not use -2: {g}");
        }
    }
}
