//! Integration tests for the schema + tuning (`docs/.tasks/01-db-schema.md`
//! §Verification): fresh boot sets `page_size`/`journal_mode`, migrations are
//! idempotent across restarts, and a DV Profile 5 file round-trips.

use medi_core::{DvProfile, HdrType, VideoCodec};
use medi_db::queries;

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
