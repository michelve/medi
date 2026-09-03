//! `GET /api/status*` — enrichment & ingest observability (`docs/.tasks/96` Part A).
//!
//! The scheduler + enrichment pipeline already run; before this, the only signal was
//! `tracing` logs, so a correct-but-quiet system (e.g. a title left `unmatched` because its
//! folder name is junk, or logos off because `FANARTTV_API_KEY` is unset) looked broken.
//! These read-only endpoints expose:
//!   - per-`metadata_state` title counts (from the DB — the durable truth),
//!   - which providers are configured (from `AppConfig` — surfaces the missing fanart key),
//!   - what the last scan / enrichment pass did + whether the watcher is alive (from the
//!     in-memory [`medi_ingest::EnrichmentStatus`] the worker updates),
//!   - a list of `unmatched`/`failed` titles and of ffprobe-failed files to act on.

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use medi_db::queries;
use medi_db::writes::TitleKind;

use crate::error::{ApiError, ApiResult};
use crate::routes::run_blocking;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// GET /api/status
// ---------------------------------------------------------------------------

/// Per-kind, per-state title counts (`docs/.tasks/96`).
#[derive(Debug, Serialize)]
pub struct StateCounts {
    pub total: i64,
    pub matched: i64,
    pub pending: i64,
    pub unmatched: i64,
    pub failed: i64,
}

impl From<queries::MetadataStateCounts> for StateCounts {
    fn from(c: queries::MetadataStateCounts) -> Self {
        Self {
            total: c.total,
            matched: c.matched,
            pending: c.pending,
            unmatched: c.unmatched,
            failed: c.failed,
        }
    }
}

/// Whether a provider is configured, plus its name for the metadata provider.
#[derive(Debug, Serialize)]
pub struct ProviderStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub configured: bool,
}

/// The providers block: the active metadata provider and fanart.tv, each with whether its
/// key is set. `fanart.configured == false` is the durable signal that title logos are off
/// because `FANARTTV_API_KEY` is unset (`docs/.tasks/96` Diagnosis #2).
#[derive(Debug, Serialize)]
pub struct ProvidersStatus {
    pub metadata: ProviderStatus,
    pub fanart: ProviderStatus,
}

/// What the last scan did (mirrors [`medi_ingest::LastScan`], serialized).
#[derive(Debug, Serialize)]
pub struct LastScan {
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub written: u64,
    pub probe_failures: u64,
}

/// What the last enrichment pass did (mirrors [`medi_ingest::LastEnrichment`]).
#[derive(Debug, Serialize)]
pub struct LastEnrichment {
    pub finished_at: Option<i64>,
    pub matched: u64,
    pub unmatched: u64,
    pub failed: u64,
}

/// Worker liveness + schedule knobs.
#[derive(Debug, Serialize)]
pub struct WorkersStatus {
    pub watcher_alive: bool,
    pub backfill_interval_hours: u32,
}

/// The full `GET /api/status` envelope (`docs/.tasks/96`).
#[derive(Debug, Serialize)]
pub struct SystemStatus {
    pub version: &'static str,
    pub media_dir_present: bool,
    pub counts: StatusCounts,
    pub providers: ProvidersStatus,
    pub last_scan: LastScan,
    pub last_enrichment: LastEnrichment,
    pub workers: WorkersStatus,
}

/// Per-kind counts wrapper.
#[derive(Debug, Serialize)]
pub struct StatusCounts {
    pub movies: StateCounts,
    pub series: StateCounts,
}

/// `GET /api/status` — one call answering "is enrichment working, what's configured, and
/// what has run". Uncached: the counts are a fast grouped scan and the status must be live.
pub async fn status(State(state): State<AppState>) -> ApiResult<Response> {
    let db = state.db.clone();
    let (movies, series) = run_blocking(&db, move |conn| {
        let m = queries::metadata_state_counts(conn, TitleKind::Movie)?;
        let s = queries::metadata_state_counts(conn, TitleKind::Series)?;
        Ok((m, s))
    })
    .await?;

    let cfg = &state.config;
    let providers = ProvidersStatus {
        metadata: ProviderStatus {
            name: Some(cfg.metadata_provider.clone()),
            configured: cfg.active_metadata_key().is_some(),
        },
        fanart: ProviderStatus {
            name: None,
            configured: cfg.fanart_enabled(),
        },
    };

    // The in-memory scan/enrichment status the worker records (`None` before anything runs
    // or when observability isn't wired — then all-defaults, which serializes cleanly).
    let snap = state
        .status
        .as_ref()
        .map(|s| s.snapshot())
        .unwrap_or_default();

    let body = SystemStatus {
        version: env!("CARGO_PKG_VERSION"),
        media_dir_present: cfg.media_dir.is_dir(),
        counts: StatusCounts {
            movies: movies.into(),
            series: series.into(),
        },
        providers,
        last_scan: LastScan {
            started_at: snap.last_scan.started_at,
            finished_at: snap.last_scan.finished_at,
            written: snap.last_scan.written,
            probe_failures: snap.last_scan.probe_failures,
        },
        last_enrichment: LastEnrichment {
            finished_at: snap.last_enrichment.finished_at,
            matched: snap.last_enrichment.matched,
            unmatched: snap.last_enrichment.unmatched,
            failed: snap.last_enrichment.failed,
        },
        workers: WorkersStatus {
            watcher_alive: snap.watcher_alive,
            backfill_interval_hours: cfg.backfill_interval_hours,
        },
    };
    Ok(Json(body).into_response())
}

// ---------------------------------------------------------------------------
// GET /api/status/unmatched
// ---------------------------------------------------------------------------

/// Query params for `/api/status/unmatched`: `kind`, keyset `after`, `limit`.
#[derive(Debug, Deserialize)]
pub struct UnmatchedQuery {
    /// `movie` (default) or `series`.
    #[serde(default)]
    kind: Option<String>,
    /// Keyset cursor: the `id` of the last row of the prior page.
    #[serde(default)]
    after: Option<i64>,
    #[serde(default)]
    limit: Option<u32>,
}

/// One unmatched/failed title row.
#[derive(Debug, Serialize)]
pub struct UnmatchedItem {
    pub id: i64,
    pub kind: &'static str,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i64>,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// A page of unmatched titles.
#[derive(Debug, Serialize)]
pub struct UnmatchedPage {
    pub items: Vec<UnmatchedItem>,
    pub next_cursor: Option<i64>,
}

/// `GET /api/status/unmatched?kind=movie&after=&limit=` — the titles the operator can act on
/// (fix a folder name, or pin a match). Keyset-paginated by id.
pub async fn unmatched(
    State(state): State<AppState>,
    Query(q): Query<UnmatchedQuery>,
) -> ApiResult<Response> {
    let (kind, kind_str) = match q.kind.as_deref() {
        Some("series") => (TitleKind::Series, "series"),
        _ => (TitleKind::Movie, "movie"),
    };
    let limit = q.limit.unwrap_or(queries::DEFAULT_LIMIT);
    let after = q.after;
    let db = state.db.clone();
    let rows = run_blocking(&db, move |conn| {
        queries::list_unmatched(conn, kind, after, limit)
    })
    .await?;

    let next_cursor = if (rows.len() as u32) >= limit.clamp(1, queries::MAX_LIMIT) {
        rows.last().map(|r| r.id)
    } else {
        None
    };
    let items = rows
        .into_iter()
        .map(|r| UnmatchedItem {
            id: r.id,
            kind: kind_str,
            title: r.title,
            year: r.year,
            state: r.state,
            path: r.path,
        })
        .collect();
    Ok(Json(UnmatchedPage { items, next_cursor }).into_response())
}

// ---------------------------------------------------------------------------
// GET /api/status/probe-failures
// ---------------------------------------------------------------------------

/// Query params for `/api/status/probe-failures`: keyset `after` (a `last_attempt_at`), `limit`.
#[derive(Debug, Deserialize)]
pub struct ProbeFailuresQuery {
    #[serde(default)]
    after: Option<i64>,
    #[serde(default)]
    limit: Option<u32>,
}

/// One ffprobe-failed file.
#[derive(Debug, Serialize)]
pub struct ProbeFailureItem {
    pub path: String,
    pub error: String,
    pub last_attempt_at: i64,
}

/// A page of probe failures.
#[derive(Debug, Serialize)]
pub struct ProbeFailuresPage {
    pub items: Vec<ProbeFailureItem>,
    pub next_cursor: Option<i64>,
}

/// `GET /api/status/probe-failures` — files ffprobe could not read (bad container, truncated
/// download, unsupported codec), so a "silently missing" title is explainable.
pub async fn probe_failures(
    State(state): State<AppState>,
    Query(q): Query<ProbeFailuresQuery>,
) -> ApiResult<Response> {
    let limit = q.limit.unwrap_or(queries::DEFAULT_LIMIT);
    let after = q.after;
    let db = state.db.clone();
    let rows = run_blocking(&db, move |conn| {
        queries::list_probe_failures(conn, after, limit)
    })
    .await?;

    let next_cursor = if (rows.len() as u32) >= limit.clamp(1, queries::MAX_LIMIT) {
        rows.last().map(|r| r.last_attempt_at)
    } else {
        None
    };
    let items = rows
        .into_iter()
        .map(|r| ProbeFailureItem {
            path: r.path,
            error: r.error,
            last_attempt_at: r.last_attempt_at,
        })
        .collect();
    Ok(Json(ProbeFailuresPage { items, next_cursor }).into_response())
}

// ---------------------------------------------------------------------------
// POST /api/metadata/enrich
// ---------------------------------------------------------------------------

/// `POST /api/metadata/enrich` — kick a `run_enrichment` pass over `pending`/`failed` titles
/// on demand (`docs/.tasks/96` Part D), so the operator need not wait for the next scan or the
/// periodic backstop. `501` when no provider is configured. Runs in the background and returns
/// `202` immediately; the resulting tallies land in `GET /api/status`.
pub async fn metadata_enrich(State(state): State<AppState>) -> ApiResult<Response> {
    let ctx = state.enrich.as_ref().cloned().ok_or_else(|| {
        ApiError::not_implemented("metadata provider not configured (set TMDB_API_KEY)")
    })?;
    let db = state.db.clone();
    let cache = state.cache.clone();
    let status = state.status.clone();
    tokio::spawn(async move {
        let invalidate: medi_ingest::Invalidator =
            std::sync::Arc::new(move || cache.invalidate_all());
        if let Err(err) =
            medi_ingest::run_enrichment(&db, &ctx, 4, &invalidate, status.as_ref()).await
        {
            tracing::error!(error = %err, "manual enrichment pass failed");
        }
    });
    Ok((
        axum::http::StatusCode::ACCEPTED,
        Json(serde_json::json!({ "status": "accepted" })),
    )
        .into_response())
}
