//! Shared application state handed to every handler via axum `State`.
//!
//! Holds the database handle, the catalog response cache, and the resolved config.
//! Cheap to clone — `Db` wraps an `Arc`'d pool, `ResponseCache` an `Arc`'d moka
//! cache, and `AppConfig` is small — so axum can clone it per request freely.

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
        }
    }

    /// Attach the metadata enrichment context (built at boot from config).
    pub fn with_enrichment(mut self, ctx: medi_metadata::EnrichContext) -> Self {
        self.enrich = Some(ctx);
        self
    }
}
