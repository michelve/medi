//! Shared application state handed to every handler via axum `State`.
//!
//! Holds the database handle, the catalog response cache, and the resolved config.
//! Cheap to clone — `Db` wraps an `Arc`'d pool, `ResponseCache` an `Arc`'d moka
//! cache, and `AppConfig` is small — so axum can clone it per request freely.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use medi_core::AppConfig;
use medi_db::Db;
use medi_transcode::{HwCaps, SessionManager};

use crate::cache::ResponseCache;

/// Injected into handlers as `State<AppState>`.
#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub cache: ResponseCache,
    pub config: Arc<AppConfig>,
    /// Live HLS transcode sessions + host capabilities (Phase 2). Present even on a
    /// GPU-less host (software-only caps), so `/api/stream` always has a target.
    pub transcode: SessionManager,
    pub caps: Arc<HwCaps>,
    /// Metadata enrichment context (`docs/.tasks/60` Phase A). `None` when no provider is
    /// configured — the manual metadata endpoints (`refresh`/`matches`/`match`) then
    /// return `501 not_implemented` instead of silently doing nothing.
    pub enrich: Option<medi_metadata::EnrichContext>,
    /// Set while a `POST /api/metadata/backfill` (`docs/.tasks/91`) task is running, so a
    /// re-hit is idempotent (acknowledged as "already running") rather than spawning a
    /// second concurrent backfill. `Arc` so every cloned handler state shares the one flag.
    pub backfill_running: Arc<AtomicBool>,
    /// Handle to ask the ingest worker to scan now (library create / `POST
    /// /api/libraries/:id/scan`). `None` when ingestion isn't running (no MEDIA_DIR, or in
    /// tests) — the scan endpoint then just acknowledges without a real trigger.
    pub scan_trigger: Option<medi_ingest::ScanTrigger>,
    /// Shared enrichment/scan status the worker updates and `GET /api/status` reads
    /// (`docs/.tasks/96`). `None` in tests / when ingestion isn't wired — status then reports
    /// all-defaults ("nothing has run").
    pub status: Option<medi_ingest::EnrichmentStatus>,
}

impl AppState {
    pub fn new(
        db: Db,
        cache: ResponseCache,
        config: AppConfig,
        transcode: SessionManager,
        caps: HwCaps,
    ) -> Self {
        Self {
            db,
            cache,
            config: Arc::new(config),
            transcode,
            caps: Arc::new(caps),
            enrich: None,
            backfill_running: Arc::new(AtomicBool::new(false)),
            scan_trigger: None,
            status: None,
        }
    }

    /// Attach the metadata enrichment context (built at boot from config).
    pub fn with_enrichment(mut self, ctx: medi_metadata::EnrichContext) -> Self {
        self.enrich = Some(ctx);
        self
    }

    /// Attach the ingest scan trigger (built at boot alongside the worker) so the library
    /// create / rescan endpoints can request an immediate scan.
    pub fn with_scan_trigger(mut self, trigger: medi_ingest::ScanTrigger) -> Self {
        self.scan_trigger = Some(trigger);
        self
    }

    /// Attach the shared enrichment/scan status handle so `GET /api/status` can report it
    /// (`docs/.tasks/96`).
    pub fn with_status(mut self, status: medi_ingest::EnrichmentStatus) -> Self {
        self.status = Some(status);
        self
    }
}
