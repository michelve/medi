//! Write-path query functions used by the `ingest` crate (Phase 1).
//!
//! The read side ([`crate::queries`]) serves the API; this side is exercised only by
//! the ingestion worker as it walks `/media`, probes files, and populates the catalog.
//! Splitting them keeps each file's intent obvious and its SQL close to the columns it
//! touches.
//!
//! Writes are grouped so the worker can run one file's ingest inside a single
//! transaction (see [`crate::Db`] usage in `medi-ingest`): find-or-create the owning
//! movie/episode, then upsert the `media_files` row keyed by its unique `path`, then
//! stamp `scan_state.probed_at`. All of this is synchronous rusqlite and must run under
//! `tokio::task::spawn_blocking` (`01-db-schema.md` §Scaling notes).

use rusqlite::{params, Connection, OptionalExtension};

use crate::DbResult;

// ---------------------------------------------------------------------------
// scan_state — idempotent scan bookkeeping (mtime + size)
// ---------------------------------------------------------------------------

/// The `scan_state` fields the scanner compares to decide whether a file changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStat {
    /// Filesystem mtime, seconds since the unix epoch.
    pub mtime: i64,
    /// File size in bytes.
    pub size_bytes: i64,
}

/// Look up the recorded `(mtime, size, probed_at)` for a path, if the scanner has
/// seen it before. Used to skip unchanged, already-probed files on a re-scan.
///
/// Returns `(FileStat, probed_at)` where `probed_at` is `None` if the file was
/// recorded but never successfully probed.
pub fn get_scan_state(conn: &Connection, path: &str) -> DbResult<Option<(FileStat, Option<i64>)>> {
    let mut stmt =
        conn.prepare_cached("SELECT mtime, size_bytes, probed_at FROM scan_state WHERE path = ?1")?;
    let row = stmt
        .query_row(params![path], |r| {
            Ok((
                FileStat {
                    mtime: r.get(0)?,
                    size_bytes: r.get(1)?,
                },
                r.get::<_, Option<i64>>(2)?,
            ))
        })
        .optional()?;
    Ok(row)
}

/// Record (or update) the scan bookkeeping for a path with its current stat.
///
/// Resets `probed_at` to NULL whenever the stat changes, so a file that was edited
/// on disk is re-probed even though it was probed under its old contents. Leaves the
/// asset-worker columns (`preview_done_at`, `trickplay_done_at`) untouched.
pub fn upsert_scan_state(conn: &Connection, path: &str, stat: FileStat) -> DbResult<()> {
    conn.execute(
        "INSERT INTO scan_state (path, mtime, size_bytes, probed_at) \
             VALUES (?1, ?2, ?3, NULL) \
         ON CONFLICT(path) DO UPDATE SET \
             mtime = excluded.mtime, \
             size_bytes = excluded.size_bytes, \
             probed_at = CASE \
                 WHEN scan_state.mtime = excluded.mtime \
                  AND scan_state.size_bytes = excluded.size_bytes \
                 THEN scan_state.probed_at ELSE NULL END",
        params![path, stat.mtime, stat.size_bytes],
    )?;
    Ok(())
}

/// Stamp `scan_state.probed_at` for a path once ffprobe has populated its
/// `media_files` row. Marks the file as fully ingested for idempotent re-scans.
pub fn mark_probed(conn: &Connection, path: &str, probed_at: i64) -> DbResult<()> {
    conn.execute(
        "UPDATE scan_state SET probed_at = ?2 WHERE path = ?1",
        params![path, probed_at],
    )?;
    Ok(())
}

/// Remove a vanished path from the catalog: its `media_files` row and its
/// `scan_state` bookkeeping. Used when a watch-triggered rescan finds a file that no
/// longer exists on `/media`.
///
/// The owning movie/episode row is intentionally left in place — a title can outlive
/// one of its files, and Phase 1 has no reaping of now-empty titles.
pub fn delete_file(conn: &Connection, path: &str) -> DbResult<()> {
    conn.execute("DELETE FROM media_files WHERE path = ?1", params![path])?;
    conn.execute("DELETE FROM scan_state WHERE path = ?1", params![path])?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Catalog owners — find-or-create movies / series / seasons / episodes
// ---------------------------------------------------------------------------

/// Find a movie by its exact `(sort_title, year)` identity, or create it.
///
/// Phase 1 has no external metadata source, so a movie's identity is the title parsed
/// from its filename plus its year. `year` is part of the match so two films sharing a
/// title (remakes) stay distinct. Returns the movie's id.
pub fn find_or_create_movie(
    conn: &Connection,
    title: &str,
    sort_title: &str,
    year: Option<i64>,
    added_at: i64,
) -> DbResult<i64> {
    // `year IS ?2` matches NULL-to-NULL as well as value-to-value.
    let existing: Option<i64> = conn
        .prepare_cached("SELECT id FROM movies WHERE sort_title = ?1 AND year IS ?2")?
        .query_row(params![sort_title, year], |r| r.get(0))
        .optional()?;
    if let Some(id) = existing {
        return Ok(id);
    }

    conn.execute(
        "INSERT INTO movies (title, sort_title, year, added_at) VALUES (?1, ?2, ?3, ?4)",
        params![title, sort_title, year, added_at],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Find a series by `(sort_title, year)`, or create it. Returns the series id.
pub fn find_or_create_series(
    conn: &Connection,
    title: &str,
    sort_title: &str,
    year: Option<i64>,
    added_at: i64,
) -> DbResult<i64> {
    let existing: Option<i64> = conn
        .prepare_cached("SELECT id FROM series WHERE sort_title = ?1 AND year IS ?2")?
        .query_row(params![sort_title, year], |r| r.get(0))
        .optional()?;
    if let Some(id) = existing {
        return Ok(id);
    }

    conn.execute(
        "INSERT INTO series (title, sort_title, year, added_at) VALUES (?1, ?2, ?3, ?4)",
        params![title, sort_title, year, added_at],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Find the season `season_number` of `series_id`, or create it. The
/// `UNIQUE(series_id, season_number)` constraint makes the upsert race-free even if
/// two episodes of a new season are ingested back to back. Returns the season id.
pub fn find_or_create_season(
    conn: &Connection,
    series_id: i64,
    season_number: i64,
) -> DbResult<i64> {
    conn.execute(
        "INSERT INTO seasons (series_id, season_number) VALUES (?1, ?2) \
         ON CONFLICT(series_id, season_number) DO NOTHING",
        params![series_id, season_number],
    )?;
    let id: i64 = conn
        .prepare_cached("SELECT id FROM seasons WHERE series_id = ?1 AND season_number = ?2")?
        .query_row(params![series_id, season_number], |r| r.get(0))?;
    Ok(id)
}

/// Find episode `episode_number` of `season_id`, or create it, updating its title if
/// one was parsed. `UNIQUE(season_id, episode_number)` keys the upsert. Returns the
/// episode id.
pub fn find_or_create_episode(
    conn: &Connection,
    season_id: i64,
    episode_number: i64,
    title: Option<&str>,
) -> DbResult<i64> {
    conn.execute(
        "INSERT INTO episodes (season_id, episode_number, title) VALUES (?1, ?2, ?3) \
         ON CONFLICT(season_id, episode_number) DO UPDATE SET \
             title = COALESCE(excluded.title, episodes.title)",
        params![season_id, episode_number, title],
    )?;
    let id: i64 = conn
        .prepare_cached("SELECT id FROM episodes WHERE season_id = ?1 AND episode_number = ?2")?
        .query_row(params![season_id, episode_number], |r| r.get(0))?;
    Ok(id)
}

// ---------------------------------------------------------------------------
// media_files — the probed row
// ---------------------------------------------------------------------------

/// Owner of a media file: exactly one of a movie or an episode (mirrors the
/// `media_files` CHECK constraint).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOwner {
    Movie(i64),
    Episode(i64),
}

/// All the columns ffprobe fills on a `media_files` row. The scanner supplies the
/// path/owner/container/size; the probe output supplies the rest. `None` fields are
/// stored as SQL NULL.
///
/// Kept as a plain struct (not the read-side [`crate::models::MediaFile`]) because the
/// write path has no `id` yet and treats every probed value as optional.
#[derive(Debug, Clone, Default)]
pub struct MediaFileWrite {
    pub container: Option<String>,
    pub size_bytes: Option<i64>,
    pub duration_ms: Option<i64>,
    pub video_codec: Option<String>,
    pub video_profile: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub bit_depth: Option<i64>,
    pub bitrate: Option<i64>,
    pub transfer_characteristics: Option<String>,
    pub color_space: Option<String>,
    pub hdr_type: Option<String>,
    pub dv_profile: Option<i64>,
    pub dv_bl_compatible_id: Option<i64>,
    pub dv_level: Option<i64>,
    pub hw_decode_unsupported: bool,
}

/// Insert or update the `media_files` row for `path` (its unique key), attaching it to
/// `owner` and writing every probed field. Re-probing a changed file overwrites the
/// prior metadata in place, so ids and downstream asset references stay stable.
///
/// Returns the media file's id.
pub fn upsert_media_file(
    conn: &Connection,
    path: &str,
    owner: FileOwner,
    data: &MediaFileWrite,
) -> DbResult<i64> {
    let (movie_id, episode_id) = match owner {
        FileOwner::Movie(id) => (Some(id), None),
        FileOwner::Episode(id) => (None, Some(id)),
    };

    conn.execute(
        "INSERT INTO media_files ( \
             path, movie_id, episode_id, container, size_bytes, duration_ms, \
             video_codec, video_profile, width, height, bit_depth, bitrate, \
             transfer_characteristics, color_space, hdr_type, \
             dv_profile, dv_bl_compatible_id, dv_level, hw_decode_unsupported \
         ) VALUES ( \
             ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, \
             ?16, ?17, ?18, ?19 \
         ) \
         ON CONFLICT(path) DO UPDATE SET \
             movie_id = excluded.movie_id, \
             episode_id = excluded.episode_id, \
             container = excluded.container, \
             size_bytes = excluded.size_bytes, \
             duration_ms = excluded.duration_ms, \
             video_codec = excluded.video_codec, \
             video_profile = excluded.video_profile, \
             width = excluded.width, \
             height = excluded.height, \
             bit_depth = excluded.bit_depth, \
             bitrate = excluded.bitrate, \
             transfer_characteristics = excluded.transfer_characteristics, \
             color_space = excluded.color_space, \
             hdr_type = excluded.hdr_type, \
             dv_profile = excluded.dv_profile, \
             dv_bl_compatible_id = excluded.dv_bl_compatible_id, \
             dv_level = excluded.dv_level, \
             hw_decode_unsupported = excluded.hw_decode_unsupported",
        params![
            path,
            movie_id,
            episode_id,
            data.container,
            data.size_bytes,
            data.duration_ms,
            data.video_codec,
            data.video_profile,
            data.width,
            data.height,
            data.bit_depth,
            data.bitrate,
            data.transfer_characteristics,
            data.color_space,
            data.hdr_type,
            data.dv_profile,
            data.dv_bl_compatible_id,
            data.dv_level,
            data.hw_decode_unsupported as i64,
        ],
    )?;

    // last_insert_rowid is set on INSERT; on an UPDATE (conflict) it is not, so read
    // the id back by the unique path.
    let id: i64 = conn
        .prepare_cached("SELECT id FROM media_files WHERE path = ?1")?
        .query_row(params![path], |r| r.get(0))?;
    Ok(id)
}

// ---------------------------------------------------------------------------
// audio_streams — one row per audio track of a media file (Task 70)
// ---------------------------------------------------------------------------

/// One audio track's probed fields, ready to persist via [`replace_audio_streams`].
/// A file has 1..N of these (commentary, dubs, a lossless+lossy pair). `codec` and
/// `immersive` are the normalized strings the transcode decision reads back
/// (`docs/.tasks/70`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioStreamWrite {
    /// ffprobe stream index — what react-native-video's `selectedAudioTrack` selects by.
    pub stream_index: i64,
    pub codec: Option<String>,
    pub profile: Option<String>,
    pub channels: Option<i64>,
    pub channel_layout: Option<String>,
    pub bitrate: Option<i64>,
    pub sample_rate: Option<i64>,
    pub language: Option<String>,
    pub title: Option<String>,
    /// `none` | `dolby_atmos` | `dts_x`.
    pub immersive: String,
    pub is_default: bool,
}

/// Replace all `audio_streams` rows of a media file with a freshly probed set.
///
/// Delete-then-insert so a re-probe overwrites cleanly, mirroring the overwrite-in-place
/// contract of [`upsert_media_file`]. Call this **inside the same transaction** as
/// `upsert_media_file`, passing the media file id it returned, so a file's audio and
/// video metadata commit atomically.
pub fn replace_audio_streams(
    conn: &Connection,
    media_file_id: i64,
    streams: &[AudioStreamWrite],
) -> DbResult<()> {
    conn.execute(
        "DELETE FROM audio_streams WHERE media_file_id = ?1",
        params![media_file_id],
    )?;
    for s in streams {
        conn.execute(
            "INSERT INTO audio_streams ( \
                 media_file_id, stream_index, codec, profile, channels, channel_layout, \
                 bitrate, sample_rate, language, title, immersive, is_default \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                media_file_id,
                s.stream_index,
                s.codec,
                s.profile,
                s.channels,
                s.channel_layout,
                s.bitrate,
                s.sample_rate,
                s.language,
                s.title,
                s.immersive,
                s.is_default as i64,
            ],
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// subtitle_streams — one row per subtitle track of a media file (Task 90)
// ---------------------------------------------------------------------------

/// One subtitle track's fields, ready to persist via [`replace_subtitle_streams`]. A file
/// has 0..N of these (embedded commentary/foreign/forced tracks + external sidecars). A
/// row is either embedded (`stream_index` set, `external_path` None) or an external
/// sidecar (`external_path` set, `stream_index` None). `format` is `"text"` | `"image"`
/// (`medi_core::SubtitleFormat`), which drives the WebVTT-passthrough vs burn-in split.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubtitleStreamWrite {
    /// ffprobe stream index for an embedded track; `None` for an external sidecar.
    pub stream_index: Option<i64>,
    /// ffprobe `codec_name` (subrip, ass, mov_text, hdmv_pgs_subtitle, …).
    pub codec: Option<String>,
    /// `"text"` | `"image"`.
    pub format: String,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_default: bool,
    pub is_forced: bool,
    pub is_external: bool,
    /// Absolute path of the sidecar under `/media`; `None` for an embedded track.
    pub external_path: Option<String>,
}

/// Replace all `subtitle_streams` rows of a media file with a freshly probed set.
///
/// Delete-then-insert so a re-probe overwrites cleanly, mirroring the overwrite-in-place
/// contract of [`upsert_media_file`] / [`replace_audio_streams`]. Call this **inside the
/// same transaction** as `upsert_media_file`, passing the media file id it returned, so a
/// file's video / audio / subtitle metadata commit atomically.
pub fn replace_subtitle_streams(
    conn: &Connection,
    media_file_id: i64,
    streams: &[SubtitleStreamWrite],
) -> DbResult<()> {
    conn.execute(
        "DELETE FROM subtitle_streams WHERE media_file_id = ?1",
        params![media_file_id],
    )?;
    for s in streams {
        conn.execute(
            "INSERT INTO subtitle_streams ( \
                 media_file_id, stream_index, codec, format, language, title, \
                 is_default, is_forced, is_external, external_path \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                media_file_id,
                s.stream_index,
                s.codec,
                s.format,
                s.language,
                s.title,
                s.is_default as i64,
                s.is_forced as i64,
                s.is_external as i64,
                s.external_path,
            ],
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Generated assets — preview clips + trickplay sprites (Phase 3, `medi-assets`)
// ---------------------------------------------------------------------------

/// The trickplay format written for a title (`01-db-schema.md` `trickplay_assets.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrickplayKind {
    /// Roku Base Index Frames: one binary file (header + index + concatenated JPEGs).
    Bif,
    /// A single JPEG mosaic of `cols`×`rows` thumbnails plus the tile/grid metadata.
    TiledJpg,
}

impl TrickplayKind {
    /// The `kind` string persisted in `trickplay_assets.kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            TrickplayKind::Bif => "bif",
            TrickplayKind::TiledJpg => "tiled_jpg",
        }
    }
}

/// The grid geometry of a tiled-JPG trickplay sheet. `None` for a BIF (which is a flat
/// index, not a grid).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrickplayGrid {
    pub tile_w: i64,
    pub tile_h: i64,
    pub cols: i64,
    pub rows: i64,
}

/// Record (or replace) the `preview_clips` row for a media file. Keyed by
/// `media_file_id` (its PRIMARY KEY), so regenerating a preview overwrites in place —
/// the path is stable (`/config/previews/<file_id>.mp4`) across regenerations.
pub fn upsert_preview_clip(
    conn: &Connection,
    media_file_id: i64,
    path: &str,
    generated_at: i64,
) -> DbResult<()> {
    conn.execute(
        "INSERT INTO preview_clips (media_file_id, path, generated_at) \
             VALUES (?1, ?2, ?3) \
         ON CONFLICT(media_file_id) DO UPDATE SET \
             path = excluded.path, generated_at = excluded.generated_at",
        params![media_file_id, path, generated_at],
    )?;
    Ok(())
}

/// Record (or replace) the `trickplay_assets` row for a media file. Keyed by
/// `media_file_id`; regenerating overwrites in place. `grid` is `Some` for tiled-JPG
/// and `None` for BIF (whose columns stay NULL).
pub fn upsert_trickplay_asset(
    conn: &Connection,
    media_file_id: i64,
    kind: TrickplayKind,
    path: &str,
    interval_ms: i64,
    grid: Option<TrickplayGrid>,
    generated_at: i64,
) -> DbResult<()> {
    let (tile_w, tile_h, cols, rows) = match grid {
        Some(g) => (Some(g.tile_w), Some(g.tile_h), Some(g.cols), Some(g.rows)),
        None => (None, None, None, None),
    };
    conn.execute(
        "INSERT INTO trickplay_assets \
             (media_file_id, kind, path, interval_ms, tile_w, tile_h, cols, rows, generated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
         ON CONFLICT(media_file_id) DO UPDATE SET \
             kind = excluded.kind, path = excluded.path, interval_ms = excluded.interval_ms, \
             tile_w = excluded.tile_w, tile_h = excluded.tile_h, \
             cols = excluded.cols, rows = excluded.rows, \
             generated_at = excluded.generated_at",
        params![
            media_file_id,
            kind.as_str(),
            path,
            interval_ms,
            tile_w,
            tile_h,
            cols,
            rows,
            generated_at
        ],
    )?;
    Ok(())
}

/// Stamp `scan_state.preview_done_at` for a path, marking its hover preview generated
/// so the assets worker skips it on the next off-peak pass.
pub fn mark_preview_done(conn: &Connection, path: &str, done_at: i64) -> DbResult<()> {
    conn.execute(
        "UPDATE scan_state SET preview_done_at = ?2 WHERE path = ?1",
        params![path, done_at],
    )?;
    Ok(())
}

/// Stamp `scan_state.trickplay_done_at` for a path, marking its scrub sprites generated.
pub fn mark_trickplay_done(conn: &Connection, path: &str, done_at: i64) -> DbResult<()> {
    conn.execute(
        "UPDATE scan_state SET trickplay_done_at = ?2 WHERE path = ?1",
        params![path, done_at],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Metadata enrichment (`docs/.tasks/60` Phase A) — external ids, match state,
// descriptive fields, and credits.
// ---------------------------------------------------------------------------

/// The `metadata_state` lifecycle value on a `movies`/`series` row
/// (`60-metadata-and-libraries.md` V2). Kept as a typed enum so callers cannot typo a
/// state string; [`Self::as_str`] is the persisted form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataState {
    /// Ingested, not yet enriched.
    Pending,
    /// A provider match was found and its details written.
    Matched,
    /// No candidate cleared the match threshold (do not auto-retry).
    Unmatched,
    /// The provider call errored (transient; a refresh may retry).
    Failed,
}

impl MetadataState {
    pub fn as_str(self) -> &'static str {
        match self {
            MetadataState::Pending => "pending",
            MetadataState::Matched => "matched",
            MetadataState::Unmatched => "unmatched",
            MetadataState::Failed => "failed",
        }
    }
}

/// Which catalog table an enrichment write targets. The metadata columns
/// (`overview`, `poster_path`, `backdrop_path`, `tmdb_id`, `imdb_id`,
/// `metadata_state`) exist on both `movies` and `series` with identical names, so the
/// write helpers take this discriminator and interpolate the table name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleKind {
    Movie,
    Series,
}

impl TitleKind {
    fn table(self) -> &'static str {
        match self {
            TitleKind::Movie => "movies",
            TitleKind::Series => "series",
        }
    }
    /// The `credits` foreign-key column for this kind (`movie_id` / `series_id`).
    fn credit_col(self) -> &'static str {
        match self {
            TitleKind::Movie => "movie_id",
            TitleKind::Series => "series_id",
        }
    }
}

/// The descriptive fields enrichment writes onto a title row. All optional — a provider
/// may return only some — and `None` fields are left untouched (so a re-match that lacks
/// a backdrop does not blank an existing one). Paths are relative to `images_dir()`.
#[derive(Debug, Clone, Default)]
pub struct TitleMetadata {
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub tmdb_id: Option<i64>,
    pub imdb_id: Option<String>,
}

/// Write the matched descriptive metadata onto a title and mark it `matched` in one
/// statement. Uses `COALESCE(?, existing)` per column so a partial `TitleMetadata` only
/// overwrites the fields it carries. `metadata_state` is always set to `matched`.
pub fn set_title_metadata(
    conn: &Connection,
    kind: TitleKind,
    id: i64,
    meta: &TitleMetadata,
) -> DbResult<()> {
    let sql = format!(
        "UPDATE {} SET \
             overview       = COALESCE(?2, overview), \
             poster_path    = COALESCE(?3, poster_path), \
             backdrop_path  = COALESCE(?4, backdrop_path), \
             tmdb_id        = COALESCE(?5, tmdb_id), \
             imdb_id        = COALESCE(?6, imdb_id), \
             metadata_state = 'matched' \
         WHERE id = ?1",
        kind.table()
    );
    conn.execute(
        &sql,
        params![
            id,
            meta.overview,
            meta.poster_path,
            meta.backdrop_path,
            meta.tmdb_id,
            meta.imdb_id,
        ],
    )?;
    Ok(())
}

/// Set only the `metadata_state` of a title (e.g. → `unmatched` when no candidate clears
/// the threshold, or → `failed` on a provider error).
pub fn set_metadata_state(
    conn: &Connection,
    kind: TitleKind,
    id: i64,
    state: MetadataState,
) -> DbResult<()> {
    let sql = format!("UPDATE {} SET metadata_state = ?2 WHERE id = ?1", kind.table());
    conn.execute(&sql, params![id, state.as_str()])?;
    Ok(())
}

/// Read the `metadata_state` of a title, or `None` if the id does not exist.
pub fn get_metadata_state(conn: &Connection, kind: TitleKind, id: i64) -> DbResult<Option<String>> {
    let sql = format!("SELECT metadata_state FROM {} WHERE id = ?1", kind.table());
    let state = conn
        .prepare_cached(&sql)?
        .query_row(params![id], |r| r.get::<_, String>(0))
        .optional()?;
    Ok(state)
}

/// Find-or-create a `people` row by its unique `name`, returning its id. People de-dupe
/// on the `UNIQUE(name)` constraint, so two titles sharing an actor reference one row.
pub fn find_or_create_person(conn: &Connection, name: &str) -> DbResult<i64> {
    conn.execute(
        "INSERT INTO people (name) VALUES (?1) ON CONFLICT(name) DO NOTHING",
        params![name],
    )?;
    let id: i64 = conn
        .prepare_cached("SELECT id FROM people WHERE name = ?1")?
        .query_row(params![name], |r| r.get(0))?;
    Ok(id)
}

/// Replace the credits of a title with a fresh billed cast/crew list.
///
/// A re-enrichment (refresh / fix-match) supplies the authoritative current cast, so we
/// delete this title's existing `credits` rows first, then insert the new ones — this
/// keeps ordering (`ord`) correct and never leaves a stale credit from an old wrong
/// match. `people` rows are *not* deleted (they may be shared with other titles);
/// find-or-create de-dupes them.
pub fn replace_credits(
    conn: &Connection,
    kind: TitleKind,
    id: i64,
    credits: &[CreditWrite],
) -> DbResult<()> {
    let col = kind.credit_col();
    conn.execute(
        &format!("DELETE FROM credits WHERE {col} = ?1"),
        params![id],
    )?;
    let insert = format!(
        "INSERT INTO credits (person_id, {col}, role, character, ord) VALUES (?1, ?2, ?3, ?4, ?5)"
    );
    for c in credits {
        let person_id = find_or_create_person(conn, &c.name)?;
        conn.execute(
            &insert,
            params![person_id, id, c.role, c.character, c.ord],
        )?;
    }
    Ok(())
}

/// One billing entry to persist via [`replace_credits`]. Mirrors the provider's
/// `CreditIn` but lives in `medi-db` so the write path has no dependency on the
/// metadata crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreditWrite {
    pub name: String,
    pub role: String,
    pub character: Option<String>,
    pub ord: i64,
}

// ---------------------------------------------------------------------------
// Genres (`docs/.tasks/91` Phase A) — canonical genre table + per-title M:N joins.
// ---------------------------------------------------------------------------

impl TitleKind {
    /// The genre join table for this kind (`movie_genres` / `series_genres`).
    fn genre_join_table(self) -> &'static str {
        match self {
            TitleKind::Movie => "movie_genres",
            TitleKind::Series => "series_genres",
        }
    }
    /// The title FK column in the genre join table (`movie_id` / `series_id`).
    fn genre_join_col(self) -> &'static str {
        match self {
            TitleKind::Movie => "movie_id",
            TitleKind::Series => "series_id",
        }
    }
}

/// One genre to persist via [`replace_title_genres`]. Mirrors the provider's `Genre` but
/// lives in `medi-db` so the write path has no dependency on the metadata crate. `tmdb_id`
/// is TMDB's stable genre id and becomes the `genres.id` primary key (not autoincrement),
/// so a re-match upserts the same canonical row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenreWrite {
    pub tmdb_id: i64,
    pub name: String,
}

/// Upsert a canonical genre row keyed by its TMDB id, keeping the display `name` current
/// (a provider could rename "Sci-Fi" → "Science Fiction"). Returns the genre id (== the
/// TMDB id). Used by [`replace_title_genres`].
pub fn upsert_genre(conn: &Connection, genre: &GenreWrite) -> DbResult<i64> {
    conn.execute(
        "INSERT INTO genres (id, name) VALUES (?1, ?2) \
         ON CONFLICT(id) DO UPDATE SET name = excluded.name",
        params![genre.tmdb_id, genre.name],
    )?;
    Ok(genre.tmdb_id)
}

// ---------------------------------------------------------------------------
// Collections (franchises) + trailers (Task 91 detail extensions) — movie-only.
// ---------------------------------------------------------------------------

/// One collection to upsert via [`upsert_collection`]. `tmdb_id` becomes the `collections.id`
/// primary key (not autoincrement), so a re-match upserts the same canonical row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionWrite {
    pub tmdb_id: i64,
    pub name: String,
    /// Downloaded collection poster, relative to `images_dir()`, or `None`.
    pub poster_path: Option<String>,
}

/// Upsert a canonical collection keyed by its TMDB id, keeping the name + poster current.
/// Returns the collection id (== the TMDB id). `poster_path` uses `COALESCE` so a re-match
/// that hasn't (re)downloaded the art never blanks an existing path.
pub fn upsert_collection(conn: &Connection, c: &CollectionWrite) -> DbResult<i64> {
    conn.execute(
        "INSERT INTO collections (id, name, poster_path) VALUES (?1, ?2, ?3) \
         ON CONFLICT(id) DO UPDATE SET \
             name = excluded.name, \
             poster_path = COALESCE(excluded.poster_path, collections.poster_path)",
        params![c.tmdb_id, c.name, c.poster_path],
    )?;
    Ok(c.tmdb_id)
}

/// Point a movie at its collection (or clear it with `None`). Called inside the enrichment
/// transaction after [`upsert_collection`]; a re-match that finds no collection clears the
/// stale link.
pub fn set_movie_collection(conn: &Connection, movie_id: i64, collection_id: Option<i64>) -> DbResult<()> {
    conn.execute(
        "UPDATE movies SET collection_id = ?2 WHERE id = ?1",
        params![movie_id, collection_id],
    )?;
    Ok(())
}

/// Set (or clear) a movie's fanart.tv title-logo path (Task 93). Called inside the
/// enrichment transaction alongside [`set_title_metadata`] so a match commits atomically.
/// Written unconditionally (not `COALESCE`d): a re-match with no logo passes `None` and
/// clears a stale link, exactly like [`set_movie_collection`].
pub fn set_movie_logo(conn: &Connection, movie_id: i64, logo_path: Option<&str>) -> DbResult<()> {
    conn.execute(
        "UPDATE movies SET logo_path = ?2 WHERE id = ?1",
        params![movie_id, logo_path],
    )?;
    Ok(())
}

/// Set (or clear) a movie's fanart.tv background wallpaper path (Task 95). Written
/// unconditionally in the enrichment transaction like [`set_movie_logo`] — a re-match with no
/// wallpaper passes `None` and clears a stale link.
pub fn set_movie_wallpaper(conn: &Connection, movie_id: i64, wallpaper_path: Option<&str>) -> DbResult<()> {
    conn.execute(
        "UPDATE movies SET wallpaper_path = ?2 WHERE id = ?1",
        params![movie_id, wallpaper_path],
    )?;
    Ok(())
}

/// One trailer to persist via [`replace_movie_trailers`]. `ord` preserves provider order so
/// the best trailer (first) surfaces first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrailerWrite {
    pub youtube_key: String,
    pub name: Option<String>,
    pub kind: Option<String>,
    pub ord: i64,
}

/// Replace a movie's trailers with a fresh set (Task 91 detail extensions). Delete-then-insert
/// like [`replace_credits`], so a re-match never leaves a stale trailer. Call inside the
/// enrichment transaction.
pub fn replace_movie_trailers(conn: &Connection, movie_id: i64, trailers: &[TrailerWrite]) -> DbResult<()> {
    conn.execute("DELETE FROM trailers WHERE movie_id = ?1", params![movie_id])?;
    for t in trailers {
        conn.execute(
            "INSERT INTO trailers (movie_id, youtube_key, name, kind, ord) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(movie_id, youtube_key) DO NOTHING",
            params![movie_id, t.youtube_key, t.name, t.kind, t.ord],
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Person enrichment (`docs/.tasks/91` Phase B) — TMDB linkage + headshot/bio.
// ---------------------------------------------------------------------------

/// Attach TMDB linkage + a downloaded headshot + a bio onto an existing `people` row
/// (`docs/.tasks/91` Phase B). Keyed by our internal `people.id` (the credit write already
/// found-or-created the person by name), so a person with no TMDB match still has a stable
/// art path. `COALESCE(?, existing)` per column so a partial update (e.g. a person fetch
/// that returned a bio but no photo) never blanks a field it did not carry.
///
/// `photo_path` is relative to `images_dir()` (`people/<people.id>/photo.jpg`). Call from
/// the enrichment write path after the headshot is on disk.
pub fn upsert_person_meta(
    conn: &Connection,
    person_id: i64,
    tmdb_id: Option<i64>,
    photo_path: Option<&str>,
    biography: Option<&str>,
) -> DbResult<()> {
    conn.execute(
        "UPDATE people SET \
             tmdb_id    = COALESCE(?2, tmdb_id), \
             photo_path = COALESCE(?3, photo_path), \
             biography  = COALESCE(?4, biography) \
         WHERE id = ?1",
        params![person_id, tmdb_id, photo_path, biography],
    )?;
    Ok(())
}

/// Replace a title's genre associations with a fresh set (`docs/.tasks/91` Phase A).
///
/// Delete-then-insert like [`replace_credits`], so a re-match never leaves a stale genre.
/// Each genre is upserted into the canonical `genres` table first (the genre rows are
/// shared and never deleted — only the title's joins are), then the join rows are rewritten.
/// Call **inside the same transaction** as [`set_title_metadata`] / [`replace_credits`] so
/// a match writes its metadata, credits, and genres atomically.
pub fn replace_title_genres(
    conn: &Connection,
    kind: TitleKind,
    id: i64,
    genres: &[GenreWrite],
) -> DbResult<()> {
    let join = kind.genre_join_table();
    let col = kind.genre_join_col();
    conn.execute(
        &format!("DELETE FROM {join} WHERE {col} = ?1"),
        params![id],
    )?;
    let insert = format!(
        "INSERT INTO {join} ({col}, genre_id) VALUES (?1, ?2) \
         ON CONFLICT({col}, genre_id) DO NOTHING"
    );
    for g in genres {
        let genre_id = upsert_genre(conn, g)?;
        conn.execute(&insert, params![id, genre_id])?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Libraries (`docs/.tasks/60` Phase B) — Plex-style named libraries + folders.
// ---------------------------------------------------------------------------

/// Create a library row and return its id. `kind` is the [`TitleKind`] string
/// (`"movie"`/`"series"`); folders are added separately via [`add_library_folder`] so a
/// caller can validate each path (MEDIA_DIR containment) before inserting.
pub fn create_library(
    conn: &Connection,
    name: &str,
    kind: TitleKind,
    created_at: i64,
) -> DbResult<i64> {
    let kind_str = match kind {
        TitleKind::Movie => "movie",
        TitleKind::Series => "series",
    };
    conn.execute(
        "INSERT INTO libraries (name, kind, created_at) VALUES (?1, ?2, ?3)",
        params![name, kind_str, created_at],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Rename a library.
pub fn rename_library(conn: &Connection, id: i64, name: &str) -> DbResult<()> {
    conn.execute("UPDATE libraries SET name = ?2 WHERE id = ?1", params![id, name])?;
    Ok(())
}

/// Add a folder to a library. The `path` MUST already be validated as canonical and
/// under MEDIA_DIR by the caller (the API layer) — this function only persists. The
/// `UNIQUE(library_id, path)` constraint makes a duplicate add a no-op.
pub fn add_library_folder(conn: &Connection, library_id: i64, path: &str) -> DbResult<()> {
    conn.execute(
        "INSERT INTO library_folders (library_id, path) VALUES (?1, ?2) \
         ON CONFLICT(library_id, path) DO NOTHING",
        params![library_id, path],
    )?;
    Ok(())
}

/// Remove a folder from a library by its path.
pub fn remove_library_folder(conn: &Connection, library_id: i64, path: &str) -> DbResult<()> {
    conn.execute(
        "DELETE FROM library_folders WHERE library_id = ?1 AND path = ?2",
        params![library_id, path],
    )?;
    Ok(())
}

/// Delete a library. Its folders cascade (FK `ON DELETE CASCADE`), and its movies/series
/// cascade too (the `library_id` FK on those tables is `ON DELETE CASCADE`), which in
/// turn cascades their media_files/credits — so a library delete removes its whole
/// subtree. The caller reaps the corresponding artwork directories afterward.
pub fn delete_library(conn: &Connection, id: i64) -> DbResult<()> {
    conn.execute("DELETE FROM libraries WHERE id = ?1", params![id])?;
    Ok(())
}

/// Scope a movie/series row to a library. Called by the scanner when a file is
/// discovered under one of a library's folders (Phase B). Idempotent.
pub fn set_movie_library(conn: &Connection, movie_id: i64, library_id: i64) -> DbResult<()> {
    conn.execute(
        "UPDATE movies SET library_id = ?2 WHERE id = ?1",
        params![movie_id, library_id],
    )?;
    Ok(())
}

/// Scope a series row to a library.
pub fn set_series_library(conn: &Connection, series_id: i64, library_id: i64) -> DbResult<()> {
    conn.execute(
        "UPDATE series SET library_id = ?2 WHERE id = ?1",
        params![series_id, library_id],
    )?;
    Ok(())
}

/// Count how many libraries exist — used at boot to decide whether to auto-seed.
pub fn library_count(conn: &Connection) -> DbResult<i64> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM libraries", [], |r| r.get(0))?;
    Ok(n)
}

/// Auto-seed a default `movie` and `series` library, each rooted at `media_dir`, and
/// back-fill `library_id` on any pre-existing rows so a single-`/media` deployment keeps
/// working with no config change (`docs/.tasks/60` §DB migrations). A no-op if libraries
/// already exist. Returns `(movie_library_id, series_library_id)`.
pub fn seed_default_libraries(
    conn: &Connection,
    media_dir: &str,
    created_at: i64,
) -> DbResult<Option<(i64, i64)>> {
    if library_count(conn)? > 0 {
        return Ok(None);
    }
    let movie_lib = create_library(conn, "Movies", TitleKind::Movie, created_at)?;
    add_library_folder(conn, movie_lib, media_dir)?;
    let series_lib = create_library(conn, "TV Shows", TitleKind::Series, created_at)?;
    add_library_folder(conn, series_lib, media_dir)?;

    // Back-fill existing rows so they show up scoped to the seeded libraries.
    conn.execute(
        "UPDATE movies SET library_id = ?1 WHERE library_id IS NULL",
        params![movie_lib],
    )?;
    conn.execute(
        "UPDATE series SET library_id = ?1 WHERE library_id IS NULL",
        params![series_lib],
    )?;
    Ok(Some((movie_lib, series_lib)))
}

// ---------------------------------------------------------------------------
// Probe failures (`docs/.tasks/96` Part C) — record/clear ffprobe skips
// ---------------------------------------------------------------------------

/// Record (or refresh) an ffprobe failure for `path`. Upserts on the path primary key so a
/// repeatedly-failing file keeps one row with the latest error + attempt time.
pub fn upsert_probe_failure(
    conn: &Connection,
    path: &str,
    error: &str,
    now: i64,
) -> DbResult<()> {
    conn.execute(
        "INSERT INTO probe_failures (path, error, last_attempt_at) VALUES (?1, ?2, ?3) \
         ON CONFLICT(path) DO UPDATE SET error = excluded.error, last_attempt_at = excluded.last_attempt_at",
        params![path, error, now],
    )?;
    Ok(())
}

/// Clear the recorded ffprobe failure for `path` after a subsequent successful probe. A no-op
/// when there was none.
pub fn clear_probe_failure(conn: &Connection, path: &str) -> DbResult<()> {
    conn.execute("DELETE FROM probe_failures WHERE path = ?1", params![path])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> (crate::Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::open(dir.path().join("library.db"), 2).unwrap();
        (db, dir)
    }

    #[test]
    fn scan_state_upsert_clears_probed_on_change() {
        let (db, _dir) = db();
        let conn = db.conn().unwrap();
        let p = "/media/a.mkv";

        upsert_scan_state(&conn, p, FileStat { mtime: 10, size_bytes: 100 }).unwrap();
        mark_probed(&conn, p, 999).unwrap();
        let (stat, probed) = get_scan_state(&conn, p).unwrap().unwrap();
        assert_eq!(stat, FileStat { mtime: 10, size_bytes: 100 });
        assert_eq!(probed, Some(999));

        // Same stat again: probed_at is preserved.
        upsert_scan_state(&conn, p, FileStat { mtime: 10, size_bytes: 100 }).unwrap();
        assert_eq!(get_scan_state(&conn, p).unwrap().unwrap().1, Some(999));

        // Changed size: probed_at is cleared so the file is re-probed.
        upsert_scan_state(&conn, p, FileStat { mtime: 10, size_bytes: 200 }).unwrap();
        assert_eq!(get_scan_state(&conn, p).unwrap().unwrap().1, None);
    }

    #[test]
    fn find_or_create_movie_is_idempotent() {
        let (db, _dir) = db();
        let conn = db.conn().unwrap();
        let a = find_or_create_movie(&conn, "Arrival", "arrival", Some(2016), 0).unwrap();
        let b = find_or_create_movie(&conn, "Arrival", "arrival", Some(2016), 0).unwrap();
        assert_eq!(a, b);
        // Different year → a distinct row (remake).
        let c = find_or_create_movie(&conn, "Arrival", "arrival", Some(1996), 0).unwrap();
        assert_ne!(a, c);
    }

    #[test]
    fn series_season_episode_chain() {
        let (db, _dir) = db();
        let conn = db.conn().unwrap();
        let series = find_or_create_series(&conn, "Severance", "severance", Some(2022), 0).unwrap();
        let s1 = find_or_create_season(&conn, series, 1).unwrap();
        let s1_again = find_or_create_season(&conn, series, 1).unwrap();
        assert_eq!(s1, s1_again);
        let e1 = find_or_create_episode(&conn, s1, 1, Some("Good News About Hell")).unwrap();
        // Re-ingest fills nothing new but returns the same id.
        let e1_again = find_or_create_episode(&conn, s1, 1, None).unwrap();
        assert_eq!(e1, e1_again);
    }

    #[test]
    fn media_file_upsert_round_trips_and_overwrites() {
        let (db, _dir) = db();
        let conn = db.conn().unwrap();
        let movie = find_or_create_movie(&conn, "Test", "test", None, 0).unwrap();

        let mut data = MediaFileWrite {
            container: Some("mkv".into()),
            width: Some(3840),
            height: Some(2160),
            bit_depth: Some(10),
            video_codec: Some("hevc".into()),
            hdr_type: Some("dolbyvision".into()),
            dv_profile: Some(5),
            ..Default::default()
        };
        let id = upsert_media_file(&conn, "/media/t.mkv", FileOwner::Movie(movie), &data).unwrap();

        let file = crate::queries::get_media_file(&conn, id).unwrap();
        assert_eq!(file.dv_profile, Some(5));
        assert_eq!(file.width, Some(3840));

        // Re-probe with a different codec: same id, overwritten fields.
        data.video_codec = Some("av1".into());
        data.dv_profile = None;
        data.hdr_type = Some("hdr10".into());
        let id2 = upsert_media_file(&conn, "/media/t.mkv", FileOwner::Movie(movie), &data).unwrap();
        assert_eq!(id, id2);
        let file2 = crate::queries::get_media_file(&conn, id).unwrap();
        assert_eq!(file2.video_codec.as_deref(), Some("av1"));
        assert_eq!(file2.dv_profile, None);
    }

    #[test]
    fn asset_rows_upsert_and_mark_done() {
        let (db, _dir) = db();
        let conn = db.conn().unwrap();
        let movie = find_or_create_movie(&conn, "T", "t", None, 0).unwrap();
        let data = MediaFileWrite {
            container: Some("mkv".into()),
            width: Some(1920),
            height: Some(1080),
            ..Default::default()
        };
        let file_id = upsert_media_file(&conn, "/media/a.mkv", FileOwner::Movie(movie), &data)
            .unwrap();
        upsert_scan_state(&conn, "/media/a.mkv", FileStat { mtime: 1, size_bytes: 1 }).unwrap();

        // Preview: insert then overwrite in place (PK = media_file_id).
        upsert_preview_clip(&conn, file_id, "/config/previews/1.mp4", 100).unwrap();
        upsert_preview_clip(&conn, file_id, "/config/previews/1.mp4", 200).unwrap();
        let (path, gen): (String, i64) = conn
            .query_row(
                "SELECT path, generated_at FROM preview_clips WHERE media_file_id = ?1",
                params![file_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(path, "/config/previews/1.mp4");
        assert_eq!(gen, 200, "overwrite bumps generated_at, no duplicate row");

        // Trickplay BIF (no grid) then re-generate as tiled-JPG (with grid).
        upsert_trickplay_asset(&conn, file_id, TrickplayKind::Bif, "/config/trickplay/1.bif", 10000, None, 100).unwrap();
        upsert_trickplay_asset(
            &conn,
            file_id,
            TrickplayKind::TiledJpg,
            "/config/trickplay/1.jpg",
            10000,
            Some(TrickplayGrid { tile_w: 320, tile_h: 180, cols: 10, rows: 3 }),
            300,
        )
        .unwrap();
        let (kind, cols): (String, Option<i64>) = conn
            .query_row(
                "SELECT kind, cols FROM trickplay_assets WHERE media_file_id = ?1",
                params![file_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(kind, "tiled_jpg", "second write replaces the BIF row in place");
        assert_eq!(cols, Some(10));

        // Done stamps land on scan_state.
        mark_preview_done(&conn, "/media/a.mkv", 111).unwrap();
        mark_trickplay_done(&conn, "/media/a.mkv", 222).unwrap();
        let (pd, td): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT preview_done_at, trickplay_done_at FROM scan_state WHERE path = ?1",
                params!["/media/a.mkv"],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((pd, td), (Some(111), Some(222)));
    }

    #[test]
    fn audio_streams_replace_and_read_back() {
        // A file with three audio tracks yields three rows in stream_index order; a
        // re-probe replaces the whole set (Task 70).
        let (db, _dir) = db();
        let conn = db.conn().unwrap();
        let movie = find_or_create_movie(&conn, "T", "t", None, 0).unwrap();
        let data = MediaFileWrite {
            container: Some("mkv".into()),
            width: Some(3840),
            height: Some(2160),
            ..Default::default()
        };
        let file_id = upsert_media_file(&conn, "/media/t.mkv", FileOwner::Movie(movie), &data).unwrap();

        let streams = vec![
            AudioStreamWrite {
                stream_index: 1,
                codec: Some("truehd".into()),
                channels: Some(8),
                channel_layout: Some("7.1".into()),
                immersive: "dolby_atmos".into(),
                is_default: true,
                ..Default::default()
            },
            AudioStreamWrite {
                stream_index: 2,
                codec: Some("dtshd".into()),
                channels: Some(6),
                immersive: "none".into(),
                ..Default::default()
            },
            AudioStreamWrite {
                stream_index: 3,
                codec: Some("aac".into()),
                channels: Some(2),
                title: Some("Commentary".into()),
                immersive: "none".into(),
                ..Default::default()
            },
        ];
        replace_audio_streams(&conn, file_id, &streams).unwrap();

        let read = crate::queries::get_audio_streams(&conn, file_id).unwrap();
        assert_eq!(read.len(), 3);
        assert_eq!(read[0].stream_index, 1);
        assert_eq!(read[0].codec.as_deref(), Some("truehd"));
        assert_eq!(read[0].immersive, "dolby_atmos");
        assert!(read[0].is_default);
        assert_eq!(read[2].title.as_deref(), Some("Commentary"));

        // A re-probe with a single stereo track replaces the whole set.
        replace_audio_streams(
            &conn,
            file_id,
            &[AudioStreamWrite {
                stream_index: 1,
                codec: Some("eac3".into()),
                channels: Some(6),
                immersive: "none".into(),
                is_default: true,
                ..Default::default()
            }],
        )
        .unwrap();
        let read2 = crate::queries::get_audio_streams(&conn, file_id).unwrap();
        assert_eq!(read2.len(), 1, "re-probe replaces the whole set");
        assert_eq!(read2[0].codec.as_deref(), Some("eac3"));

        // And they surface on the MediaFile read model.
        let mf = crate::queries::get_media_file(&conn, file_id).unwrap();
        assert_eq!(mf.audio_streams.len(), 1);
    }

    #[test]
    fn subtitle_streams_replace_and_read_back() {
        // A file with an embedded text track, an embedded image track, and an external
        // forced sidecar yields three rows; a re-probe replaces the whole set (Task 90).
        let (db, _dir) = db();
        let conn = db.conn().unwrap();
        let movie = find_or_create_movie(&conn, "T", "t", None, 0).unwrap();
        let data = MediaFileWrite {
            container: Some("mkv".into()),
            width: Some(3840),
            height: Some(2160),
            ..Default::default()
        };
        let file_id = upsert_media_file(&conn, "/media/t.mkv", FileOwner::Movie(movie), &data).unwrap();

        let subs = vec![
            SubtitleStreamWrite {
                stream_index: Some(2),
                codec: Some("subrip".into()),
                format: "text".into(),
                language: Some("eng".into()),
                is_default: true,
                ..Default::default()
            },
            SubtitleStreamWrite {
                stream_index: Some(3),
                codec: Some("hdmv_pgs_subtitle".into()),
                format: "image".into(),
                language: Some("eng".into()),
                ..Default::default()
            },
            SubtitleStreamWrite {
                codec: Some("subrip".into()),
                format: "text".into(),
                language: Some("eng".into()),
                is_forced: true,
                is_external: true,
                external_path: Some("/media/Movie (2020).en.forced.srt".into()),
                ..Default::default()
            },
        ];
        replace_subtitle_streams(&conn, file_id, &subs).unwrap();

        let read = crate::queries::get_subtitle_streams(&conn, file_id).unwrap();
        assert_eq!(read.len(), 3);
        // Embedded tracks sort first by stream_index; the external (NULL index) sorts last.
        assert_eq!(read[0].stream_index, Some(2));
        assert_eq!(read[0].format, "text");
        assert!(read[0].is_default);
        assert_eq!(read[1].format, "image");
        assert!(read[2].is_external);
        assert!(read[2].is_forced);
        assert_eq!(read[2].language.as_deref(), Some("eng"));
        assert_eq!(read[2].stream_index, None);

        // A re-probe with a single track replaces the whole set.
        replace_subtitle_streams(
            &conn,
            file_id,
            &[SubtitleStreamWrite {
                stream_index: Some(2),
                codec: Some("ass".into()),
                format: "text".into(),
                ..Default::default()
            }],
        )
        .unwrap();
        let read2 = crate::queries::get_subtitle_streams(&conn, file_id).unwrap();
        assert_eq!(read2.len(), 1, "re-probe replaces the whole set");
        assert_eq!(read2[0].codec.as_deref(), Some("ass"));

        // And they surface on the MediaFile read model.
        let mf = crate::queries::get_media_file(&conn, file_id).unwrap();
        assert_eq!(mf.subtitle_streams.len(), 1);
    }

    #[test]
    fn metadata_write_marks_matched_and_dedupes_people() {
        let (db, _dir) = db();
        let conn = db.conn().unwrap();
        let movie = find_or_create_movie(&conn, "Arrival", "arrival", Some(2016), 0).unwrap();

        // Fresh movies default to 'pending'.
        assert_eq!(
            get_metadata_state(&conn, TitleKind::Movie, movie).unwrap().as_deref(),
            Some("pending")
        );

        let meta = TitleMetadata {
            overview: Some("Aliens arrive.".into()),
            poster_path: Some("movies/1/poster.jpg".into()),
            tmdb_id: Some(329865),
            imdb_id: Some("tt2543164".into()),
            ..Default::default()
        };
        set_title_metadata(&conn, TitleKind::Movie, movie, &meta).unwrap();

        let credits = vec![
            CreditWrite { name: "Amy Adams".into(), role: "actor".into(), character: Some("Louise".into()), ord: 0 },
            CreditWrite { name: "Jeremy Renner".into(), role: "actor".into(), character: Some("Ian".into()), ord: 1 },
        ];
        replace_credits(&conn, TitleKind::Movie, movie, &credits).unwrap();

        // State flipped to matched; fields written.
        assert_eq!(
            get_metadata_state(&conn, TitleKind::Movie, movie).unwrap().as_deref(),
            Some("matched")
        );
        let m = crate::queries::get_movie(&conn, movie).unwrap();
        assert_eq!(m.overview.as_deref(), Some("Aliens arrive."));
        assert_eq!(m.poster_path.as_deref(), Some("movies/1/poster.jpg"));

        let listed = crate::queries::credits_for_movie(&conn, movie).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].person_name, "Amy Adams");
        assert_eq!(listed[0].ord, Some(0));

        // A second person shared across a title de-dupes: reuse Amy Adams for another movie.
        let m2 = find_or_create_movie(&conn, "Nocturnal", "nocturnal", Some(2016), 0).unwrap();
        replace_credits(
            &conn,
            TitleKind::Movie,
            m2,
            &[CreditWrite { name: "Amy Adams".into(), role: "actor".into(), character: None, ord: 0 }],
        )
        .unwrap();
        let people_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM people WHERE name = 'Amy Adams'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(people_count, 1, "shared actor is one people row");
    }

    #[test]
    fn replace_credits_overwrites_prior_cast() {
        let (db, _dir) = db();
        let conn = db.conn().unwrap();
        let movie = find_or_create_movie(&conn, "X", "x", None, 0).unwrap();

        replace_credits(
            &conn,
            TitleKind::Movie,
            movie,
            &[CreditWrite { name: "Wrong Actor".into(), role: "actor".into(), character: None, ord: 0 }],
        )
        .unwrap();
        // A corrected match replaces the whole cast — the stale credit is gone.
        replace_credits(
            &conn,
            TitleKind::Movie,
            movie,
            &[CreditWrite { name: "Right Actor".into(), role: "actor".into(), character: None, ord: 0 }],
        )
        .unwrap();
        let listed = crate::queries::credits_for_movie(&conn, movie).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].person_name, "Right Actor");
    }

    #[test]
    fn replace_title_genres_upserts_and_replaces() {
        // A title's genres are written, a genre is shared across titles (one canonical row),
        // and a re-match replaces the title's genre set wholesale (`docs/.tasks/91`).
        let (db, _dir) = db();
        let conn = db.conn().unwrap();
        let m1 = find_or_create_movie(&conn, "Arrival", "arrival", Some(2016), 0).unwrap();
        let m2 = find_or_create_movie(&conn, "Dune", "dune", Some(2021), 0).unwrap();

        replace_title_genres(
            &conn,
            TitleKind::Movie,
            m1,
            &[
                GenreWrite { tmdb_id: 878, name: "Science Fiction".into() },
                GenreWrite { tmdb_id: 18, name: "Drama".into() },
            ],
        )
        .unwrap();
        // Dune shares Sci-Fi (same canonical genre row, not a duplicate).
        replace_title_genres(
            &conn,
            TitleKind::Movie,
            m2,
            &[GenreWrite { tmdb_id: 878, name: "Science Fiction".into() }],
        )
        .unwrap();

        // One canonical `genres` row for Sci-Fi despite two titles referencing it.
        let genre_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM genres WHERE id = 878", [], |r| r.get(0))
            .unwrap();
        assert_eq!(genre_count, 1);
        // Sci-Fi has both movies; Drama has one.
        let listed = crate::queries::list_genres(&conn).unwrap();
        let scifi = listed.iter().find(|g| g.id == 878).unwrap();
        assert_eq!(scifi.count, 2);
        assert!(listed.iter().any(|g| g.id == 18 && g.count == 1));

        // A re-match of m1 with a different set replaces its joins (Drama is dropped from m1).
        replace_title_genres(
            &conn,
            TitleKind::Movie,
            m1,
            &[GenreWrite { tmdb_id: 28, name: "Action".into() }],
        )
        .unwrap();
        // Drama now references no title → excluded from list_genres.
        let listed2 = crate::queries::list_genres(&conn).unwrap();
        assert!(!listed2.iter().any(|g| g.id == 18), "orphaned Drama is excluded");
        // Sci-Fi now has only Dune.
        assert_eq!(listed2.iter().find(|g| g.id == 878).unwrap().count, 1);
    }

    #[test]
    fn upsert_person_meta_coalesces_fields() {
        // Person enrichment attaches tmdb_id + photo + bio onto an existing people row, and a
        // partial later update never blanks a field it doesn't carry (`docs/.tasks/91` B).
        let (db, _dir) = db();
        let conn = db.conn().unwrap();
        let pid = find_or_create_person(&conn, "Amy Adams").unwrap();

        upsert_person_meta(&conn, pid, Some(9273), Some("people/1/photo.jpg"), Some("An actress.")).unwrap();
        let p = crate::queries::get_person(&conn, pid).unwrap();
        assert_eq!(p.tmdb_id, Some(9273));
        assert_eq!(p.photo_path.as_deref(), Some("people/1/photo.jpg"));
        assert_eq!(p.biography.as_deref(), Some("An actress."));

        // A later bio-only update keeps the existing tmdb_id + photo (COALESCE).
        upsert_person_meta(&conn, pid, None, None, Some("Updated bio.")).unwrap();
        let p2 = crate::queries::get_person(&conn, pid).unwrap();
        assert_eq!(p2.tmdb_id, Some(9273), "tmdb_id preserved");
        assert_eq!(p2.photo_path.as_deref(), Some("people/1/photo.jpg"), "photo preserved");
        assert_eq!(p2.biography.as_deref(), Some("Updated bio."));
    }

    #[test]
    fn matched_titles_missing_genres_worklist() {
        // The backfill worklist returns only `matched` titles lacking genre rows (or all
        // matched titles under force) — `docs/.tasks/91` §Backfill.
        let (db, _dir) = db();
        let conn = db.conn().unwrap();
        let with_genres = find_or_create_movie(&conn, "Has Genres", "has genres", None, 1).unwrap();
        let without = find_or_create_movie(&conn, "No Genres", "no genres", None, 2).unwrap();
        let pending = find_or_create_movie(&conn, "Pending", "pending", None, 3).unwrap();
        // Two are matched; one stays pending.
        set_metadata_state(&conn, TitleKind::Movie, with_genres, MetadataState::Matched).unwrap();
        set_metadata_state(&conn, TitleKind::Movie, without, MetadataState::Matched).unwrap();
        // `pending` keeps its default 'pending' state.
        let _ = pending;
        replace_title_genres(
            &conn,
            TitleKind::Movie,
            with_genres,
            &[GenreWrite { tmdb_id: 878, name: "Science Fiction".into() }],
        )
        .unwrap();

        // Default: only the matched title *without* genres.
        let work =
            crate::queries::matched_titles_missing_genres(&conn, TitleKind::Movie, false, 100).unwrap();
        assert_eq!(work, vec![without], "only matched-and-missing is on the worklist");

        // Force: every matched title (both), pending excluded, oldest-added first.
        let forced =
            crate::queries::matched_titles_missing_genres(&conn, TitleKind::Movie, true, 100).unwrap();
        assert_eq!(forced, vec![with_genres, without]);
    }

    #[test]
    fn metadata_state_transitions_to_unmatched() {
        let (db, _dir) = db();
        let conn = db.conn().unwrap();
        let movie = find_or_create_movie(&conn, "Obscure", "obscure", None, 0).unwrap();
        set_metadata_state(&conn, TitleKind::Movie, movie, MetadataState::Unmatched).unwrap();
        assert_eq!(
            get_metadata_state(&conn, TitleKind::Movie, movie).unwrap().as_deref(),
            Some("unmatched")
        );
    }

    #[test]
    fn seed_default_libraries_backfills_existing_rows() {
        let (db, _dir) = db();
        let conn = db.conn().unwrap();
        // Pre-existing rows from a single-/media deployment.
        let m = find_or_create_movie(&conn, "Old Movie", "old movie", None, 0).unwrap();
        let s = find_or_create_series(&conn, "Old Show", "old show", None, 0).unwrap();

        let seeded = seed_default_libraries(&conn, "/media", 100).unwrap();
        let (movie_lib, series_lib) = seeded.expect("seeded on empty libraries");

        // Two libraries, each with the /media root.
        let libs = crate::queries::list_libraries(&conn).unwrap();
        assert_eq!(libs.len(), 2);
        assert!(libs.iter().any(|l| l.library.kind == "movie" && l.folders == vec!["/media".to_string()]));
        assert!(libs.iter().any(|l| l.library.kind == "series"));

        // Existing rows back-filled to the matching library.
        let movie_lib_id: i64 = conn
            .query_row("SELECT library_id FROM movies WHERE id = ?1", params![m], |r| r.get(0))
            .unwrap();
        assert_eq!(movie_lib_id, movie_lib);
        let series_lib_id: i64 = conn
            .query_row("SELECT library_id FROM series WHERE id = ?1", params![s], |r| r.get(0))
            .unwrap();
        assert_eq!(series_lib_id, series_lib);

        // Idempotent: a second call is a no-op (returns None).
        assert!(seed_default_libraries(&conn, "/media", 200).unwrap().is_none());
    }

    #[test]
    fn library_folders_crud_and_roots() {
        let (db, _dir) = db();
        let conn = db.conn().unwrap();
        let lib = create_library(&conn, "Films", TitleKind::Movie, 0).unwrap();
        add_library_folder(&conn, lib, "/media/movies").unwrap();
        add_library_folder(&conn, lib, "/media/movies").unwrap(); // dup no-op
        add_library_folder(&conn, lib, "/media/more-movies").unwrap();

        let got = crate::queries::get_library(&conn, lib).unwrap();
        assert_eq!(got.folders.len(), 2);

        remove_library_folder(&conn, lib, "/media/more-movies").unwrap();
        assert_eq!(crate::queries::folders_for_library(&conn, lib).unwrap().len(), 1);

        let roots = crate::queries::library_roots(&conn).unwrap();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].kind, "movie");
        assert_eq!(roots[0].path, "/media/movies");
    }

    #[test]
    fn delete_library_cascades_titles() {
        let (db, _dir) = db();
        let conn = db.conn().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
        let lib = create_library(&conn, "Films", TitleKind::Movie, 0).unwrap();
        let m = find_or_create_movie(&conn, "A", "a", None, 0).unwrap();
        set_movie_library(&conn, m, lib).unwrap();

        delete_library(&conn, lib).unwrap();
        // The movie cascaded away with its library.
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM movies WHERE id = ?1", params![m], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 0);
    }

    // --- Enrichment observability (`docs/.tasks/96`) --------------------------

    #[test]
    fn probe_failure_upsert_then_clear() {
        let (db, _dir) = db();
        let conn = db.conn().unwrap();
        let p = "/media/bad.mp4";

        // Record a failure, then refresh it (upsert keeps one row with the latest error).
        upsert_probe_failure(&conn, p, "exit status 1", 100).unwrap();
        upsert_probe_failure(&conn, p, "moov atom not found", 200).unwrap();
        let rows = crate::queries::list_probe_failures(&conn, None, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, p);
        assert_eq!(rows[0].error, "moov atom not found");
        assert_eq!(rows[0].last_attempt_at, 200);

        // A successful re-probe clears it.
        clear_probe_failure(&conn, p).unwrap();
        assert!(crate::queries::list_probe_failures(&conn, None, 10).unwrap().is_empty());
    }

    #[test]
    fn metadata_state_counts_and_unmatched_list() {
        let (db, _dir) = db();
        let conn = db.conn().unwrap();

        // Seed a mix of states. find_or_create defaults to 'pending'.
        let matched = find_or_create_movie(&conn, "Matched", "matched", Some(2020), 10).unwrap();
        let unmatched = find_or_create_movie(&conn, "Junk Name", "junk name", None, 20).unwrap();
        let failed = find_or_create_movie(&conn, "Failed", "failed", None, 30).unwrap();
        let _pending = find_or_create_movie(&conn, "Pending", "pending", None, 40).unwrap();
        set_metadata_state(&conn, TitleKind::Movie, matched, MetadataState::Matched).unwrap();
        set_metadata_state(&conn, TitleKind::Movie, unmatched, MetadataState::Unmatched).unwrap();
        set_metadata_state(&conn, TitleKind::Movie, failed, MetadataState::Failed).unwrap();

        let counts = crate::queries::metadata_state_counts(&conn, TitleKind::Movie).unwrap();
        assert_eq!(counts.total, 4);
        assert_eq!(counts.matched, 1);
        assert_eq!(counts.unmatched, 1);
        assert_eq!(counts.failed, 1);
        assert_eq!(counts.pending, 1);

        // The unmatched list returns only unmatched + failed, oldest-added (by id) first.
        let list = crate::queries::list_unmatched(&conn, TitleKind::Movie, None, 10).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, unmatched);
        assert_eq!(list[0].state, "unmatched");
        assert_eq!(list[1].id, failed);
        assert_eq!(list[1].state, "failed");
        // Keyset: after the first row returns only the second.
        let page2 = crate::queries::list_unmatched(&conn, TitleKind::Movie, Some(unmatched), 10).unwrap();
        assert_eq!(page2.len(), 1);
        assert_eq!(page2[0].id, failed);
    }
}
