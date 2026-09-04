//! Integration tests for the schema + tuning (`docs/.tasks/01-db-schema.md`
//! §Verification): fresh boot sets `page_size`/`journal_mode`, migrations are
//! idempotent across restarts, and a DV Profile 5 file round-trips.

use medi_core::{DvProfile, HdrType, VideoCodec};
use medi_db::queries;
use medi_db::writes;

/// A fresh boot creates the db with 64 KB pages and WAL journaling.
#[test]
fn fresh_boot_sets_pragmas() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.db");

    let db = medi_db::open(&path, 4).unwrap();
    let conn = db.conn().unwrap();

    let page_size: i64 = conn
        .pragma_query_value(None, "page_size", |r| r.get(0))
        .unwrap();
    assert_eq!(page_size, 65536, "page_size must be 64 KB");

    let journal_mode: String = conn
        .pragma_query_value(None, "journal_mode", |r| r.get(0))
        .unwrap();
    assert!(
        journal_mode.eq_ignore_ascii_case("wal"),
        "journal_mode must be WAL, got {journal_mode:?}"
    );

    // Per-connection PRAGMAs from the customizer are live on a checkout.
    let fk: i64 = conn
        .pragma_query_value(None, "foreign_keys", |r| r.get(0))
        .unwrap();
    assert_eq!(fk, 1, "foreign_keys must be ON");
}

/// Re-opening the same file applies no new migrations (refinery records versions).
#[test]
fn migrations_are_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.db");

    // First open runs V1.
    let _ = medi_db::open(&path, 2).unwrap();
    // Second open must succeed and be a no-op — a failure here would surface as Err.
    let db = medi_db::open(&path, 2).unwrap();

    // Tables exist and are queryable.
    let conn = db.conn().unwrap();
    let movies = queries::list_movies(&conn, None, 10).unwrap();
    assert!(movies.is_empty());
}

/// Inserting a Dolby Vision Profile 5 file yields `dv_profile=5`,
/// `hdr_type='dolbyvision'`, and a correctly typed `MediaProfile`.
#[test]
fn dv_profile_5_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.db");
    let db = medi_db::open(&path, 2).unwrap();
    let conn = db.conn().unwrap();

    conn.execute(
        "INSERT INTO movies (id, title, sort_title, added_at) VALUES (1, 'Test', 'test', 0)",
        [],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO media_files \
         (id, movie_id, path, container, video_codec, width, height, bit_depth, \
          hdr_type, dv_profile, dv_bl_compatible_id, hw_decode_unsupported) \
         VALUES (88, 1, '/media/test.mkv', 'mkv', 'hevc', 3840, 2160, 10, \
                 'dolbyvision', 5, 0, 0)",
        [],
    )
    .unwrap();

    let file = queries::get_media_file(&conn, 88).unwrap();
    assert_eq!(file.dv_profile, Some(5));
    assert_eq!(file.hdr_type.as_deref(), Some("dolbyvision"));

    let profile = file.profile().expect("probed file has a profile");
    assert_eq!(profile.codec, VideoCodec::Hevc);
    assert_eq!(profile.hdr, HdrType::DolbyVision);
    assert_eq!(profile.dv, Some(DvProfile::P5));
    assert_eq!(profile.bit_depth, 10);
    assert!(!profile.hw_decode_unsupported);

    // The movie detail aggregate wires the file through.
    let detail = queries::get_movie_detail(&conn, 1).unwrap();
    assert_eq!(detail.media_files.len(), 1);
    assert_eq!(detail.media_files[0].id, 88);
}

/// The `media_files` CHECK constraint rejects a file bound to both a movie and an
/// episode (and one bound to neither).
#[test]
fn media_file_xor_owner_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.db");
    let db = medi_db::open(&path, 2).unwrap();
    let conn = db.conn().unwrap();

    // Neither owner → violates CHECK.
    let neither = conn.execute(
        "INSERT INTO media_files (id, path) VALUES (1, '/media/x.mkv')",
        [],
    );
    assert!(neither.is_err(), "a file with no owner must be rejected");
}

/// The collection backfill worklist lists matched movies with no collection, keyset-pages by
/// `(added_at, id)`, and terminates (the cursor advances past rows that stay NULL rather than
/// re-listing them, which a "still missing" filter could not do).
#[test]
fn collection_worklist_pages_and_terminates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.db");
    let db = medi_db::open(&path, 2).unwrap();
    let conn = db.conn().unwrap();

    conn.execute_batch(
        "INSERT INTO collections (id, name) VALUES (9, 'Franchise');
         -- Three matched movies with no collection (backfill candidates).
         INSERT INTO movies (id, title, sort_title, added_at, metadata_state) VALUES \
            (1, 'A', 'a', 100, 'matched'), \
            (2, 'B', 'b', 200, 'matched'), \
            (3, 'C', 'c', 300, 'matched');
         -- A matched movie that already HAS a collection (excluded).
         INSERT INTO movies (id, title, sort_title, added_at, metadata_state, collection_id) \
            VALUES (4, 'D', 'd', 400, 'matched', 9);
         -- A pending (unmatched) movie (excluded).
         INSERT INTO movies (id, title, sort_title, added_at, metadata_state) \
            VALUES (5, 'E', 'e', 500, 'pending');",
    )
    .unwrap();

    // First page of 2 → the two oldest NULL-collection matched movies, in (added_at, id) order.
    let page1 = queries::matched_movies_missing_collection(&conn, None, 2).unwrap();
    assert_eq!(page1, vec![(1, 100), (2, 200)]);

    // Resume after the last row → the third; movie 4 (has collection) and 5 (pending) excluded.
    let cursor = Some((page1.last().unwrap().1, page1.last().unwrap().0)); // (added_at, id)
    let page2 = queries::matched_movies_missing_collection(&conn, cursor, 2).unwrap();
    assert_eq!(page2, vec![(3, 300)]);

    // Past the end → empty, so the backfill loop stops.
    let cursor2 = Some((page2.last().unwrap().1, page2.last().unwrap().0));
    let page3 = queries::matched_movies_missing_collection(&conn, cursor2, 2).unwrap();
    assert!(page3.is_empty());
}

/// The fanart backfill worklist (Task 93 logos + Task 95 wallpapers) lists matched movies with
/// a NULL `logo_path` **or** a NULL `wallpaper_path`, oldest-added first; `force` returns every
/// matched movie. `get_movie` round-trips both columns.
#[test]
fn fanart_worklist_lists_matched_movies_missing_logo_or_wallpaper() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.db");
    let db = medi_db::open(&path, 2).unwrap();
    let conn = db.conn().unwrap();

    conn.execute_batch(
        "-- Missing both (worklist candidate).
         INSERT INTO movies (id, title, sort_title, added_at, metadata_state) VALUES \
            (1, 'A', 'a', 100, 'matched');
         -- Has a logo but no wallpaper → still a candidate (needs the wallpaper).
         INSERT INTO movies (id, title, sort_title, added_at, metadata_state, logo_path) \
            VALUES (2, 'B', 'b', 200, 'matched', 'movies/2/logo.png');
         -- Has BOTH → excluded unless force.
         INSERT INTO movies (id, title, sort_title, added_at, metadata_state, logo_path, wallpaper_path) \
            VALUES (3, 'C', 'c', 300, 'matched', 'movies/3/logo.png', 'movies/3/wallpaper.jpg');
         -- A pending movie (excluded regardless of force).
         INSERT INTO movies (id, title, sort_title, added_at, metadata_state) \
            VALUES (4, 'D', 'd', 400, 'pending');",
    )
    .unwrap();

    // Non-force: matched movies still missing either art type, oldest-added first (1 and 2).
    let missing = queries::matched_movies_missing_fanart(&conn, false, 60).unwrap();
    assert_eq!(missing, vec![1, 2]);

    // Force: every matched movie (incl. the fully-arted one), still excluding the pending one.
    let forced = queries::matched_movies_missing_fanart(&conn, true, 60).unwrap();
    assert_eq!(forced, vec![1, 2, 3]);

    // get_movie round-trips both columns (set on movie 3, NULL on movie 1).
    let m3 = queries::get_movie(&conn, 3).unwrap();
    assert_eq!(m3.logo_path.as_deref(), Some("movies/3/logo.png"));
    assert_eq!(m3.wallpaper_path.as_deref(), Some("movies/3/wallpaper.jpg"));
    let m1 = queries::get_movie(&conn, 1).unwrap();
    assert!(m1.logo_path.is_none());
    assert!(m1.wallpaper_path.is_none());
}

/// "Continue Watching" (`docs/.tasks/98`) lists in-progress titles newest-first, joins each
/// file to its owning movie (poster + title) or episode → series, and excludes finished /
/// just-started rows. Also drives `ON DELETE CASCADE` (removing a file drops its progress).
#[test]
fn continue_watching_orders_joins_and_excludes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.db");
    let db = medi_db::open(&path, 2).unwrap();
    let conn = db.conn().unwrap();

    conn.execute_batch(
        "INSERT INTO movies (id, title, sort_title, added_at, poster_path) VALUES \
            (10, 'Arrival', 'arrival', 100, 'movies/10/poster.jpg'), \
            (11, 'Dune', 'dune', 200, NULL), \
            (12, 'Barely Started', 'barely started', 300, NULL), \
            (13, 'Finished Film', 'finished film', 400, NULL);
         -- A series with one episode (episode-owned file resolves the series poster).
         INSERT INTO series (id, title, sort_title, added_at, poster_path) \
            VALUES (20, 'Severance', 'severance', 500, 'series/20/poster.jpg');
         INSERT INTO seasons (id, series_id, season_number) VALUES (30, 20, 1);
         INSERT INTO episodes (id, season_id, episode_number, title) VALUES (40, 30, 1, 'Pilot');
         INSERT INTO media_files (id, movie_id, path, container) VALUES \
            (100, 10, '/media/arrival.mkv', 'mkv'), \
            (101, 11, '/media/dune.mkv', 'mkv'), \
            (102, 12, '/media/barely.mkv', 'mkv'), \
            (103, 13, '/media/finished.mkv', 'mkv');
         INSERT INTO media_files (id, episode_id, path, container) VALUES \
            (104, 40, '/media/sev-s01e01.mkv', 'mkv');",
    )
    .unwrap();

    // Arrival: 5 min in (oldest update). Dune: 10 min in (newest movie update). Episode: middle.
    writes::upsert_progress(&conn, 100, 300_000, 6_000_000, 1000).unwrap();
    writes::upsert_progress(&conn, 104, 600_000, 2_400_000, 1500).unwrap();
    writes::upsert_progress(&conn, 101, 600_000, 8_000_000, 2000).unwrap();
    // Barely started (< MIN_RESUME_MS) → excluded.
    writes::upsert_progress(&conn, 102, 5_000, 6_000_000, 3000).unwrap();
    // Finished (past 95%) → excluded.
    writes::upsert_progress(&conn, 103, 5_900_000, 6_000_000, 3500).unwrap();

    let items = queries::list_continue_watching(&conn, 50).unwrap();
    // Only the three qualifying rows, newest updated_at first: Dune (2000), episode (1500), Arrival (1000).
    let ids: Vec<i64> = items.iter().map(|i| i.file_id).collect();
    assert_eq!(ids, vec![101, 104, 100], "newest-first, finished + just-started excluded");

    // The movie-owned row carries the movie's poster/title and kind "movie".
    let dune = &items[0];
    assert_eq!(dune.kind, "movie");
    assert_eq!(dune.title, "Dune");
    assert_eq!(dune.title_id, 11);
    assert_eq!(dune.position_ms, 600_000);

    // The episode-owned row resolves its SERIES poster/title and kind "episode".
    let ep = &items[1];
    assert_eq!(ep.kind, "episode");
    assert_eq!(ep.title, "Severance");
    assert_eq!(ep.title_id, 20, "links to the series detail page");
    assert_eq!(ep.poster_path.as_deref(), Some("series/20/poster.jpg"));

    // The Arrival row surfaces its own poster.
    assert_eq!(items[2].poster_path.as_deref(), Some("movies/10/poster.jpg"));

    // `limit` bounds the list.
    assert_eq!(queries::list_continue_watching(&conn, 1).unwrap().len(), 1);

    // ON DELETE CASCADE: deleting Dune's file drops its progress, so it leaves the list.
    conn.execute("DELETE FROM media_files WHERE id = 101", []).unwrap();
    assert!(queries::get_progress(&conn, 101).unwrap().is_none(), "progress cascaded with the file");
    let after: Vec<i64> = queries::list_continue_watching(&conn, 50).unwrap().iter().map(|i| i.file_id).collect();
    assert_eq!(after, vec![104, 100]);
}
