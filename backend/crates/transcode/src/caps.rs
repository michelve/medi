//! Host hardware-acceleration capability probe (`docs/.tasks/20` sub-task 1).
//!
//! At boot the server probes what the *host* can do — which GPU vendor is present,
//! whether OpenCL / CUDA runtimes are usable for Dolby Vision tone-mapping, and which
//! encoders `jellyfin-ffmpeg` exposes — and caches the result in a [`HwCaps`] struct.
//! `decision.rs` reads it to pick a vendor path (and to fall back to software decode
//! when no HW can handle a source).
//!
//! Probing is best-effort and never fatal: a box with no GPU yields
//! [`HwCaps::software_only`], and the decision engine still produces a valid
//! (software-decode) pipeline. Detection is filesystem/subprocess based — no FFI:
//! - `/dev/dri/renderD*` render nodes → Intel/AMD VA-API/QSV present;
//! - `nvidia-smi` on `PATH` (or `/dev/nvidia0`) → NVIDIA present;
//! - `ffmpeg -hwaccels` and `-encoders` → which accels/encoders the binary supports;
//! - `intel_compute_runtime` / `nvidia` presence → OpenCL / CUDA for DV tone-map.

use std::path::Path;
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::Vendor;

/// The ffmpeg binary. jellyfin-ffmpeg installs as `ffmpeg` on `PATH` inside the
/// container (`docs/.tasks/50`); overridable via `FFMPEG_BIN` for tests / dev.
pub fn ffmpeg_bin() -> String {
    std::env::var("FFMPEG_BIN").unwrap_or_else(|_| "ffmpeg".to_string())
}

/// The cached host capability picture. Cheap to clone; built once at boot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HwCaps {
    /// The HWA vendor path to use, or `None` for software-only hosts.
    pub vendor: Option<Vendor>,
    /// The DRI render node (`/dev/dri/renderD128`) for Intel/AMD VA-API/QSV/OpenCL.
    pub render_node: Option<String>,
    /// OpenCL runtime usable for Dolby Vision tone-mapping (Intel/AMD).
    pub opencl: bool,
    /// CUDA runtime usable for Dolby Vision tone-mapping (NVIDIA).
    pub cuda: bool,
    /// Encoder names ffmpeg reports (e.g. `h264_qsv`, `hevc_nvenc`).
    pub encoders: Vec<String>,
    /// `-hwaccels` ffmpeg reports (e.g. `qsv`, `cuda`, `vaapi`).
    pub hwaccels: Vec<String>,
}

impl HwCaps {
    /// A host with no usable GPU — every transcode uses software decode + software
    /// (libx264) encode. Always a valid target so the server never fails to start.
    pub fn software_only() -> Self {
        Self {
            vendor: None,
            render_node: None,
            opencl: false,
            cuda: false,
            encoders: Vec::new(),
            hwaccels: Vec::new(),
        }
    }

    /// Can this host tone-map Dolby Vision (needs OpenCL for Intel/AMD or CUDA for
    /// NVIDIA)? Plain VPP is insufficient for DV — see `docs/.tasks/20`.
    pub fn can_tonemap_dv(&self) -> bool {
        match self.vendor {
            Some(Vendor::Nvidia) => self.cuda,
            Some(Vendor::Intel) | Some(Vendor::Amd) => self.opencl,
            None => false,
        }
    }

    /// Does ffmpeg report an encoder by this exact name?
    pub fn has_encoder(&self, name: &str) -> bool {
        self.encoders.iter().any(|e| e == name)
    }
}

/// Probe the host and build [`HwCaps`]. Runs a few subprocesses; call once at boot.
pub async fn probe() -> HwCaps {
    let render_node = first_render_node();
    let nvidia = nvidia_present().await;

    let hwaccels = list_tokens(&["-hwaccels"]).await;
    let encoders = list_encoders().await;

    // Vendor precedence: an explicit override wins; else NVIDIA (discrete) if present,
    // else a DRI node implies Intel/AMD. QSV vs VA-API is disambiguated by encoder
    // availability in `decision.rs`.
    let vendor = match std::env::var("MEDI_GPU_VENDOR").ok().as_deref() {
        Some("intel") => Some(Vendor::Intel),
        Some("nvidia") => Some(Vendor::Nvidia),
        Some("amd") => Some(Vendor::Amd),
        Some("none") => None,
        _ if nvidia => Some(Vendor::Nvidia),
        _ if render_node.is_some() => {
            // Prefer QSV when a qsv encoder exists, else treat as AMD VA-API.
            if encoders.iter().any(|e| e.ends_with("_qsv")) {
                Some(Vendor::Intel)
            } else {
                Some(Vendor::Amd)
            }
        }
        _ => None,
    };

    let opencl = matches!(vendor, Some(Vendor::Intel) | Some(Vendor::Amd))
        && opencl_runtime_present();
    let cuda = matches!(vendor, Some(Vendor::Nvidia)) && nvidia;

    let caps = HwCaps {
        vendor,
        render_node,
        opencl,
        cuda,
        encoders,
        hwaccels,
    };
    tracing::info!(
        vendor = ?caps.vendor,
        render_node = ?caps.render_node,
        opencl = caps.opencl,
        cuda = caps.cuda,
        "probed host hardware capabilities",
    );
    caps
}

/// First `/dev/dri/renderD*` node, if any (Intel/AMD).
fn first_render_node() -> Option<String> {
    let dri = Path::new("/dev/dri");
    let entries = std::fs::read_dir(dri).ok()?;
    let mut nodes: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("renderD"))
        .collect();
    nodes.sort();
    nodes.into_iter().next().map(|n| format!("/dev/dri/{n}"))
}

/// Is an NVIDIA GPU exposed? `/dev/nvidia0` (host runtime injects it) or `nvidia-smi`.
async fn nvidia_present() -> bool {
    if Path::new("/dev/nvidia0").exists() {
        return true;
    }
    Command::new("nvidia-smi")
        .arg("-L")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Is the Intel/AMD OpenCL runtime present (intel-compute-runtime installs an ICD)?
fn opencl_runtime_present() -> bool {
    Path::new("/etc/OpenCL/vendors").is_dir()
        && std::fs::read_dir("/etc/OpenCL/vendors")
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
}

/// Run `ffmpeg <args>` and return whitespace/line tokens from stdout, lowercased.
/// Used for `-hwaccels`. Returns empty on any failure (best-effort probe).
async fn list_tokens(args: &[&str]) -> Vec<String> {
    let out = Command::new(ffmpeg_bin())
        .args(["-hide_banner", "-loglevel", "quiet"])
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;
    let Ok(out) = out else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_ascii_lowercase())
        // The first line of `-hwaccels` is a header; keep only single-token lines.
        .filter(|l| !l.is_empty() && !l.contains(' ') && l != "hardware acceleration methods:")
        .collect()
}

/// Parse `ffmpeg -encoders` into the list of encoder names (the second column of each
/// `" V..... h264_qsv  ..."` line).
async fn list_encoders() -> Vec<String> {
    let out = Command::new(ffmpeg_bin())
        .args(["-hide_banner", "-loglevel", "quiet", "-encoders"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;
    let Ok(out) = out else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(parse_encoder_line)
        .collect()
}

/// A `-encoders` line looks like ` V....D h264_qsv   H.264 ... (Intel Quick Sync)`.
/// The flags column is 6 chars; take the token after it. Header/separator lines have
/// no leading-space+flags shape and are skipped.
fn parse_encoder_line(line: &str) -> Option<String> {
    let line = line.strip_prefix(' ')?;
    // Must start with a video/audio/subtitle type flag.
    let first = line.chars().next()?;
    if !matches!(first, 'V' | 'A' | 'S') {
        return None;
    }
    let mut parts = line.split_whitespace();
    let _flags = parts.next()?;
    parts.next().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn software_only_has_no_dv_tonemap() {
        let caps = HwCaps::software_only();
        assert!(!caps.can_tonemap_dv());
        assert!(caps.vendor.is_none());
    }

    #[test]
    fn dv_tonemap_gated_by_runtime() {
        let mut caps = HwCaps::software_only();
        caps.vendor = Some(Vendor::Intel);
        assert!(!caps.can_tonemap_dv(), "Intel without OpenCL cannot tone-map DV");
        caps.opencl = true;
        assert!(caps.can_tonemap_dv());

        caps.vendor = Some(Vendor::Nvidia);
        caps.opencl = false;
        assert!(!caps.can_tonemap_dv(), "NVIDIA needs CUDA, not OpenCL");
        caps.cuda = true;
        assert!(caps.can_tonemap_dv());
    }

    #[test]
    fn parses_encoder_lines() {
        assert_eq!(
            parse_encoder_line(" V....D h264_qsv             H.264 / AVC (Intel Quick Sync)"),
            Some("h264_qsv".to_string())
        );
        assert_eq!(
            parse_encoder_line(" A..... aac                  AAC (Advanced Audio Coding)"),
            Some("aac".to_string())
        );
        // Header / separator lines are ignored.
        assert_eq!(parse_encoder_line("Encoders:"), None);
        assert_eq!(parse_encoder_line(" ------"), None);
    }

    #[test]
    fn has_encoder_lookup() {
        let mut caps = HwCaps::software_only();
        caps.encoders = vec!["h264_qsv".into(), "libx264".into()];
        assert!(caps.has_encoder("h264_qsv"));
        assert!(!caps.has_encoder("hevc_nvenc"));
    }
}
