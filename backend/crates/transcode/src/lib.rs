//! `medi-transcode` — dynamic jellyfin-ffmpeg command generation and HLS session
//! management (Phase 2, `docs/.tasks/20-phase2-hwa-transcode.md`).
//!
//! Reads the `MediaProfile` (incl. `DvProfile`) from `medi-core` plus a client profile
//! to decide **direct-play vs transcode**, picks a vendor HWA path (Intel QSV, NVIDIA
//! NVENC, AMD AMF/VA-API), and builds the ffmpeg argv — including OpenCL/CUDA Dolby
//! Vision tone-mapping and **fragmented-MP4 (CMAF) HLS** output tuned for tvOS
//! AVPlayer / Apple TV.
//!
//! ## Modules
//! - [`caps`] — probe host GPU/OpenCL/CUDA/encoder capabilities at boot.
//! - [`decision`] — the direct-play vs transcode decision table + Apple TV client profile.
//! - [`command`] — assemble the concrete ffmpeg argv per decision (fMP4 HLS).
//! - [`session`] — spawn/track/tear-down live HLS transcode sessions with a capacity cap.
//!
//! ## Boot + request flow
//! ```no_run
//! # async fn wire() {
//! let caps = medi_transcode::caps::probe().await;
//! let mgr = medi_transcode::SessionManager::new("/config/hls".into(), 6, caps.clone());
//! mgr.spawn_reaper();
//! // On GET /api/stream: decision::decide(...) → if Transcode, mgr.start(...).
//! # }
//! ```

pub mod caps;
pub mod command;
pub mod decision;
pub mod session;

pub use caps::HwCaps;
pub use command::{build_vod_playlist, segment_index, AudioTarget, PLAYLIST_NAME};
pub use decision::{
    audio_plan, decide, AudioPlan, AudioTrack, ClientProfile, Decision, Quality, SubtitlePlan,
    TranscodeTarget, Vendor,
};
// `AudioCodec` is promoted to `medi-core` (`docs/.tasks/70`); re-export here so existing
// `medi_transcode::AudioCodec` paths keep working after the move.
pub use medi_core::AudioCodec;
pub use session::{SessionError, SessionManager};
