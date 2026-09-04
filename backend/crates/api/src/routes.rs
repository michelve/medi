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
    BackfillResponse, CategoryRow, ContinueWatchingItem, FileTracks, GenreListItem, LibraryItem,
    LibraryPage, LibraryRows, MatchCandidate, MatchRequest, MatchesResponse, PersonPage,
    ProgressResponse, ProgressWrite, RefreshResponse, StreamDecision, TrickplayMeta,
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
        // Per-file tracks (`docs/.tasks/97` Part C): audio + subtitle tracks for a deep link
        // to populate the player's menus. Shared with `99` (adds chapters).
        .route("/api/files/:file_id", get(file_tracks))
        // Playback progress + Continue Watching (`docs/.tasks/98`). Not ETag-cached (progress
        // is live). `PUT` is the throttled in-play write; `POST` accepts the same body so the
        // unload/tab-hide flush can go out via `navigator.sendBeacon` (which only sends POST).
        .route(
            "/api/progress/:file_id",
            get(get_progress).put(put_progress).post(put_progress),
        )
        .route("/api/continue-watching", get(continue_watching))
        // Playback — Phase 2 (transcode crate).
        .route("/api/stream/:file_id", get(stream_decision))
        .route("/api/direct/:file_id", get(direct_play))
        .route("/api/hls/:session_id/:file", get(hls_asset))
        // Subtitles — Phase 90. Serve a text subtitle track as WebVTT (embedded → extract
        // + convert, cached under /config/subs; external .vtt → served directly). Image
        // tracks are not served here — the client requests a burn-in via /api/stream.
        .route("/api/subtitles/:file_id/:index", get(subtitle_vtt))
        // Raw subtitle bytes (`docs/.tasks/99`): the ORIGINAL track, unconverted, for
        // client-side rendering (ASS/SSA → libass, PGS/VobSub → libbitsub). Embedded tracks
        // are extracted with `-c:s copy` and cached; external sidecars served straight.
        // Supports Range (libbitsub streams bitmap subs with range requests).
        .route("/api/subtitles/:file_id/:index/raw", get(subtitle_raw))
        // VobSub `.idx` companion for the `.sub` that `/raw` serves (libbitsub needs both).
        .route("/api/subtitles/:file_id/:index/raw.idx", get(subtitle_raw_idx))
        // Embedded font attachments (`docs/.tasks/99`) so libass renders ASS with the file's
        // real fonts. `/fonts` lists them (JSON); `/fonts/:name` serves one.
        .route("/api/files/:file_id/fonts", get(file_fonts))
        .route("/api/files/:file_id/fonts/:name", get(file_font))
        // Generated assets — Phase 3 (assets crate). Served as static files; a
        // missing file is a natural 404 from ServeDir.
        .nest_service("/api/preview", previews)
        // Trickplay (`docs/.tasks/50` Part A). Two *plain* routes with different segment
        // counts — they do not overlap, so no wildcard-nest conflict:
        //   /api/trickplay/:file        → the sprite file (e.g. `88.jpg`, `88.bif`)
        //   /api/trickplay/:file_id/meta → the tiled-JPG grid geometry (JSON)
        .route("/api/trickplay/:file", get(trickplay_file))
        .route("/api/trickplay/:file_id/meta", get(trickplay_meta))
        // Per-chapter poster frame (`docs/.tasks/99` Part C): the scene-selection grid + the
        // scrub-bar hover fallback. Served from `/config/chapter-images/<file_id>/<ordinal>.jpg`;
        // a 404 means "no image yet" (the client falls back like it does for trickplay).
        .route("/api/chapters/:file_id/image/:ordinal", get(chapter_image))
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
    /// Selected audio track (`docs/.tasks/97` Part C): the ffprobe `stream_index` of one of
    /// the file's `audio_streams`. Absent (or invalid) → the default track
    /// (`default_audio_track`). A non-default selection changes the source audio the transcode
    /// maps (`-map 0:a:<n>`) and, because it feeds the session key, spawns its own transcode
    /// session. A browser `<video>` can't switch an embedded track, so the client pairs this
    /// with `force_transcode=1` when the base decision was `direct`.
    #[serde(default)]
    audio_track: Option<i64>,
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

/// The default source audio track `decide` reasons over: the `is_default` track, else the
/// lowest `stream_index`. Maps the db read model into the transcode [`AudioTrack`]
/// descriptor. Returns the AAC-safe default when the file has no `audio_streams`
/// children (un-probed / pre-Task-70), so an unknown-audio row never forces a needless
/// remux (`docs/.tasks/70` §Backward compat).
fn default_audio_track(streams: &[medi_db::models::AudioStream]) -> AudioTrack {
    match default_audio_row(streams) {
        Some(track) => audio_track_of(track),
        None => AudioTrack::unknown_safe(),
    }
}

/// The default `audio_streams` row: the `is_default` track, else the lowest `stream_index`.
fn default_audio_row(streams: &[medi_db::models::AudioStream]) -> Option<&medi_db::models::AudioStream> {
    streams
        .iter()
        .find(|s| s.is_default)
        .or_else(|| streams.iter().min_by_key(|s| s.stream_index))
}

/// Map one `audio_streams` row into the transcode [`AudioTrack`] descriptor `decide` reads.
/// An unknown / absent codec becomes the AAC-safe default so a row we can't classify never
/// forces a needless remux (`docs/.tasks/70` §Backward compat).
fn audio_track_of(track: &medi_db::models::AudioStream) -> AudioTrack {
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

/// Resolve the source audio track to serve (`docs/.tasks/97` Part C), honoring a client's
/// `audio_track` selection.
///
/// Returns the track's [`AudioTrack`] descriptor plus its **audio-relative** index
/// (`0:a:<n>` — its position among the file's audio streams ordered by `stream_index`), which
/// `command.rs` maps into the output. When `selected` names a real track, that track is used;
/// when it is absent or doesn't match any stream, the default track is used and the relative
/// index is `None` (ffmpeg picks its own default first audio — the pre-Task-97 behavior).
fn select_audio_track(
    streams: &[medi_db::models::AudioStream],
    selected: Option<i64>,
) -> (AudioTrack, Option<u32>) {
    // Only an explicit, valid selection carries a relative-index map; the default path leaves
    // it `None` so an un-switched stream is byte-for-byte the same session it always was.
    if let Some(idx) = selected {
        if let Some(rel) = audio_relative_index(streams, idx) {
            if let Some(row) = streams.iter().find(|s| s.stream_index == idx) {
                return (audio_track_of(row), Some(rel));
            }
        }
    }
    (default_audio_track(streams), None)
}

/// The audio-relative (`0:a:<n>`) index of an absolute ffprobe `stream_index` — its position
/// among the file's audio streams, ordered by `stream_index`. `None` if no track matches.
fn audio_relative_index(streams: &[medi_db::models::AudioStream], stream_index: i64) -> Option<u32> {
    let mut ordered: Vec<&medi_db::models::AudioStream> = streams.iter().collect();
    ordered.sort_by_key(|s| s.stream_index);
    ordered
        .iter()
        .position(|s| s.stream_index == stream_index)
        .map(|p| p as u32)
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

/// `GET /api/files/:file_id` — a file's audio + subtitle tracks (`docs/.tasks/97` Part C).
///
/// Reuses `queries::get_media_file` (which already hydrates `audio_streams` +
/// `subtitle_streams`) and projects each into the player-facing menu shape. A deep link to
/// `/play/:file_id` (with no router state) fetches this to populate its audio-track and
/// caption menus. Not moka-cached: a tiny single-file read, like `/api/stream`.
async fn file_tracks(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
) -> ApiResult<Response> {
    let db = state.db.clone();
    let file = run_blocking(&db, move |conn| queries::get_media_file(conn, file_id)).await?;
    let body = FileTracks {
        file_id,
        video_fps: file.frame_rate,
        audio: file.audio_streams.into_iter().map(Into::into).collect(),
        subtitles: file.subtitle_streams.into_iter().map(Into::into).collect(),
        chapters: file.chapters.into_iter().map(Into::into).collect(),
    };
    Ok(Json(body).into_response())
}

// ---------------------------------------------------------------------------
// Playback progress + Continue Watching (`docs/.tasks/98`)
// ---------------------------------------------------------------------------

/// Current unix time in seconds — the epoch `playback_progress.updated_at` is stamped with,
/// matching `probe_failures.last_attempt_at` and the other epoch columns.
fn now_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `GET /api/progress/:file_id` — the saved playback position of a file (`docs/.tasks/98`).
///
/// Returns `{ position_ms, duration_ms, updated_at, finished }` when the file has progress,
/// or `204 No Content` (empty body) when it has never been played — so the player resumes
/// only when there is something to resume. Not moka-cached: progress is live.
async fn get_progress(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
) -> ApiResult<Response> {
    let db = state.db.clone();
    let progress = run_blocking(&db, move |conn| queries::get_progress(conn, file_id)).await?;
    match progress {
        Some(p) => Ok(Json(ProgressResponse::from(p)).into_response()),
        None => Ok(axum::http::StatusCode::NO_CONTENT.into_response()),
    }
}

/// `PUT` (and `POST`) `/api/progress/:file_id` — persist the playback position of a file
/// (`docs/.tasks/98`).
///
/// The throttled write the player sends as it plays, and once more on pause / tab-hide /
/// unmount. The in-play writes use `PUT`; the unload/hide flush goes out via
/// `navigator.sendBeacon`, which only sends `POST`, so this handler backs both verbs.
/// Upserts the single per-file row (single-user), deriving
/// `finished` past ~95%. Returns `204 No Content`. A negative position is clamped to 0 (a
/// beacon can occasionally report a bogus value); an unknown `file_id` still writes a row only
/// if the FK holds — the upsert's `REFERENCES media_files(id)` rejects a phantom file with a
/// constraint error, surfaced as a `500` (a real client never sends one).
async fn put_progress(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    Json(body): Json<ProgressWrite>,
) -> ApiResult<Response> {
    let position_ms = body.position_ms.max(0);
    let duration_ms = body.duration_ms.max(0);
    let now = now_secs();
    let db = state.db.clone();
    run_blocking(&db, move |conn| {
        medi_db::writes::upsert_progress(conn, file_id, position_ms, duration_ms, now)
    })
    .await?;
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

/// Query params for `GET /api/continue-watching`.
#[derive(Debug, Deserialize)]
pub struct ContinueQuery {
    /// Max rows to return; clamped by the DB layer. Absent → a sensible default.
    #[serde(default)]
    limit: Option<u32>,
}

/// Default number of Continue-Watching cards when the client sends no `limit`.
const CONTINUE_DEFAULT_LIMIT: u32 = 20;

/// `GET /api/continue-watching?limit=` — the "Continue Watching" row (`docs/.tasks/98`):
/// in-progress titles newest-first, each with its poster/title and the position to resume from.
/// Finished (past ~95%) and just-started (< 30 s) titles are excluded by the query. Not
/// moka-cached: it changes on every `PUT /api/progress`.
async fn continue_watching(
    State(state): State<AppState>,
    Query(q): Query<ContinueQuery>,
) -> ApiResult<Response> {
    let limit = q.limit.unwrap_or(CONTINUE_DEFAULT_LIMIT);
    let db = state.db.clone();
    let items = run_blocking(&db, move |conn| queries::list_continue_watching(conn, limit)).await?;
    let body: Vec<ContinueWatchingItem> = items.into_iter().map(Into::into).collect();
    Ok(Json(body).into_response())
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
    // The source audio track's real descriptor (`docs/.tasks/70`) plus its audio-relative
    // index for the `-map` (`docs/.tasks/97` Part C): the client's `audio_track` selection when
    // valid, else the default track (with no explicit map — the pre-Task-97 behavior). The
    // AAC-safe default covers a file with no probed `audio_streams` (never a needless remux).
    let (audio, audio_source) = select_audio_track(&file.audio_streams, q.audio_track);
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
            // Carry the selected source track's audio-relative index into the target so
            // `command.rs` maps `0:a:<n>` and the session key differs per track
            // (`docs/.tasks/97` Part C). The codec/downmix decision is unchanged — only the
            // *source* track changes.
            let audio_tgt = match audio_plan(audio, &client) {
                AudioPlan::Copy => AudioTarget::Copy { source: audio_source },
                AudioPlan::Transcode { codec, channels } => {
                    AudioTarget::Transcode { codec, channels, source: audio_source }
                }
            };
            // Duration drives the synthesized VOD playlist (full runtime + seekable timeline).
            let duration_ms = file.duration_ms.unwrap_or(0).max(0) as u64;
            let session_id = state
                .transcode
                .start(&src, target, audio_tgt, duration_ms)
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

/// `GET /api/hls/:session_id/:file` — one artifact of a seekable VOD transcode session.
///
/// - **`index.m3u8`** → the server-synthesized VOD playlist (full runtime, every segment,
///   `#EXT-X-ENDLIST`), so the client shows the real duration and can seek anywhere.
/// - **`segNNNNN.m4s`** → produced ON DEMAND: `ensure_segment` (re)starts the transcode at
///   that segment on a seek, then waits for the file. This is what makes a jump to 45 min
///   work without transcoding the whole film first.
/// - **`init.mp4`** (and anything else) → served from disk once the transcode has written it.
async fn hls_asset(
    State(state): State<AppState>,
    Path((session_id, file)): Path<(String, String)>,
    req: Request,
) -> ApiResult<Response> {
    // The playlist is synthesized, not read from disk.
    if file == PLAYLIST_NAME {
        let body = state
            .transcode
            .vod_playlist(&session_id)
            .await
            .map_err(map_session_err)?;
        return Ok((
            [(axum::http::header::CONTENT_TYPE, "application/vnd.apple.mpegurl")],
            body,
        )
            .into_response());
    }

    // A segment: ensure it's being produced (seek-restart if needed) and wait for it.
    let path = if let Some(seg) = medi_transcode::segment_index(&file) {
        state
            .transcode
            .ensure_segment(&session_id, seg)
            .await
            .map_err(map_session_err)?
    } else {
        // init.mp4 or another artifact: resolve within the session dir and briefly wait for
        // it (the init header is written alongside the first segment).
        let path = state
            .transcode
            .resolve_file(&session_id, &file)
            .await
            .map_err(map_session_err)?;
        if !path.exists() {
            wait_for_file(&path, std::time::Duration::from_secs(15)).await;
        }
        path
    };

    // The file may still be absent if the transcode couldn't produce it in time — ServeFile
    // returns a natural 404 the client retries.
    let serve = ServeFile::new(&path);
    let resp = serve
        .oneshot(req)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(resp.into_response())
}

/// Wait for `path` to exist, up to `timeout`, polling briefly. Used to let a just-started
/// transcode write its `init.mp4` before we serve it. Returns as soon as the file appears; on
/// timeout the caller serves whatever's there (a natural 404).
async fn wait_for_file(path: &std::path::Path, timeout: std::time::Duration) {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if path.exists() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
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

    // Resolve the selected subtitle row (`ext<id>` sidecar id / bare embedded `stream_index`).
    let sub = resolve_subtitle_row(&file, &index)?;

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

/// Resolve a subtitle row of `file` from the `:index` path fragment: `ext<id>` addresses an
/// external sidecar by row id; a bare number is an embedded track's ffprobe `stream_index`.
/// Shared by the WebVTT and raw subtitle handlers.
fn resolve_subtitle_row(
    file: &medi_db::models::MediaFile,
    index: &str,
) -> ApiResult<medi_db::models::SubtitleStream> {
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
    sub.ok_or_else(|| ApiError::not_found("no such subtitle track"))
}

/// Map a subtitle codec to the ffmpeg output format + file extension + content type used when
/// extracting the RAW track for client-side rendering (`docs/.tasks/99`). Returns `None` for a
/// codec we don't extract raw (the client falls back to WebVTT / burn-in).
fn raw_subtitle_kind(codec: &str) -> Option<(&'static str, &'static str, &'static str)> {
    // (ffmpeg -f format, extension, Content-Type)
    match codec.to_ascii_lowercase().as_str() {
        "ass" | "ssa" => Some(("ass", "ass", "text/x-ssa; charset=utf-8")),
        "subrip" | "srt" => Some(("srt", "srt", "application/x-subrip; charset=utf-8")),
        "webvtt" => Some(("webvtt", "vtt", "text/vtt; charset=utf-8")),
        "hdmv_pgs_subtitle" | "pgssub" | "pgs" => Some(("sup", "sup", "application/octet-stream")),
        // VobSub is a `.idx`(text index) + `.sub`(bitmap) pair. `/raw` serves the `.sub`; the
        // sibling `.idx` is served by `subtitle_raw_idx`. ffmpeg writes both when the output
        // extension is `.idx`, so extraction targets that.
        "dvd_subtitle" | "dvdsub" | "vobsub" => Some(("vobsub", "sub", "application/octet-stream")),
        _ => None,
    }
}

/// Whether a codec is VobSub-family (a `.idx`+`.sub` pair).
fn is_vobsub_codec(codec: &str) -> bool {
    matches!(codec.to_ascii_lowercase().as_str(), "dvd_subtitle" | "dvdsub" | "vobsub")
}

/// `GET /api/subtitles/:file_id/:index/raw` — the subtitle track in its ORIGINAL format, for
/// client-side rendering (`docs/.tasks/99`): ASS/SSA → libass, PGS → libbitsub. An external
/// sidecar is served straight from disk; an embedded track is extracted with `-c:s copy` and
/// cached under `subs_raw_dir()`. `ServeFile` handles `Range` (libbitsub streams with ranges).
async fn subtitle_raw(
    State(state): State<AppState>,
    Path((file_id, index)): Path<(i64, String)>,
    req: Request,
) -> ApiResult<Response> {
    let db = state.db.clone();
    let file = run_blocking(&db, move |conn| queries::get_media_file(conn, file_id)).await?;
    let sub = resolve_subtitle_row(&file, &index)?;

    // External sidecar: serve the original file verbatim (VobSub `.sub`, ASS, etc. — read-only).
    if sub.is_external {
        if let Some(path) = &sub.external_path {
            return serve_static_file(std::path::Path::new(path), req, None).await;
        }
        return Err(ApiError::not_found("external subtitle missing path"));
    }

    let codec = sub.codec.as_deref().unwrap_or("");
    raw_subtitle_kind(codec)
        .ok_or_else(|| ApiError::unsupported_media_type("subtitle codec not available raw"))?;

    // Serve the `.sub` (VobSub) or the single extract file, extracting the pair/file on demand.
    let (path, content_type) = ensure_raw_extract(&state, &file, &sub, &index).await?;
    serve_static_file(&path, req, content_type).await
}

/// `GET /api/subtitles/:file_id/:index/raw.idx` — the VobSub `.idx` companion for the `.sub`
/// that `/raw` serves (`docs/.tasks/99`). libbitsub needs both. Only valid for VobSub codecs.
async fn subtitle_raw_idx(
    State(state): State<AppState>,
    Path((file_id, index)): Path<(i64, String)>,
    req: Request,
) -> ApiResult<Response> {
    let db = state.db.clone();
    let file = run_blocking(&db, move |conn| queries::get_media_file(conn, file_id)).await?;
    let sub = resolve_subtitle_row(&file, &index)?;
    if sub.is_external {
        return Err(ApiError::bad_request("external VobSub is served as a sidecar, not /raw.idx"));
    }
    if !is_vobsub_codec(sub.codec.as_deref().unwrap_or("")) {
        return Err(ApiError::unsupported_media_type("no .idx for a non-VobSub subtitle"));
    }
    // Extraction (triggered by `/raw` or here) writes the pair; return the `.idx` sibling.
    let (sub_path, _) = ensure_raw_extract(&state, &file, &sub, &index).await?;
    let idx_path = sub_path.with_extension("idx");
    serve_static_file(&idx_path, req, Some("text/plain; charset=utf-8")).await
}

/// Ensure the raw extract for an embedded track exists in the cache, returning the primary file
/// path (the `.sub` for VobSub) + its content type. Extraction is a `-c:s copy` stream copy —
/// CPU-trivial, no GPU transcode session. For VobSub, ffmpeg writes the `.idx`+`.sub` pair.
async fn ensure_raw_extract(
    state: &AppState,
    file: &medi_db::models::MediaFile,
    sub: &medi_db::models::SubtitleStream,
    index: &str,
) -> ApiResult<(std::path::PathBuf, Option<&'static str>)> {
    let codec = sub.codec.as_deref().unwrap_or("");
    let (fmt, ext, content_type) = raw_subtitle_kind(codec)
        .ok_or_else(|| ApiError::unsupported_media_type("subtitle codec not available raw"))?;
    let base = state
        .config
        .subs_raw_dir()
        .join(format!("{}.{}", file.id, cache_key_for(index)));
    let primary = base.with_extension(ext);
    if !primary.exists() {
        let rel = embedded_subtitle_relative_index(&file.subtitle_streams, sub)
            .ok_or_else(|| ApiError::internal("subtitle track not found for extraction"))?;
        if let Some(parent) = primary.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ApiError::internal(format!("subs-raw dir: {e}")))?;
        }
        let map = format!("0:s:{rel}");
        let mut cmd = tokio::process::Command::new(medi_transcode::caps::ffmpeg_bin());
        cmd.args(["-hide_banner", "-loglevel", "warning", "-nostdin", "-y", "-i"])
            .arg(&file.path)
            .args(["-map", &map, "-c:s", "copy"]);
        // VobSub: no `-f` — ffmpeg picks the muxer from the `.idx` output and writes both files.
        // Everything else: force the format and write the single file.
        let out = if is_vobsub_codec(codec) {
            base.with_extension("idx")
        } else {
            cmd.args(["-f", fmt]);
            primary.clone()
        };
        let status = cmd
            .arg(&out)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .map_err(|e| ApiError::internal(format!("ffmpeg spawn: {e}")))?;
        if !status.success() {
            let _ = tokio::fs::remove_file(&out).await;
            if is_vobsub_codec(codec) {
                let _ = tokio::fs::remove_file(base.with_extension("sub")).await;
            }
            return Err(ApiError::internal("subtitle extraction failed"));
        }
    }
    Ok((primary, Some(content_type)))
}

/// Serve a file via `ServeFile` (handles `Range`), optionally forcing a content type.
async fn serve_static_file(
    path: &std::path::Path,
    req: Request,
    content_type: Option<&'static str>,
) -> ApiResult<Response> {
    let serve = ServeFile::new(path);
    let mut resp = serve
        .oneshot(req)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .into_response();
    if let Some(ct) = content_type {
        resp.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static(ct),
        );
    }
    Ok(resp)
}

/// The per-file directory holding this media file's dumped font attachments.
fn fonts_cache_dir(state: &AppState, file_id: i64) -> std::path::PathBuf {
    state.config.subs_raw_dir().join(format!("fonts-{file_id}"))
}

/// Font file extensions libass consumes; used to filter dumped attachments (a matroska file can
/// also carry cover-art image attachments, which we skip).
fn is_font_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".ttf", ".otf", ".ttc", ".woff", ".woff2", ".pfb", ".pfa"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// Dump all attachments of `file` into `dir` (idempotent — skipped if `dir` already exists).
/// Uses jellyfin-ffmpeg `-dump_attachment:t ""`, run with `dir` as the working directory so each
/// attachment lands under its `filename` tag there.
async fn ensure_fonts_dumped(file_path: &std::path::Path, dir: &std::path::Path) -> ApiResult<()> {
    if dir.exists() {
        return Ok(());
    }
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| ApiError::internal(format!("fonts dir: {e}")))?;
    // `-dump_attachment:t ""` writes every attachment to its tagged filename in the cwd; the
    // trailing `-f null -` gives ffmpeg a (discarded) output so it runs. A file with no
    // attachments exits cleanly and leaves the dir empty.
    let _ = tokio::process::Command::new(medi_transcode::caps::ffmpeg_bin())
        .current_dir(dir)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-dump_attachment:t", ""])
        .arg("-i")
        .arg(file_path)
        .args(["-f", "null", "-"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map_err(|e| ApiError::internal(format!("ffmpeg spawn: {e}")))?;
    Ok(())
}

/// `GET /api/files/:file_id/fonts` — list the file's embedded font attachments (`docs/.tasks/99`)
/// so the web player can hand their URLs to libass. Returns `{ fonts: ["Arial.ttf", ...] }`.
async fn file_fonts(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
) -> ApiResult<Response> {
    let db = state.db.clone();
    let file = run_blocking(&db, move |conn| queries::get_media_file(conn, file_id)).await?;
    let dir = fonts_cache_dir(&state, file_id);
    ensure_fonts_dumped(std::path::Path::new(&file.path), &dir).await?;

    let mut fonts = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            if let Some(name) = entry.file_name().to_str() {
                if is_font_file(name) {
                    fonts.push(name.to_string());
                }
            }
        }
    }
    fonts.sort();
    Ok(Json(serde_json::json!({ "fonts": fonts })).into_response())
}

/// `GET /api/files/:file_id/fonts/:name` — serve one dumped font attachment by filename
/// (`docs/.tasks/99`). Rejects path traversal; only serves recognized font files.
async fn file_font(
    State(state): State<AppState>,
    Path((file_id, name)): Path<(i64, String)>,
    req: Request,
) -> ApiResult<Response> {
    // Reject any name that isn't a plain filename (no separators / traversal).
    if name.contains('/') || name.contains('\\') || name.contains("..") || !is_font_file(&name) {
        return Err(ApiError::bad_request("invalid font name"));
    }
    let dir = fonts_cache_dir(&state, file_id);
    let path = dir.join(&name);
    if !path.exists() {
        return Err(ApiError::not_found("no such font"));
    }
    serve_static_file(&path, req, Some("application/octet-stream")).await
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

/// `GET /api/chapters/:file_id/image/:ordinal` — a chapter's poster frame (`docs/.tasks/99`
/// Part C).
///
/// Serves `<chapter_images_dir>/<file_id>/<ordinal>.jpg`, generated by the off-peak asset worker.
/// Both path segments are typed `i64`, so axum rejects any non-integer input before we build the
/// path — no traversal is possible and no manual sanitization is needed. A missing file is a
/// natural `404` from `ServeFile`; the client treats that as "no image" and falls back (trickplay
/// tile, then name/time only), exactly like `trickplay_meta`'s 404.
async fn chapter_image(
    State(state): State<AppState>,
    Path((file_id, ordinal)): Path<(i64, i64)>,
    req: Request,
) -> ApiResult<Response> {
    let path = state
        .config
        .chapter_images_dir()
        .join(file_id.to_string())
        .join(format!("{ordinal}.jpg"));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_subtitle_kind_maps_known_codecs() {
        // Text codecs extract to their native container; PGS to .sup; unknowns fall through.
        assert_eq!(raw_subtitle_kind("ass").map(|k| k.1), Some("ass"));
        assert_eq!(raw_subtitle_kind("SSA").map(|k| k.1), Some("ass"));
        assert_eq!(raw_subtitle_kind("subrip").map(|k| k.1), Some("srt"));
        assert_eq!(raw_subtitle_kind("webvtt").map(|k| k.1), Some("vtt"));
        assert_eq!(raw_subtitle_kind("hdmv_pgs_subtitle").map(|k| k.1), Some("sup"));
        // VobSub extracts as a `.idx`+`.sub` pair; `/raw` serves the `.sub`.
        assert_eq!(raw_subtitle_kind("dvd_subtitle").map(|k| k.1), Some("sub"));
        assert!(is_vobsub_codec("dvd_subtitle"));
        assert!(!is_vobsub_codec("hdmv_pgs_subtitle"));
        // A codec we don't extract raw falls through (client uses WebVTT / burn-in).
        assert!(raw_subtitle_kind("mov_text").is_none());
    }

    #[test]
    fn is_font_file_accepts_fonts_rejects_others() {
        assert!(is_font_file("Arial.ttf"));
        assert!(is_font_file("Deja Vu.OTF"));
        assert!(is_font_file("x.woff2"));
        assert!(!is_font_file("cover.jpg"));
        assert!(!is_font_file("notes.txt"));
    }
}
