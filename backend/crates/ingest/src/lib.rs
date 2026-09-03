//! `medi-ingest` — scans `/media`, runs `ffprobe` per file, populates the catalog.
//!
//! ffprobe runs as a bounded-concurrency subprocess (no libav FFI); ingestion is
//! idempotent via `scan_state` (mtime + size). Dolby Vision + HDR extraction detail:
//! `docs/.tasks/10-phase1-foundation-data.md`. Implemented in Phase 1.
//!
//! ## Usage (from the `api` binary at boot)
//!
//! ```no_run
//! # use std::sync::Arc;
//! # use std::path::PathBuf;
//! # async fn boot(db: medi_db::Db) -> anyhow::Result<()> {
//! let cfg = medi_ingest::WorkerConfig::new(PathBuf::from("/media"));
//! // Wire the API cache's invalidate_all here; a no-op closure works standalone.
//! let invalidate: medi_ingest::Invalidator = Arc::new(|| {});
//!
//! // One-shot scan at boot, then watch for changes for the process lifetime. The
//! // `ScanTrigger` lets the API request an out-of-band scan (library create / rescan).
//! let (_trigger, signal) = medi_ingest::ScanTrigger::new();
//! medi_ingest::run_scan(&db, &cfg, &invalidate).await?;
//! tokio::spawn(medi_ingest::watch(db, cfg, invalidate, signal));
//! # Ok(()) }
//! ```

pub mod enrich;
pub mod ffprobe;
pub mod scanner;
pub mod status;
pub mod worker;

pub use enrich::run_enrichment;
pub use scanner::{scan, scan_root, Classification, DiscoveredFile, KindHint};
pub use status::{EnrichmentStatus, LastEnrichment, LastScan, StatusSnapshot};
pub use worker::{run_scan, watch, Invalidator, ScanSignal, ScanTrigger, WorkerConfig};
