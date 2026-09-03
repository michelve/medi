//! HTTP routing + handlers for the contract in `docs/.tasks/02-api-contract.md`.
//!
//! Catalog routes (`/api/library`, `/api/movies/:id`, `/api/series/:id`) are backed
//! by `medi_db` through the moka cache and are fully live. Playback (`/api/stream`,
//! `/api/direct`, `/api/hls/*`) is live via the `transcode` crate (Phase 2):
//! `/api/stream` runs the direct-play-vs-transcode decision (tuned for Apple TV),
//! starting an fMP4/CMAF HLS session on a transcode; `/api/direct` byte-range-streams
//! the source; `/api/hls/*` serves a session's playlist/segments. Preview/trickplay
//! *generation* is Phase 3. `/api/preview` and `/api/images` are static `ServeDir`s;
//! `/api/trickplay/:file` is a small `ServeFile` handler so the sibling
//! `/api/trickplay/:file_id/meta` grid-metadata route can coexist (Phase 5 Part A).
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

use medi_core::{AudioCodec, ImmersiveAudio, Platform, QualityProfile};
use medi_db::queries::{self, LibraryCursor, LibrarySort};
use medi_transcode::{
    audio_plan, decide, AudioPlan, AudioTarget, AudioTrack, ClientProfile, Decision, Quality,
    PLAYLIST_NAME,
};

use crate::cursor;
use crate::dto::{
    BackfillResponse, CategoryRow, GenreListItem, LibraryItem, LibraryPage, LibraryRows,
    MatchCandidate, MatchRequest, MatchesResponse, PersonPage, RefreshResponse, StreamDecision,
    TrickplayMeta,
};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

use medi_db::writes::TitleKind;
use medi_metadata::{EnrichOutcome, ProviderId};

/// Build the full application router over the shared [`AppState`].
pub fn router(state: AppState) -> Router {
    let images = ServeDir::new(state.config.images_dir());
    let previews = ServeDir::new(state.config.previews_dir());
    let web_dir = state.config.web_dir();

    Router::new()
        // Liveness (no state needed, but uniform under /api).
        .route("/api/health", get(health))
        // Enrichment & ingest observability (`docs/.tasks/96`): counts, provider config,
        // last-scan/enrichment, and lists of unmatched titles + probe failures to act on.
        .route("/api/status", get(crate::status::status))
        .route("/api/status/unmatched", get(crate::status::unmatched))
        .route("/api/status/probe-failures", get(crate::status::probe_failures))
        // Kick a pending-title enrichment pass on demand (`docs/.tasks/96` Part D).
        .route("/api/metadata/enrich", axum::routing::post(crate::status::metadata_enrich))
        // Catalog — cached, ETag'd, keyset-paginated.
        .route("/api/library", get(library))
        // Landing-page category rows (`docs/.tasks/91`): "Recently Added" + top genres, one
        // request. Placed before `/api/library/:id`-style routes would be (there are none).
        .route("/api/library/rows", get(library_rows))
        .route("/api/movies/:id", get(movie_detail))
        .route("/api/series/:id", get(series_detail))
        // Genres & discovery (`docs/.tasks/91` Phase A) — cached + ETag'd.
        .route("/api/genres", get(genres_list))
        .route("/api/genres/:id", get(genre_titles))
        // Person pages (`docs/.tasks/91` Phase B) — cached + ETag'd.
        .route("/api/people/:id", get(person_page))
        // Kick a one-shot genre/person backfill over already-matched titles. Reuses the
        // `require_enrich` 501-when-unconfigured guard.
        .route("/api/metadata/backfill", axum::routing::post(metadata_backfill))
        // Metadata enrichment — Phase A (`docs/.tasks/60`). Manual controls over the
        // background enrichment: force a refresh, list candidate matches, pin a match.
        .route("/api/movies/:id/refresh", axum::routing::post(movie_refresh))
        .route("/api/movies/:id/matches", get(movie_matches))
        .route("/api/movies/:id/match", axum::routing::post(movie_match))
        // Libraries — Phase B (`docs/.tasks/60`). CRUD + per-library scan.
        .route(
            "/api/libraries",
            get(crate::libraries::list_libraries)
                .post(crate::libraries::create_library),
        )
        .route(
            "/api/libraries/:id",
            axum::routing::patch(crate::libraries::patch_library)
                .delete(crate::libraries::delete_library),
        )
        .route(
            "/api/libraries/:id/scan",
            axum::routing::post(crate::libraries::scan_library),
        )
        // Playback — Phase 2 (transcode crate).
        .route("/api/stream/:file_id", get(stream_decision))
        .route("/api/direct/:file_id", get(direct_play))
        .route("/api/hls/:session_id/:file", get(hls_asset))
        // Subtitles — Phase 90. Serve a text subtitle track as WebVTT (embedded → extract
        // + convert, cached under /config/subs; external .vtt → served directly). Image
        // tracks are not served here — the client requests a burn-in via /api/stream.
        .route("/api/subtitles/:file_id/:index", get(subtitle_vtt))
        // Generated assets — Phase 3 (assets crate). Served as static files; a
        // missing file is a natural 404 from ServeDir.
        .nest_service("/api/preview", previews)
        // Trickplay (`docs/.tasks/50` Part A). Two *plain* routes with different segment
        // counts — they do not overlap, so no wildcard-nest conflict:
        //   /api/trickplay/:file        → the sprite file (e.g. `88.jpg`, `88.bif`)
        //   /api/trickplay/:file_id/meta → the tiled-JPG grid geometry (JSON)
        .route("/api/trickplay/:file", get(trickplay_file))
        .route("/api/trickplay/:file_id/meta", get(trickplay_meta))
        // Artwork.
        .nest_service("/api/images", images)
        // Web SPA (`docs/.tasks/80`): serve the built browser client at `/` and any
        // non-`/api` path. `fallback_service` only fires when no route above matched, so
        // the `/api/*` contract is never shadowed. `not_found_service(index.html)` gives
        // SPA history-fallback — a deep link like `/movie/1` returns the app shell (200),
        // and the client router takes over. The assets ship in the image at `web_dir`.
        .fallback_service(web_spa(&web_dir))
        .with_state(state)
}

/// The static-file service for the web SPA: serve files from `web_dir`, falling back to
/// `index.html` for unmatched paths (client-side routing / deep links). `ServeFile`
/// returns `200`, so a deep link like `/movie/1` yields the shell with a `200`, not a
/// `404`.
fn web_spa(web_dir: &std::path::Path) -> ServeDir<ServeFile> {
    let index = web_dir.join("index.html");
    ServeDir::new(web_dir).fallback(ServeFile::new(index))
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
            let next_cursor = next_cursor_for(&cards, sort, limit);

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

/// Build the opaque `next_cursor` for a page of library cards, given the effective sort +
/// limit. Returns `None` when the page is short (list exhausted). Shared by `/api/library`
/// and `/api/genres/:id`, whose page shape is identical (`docs/.tasks/91`).
fn next_cursor_for(cards: &[medi_db::models::LibraryCard], sort: LibrarySort, limit: u32) -> Option<String> {
    if (cards.len() as u32) < clamp(limit) {
        return None;
    }
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
}

// ---------------------------------------------------------------------------
// Genres & discovery (`docs/.tasks/91` Phase A)
// ---------------------------------------------------------------------------

/// `GET /api/genres` — genres with ≥1 title, `count` = movies+series, ordered by count desc
/// then name. Cached + ETag'd via `get_or_render`; invalidated with the catalog whenever a
/// (re-)match or backfill can change a title's genres.
async fn genres_list(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    let db = state.db.clone();
    state
        .cache
        .get_or_render("genres".to_string(), &headers, move || async move {
            let genres = run_blocking(&db, queries::list_genres).await?;
            let body: Vec<GenreListItem> = genres.into_iter().map(GenreListItem::from).collect();
            serde_json::to_vec(&body).map_err(|e| ApiError::internal(e.to_string()))
        })
        .await
}

/// `GET /api/genres/:id?cursor=&limit=&sort=` — keyset-paginated grid of the titles in one
/// genre. **Identical page shape to `/api/library`** (same `LibraryPage`/`LibraryItem` +
/// cursor codec) so the client reuses its paging hook verbatim. Cached + ETag'd.
async fn genre_titles(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(genre_id): Path<i64>,
    Query(q): Query<LibraryQuery>,
) -> ApiResult<Response> {
    let sort = parse_sort(q.sort.as_deref())?;
    let limit = q.limit.unwrap_or(queries::DEFAULT_LIMIT);
    let cursor = match q.cursor.as_deref() {
        Some(token) if !token.is_empty() => Some(cursor::decode(token)?),
        _ => None,
    };

    let cache_key = format!(
        "genre/{genre_id}?sort={}&limit={}&cursor={}",
        sort_label(sort),
        limit,
        q.cursor.as_deref().unwrap_or("")
    );

    let db = state.db.clone();
    state
        .cache
        .get_or_render(cache_key, &headers, move || async move {
            let cards = run_blocking(&db, move |conn| {
                queries::list_by_genre(conn, genre_id, sort, cursor.as_ref(), limit)
            })
            .await?;
            let next_cursor = next_cursor_for(&cards, sort, limit);
            let page = LibraryPage {
                items: cards.into_iter().map(LibraryItem::from_card).collect(),
                next_cursor,
            };
            serde_json::to_vec(&page).map_err(|e| ApiError::internal(e.to_string()))
        })
        .await
}

/// `GET /api/people/:id` — a person page (`docs/.tasks/91` Phase B): the enriched person +
/// their in-library filmography (newest first). Mirrors `movie_detail` (blocking DB read
/// inside `get_or_render`); a `404` for an unknown id. Cache key `person/{id}`, invalidated
/// with the catalog whenever a (re-)match / backfill can change a person's filmography.
async fn person_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    let key = format!("person/{id}");
    let db = state.db.clone();
    state
        .cache
        .get_or_render(key, &headers, move || async move {
            let (person, films) = run_blocking(&db, move |conn| {
                let person = queries::get_person(conn, id)?;
                let films = queries::person_filmography(conn, id)?;
                Ok((person, films))
            })
            .await?;
            let filmography = films.into_iter().map(LibraryItem::from_card).collect();
            let page = PersonPage::build(person, filmography);
            serde_json::to_vec(&page).map_err(|e| ApiError::internal(e.to_string()))
        })
        .await
}

/// How many category rows the landing page shows beyond "Recently Added", and how many
/// tiles each row carries. Small caps: the rows are a browse teaser, not a full grid.
const ROWS_GENRE_COUNT: usize = 12;
const ROW_ITEM_CAP: u32 = 20;

/// `GET /api/library/rows` — the landing page's curated category rows in one request
/// (`docs/.tasks/91`): "Recently Added" (by `added_at`) plus the top genres by count, each
/// capped to `ROW_ITEM_CAP` tiles. A convenience aggregation so the landing page is one
/// request instead of N. Cached + ETag'd; invalidated with the catalog.
async fn library_rows(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    let db = state.db.clone();
    state
        .cache
        .get_or_render("library/rows".to_string(), &headers, move || async move {
            let rows = run_blocking(&db, move |conn| {
                let mut out: Vec<CategoryRow> = Vec::new();

                // "Recently Added" — the existing added_at ordering, capped.
                let recent = queries::list_library(
                    conn,
                    LibrarySort::AddedAt,
                    None,
                    ROW_ITEM_CAP,
                )?;
                if !recent.is_empty() {
                    out.push(CategoryRow {
                        key: "recently_added".to_string(),
                        title: "Recently Added".to_string(),
                        items: recent.into_iter().map(LibraryItem::from_card).collect(),
                        genre_id: None,
                    });
                }

                // Top-N genres by count, each capped. list_genres is already count-ordered.
                let genres = queries::list_genres(conn)?;
                for g in genres.into_iter().take(ROWS_GENRE_COUNT) {
                    let cards = queries::list_by_genre(
                        conn,
                        g.id,
                        LibrarySort::AddedAt,
                        None,
                        ROW_ITEM_CAP,
                    )?;
                    if cards.is_empty() {
                        continue;
                    }
                    out.push(CategoryRow {
                        key: format!("genre:{}", g.id),
                        title: g.name,
                        items: cards.into_iter().map(LibraryItem::from_card).collect(),
                        genre_id: Some(g.id),
                    });
                }
                Ok(out)
            })
            .await?;

            let body = LibraryRows { rows };
            serde_json::to_vec(&body).map_err(|e| ApiError::internal(e.to_string()))
        })
        .await
}

/// Query params for `POST /api/metadata/backfill`.
#[derive(Debug, Deserialize)]
pub struct BackfillQuery {
    /// `1`/`true` re-fetches **every** matched title (not just those missing genres), so a
    /// library already genre-enriched picks up newly-added fields (collections, trailers,
    /// person data). Default (absent) only touches titles still missing genres.
    #[serde(default)]
    force: Option<String>,
}

/// `POST /api/metadata/backfill[?force=1]` — kick a one-shot genre/person/collection/trailer
/// backfill over already-matched titles (`docs/.tasks/91` §Backfill). `501` when no provider
/// is configured (reuses `require_enrich`); otherwise spawns the backfill in the background
/// and returns `202`. Idempotent to re-hit: a concurrent request while one runs is
/// acknowledged as "already running" rather than starting a second pass. `?force=1` re-fetches
/// every matched title so a library already genre-enriched picks up the newer detail fields.
async fn metadata_backfill(
    State(state): State<AppState>,
    Query(q): Query<BackfillQuery>,
) -> ApiResult<Response> {
    // 501 when metadata is off — a client can tell "no provider" from a real failure.
    let ctx = require_enrich(&state)?.clone();
    let force = truthy(q.force.as_deref()) == Some(true);

    // Spawn under the shared run-flag guard; a concurrent run is acknowledged, not doubled.
    let already_running = spawn_backfill(&state, ctx, force);

    let body = BackfillResponse {
        status: "accepted",
        already_running,
    };
    Ok((axum::http::StatusCode::ACCEPTED, Json(body)).into_response())
}

/// Spawn a background genre/person/collection/fanart backfill, guarded by
/// [`AppState::backfill_running`] so at most one runs at a time. Returns whether one was
/// **already** running (in which case nothing new is spawned). Shared by the
/// `POST /api/metadata/backfill` handler and the periodic backfill task (`main.rs`) so both
/// honor the same single-flight guard.
pub fn spawn_backfill(
    state: &AppState,
    ctx: medi_metadata::EnrichContext,
    force: bool,
) -> bool {
    use std::sync::atomic::Ordering;

    let already_running = state.backfill_running.swap(true, Ordering::SeqCst);
    if !already_running {
        let flag = state.backfill_running.clone();
        let cache = state.cache.clone();
        tokio::spawn(async move {
            match medi_metadata::backfill_genres_people(&ctx, force).await {
                Ok(report) => {
                    tracing::info!(
                        processed = report.processed,
                        matched = report.matched,
                        failed = report.failed,
                        "genre/person/fanart backfill complete",
                    );
                    // A backfill can add genres/art to matched titles → drop cached catalog +
                    // genre + detail responses so the new rows/artwork show up.
                    if report.matched > 0 {
                        cache.invalidate_all();
                    }
                }
                Err(err) => tracing::error!(error = %err, "backfill failed"),
            }
            flag.store(false, Ordering::SeqCst);
        });
    }
    already_running
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
            let (detail, collection_cards) = run_blocking(&db, move |conn| {
                let detail = queries::get_movie_detail(conn, id)?;
                // The collection's other in-library movies (this one excluded), for the
                // "Collection" row (Task 91 detail extensions).
                let cards = match &detail.collection {
                    Some(c) => queries::collection_movies(conn, c.id, id)?,
                    None => Vec::new(),
                };
                Ok((detail, cards))
            })
            .await?;
            let body = crate::dto::MovieDetailResponse {
                detail,
                collection_movies: collection_cards
                    .into_iter()
                    .map(LibraryItem::from_card)
                    .collect(),
            };
            serde_json::to_vec(&body).map_err(|e| ApiError::internal(e.to_string()))
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
// Metadata enrichment — Phase A (`docs/.tasks/60`).
// ---------------------------------------------------------------------------

/// Borrow the enrichment context or return `501` when no provider is configured — so a
/// client can tell "metadata is off (no API key)" from an outright failure.
fn require_enrich(state: &AppState) -> ApiResult<&medi_metadata::EnrichContext> {
    state.enrich.as_ref().ok_or_else(|| {
        ApiError::not_implemented("metadata provider not configured (set TMDB_API_KEY)")
    })
}

/// `POST /api/movies/:id/refresh` — force re-enrichment of one movie, overwriting any
/// prior match (and its artwork, in place). Invalidates the response cache on success.
async fn movie_refresh(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    let ctx = require_enrich(&state)?;
    let outcome = medi_metadata::enrich_movie(ctx, id, true)
        .await
        .map_err(map_enrich_err)?;
    // Overview/art/cast may have changed → drop cached catalog + detail responses.
    state.cache.invalidate_all();

    let (outcome_str, provider_id) = match outcome {
        EnrichOutcome::Matched { provider_id } => ("matched", Some(provider_id)),
        EnrichOutcome::Unmatched => ("unmatched", None),
        EnrichOutcome::Skipped => ("skipped", None),
    };
    Ok(Json(RefreshResponse {
        id,
        outcome: outcome_str,
        provider_id,
    })
    .into_response())
}

/// Query params for `GET /api/movies/:id/matches`.
#[derive(Debug, Deserialize)]
pub struct MatchesQuery {
    /// Optional corrected search term overriding the filename-parsed title.
    #[serde(default)]
    query: Option<String>,
}

/// `GET /api/movies/:id/matches?query=` — list provider candidates for a movie so the
/// client can pick the right one when auto-match got it wrong (or left it `unmatched`).
async fn movie_matches(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<MatchesQuery>,
) -> ApiResult<Response> {
    let ctx = require_enrich(&state)?;
    let candidates = medi_metadata::candidates_for(ctx, TitleKind::Movie, id, q.query.as_deref())
        .await
        .map_err(map_enrich_err)?;
    let body = MatchesResponse {
        id,
        candidates: candidates
            .into_iter()
            .map(|m| MatchCandidate {
                provider_id: m.provider_id.to_token(),
                title: m.title,
                year: m.year,
                score: m.score,
            })
            .collect(),
    };
    Ok(Json(body).into_response())
}

/// `POST /api/movies/:id/match` — pin a specific provider id and re-enrich against it.
/// Body: `{ "provider_id": "tmdb:movie:329865" }`. Invalidates the cache on success.
async fn movie_match(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<MatchRequest>,
) -> ApiResult<Response> {
    let ctx = require_enrich(&state)?;
    let provider_id = ProviderId::from_token(&body.provider_id)
        .ok_or_else(|| ApiError::bad_request(format!("malformed provider_id '{}'", body.provider_id)))?;

    let outcome = medi_metadata::enrich_with_id(ctx, TitleKind::Movie, id, &provider_id)
        .await
        .map_err(map_enrich_err)?;
    state.cache.invalidate_all();

    let (outcome_str, pinned) = match outcome {
        EnrichOutcome::Matched { provider_id } => ("matched", Some(provider_id)),
        EnrichOutcome::Unmatched => ("unmatched", None),
        EnrichOutcome::Skipped => ("skipped", None),
    };
    Ok(Json(RefreshResponse {
        id,
        outcome: outcome_str,
        provider_id: pinned,
    })
    .into_response())
}

/// Map a metadata-enrichment error onto the API error model. A `NotFound` from the DB
/// read (unknown title id) is a `404`; a provider/HTTP failure is a `502`-style upstream
/// error surfaced as `503 unavailable` (transient — the client may retry).
fn map_enrich_err(e: medi_metadata::Error) -> ApiError {
    use medi_metadata::Error;
    match e {
        Error::Db(medi_db::DbError::NotFound) => ApiError::not_found("no such title"),
        Error::Db(other) => ApiError::from(other),
        Error::Provider(msg) | Error::Http(msg) => {
            tracing::warn!(error = %msg, "metadata provider error");
            ApiError::unavailable("metadata provider unavailable")
        }
        other => {
            tracing::error!(error = %other, "metadata enrichment error");
            ApiError::internal("metadata error")
        }
    }
}

// ---------------------------------------------------------------------------
// Playback — Phase 2 (transcode crate).
// ---------------------------------------------------------------------------

/// Client capability hints for `/api/stream` (query params, `docs/.tasks/70`). The
/// `platform` selects a static per-device default; explicitly-sent params overlay it, so
/// a Shield reporting `max_channels=6` overrides the Shield 8-channel default. Absent
/// entirely → the Apple TV 4K baseline (back-compat with the old `hdr`/`dv`/`sdr` hints).
#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    /// `appletv` | `shield` | `androidtv` — selects the static default profile.
    #[serde(default)]
    platform: Option<String>,
    /// `1`/`true` when the connected display is HDR-capable. Omitted → the platform
    /// default; pass `hdr=0` to force SDR tone-mapping.
    #[serde(default)]
    hdr: Option<String>,
    /// `1`/`true` when the client+display can present Dolby Vision.
    #[serde(default)]
    dv: Option<String>,
    /// Force a conservative SDR/H.264 profile (testing / legacy clients).
    #[serde(default)]
    sdr: Option<String>,
    /// `EXTRA_MAX_CHANNEL_COUNT` — max audio channels the sink accepts.
    #[serde(default)]
    max_channels: Option<u8>,
    /// ExoPlayer `EXTRA_ENCODINGS`, comma-separated (`eac3,ac3,aac,truehd,dtshd,eac3_joc`);
    /// `eac3_joc` ⇒ lossy Atmos passthrough. Overlays the platform default when present.
    #[serde(default)]
    audio: Option<String>,
    /// `MaxStreamingBitrate` in bits/sec (`0`/absent = uncapped).
    #[serde(default)]
    max_bitrate: Option<u64>,
    /// `original` | `auto` | `capped`.
    #[serde(default)]
    quality: Option<String>,
    /// Selected subtitle track (`docs/.tasks/90`): the ffprobe `stream_index` of an
    /// embedded track, or the synthetic id (negative) of an external sidecar. Only used
    /// together with `sub_burn=1` to force an image-subtitle burn-in; a text sub is
    /// fetched as a `.vtt` sidecar and does not affect the decision.
    #[serde(default)]
    sub: Option<i64>,
    /// `1` ⇒ burn the selected image subtitle into the video (the client sends this only
    /// for image tracks). A text sub is never sent as `sub_burn`.
    #[serde(default)]
    sub_burn: Option<String>,
    /// `1` ⇒ force a transcode even when the file would direct-play. The web player sends
    /// this to recover when a `direct` stream turns out to be unplayable in the browser
    /// (`<video>` raised `MEDIA_ERR_SRC_NOT_SUPPORTED` / `MEDIA_ERR_DECODE`) — it re-requests
    /// with this flag to demand an H.264+AAC HLS stream hls.js can always play.
    #[serde(default)]
    force_transcode: Option<String>,
}

fn truthy(v: Option<&str>) -> Option<bool> {
    match v {
        Some("1") | Some("true") | Some("yes") => Some(true),
        Some("0") | Some("false") | Some("no") => Some(false),
        _ => None,
    }
}

/// Parse the `platform` param to a [`Platform`] (unknown/absent → `Unknown`, which the
/// profile builder resolves to the Apple TV baseline).
fn parse_platform(raw: Option<&str>) -> Platform {
    match raw {
        Some("appletv") => Platform::AppleTv,
        Some("shield") => Platform::Shield,
        Some("androidtv") => Platform::AndroidTv,
        Some("web") => Platform::Web,
        _ => Platform::Unknown,
    }
}

/// Parse one `EXTRA_ENCODINGS` token to an [`AudioCodec`]. `eac3_joc` is Atmos, handled
/// by the caller as `atmos_passthrough`, so here it maps to `eac3`.
fn parse_audio_codec(tok: &str) -> Option<AudioCodec> {
    match tok.trim().to_ascii_lowercase().as_str() {
        "aac" => Some(AudioCodec::Aac),
        "ac3" => Some(AudioCodec::Ac3),
        "eac3" | "eac3_joc" => Some(AudioCodec::Eac3),
        "dts" => Some(AudioCodec::Dts),
        "dtshd" => Some(AudioCodec::DtsHd),
        "truehd" => Some(AudioCodec::TrueHd),
        "flac" => Some(AudioCodec::Flac),
        "opus" => Some(AudioCodec::Opus),
        "pcm" => Some(AudioCodec::Pcm),
        _ => None,
    }
}

fn parse_quality(raw: Option<&str>) -> QualityProfile {
    match raw {
        Some("original") => QualityProfile::Original,
        Some("capped") => QualityProfile::Capped,
        _ => QualityProfile::Auto,
    }
}

/// Build the effective [`ClientProfile`] from the request hints: start from the
/// `platform` static default (or the SDR baseline when `sdr=1`), then overlay any
/// explicitly-sent params (`docs/.tasks/70`).
fn client_profile(q: &StreamQuery) -> ClientProfile {
    if truthy(q.sdr.as_deref()) == Some(true) {
        return ClientProfile::sdr_baseline();
    }
    let mut p = ClientProfile::for_platform(parse_platform(q.platform.as_deref()));

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

    // Overlay a detected max-channel count (e.g. a 5.1 AVR on a Shield).
    if let Some(mc) = q.max_channels {
        p.max_channels = mc.max(2);
    }
    // Overlay a detected `EXTRA_ENCODINGS` set — the client is authoritative when it
    // reports one (Android). `eac3_joc` present ⇒ lossy Atmos passthrough.
    if let Some(raw) = q.audio.as_deref() {
        let tokens: Vec<&str> = raw.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        if !tokens.is_empty() {
            p.atmos_passthrough = tokens.iter().any(|t| t.eq_ignore_ascii_case("eac3_joc"));
            let codecs: Vec<AudioCodec> = tokens.iter().filter_map(|t| parse_audio_codec(t)).collect();
            if !codecs.is_empty() {
                p.audio_codecs = codecs;
            }
        }
    }
    p
}

/// Build the [`Quality`] control from the request (`docs/.tasks/70`).
fn quality_from(q: &StreamQuery) -> Quality {
    Quality {
        profile: parse_quality(q.quality.as_deref()),
        // A `0` cap means "uncapped".
        max_bitrate: q.max_bitrate.filter(|&b| b > 0),
    }
}

/// Pick the source audio track `decide` reasons over: the `is_default` track, else the
/// lowest `stream_index`. Maps the db read model into the transcode [`AudioTrack`]
/// descriptor. Returns the AAC-safe default when the file has no `audio_streams`
/// children (un-probed / pre-Task-70), so an unknown-audio row never forces a needless
/// remux (`docs/.tasks/70` §Backward compat).
fn default_audio_track(streams: &[medi_db::models::AudioStream]) -> AudioTrack {
    let Some(track) = streams
        .iter()
        .find(|s| s.is_default)
        .or_else(|| streams.iter().min_by_key(|s| s.stream_index))
    else {
        return AudioTrack::unknown_safe();
    };

    let codec = match track.codec.as_deref() {
        Some("aac") => AudioCodec::Aac,
        Some("ac3") => AudioCodec::Ac3,
        Some("eac3") => AudioCodec::Eac3,
        Some("dts") => AudioCodec::Dts,
        Some("dtshd") => AudioCodec::DtsHd,
        Some("truehd") => AudioCodec::TrueHd,
        Some("flac") => AudioCodec::Flac,
        Some("opus") => AudioCodec::Opus,
        Some("pcm") => AudioCodec::Pcm,
        // An unknown / absent codec is treated as the AAC-safe default: never force a
        // needless remux on a row we can't classify.
        _ => return AudioTrack::unknown_safe(),
    };
    let immersive = match track.immersive.as_str() {
        "dolby_atmos" => ImmersiveAudio::DolbyAtmos,
        "dts_x" => ImmersiveAudio::DtsX,
        _ => ImmersiveAudio::None,
    };
    AudioTrack {
        codec,
        channels: track.channels.unwrap_or(0).clamp(0, 255) as u8,
        immersive,
    }
}

/// Map a selected absolute ffprobe subtitle `stream_index` to the *subtitle-relative*
/// index ffmpeg's `0:s:<n>` selector uses — the track's position among the file's embedded
/// subtitle tracks, ordered by `stream_index`. Returns `None` unless the selected track
/// exists, is embedded (not an external sidecar), and is an **image** track (only image
/// subtitles are burned in; a text track rides as a WebVTT sidecar).
fn image_subtitle_relative_index(
    subs: &[medi_db::models::SubtitleStream],
    selected_stream_index: i64,
) -> Option<i64> {
    let mut embedded: Vec<&medi_db::models::SubtitleStream> =
        subs.iter().filter(|s| !s.is_external && s.stream_index.is_some()).collect();
    embedded.sort_by_key(|s| s.stream_index);
    embedded
        .iter()
        .position(|s| s.stream_index == Some(selected_stream_index))
        .filter(|&pos| embedded[pos].format == "image")
        .map(|pos| pos as i64)
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
    let quality = quality_from(&q);
    // The default audio track's real descriptor (`docs/.tasks/70`), or the AAC-safe
    // default when the file has no probed `audio_streams` — never a needless remux.
    let audio = default_audio_track(&file.audio_streams);
    let container = file.container.as_deref().unwrap_or("");

    let mut decision = decide(&profile, audio, container, &client, quality, &state.caps);

    // Image-subtitle burn-in (`docs/.tasks/90` §5): when the client selects an image track
    // with `sub_burn=1`, force a transcode that overlays it. The value threaded to
    // `command.rs` is the *subtitle-relative* index (`0:s:<n>`), not the absolute ffprobe
    // stream index — so map the selected `stream_index` to its position among the file's
    // embedded subtitle tracks. A text track (or a mismatched selection) is ignored here.
    if truthy(q.sub_burn.as_deref()) == Some(true) {
        if let Some(sel) = q.sub {
            if let Some(rel) = image_subtitle_relative_index(&file.subtitle_streams, sel) {
                decision = decision.with_burn_in(rel, &profile, &client, quality, &state.caps);
            }
        }
    }

    // `force_transcode=1`: promote a would-be direct-play into an HLS transcode. The web
    // player sends this to recover when a `direct` stream is unplayable in the browser.
    if truthy(q.force_transcode.as_deref()) == Some(true) {
        decision = decision.force_transcode(&profile, &client, quality, &state.caps);
    }

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
            let audio_tgt = match audio_plan(audio, &client) {
                AudioPlan::Copy => AudioTarget::Copy,
                AudioPlan::Transcode { codec, channels } => {
                    AudioTarget::Transcode { codec, channels }
                }
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

// ---------------------------------------------------------------------------
// GET /api/subtitles/:file_id/:index.vtt  — Phase 90 subtitle serving.
// ---------------------------------------------------------------------------

/// `GET /api/subtitles/:file_id/:index.vtt` — serve a **text** subtitle track as WebVTT
/// (`docs/.tasks/90` §5). `:index` is `<stream_index>.vtt` (embedded) or `ext<id>.vtt`
/// (an external sidecar, addressed by its `subtitle_streams` row id).
///
/// - **Embedded text** (SRT / ASS / SSA / mov_text) → extract + convert with
///   jellyfin-ffmpeg (`-map 0:s:<rel> -c:s webvtt`), cached at
///   `/config/subs/<file_id>.<index>.vtt`, then served. CPU-trivial — it does **not**
///   count against the GPU transcode-session cap.
/// - **External `.vtt`** → served directly. **External SRT/ASS/SSA** → converted the same
///   way and cached.
/// - **Image** tracks (PGS / VobSub) → `415` (they can't become text; the client requests
///   a burn-in via `/api/stream?...&sub_burn=1` instead).
async fn subtitle_vtt(
    State(state): State<AppState>,
    Path((file_id, index)): Path<(i64, String)>,
    req: Request,
) -> ApiResult<Response> {
    // Strip the trailing `.vtt` the client requests (the route matches `:index` greedily).
    let index = index.strip_suffix(".vtt").unwrap_or(&index).to_string();

    let db = state.db.clone();
    let file = run_blocking(&db, move |conn| queries::get_media_file(conn, file_id)).await?;

    // Resolve the selected subtitle row: `ext<id>` addresses an external sidecar by row id;
    // a bare number is an embedded track's ffprobe `stream_index`.
    let sub = if let Some(rest) = index.strip_prefix("ext") {
        let row_id: i64 = rest
            .parse()
            .map_err(|_| ApiError::bad_request("malformed external subtitle index"))?;
        file.subtitle_streams
            .iter()
            .find(|s| s.is_external && s.id == row_id)
            .cloned()
    } else {
        let stream_index: i64 = index
            .parse()
            .map_err(|_| ApiError::bad_request("malformed subtitle index"))?;
        file.subtitle_streams
            .iter()
            .find(|s| !s.is_external && s.stream_index == Some(stream_index))
            .cloned()
    };
    let sub = sub.ok_or_else(|| ApiError::not_found("no such subtitle track"))?;

    // Image subtitles can't become text — the client must request a burn-in instead.
    if sub.format == "image" {
        return Err(ApiError::unsupported_media_type(
            "image subtitle cannot be served as text; request a burn-in via /api/stream",
        ));
    }

    // An external .vtt is already WebVTT — serve it straight from /media (read-only).
    if sub.is_external {
        if let Some(path) = &sub.external_path {
            let is_vtt = std::path::Path::new(path)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("vtt"));
            if is_vtt {
                return serve_vtt_file(std::path::Path::new(path), req).await;
            }
        }
    }

    // Otherwise convert to WebVTT with jellyfin-ffmpeg and cache under /config/subs.
    let cache_path = state
        .config
        .subs_dir()
        .join(format!("{file_id}.{}.vtt", cache_key_for(&index)));

    if !cache_path.exists() {
        convert_to_webvtt(&file, &sub, &cache_path).await?;
    }
    serve_vtt_file(&cache_path, req).await
}

/// A filesystem-safe cache key fragment for a subtitle index (`ext5` / `2`). The index is
/// already `[0-9]+` or `ext[0-9]+`, so this is a passthrough kept as a single choke point.
fn cache_key_for(index: &str) -> String {
    index.replace(|c: char| !c.is_ascii_alphanumeric(), "_")
}

/// Extract + convert one subtitle track (embedded or an external text sidecar) to WebVTT
/// with jellyfin-ffmpeg, writing `out`. Text extraction is a stream convert (no video
/// decode) so it is CPU-trivial and does not touch the GPU transcode-session cap.
async fn convert_to_webvtt(
    file: &medi_db::models::MediaFile,
    sub: &medi_db::models::SubtitleStream,
    out: &std::path::Path,
) -> ApiResult<()> {
    if let Some(parent) = out.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ApiError::internal(format!("subs dir: {e}")))?;
    }

    // Input + stream selection: for an external sidecar the input is the sidecar file
    // (its single stream 0:s:0); for an embedded track the input is the media file and we
    // select its subtitle-relative index.
    let (input, map) = if sub.is_external {
        let path = sub
            .external_path
            .clone()
            .ok_or_else(|| ApiError::internal("external subtitle missing path"))?;
        (path, "0:s:0".to_string())
    } else {
        let rel = embedded_subtitle_relative_index(&file.subtitle_streams, sub)
            .ok_or_else(|| ApiError::internal("subtitle track not found for conversion"))?;
        (file.path.clone(), format!("0:s:{rel}"))
    };

    let status = tokio::process::Command::new(medi_transcode::caps::ffmpeg_bin())
        .args(["-hide_banner", "-loglevel", "warning", "-nostdin", "-y", "-i"])
        .arg(&input)
        .args(["-map", &map, "-c:s", "webvtt", "-f", "webvtt"])
        .arg(out)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map_err(|e| ApiError::internal(format!("ffmpeg spawn: {e}")))?;

    if !status.success() {
        // Don't leave a partial/empty cache file behind on failure.
        let _ = tokio::fs::remove_file(out).await;
        return Err(ApiError::internal("subtitle conversion failed"));
    }
    Ok(())
}

/// The subtitle-relative (`0:s:<n>`) index of an embedded track — its position among the
/// file's embedded subtitle tracks ordered by `stream_index`.
fn embedded_subtitle_relative_index(
    subs: &[medi_db::models::SubtitleStream],
    target: &medi_db::models::SubtitleStream,
) -> Option<i64> {
    let mut embedded: Vec<&medi_db::models::SubtitleStream> =
        subs.iter().filter(|s| !s.is_external && s.stream_index.is_some()).collect();
    embedded.sort_by_key(|s| s.stream_index);
    embedded
        .iter()
        .position(|s| s.id == target.id)
        .map(|p| p as i64)
}

/// Serve a WebVTT file with the correct content type, via `ServeFile` (handles `Range`).
async fn serve_vtt_file(path: &std::path::Path, req: Request) -> ApiResult<Response> {
    let serve = ServeFile::new(path);
    let mut resp = serve
        .oneshot(req)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .into_response();
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("text/vtt; charset=utf-8"),
    );
    Ok(resp)
}

// ---------------------------------------------------------------------------
// GET /api/trickplay/:file_id/meta  — Phase 3 asset, Phase 5 client consumer.
// ---------------------------------------------------------------------------

/// `GET /api/trickplay/:file` — serve a trickplay sprite file (`<id>.jpg` / `<id>.bif`).
///
/// Replaces the former `ServeDir` nest so a sibling `/meta` route can coexist without an
/// axum wildcard-nest conflict. `file` is a single path segment; we reject anything with a
/// separator or `..` and serve only the basename from the trickplay dir (no traversal).
async fn trickplay_file(
    State(state): State<AppState>,
    Path(file): Path<String>,
    req: Request,
) -> ApiResult<Response> {
    // A single URL segment never legitimately contains a slash or `..`; refuse if it does.
    if file.contains('/') || file.contains('\\') || file.contains("..") {
        return Err(ApiError::not_found("invalid trickplay path"));
    }
    let path = state.config.trickplay_dir().join(&file);
    let serve = ServeFile::new(&path);
    let resp = serve
        .oneshot(req)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(resp.into_response())
}

/// `GET /api/trickplay/:file_id/meta` — grid geometry for scrub thumbnails.
///
/// Reads the `trickplay_assets` row and returns the tiled-JPG mosaic's geometry
/// (`interval_ms`, `tile_w/h`, `cols`, `rows`) so the client can crop the cell that
/// covers a scrub position (`docs/.tasks/50` Part A). The mosaic image itself is served
/// by the static `/api/trickplay` route as `<file_id>.jpg`.
///
/// A `404` is returned when the file has no trickplay asset yet, **or** when the asset
/// is a BIF (no client-croppable grid). The player treats `404` as "no thumbnails" and
/// falls back to a plain scrub bar, so this is a graceful, expected outcome.
///
/// Not cached in the moka layer: it is a tiny single-row read that changes only when the
/// asset worker (re)generates the sprite, and it carries no ETag machinery.
async fn trickplay_meta(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
) -> ApiResult<Response> {
    let db = state.db.clone();
    let asset =
        run_blocking(&db, move |conn| queries::get_trickplay_asset(conn, file_id)).await?;

    // Only the tiled-JPG kind carries a grid the client can crop. A BIF row (or a row
    // somehow missing its grid dims) has nothing to serve here → 404, client falls back.
    if asset.kind != "tiled_jpg" {
        return Err(ApiError::not_found(
            "trickplay asset is not a tiled-JPG mosaic (no croppable grid)",
        ));
    }
    let (Some(tile_w), Some(tile_h), Some(cols), Some(rows)) =
        (asset.tile_w, asset.tile_h, asset.cols, asset.rows)
    else {
        return Err(ApiError::not_found("trickplay asset has no grid metadata"));
    };

    let body = TrickplayMeta {
        file_id,
        kind: asset.kind,
        interval_ms: asset.interval_ms,
        tile_w,
        tile_h,
        cols,
        rows,
    };
    Ok(Json(body).into_response())
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
pub(crate) async fn run_blocking<T, F>(db: &medi_db::Db, f: F) -> ApiResult<T>
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
