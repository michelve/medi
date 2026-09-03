//! Enrichment orchestration: turn one catalog title into a matched, artworked,
//! cast-populated row (`docs/.tasks/60` §Sub-tasks 4 + §Asset storage).
//!
//! [`enrich_movie`] / [`enrich_series`] each run the same pipeline:
//!   1. read the title's parsed `(title, year)` from the DB,
//!   2. `search` the provider and pick the best candidate above the match threshold
//!      (below ⇒ mark `unmatched`, no writes),
//!   3. `details` for the chosen candidate,
//!   4. download poster/backdrop **atomically** (temp file + rename) into
//!      `/config/images/<kind>/<id>/{poster,backdrop}.jpg`,
//!   5. write overview + artwork paths + credits + external ids and flip the row to
//!      `matched`, then invalidate the response cache.
//!
//! Idempotent: a `matched` row is skipped unless `force`. A `matched` row whose image
//! files are missing on disk re-downloads only the absent asset. Downloads stream, so a
//! crash leaves at most a stale `.tmp`, never a half-written `.jpg`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use medi_db::writes::{
    self, CollectionWrite, CreditWrite, GenreWrite, MetadataState, TitleKind, TitleMetadata,
    TrailerWrite,
};
use medi_db::Db;

use crate::fanart::FanartArt;
use crate::matcher::MATCH_THRESHOLD;
use crate::provider::{Details, MediaKind, MetadataProvider, ProviderId};
use crate::{Error, Result};

/// What one enrichment attempt did — useful for logs, tests, and the manual-refresh API
/// (which reports whether a match was found).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnrichOutcome {
    /// A provider match was found and written; carries the chosen provider token.
    Matched { provider_id: String },
    /// No candidate cleared the threshold; the row is marked `unmatched`.
    Unmatched,
    /// The row was already `matched` with its artwork present and `force` was not set.
    Skipped,
}

/// A source of image bytes for a URL. The default is [`HttpFetcher`] (reqwest); tests
/// inject a stub so the atomic-write and idempotency logic run with no network.
#[async_trait::async_trait]
pub trait ImageFetcher: Send + Sync {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>>;
}

/// The production image fetcher: a shared reqwest client streaming the body to bytes.
pub struct HttpFetcher {
    client: reqwest::Client,
}

impl HttpFetcher {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent("medi/0.1 (+https://github.com/michelve/medi)")
            .build()
            .map_err(|e| Error::Http(e.to_string()))?;
        Ok(Self { client })
    }
}

#[async_trait::async_trait]
impl ImageFetcher for HttpFetcher {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| Error::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(Error::Provider(format!("image {url} → HTTP {}", resp.status())));
        }
        let bytes = resp.bytes().await.map_err(|e| Error::Http(e.to_string()))?;
        Ok(bytes.to_vec())
    }
}

/// Everything the enrichment functions need beyond the per-title inputs, bundled so the
/// worker constructs it once and the API handlers reuse it.
#[derive(Clone)]
pub struct EnrichContext {
    pub db: Db,
    pub provider: Arc<dyn MetadataProvider>,
    pub fetcher: Arc<dyn ImageFetcher>,
    /// `AppConfig::images_dir()` — root under which `<kind>/<id>/{poster,backdrop}.jpg`
    /// live. The DB stores paths relative to this.
    pub images_dir: PathBuf,
    /// fanart.tv art client (logos `docs/.tasks/93` + wallpapers `docs/.tasks/95`), or `None`
    /// when `FANARTTV_API_KEY` is unset — the fanart features are then inert (no request, no
    /// write, enrichment as today).
    pub fanart: Option<Arc<dyn FanartArt>>,
}

/// Enrich one movie by id. See the module docs for the pipeline.
pub async fn enrich_movie(ctx: &EnrichContext, movie_id: i64, force: bool) -> Result<EnrichOutcome> {
    enrich(ctx, TitleKind::Movie, MediaKind::Movie, movie_id, force).await
}

/// Enrich one series by id (same pipeline, TV search/details endpoints).
pub async fn enrich_series(ctx: &EnrichContext, series_id: i64, force: bool) -> Result<EnrichOutcome> {
    enrich(ctx, TitleKind::Series, MediaKind::Series, series_id, force).await
}

async fn enrich(
    ctx: &EnrichContext,
    title_kind: TitleKind,
    media_kind: MediaKind,
    id: i64,
    force: bool,
) -> Result<EnrichOutcome> {
    // --- Idempotency gate -------------------------------------------------------
    // A matched row with both artwork files present is skipped unless force. If it is
    // matched but a file is missing, we fall through and re-run (re-downloading only the
    // absent asset below).
    if !force {
        let state = db_read(&ctx.db, move |conn| {
            writes::get_metadata_state(conn, title_kind, id)
        })
        .await?;
        if state.as_deref() == Some("matched") && artwork_complete(&ctx.images_dir, title_kind, id) {
            return Ok(EnrichOutcome::Skipped);
        }
        if state.as_deref() == Some("unmatched") {
            // Don't auto-retry a title we already failed to match; a manual refresh
            // passes force=true to override this.
            return Ok(EnrichOutcome::Skipped);
        }
    }

    // --- Read parsed identity ---------------------------------------------------
    let (title, year) =
        db_read(&ctx.db, move |conn| medi_db::queries::get_title_year(conn, title_kind, id)).await?;

    // --- Search + best match ----------------------------------------------------
    let candidates = match ctx.provider.search(&title, year, media_kind).await {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(id, %title, error = %err, "metadata search failed");
            mark_state(ctx, title_kind, id, MetadataState::Failed).await?;
            return Err(err);
        }
    };

    let best = candidates
        .into_iter()
        .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal));

    let chosen = match best {
        Some(m) if m.score >= MATCH_THRESHOLD => m,
        _ => {
            tracing::info!(id, %title, "no candidate cleared match threshold → unmatched");
            mark_state(ctx, title_kind, id, MetadataState::Unmatched).await?;
            return Ok(EnrichOutcome::Unmatched);
        }
    };

    // Delegate to the pinned-match path so search-match and manual-pin share one body.
    enrich_with_id(ctx, title_kind, id, &chosen.provider_id).await
}

/// List provider candidates for a title, best-first — backs `GET /api/movies/:id/matches`.
///
/// Uses the title's parsed `(title, year)` by default, or `query_override` when the user
/// supplies a corrected search term (`?query=`). Read-only: no DB writes, no downloads.
pub async fn candidates_for(
    ctx: &EnrichContext,
    title_kind: TitleKind,
    id: i64,
    query_override: Option<&str>,
) -> Result<Vec<crate::provider::Match>> {
    let media_kind = match title_kind {
        TitleKind::Movie => MediaKind::Movie,
        TitleKind::Series => MediaKind::Series,
    };
    let (parsed_title, parsed_year) =
        db_read(&ctx.db, move |conn| medi_db::queries::get_title_year(conn, title_kind, id)).await?;
    let (query, year) = match query_override {
        // An explicit query overrides the parsed title; drop the year filter so a
        // corrected spelling isn't also constrained to the (possibly wrong) parsed year.
        Some(q) if !q.trim().is_empty() => (q.to_string(), None),
        _ => (parsed_title, parsed_year),
    };
    ctx.provider.search(&query, year, media_kind).await
}

/// Enrich a title against a specific, already-chosen provider id — the shared body of an
/// auto-match and a manual `POST /api/movies/:id/match`. Fetches details, downloads
/// artwork atomically, writes the row, and returns the outcome.
pub async fn enrich_with_id(
    ctx: &EnrichContext,
    title_kind: TitleKind,
    id: i64,
    provider_id: &ProviderId,
) -> Result<EnrichOutcome> {
    let details = match ctx.provider.details(provider_id).await {
        Ok(d) => d,
        Err(err) => {
            tracing::warn!(id, error = %err, "metadata details failed");
            mark_state(ctx, title_kind, id, MetadataState::Failed).await?;
            return Err(err);
        }
    };

    // --- Download artwork atomically -------------------------------------------
    let (poster_rel, backdrop_rel) = download_artwork(ctx, title_kind, id, &details).await?;

    // --- Persist ---------------------------------------------------------------
    let meta = TitleMetadata {
        overview: details.overview.clone(),
        poster_path: poster_rel,
        backdrop_path: backdrop_rel,
        tmdb_id: details.tmdb_id,
        imdb_id: details.imdb_id.clone(),
    };
    let credits: Vec<CreditWrite> = details
        .cast
        .iter()
        .map(|c| CreditWrite {
            name: c.name.clone(),
            role: c.role.clone(),
            character: c.character.clone(),
            ord: c.ord,
        })
        .collect();
    // Genres come free from the same details response (`docs/.tasks/91` Phase A) — no extra
    // TMDB request. Written in the same transaction as metadata + credits so a match (or a
    // re-match) commits atomically and a re-match replaces the genre set wholesale.
    let genres: Vec<GenreWrite> = details
        .genres
        .iter()
        .map(|g| GenreWrite {
            tmdb_id: g.tmdb_id,
            name: g.name.clone(),
        })
        .collect();

    // Collection (franchise) + trailers are movie-only (Task 91 detail extensions), also from
    // the same details response. Download the collection poster atomically first (like title
    // art), then persist the linkage/trailers in the same transaction.
    let collection = if title_kind == TitleKind::Movie {
        build_collection(ctx, &details).await
    } else {
        None
    };

    // fanart.tv art (title logo `docs/.tasks/93` + background wallpaper `docs/.tasks/95`) —
    // movies only, best-effort, non-fatal. Fetched in ONE request when fanart is configured
    // and the movie resolved a TMDB id (fanart is keyed by it). Downloads are atomic +
    // skip-if-present via `maybe_download` (logo is `.png`, wallpaper `.jpg`); the relative
    // paths are persisted in the same transaction as the rest of the match below.
    let (logo_rel, wallpaper_rel) = if title_kind == TitleKind::Movie {
        download_fanart_art(ctx, id, &details).await
    } else {
        (None, None)
    };
    let trailers: Vec<TrailerWrite> = if title_kind == TitleKind::Movie {
        details
            .trailers
            .iter()
            .enumerate()
            .map(|(i, t)| TrailerWrite {
                youtube_key: t.youtube_key.clone(),
                name: t.name.clone(),
                kind: t.kind.clone(),
                ord: i as i64,
            })
            .collect()
    } else {
        Vec::new()
    };

    let db = ctx.db.clone();
    tokio::task::spawn_blocking(move || -> medi_db::DbResult<()> {
        let mut conn = db.conn()?;
        let tx = conn.transaction()?;
        writes::set_title_metadata(&tx, title_kind, id, &meta)?;
        writes::replace_credits(&tx, title_kind, id, &credits)?;
        writes::replace_title_genres(&tx, title_kind, id, &genres)?;
        if title_kind == TitleKind::Movie {
            // Upsert the collection (if any) and (re)point the movie at it — a re-match with
            // no collection clears a stale link.
            let collection_id = match &collection {
                Some(c) => Some(writes::upsert_collection(&tx, c)?),
                None => None,
            };
            writes::set_movie_collection(&tx, id, collection_id)?;
            writes::replace_movie_trailers(&tx, id, &trailers)?;
            // Set (or clear) the fanart.tv logo + wallpaper paths — a re-match with no art
            // clears a stale link, like the collection FK (`docs/.tasks/93`, `docs/.tasks/95`).
            writes::set_movie_logo(&tx, id, logo_rel.as_deref())?;
            writes::set_movie_wallpaper(&tx, id, wallpaper_rel.as_deref())?;
        }
        tx.commit()?;
        Ok(())
    })
    .await??;

    // Person enrichment (`docs/.tasks/91` Phase B) runs after the credits are committed, so
    // the `people` rows exist to attach photos/bios to. It is off the title's critical write
    // path: a failed person fetch logs and continues (a missing headshot never fails the
    // whole enrichment, same policy as a missing poster).
    enrich_people(ctx, &details).await;

    Ok(EnrichOutcome::Matched {
        provider_id: provider_id.to_token(),
    })
}

/// Build the [`CollectionWrite`] for a movie's franchise (Task 91 detail extensions),
/// downloading the collection poster atomically to `collections/<id>/poster.jpg` (skipping an
/// existing file, same as title art). Returns `None` when the movie has no collection. A
/// failed poster download logs and yields a `None` poster path — never fatal.
async fn build_collection(ctx: &EnrichContext, details: &Details) -> Option<CollectionWrite> {
    let c = details.collection.as_ref()?;
    let rel_dir = format!("collections/{}", c.tmdb_id);
    let abs_dir = ctx.images_dir.join("collections").join(c.tmdb_id.to_string());
    let poster_path = match maybe_download(
        ctx,
        c.poster_url.as_deref(),
        &abs_dir,
        &rel_dir,
        "poster.jpg",
    )
    .await
    {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!(collection = c.tmdb_id, error = %err, "collection poster download failed; continuing");
            None
        }
    };
    Some(CollectionWrite {
        tmdb_id: c.tmdb_id,
        name: c.name.clone(),
        poster_path,
    })
}

/// Fetch + download a movie's fanart.tv art — title logo (`docs/.tasks/93`) and background
/// wallpaper (`docs/.tasks/95`) — in **one** fanart request, returning
/// `(logo_rel, wallpaper_rel)` relative to `images_dir()` (each `None` when absent).
/// Best-effort and non-fatal — every skip/failure yields `(None, None)` (or a partial), and
/// the caller writes the corresponding `NULL`, never failing the enrichment:
///
/// - fanart unconfigured (`ctx.fanart == None`) → `(None, None)` (feature off).
/// - the movie has no resolved TMDB id (an OMDb match) → `(None, None)` (fanart is keyed by it).
/// - **both** files already on disk → keep them, no fanart request (the file check here
///   short-circuits the HTTP lookup for an already-cached movie).
/// - a fanart HTTP error → `warn` + `(None, None)` (a re-match/backfill retries later).
///
/// The logo is **PNG** (transparency); the wallpaper is **JPEG**. `maybe_download` skips any
/// file already present, so a movie with only one of the two on disk re-fetches fanart but
/// re-downloads only the missing asset.
async fn download_fanart_art(
    ctx: &EnrichContext,
    movie_id: i64,
    details: &Details,
) -> (Option<String>, Option<String>) {
    let Some(fanart) = ctx.fanart.as_ref() else {
        return (None, None); // feature off
    };
    let Some(tmdb_id) = details.tmdb_id else {
        return (None, None); // no TMDB linkage (OMDb match)
    };

    let rel_dir = format!("movies/{movie_id}");
    let abs_dir = ctx.images_dir.join("movies").join(movie_id.to_string());
    let logo_on_disk = abs_dir.join("logo.png").exists();
    let wallpaper_on_disk = abs_dir.join("wallpaper.jpg").exists();

    // Idempotency: when BOTH assets are already cached, skip the fanart request entirely.
    if logo_on_disk && wallpaper_on_disk {
        return (
            Some(format!("{rel_dir}/logo.png")),
            Some(format!("{rel_dir}/wallpaper.jpg")),
        );
    }

    let art = match fanart.movie_art(tmdb_id).await {
        Ok(a) => a.unwrap_or_default(), // 404 (no art) → both None
        Err(err) => {
            tracing::warn!(movie_id, tmdb_id, error = %err, "fanart art lookup failed; continuing without logo/wallpaper");
            return (None, None);
        }
    };

    // `maybe_download` returns the existing path for a file already on disk, and downloads
    // (atomic temp + rename) when a URL is present and the file is absent; a download error
    // logs and yields None. So an on-disk logo is preserved even if `art.logo_url` is None.
    let logo_rel = match maybe_download(ctx, art.logo_url.as_deref(), &abs_dir, &rel_dir, "logo.png").await {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!(movie_id, error = %err, "logo download failed; continuing");
            None
        }
    };
    let wallpaper_rel =
        match maybe_download(ctx, art.wallpaper_url.as_deref(), &abs_dir, &rel_dir, "wallpaper.jpg").await {
            Ok(p) => p,
            Err(err) => {
                tracing::warn!(movie_id, error = %err, "wallpaper download failed; continuing");
                None
            }
        };
    (logo_rel, wallpaper_rel)
}

/// Enrich the people credited on a just-written title (`docs/.tasks/91` Phase B): for each
/// cast/crew member carrying a `person_tmdb_id`, fetch their bio + headshot, download the
/// photo atomically to `people/<people.id>/photo.jpg`, and write the linkage/art/bio.
///
/// Idempotent on DB state: a person already carrying **both** a `tmdb_id` and a
/// `photo_path` is skipped with no provider round-trip (a person is shared across titles, so
/// once enriched they stay so). Distinct `person_tmdb_id`s are de-duped within one title so
/// a person credited twice (actor + director) is fetched once. Every failure is logged and
/// skipped — person data is best-effort.
async fn enrich_people(ctx: &EnrichContext, details: &Details) {
    use std::collections::HashSet;

    let mut seen: HashSet<i64> = HashSet::new();
    for credit in &details.cast {
        let Some(person_tmdb_id) = credit.person_tmdb_id else {
            continue; // a name-only credit (OMDb) — nothing to fetch
        };
        if !seen.insert(person_tmdb_id) {
            continue; // already handled this person on this title
        }
        let name = credit.name.clone();

        // Resolve the person row the credit write created, and its current enrichment state.
        let state = {
            let db = ctx.db.clone();
            match db_read(&db, move |conn| {
                medi_db::queries::get_person_enrichment_state(conn, &name)
            })
            .await
            {
                Ok(s) => s,
                Err(err) => {
                    tracing::warn!(error = %err, "person lookup failed; skipping");
                    continue;
                }
            }
        };
        let Some((person_id, existing_tmdb, existing_photo)) = state else {
            continue; // the credit's person row is gone (raced with a reap) — skip
        };
        // Already fully enriched → no re-fetch (idempotent).
        if existing_tmdb.is_some() && existing_photo.is_some() {
            continue;
        }

        // Fetch the person's bio + headshot from the provider.
        let person = match ctx.provider.person_details(person_tmdb_id).await {
            Ok(Some(p)) => p,
            Ok(None) => continue, // provider has no people (OMDb default) — nothing to write
            Err(err) => {
                tracing::warn!(person_tmdb_id, error = %err, "person details fetch failed; continuing");
                continue;
            }
        };

        // Download the headshot atomically to people/<person_id>/photo.jpg, reusing the same
        // skip-if-present + atomic-rename path as poster/backdrop art.
        let rel_dir = format!("people/{person_id}");
        let abs_dir = ctx.images_dir.join("people").join(person_id.to_string());
        let photo_path = match maybe_download(
            ctx,
            person.photo_url.as_deref(),
            &abs_dir,
            &rel_dir,
            "photo.jpg",
        )
        .await
        {
            Ok(p) => p,
            Err(err) => {
                tracing::warn!(person_id, error = %err, "headshot download failed; continuing");
                None
            }
        };

        // Persist the linkage + art + bio (COALESCE keeps any field the fetch lacked).
        let db = ctx.db.clone();
        let bio = person.biography.clone();
        let write = db_read(&db, move |conn| {
            medi_db::writes::upsert_person_meta(
                conn,
                person_id,
                Some(person_tmdb_id),
                photo_path.as_deref(),
                bio.as_deref(),
            )
        })
        .await;
        if let Err(err) = write {
            tracing::warn!(person_id, error = %err, "person meta write failed; continuing");
        }
    }
}

/// What a backfill pass did (`docs/.tasks/91` §Backfill) — surfaced by the API's backfill
/// trigger and useful for logs/tests. `processed` counts titles whose details were
/// re-fetched and genre/person rows written; `matched` is how many of those actually wrote
/// genres; `failed` counts titles whose details fetch errored (logged, not fatal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BackfillReport {
    pub processed: usize,
    pub matched: usize,
    pub failed: usize,
}

/// Backfill genres, person data **and movie collections** for already-`matched` titles
/// enriched before those features, **without a rescan** (`docs/.tasks/91` §Backfill).
///
/// Two passes. (1) Iterates every `matched` movie/series lacking genres (or, with `force`,
/// all matched titles) and re-runs the *details + genre/credit + person* write half of the
/// pipeline (via [`enrich_with_id`]). (2) A movies-only collection pass over matched movies
/// with no `collection_id`, paged by a `(added_at, id)` cursor so it terminates even though
/// standalone movies stay NULL — this catches movies matched before the collections feature
/// landed. Artwork already on disk is **not** re-downloaded
/// (the per-asset `maybe_download` skips an existing file), so re-processing a 10k library
/// costs one TMDB `details()` per title — bounded by the provider's own semaphore — and no
/// image bytes. Resumable: it only touches titles still missing genres, so a crash
/// mid-backfill re-runs cleanly. Invalidates the response cache once at the end if anything
/// changed.
pub async fn backfill_genres_people(ctx: &EnrichContext, force: bool) -> Result<BackfillReport> {
    /// Titles pulled per DB read; keeps memory bounded and each batch checkpointed.
    const BATCH: u32 = 500;
    let mut report = BackfillReport::default();

    for title_kind in [TitleKind::Movie, TitleKind::Series] {
        loop {
            let batch = {
                let db = ctx.db.clone();
                db_read(&db, move |conn| {
                    medi_db::queries::matched_titles_missing_genres(conn, title_kind, force, BATCH)
                })
                .await?
            };
            let batch_len = batch.len();
            if batch_len == 0 {
                break;
            }

            for id in batch {
                // Read the stored TMDB id of this matched title; without one (e.g. matched
                // by OMDb, which has no TMDB genres) there is nothing to backfill.
                let tmdb_id = {
                    let db = ctx.db.clone();
                    db_read(&db, move |conn| {
                        medi_db::queries::get_title_tmdb_id(conn, title_kind, id)
                    })
                    .await?
                };
                let media_kind = match title_kind {
                    TitleKind::Movie => MediaKind::Movie,
                    TitleKind::Series => MediaKind::Series,
                };
                let Some(tmdb_id) = tmdb_id else {
                    continue; // no TMDB linkage → skip (counts as neither processed nor failed)
                };
                let provider_id = ProviderId::Tmdb {
                    id: tmdb_id,
                    kind: media_kind,
                };
                match enrich_with_id(ctx, title_kind, id, &provider_id).await {
                    Ok(EnrichOutcome::Matched { .. }) => {
                        report.processed += 1;
                        report.matched += 1;
                    }
                    Ok(_) => report.processed += 1,
                    Err(err) => {
                        tracing::warn!(id, ?title_kind, error = %err, "backfill details fetch failed");
                        report.failed += 1;
                    }
                }
            }

            // With `force`, `matched_titles_missing_genres` returns the same rows every pass
            // (they stay matched), so a short batch is the only stop signal; a full batch
            // under force would loop forever, so cap force to a single batch per kind.
            if (batch_len as u32) < BATCH || force {
                break;
            }
        }
    }

    // Collection pass (movies only). Movies matched before the collections feature shipped
    // have `collection_id = NULL` and are already past the genre worklist, so they never get
    // their `belongs_to_collection` fetched. Re-run the movie half of the pipeline for those,
    // paging by a `(added_at, id)` cursor so the loop advances even though standalone movies
    // legitimately stay NULL (a "still missing" filter would spin forever). `enrich_with_id`
    // writes the collection when TMDB reports one; artwork already on disk is not re-fetched.
    let mut cursor: Option<(i64, i64)> = None;
    loop {
        let page = {
            let db = ctx.db.clone();
            let cur = cursor;
            db_read(&db, move |conn| {
                medi_db::queries::matched_movies_missing_collection(conn, cur, BATCH)
            })
            .await?
        };
        let page_len = page.len();
        if page_len == 0 {
            break;
        }
        // Advance the cursor past this page's last row before processing, so the next read
        // resumes correctly regardless of whether a collection was written.
        if let Some(&(last_id, last_added)) = page.last() {
            cursor = Some((last_added, last_id));
        }

        for (id, _added_at) in page {
            let tmdb_id = {
                let db = ctx.db.clone();
                db_read(&db, move |conn| {
                    medi_db::queries::get_title_tmdb_id(conn, TitleKind::Movie, id)
                })
                .await?
            };
            let Some(tmdb_id) = tmdb_id else {
                continue; // no TMDB linkage (e.g. OMDb match) → no collection to fetch
            };
            let provider_id = ProviderId::Tmdb {
                id: tmdb_id,
                kind: MediaKind::Movie,
            };
            match enrich_with_id(ctx, TitleKind::Movie, id, &provider_id).await {
                Ok(EnrichOutcome::Matched { .. }) => {
                    report.processed += 1;
                    report.matched += 1;
                }
                Ok(_) => report.processed += 1,
                Err(err) => {
                    tracing::warn!(id, error = %err, "collection backfill fetch failed");
                    report.failed += 1;
                }
            }
        }

        if (page_len as u32) < BATCH {
            break;
        }
    }

    // fanart pass (movies only, `docs/.tasks/93` logos + `docs/.tasks/95` wallpapers). Only
    // runs when fanart is configured. Movies matched before the fanart features — or that have
    // genres + a collection already, so the passes above never touched them — still have
    // `logo_path`/`wallpaper_path = NULL`. Re-run the movie half of the pipeline for those;
    // `enrich_with_id` fetches both art types in ONE fanart request and downloads them, and
    // posters/backdrops already on disk are not re-downloaded (per-asset skip). The worklist
    // filters on either column still NULL, so it shrinks each pass as art lands; a movie fanart
    // has no art for stays NULL and is re-checked on a later backfill run (the same accepted
    // trade-off as the collection pass — a full non-force batch is capped to avoid a
    // movie-with-no-art spinning forever).
    if ctx.fanart.is_some() {
        loop {
            let batch = {
                let db = ctx.db.clone();
                db_read(&db, move |conn| {
                    medi_db::queries::matched_movies_missing_fanart(conn, force, BATCH)
                })
                .await?
            };
            let batch_len = batch.len();
            if batch_len == 0 {
                break;
            }
            for id in batch {
                let tmdb_id = {
                    let db = ctx.db.clone();
                    db_read(&db, move |conn| {
                        medi_db::queries::get_title_tmdb_id(conn, TitleKind::Movie, id)
                    })
                    .await?
                };
                let Some(tmdb_id) = tmdb_id else {
                    continue; // no TMDB linkage (OMDb match) → fanart can't key it
                };
                let provider_id = ProviderId::Tmdb {
                    id: tmdb_id,
                    kind: MediaKind::Movie,
                };
                match enrich_with_id(ctx, TitleKind::Movie, id, &provider_id).await {
                    Ok(EnrichOutcome::Matched { .. }) => {
                        report.processed += 1;
                        report.matched += 1;
                    }
                    Ok(_) => report.processed += 1,
                    Err(err) => {
                        tracing::warn!(id, error = %err, "fanart backfill fetch failed");
                        report.failed += 1;
                    }
                }
            }
            // A non-force pass shrinks the worklist as art lands, but a movie fanart has no art
            // for stays NULL — so a full batch of all-artless movies would loop forever. Cap to
            // a single batch per run (like the force branch above); the next backfill run picks
            // up where this left off.
            if (batch_len as u32) < BATCH || force {
                break;
            }
        }
    }

    Ok(report)
}

/// Download the poster and backdrop for a title into
/// `<images_dir>/<kind>/<id>/{poster,backdrop}.jpg`, atomically, skipping any asset
/// already present on disk. Returns the *relative* paths (for the DB) of whichever files
/// now exist, or `None` for an asset the provider did not supply.
async fn download_artwork(
    ctx: &EnrichContext,
    title_kind: TitleKind,
    id: i64,
    details: &Details,
) -> Result<(Option<String>, Option<String>)> {
    let kind_dir = match title_kind {
        TitleKind::Movie => "movies",
        TitleKind::Series => "series",
    };
    let rel_dir = format!("{kind_dir}/{id}");
    let abs_dir = ctx.images_dir.join(kind_dir).join(id.to_string());

    let poster = maybe_download(
        ctx,
        details.poster_url.as_deref(),
        &abs_dir,
        &rel_dir,
        "poster.jpg",
    )
    .await?;
    let backdrop = maybe_download(
        ctx,
        details.backdrop_url.as_deref(),
        &abs_dir,
        &rel_dir,
        "backdrop.jpg",
    )
    .await?;
    Ok((poster, backdrop))
}

/// Download one asset if a URL is present and the target file does not already exist.
/// Returns its relative path (`<rel_dir>/<name>`) if the file exists afterward, else
/// `None`. On a download error we log and return `None` rather than failing the whole
/// enrichment — a missing poster should not block writing the overview + cast.
async fn maybe_download(
    ctx: &EnrichContext,
    url: Option<&str>,
    abs_dir: &Path,
    rel_dir: &str,
    name: &str,
) -> Result<Option<String>> {
    let abs_path = abs_dir.join(name);
    let rel_path = format!("{rel_dir}/{name}");

    // Already on disk (matched row, re-scan): keep it, record the path, no fetch.
    if abs_path.exists() {
        return Ok(Some(rel_path));
    }
    let Some(url) = url else {
        return Ok(None); // provider had no such image
    };

    match ctx.fetcher.fetch(url).await {
        Ok(bytes) => {
            write_atomic(abs_dir, name, &bytes).await?;
            Ok(Some(rel_path))
        }
        Err(err) => {
            tracing::warn!(%url, error = %err, "artwork download failed; continuing without it");
            Ok(None)
        }
    }
}

/// Write `bytes` to `dir/name` atomically: create the dir, stream to `name.tmp`, fsync,
/// then `rename` into place — `rename` within one filesystem is atomic, so a crash never
/// leaves a half-written `.jpg` that `ServeDir` would serve (`docs/.tasks/60` §Atomic
/// writes). Runs the blocking fs work on the blocking pool.
async fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let dir = dir.to_path_buf();
    let name = name.to_string();
    let bytes = bytes.to_vec();
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        use std::io::Write;
        std::fs::create_dir_all(&dir)?;
        let final_path = dir.join(&name);
        let tmp_path = dir.join(format!("{name}.tmp"));
        {
            let mut f = std::fs::File::create(&tmp_path)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp_path, &final_path)?;
        Ok(())
    })
    .await??;
    Ok(())
}

/// Whether both nominally-expected artwork files for a matched title exist. Used by the
/// idempotency gate: a matched row missing an image falls through to re-download it.
/// We consider a title's artwork "complete enough to skip" if the poster is present;
/// the backdrop is optional (some titles have none) and re-checked per asset anyway.
fn artwork_complete(images_dir: &Path, title_kind: TitleKind, id: i64) -> bool {
    let kind_dir = match title_kind {
        TitleKind::Movie => "movies",
        TitleKind::Series => "series",
    };
    images_dir.join(kind_dir).join(id.to_string()).join("poster.jpg").exists()
}

// ---------------------------------------------------------------------------
// Small async DB helpers
// ---------------------------------------------------------------------------

async fn db_read<T, F>(db: &Db, f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&rusqlite::Connection) -> medi_db::DbResult<T> + Send + 'static,
{
    let db = db.clone();
    let out = tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        f(&conn)
    })
    .await??;
    Ok(out)
}

async fn mark_state(
    ctx: &EnrichContext,
    kind: TitleKind,
    id: i64,
    state: MetadataState,
) -> Result<()> {
    let db = ctx.db.clone();
    tokio::task::spawn_blocking(move || -> medi_db::DbResult<()> {
        let conn = db.conn()?;
        writes::set_metadata_state(&conn, kind, id, state)
    })
    .await??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{CreditIn, Match};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A provider stub returning canned candidates/details, counting fetches.
    struct StubProvider {
        matches: Vec<Match>,
        details: Details,
        detail_calls: AtomicUsize,
        /// Counts `person_details` round-trips so tests assert person idempotency
        /// (`docs/.tasks/91` Phase B).
        person_calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl MetadataProvider for StubProvider {
        fn name(&self) -> &'static str {
            "stub"
        }
        async fn search(&self, _t: &str, _y: Option<i64>, _k: MediaKind) -> Result<Vec<Match>> {
            Ok(self.matches.clone())
        }
        async fn details(&self, _id: &ProviderId) -> Result<Details> {
            self.detail_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.details.clone())
        }
        async fn person_details(
            &self,
            person_tmdb_id: i64,
        ) -> Result<Option<crate::provider::PersonDetails>> {
            self.person_calls.fetch_add(1, Ordering::SeqCst);
            Ok(Some(crate::provider::PersonDetails {
                tmdb_id: person_tmdb_id,
                name: "Amy Adams".into(),
                biography: Some("An American actress.".into()),
                photo_url: Some("https://img/amy.jpg".into()),
            }))
        }
    }

    /// An image fetcher returning fixed bytes, counting downloads.
    struct StubFetcher {
        bytes: Vec<u8>,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ImageFetcher for StubFetcher {
        async fn fetch(&self, _url: &str) -> Result<Vec<u8>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.bytes.clone())
        }
    }

    /// A fanart stub returning canned logo + wallpaper URLs (each maybe `None`), counting
    /// lookups so the fanart tests can assert idempotency (`docs/.tasks/93`, `docs/.tasks/95`).
    struct StubFanart {
        logo_url: Option<String>,
        wallpaper_url: Option<String>,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl FanartArt for StubFanart {
        async fn movie_art(&self, _tmdb_id: i64) -> Result<Option<crate::fanart::MovieArt>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Some(crate::fanart::MovieArt {
                logo_url: self.logo_url.clone(),
                wallpaper_url: self.wallpaper_url.clone(),
            }))
        }
    }

    fn ctx_with(
        db: Db,
        images_dir: PathBuf,
        matches: Vec<Match>,
        details: Details,
        fetch_calls: Arc<AtomicUsize>,
    ) -> EnrichContext {
        ctx_with_person_counter(db, images_dir, matches, details, fetch_calls, Arc::new(AtomicUsize::new(0)))
    }

    /// Like [`ctx_with`] but with an explicit `person_calls` counter, so the person-enrichment
    /// tests can assert how many `person_details` round-trips happened (`docs/.tasks/91`).
    /// `fanart` is `None` — the logo feature is inert unless a test opts in via
    /// [`with_fanart`].
    fn ctx_with_person_counter(
        db: Db,
        images_dir: PathBuf,
        matches: Vec<Match>,
        details: Details,
        fetch_calls: Arc<AtomicUsize>,
        person_calls: Arc<AtomicUsize>,
    ) -> EnrichContext {
        EnrichContext {
            db,
            provider: Arc::new(StubProvider {
                matches,
                details,
                detail_calls: AtomicUsize::new(0),
                person_calls,
            }),
            fetcher: Arc::new(StubFetcher {
                bytes: vec![0xFF, 0xD8, 0xFF, 0xD9], // minimal JPEG-ish
                calls: fetch_calls,
            }),
            images_dir,
            fanart: None,
        }
    }

    /// Attach a fanart stub to a context (`docs/.tasks/93`, `docs/.tasks/95`), returning the
    /// given logo + wallpaper URLs and counting fanart lookups via `calls`.
    fn with_fanart(
        mut ctx: EnrichContext,
        logo_url: Option<&str>,
        wallpaper_url: Option<&str>,
        calls: Arc<AtomicUsize>,
    ) -> EnrichContext {
        ctx.fanart = Some(Arc::new(StubFanart {
            logo_url: logo_url.map(|s| s.to_string()),
            wallpaper_url: wallpaper_url.map(|s| s.to_string()),
            calls,
        }));
        ctx
    }

    fn seed_movie(db: &Db, title: &str, year: Option<i64>) -> i64 {
        let conn = db.conn().unwrap();
        writes::find_or_create_movie(&conn, title, &title.to_lowercase(), year, 0).unwrap()
    }

    fn arrival_match() -> Match {
        Match {
            provider_id: ProviderId::Tmdb { id: 329865, kind: MediaKind::Movie },
            title: "Arrival".into(),
            year: Some(2016),
            score: 1.0,
        }
    }

    /// The default fixture used by the **artwork-focused** tests: its single credit carries
    /// **no** `person_tmdb_id`, so person enrichment is a no-op and the download counter
    /// stays purely poster + backdrop. The person-enrichment test uses
    /// [`arrival_details_with_person`] to add a headshot download.
    fn arrival_details() -> Details {
        Details {
            overview: Some("Aliens arrive.".into()),
            cast: vec![CreditIn {
                name: "Amy Adams".into(),
                role: "actor".into(),
                character: Some("Louise".into()),
                ord: 0,
                person_tmdb_id: None,
            }],
            poster_url: Some("https://img/poster.jpg".into()),
            backdrop_url: Some("https://img/backdrop.jpg".into()),
            imdb_id: Some("tt2543164".into()),
            tmdb_id: Some(329865),
            genres: vec![
                crate::provider::Genre { tmdb_id: 878, name: "Science Fiction".into() },
                crate::provider::Genre { tmdb_id: 18, name: "Drama".into() },
            ],
            collection: None,
            trailers: Vec::new(),
        }
    }

    /// Like [`arrival_details`] but with a `person_tmdb_id` on the credit, so the person is
    /// enriched (headshot download + meta write) — used by the Phase B person test.
    fn arrival_details_with_person() -> Details {
        let mut d = arrival_details();
        d.cast[0].person_tmdb_id = Some(9273);
        d
    }

    #[tokio::test]
    async fn enriches_and_writes_artwork_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let db = medi_db::open(dir.path().join("library.db"), 2).unwrap();
        let images = dir.path().join("images");
        let id = seed_movie(&db, "Arrival", Some(2016));

        let calls = Arc::new(AtomicUsize::new(0));
        let ctx = ctx_with(db.clone(), images.clone(), vec![arrival_match()], arrival_details(), calls.clone());

        let outcome = enrich_movie(&ctx, id, false).await.unwrap();
        assert!(matches!(outcome, EnrichOutcome::Matched { .. }));

        // Row is matched, overview + poster path written.
        let conn = db.conn().unwrap();
        let m = medi_db::queries::get_movie(&conn, id).unwrap();
        assert_eq!(m.overview.as_deref(), Some("Aliens arrive."));
        assert_eq!(m.poster_path.as_deref(), Some(&*format!("movies/{id}/poster.jpg")));
        assert_eq!(m.backdrop_path.as_deref(), Some(&*format!("movies/{id}/backdrop.jpg")));

        // Files exist on disk, no stray .tmp.
        let poster = images.join(format!("movies/{id}/poster.jpg"));
        assert!(poster.exists());
        assert!(!images.join(format!("movies/{id}/poster.jpg.tmp")).exists());

        // Two downloads (poster + backdrop).
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // Credits written.
        let credits = medi_db::queries::credits_for_movie(&conn, id).unwrap();
        assert_eq!(credits.len(), 1);
        assert_eq!(credits[0].person_name, "Amy Adams");

        // Genres written + surfaced (`docs/.tasks/91` Phase A): the title's two genres show
        // up in the genre list with a count of 1 each.
        let genres = medi_db::queries::list_genres(&conn).unwrap();
        assert_eq!(genres.len(), 2);
        assert!(genres.iter().any(|g| g.id == 878 && g.name == "Science Fiction" && g.count == 1));
        // And the genre grid returns this movie.
        let in_genre = medi_db::queries::list_by_genre(
            &conn, 878, medi_db::queries::LibrarySort::SortTitle, None, 60,
        )
        .unwrap();
        assert_eq!(in_genre.len(), 1);
        assert_eq!(in_genre[0].id, id);
    }

    #[tokio::test]
    async fn re_match_replaces_genres() {
        // A corrected re-match writes a different genre set; the stale genres are dropped
        // from the title's joins (delete-then-insert), so the grid reflects only the new set.
        let dir = tempfile::tempdir().unwrap();
        let db = medi_db::open(dir.path().join("library.db"), 2).unwrap();
        let images = dir.path().join("images");
        let id = seed_movie(&db, "Arrival", Some(2016));

        let calls = Arc::new(AtomicUsize::new(0));
        let ctx = ctx_with(db.clone(), images.clone(), vec![arrival_match()], arrival_details(), calls.clone());
        enrich_movie(&ctx, id, false).await.unwrap();

        // Force a re-enrich against a provider whose details carry only "Action".
        let mut other = arrival_details();
        other.genres = vec![crate::provider::Genre { tmdb_id: 28, name: "Action".into() }];
        let ctx2 = ctx_with(db.clone(), images.clone(), vec![arrival_match()], other, calls.clone());
        enrich_movie(&ctx2, id, true).await.unwrap();

        let conn = db.conn().unwrap();
        // The two original genres no longer reference this title; only Action does.
        assert!(medi_db::queries::list_by_genre(&conn, 878, medi_db::queries::LibrarySort::SortTitle, None, 60).unwrap().is_empty());
        let action = medi_db::queries::list_by_genre(&conn, 28, medi_db::queries::LibrarySort::SortTitle, None, 60).unwrap();
        assert_eq!(action.len(), 1);
    }

    #[tokio::test]
    async fn enrich_writes_person_meta_and_downloads_headshot() {
        // A match with a cast member carrying a person_tmdb_id fetches the person, writes
        // tmdb_id + bio, and downloads the headshot atomically to people/<id>/photo.jpg
        // (`docs/.tasks/91` Phase B). A re-enrich does NOT re-fetch the already-enriched
        // person (idempotent).
        let dir = tempfile::tempdir().unwrap();
        let db = medi_db::open(dir.path().join("library.db"), 2).unwrap();
        let images = dir.path().join("images");
        let id = seed_movie(&db, "Arrival", Some(2016));

        let fetch_calls = Arc::new(AtomicUsize::new(0));
        let person_calls = Arc::new(AtomicUsize::new(0));
        let ctx = ctx_with_person_counter(
            db.clone(),
            images.clone(),
            vec![arrival_match()],
            arrival_details_with_person(),
            fetch_calls.clone(),
            person_calls.clone(),
        );

        enrich_movie(&ctx, id, false).await.unwrap();
        assert_eq!(person_calls.load(Ordering::SeqCst), 1, "one person fetched");
        // poster + backdrop + one headshot.
        assert_eq!(fetch_calls.load(Ordering::SeqCst), 3, "artwork + headshot downloaded");

        // The person row carries the TMDB linkage, bio, and a photo path.
        let conn = db.conn().unwrap();
        let person = {
            // Amy Adams is the only credited person.
            let credits = medi_db::queries::credits_for_movie(&conn, id).unwrap();
            let pid = credits[0].person_id;
            medi_db::queries::get_person(&conn, pid).unwrap()
        };
        assert_eq!(person.name, "Amy Adams");
        assert_eq!(person.tmdb_id, Some(9273));
        assert_eq!(person.biography.as_deref(), Some("An American actress."));
        let expected_photo = format!("people/{}/photo.jpg", person.id);
        assert_eq!(person.photo_path.as_deref(), Some(&*expected_photo));

        // The headshot exists on disk, no stray .tmp.
        let photo = images.join(&expected_photo);
        assert!(photo.exists(), "headshot downloaded to {expected_photo}");
        assert!(!images.join(format!("{expected_photo}.tmp")).exists());

        // A forced re-enrich of the title does NOT re-fetch the already-enriched person
        // (has tmdb_id + photo → skipped), so the person-call count stays at 1.
        drop(conn);
        enrich_movie(&ctx, id, true).await.unwrap();
        assert_eq!(person_calls.load(Ordering::SeqCst), 1, "enriched person is not re-fetched");
    }

    #[tokio::test]
    async fn person_filmography_lists_credited_titles_only() {
        // A person's filmography is the in-library titles they are credited on, and excludes
        // titles they are not on (`docs/.tasks/91` Phase B db test).
        let dir = tempfile::tempdir().unwrap();
        let db = medi_db::open(dir.path().join("library.db"), 2).unwrap();
        let conn = db.conn().unwrap();
        // Two movies credited to Amy Adams, one to someone else.
        let m1 = writes::find_or_create_movie(&conn, "Arrival", "arrival", Some(2016), 100).unwrap();
        let m2 = writes::find_or_create_movie(&conn, "Nocturnal", "nocturnal", Some(2016), 200).unwrap();
        let other = writes::find_or_create_movie(&conn, "The Departed", "departed", Some(2006), 50).unwrap();
        for id in [m1, m2] {
            writes::replace_credits(
                &conn,
                TitleKind::Movie,
                id,
                &[medi_db::writes::CreditWrite {
                    name: "Amy Adams".into(),
                    role: "actor".into(),
                    character: None,
                    ord: 0,
                }],
            )
            .unwrap();
        }
        writes::replace_credits(
            &conn,
            TitleKind::Movie,
            other,
            &[medi_db::writes::CreditWrite {
                name: "Leonardo DiCaprio".into(),
                role: "actor".into(),
                character: None,
                ord: 0,
            }],
        )
        .unwrap();

        let amy = medi_db::queries::get_person_enrichment_state(&conn, "Amy Adams").unwrap().unwrap().0;
        let films = medi_db::queries::person_filmography(&conn, amy).unwrap();
        // Only Amy's two titles, newest-added first (Nocturnal added_at=200 before Arrival=100).
        assert_eq!(films.len(), 2);
        assert_eq!(films[0].id, m2);
        assert_eq!(films[1].id, m1);
        assert!(!films.iter().any(|c| c.id == other), "excludes uncredited titles");
    }

    #[tokio::test]
    async fn backfill_fills_genres_without_redownloading_art() {
        // A movie matched *before* this task has artwork + credits but no genres. The
        // backfill re-fetches details and fills the genres — and does NOT re-download the
        // poster/backdrop already on disk (asserted via the fetch counter).
        let dir = tempfile::tempdir().unwrap();
        let db = medi_db::open(dir.path().join("library.db"), 2).unwrap();
        let images = dir.path().join("images");
        let id = seed_movie(&db, "Arrival", Some(2016));

        let calls = Arc::new(AtomicUsize::new(0));
        let ctx = ctx_with(db.clone(), images.clone(), vec![arrival_match()], arrival_details(), calls.clone());
        // Initial enrich lands art + genres; then simulate a "pre-genre" matched row by
        // deleting the join rows (as a library enriched before V6 would have none).
        enrich_movie(&ctx, id, false).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2, "poster + backdrop downloaded once");
        {
            let conn = db.conn().unwrap();
            conn.execute("DELETE FROM movie_genres", []).unwrap();
            assert!(medi_db::queries::list_genres(&conn).unwrap().is_empty());
        }

        // Backfill: re-fetches details, re-writes genres, re-downloads NO art. Arrival is a
        // standalone movie (no `belongs_to_collection`), so it's touched twice — once by the
        // genre pass, once by the collection pass (matched movies with a NULL collection are
        // the collection worklist) — hence processed/matched == 2.
        let report = backfill_genres_people(&ctx, false).await.unwrap();
        assert_eq!(report.processed, 2);
        assert_eq!(report.matched, 2);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "no artwork re-download during backfill");

        let conn = db.conn().unwrap();
        let genres = medi_db::queries::list_genres(&conn).unwrap();
        assert_eq!(genres.len(), 2, "backfill restored the title's genres");
        drop(conn);

        // A second backfill: the genre pass now finds nothing missing, but the collection pass
        // re-checks the still-collectionless movie once (the accepted no-marker trade-off), so
        // exactly one title is processed.
        let again = backfill_genres_people(&ctx, false).await.unwrap();
        assert_eq!(again.processed, 1);
    }

    #[tokio::test]
    async fn matched_row_is_skipped_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let db = medi_db::open(dir.path().join("library.db"), 2).unwrap();
        let images = dir.path().join("images");
        let id = seed_movie(&db, "Arrival", Some(2016));

        let calls = Arc::new(AtomicUsize::new(0));
        let ctx = ctx_with(db.clone(), images.clone(), vec![arrival_match()], arrival_details(), calls.clone());

        // First pass matches + downloads.
        enrich_movie(&ctx, id, false).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // Second pass: matched + artwork present → skipped, no new downloads.
        let outcome = enrich_movie(&ctx, id, false).await.unwrap();
        assert_eq!(outcome, EnrichOutcome::Skipped);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "no re-download on idempotent re-run");

        // Force re-download re-fetches.
        let outcome = enrich_movie(&ctx, id, true).await.unwrap();
        assert!(matches!(outcome, EnrichOutcome::Matched { .. }));
    }

    #[tokio::test]
    async fn below_threshold_marks_unmatched_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let db = medi_db::open(dir.path().join("library.db"), 2).unwrap();
        let images = dir.path().join("images");
        let id = seed_movie(&db, "Some Obscure Home Video", None);

        // A low-scoring candidate.
        let weak = Match {
            provider_id: ProviderId::Tmdb { id: 1, kind: MediaKind::Movie },
            title: "Completely Different Film".into(),
            year: Some(1980),
            score: 0.1,
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let ctx = ctx_with(db.clone(), images.clone(), vec![weak], arrival_details(), calls.clone());

        let outcome = enrich_movie(&ctx, id, false).await.unwrap();
        assert_eq!(outcome, EnrichOutcome::Unmatched);
        assert_eq!(calls.load(Ordering::SeqCst), 0, "no download for an unmatched title");

        let conn = db.conn().unwrap();
        assert_eq!(
            writes::get_metadata_state(&conn, TitleKind::Movie, id).unwrap().as_deref(),
            Some("unmatched")
        );
        let m = medi_db::queries::get_movie(&conn, id).unwrap();
        assert!(m.overview.is_none());
    }

    #[tokio::test]
    async fn re_download_only_missing_asset_when_matched_but_file_gone() {
        let dir = tempfile::tempdir().unwrap();
        let db = medi_db::open(dir.path().join("library.db"), 2).unwrap();
        let images = dir.path().join("images");
        let id = seed_movie(&db, "Arrival", Some(2016));

        let calls = Arc::new(AtomicUsize::new(0));
        let ctx = ctx_with(db.clone(), images.clone(), vec![arrival_match()], arrival_details(), calls.clone());
        enrich_movie(&ctx, id, false).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // Manually delete just the backdrop, keep the poster.
        std::fs::remove_file(images.join(format!("movies/{id}/backdrop.jpg"))).unwrap();

        // A non-force pass on a matched row whose poster is present is skipped by the gate
        // (poster is the completeness signal) — so force here to exercise re-download of
        // only the gone file: the poster already on disk is NOT re-fetched.
        let before = calls.load(Ordering::SeqCst);
        enrich_movie(&ctx, id, true).await.unwrap();
        let after = calls.load(Ordering::SeqCst);
        assert_eq!(after - before, 1, "only the missing backdrop is re-downloaded");
    }

    // -----------------------------------------------------------------------
    // fanart.tv title logos (`docs/.tasks/93`)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn enrich_downloads_and_writes_logo_and_wallpaper() {
        // With a fanart client returning a logo + wallpaper URL, a matched movie writes both
        // paths, both files exist (no stray .tmp), and a forced re-enrich re-downloads neither
        // (both already on disk → the fanart request is short-circuited).
        let dir = tempfile::tempdir().unwrap();
        let db = medi_db::open(dir.path().join("library.db"), 2).unwrap();
        let images = dir.path().join("images");
        let id = seed_movie(&db, "Arrival", Some(2016));

        let fetch_calls = Arc::new(AtomicUsize::new(0));
        let fanart_calls = Arc::new(AtomicUsize::new(0));
        let ctx = with_fanart(
            ctx_with(db.clone(), images.clone(), vec![arrival_match()], arrival_details(), fetch_calls.clone()),
            Some("https://assets.fanart.tv/logo.png"),
            Some("https://assets.fanart.tv/wallpaper.jpg"),
            fanart_calls.clone(),
        );

        enrich_movie(&ctx, id, false).await.unwrap();

        // Both paths written on the movie row.
        {
            let conn = db.conn().unwrap();
            let m = medi_db::queries::get_movie(&conn, id).unwrap();
            assert_eq!(m.logo_path.as_deref(), Some(&*format!("movies/{id}/logo.png")));
            assert_eq!(m.wallpaper_path.as_deref(), Some(&*format!("movies/{id}/wallpaper.jpg")));
        }
        // Both files exist, no stray .tmp.
        assert!(images.join(format!("movies/{id}/logo.png")).exists(), "logo downloaded");
        assert!(images.join(format!("movies/{id}/wallpaper.jpg")).exists(), "wallpaper downloaded");
        assert!(!images.join(format!("movies/{id}/logo.png.tmp")).exists());
        assert!(!images.join(format!("movies/{id}/wallpaper.jpg.tmp")).exists());
        // poster + backdrop + logo + wallpaper were fetched; fanart was queried once.
        assert_eq!(fetch_calls.load(Ordering::SeqCst), 4, "poster + backdrop + logo + wallpaper");
        assert_eq!(fanart_calls.load(Ordering::SeqCst), 1, "one fanart lookup for both art types");

        // A forced re-enrich keeps both on-disk assets and never re-queries fanart (both files
        // exist → short-circuit before the HTTP lookup).
        enrich_movie(&ctx, id, true).await.unwrap();
        assert_eq!(fanart_calls.load(Ordering::SeqCst), 1, "both on disk → no re-lookup");
        let conn = db.conn().unwrap();
        let m = medi_db::queries::get_movie(&conn, id).unwrap();
        assert_eq!(m.logo_path.as_deref(), Some(&*format!("movies/{id}/logo.png")));
        assert_eq!(m.wallpaper_path.as_deref(), Some(&*format!("movies/{id}/wallpaper.jpg")));
    }

    #[tokio::test]
    async fn enrich_with_no_fanart_art_writes_null_and_still_matches() {
        // fanart configured but has neither logo nor wallpaper for this movie → both columns
        // stay NULL and the movie still matches (missing art never fails enrichment).
        let dir = tempfile::tempdir().unwrap();
        let db = medi_db::open(dir.path().join("library.db"), 2).unwrap();
        let images = dir.path().join("images");
        let id = seed_movie(&db, "Arrival", Some(2016));

        let fetch_calls = Arc::new(AtomicUsize::new(0));
        let fanart_calls = Arc::new(AtomicUsize::new(0));
        let ctx = with_fanart(
            ctx_with(db.clone(), images.clone(), vec![arrival_match()], arrival_details(), fetch_calls.clone()),
            None, // no logo
            None, // no wallpaper
            fanart_calls.clone(),
        );

        let outcome = enrich_movie(&ctx, id, false).await.unwrap();
        assert!(matches!(outcome, EnrichOutcome::Matched { .. }));
        assert_eq!(fanart_calls.load(Ordering::SeqCst), 1, "fanart was queried");
        assert_eq!(fetch_calls.load(Ordering::SeqCst), 2, "poster + backdrop only, no fanart art");

        let conn = db.conn().unwrap();
        let m = medi_db::queries::get_movie(&conn, id).unwrap();
        assert!(m.logo_path.is_none(), "no logo → logo_path NULL");
        assert!(m.wallpaper_path.is_none(), "no wallpaper → wallpaper_path NULL");
        assert!(!images.join(format!("movies/{id}/logo.png")).exists());
        assert!(!images.join(format!("movies/{id}/wallpaper.jpg")).exists());
    }

    #[tokio::test]
    async fn enrich_downloads_only_the_present_fanart_art() {
        // fanart has a wallpaper but no logo → only the wallpaper is written/downloaded; the
        // logo column stays NULL. Proves the two art types are independent.
        let dir = tempfile::tempdir().unwrap();
        let db = medi_db::open(dir.path().join("library.db"), 2).unwrap();
        let images = dir.path().join("images");
        let id = seed_movie(&db, "Arrival", Some(2016));

        let fetch_calls = Arc::new(AtomicUsize::new(0));
        let fanart_calls = Arc::new(AtomicUsize::new(0));
        let ctx = with_fanart(
            ctx_with(db.clone(), images.clone(), vec![arrival_match()], arrival_details(), fetch_calls.clone()),
            None,
            Some("https://assets.fanart.tv/wallpaper.jpg"),
            fanart_calls.clone(),
        );

        enrich_movie(&ctx, id, false).await.unwrap();
        assert_eq!(fetch_calls.load(Ordering::SeqCst), 3, "poster + backdrop + wallpaper (no logo)");

        let conn = db.conn().unwrap();
        let m = medi_db::queries::get_movie(&conn, id).unwrap();
        assert!(m.logo_path.is_none(), "no logo");
        assert_eq!(m.wallpaper_path.as_deref(), Some(&*format!("movies/{id}/wallpaper.jpg")));
        assert!(images.join(format!("movies/{id}/wallpaper.jpg")).exists());
        assert!(!images.join(format!("movies/{id}/logo.png")).exists());
    }

    #[tokio::test]
    async fn fanart_feature_inert_without_client() {
        // ctx.fanart == None → no fanart lookup, no logo/wallpaper write; the movie still matches.
        let dir = tempfile::tempdir().unwrap();
        let db = medi_db::open(dir.path().join("library.db"), 2).unwrap();
        let images = dir.path().join("images");
        let id = seed_movie(&db, "Arrival", Some(2016));

        let calls = Arc::new(AtomicUsize::new(0));
        let ctx = ctx_with(db.clone(), images.clone(), vec![arrival_match()], arrival_details(), calls.clone());
        let outcome = enrich_movie(&ctx, id, false).await.unwrap();
        assert!(matches!(outcome, EnrichOutcome::Matched { .. }));
        assert_eq!(calls.load(Ordering::SeqCst), 2, "poster + backdrop only");

        let conn = db.conn().unwrap();
        let m = medi_db::queries::get_movie(&conn, id).unwrap();
        assert!(m.logo_path.is_none(), "feature off → no logo write");
        assert!(m.wallpaper_path.is_none(), "feature off → no wallpaper write");
    }

    #[tokio::test]
    async fn backfill_fills_fanart_art_without_redownloading_art() {
        // A movie matched *before* the fanart features has poster/backdrop + genres + collection
        // but no logo/wallpaper. A backfill (with fanart configured) fills both — and does NOT
        // re-download the poster/backdrop already on disk.
        let dir = tempfile::tempdir().unwrap();
        let db = medi_db::open(dir.path().join("library.db"), 2).unwrap();
        let images = dir.path().join("images");
        let id = seed_movie(&db, "Arrival", Some(2016));

        let fetch_calls = Arc::new(AtomicUsize::new(0));
        let fanart_calls = Arc::new(AtomicUsize::new(0));

        // First enrich WITHOUT fanart → art + genres land, fanart columns stay NULL (pre-feature).
        let ctx_no_fanart = ctx_with(db.clone(), images.clone(), vec![arrival_match()], arrival_details(), fetch_calls.clone());
        enrich_movie(&ctx_no_fanart, id, false).await.unwrap();
        assert_eq!(fetch_calls.load(Ordering::SeqCst), 2, "poster + backdrop downloaded once");
        {
            let conn = db.conn().unwrap();
            let m = medi_db::queries::get_movie(&conn, id).unwrap();
            assert!(m.logo_path.is_none() && m.wallpaper_path.is_none());
        }

        // Backfill WITH fanart: fills logo + wallpaper, downloads each once, no art re-download.
        let ctx = with_fanart(
            ctx_with(db.clone(), images.clone(), vec![arrival_match()], arrival_details(), fetch_calls.clone()),
            Some("https://assets.fanart.tv/logo.png"),
            Some("https://assets.fanart.tv/wallpaper.jpg"),
            fanart_calls.clone(),
        );
        backfill_genres_people(&ctx, false).await.unwrap();

        let conn = db.conn().unwrap();
        let m = medi_db::queries::get_movie(&conn, id).unwrap();
        assert_eq!(m.logo_path.as_deref(), Some(&*format!("movies/{id}/logo.png")), "backfill filled the logo");
        assert_eq!(m.wallpaper_path.as_deref(), Some(&*format!("movies/{id}/wallpaper.jpg")), "backfill filled the wallpaper");
        assert!(images.join(format!("movies/{id}/logo.png")).exists());
        assert!(images.join(format!("movies/{id}/wallpaper.jpg")).exists());
        // Two new downloads (logo + wallpaper); poster/backdrop already on disk were not re-fetched.
        assert_eq!(fetch_calls.load(Ordering::SeqCst), 4, "only logo + wallpaper downloaded during backfill");
    }
}
