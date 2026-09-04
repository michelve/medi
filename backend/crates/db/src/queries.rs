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
    AudioStream, Chapter, Credit, Episode, EpisodeWithFiles, LibraryCard, MediaFile, Movie,
    MovieDetail, Season, SeasonWithEpisodes, Series, SeriesDetail, SubtitleStream, TrickplayAsset,
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
    dv_profile, dv_bl_compatible_id, dv_level, hw_decode_unsupported, frame_rate";

/// Column list for the series catalog rows (also the shared head of the movie list).
const CATALOG_COLUMNS: &str =
    "id, title, sort_title, year, overview, added_at, poster_path, backdrop_path";

/// Column list for the movies catalog rows — the shared [`CATALOG_COLUMNS`] plus the
/// movie-only fanart art columns `logo_path` (Task 93) + `wallpaper_path` (Task 95). Kept
/// positionally aligned with [`Movie::from_row`].
const MOVIE_COLUMNS: &str =
    "id, title, sort_title, year, overview, added_at, poster_path, backdrop_path, \
     logo_path, wallpaper_path";

/// A SQL `CASE` that ranks an `hdr_type` column so the strongest HDR format sorts highest:
/// Dolby Vision (4) > HDR10+ (3) > HDR10 (2) > HLG (1) > SDR/NULL (0). `col` is the
/// (possibly table-qualified) column, e.g. `"hdr_type"` or `"mf.hdr_type"`. Shared by the
/// library poster-badge `MAX()` and the detail "best file" ordering so the ranking lives in
/// one place.
fn hdr_rank_case(col: &str) -> String {
    format!(
        "CASE {col} \
            WHEN 'dolbyvision' THEN 4 WHEN 'hdr10plus' THEN 3 \
            WHEN 'hdr10' THEN 2 WHEN 'hlg' THEN 1 ELSE 0 END"
    )
}

/// The best-first `ORDER BY` for a title's media files: highest resolution, then strongest
/// HDR tier, then highest bitrate, with `id` as a stable final tiebreak. Unprobed files
/// (NULL height/bitrate) sort last. So `media_files[0]` is always the best copy to play.
fn media_file_best_order() -> String {
    format!(
        "ORDER BY height DESC, {} DESC, bitrate DESC, id",
        hdr_rank_case("hdr_type")
    )
}

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
        "SELECT {MOVIE_COLUMNS} FROM movies \
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
        "SELECT {MOVIE_COLUMNS} FROM movies \
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
    let sql = format!("SELECT {MOVIE_COLUMNS} FROM movies WHERE id = ?1");
    let mut stmt = conn.prepare_cached(&sql)?;
    stmt.query_row(params![id], Movie::from_row)
        .optional()?
        .ok_or(DbError::NotFound)
}

/// Fetch full movie detail (movie + media files + billed credits + trailers + collection).
pub fn get_movie_detail(conn: &Connection, id: i64) -> DbResult<MovieDetail> {
    let movie = get_movie(conn, id)?;
    let media_files = media_files_for_movie(conn, id)?;
    let credits = credits_for_movie(conn, id)?;
    let trailers = movie_trailers(conn, id)?;
    let collection = movie_collection(conn, id)?;
    let genres = movie_genres(conn, id)?;
    Ok(MovieDetail {
        movie,
        media_files,
        credits,
        trailers,
        collection,
        genres,
    })
}

/// A movie's genres, in name order. Empty when the movie is unmatched (Task 91). Backs the
/// detail-page metadata line (genre · runtime · year).
pub fn movie_genres(conn: &Connection, movie_id: i64) -> DbResult<Vec<crate::models::Genre>> {
    let mut stmt = conn.prepare_cached(
        "SELECT g.id, g.name FROM genres g \
         JOIN movie_genres mg ON mg.genre_id = g.id \
         WHERE mg.movie_id = ?1 ORDER BY g.name",
    )?;
    let rows = stmt
        .query_map(params![movie_id], crate::models::Genre::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// A movie's YouTube trailers, best-first (by stored `ord`). Empty when it has none
/// (Task 91 detail extensions).
pub fn movie_trailers(conn: &Connection, movie_id: i64) -> DbResult<Vec<crate::models::Trailer>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, youtube_key, name, kind FROM trailers WHERE movie_id = ?1 ORDER BY ord, id",
    )?;
    let rows = stmt
        .query_map(params![movie_id], crate::models::Trailer::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The collection (franchise) a movie belongs to, or `None` (Task 91 detail extensions).
pub fn movie_collection(conn: &Connection, movie_id: i64) -> DbResult<Option<crate::models::Collection>> {
    conn.prepare_cached(
        "SELECT col.id, col.name, col.poster_path \
         FROM movies m JOIN collections col ON col.id = m.collection_id \
         WHERE m.id = ?1",
    )?
    .query_row(params![movie_id], crate::models::Collection::from_row)
    .optional()
    .map_err(Into::into)
}

/// The **other** in-library movies of a collection (the given movie excluded), newest-added
/// first, as [`LibraryCard`]s (Task 91 detail extensions — the "Collection" row). Empty when
/// the movie is standalone or the only one of its franchise in the library.
pub fn collection_movies(conn: &Connection, collection_id: i64, exclude_movie_id: i64) -> DbResult<Vec<LibraryCard>> {
    // Reuse the movies side of the library card select, filtered to the collection.
    let select = library_select(0, "movies", "mf.movie_id = t.id");
    let sql = format!(
        "SELECT kind_tag, id, title, sort_title, year, added_at, poster_path, hdr \
         FROM ( {select} WHERE t.collection_id = ?1 AND t.id != ?2 ) \
         ORDER BY added_at DESC, id DESC"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt
        .query_map(params![collection_id, exclude_movie_id], LibraryCard::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
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
    // Rank HDR strings so MAX() picks the strongest; map the rank back to the label after.
    // NULL/SDR sort lowest and yield a NULL badge. The rank `CASE` is shared with the
    // detail "best file" ordering via `hdr_rank_case`.
    let rank = hdr_rank_case("mf.hdr_type");
    format!(
        "SELECT {kind_tag} AS kind_tag, t.id, t.title, t.sort_title, t.year, t.added_at, \
                t.poster_path, \
                ( SELECT CASE MAX( {rank} ) \
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
// Genres (`docs/.tasks/91` Phase A) — category list + per-genre keyset grid
// ---------------------------------------------------------------------------

/// List genres carrying at least one title, with `count = movies + series`, ordered by
/// count desc then name (`docs/.tasks/91`). Backs `GET /api/genres` — the browse rows and
/// the genre chips. A genre no title references (a stale row after a re-match dropped its
/// last user) is excluded by the `HAVING count > 0`, so the list is always non-empty rows.
pub fn list_genres(conn: &Connection) -> DbResult<Vec<crate::models::GenreCount>> {
    // Sum the per-kind join counts per genre. LEFT JOINs keep a genre with titles on only
    // one side; the correlated COUNTs avoid a fan-out cartesian product across the two joins.
    let mut stmt = conn.prepare_cached(
        "SELECT g.id, g.name, \
                ( (SELECT COUNT(*) FROM movie_genres mg WHERE mg.genre_id = g.id) \
                + (SELECT COUNT(*) FROM series_genres sg WHERE sg.genre_id = g.id) ) AS cnt \
         FROM genres g \
         WHERE cnt > 0 \
         ORDER BY cnt DESC, g.name ASC",
    )?;
    let rows = stmt
        .query_map([], crate::models::GenreCount::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The display name of a genre by id, or [`DbError::NotFound`]. Used by the genre-grid
/// handler to render the page header without a second round trip.
pub fn genre_name(conn: &Connection, genre_id: i64) -> DbResult<String> {
    conn.prepare_cached("SELECT name FROM genres WHERE id = ?1")?
        .query_row(params![genre_id], |r| r.get::<_, String>(0))
        .optional()?
        .ok_or(DbError::NotFound)
}

/// List the titles carrying one genre, ordered per `sort`, resuming after `cursor`
/// (`docs/.tasks/91`). **Identical page shape to [`list_library`]** — same `LibraryCard`,
/// same [`LibraryCursor`] codec — so the API reuses `LibraryPage`/`LibraryItem` and the
/// client's paging hook verbatim; the only difference is the `EXISTS` genre filter added
/// to each side of the union.
pub fn list_by_genre(
    conn: &Connection,
    genre_id: i64,
    sort: LibrarySort,
    cursor: Option<&LibraryCursor>,
    limit: u32,
) -> DbResult<Vec<LibraryCard>> {
    let limit = clamp_limit(limit);
    let union = library_union_by_genre_sql();

    let (predicate, order) = match sort {
        LibrarySort::SortTitle => (
            "(?2 IS NULL) OR (sort_title, kind_tag, id) > (?2, ?3, ?4)",
            "sort_title ASC, kind_tag ASC, id ASC",
        ),
        LibrarySort::AddedAt => (
            "(?2 IS NULL) OR (added_at, kind_tag, id) < (CAST(?2 AS INTEGER), ?3, ?4)",
            "added_at DESC, kind_tag DESC, id DESC",
        ),
    };

    // `?1` is the genre id (used by both sides of the union's EXISTS filter); the cursor
    // params shift to `?2..?4` and the limit to `?5`.
    let sql = format!(
        "SELECT kind_tag, id, title, sort_title, year, added_at, poster_path, hdr \
         FROM ( {union} ) \
         WHERE {predicate} \
         ORDER BY {order} \
         LIMIT ?5"
    );

    let mut stmt = conn.prepare_cached(&sql)?;
    let sort_value = cursor.map(|c| c.sort_value.as_str());
    let kind_tag = cursor.map(|c| c.kind_tag).unwrap_or(0);
    let id = cursor.map(|c| c.id).unwrap_or(0);
    let rows = stmt
        .query_map(
            params![genre_id, sort_value, kind_tag, id, limit],
            LibraryCard::from_row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The library `UNION ALL` restricted to titles in genre `?1`. Same two-side shape as
/// [`library_union_sql`] with an `EXISTS` over the per-kind genre join table added to each
/// side, so the outer keyset predicate/order in [`list_by_genre`] is unchanged.
fn library_union_by_genre_sql() -> String {
    let movies = format!(
        "{} WHERE EXISTS (SELECT 1 FROM movie_genres mg \
             WHERE mg.movie_id = t.id AND mg.genre_id = ?1)",
        library_select(0, "movies", "mf.movie_id = t.id")
    );
    let series = format!(
        "{} WHERE EXISTS (SELECT 1 FROM series_genres sg \
             WHERE sg.series_id = t.id AND sg.genre_id = ?1)",
        library_select(
            1,
            "series",
            "mf.episode_id IN ( SELECT e.id FROM episodes e \
                JOIN seasons s ON s.id = e.season_id WHERE s.series_id = t.id )",
        )
    );
    format!("{movies} UNION ALL {series}")
}

/// List title ids of one kind that are `matched` but have **no** genre rows yet — the
/// backfill worklist (`docs/.tasks/91` §Backfill). With `force`, list all `matched` titles
/// so a forced backfill re-fetches genres for everything. Oldest-added first so a backfill
/// processes the existing library in insertion order; `limit` bounds the batch.
pub fn matched_titles_missing_genres(
    conn: &Connection,
    kind: crate::writes::TitleKind,
    force: bool,
    limit: u32,
) -> DbResult<Vec<i64>> {
    let limit = clamp_limit(limit);
    let (table, join, col) = match kind {
        crate::writes::TitleKind::Movie => ("movies", "movie_genres", "movie_id"),
        crate::writes::TitleKind::Series => ("series", "series_genres", "series_id"),
    };
    // A matched title with no join row is missing genres. `force` drops that filter so
    // every matched title is re-fetched (a re-match replaces its genres wholesale).
    let missing = if force {
        String::new()
    } else {
        format!("AND NOT EXISTS (SELECT 1 FROM {join} j WHERE j.{col} = t.id) ")
    };
    let sql = format!(
        "SELECT t.id FROM {table} t \
         WHERE t.metadata_state = 'matched' {missing}\
         ORDER BY t.added_at, t.id \
         LIMIT ?1"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt
        .query_map(params![limit], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// One page of matched movies that have **no collection** yet, for the collection backfill
/// (`docs/.tasks/91`). Returns `(id, added_at)` pairs ordered by `(added_at, id)`, resuming
/// strictly after `cursor` (pass `None` for the first page).
///
/// Unlike [`matched_titles_missing_genres`], this **cannot** filter on "still missing" to
/// page: a standalone movie legitimately keeps `collection_id = NULL` after enrichment, so a
/// missing-filter worklist would return the same rows forever. Keyset paging on `(added_at,
/// id)` advances past processed rows, so the backfill loop terminates once a short page comes
/// back. The caller feeds the last row's `(added_at, id)` back as the next `cursor`.
pub fn matched_movies_missing_collection(
    conn: &Connection,
    cursor: Option<(i64, i64)>,
    limit: u32,
) -> DbResult<Vec<(i64, i64)>> {
    let limit = clamp_limit(limit);
    // `?1`/`?2` are the cursor (added_at, id); `?3` is NULL on the first page to disable the
    // keyset predicate. `?4` is the limit.
    let sql = "SELECT id, added_at FROM movies \
         WHERE metadata_state = 'matched' AND collection_id IS NULL \
           AND ( ?3 IS NULL OR (added_at, id) > (?1, ?2) ) \
         ORDER BY added_at, id \
         LIMIT ?4";
    let (c_added, c_id) = cursor.unwrap_or((0, 0));
    let has_cursor: Option<i64> = cursor.map(|_| 1);
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt
        .query_map(params![c_added, c_id, has_cursor, limit], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Matched movies still lacking fanart art — a NULL `logo_path` **or** a NULL
/// `wallpaper_path` — for the fanart.tv backfill (Task 93 logos + Task 95 wallpapers). Both
/// art types come from the *same* `/v3/movies/{id}` response in one `enrich_with_id` pass, so
/// one worklist covers both: a movie missing either needs the (single) fanart fetch. Oldest-
/// added first, `limit`-bounded — mirrors [`matched_titles_missing_genres`] (movies only;
/// series art is out of scope).
///
/// Unlike the collection worklist, this **can** filter on "still missing". `force` (a manual
/// full re-backfill) drops the filter and returns every matched movie; a plain backfill
/// relies on the "still NULL" filter shrinking the worklist as art lands. A movie fanart has
/// no art for keeps its columns NULL, so it stays on the list — the backfill caller caps a
/// non-force run to a single batch (like the collection pass) so an all-artless batch can't
/// spin forever; the next backfill run picks up where it left off.
pub fn matched_movies_missing_fanart(
    conn: &Connection,
    force: bool,
    limit: u32,
) -> DbResult<Vec<i64>> {
    let limit = clamp_limit(limit);
    let missing = if force {
        ""
    } else {
        "AND (logo_path IS NULL OR wallpaper_path IS NULL) "
    };
    let sql = format!(
        "SELECT id FROM movies \
         WHERE metadata_state = 'matched' {missing}\
         ORDER BY added_at, id \
         LIMIT ?1"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt
        .query_map(params![limit], |r| r.get::<_, i64>(0))?
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
        // Hydrate each episode with its media files so clients can play it directly
        // (Task 82): the primary file's id is the `file_id` for `GET /api/stream`.
        let episodes = episodes_for_season(conn, season.id)?
            .into_iter()
            .map(|episode| {
                let media_files = media_files_for_episode(conn, episode.id)?;
                Ok(EpisodeWithFiles {
                    episode,
                    media_files,
                })
            })
            .collect::<DbResult<Vec<_>>>()?;
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

/// Explicit `audio_streams` column list, in the order [`AudioStream::from_row`] reads.
const AUDIO_STREAM_COLUMNS: &str = "\
    id, media_file_id, stream_index, codec, profile, channels, channel_layout, \
    bitrate, sample_rate, language, title, immersive, is_default";

/// All audio tracks of a media file, ordered by `stream_index` (Task 70). Empty when the
/// file has not been probed (or was probed before Task 70). Uses `idx_audio_streams_file`.
pub fn get_audio_streams(conn: &Connection, media_file_id: i64) -> DbResult<Vec<AudioStream>> {
    let sql = format!(
        "SELECT {AUDIO_STREAM_COLUMNS} FROM audio_streams \
         WHERE media_file_id = ?1 ORDER BY stream_index"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt
        .query_map(params![media_file_id], AudioStream::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Explicit `subtitle_streams` column list, in the order [`SubtitleStream::from_row`]
/// reads (Task 90). Kept adjacent to the read so the positions stay aligned.
const SUBTITLE_STREAM_COLUMNS: &str = "\
    id, media_file_id, stream_index, codec, format, language, title, \
    is_default, is_forced, is_external, external_path";

/// All subtitle tracks of a media file (Task 90) — embedded tracks + external sidecars.
/// Ordered by `stream_index` (embedded first, in ffprobe order; externals — with a NULL
/// index — sort last, then by id). Empty when the file has no subtitles or is unprobed.
/// Uses `idx_subtitle_streams_file`.
pub fn get_subtitle_streams(
    conn: &Connection,
    media_file_id: i64,
) -> DbResult<Vec<SubtitleStream>> {
    let sql = format!(
        "SELECT {SUBTITLE_STREAM_COLUMNS} FROM subtitle_streams \
         WHERE media_file_id = ?1 ORDER BY stream_index IS NULL, stream_index, id"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt
        .query_map(params![media_file_id], SubtitleStream::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Explicit `chapters` column list, in the order [`Chapter::from_row`] reads (Task 99).
const CHAPTER_COLUMNS: &str = "id, media_file_id, ordinal, start_ms, end_ms, title";

/// All chapter markers of a media file, ordered by `ordinal` (Task 99). Empty when the file
/// has no chapters or was probed before Task 99. Uses `idx_chapters_file`.
pub fn chapters_for(conn: &Connection, media_file_id: i64) -> DbResult<Vec<Chapter>> {
    let sql = format!(
        "SELECT {CHAPTER_COLUMNS} FROM chapters WHERE media_file_id = ?1 ORDER BY ordinal"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt
        .query_map(params![media_file_id], Chapter::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Attach each file's `audio_streams`, `subtitle_streams`, and `chapters` child rows
/// (Tasks 70 / 90 / 99). Kept in one place so every path that hydrates a `MediaFile` returns
/// all child lists.
fn attach_audio_streams(conn: &Connection, mut files: Vec<MediaFile>) -> DbResult<Vec<MediaFile>> {
    for f in &mut files {
        f.audio_streams = get_audio_streams(conn, f.id)?;
        f.subtitle_streams = get_subtitle_streams(conn, f.id)?;
        f.chapters = chapters_for(conn, f.id)?;
    }
    Ok(files)
}

/// All media files attached to a movie, each with its audio tracks, **best-first**
/// (resolution, then HDR tier, then bitrate — see [`media_file_best_order`]). A movie may
/// carry several files at different resolutions; ordering them so `[0]` is the best copy lets
/// every client default to playing the best without re-deriving the ranking. Uses
/// `idx_files_movie`.
pub fn media_files_for_movie(conn: &Connection, movie_id: i64) -> DbResult<Vec<MediaFile>> {
    let order = media_file_best_order();
    let sql =
        format!("SELECT {MEDIA_FILE_COLUMNS} FROM media_files WHERE movie_id = ?1 {order}");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt
        .query_map(params![movie_id], MediaFile::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    attach_audio_streams(conn, rows)
}

/// All media files attached to an episode, each with its audio tracks, **best-first** (same
/// ordering as [`media_files_for_movie`]). Uses `idx_files_episode`.
pub fn media_files_for_episode(conn: &Connection, episode_id: i64) -> DbResult<Vec<MediaFile>> {
    let order = media_file_best_order();
    let sql =
        format!("SELECT {MEDIA_FILE_COLUMNS} FROM media_files WHERE episode_id = ?1 {order}");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt
        .query_map(params![episode_id], MediaFile::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    attach_audio_streams(conn, rows)
}

/// Fetch a single media file by id (with its audio tracks), or [`DbError::NotFound`].
///
/// This is the row `GET /api/stream/:file_id` reads to make the transcode
/// decision; call [`MediaFile::profile`] on the result and pick the default audio track
/// from `audio_streams`.
pub fn get_media_file(conn: &Connection, id: i64) -> DbResult<MediaFile> {
    let sql = format!("SELECT {MEDIA_FILE_COLUMNS} FROM media_files WHERE id = ?1");
    let mut stmt = conn.prepare_cached(&sql)?;
    let mut file = stmt
        .query_row(params![id], MediaFile::from_row)
        .optional()?
        .ok_or(DbError::NotFound)?;
    file.audio_streams = get_audio_streams(conn, file.id)?;
    file.subtitle_streams = get_subtitle_streams(conn, file.id)?;
    file.chapters = chapters_for(conn, file.id)?;
    Ok(file)
}

// ---------------------------------------------------------------------------
// Playback progress (`docs/.tasks/98`) — resume position + "Continue Watching"
// ---------------------------------------------------------------------------

/// Read the saved playback position of a file, or `None` when it has never been played
/// (Task 98). Backs `GET /api/progress/:file_id`.
pub fn get_progress(conn: &Connection, media_file_id: i64) -> DbResult<Option<crate::models::Progress>> {
    conn.prepare_cached(
        "SELECT media_file_id, position_ms, duration_ms, updated_at, finished \
         FROM playback_progress WHERE media_file_id = ?1",
    )?
    .query_row(params![media_file_id], crate::models::Progress::from_row)
    .optional()
    .map_err(Into::into)
}

/// List the "Continue Watching" titles newest-first (Task 98), each joined to its owning
/// movie/episode so a card can render a poster + title and link to `/play/:file_id`.
///
/// A row qualifies when it is **not** `finished` AND is meaningfully into the film — past
/// `MIN_RESUME_MS` (so a title barely started isn't surfaced) and below the finished
/// threshold (a belt-and-braces guard alongside `finished=0`). Ordered by `updated_at DESC`
/// via `idx_playback_progress_updated`; `limit` bounds the row count.
///
/// The movie side reads the movie's poster/title directly; the episode side climbs
/// episode → season → series for the series' poster/title (episodes carry no art of their
/// own). `kind_tag` is 0 for a movie, 1 for an episode, matching [`ContinueItem::from_row`].
pub fn list_continue_watching(conn: &Connection, limit: u32) -> DbResult<Vec<crate::models::ContinueItem>> {
    let limit = clamp_limit(limit);
    // Both sides project the same 8-column shape; a UNION ALL joins the movie- and
    // episode-owned files, then the outer query orders + limits across both.
    let sql = format!(
        "SELECT file_id, kind_tag, title_id, title, poster_path, position_ms, duration_ms, updated_at \
         FROM ( \
             SELECT mf.id AS file_id, 0 AS kind_tag, m.id AS title_id, m.title AS title, \
                    m.poster_path AS poster_path, \
                    p.position_ms AS position_ms, p.duration_ms AS duration_ms, \
                    p.updated_at AS updated_at \
             FROM playback_progress p \
             JOIN media_files mf ON mf.id = p.media_file_id \
             JOIN movies m ON m.id = mf.movie_id \
             WHERE p.finished = 0 AND p.position_ms > ?1 \
               AND (p.duration_ms <= 0 OR p.position_ms < CAST(?2 * p.duration_ms AS INTEGER)) \
             UNION ALL \
             SELECT mf.id AS file_id, 1 AS kind_tag, sr.id AS title_id, sr.title AS title, \
                    sr.poster_path AS poster_path, \
                    p.position_ms AS position_ms, p.duration_ms AS duration_ms, \
                    p.updated_at AS updated_at \
             FROM playback_progress p \
             JOIN media_files mf ON mf.id = p.media_file_id \
             JOIN episodes e ON e.id = mf.episode_id \
             JOIN seasons se ON se.id = e.season_id \
             JOIN series sr ON sr.id = se.series_id \
             WHERE p.finished = 0 AND p.position_ms > ?1 \
               AND (p.duration_ms <= 0 OR p.position_ms < CAST(?2 * p.duration_ms AS INTEGER)) \
         ) \
         ORDER BY updated_at DESC, file_id DESC \
         LIMIT ?3"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt
        .query_map(
            params![MIN_RESUME_MS, crate::writes::FINISHED_FRACTION, limit],
            crate::models::ContinueItem::from_row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Below this many ms into a title, progress is treated as "just started" and is neither
/// offered as a resume nor listed in Continue Watching (`docs/.tasks/98`). Shared between the
/// Continue-Watching query and the resume check on the client (mirrored there).
pub const MIN_RESUME_MS: i64 = 30_000;

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

// ---------------------------------------------------------------------------
// Enrichment observability (`docs/.tasks/96`) — state counts + unmatched list
// ---------------------------------------------------------------------------

/// Per-`metadata_state` title counts for one kind, backing `GET /api/status`
/// (`docs/.tasks/96`). One grouped scan, fast on a large library (the state column is
/// indexed via `idx_*_meta_state`). Every field defaults to 0 so an empty table yields
/// all-zero counts rather than an error.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetadataStateCounts {
    pub total: i64,
    pub matched: i64,
    pub pending: i64,
    pub unmatched: i64,
    pub failed: i64,
}

/// Count titles of one kind grouped by `metadata_state` (`docs/.tasks/96`). Unknown/NULL
/// states are folded into `pending` (the pre-enrichment default) so the four buckets always
/// sum to `total`.
pub fn metadata_state_counts(
    conn: &Connection,
    kind: crate::writes::TitleKind,
) -> DbResult<MetadataStateCounts> {
    let table = match kind {
        crate::writes::TitleKind::Movie => "movies",
        crate::writes::TitleKind::Series => "series",
    };
    let sql = format!("SELECT metadata_state, COUNT(*) FROM {table} GROUP BY metadata_state");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut c = MetadataStateCounts::default();
    for (state, n) in rows {
        c.total += n;
        match state.as_deref() {
            Some("matched") => c.matched += n,
            Some("unmatched") => c.unmatched += n,
            Some("failed") => c.failed += n,
            // `pending`, NULL, or any unexpected value → the pre-enrichment bucket.
            _ => c.pending += n,
        }
    }
    Ok(c)
}

/// One `unmatched`/`failed` title for the operator to act on (`docs/.tasks/96`): its id,
/// parsed title/year, current state, and one on-disk file path so the operator can find and
/// rename/replace it. `path` is any one of the title's media files (the first by id), or
/// `None` for a title with no files yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmatchedTitle {
    pub id: i64,
    pub title: String,
    pub year: Option<i64>,
    pub state: String,
    pub path: Option<String>,
}

/// List titles of one kind whose `metadata_state` is `unmatched` or `failed`, oldest-added
/// first, keyset-paginated by `id` (`docs/.tasks/96`). `after_id` continues after the last id
/// of the prior page (`None` for the first page). Joins one media-file path per title so the
/// status UI can show where the file lives.
pub fn list_unmatched(
    conn: &Connection,
    kind: crate::writes::TitleKind,
    after_id: Option<i64>,
    limit: u32,
) -> DbResult<Vec<UnmatchedTitle>> {
    let limit = clamp_limit(limit);
    // Movies join media_files directly by movie_id; series have no direct file link (files
    // hang off episodes), so their path is left NULL — the title + state is enough to act on.
    let (table, path_select) = match kind {
        crate::writes::TitleKind::Movie => (
            "movies",
            "(SELECT mf.path FROM media_files mf WHERE mf.movie_id = t.id ORDER BY mf.id LIMIT 1)",
        ),
        crate::writes::TitleKind::Series => ("series", "NULL"),
    };
    let sql = format!(
        "SELECT t.id, t.title, t.year, t.metadata_state, {path_select} AS path \
         FROM {table} t \
         WHERE t.metadata_state IN ('unmatched', 'failed') AND t.id > ?1 \
         ORDER BY t.id \
         LIMIT ?2"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt
        .query_map(params![after_id.unwrap_or(0), limit], |r| {
            Ok(UnmatchedTitle {
                id: r.get(0)?,
                title: r.get(1)?,
                year: r.get(2)?,
                state: r.get::<_, Option<String>>(3)?.unwrap_or_else(|| "pending".into()),
                path: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// One recorded ffprobe failure (`docs/.tasks/96` Part C): the media path, the error text,
/// and when it last failed. Backs `GET /api/status/probe-failures`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeFailure {
    pub path: String,
    pub error: String,
    pub last_attempt_at: i64,
}

/// List recorded ffprobe failures, newest attempt first, keyset-paginated by descending
/// `last_attempt_at` then `path` (`docs/.tasks/96`). `after` is the `last_attempt_at` of the
/// last row of the prior page (`None` for the first page); rows strictly older are returned.
/// A simple offset would be fine here (the list is small) but keyset keeps it consistent with
/// the rest of the API.
pub fn list_probe_failures(
    conn: &Connection,
    after: Option<i64>,
    limit: u32,
) -> DbResult<Vec<ProbeFailure>> {
    let limit = clamp_limit(limit);
    // `after` = i64::MAX on the first page returns everything newest-first.
    let cursor = after.unwrap_or(i64::MAX);
    let mut stmt = conn.prepare_cached(
        "SELECT path, error, last_attempt_at FROM probe_failures \
         WHERE last_attempt_at < ?1 \
         ORDER BY last_attempt_at DESC, path \
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![cursor, limit], |r| {
            Ok(ProbeFailure {
                path: r.get(0)?,
                error: r.get(1)?,
                last_attempt_at: r.get(2)?,
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
// People (`docs/.tasks/91` Phase B) — person page + in-library filmography
// ---------------------------------------------------------------------------

/// Fetch the enriched `people` row for a person page (`docs/.tasks/91` Phase B), or
/// [`DbError::NotFound`]. Reads the V6 linkage/art/bio columns alongside the base identity.
pub fn get_person(conn: &Connection, id: i64) -> DbResult<crate::models::PersonMeta> {
    conn.prepare_cached("SELECT id, name, tmdb_id, photo_path, biography FROM people WHERE id = ?1")?
        .query_row(params![id], crate::models::PersonMeta::from_row)
        .optional()?
        .ok_or(DbError::NotFound)
}

/// A person's enrichment state as read by [`get_person_enrichment_state`]:
/// `(people.id, tmdb_id, photo_path)`. The two `Option`s tell enrichment whether the person
/// still needs a headshot (both present ⇒ already enriched ⇒ skip).
pub type PersonEnrichmentState = (i64, Option<i64>, Option<String>);

/// Read a person's enrichment state by their unique `name`, or `None` if no such person
/// exists. Used by enrichment (`docs/.tasks/91` Phase B) to resolve the person row a
/// just-written credit created and to decide whether the person still needs a headshot
/// (idempotency: a person already carrying both a `tmdb_id` and a `photo_path` is skipped).
pub fn get_person_enrichment_state(
    conn: &Connection,
    name: &str,
) -> DbResult<Option<PersonEnrichmentState>> {
    conn.prepare_cached("SELECT id, tmdb_id, photo_path FROM people WHERE name = ?1")?
        .query_row(params![name], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<String>>(2)?))
        })
        .optional()
        .map_err(Into::into)
}

/// A person's in-library filmography (`docs/.tasks/91` Phase B): the titles they are
/// credited on, newest-added first, as [`LibraryCard`]s so the API reuses `LibraryItem`.
///
/// A person can hold more than one credit on a title (actor + director), so the two sides
/// filter `credits` with `EXISTS` (a de-duped membership test) rather than joining it — one
/// card per title regardless of credit count. Ordered `added_at DESC, id DESC` to match the
/// library's recency ordering; not paginated (a single person's in-library catalog is small).
pub fn person_filmography(conn: &Connection, person_id: i64) -> DbResult<Vec<LibraryCard>> {
    let movies = format!(
        "{} WHERE EXISTS (SELECT 1 FROM credits c \
             WHERE c.movie_id = t.id AND c.person_id = ?1)",
        library_select(0, "movies", "mf.movie_id = t.id")
    );
    let series = format!(
        "{} WHERE EXISTS (SELECT 1 FROM credits c \
             WHERE c.series_id = t.id AND c.person_id = ?1)",
        library_select(
            1,
            "series",
            "mf.episode_id IN ( SELECT e.id FROM episodes e \
                JOIN seasons s ON s.id = e.season_id WHERE s.series_id = t.id )",
        )
    );
    let sql = format!(
        "SELECT kind_tag, id, title, sort_title, year, added_at, poster_path, hdr \
         FROM ( {movies} UNION ALL {series} ) \
         ORDER BY added_at DESC, kind_tag DESC, id DESC"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt
        .query_map(params![person_id], LibraryCard::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Fetch a title's stored provider `tmdb_id` (the link back to TMDB), or `None` when the
/// column is NULL (an unmatched title, or one matched by a non-TMDB provider). Used by the
/// genre backfill (`docs/.tasks/91`) to re-`details()` an already-matched title without a
/// rescan. Returns [`DbError::NotFound`] only when the id itself does not exist.
pub fn get_title_tmdb_id(
    conn: &Connection,
    kind: crate::writes::TitleKind,
    id: i64,
) -> DbResult<Option<i64>> {
    let table = match kind {
        crate::writes::TitleKind::Movie => "movies",
        crate::writes::TitleKind::Series => "series",
    };
    let sql = format!("SELECT tmdb_id FROM {table} WHERE id = ?1");
    conn.prepare_cached(&sql)?
        .query_row(params![id], |r| r.get::<_, Option<i64>>(0))
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
        "SELECT c.id, c.person_id, p.name, c.role, c.character, c.ord, p.photo_path \
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
        "SELECT c.id, c.person_id, p.name, c.role, c.character, c.ord, p.photo_path \
         FROM credits c JOIN people p ON p.id = c.person_id \
         WHERE c.series_id = ?1 \
         ORDER BY c.ord",
    )?;
    let rows = stmt
        .query_map(params![series_id], Credit::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
