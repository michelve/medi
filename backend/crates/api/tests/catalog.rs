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
