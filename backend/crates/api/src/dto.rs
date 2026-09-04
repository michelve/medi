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
// Per-file tracks (`docs/.tasks/97` Part C) — `GET /api/files/:file_id`
// ---------------------------------------------------------------------------

/// One audio track in `GET /api/files/:file_id` — the subset of `audio_streams` a player's
/// audio menu needs to label + select a track (`docs/.tasks/97` Part C). `stream_index` is
/// the value the client passes back as `?audio_track=` on `/api/stream`.
#[derive(Debug, Serialize)]
pub struct FileAudioTrack {
    pub stream_index: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_layout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub is_default: bool,
}

impl From<medi_db::models::AudioStream> for FileAudioTrack {
    fn from(a: medi_db::models::AudioStream) -> Self {
        Self {
            stream_index: a.stream_index,
            codec: a.codec,
            channels: a.channels,
            channel_layout: a.channel_layout,
            language: a.language,
            title: a.title,
            is_default: a.is_default,
        }
    }
}

/// One subtitle track in `GET /api/files/:file_id` — enough for a player's caption menu +
/// the burn-in / WebVTT-sidecar choice (`docs/.tasks/97` Part C, consumed by `99`). `id` is
/// the `subtitle_streams` row id (used to address an external sidecar as `ext<id>`);
/// `stream_index` is the embedded ffprobe index (absent for an external track).
#[derive(Debug, Serialize)]
pub struct FileSubtitleTrack {
    pub id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_index: Option<i64>,
    pub external: bool,
    /// ffprobe `codec_name` (subrip, ass, ssa, webvtt, hdmv_pgs_subtitle, dvd_subtitle, …).
    /// The client keys its render path on this: `ass`/`ssa` → libass, PGS/VobSub → libbitsub,
    /// plain text → native `<track>` (`docs/.tasks/99`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    /// `"text"` | `"image"`.
    pub format: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub is_default: bool,
    pub is_forced: bool,
}

impl From<medi_db::models::SubtitleStream> for FileSubtitleTrack {
    fn from(s: medi_db::models::SubtitleStream) -> Self {
        Self {
            id: s.id,
            stream_index: s.stream_index,
            external: s.is_external,
            codec: s.codec,
            format: s.format,
            language: s.language,
            title: s.title,
            is_default: s.is_default,
            is_forced: s.is_forced,
        }
    }
}

/// One chapter marker of a file (`docs/.tasks/99`), projected for the player's scrub bar.
#[derive(Debug, Serialize)]
pub struct FileChapter {
    pub ordinal: i64,
    pub start_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Whether a poster frame is available at `GET /api/chapters/:file_id/image/:ordinal`
    /// (`docs/.tasks/99` Part C). Omitted when false to keep the payload lean; the client shows
    /// the hover image / scene card only when this is `true`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub image: bool,
}

impl From<medi_db::models::Chapter> for FileChapter {
    fn from(c: medi_db::models::Chapter) -> Self {
        Self {
            ordinal: c.ordinal,
            start_ms: c.start_ms,
            end_ms: c.end_ms,
            title: c.title,
            image: c.has_image,
        }
    }
}

/// `GET /api/files/:file_id` — a file's audio + subtitle tracks (`docs/.tasks/97` Part C).
///
/// Lets a **deep link** to `/play/:file_id` (with no router state) populate the player's
/// audio-track and caption menus, and (Task 99) chapter ticks on the scrub bar. Defined once
/// here and consumed by both specs.
#[derive(Debug, Serialize)]
pub struct FileTracks {
    pub file_id: i64,
    pub audio: Vec<FileAudioTrack>,
    pub subtitles: Vec<FileSubtitleTrack>,
    pub chapters: Vec<FileChapter>,
    /// Video frame rate (`docs/.tasks/99`), for the web player's libass `targetFps`. Absent
    /// until the file has been (re)probed since V13.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_fps: Option<f64>,
}

// ---------------------------------------------------------------------------
// Playback progress (`docs/.tasks/98`) — resume + "Continue Watching"
// ---------------------------------------------------------------------------

/// `GET /api/progress/:file_id` — the saved playback position of one file (`docs/.tasks/98`).
/// The player reads this on mount to resume; a file never played returns `204` (no body), not
/// this shape.
#[derive(Debug, Serialize)]
pub struct ProgressResponse {
    pub position_ms: i64,
    pub duration_ms: i64,
    pub updated_at: i64,
    pub finished: bool,
}

impl From<medi_db::models::Progress> for ProgressResponse {
    fn from(p: medi_db::models::Progress) -> Self {
        Self {
            position_ms: p.position_ms,
            duration_ms: p.duration_ms,
            updated_at: p.updated_at,
            finished: p.finished,
        }
    }
}

/// `PUT /api/progress/:file_id` body (`docs/.tasks/98`) — the throttled write the player sends
/// as it plays (and once more on pause / tab-hide / unmount). `duration_ms` is snapshotted with
/// the position so the resume/Continue-Watching `%` needs no second read.
#[derive(Debug, Deserialize)]
pub struct ProgressWrite {
    pub position_ms: i64,
    pub duration_ms: i64,
}

/// One card in `GET /api/continue-watching` (`docs/.tasks/98`) — an in-progress title with the
/// position to resume from. `kind` is `"movie"` | `"episode"`; `title_id` is the movie/series id
/// the poster links its detail page to, while the whole card links to `/play/:file_id`. `poster`
/// is a ready-to-fetch `/api/images/...` URL (the owning movie's, or the episode's series'), like
/// a [`LibraryItem`].
#[derive(Debug, Serialize)]
pub struct ContinueWatchingItem {
    pub file_id: i64,
    pub kind: String,
    pub title_id: i64,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poster: Option<String>,
    pub position_ms: i64,
    pub duration_ms: i64,
    pub updated_at: i64,
}

impl From<medi_db::models::ContinueItem> for ContinueWatchingItem {
    fn from(c: medi_db::models::ContinueItem) -> Self {
        Self {
            file_id: c.file_id,
            kind: c.kind,
            title_id: c.title_id,
            title: c.title,
            poster: c.poster_path.map(image_url),
            position_ms: c.position_ms,
            duration_ms: c.duration_ms,
            updated_at: c.updated_at,
        }
    }
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
