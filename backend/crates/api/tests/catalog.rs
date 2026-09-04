//! Integration tests for the catalog API contract
//! (`docs/.tasks/02-api-contract.md` §Verification).
//!
//! Drives the real router in-process with `tower::ServiceExt::oneshot` against a
//! temp SQLite database seeded with a couple of titles — no port binding, no ffmpeg.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use medi_api::cache::ResponseCache;
use medi_api::{router, AppState};
use medi_core::AppConfig;

/// Build an app backed by a fresh temp DB seeded with two movies and one series.
/// Returns the router plus the tempdir guard (kept alive for the test's duration).
fn seeded_app() -> (axum::Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = AppConfig::default();
    config.config_dir = dir.path().to_path_buf();

    let db = medi_db::open(config.db_path(), 4).unwrap();
    {
        let conn = db.conn().unwrap();
        // Two movies (one Dolby Vision) + one series with one episode+file.
        conn.execute_batch(
            "INSERT INTO movies (id, title, sort_title, year, added_at) \
                VALUES (12, 'Blade Runner 2049', 'blade runner 2049', 2017, 100), \
                       (20, 'Arrival', 'arrival', 2016, 200);
             INSERT INTO series (id, title, sort_title, year, added_at) \
                VALUES (3, 'Severance', 'severance', 2022, 300);
             INSERT INTO seasons (id, series_id, season_number) VALUES (5, 3, 1);
             INSERT INTO episodes (id, season_id, episode_number, title) \
                VALUES (7, 5, 1, 'Good News About Hell');
             -- DV P5 file on the Blade Runner movie.
             INSERT INTO media_files \
                (id, movie_id, path, container, video_codec, width, height, bit_depth, \
                 hdr_type, dv_profile) \
                VALUES (88, 12, '/media/br2049.mkv', 'mkv', 'hevc', 3840, 2160, 10, \
                        'dolbyvision', 5);
             -- Plain SDR H.264 file on Arrival.
             INSERT INTO media_files \
                (id, movie_id, path, container, video_codec, width, height, bit_depth) \
                VALUES (89, 20, '/media/arrival.mp4', 'mp4', 'h264', 1920, 1080, 8);
             -- HDR10 file on the series episode.
             INSERT INTO media_files \
                (id, episode_id, path, container, video_codec, width, height, bit_depth, \
                 hdr_type) \
                VALUES (90, 7, '/media/sev-s01e01.mkv', 'mkv', 'hevc', 3840, 2160, 10, 'hdr10');
             -- Trickplay assets: a tiled-JPG mosaic on file 88 (client-croppable), and a
             -- BIF on file 89 (no client grid). File 90 has none. Drives the meta tests.
             INSERT INTO trickplay_assets \
                (media_file_id, kind, path, interval_ms, tile_w, tile_h, cols, rows, generated_at) \
                VALUES (88, 'tiled_jpg', '/config/trickplay/88.jpg', 10000, 320, 180, 10, 6, 111);
             INSERT INTO trickplay_assets \
                (media_file_id, kind, path, interval_ms, generated_at) \
                VALUES (89, 'bif', '/config/trickplay/89.bif', 10000, 222);",
        )
        .unwrap();
    }

    // A software-only transcode manager keeps the catalog tests self-contained (no GPU,
    // no ffmpeg): catalog routes never touch it, and the stream tests below exercise the
    // decision path without spawning a real transcode.
    let caps = medi_transcode::HwCaps::software_only();
    let transcode = medi_transcode::SessionManager::new(
        dir.path().join("hls"),
        2,
        caps.clone(),
    );
    let state = AppState::new(db, ResponseCache::new(64), config, transcode, caps);
    (router(state), dir)
}

async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

/// Build a fresh app whose MEDIA_DIR is a real temp directory (with a `movies` subdir),
/// for the Phase B library tests. Returns the app, its tempdir guard, and the canonical
/// media root path. No enrichment context is attached.
fn app_with_media() -> (axum::Router, tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let media = dir.path().join("media");
    std::fs::create_dir_all(media.join("movies")).unwrap();
    std::fs::create_dir_all(media.join("tv")).unwrap();

    let mut config = AppConfig::default();
    config.config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config.config_dir).unwrap();
    config.media_dir = media.clone();

    let db = medi_db::open(config.db_path(), 4).unwrap();
    let caps = medi_transcode::HwCaps::software_only();
    let transcode = medi_transcode::SessionManager::new(config.config_dir.join("hls"), 2, caps.clone());
    let state = AppState::new(db, ResponseCache::new(64), config, transcode, caps);
    let canonical = media.canonicalize().unwrap();
    (router(state), dir, canonical)
}

#[tokio::test]
async fn health_is_ok() {
    let (app, _dir) = seeded_app();
    let resp = app
        .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn library_lists_cards_with_poster_and_hdr() {
    let (app, _dir) = seeded_app();
    let resp = app
        .oneshot(
            Request::get("/api/library?limit=50")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );

    let json = body_json(resp).await;
    let items = json["items"].as_array().unwrap();
    // 2 movies + 1 series.
    assert_eq!(items.len(), 3);

    // Default sort is by sort_title: arrival, blade runner 2049, severance.
    assert_eq!(items[0]["title"], "Arrival");
    assert_eq!(items[1]["title"], "Blade Runner 2049");
    assert_eq!(items[1]["kind"], "movie");
    assert_eq!(items[1]["hdr"], "dolbyvision");
    assert_eq!(items[2]["kind"], "series");
    assert_eq!(items[2]["hdr"], "hdr10");
    // A full page (>= limit rows) would carry a cursor; this short page is exhausted.
    assert!(json["next_cursor"].is_null());
}

#[tokio::test]
async fn library_paginates_with_keyset_cursor() {
    let (app, _dir) = seeded_app();

    // Page size 2 → first page has 2 items and a non-null cursor.
    let resp = app
        .clone()
        .oneshot(
            Request::get("/api/library?limit=2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let page1 = body_json(resp).await;
    let items1 = page1["items"].as_array().unwrap();
    assert_eq!(items1.len(), 2);
    let cursor = page1["next_cursor"].as_str().expect("cursor present");

    // Second page resumes after the cursor and returns the remaining item.
    let resp2 = app
        .oneshot(
            Request::get(format!("/api/library?limit=2&cursor={cursor}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let page2 = body_json(resp2).await;
    let items2 = page2["items"].as_array().unwrap();
    assert_eq!(items2.len(), 1);
    // The second page differs from the first (no overlap).
    assert_ne!(items1[0]["title"], items2[0]["title"]);
    assert_ne!(items1[1]["title"], items2[0]["title"]);
    assert_eq!(items2[0]["title"], "Severance");
}

#[tokio::test]
async fn catalog_etag_yields_304() {
    let (app, _dir) = seeded_app();

    // First request: capture the ETag.
    let resp = app
        .clone()
        .oneshot(
            Request::get("/api/movies/12")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let etag = resp
        .headers()
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    // Second request with If-None-Match → 304, no body.
    let resp2 = app
        .oneshot(
            Request::get("/api/movies/12")
                .header(header::IF_NONE_MATCH, &etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::NOT_MODIFIED);
    let bytes = resp2.into_body().collect().await.unwrap().to_bytes();
    assert!(bytes.is_empty());
}

#[tokio::test]
async fn movie_detail_includes_files_and_dv_profile() {
    let (app, _dir) = seeded_app();
    let resp = app
        .oneshot(
            Request::get("/api/movies/12")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["title"], "Blade Runner 2049");
    let files = json["media_files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["id"], 88);
    assert_eq!(files[0]["dv_profile"], 5);
}

#[tokio::test]
async fn unknown_movie_is_404_with_error_envelope() {
    let (app, _dir) = seeded_app();
    let resp = app
        .oneshot(
            Request::get("/api/movies/999")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let json = body_json(resp).await;
    assert_eq!(json["error"]["code"], "not_found");
    assert!(json["error"]["message"].is_string());
}

#[tokio::test]
async fn bad_sort_is_400() {
    let (app, _dir) = seeded_app();
    let resp = app
        .oneshot(
            Request::get("/api/library?sort=bogus")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert_eq!(json["error"]["code"], "bad_request");
}

#[tokio::test]
async fn stream_unknown_file_is_404() {
    let (app, _dir) = seeded_app();
    let resp = app
        .oneshot(
            Request::get("/api/stream/9999")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn stream_dv_to_apple_tv_default_direct_plays() {
    // File 88 is a Dolby Vision P5 movie in an **MKV** container. With no client hints the
    // server assumes the Apple TV 4K baseline (DV-capable, HDR display), so the *video*
    // direct-plays — no GPU/HLS transcode — but AVPlayer cannot open MKV, so the response
    // is a container remux (`mode: direct`, reason `remux_container_or_audio`), served over
    // `/api/direct`. The point of the test is that DV does NOT force a video transcode.
    let (app, _dir) = seeded_app();
    let resp = app
        .oneshot(
            Request::get("/api/stream/88")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["mode"], "direct", "DV video must not be transcoded to HLS");
    assert_eq!(json["reason"], "remux_container_or_audio", "MKV can't direct-play on AVPlayer");
    assert_eq!(json["url"], "/api/direct/88");
}

#[tokio::test]
async fn stream_dv_to_sdr_display_transcodes_via_hls() {
    // Same DV P5 file, but the client reports an SDR display (`sdr=1`) → the server must
    // tone-map, so it starts an HLS transcode session and returns an fMP4 playlist URL.
    // The host is software-only here, so this exercises the software tone-map path.
    //
    // Use the shell `true` builtin as a stand-in ffmpeg so the session spawns without a
    // real transcoder (the CI image always has it). We only assert the returned URL
    // shape, not that segments were produced.
    std::env::set_var("FFMPEG_BIN", "true");
    let (app, _dir) = seeded_app();
    let resp = app
        .oneshot(
            Request::get("/api/stream/88?sdr=1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["mode"], "hls");
    assert_eq!(json["reason"], "dv_p5_sdr_display");
    let url = json["url"].as_str().unwrap();
    assert!(url.starts_with("/api/hls/"));
    assert!(url.ends_with("/index.m3u8"), "fMP4 HLS playlist url: {url}");
}

// ---------------------------------------------------------------------------
// Audio-aware playback decision (Task 70) — per-device passthrough + downmix.
// ---------------------------------------------------------------------------

/// An app seeded with H.264/MP4 files carrying specific audio tracks, so the *video*
/// always direct-plays and only the audio axis drives the decision (`docs/.tasks/70`).
///   file 200: TrueHD 7.1 Atmos default track (lossless bitstream).
///   file 201: E-AC-3 5.1 default track (supported everywhere).
///   file 202: high-bitrate H.264 (40 Mbps) with an AAC track (for the Capped test).
fn audio_app() -> (axum::Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = AppConfig::default();
    config.config_dir = dir.path().to_path_buf();

    let db = medi_db::open(config.db_path(), 4).unwrap();
    {
        let conn = db.conn().unwrap();
        conn.execute_batch(
            "INSERT INTO movies (id, title, sort_title, added_at) \
                VALUES (30, 'TrueHD Movie', 'truehd movie', 100), \
                       (31, 'EAC3 Movie', 'eac3 movie', 200), \
                       (32, 'Big Movie', 'big movie', 300), \
                       (33, 'Multi Audio Movie', 'multi audio movie', 400);
             INSERT INTO media_files (id, movie_id, path, container, video_codec, width, height, bit_depth) \
                VALUES (200, 30, '/media/truehd.mp4', 'mp4', 'h264', 1920, 1080, 8);
             INSERT INTO media_files (id, movie_id, path, container, video_codec, width, height, bit_depth) \
                VALUES (201, 31, '/media/eac3.mp4', 'mp4', 'h264', 1920, 1080, 8);
             INSERT INTO media_files (id, movie_id, path, container, video_codec, width, height, bit_depth, bitrate) \
                VALUES (202, 32, '/media/big.mp4', 'mp4', 'h264', 3840, 2160, 8, 40000000);
             -- A file with TWO audio tracks (an English 5.1 default + a French stereo), so the
             -- audio-track switch (`docs/.tasks/97` Part C) has something to select. Both AC-3,
             -- which a browser can't decode, so the web profile always transcodes → an HLS
             -- session whose key differs per selected track.
             INSERT INTO media_files (id, movie_id, path, container, video_codec, width, height, bit_depth, duration_ms) \
                VALUES (203, 33, '/media/multi.mkv', 'mkv', 'h264', 1920, 1080, 8, 60000);
             -- Default audio tracks.
             INSERT INTO audio_streams (media_file_id, stream_index, codec, channels, immersive, is_default) \
                VALUES (200, 1, 'truehd', 8, 'dolby_atmos', 1);
             INSERT INTO audio_streams (media_file_id, stream_index, codec, channels, immersive, is_default) \
                VALUES (201, 1, 'eac3', 6, 'none', 1);
             INSERT INTO audio_streams (media_file_id, stream_index, codec, channels, immersive, is_default) \
                VALUES (202, 1, 'aac', 2, 'none', 1);
             INSERT INTO audio_streams (media_file_id, stream_index, codec, channels, language, title, immersive, is_default) \
                VALUES (203, 1, 'ac3', 6, 'eng', 'English', 'none', 1), \
                       (203, 2, 'ac3', 2, 'fre', 'Français', 'none', 0);
             -- Embedded chapters (`docs/.tasks/99`) so GET /api/files/:id lists them. Chapter 0
             -- has a generated poster frame (has_image = 1); chapter 1 does not.
             INSERT INTO chapters (media_file_id, ordinal, start_ms, end_ms, title, has_image) \
                VALUES (203, 0, 0, 30000, 'Opening', 1), \
                       (203, 1, 30000, NULL, 'Act One', 0);",
        )
        .unwrap();
    }

    let caps = medi_transcode::HwCaps::software_only();
    let transcode = medi_transcode::SessionManager::new(dir.path().join("hls"), 4, caps.clone());
    let state = AppState::new(db, ResponseCache::new(64), config, transcode, caps);
    (router(state), dir)
}

#[tokio::test]
async fn truehd_direct_plays_to_shield() {
    // Shield bitstreams TrueHD losslessly and opens MP4 → full direct play, no transcode.
    let (app, _dir) = audio_app();
    let resp = app
        .oneshot(Request::get("/api/stream/200?platform=shield").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["mode"], "direct");
    assert_eq!(json["reason"], "direct_play", "Shield bitstreams TrueHD");
    assert_eq!(json["url"], "/api/direct/200");
}

#[tokio::test]
async fn truehd_remuxes_audio_for_apple_tv() {
    // Apple TV can't bitstream TrueHD → video copies, audio must re-encode (a remux). No
    // FFMPEG needed: an audio-only fix is served over /api/direct, not an HLS session.
    std::env::set_var("FFMPEG_BIN", "true");
    let (app, _dir) = audio_app();
    let resp = app
        .oneshot(Request::get("/api/stream/200?platform=appletv").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["mode"], "direct");
    assert_eq!(json["reason"], "remux_container_or_audio", "Apple TV re-encodes TrueHD");
}

#[tokio::test]
async fn low_channel_cap_downmixes() {
    // An E-AC-3 5.1 track to an androidtv default (stereo, no passthrough) exceeds the
    // 2-channel cap → the audio must be downmixed (a remux), even though the codec is fine.
    let (app, _dir) = audio_app();
    let resp = app
        .oneshot(Request::get("/api/stream/201?platform=androidtv").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["reason"], "remux_container_or_audio", "5.1 over a stereo cap downmixes");
}

#[tokio::test]
async fn eac3_direct_plays_to_apple_tv() {
    // Baseline: a supported E-AC-3 5.1 track in MP4 to Apple TV → full direct play.
    let (app, _dir) = audio_app();
    let resp = app
        .oneshot(Request::get("/api/stream/201?platform=appletv").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["reason"], "direct_play");
}

#[tokio::test]
async fn capped_bitrate_forces_hls_transcode() {
    // A 40 Mbps file under an 8 Mbps cap forces a full transcode even though the codec
    // would direct-play (`docs/.tasks/70` §QualityProfile::Capped).
    std::env::set_var("FFMPEG_BIN", "true");
    let (app, _dir) = audio_app();
    let resp = app
        .oneshot(
            Request::get("/api/stream/202?platform=appletv&quality=capped&max_bitrate=8000000")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["mode"], "hls");
    assert_eq!(json["reason"], "bitrate_capped");
    let url = json["url"].as_str().unwrap();
    assert!(url.starts_with("/api/hls/"));
}

#[tokio::test]
async fn force_transcode_promotes_web_direct_play_to_hls() {
    // File 202 is H.264 + AAC + mp4 — it direct-plays for the web profile. When the browser
    // finds that `direct` stream unplayable it re-requests with force_transcode=1, which must
    // flip the decision to an HLS transcode hls.js can always play.
    std::env::set_var("FFMPEG_BIN", "true");
    let (app, _dir) = audio_app();

    // Baseline: web direct-plays this file.
    let resp = app
        .clone()
        .oneshot(Request::get("/api/stream/202?platform=web").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let json = body_json(resp).await;
    assert_eq!(json["mode"], "direct", "H.264/AAC/mp4 direct-plays for web");

    // Forced: same file, force_transcode=1 → HLS.
    let resp = app
        .oneshot(
            Request::get("/api/stream/202?platform=web&force_transcode=1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["mode"], "hls");
    assert_eq!(json["reason"], "forced_transcode");
    assert!(json["url"].as_str().unwrap().starts_with("/api/hls/"));
}

// ---------------------------------------------------------------------------
// Audio-track switching (`docs/.tasks/97` Part C) — /api/files + audio_track select
// ---------------------------------------------------------------------------

#[tokio::test]
async fn files_endpoint_lists_audio_and_subtitle_tracks() {
    // A deep link populates its menus from GET /api/files/:id (audio + subtitles).
    let (app, _dir) = audio_app();
    let resp = app
        .oneshot(Request::get("/api/files/203").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["file_id"], 203);
    let audio = json["audio"].as_array().unwrap();
    assert_eq!(audio.len(), 2, "both audio tracks listed");
    assert_eq!(audio[0]["stream_index"], 1);
    assert_eq!(audio[0]["language"], "eng");
    assert_eq!(audio[0]["is_default"], true);
    assert_eq!(audio[1]["stream_index"], 2);
    assert_eq!(audio[1]["title"], "Français");
    // No subtitle streams on this file → an empty (but present) list.
    assert!(json["subtitles"].as_array().unwrap().is_empty());
    // Chapters (`docs/.tasks/99`) are listed in ordinal order; a missing end_ms is omitted.
    let chapters = json["chapters"].as_array().unwrap();
    assert_eq!(chapters.len(), 2, "both chapters listed");
    assert_eq!(chapters[0]["ordinal"], 0);
    assert_eq!(chapters[0]["start_ms"], 0);
    assert_eq!(chapters[0]["end_ms"], 30000);
    assert_eq!(chapters[0]["title"], "Opening");
    assert_eq!(chapters[1]["title"], "Act One");
    assert!(chapters[1].get("end_ms").is_none(), "NULL end_ms is omitted from JSON");
    // Chapter poster frames (`docs/.tasks/99` Part C): a chapter WITH an image reports
    // `image: true`; one without omits the field (skip-false) so the client shows no scene card.
    assert_eq!(chapters[0]["image"], true, "chapter 0 has a generated frame");
    assert!(chapters[1].get("image").is_none(), "no image ⇒ field omitted");
}

#[tokio::test]
async fn chapter_image_404s_when_absent() {
    // The chapter-image route serves from disk; with nothing generated it's a clean 404 the
    // client treats as "no image" and falls back (`docs/.tasks/99` Part C).
    let (app, _dir) = audio_app();
    let resp = app
        .oneshot(Request::get("/api/chapters/203/image/0").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn audio_track_selection_yields_a_distinct_hls_session() {
    // Selecting track 2 vs the default (track 1) must transcode a DIFFERENT source track and
    // therefore spawn a DISTINCT session (a different session id in the returned HLS url). The
    // `FFMPEG_BIN=cat` fake keeps each session alive so the two starts don't collapse.
    std::env::set_var("FFMPEG_BIN", "cat");
    let (app, _dir) = audio_app();

    // Default track: browser (web) can't decode AC-3 / open MKV → an HLS transcode session.
    let resp = app
        .clone()
        .oneshot(Request::get("/api/stream/203?platform=web").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let json = body_json(resp).await;
    assert_eq!(json["mode"], "hls");
    let default_url = json["url"].as_str().unwrap().to_string();
    assert!(default_url.starts_with("/api/hls/"));

    // Explicit selection of the same default track (stream_index=1) — this DOES set a source
    // map, so it is its own session, distinct from the ffmpeg-default one above.
    let resp = app
        .clone()
        .oneshot(Request::get("/api/stream/203?platform=web&audio_track=1").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let track1_url = body_json(resp).await["url"].as_str().unwrap().to_string();

    // The French track (stream_index=2) → a different mapped source → a different session.
    let resp = app
        .oneshot(Request::get("/api/stream/203?platform=web&audio_track=2").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let json = body_json(resp).await;
    assert_eq!(json["mode"], "hls");
    let track2_url = json["url"].as_str().unwrap().to_string();

    assert_ne!(track1_url, track2_url, "a different audio track must be a distinct session");
    std::env::remove_var("FFMPEG_BIN");
}

// ---------------------------------------------------------------------------
// GET /api/trickplay/:file_id/meta  (Phase 5 Part A: scrub-thumbnail geometry)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Metadata enrichment endpoints (Phase A) — behavior with no provider configured
// ---------------------------------------------------------------------------

#[tokio::test]
async fn metadata_refresh_501_without_provider() {
    // seeded_app has no enrichment context → the manual metadata endpoints return 501
    // not_implemented (distinct from a failure), so a client knows metadata is simply off.
    let (app, _dir) = seeded_app();
    let resp = app
        .oneshot(
            Request::post("/api/movies/12/refresh")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    let json = body_json(resp).await;
    assert_eq!(json["error"]["code"], "not_implemented");
}

#[tokio::test]
async fn metadata_matches_501_without_provider() {
    let (app, _dir) = seeded_app();
    let resp = app
        .oneshot(
            Request::get("/api/movies/12/matches")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
}

// ---------------------------------------------------------------------------
// Genres & discovery (`docs/.tasks/91` Phase A)
// ---------------------------------------------------------------------------

/// An app seeded with three genres joined to the `seeded_app` titles: Sci-Fi (id 878) on
/// both movies + the series (count 3), Drama (18) on Arrival only (count 1), and Comedy
/// (35) with no titles (excluded from the list). Reuses `seeded_app`'s catalog rows.
fn genre_app() -> (axum::Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = AppConfig::default();
    config.config_dir = dir.path().to_path_buf();

    let db = medi_db::open(config.db_path(), 4).unwrap();
    {
        let conn = db.conn().unwrap();
        conn.execute_batch(
            "INSERT INTO movies (id, title, sort_title, year, added_at, metadata_state, logo_path) \
                VALUES (12, 'Blade Runner 2049', 'blade runner 2049', 2017, 100, 'matched', NULL), \
                       (20, 'Arrival', 'arrival', 2016, 200, 'matched', 'movies/20/logo.png');
             INSERT INTO series (id, title, sort_title, year, added_at, metadata_state) \
                VALUES (3, 'Severance', 'severance', 2022, 300, 'matched');
             INSERT INTO genres (id, name) VALUES (878, 'Science Fiction'), (18, 'Drama'), (35, 'Comedy');
             INSERT INTO movie_genres (movie_id, genre_id) VALUES (12, 878), (20, 878), (20, 18);
             INSERT INTO series_genres (series_id, genre_id) VALUES (3, 878);",
        )
        .unwrap();
    }

    let caps = medi_transcode::HwCaps::software_only();
    let transcode = medi_transcode::SessionManager::new(dir.path().join("hls"), 2, caps.clone());
    let state = AppState::new(db, ResponseCache::new(64), config, transcode, caps);
    (router(state), dir)
}

#[tokio::test]
async fn genres_list_ranks_by_count_and_excludes_empty() {
    let (app, _dir) = genre_app();
    let resp = app
        .oneshot(Request::get("/api/genres").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // ETag'd like the rest of the catalog.
    assert!(resp.headers().get(header::ETAG).is_some());
    let json = body_json(resp).await;
    let arr = json.as_array().unwrap();
    // Comedy (0 titles) is excluded → only Sci-Fi and Drama.
    assert_eq!(arr.len(), 2);
    // Sci-Fi has 3 titles (2 movies + 1 series), Drama has 1 → Sci-Fi first.
    assert_eq!(arr[0]["id"], 878);
    assert_eq!(arr[0]["name"], "Science Fiction");
    assert_eq!(arr[0]["count"], 3);
    assert_eq!(arr[1]["id"], 18);
    assert_eq!(arr[1]["count"], 1);
}

#[tokio::test]
async fn movie_detail_carries_its_genres() {
    let (app, _dir) = genre_app();
    let resp = app
        .oneshot(Request::get("/api/movies/20").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    // Arrival is joined to Drama (18) + Science Fiction (878); names come back sorted.
    let genres = json["genres"].as_array().unwrap();
    let names: Vec<&str> = genres.iter().map(|g| g["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["Drama", "Science Fiction"]);
    assert_eq!(genres[0]["id"], 18);
}

#[tokio::test]
async fn movie_detail_surfaces_logo_path() {
    // The flattened Movie carries its fanart.tv `logo_path` (Task 93) so the client can
    // resolve it via imageUrl(); a movie with no logo omits/nulls it.
    let (app, _dir) = genre_app();
    // Arrival (20) has a logo.
    let resp = app
        .clone()
        .oneshot(Request::get("/api/movies/20").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["logo_path"], "movies/20/logo.png");

    // Blade Runner 2049 (12) has no logo → null.
    let resp2 = app
        .oneshot(Request::get("/api/movies/12").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let json2 = body_json(resp2).await;
    assert!(json2["logo_path"].is_null(), "no logo → logo_path null");
}

#[tokio::test]
async fn movie_detail_returns_collection_and_siblings() {
    // Two matched movies of one franchise (collection 500). The detail for one returns its
    // `collection` plus the OTHER in-library movie as `collection_movies`.
    let dir = tempfile::tempdir().unwrap();
    let mut config = AppConfig::default();
    config.config_dir = dir.path().to_path_buf();
    let db = medi_db::open(config.db_path(), 4).unwrap();
    {
        let conn = db.conn().unwrap();
        conn.execute_batch(
            "INSERT INTO collections (id, name) VALUES (500, 'Thor Collection');
             INSERT INTO movies (id, title, sort_title, year, added_at, metadata_state, collection_id) \
                VALUES (40, 'Thor', 'thor', 2011, 500, 'matched', 500), \
                       (41, 'Thor Ragnarok', 'thor ragnarok', 2017, 600, 'matched', 500);",
        )
        .unwrap();
    }
    let caps = medi_transcode::HwCaps::software_only();
    let transcode = medi_transcode::SessionManager::new(dir.path().join("hls"), 2, caps.clone());
    let state = AppState::new(db, ResponseCache::new(64), config, transcode, caps);
    let app = router(state);

    let resp = app
        .oneshot(Request::get("/api/movies/41").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["collection"]["id"], 500);
    assert_eq!(json["collection"]["name"], "Thor Collection");
    let siblings = json["collection_movies"].as_array().unwrap();
    // The current movie (41) is excluded; only Thor (40) remains.
    assert_eq!(siblings.len(), 1);
    assert_eq!(siblings[0]["id"], 40);
    assert_eq!(siblings[0]["title"], "Thor");
}

#[tokio::test]
async fn movie_detail_orders_files_best_first() {
    // A movie with two copies: a 1080p SDR file inserted FIRST (lower id) and a 2160p HDR10
    // file inserted second. Best-first ordering (resolution → HDR → bitrate) must put the
    // 2160p file at [0] despite its higher id, so a client's `media_files[0]` plays the best.
    let dir = tempfile::tempdir().unwrap();
    let mut config = AppConfig::default();
    config.config_dir = dir.path().to_path_buf();
    let db = medi_db::open(config.db_path(), 4).unwrap();
    {
        let conn = db.conn().unwrap();
        conn.execute_batch(
            "INSERT INTO movies (id, title, sort_title, year, added_at) \
                VALUES (30, 'Dune', 'dune', 2021, 400);
             -- 1080p SDR, inserted first (id 100).
             INSERT INTO media_files \
                (id, movie_id, path, container, video_codec, width, height, bit_depth, bitrate) \
                VALUES (100, 30, '/media/dune-1080p.mkv', 'mkv', 'h264', 1920, 1080, 8, 8000000);
             -- 2160p HDR10, inserted second (id 101).
             INSERT INTO media_files \
                (id, movie_id, path, container, video_codec, width, height, bit_depth, bitrate, hdr_type) \
                VALUES (101, 30, '/media/dune-2160p.mkv', 'mkv', 'hevc', 3840, 2160, 10, 40000000, 'hdr10');",
        )
        .unwrap();
    }
    let caps = medi_transcode::HwCaps::software_only();
    let transcode = medi_transcode::SessionManager::new(dir.path().join("hls"), 2, caps.clone());
    let state = AppState::new(db, ResponseCache::new(64), config, transcode, caps);
    let app = router(state);

    let resp = app
        .oneshot(Request::get("/api/movies/30").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let files = json["media_files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    // Best (2160p HDR10) first, despite being inserted later / having the higher id.
    assert_eq!(files[0]["id"], 101);
    assert_eq!(files[0]["height"], 2160);
    assert_eq!(files[1]["id"], 100);
}

#[tokio::test]
async fn genre_titles_returns_library_page_shape() {
    let (app, _dir) = genre_app();
    // Sci-Fi (878) contains both movies and the series.
    let resp = app
        .clone()
        .oneshot(Request::get("/api/genres/878?limit=50").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    // Same LibraryItem shape as /api/library: kind/id/title present.
    assert!(items.iter().all(|i| i["kind"].is_string() && i["id"].is_number()));
    assert!(json["next_cursor"].is_null(), "short page is exhausted");

    // Drama (18) is Arrival only.
    let resp2 = app
        .oneshot(Request::get("/api/genres/18").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let json2 = body_json(resp2).await;
    let items2 = json2["items"].as_array().unwrap();
    assert_eq!(items2.len(), 1);
    assert_eq!(items2[0]["title"], "Arrival");
}

#[tokio::test]
async fn genre_titles_paginates_with_shared_cursor_codec() {
    let (app, _dir) = genre_app();
    // Page size 2 over Sci-Fi's 3 titles → first page carries a cursor.
    let resp = app
        .clone()
        .oneshot(Request::get("/api/genres/878?limit=2").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let page1 = body_json(resp).await;
    assert_eq!(page1["items"].as_array().unwrap().len(), 2);
    let cursor = page1["next_cursor"].as_str().expect("cursor present");

    let resp2 = app
        .oneshot(
            Request::get(format!("/api/genres/878?limit=2&cursor={cursor}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let page2 = body_json(resp2).await;
    assert_eq!(page2["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn library_rows_has_recently_added_and_genre_rows() {
    let (app, _dir) = genre_app();
    let resp = app
        .oneshot(Request::get("/api/library/rows").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().get(header::ETAG).is_some());
    let json = body_json(resp).await;
    let rows = json["rows"].as_array().unwrap();
    // "Recently Added" first, then the two nonempty genres by count (Sci-Fi, Drama).
    assert_eq!(rows[0]["key"], "recently_added");
    assert!(rows[0]["genre_id"].is_null());
    assert!(!rows[0]["items"].as_array().unwrap().is_empty());
    let genre_keys: Vec<&str> = rows[1..].iter().map(|r| r["key"].as_str().unwrap()).collect();
    assert!(genre_keys.contains(&"genre:878"));
    assert!(genre_keys.contains(&"genre:18"));
    // The Sci-Fi row carries its genre_id for the "See all →" link.
    let scifi = rows.iter().find(|r| r["key"] == "genre:878").unwrap();
    assert_eq!(scifi["genre_id"], 878);
    assert_eq!(scifi["title"], "Science Fiction");
}

// ---------------------------------------------------------------------------
// Person pages (`docs/.tasks/91` Phase B)
// ---------------------------------------------------------------------------

/// An app seeded with an enriched person (Amy Adams) credited on two movies, plus a second
/// person on an uncredited-to-Amy movie, so the filmography test can assert exclusion.
fn person_app() -> (axum::Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = AppConfig::default();
    config.config_dir = dir.path().to_path_buf();

    let db = medi_db::open(config.db_path(), 4).unwrap();
    {
        let conn = db.conn().unwrap();
        conn.execute_batch(
            "INSERT INTO movies (id, title, sort_title, year, added_at, poster_path) \
                VALUES (12, 'Arrival', 'arrival', 2016, 100, 'movies/12/poster.jpg'), \
                       (20, 'Nocturnal', 'nocturnal', 2016, 200, NULL), \
                       (30, 'The Departed', 'departed', 2006, 50, NULL);
             INSERT INTO people (id, name, tmdb_id, photo_path, biography) \
                VALUES (7, 'Amy Adams', 9273, 'people/7/photo.jpg', 'An American actress.'), \
                       (8, 'Leonardo DiCaprio', 6193, NULL, NULL);
             INSERT INTO credits (person_id, movie_id, role, ord) VALUES (7, 12, 'actor', 0);
             INSERT INTO credits (person_id, movie_id, role, ord) VALUES (7, 20, 'actor', 0);
             INSERT INTO credits (person_id, movie_id, role, ord) VALUES (8, 30, 'actor', 0);",
        )
        .unwrap();
    }

    let caps = medi_transcode::HwCaps::software_only();
    let transcode = medi_transcode::SessionManager::new(dir.path().join("hls"), 2, caps.clone());
    let state = AppState::new(db, ResponseCache::new(64), config, transcode, caps);
    (router(state), dir)
}

#[tokio::test]
async fn person_page_returns_meta_and_filmography() {
    let (app, _dir) = person_app();
    let resp = app
        .oneshot(Request::get("/api/people/7").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().get(header::ETAG).is_some());
    let json = body_json(resp).await;
    assert_eq!(json["id"], 7);
    assert_eq!(json["name"], "Amy Adams");
    assert_eq!(json["tmdb_id"], 9273);
    assert_eq!(json["biography"], "An American actress.");
    // Stored photo_path is surfaced as an /api/images URL.
    assert_eq!(json["photo"], "/api/images/people/7/photo.jpg");
    // Filmography: her two movies, newest-added first (Nocturnal 200 before Arrival 100).
    let films = json["filmography"].as_array().unwrap();
    assert_eq!(films.len(), 2);
    assert_eq!(films[0]["title"], "Nocturnal");
    assert_eq!(films[1]["title"], "Arrival");
    // The Departed (credited to someone else) is excluded.
    assert!(!films.iter().any(|f| f["title"] == "The Departed"));
    // A LibraryItem-shaped tile: the poster URL is resolved for Arrival.
    let arrival = films.iter().find(|f| f["title"] == "Arrival").unwrap();
    assert_eq!(arrival["poster"], "/api/images/movies/12/poster.jpg");
}

#[tokio::test]
async fn person_page_unknown_is_404() {
    let (app, _dir) = person_app();
    let resp = app
        .oneshot(Request::get("/api/people/999").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let json = body_json(resp).await;
    assert_eq!(json["error"]["code"], "not_found");
}

#[tokio::test]
async fn metadata_backfill_501_without_provider() {
    // No enrichment context → the backfill trigger returns 501 (metadata is off), matching
    // the other manual metadata endpoints.
    let (app, _dir) = seeded_app();
    let resp = app
        .oneshot(Request::post("/api/metadata/backfill").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    let json = body_json(resp).await;
    assert_eq!(json["error"]["code"], "not_implemented");
}

// ---------------------------------------------------------------------------
// Libraries CRUD + MEDIA_DIR containment (Phase B)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn libraries_seed_absent_until_created() {
    // A fresh app with no seeding done here has no libraries.
    let (app, _guard, _media) = app_with_media();
    let resp = app
        .oneshot(Request::get("/api/libraries").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert!(json.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn create_library_inside_media_succeeds() {
    let (app, _guard, media) = app_with_media();
    let folder = media.join("movies");
    let body = serde_json::json!({
        "name": "Films",
        "kind": "movie",
        "folders": [folder.to_string_lossy()],
    });
    let resp = app
        .oneshot(
            Request::post("/api/libraries")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = body_json(resp).await;
    assert_eq!(json["name"], "Films");
    assert_eq!(json["kind"], "movie");
    assert_eq!(json["folders"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn create_library_outside_media_is_400() {
    let (app, _guard, _media) = app_with_media();
    // A path clearly outside MEDIA_DIR (a second temp dir).
    let outside = tempfile::tempdir().unwrap();
    let body = serde_json::json!({
        "name": "Escape",
        "kind": "movie",
        "folders": [outside.path().to_string_lossy()],
    });
    let resp = app
        .oneshot(
            Request::post("/api/libraries")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert_eq!(json["error"]["code"], "bad_request");
}

#[tokio::test]
async fn create_library_dotdot_escape_is_400() {
    let (app, _guard, media) = app_with_media();
    // A `..` traversal out of MEDIA_DIR resolves outside → rejected.
    let escape = media.join("movies").join("..").join("..");
    let body = serde_json::json!({
        "name": "Escape",
        "kind": "movie",
        "folders": [escape.to_string_lossy()],
    });
    let resp = app
        .oneshot(
            Request::post("/api/libraries")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn library_bad_kind_is_400() {
    let (app, _guard, media) = app_with_media();
    let body = serde_json::json!({
        "name": "Weird",
        "kind": "audiobook",
        "folders": [media.join("movies").to_string_lossy()],
    });
    let resp = app
        .oneshot(
            Request::post("/api/libraries")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn delete_library_returns_204() {
    let (app, _guard, media) = app_with_media();
    // Create then delete.
    let body = serde_json::json!({
        "name": "Films", "kind": "movie",
        "folders": [media.join("movies").to_string_lossy()],
    });
    let created = app
        .clone()
        .oneshot(
            Request::post("/api/libraries")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let id = body_json(created).await["id"].as_i64().unwrap();

    let resp = app
        .oneshot(
            Request::delete(format!("/api/libraries/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn patch_missing_library_is_404() {
    let (app, _guard, _media) = app_with_media();
    let resp = app
        .oneshot(
            Request::patch("/api/libraries/999")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"name":"x"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn trickplay_meta_returns_grid_for_tiled_jpg() {
    let (app, _dir) = seeded_app();
    let resp = app
        .oneshot(
            Request::get("/api/trickplay/88/meta")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );
    let json = body_json(resp).await;
    assert_eq!(json["file_id"], 88);
    assert_eq!(json["kind"], "tiled_jpg");
    assert_eq!(json["interval_ms"], 10000);
    assert_eq!(json["tile_w"], 320);
    assert_eq!(json["tile_h"], 180);
    assert_eq!(json["cols"], 10);
    assert_eq!(json["rows"], 6);
}

#[tokio::test]
async fn trickplay_meta_404s_for_bif_asset() {
    // A BIF row has no client-croppable grid → 404 so the player falls back cleanly.
    let (app, _dir) = seeded_app();
    let resp = app
        .oneshot(
            Request::get("/api/trickplay/89/meta")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn trickplay_meta_404s_when_absent() {
    // File 90 has no trickplay asset at all.
    let (app, _dir) = seeded_app();
    let resp = app
        .oneshot(
            Request::get("/api/trickplay/90/meta")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn trickplay_sprite_static_route_still_served() {
    // The `:file` sprite route and the `:file_id/meta` route coexist (different segment
    // counts). A request for the image path hits the sprite handler (404 here since no
    // file exists on disk in the test tempdir), NOT the JSON meta handler, and the router
    // builds without a wildcard-nest conflict panic.
    let (app, _dir) = seeded_app();
    let resp = app
        .oneshot(
            Request::get("/api/trickplay/88.jpg")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // No file on disk → ServeDir 404. (If the /meta route had wrongly captured this,
    // we'd get a JSON 404 from the DB handler; either way it's 404, but the point of
    // this test is that the route table accepts the static path without a panic.)
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Playback progress + Continue Watching (`docs/.tasks/98`)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn progress_put_then_get_and_continue_watching() {
    // seeded_app: movie files 88 (Blade Runner) + 89 (Arrival), episode file 90 (Severance).
    let (app, _dir) = seeded_app();

    // No progress yet → GET is 204 (no body).
    let resp = app
        .clone()
        .oneshot(Request::get("/api/progress/88").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // PUT ~5 min into Blade Runner (file 88) → 204.
    let put = app
        .clone()
        .oneshot(
            Request::put("/api/progress/88")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"position_ms":300000,"duration_ms":6000000}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(put.status(), StatusCode::NO_CONTENT);

    // The beacon flush path (POST, same body) is accepted too — sendBeacon only sends POST.
    let post = app
        .clone()
        .oneshot(
            Request::post("/api/progress/88")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"position_ms":300000,"duration_ms":6000000}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(post.status(), StatusCode::NO_CONTENT, "POST (sendBeacon) is accepted");

    // GET now returns the saved position, not finished (5 min of 100 min).
    let resp = app
        .clone()
        .oneshot(Request::get("/api/progress/88").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["position_ms"], 300000);
    assert_eq!(json["duration_ms"], 6000000);
    assert_eq!(json["finished"], false);

    // PUT the episode file (90) too, a bit later, so ordering is deterministic.
    let put = app
        .clone()
        .oneshot(
            Request::put("/api/progress/90")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"position_ms":600000,"duration_ms":2400000}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(put.status(), StatusCode::NO_CONTENT);

    // Continue Watching lists both, newest write first (episode 90 before movie 88).
    let resp = app
        .clone()
        .oneshot(Request::get("/api/continue-watching").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let items = body_json(resp).await;
    let arr = items.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    // The episode file resolves to its series (kind "episode", title "Severance").
    assert_eq!(arr[0]["file_id"], 90);
    assert_eq!(arr[0]["kind"], "episode");
    assert_eq!(arr[0]["title"], "Severance");
    assert_eq!(arr[0]["position_ms"], 600000);
    // The movie file carries kind "movie" + its own title.
    assert_eq!(arr[1]["file_id"], 88);
    assert_eq!(arr[1]["kind"], "movie");
    assert_eq!(arr[1]["title"], "Blade Runner 2049");

    // A title watched past ~95% is marked finished and drops off the row.
    let put = app
        .clone()
        .oneshot(
            Request::put("/api/progress/88")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"position_ms":5900000,"duration_ms":6000000}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(put.status(), StatusCode::NO_CONTENT);

    let resp = app
        .clone()
        .oneshot(Request::get("/api/progress/88").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(body_json(resp).await["finished"], true);

    let resp = app
        .oneshot(Request::get("/api/continue-watching").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let arr = body_json(resp).await;
    let ids: Vec<i64> = arr.as_array().unwrap().iter().map(|i| i["file_id"].as_i64().unwrap()).collect();
    assert_eq!(ids, vec![90], "finished title drops from Continue Watching");
}

// ---------------------------------------------------------------------------
// Web SPA fallback (Task 80) — same-origin serving of the browser client at `/`.
// ---------------------------------------------------------------------------

/// Build an app whose `web_dir` is a temp directory containing a marker `index.html` and
/// one hashed asset, so the SPA fallback has something real to serve.
fn app_with_web() -> (axum::Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = AppConfig::default();
    config.config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config.config_dir).unwrap();
    let web = dir.path().join("web");
    std::fs::create_dir_all(web.join("assets")).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html><title>medi</title><div id=root></div>").unwrap();
    std::fs::write(web.join("assets/app.abc123.js"), "console.log('medi')").unwrap();
    config.web_dir = web;

    let db = medi_db::open(config.db_path(), 4).unwrap();
    let caps = medi_transcode::HwCaps::software_only();
    let transcode = medi_transcode::SessionManager::new(config.config_dir.join("hls"), 2, caps.clone());
    let state = AppState::new(db, ResponseCache::new(64), config, transcode, caps);
    (router(state), dir)
}

#[tokio::test]
async fn web_root_serves_spa_index() {
    let (app, _dir) = app_with_web();
    let resp = app
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let html = String::from_utf8_lossy(&bytes);
    assert!(html.contains("id=root"), "root serves the SPA shell: {html}");
}

#[tokio::test]
async fn web_deep_link_returns_shell_with_200() {
    // A client-router deep link has no matching file → history fallback to index.html with
    // a 200 (NOT a 404), so the SPA can boot and route on the path.
    let (app, _dir) = app_with_web();
    let resp = app
        .oneshot(Request::get("/movie/1").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "deep link is history-fallback, not 404");
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&bytes).contains("id=root"));
}

#[tokio::test]
async fn web_serves_hashed_asset() {
    let (app, _dir) = app_with_web();
    let resp = app
        .oneshot(Request::get("/assets/app.abc123.js").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn api_routes_are_not_shadowed_by_web_fallback() {
    // The SPA fallback must never capture a defined `/api/*` route: health still returns
    // "ok" and the catalog still returns JSON, not the HTML shell. (The fallback fires
    // only when no route above matched.)
    let (app, _dir) = app_with_web();
    let health = app
        .clone()
        .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
    let bytes = health.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], b"ok", "api/health wins over the web fallback");

    // The catalog returns JSON (an empty page here), not the HTML shell.
    let library = app
        .oneshot(Request::get("/api/library").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(library.status(), StatusCode::OK);
    let ct = library
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("application/json"), "catalog stays JSON, not the SPA HTML: {ct}");
}
