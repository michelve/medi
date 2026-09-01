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
                VALUES (90, 7, '/media/sev-s01e01.mkv', 'mkv', 'hevc', 3840, 2160, 10, 'hdr10');",
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
    // File 88 is a Dolby Vision P5 movie. With no client hints the server assumes the
    // Apple TV 4K baseline (DV-capable, HDR display) → direct play, no transcode.
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
    assert_eq!(json["mode"], "direct");
    assert_eq!(json["reason"], "direct_play");
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
