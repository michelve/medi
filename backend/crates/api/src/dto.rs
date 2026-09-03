//! Public response DTOs — the exact JSON shapes in
//! `docs/.tasks/02-api-contract.md` §Representative response shapes.
//!
//! Movie/series *detail* responses reuse the aggregates from `medi_db::models`
//! (`MovieDetail` / `SeriesDetail`) directly. This module holds the shapes that
//! differ from a raw row: the unified library card (which exposes a poster *URL*,
//! not a stored path) and the stream decision envelope.

use serde::{Deserialize, Serialize};

use medi_db::models::{LibraryCard, LibraryKind};

/// One page of the unified catalog. `next_cursor` is `null` when the list is
/// exhausted (`docs/.tasks/02-api-contract.md`).
#[derive(Debug, Serialize)]
pub struct LibraryPage {
    pub items: Vec<LibraryItem>,
    pub next_cursor: Option<String>,
}

/// A single poster tile in `/api/library`.
#[derive(Debug, Serialize)]
pub struct LibraryItem {
    /// `"movie"` or `"series"`.
    pub kind: &'static str,
    pub id: i64,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i64>,
    /// A ready-to-fetch `/api/images/...` URL, or `null` when the title has no art.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poster: Option<String>,
    /// Highest HDR tier across the title's files (`"dolbyvision"`, `"hdr10"`, …),
    /// omitted for SDR / unprobed titles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hdr: Option<String>,
}

impl LibraryItem {
    /// Build the public tile from a DB [`LibraryCard`], turning the stored
    /// `poster_path` into a client-facing `/api/images/<path>` URL.
    pub fn from_card(card: LibraryCard) -> Self {
        let kind = match card.kind {
            LibraryKind::Movie => "movie",
            LibraryKind::Series => "series",
        };
        Self {
            kind,
            id: card.id,
            title: card.title,
            year: card.year,
            poster: card.poster_path.map(image_url),
            hdr: card.hdr,
        }
    }
}

/// Turn a stored artwork path (relative to the images root, per `AppConfig`) into
/// the public URL the client fetches. Leading slashes are trimmed so the result is
/// always `/api/images/<clean path>`.
pub fn image_url(stored_path: String) -> String {
    format!("/api/images/{}", stored_path.trim_start_matches('/'))
}

/// Trickplay scrub-thumbnail metadata returned by `GET /api/trickplay/:file_id/meta`.
///
/// The sprite *image* is served separately as a static file (`/api/trickplay/<id>.jpg`);
/// this envelope carries the grid geometry the client needs to crop the right cell out
/// of the tiled-JPG mosaic while scrubbing (`docs/.tasks/50` Part A; the API contract's
/// promised "sprite + metadata"). Only the **tiled-JPG** kind is representable here —
/// a BIF asset has no client-croppable grid, so the handler returns `404` for it and
/// the player falls back to a plain scrub bar.
#[derive(Debug, Serialize)]
pub struct TrickplayMeta {
    pub file_id: i64,
    /// Always `"tiled_jpg"` for a served meta (BIF is 404'd — see above).
    pub kind: String,
    /// Milliseconds between sampled frames (one tile per interval).
    pub interval_ms: i64,
    /// Width of a single thumbnail cell, px.
    pub tile_w: i64,
    /// Height of a single thumbnail cell, px.
    pub tile_h: i64,
    /// Columns in the mosaic.
    pub cols: i64,
    /// Rows in the mosaic.
    pub rows: i64,
}

// ---------------------------------------------------------------------------
// Genres & discovery (`docs/.tasks/91` Phase A)
// ---------------------------------------------------------------------------

/// One entry in `GET /api/genres` — a genre with a nonzero title count. Backs the browse
/// rows and the genre chips.
#[derive(Debug, Serialize)]
pub struct GenreListItem {
    pub id: i64,
    pub name: String,
    /// Number of titles (movies + series) carrying this genre.
    pub count: i64,
}

impl From<medi_db::models::GenreCount> for GenreListItem {
    fn from(g: medi_db::models::GenreCount) -> Self {
        Self { id: g.id, name: g.name, count: g.count }
    }
}

/// One horizontal category row on the landing page (`GET /api/library/rows`).
///
/// `key` is a stable machine id (`recently_added`, or `genre:878`); `title` is the display
/// heading; `items` is a capped set of poster tiles (same `LibraryItem` shape as the grid).
#[derive(Debug, Serialize)]
pub struct CategoryRow {
    pub key: String,
    pub title: String,
    pub items: Vec<LibraryItem>,
    /// The genre id backing a genre row, so the client can link "See all →" to
    /// `/genre/:id`. `null` for the synthetic "Recently Added" row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre_id: Option<i64>,
}

/// `GET /api/library/rows` — the landing page's curated category rows in one request
/// (`docs/.tasks/91`): "Recently Added" plus the top-N genres by count, each capped.
#[derive(Debug, Serialize)]
pub struct LibraryRows {
    pub rows: Vec<CategoryRow>,
}

/// `GET /api/movies/:id` response (Task 91 detail extensions): the DB movie detail (which
/// carries the movie row, files, credits, trailers, and collection) plus the collection's
/// **other** in-library movies as poster tiles. The DB `MovieDetail` is flattened in, so the
/// existing fields keep their positions; `collection_movies` is the only added key.
#[derive(Debug, Serialize)]
pub struct MovieDetailResponse {
    #[serde(flatten)]
    pub detail: medi_db::models::MovieDetail,
    /// The other in-library movies of this movie's franchise (this movie excluded), newest
    /// first — the "Collection" row. Empty when standalone.
    pub collection_movies: Vec<LibraryItem>,
}

/// `GET /api/people/:id` — a person page (`docs/.tasks/91` Phase B): the enriched person
/// plus their in-library filmography (poster tiles, newest first). `photo` is a ready-to-fetch
/// `/api/images/people/<id>/photo.jpg` URL (or `null` pre-enrichment), mirroring how a card's
/// `poster` is surfaced.
#[derive(Debug, Serialize)]
pub struct PersonPage {
    pub id: i64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub photo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub biography: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    /// The person's titles present in this library, newest first.
    pub filmography: Vec<LibraryItem>,
}

impl PersonPage {
    /// Build the page from the person row + their filmography cards, turning the stored
    /// `photo_path` into a client-facing `/api/images/...` URL.
    pub fn build(person: medi_db::models::PersonMeta, filmography: Vec<LibraryItem>) -> Self {
        Self {
            id: person.id,
            name: person.name,
            photo: person.photo_path.map(image_url),
            biography: person.biography,
            tmdb_id: person.tmdb_id,
            filmography,
        }
    }
}

/// `POST /api/metadata/backfill` acknowledgement — the backfill runs in the background, so
/// this reports only that it was accepted (the counts are logged as it progresses).
#[derive(Debug, Serialize)]
pub struct BackfillResponse {
    /// `"accepted"` — the backfill task was spawned.
    pub status: &'static str,
    /// Whether a backfill was already running (a re-hit is idempotent, not queued twice).
    pub already_running: bool,
}

// ---------------------------------------------------------------------------
// Metadata enrichment (`docs/.tasks/60` Phase A) — refresh / matches / match
// ---------------------------------------------------------------------------

/// The result of `POST /api/movies/:id/refresh` — a forced re-enrichment.
#[derive(Debug, Serialize)]
pub struct RefreshResponse {
    pub id: i64,
    /// `"matched"` | `"unmatched"` | `"skipped"`.
    pub outcome: &'static str,
    /// The pinned provider token when matched (`tmdb:movie:603`), else `null`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
}

/// One candidate in `GET /api/movies/:id/matches` — a provider result the client can pin
/// via `POST /api/movies/:id/match`.
#[derive(Debug, Serialize)]
pub struct MatchCandidate {
    /// Opaque provider token to pass back to `/match` (`tmdb:movie:603`, `imdb:tt…`).
    pub provider_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i64>,
    /// Match confidence in `[0,1]` (title similarity + year agreement).
    pub score: f64,
}

/// `GET /api/movies/:id/matches` response: the candidate list, best-first.
#[derive(Debug, Serialize)]
pub struct MatchesResponse {
    pub id: i64,
    pub candidates: Vec<MatchCandidate>,
}

/// `POST /api/movies/:id/match` body: the provider token to pin, then re-enrich against.
#[derive(Debug, Deserialize)]
pub struct MatchRequest {
    pub provider_id: String,
}

/// The playback decision returned by `GET /api/stream/:file_id`.
///
/// `mode` is `"direct"` (client fetches `/api/direct/:file_id` with `Range`) or
/// `"hls"` (client opens the returned `url`, an `index.m3u8`). `reason` is a stable
/// slug for logs/debugging (e.g. `"dv_p5_sdr_display"`), produced by
/// `medi_transcode::Decision::reason`.
#[derive(Debug, Serialize)]
pub struct StreamDecision {
    pub file_id: i64,
    /// `"direct"` or `"hls"`.
    pub mode: &'static str,
    pub reason: &'static str,
    pub url: String,
}

// ---------------------------------------------------------------------------
// Libraries (`docs/.tasks/60` Phase B) — request bodies
// ---------------------------------------------------------------------------

/// `POST /api/libraries` body: create a library with an initial folder set.
#[derive(Debug, Deserialize)]
pub struct CreateLibraryRequest {
    pub name: String,
    /// `"movie"` | `"series"`.
    pub kind: String,
    #[serde(default)]
    pub folders: Vec<String>,
}

/// `PATCH /api/libraries/:id` body: rename and/or add/remove folders. All fields
/// optional — a request may just rename, just add, or just remove.
#[derive(Debug, Deserialize)]
pub struct PatchLibraryRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub add_folders: Vec<String>,
    #[serde(default)]
    pub remove_folders: Vec<String>,
}
