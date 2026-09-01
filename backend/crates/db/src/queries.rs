//! Typed query functions over the pool.
//!
//! Every function takes a `&Connection` (a pooled checkout) so the caller controls
//! transaction scope, and every call site runs these under
//! `tokio::task::spawn_blocking` — rusqlite is synchronous and must never block the
//! async runtime (`docs/.tasks/01-db-schema.md` §Scaling notes).
//!
//! Lists use **keyset pagination**: instead of `OFFSET n` (which SQLite must walk),
//! callers pass the sort key of the last row they saw and we resume after it using a
//! covering index. See [`MovieCursor`] / [`list_movies`].

use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{
    Credit, Episode, LibraryCard, MediaFile, Movie, MovieDetail, Season, SeasonWithEpisodes,
    Series, SeriesDetail, TrickplayAsset,
};
use crate::{DbError, DbResult};

/// Default page size for list endpoints when the caller does not specify one.
pub const DEFAULT_LIMIT: u32 = 60;
/// Hard cap so a client cannot request an unbounded page.
pub const MAX_LIMIT: u32 = 200;

/// Explicit `media_files` column list, in the order [`MediaFile::from_row`] reads.
/// Shared by every query that hydrates a [`MediaFile`] so the positions stay aligned.
pub const MEDIA_FILE_COLUMNS: &str = "\
    id, movie_id, episode_id, path, container, size_bytes, duration_ms, \
    video_codec, video_profile, width, height, bit_depth, bitrate, \
    transfer_characteristics, color_space, hdr_type, \
    dv_profile, dv_bl_compatible_id, dv_level, hw_decode_unsupported";

/// Column list for the movies/series catalog rows.
const CATALOG_COLUMNS: &str =
    "id, title, sort_title, year, overview, added_at, poster_path, backdrop_path";

fn clamp_limit(limit: u32) -> u32 {
    limit.clamp(1, MAX_LIMIT)
}

// ---------------------------------------------------------------------------
// Movies
// ---------------------------------------------------------------------------

/// Keyset cursor for the alphabetical (`sort_title`) movie list.
///
/// Rows are ordered by `(sort_title, id)` so the pair is unique and monotonic;
/// the cursor carries the last row's pair. The `api` crate serializes this into
/// the opaque base64 `next_cursor` string from `02-api-contract.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovieCursor {
    pub sort_title: String,
    pub id: i64,
}

/// List movies alphabetically by `sort_title`, resuming after `cursor`.
///
/// Pass `None` for the first page. Uses the `idx_movies_sort` index; the trailing
/// `id` tiebreak keeps pagination stable across rows that share a `sort_title`.
pub fn list_movies(
    conn: &Connection,
    cursor: Option<&MovieCursor>,
    limit: u32,
) -> DbResult<Vec<Movie>> {
    let limit = clamp_limit(limit);

    let sql = format!(
        "SELECT {CATALOG_COLUMNS} FROM movies \
         WHERE (?1 IS NULL) OR (sort_title, id) > (?1, ?2) \
         ORDER BY sort_title, id \
         LIMIT ?3"
    );

    let mut stmt = conn.prepare_cached(&sql)?;
    let sort_key = cursor.map(|c| c.sort_title.as_str());
    let last_id = cursor.map(|c| c.id).unwrap_or(0);
    let rows = stmt
        .query_map(params![sort_key, last_id, limit], Movie::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// List movies most-recently-added first (`added_at DESC`), resuming after
/// `(added_at, id)`. Uses `idx_movies_added`.
pub fn list_movies_recent(
    conn: &Connection,
    cursor: Option<(i64, i64)>,
    limit: u32,
) -> DbResult<Vec<Movie>> {
    let limit = clamp_limit(limit);

    // For DESC order the keyset predicate flips to `<`.
    let sql = format!(
        "SELECT {CATALOG_COLUMNS} FROM movies \
         WHERE (?1 IS NULL) OR (added_at, id) < (?1, ?2) \
         ORDER BY added_at DESC, id DESC \
         LIMIT ?3"
    );

    let mut stmt = conn.prepare_cached(&sql)?;
    let last_added = cursor.map(|(a, _)| a);
    let last_id = cursor.map(|(_, i)| i).unwrap_or(i64::MAX);
    let rows = stmt
        .query_map(params![last_added, last_id, limit], Movie::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Fetch a single movie by id, or [`DbError::NotFound`].
pub fn get_movie(conn: &Connection, id: i64) -> DbResult<Movie> {
    let sql = format!("SELECT {CATALOG_COLUMNS} FROM movies WHERE id = ?1");
    let mut stmt = conn.prepare_cached(&sql)?;
    stmt.query_row(params![id], Movie::from_row)
        .optional()?
        .ok_or(DbError::NotFound)
}

/// Fetch full movie detail (movie + media files + billed credits).
pub fn get_movie_detail(conn: &Connection, id: i64) -> DbResult<MovieDetail> {
    let movie = get_movie(conn, id)?;
    let media_files = media_files_for_movie(conn, id)?;
    let credits = credits_for_movie(conn, id)?;
    Ok(MovieDetail {
        movie,
        media_files,
        credits,
    })
}

// ---------------------------------------------------------------------------
// Unified library (movies + series) — backs GET /api/library
// ---------------------------------------------------------------------------

/// How the unified `/api/library` grid is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibrarySort {
    /// Alphabetical by `sort_title` (ascending). The default browse order.
    SortTitle,
    /// Most-recently-added first (`added_at` descending).
    AddedAt,
}

/// Keyset cursor for [`list_library`]. Carries the last row's ordering key so the
/// next page resumes immediately after it — no `OFFSET`.
///
/// The tuple `(sort_value, kind_tag, id)` is globally unique across the movies and
/// series tables (two titles can share a `sort_title`/`added_at`, and a movie and a
/// series can share an `id`, so both `kind_tag` and `id` are needed as tiebreaks).
/// The `api` crate serializes this to/from the opaque base64 `next_cursor` string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryCursor {
    /// The `sort_title` (for [`LibrarySort::SortTitle`]) or `added_at` as text
    /// (for [`LibrarySort::AddedAt`]) of the last row on the previous page.
    pub sort_value: String,
    /// 0 = movie, 1 = series — matches the `LibraryCard` discriminator.
    pub kind_tag: i64,
    pub id: i64,
}

/// The `(kind_tag, id, title, sort_title, year, added_at, poster_path, hdr)` select
/// list for each side of the library `UNION ALL`.
///
/// `hdr` is a correlated subquery over the title's media files that surfaces the
/// strongest HDR tier present (Dolby Vision beats HDR10+/HDR10/HLG). It is index-light
/// (each title has a handful of files) and lets a poster tile badge its format without
/// a second round trip.
fn library_select(kind_tag: i64, table: &str, join_col: &str) -> String {
    // Rank HDR strings so MAX() picks the strongest; map back to the label after.
    // NULL/SDR sort lowest and yield a NULL badge.
    format!(
        "SELECT {kind_tag} AS kind_tag, t.id, t.title, t.sort_title, t.year, t.added_at, \
                t.poster_path, \
                ( SELECT CASE MAX( CASE mf.hdr_type \
                        WHEN 'dolbyvision' THEN 4 WHEN 'hdr10plus' THEN 3 \
                        WHEN 'hdr10' THEN 2 WHEN 'hlg' THEN 1 ELSE 0 END ) \
                    WHEN 4 THEN 'dolbyvision' WHEN 3 THEN 'hdr10plus' \
                    WHEN 2 THEN 'hdr10' WHEN 1 THEN 'hlg' ELSE NULL END \
                  FROM media_files mf WHERE {join_col} ) AS hdr \
         FROM {table} t"
    )
}

/// The movies side joins `media_files` directly; the series side must reach files
/// through `seasons` → `episodes`, so its correlated `hdr` subquery uses a different
/// predicate. This returns the two `SELECT`s already carrying the right join.
fn library_union_sql() -> String {
    // Movies: media_files.movie_id == the movie row.
    let movies = library_select(0, "movies", "mf.movie_id = t.id");
    // Series: episode → season → this series.
    let series = library_select(
        1,
        "series",
        "mf.episode_id IN ( SELECT e.id FROM episodes e \
            JOIN seasons s ON s.id = e.season_id WHERE s.series_id = t.id )",
    );
    format!("{movies} UNION ALL {series}")
}

/// List the unified catalog (movies + series cards), ordered per `sort`, resuming
/// after `cursor`. Pass `None` for the first page. Backs `GET /api/library`.
///
/// Both orderings append `(kind_tag, id)` as tiebreaks so the cursor is unambiguous
/// even when titles share a `sort_title` or `added_at`.
pub fn list_library(
    conn: &Connection,
    sort: LibrarySort,
    cursor: Option<&LibraryCursor>,
    limit: u32,
) -> DbResult<Vec<LibraryCard>> {
    let limit = clamp_limit(limit);
    let union = library_union_sql();

    // The keyset predicate and ORDER BY differ by sort column and direction.
    // `?1` is the cursor sort_value (NULL on the first page), `?2` kind_tag, `?3` id.
    let (predicate, order) = match sort {
        LibrarySort::SortTitle => (
            "(?1 IS NULL) OR (sort_title, kind_tag, id) > (?1, ?2, ?3)",
            "sort_title ASC, kind_tag ASC, id ASC",
        ),
        LibrarySort::AddedAt => (
            // added_at compared as text is wrong; bind the numeric value via CAST.
            "(?1 IS NULL) OR (added_at, kind_tag, id) < (CAST(?1 AS INTEGER), ?2, ?3)",
            "added_at DESC, kind_tag DESC, id DESC",
        ),
    };

    let sql = format!(
        "SELECT kind_tag, id, title, sort_title, year, added_at, poster_path, hdr \
         FROM ( {union} ) \
         WHERE {predicate} \
         ORDER BY {order} \
         LIMIT ?4"
    );

    let mut stmt = conn.prepare_cached(&sql)?;
    let sort_value = cursor.map(|c| c.sort_value.as_str());
    let kind_tag = cursor.map(|c| c.kind_tag).unwrap_or(0);
    let id = cursor.map(|c| c.id).unwrap_or(0);
    let rows = stmt
        .query_map(
            params![sort_value, kind_tag, id, limit],
            LibraryCard::from_row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Series / seasons / episodes
// ---------------------------------------------------------------------------

/// List series alphabetically by `sort_title`, resuming after `cursor`.
/// Uses `idx_series_sort`.
pub fn list_series(
    conn: &Connection,
    cursor: Option<&MovieCursor>,
    limit: u32,
) -> DbResult<Vec<Series>> {
    let limit = clamp_limit(limit);

    let sql = format!(
        "SELECT {CATALOG_COLUMNS} FROM series \
         WHERE (?1 IS NULL) OR (sort_title, id) > (?1, ?2) \
         ORDER BY sort_title, id \
         LIMIT ?3"
    );

    let mut stmt = conn.prepare_cached(&sql)?;
    let sort_key = cursor.map(|c| c.sort_title.as_str());
    let last_id = cursor.map(|c| c.id).unwrap_or(0);
    let rows = stmt
        .query_map(params![sort_key, last_id, limit], Series::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Fetch a single series by id, or [`DbError::NotFound`].
pub fn get_series(conn: &Connection, id: i64) -> DbResult<Series> {
    let sql = format!("SELECT {CATALOG_COLUMNS} FROM series WHERE id = ?1");
    let mut stmt = conn.prepare_cached(&sql)?;
    stmt.query_row(params![id], Series::from_row)
        .optional()?
        .ok_or(DbError::NotFound)
}

/// Fetch full series detail: series + its seasons (each with ordered episodes) +
/// billed credits. Assembled with a couple of ordered scans rather than one wide
/// join, which keeps the row → struct mapping simple and the queries index-friendly.
pub fn get_series_detail(conn: &Connection, id: i64) -> DbResult<SeriesDetail> {
    let series = get_series(conn, id)?;

    let seasons = {
        let mut stmt = conn.prepare_cached(
            "SELECT id, series_id, season_number FROM seasons \
             WHERE series_id = ?1 ORDER BY season_number",
        )?;
        let rows = stmt
            .query_map(params![id], Season::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };

    let mut seasons_with_eps = Vec::with_capacity(seasons.len());
    for season in seasons {
        let episodes = episodes_for_season(conn, season.id)?;
        seasons_with_eps.push(SeasonWithEpisodes { season, episodes });
    }

    let credits = credits_for_series(conn, id)?;

    Ok(SeriesDetail {
        series,
        seasons: seasons_with_eps,
        credits,
    })
}

/// Episodes of a season, ordered by `episode_number`. Uses `idx_episodes_season`.
pub fn episodes_for_season(conn: &Connection, season_id: i64) -> DbResult<Vec<Episode>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, season_id, episode_number, title, overview FROM episodes \
         WHERE season_id = ?1 ORDER BY episode_number",
    )?;
    let rows = stmt
        .query_map(params![season_id], Episode::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Media files
// ---------------------------------------------------------------------------

/// All media files attached to a movie. Uses `idx_files_movie`.
pub fn media_files_for_movie(conn: &Connection, movie_id: i64) -> DbResult<Vec<MediaFile>> {
    let sql = format!("SELECT {MEDIA_FILE_COLUMNS} FROM media_files WHERE movie_id = ?1 ORDER BY id");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt
        .query_map(params![movie_id], MediaFile::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// All media files attached to an episode. Uses `idx_files_episode`.
pub fn media_files_for_episode(conn: &Connection, episode_id: i64) -> DbResult<Vec<MediaFile>> {
    let sql =
        format!("SELECT {MEDIA_FILE_COLUMNS} FROM media_files WHERE episode_id = ?1 ORDER BY id");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt
        .query_map(params![episode_id], MediaFile::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Fetch a single media file by id, or [`DbError::NotFound`].
///
/// This is the row `GET /api/stream/:file_id` reads to make the transcode
/// decision; call [`MediaFile::profile`] on the result.
pub fn get_media_file(conn: &Connection, id: i64) -> DbResult<MediaFile> {
    let sql = format!("SELECT {MEDIA_FILE_COLUMNS} FROM media_files WHERE id = ?1");
    let mut stmt = conn.prepare_cached(&sql)?;
    stmt.query_row(params![id], MediaFile::from_row)
        .optional()?
        .ok_or(DbError::NotFound)
}

/// Explicit `trickplay_assets` column list, in the order [`TrickplayAsset::from_row`]
/// reads. Kept adjacent to the read so the positions stay aligned.
const TRICKPLAY_COLUMNS: &str =
    "media_file_id, kind, path, interval_ms, tile_w, tile_h, cols, rows, generated_at";

/// Fetch the `trickplay_assets` row for a file, or [`DbError::NotFound`] when the
/// sprite has not been generated yet.
///
/// This backs `GET /api/trickplay/:file_id/meta` — the client needs the grid geometry
/// (`tile_w/h`, `cols`, `rows`) and `interval_ms` to crop the correct cell out of a
/// tiled-JPG mosaic while scrubbing (`docs/.tasks/50` Part A). The sprite image itself
/// is served separately as a static file by the `/api/trickplay/:file` route.
pub fn get_trickplay_asset(conn: &Connection, media_file_id: i64) -> DbResult<TrickplayAsset> {
    let sql =
        format!("SELECT {TRICKPLAY_COLUMNS} FROM trickplay_assets WHERE media_file_id = ?1");
    let mut stmt = conn.prepare_cached(&sql)?;
    stmt.query_row(params![media_file_id], TrickplayAsset::from_row)
        .optional()?
        .ok_or(DbError::NotFound)
}

// ---------------------------------------------------------------------------
// Asset generation (Phase 3, `medi-assets`) — pick the next un-done file
// ---------------------------------------------------------------------------

/// A probed media file still missing one or both generated assets. Returned by
/// [`list_pending_assets`] for the off-peak worker to process. Carries only what the
/// preview/trickplay commands need: the file id, its source path, its duration (to pick
/// the preview mid-point and the sprite count), and which assets are already done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAsset {
    pub media_file_id: i64,
    pub path: String,
    pub duration_ms: Option<i64>,
    /// True when the 720p hover preview already exists (skip preview generation).
    pub preview_done: bool,
    /// True when the trickplay sprites already exist (skip trickplay generation).
    pub trickplay_done: bool,
}

/// List probed files that still need at least one generated asset, oldest-added first
/// so a first-run backfill processes the existing library before newer additions
/// (`docs/.tasks/30` §Scaling notes). A file qualifies when it has been ffprobed
/// (`scan_state.probed_at` set) but its `preview_done_at` **or** `trickplay_done_at` is
/// still NULL.
///
/// Joins `media_files` ↔ `scan_state` on the file path (`scan_state` is keyed by path,
/// `media_files` carries the same unique path). `limit` bounds the batch a single
/// worker tick pulls so progress is checkpointed to the DB between batches (restart
/// resumes rather than restarts).
pub fn list_pending_assets(conn: &Connection, limit: u32) -> DbResult<Vec<PendingAsset>> {
    let limit = clamp_limit(limit);
    // added_at lives on the owning movie/episode → we order by the file id as a stable,
    // monotonic proxy for insertion order (ids are assigned in ingest order), which is
    // index-friendly (PRIMARY KEY) and gives the "oldest first" backfill order.
    let mut stmt = conn.prepare_cached(
        "SELECT mf.id, mf.path, mf.duration_ms, \
                ss.preview_done_at IS NOT NULL, ss.trickplay_done_at IS NOT NULL \
         FROM media_files mf \
         JOIN scan_state ss ON ss.path = mf.path \
         WHERE ss.probed_at IS NOT NULL \
           AND (ss.preview_done_at IS NULL OR ss.trickplay_done_at IS NULL) \
         ORDER BY mf.id \
         LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![limit], |r| {
            Ok(PendingAsset {
                media_file_id: r.get(0)?,
                path: r.get(1)?,
                duration_ms: r.get(2)?,
                preview_done: r.get::<_, i64>(3)? != 0,
                trickplay_done: r.get::<_, i64>(4)? != 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Metadata enrichment (`docs/.tasks/60` Phase A) — pending-title selection
// ---------------------------------------------------------------------------

/// A title awaiting metadata enrichment: its id and the parsed `(title, year)` the
/// provider search keys on. Returned by [`list_pending_metadata`] for the enrichment
/// worker's first-run backfill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingTitle {
    pub id: i64,
    pub title: String,
    pub year: Option<i64>,
}

/// List movies (or series) still needing enrichment — `metadata_state` in
/// (`pending`, `failed`) — oldest-added first so a first-run backfill processes the
/// existing library in insertion order. `limit` bounds the batch a worker tick pulls.
///
/// `matched` and `unmatched` rows are intentionally excluded: a matched row is done
/// (idempotent — no re-fetch without an explicit refresh) and an unmatched row had no
/// good candidate and should not be retried automatically.
pub fn list_pending_metadata(
    conn: &Connection,
    kind: crate::writes::TitleKind,
    limit: u32,
) -> DbResult<Vec<PendingTitle>> {
    let limit = clamp_limit(limit);
    let table = match kind {
        crate::writes::TitleKind::Movie => "movies",
        crate::writes::TitleKind::Series => "series",
    };
    let sql = format!(
        "SELECT id, title, year FROM {table} \
         WHERE metadata_state IN ('pending', 'failed') \
         ORDER BY added_at, id \
         LIMIT ?1"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt
        .query_map(params![limit], |r| {
            Ok(PendingTitle {
                id: r.get(0)?,
                title: r.get(1)?,
                year: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// All ids currently present in the `movies` (or `series`) table. Used by the artwork
/// orphan sweep (`docs/.tasks/60` §Orphan reaping) to reconcile `/config/images`
/// against surviving titles: any `images/<kind>/<id>/` dir whose id is not in this set
/// is orphaned and reclaimable.
pub fn all_title_ids(conn: &Connection, kind: crate::writes::TitleKind) -> DbResult<Vec<i64>> {
    let table = match kind {
        crate::writes::TitleKind::Movie => "movies",
        crate::writes::TitleKind::Series => "series",
    };
    let mut stmt = conn.prepare_cached(&format!("SELECT id FROM {table}"))?;
    let rows = stmt
        .query_map([], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Fetch just the `(title, year)` of a movie for an enrichment search, or
/// [`DbError::NotFound`]. A lighter read than [`get_movie`] for the worker's hot path.
pub fn get_title_year(
    conn: &Connection,
    kind: crate::writes::TitleKind,
    id: i64,
) -> DbResult<(String, Option<i64>)> {
    let table = match kind {
        crate::writes::TitleKind::Movie => "movies",
        crate::writes::TitleKind::Series => "series",
    };
    let sql = format!("SELECT title, year FROM {table} WHERE id = ?1");
    conn.prepare_cached(&sql)?
        .query_row(params![id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .ok_or(DbError::NotFound)
}

// ---------------------------------------------------------------------------
// Libraries (`docs/.tasks/60` Phase B)
// ---------------------------------------------------------------------------

/// A scan root derived from a library folder: the folder path plus the library it
/// belongs to and that library's kind. The scanner loops over these instead of a single
/// `media_dir`, tagging each discovered file with its `library_id`, and the library
/// `kind` overrides filename guessing (a stray `SxxEyy` in a Movies library stays a
/// movie).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryRoot {
    pub library_id: i64,
    /// `crate::writes::TitleKind` as a string (`"movie"`/`"series"`).
    pub kind: String,
    pub path: String,
}

/// Every (library, folder) pair, as scan roots. Empty when no libraries are defined
/// (before auto-seed). Ordered by library id for stable iteration.
pub fn library_roots(conn: &Connection) -> DbResult<Vec<LibraryRoot>> {
    let mut stmt = conn.prepare_cached(
        "SELECT l.id, l.kind, f.path \
         FROM libraries l JOIN library_folders f ON f.library_id = l.id \
         ORDER BY l.id, f.id",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(LibraryRoot {
                library_id: r.get(0)?,
                kind: r.get(1)?,
                path: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// List all libraries, each with its folder paths. Backs `GET /api/libraries`.
pub fn list_libraries(conn: &Connection) -> DbResult<Vec<crate::models::LibraryWithFolders>> {
    let libraries = {
        let mut stmt =
            conn.prepare_cached("SELECT id, name, kind, created_at FROM libraries ORDER BY id")?;
        let rows = stmt
            .query_map([], crate::models::Library::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let mut out = Vec::with_capacity(libraries.len());
    for library in libraries {
        let folders = folders_for_library(conn, library.id)?;
        out.push(crate::models::LibraryWithFolders { library, folders });
    }
    Ok(out)
}

/// Fetch one library with its folders, or [`DbError::NotFound`].
pub fn get_library(conn: &Connection, id: i64) -> DbResult<crate::models::LibraryWithFolders> {
    let library = conn
        .prepare_cached("SELECT id, name, kind, created_at FROM libraries WHERE id = ?1")?
        .query_row(params![id], crate::models::Library::from_row)
        .optional()?
        .ok_or(DbError::NotFound)?;
    let folders = folders_for_library(conn, id)?;
    Ok(crate::models::LibraryWithFolders { library, folders })
}

/// The folder paths of one library, ordered by id.
pub fn folders_for_library(conn: &Connection, library_id: i64) -> DbResult<Vec<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT path FROM library_folders WHERE library_id = ?1 ORDER BY id",
    )?;
    let rows = stmt
        .query_map(params![library_id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Credits
// ---------------------------------------------------------------------------

/// Billed credits for a movie, joined to `people`, ordered by billing (`ord`).
/// Uses `idx_credits_movie`.
pub fn credits_for_movie(conn: &Connection, movie_id: i64) -> DbResult<Vec<Credit>> {
    let mut stmt = conn.prepare_cached(
        "SELECT c.id, c.person_id, p.name, c.role, c.character, c.ord \
         FROM credits c JOIN people p ON p.id = c.person_id \
         WHERE c.movie_id = ?1 \
         ORDER BY c.ord",
    )?;
    let rows = stmt
        .query_map(params![movie_id], Credit::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Billed credits for a series, joined to `people`, ordered by billing (`ord`).
/// Uses `idx_credits_series`.
pub fn credits_for_series(conn: &Connection, series_id: i64) -> DbResult<Vec<Credit>> {
    let mut stmt = conn.prepare_cached(
        "SELECT c.id, c.person_id, p.name, c.role, c.character, c.ord \
         FROM credits c JOIN people p ON p.id = c.person_id \
         WHERE c.series_id = ?1 \
         ORDER BY c.ord",
    )?;
    let rows = stmt
        .query_map(params![series_id], Credit::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
