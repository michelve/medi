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
             -- a changed file must be re-probed: clear probed_at when stat differs. \
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
}
