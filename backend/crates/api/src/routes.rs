//! HTTP routing + handlers for the contract in `docs/.tasks/02-api-contract.md`.
//!
//! Catalog routes (`/api/library`, `/api/movies/:id`, `/api/series/:id`) are backed
//! by `medi_db` through the moka cache and are fully live. Playback (`/api/stream`,
//! `/api/direct`, `/api/hls/*`) is live via the `transcode` crate (Phase 2):
//! `/api/stream` runs the direct-play-vs-transcode decision (tuned for Apple TV),
//! starting an fMP4/CMAF HLS session on a transcode; `/api/direct` byte-range-streams
//! the source; `/api/hls/*` serves a session's playlist/segments. Preview/trickplay
//! *generation* is Phase 3 (the serving routes are static `ServeDir`s already). Static
//! roots (`/api/images` and the asset dirs) are served via `tower_http::ServeDir`.
//!
//! Every DB call runs under `tokio::task::spawn_blocking` — rusqlite is synchronous
//! and must never block the async runtime (`01-db-schema.md` §Scaling notes).

use axum::extract::{Path, Query, Request, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Json;
use axum::Router;
use serde::Deserialize;
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

use medi_db::queries::{self, LibraryCursor, LibrarySort};
use medi_transcode::{
    audio_target, decide, AudioCodec, AudioTarget, ClientProfile, Decision, PLAYLIST_NAME,
};

use crate::cursor;
use crate::dto::{LibraryItem, LibraryPage, StreamDecision};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Build the full application router over the shared [`AppState`].
pub fn router(state: AppState) -> Router {
    let images = ServeDir::new(state.config.images_dir());
    let previews = ServeDir::new(state.config.previews_dir());
    let trickplay = ServeDir::new(state.config.trickplay_dir());

    Router::new()
        // Liveness (no state needed, but uniform under /api).
        .route("/api/health", get(health))
        // Catalog — cached, ETag'd, keyset-paginated.
        .route("/api/library", get(library))
        .route("/api/movies/:id", get(movie_detail))
        .route("/api/series/:id", get(series_detail))
        // Playback — Phase 2 (transcode crate).
        .route("/api/stream/:file_id", get(stream_decision))
        .route("/api/direct/:file_id", get(direct_play))
        .route("/api/hls/:session_id/:file", get(hls_asset))
        // Generated assets — Phase 3 (assets crate). Served as static files; a
        // missing file is a natural 404 from ServeDir.
        .nest_service("/api/preview", previews)
        .nest_service("/api/trickplay", trickplay)
        // Artwork.
        .nest_service("/api/images", images)
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Liveness
// ---------------------------------------------------------------------------

/// Liveness probe for the Docker healthcheck. `200 "ok"`.
pub async fn health() -> &'static str {
    "ok"
}

// ---------------------------------------------------------------------------
// GET /api/library
// ---------------------------------------------------------------------------

/// Query params for `/api/library`: `cursor`, `limit`, `sort`.
#[derive(Debug, Deserialize)]
pub struct LibraryQuery {
    /// Opaque keyset cursor from a previous page's `next_cursor`. Absent → first page.
    #[serde(default)]
    cursor: Option<String>,
    /// Page size; clamped by the DB layer to `[1, MAX_LIMIT]`.
    #[serde(default)]
    limit: Option<u32>,
    /// `sort_title` (default) or `added_at`.
    #[serde(default)]
    sort: Option<String>,
}

/// Paginated unified catalog (movie + series cards). Cached with ETag.
async fn library(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<LibraryQuery>,
) -> ApiResult<Response> {
    let sort = parse_sort(q.sort.as_deref())?;
    let limit = q.limit.unwrap_or(queries::DEFAULT_LIMIT);
    // Treat an absent OR empty `cursor` as "first page".
    let cursor = match q.cursor.as_deref() {
        Some(token) if !token.is_empty() => Some(cursor::decode(token)?),
        _ => None,
    };

    // Cache key is the normalized request identity: path + the params that change
    // the body. Two clients asking for the same page share one cached entry.
    let cache_key = format!(
        "library?sort={}&limit={}&cursor={}",
        sort_label(sort),
        limit,
        q.cursor.as_deref().unwrap_or("")
    );

    let db = state.db.clone();
    state
        .cache
        .get_or_render(cache_key, &headers, move || async move {
            let cards = run_blocking(&db, move |conn| {
                queries::list_library(conn, sort, cursor.as_ref(), limit)
            })
            .await?;

            // The next cursor is the ordering key of the last row returned; a short
            // (or empty) page means the list is exhausted → `next_cursor: null`.
            let next_cursor = if (cards.len() as u32) < clamp(limit) {
                None
            } else {
                cards.last().map(|last| {
                    let sort_value = match sort {
                        LibrarySort::SortTitle => last.sort_title.clone(),
                        LibrarySort::AddedAt => last.added_at.to_string(),
                    };
                    cursor::encode(&LibraryCursor {
                        sort_value,
                        kind_tag: kind_tag(last),
                        id: last.id,
                    })
                })
            };

            let page = LibraryPage {
                items: cards.into_iter().map(LibraryItem::from_card).collect(),
                next_cursor,
            };
            serde_json::to_vec(&page).map_err(|e| ApiError::internal(e.to_string()))
        })
        .await
}

fn parse_sort(raw: Option<&str>) -> ApiResult<LibrarySort> {
    match raw {
        None | Some("") | Some("sort_title") => Ok(LibrarySort::SortTitle),
        Some("added_at") => Ok(LibrarySort::AddedAt),
        Some(other) => Err(ApiError::bad_request(format!(
            "unknown sort '{other}' (expected 'sort_title' or 'added_at')"
        ))),
    }
}

fn sort_label(sort: LibrarySort) -> &'static str {
    match sort {
        LibrarySort::SortTitle => "sort_title",
        LibrarySort::AddedAt => "added_at",
    }
}

/// The kind tag stored in the cursor, matching the DB `UNION` discriminator.
fn kind_tag(card: &medi_db::models::LibraryCard) -> i64 {
    match card.kind {
        medi_db::models::LibraryKind::Movie => 0,
        medi_db::models::LibraryKind::Series => 1,
    }
}

/// Mirror of the DB layer's limit clamp, so the "short page ⇒ exhausted" check
/// compares against the *effective* page size the query used.
fn clamp(limit: u32) -> u32 {
    limit.clamp(1, queries::MAX_LIMIT)
}

// ---------------------------------------------------------------------------
// GET /api/movies/:id  and  GET /api/series/:id
// ---------------------------------------------------------------------------

/// Movie detail: movie + media_files + credits. Cached with ETag.
async fn movie_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    let key = format!("movie/{id}");
    let db = state.db.clone();
    state
        .cache
        .get_or_render(key, &headers, move || async move {
            let detail =
                run_blocking(&db, move |conn| queries::get_movie_detail(conn, id)).await?;
            serde_json::to_vec(&detail).map_err(|e| ApiError::internal(e.to_string()))
        })
        .await
}

/// Series detail: series + seasons + episodes + credits. Cached with ETag.
async fn series_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    let key = format!("series/{id}");
    let db = state.db.clone();
    state
        .cache
        .get_or_render(key, &headers, move || async move {
            let detail =
                run_blocking(&db, move |conn| queries::get_series_detail(conn, id)).await?;
            serde_json::to_vec(&detail).map_err(|e| ApiError::internal(e.to_string()))
        })
        .await
}

// ---------------------------------------------------------------------------
// Playback — Phase 2 (transcode crate).
// ---------------------------------------------------------------------------

/// Client capability hints for `/api/stream` (query params). Absent → the Apple TV 4K
/// baseline. The client sends what its display/decoder can present so the server can
/// direct-play when possible (e.g. Dolby Vision to a DV-capable Apple TV).
#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    /// `1`/`true` when the connected display is HDR-capable. Omitted → assume HDR
    /// (Apple TV 4K default); pass `hdr=0` to force SDR tone-mapping.
    #[serde(default)]
    hdr: Option<String>,
    /// `1`/`true` when the client+display can present Dolby Vision.
    #[serde(default)]
    dv: Option<String>,
    /// Force a conservative SDR/H.264 profile (testing / legacy clients).
    #[serde(default)]
    sdr: Option<String>,
}

fn truthy(v: Option<&str>) -> Option<bool> {
    match v {
        Some("1") | Some("true") | Some("yes") => Some(true),
        Some("0") | Some("false") | Some("no") => Some(false),
        _ => None,
    }
}

/// Build the effective [`ClientProfile`] from the request hints, starting from the
/// Apple TV 4K baseline (or the SDR baseline when `sdr=1`).
fn client_profile(q: &StreamQuery) -> ClientProfile {
    if truthy(q.sdr.as_deref()) == Some(true) {
        return ClientProfile::sdr_baseline();
    }
    let mut p = ClientProfile::apple_tv_4k();
    if let Some(hdr) = truthy(q.hdr.as_deref()) {
        p.hdr_display = hdr;
    }
    if let Some(dv) = truthy(q.dv.as_deref()) {
        p.dolby_vision = dv;
    }
    // DV can only be presented on an HDR display.
    if !p.hdr_display {
        p.dolby_vision = false;
    }
    p
}

/// `GET /api/stream/:file_id` — direct-play vs transcode decision.
///
/// Reads the `media_files` row, reconstructs its [`medi_core::MediaProfile`], and calls
/// `transcode::decide` with the client hints and host caps. On a transcode it starts an
/// HLS session and returns its playlist URL; on direct-play it returns `/api/direct`.
async fn stream_decision(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    Query(q): Query<StreamQuery>,
) -> ApiResult<Response> {
    let db = state.db.clone();
    let file = run_blocking(&db, move |conn| queries::get_media_file(conn, file_id)).await?;

    let Some(profile) = file.profile() else {
        // Unprobed row (no width/height) — nothing to decide over yet.
        return Err(ApiError::unavailable("file has not been probed yet"));
    };

    let client = client_profile(&q);
    // Audio codec is not yet extracted into `media_files` (Phase 1 probed video only),
    // so assume a client-supported track: this never forces a *needless* audio remux.
    // A future schema column feeds the real audio codec here.
    let audio = AudioCodec::Aac;
    let container = file.container.as_deref().unwrap_or("");

    let decision = decide(&profile, audio, container, &client, &state.caps);
    tracing::info!(
        file_id,
        mode = decision.mode(),
        reason = decision.reason(),
        "stream decision",
    );

    let url = match &decision {
        Decision::DirectPlay { .. } => format!("/api/direct/{file_id}"),
        Decision::Transcode { target, .. } => {
            let src = std::path::PathBuf::from(&file.path);
            let audio_tgt = match audio_target(audio, &client) {
                Some(c) => AudioTarget::Transcode(c),
                None => AudioTarget::Copy,
            };
            let session_id = state
                .transcode
                .start(&src, target, audio_tgt)
                .await
                .map_err(map_session_err)?;
            format!("/api/hls/{session_id}/{PLAYLIST_NAME}")
        }
    };

    let body = StreamDecision {
        file_id,
        mode: decision.mode(),
        reason: decision.reason(),
        url,
    };
    Ok(Json(body).into_response())
}

/// `GET /api/direct/:file_id` — `Range`-capable direct byte stream of the source file.
/// Serves the bytes at `MediaFile.path` with `tower_http::ServeFile` (handles `Range`,
/// `If-Range`, and `206 Partial Content`). No transcode.
async fn direct_play(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    req: Request,
) -> ApiResult<Response> {
    let db = state.db.clone();
    let file = run_blocking(&db, move |conn| queries::get_media_file(conn, file_id)).await?;

    let serve = ServeFile::new(&file.path);
    // ServeFile is infallible in its error type; a missing file yields a 404 response.
    let resp = serve
        .oneshot(req)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(resp.into_response())
}

/// `GET /api/hls/:session_id/:file` — HLS playlist / init / segment for a live session.
/// Refreshes the session's idle timer and serves the on-disk fMP4 artifact.
async fn hls_asset(
    State(state): State<AppState>,
    Path((session_id, file)): Path<(String, String)>,
    req: Request,
) -> ApiResult<Response> {
    let path = state
        .transcode
        .resolve_file(&session_id, &file)
        .await
        .map_err(map_session_err)?;

    // The file may not exist yet if ffmpeg hasn't written this segment — ServeFile
    // returns a natural 404 the client retries.
    let serve = ServeFile::new(&path);
    let resp = serve
        .oneshot(req)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(resp.into_response())
}

/// Map a transcode session error onto the API error model. A full session table is a
/// `409` (`docs/.tasks/20` §Scaling); an unknown session is a `404`.
fn map_session_err(e: medi_transcode::SessionError) -> ApiError {
    use medi_transcode::SessionError;
    match e {
        SessionError::CapacityReached(n) => {
            ApiError::busy(format!("transcode capacity reached ({n} sessions)"))
        }
        SessionError::NotFound => ApiError::not_found("no such transcode session"),
        other => {
            tracing::error!(error = %other, "transcode session error");
            ApiError::internal("transcode error")
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Run a synchronous rusqlite closure on the blocking pool and map its errors into
/// the API error model. Checks out a pooled connection inside the blocking task so
/// the checkout itself never touches the async runtime.
async fn run_blocking<T, F>(db: &medi_db::Db, f: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(&rusqlite::Connection) -> medi_db::DbResult<T> + Send + 'static,
{
    let db = db.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        f(&conn)
    })
    .await?; // JoinError → 500
    Ok(result?) // DbError → mapped status
}
