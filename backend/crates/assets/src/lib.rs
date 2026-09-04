//! `medi-assets` — off-peak background worker (Phase 3, `docs/.tasks/30`).
//!
//! Extracts silent hover-preview clips (HW-downscaled to 720p H.264) and generates
//! trickplay scrub sprites (BIF by default, or a tiled-JPG mosaic), writing them under
//! `/config` only. Runs inside a configurable off-peak window and yields the GPU to
//! live transcode sessions (the GPU-idle guard), so background asset generation never
//! competes with user-requested streaming.
//!
//! ## Modules
//! - [`scheduler`] — off-peak window check + GPU-idle guard + concurrency throttle.
//! - [`preview`] — mid-point 15s extract → 720p H.264, audio stripped, `+faststart`.
//! - [`trickplay`] — interval frame sampling → BIF packer / tiled-JPG mosaic.
//! - [`chapters`] — one poster frame per embedded chapter (scene-selection + hover fallback).
//! - [`worker`] — the main loop: pick the next un-done file, generate, record, stamp.
//!
//! ## Boot (from the `api` binary)
//! ```no_run
//! # use std::sync::Arc;
//! # async fn boot(
//! #     db: medi_db::Db,
//! #     config: Arc<medi_core::AppConfig>,
//! #     transcode: medi_transcode::SessionManager,
//! #     caps: medi_transcode::HwCaps,
//! # ) {
//! let scheduler = medi_assets::Scheduler::new(config, transcode);
//! let cfg = medi_assets::AssetWorkerConfig::new(caps);
//! tokio::spawn(medi_assets::run(db, scheduler, cfg));
//! # }
//! ```
//!
//! Serving is already wired in Phase 1: `/api/preview` and `/api/trickplay` are static
//! `ServeDir`s over `/config/previews` and `/config/trickplay`, so a generated
//! `<file_id>.mp4` / `<file_id>.{bif,jpg}` is served the moment it lands — no route
//! changes are needed for Phase 3.

pub mod chapters;
pub mod preview;
pub mod scheduler;
pub mod trickplay;
pub mod worker;

pub use scheduler::Scheduler;
pub use worker::{run, AssetWorkerConfig};
