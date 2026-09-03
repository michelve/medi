//! `medi-metadata` — pluggable descriptive-metadata providers and the enrichment
//! orchestration that fills the catalog's `overview` / artwork / cast columns.
//!
//! Shape (`docs/.tasks/60-metadata-and-libraries.md`):
//! - [`provider::MetadataProvider`] is the trait; [`tmdb::TmdbProvider`] (default) and
//!   [`omdb::OmdbProvider`] implement it.
//! - [`matcher`] scores provider candidates against the filename-parsed `(title, year)`.
//! - [`enrich::enrich_movie`] / [`enrich::enrich_series`] orchestrate one title:
//!   search → best-match → details → atomic artwork download → DB write, idempotently.
//!
//! Every provider call is bounded (a semaphore per provider plus the worker's own
//! fan-out cap) so a first-run scan of a large library respects provider rate limits,
//! and enrichment runs entirely off the request path.

pub mod enrich;
pub mod fanart;
pub mod matcher;
pub mod omdb;
pub mod provider;
pub mod reap;
pub mod tmdb;

pub use enrich::{
    backfill_genres_people, candidates_for, enrich_movie, enrich_series, enrich_with_id,
    BackfillReport, EnrichContext, EnrichOutcome, HttpFetcher, ImageFetcher,
};
pub use fanart::{
    parse_movie_logo, parse_movie_wallpaper, FanartArt, FanartClient, MovieArt,
};
pub use reap::{remove_title_images, sweep_orphan_images};
pub use provider::{
    Collection, CreditIn, Details, Genre, Match, MediaKind, MetadataProvider, PersonDetails,
    ProviderId, TrailerIn,
};

use std::sync::Arc;

use thiserror::Error;

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors from metadata providers and enrichment.
#[derive(Debug, Error)]
pub enum Error {
    /// The HTTP client failed to build or send (network, TLS, timeout).
    #[error("http error: {0}")]
    Http(String),

    /// A provider returned a non-success status or a logical error (unknown id, etc.).
    #[error("provider error: {0}")]
    Provider(String),

    /// A provider response could not be parsed into the expected shape.
    #[error("parse error: {0}")]
    Parse(String),

    /// A database read/write during enrichment failed.
    #[error("db error: {0}")]
    Db(#[from] medi_db::DbError),

    /// Filesystem error writing/renaming an artwork file.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// A join failure from `spawn_blocking` during a DB write.
    #[error("task join error: {0}")]
    Join(String),
}

impl From<tokio::task::JoinError> for Error {
    fn from(e: tokio::task::JoinError) -> Self {
        Error::Join(e.to_string())
    }
}

/// Construct the configured provider (`docs/.tasks/60` §Config additions), or `None`
/// when metadata is disabled or no key is set for the selected provider — the caller
/// then runs filename-only ingest with no error (graceful degradation).
///
/// Returns a trait object so the ingest worker holds one `Arc<dyn MetadataProvider>`
/// regardless of which service is active.
pub fn build_provider(cfg: &medi_core::AppConfig) -> Option<Arc<dyn MetadataProvider>> {
    if !cfg.metadata_enabled {
        tracing::info!("metadata enrichment disabled (METADATA_ENABLED=false)");
        return None;
    }
    let key = cfg.active_metadata_key()?; // None ⇒ no provider available
    let provider = cfg.metadata_provider.to_ascii_lowercase();
    let built: Result<Arc<dyn MetadataProvider>> = match provider.as_str() {
        "omdb" => omdb::OmdbProvider::new(key).map(|p| Arc::new(p) as Arc<dyn MetadataProvider>),
        // Default / unknown ⇒ TMDB.
        _ => tmdb::TmdbProvider::new(key, cfg.metadata_language.clone())
            .map(|p| Arc::new(p) as Arc<dyn MetadataProvider>),
    };
    match built {
        Ok(p) => {
            tracing::info!(provider = p.name(), "metadata provider ready");
            Some(p)
        }
        Err(err) => {
            tracing::warn!(error = %err, "failed to build metadata provider; enrichment disabled");
            None
        }
    }
}

/// Construct the fanart.tv title-logo client (`docs/.tasks/93`), or `None` when
/// `FANARTTV_API_KEY` is unset/empty — the logo feature is then inert (enrichment behaves
/// exactly as today, no request, no error). Returned as a trait object so [`EnrichContext`]
/// holds one `Arc<dyn FanartArt>` regardless of source.
pub fn build_fanart(cfg: &medi_core::AppConfig) -> Option<Arc<dyn FanartArt>> {
    let key = cfg.fanart_key()?; // None ⇒ feature off
    match FanartClient::new(key, &cfg.metadata_language) {
        Ok(c) => {
            tracing::info!("fanart.tv art client ready (logos + wallpapers)");
            Some(Arc::new(c) as Arc<dyn FanartArt>)
        }
        Err(err) => {
            tracing::warn!(error = %err, "failed to build fanart client; title logos disabled");
            None
        }
    }
}
