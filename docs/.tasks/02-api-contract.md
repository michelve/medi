# 02 — REST API Contract

> Cross-cutting task. Defines every HTTP endpoint the TV client consumes. Lives in
> `backend/crates/api`. The `client/packages/api-client` types are generated from this.
> **Gap this closes:** the README says clients "consume normalized data" but never lists
> a single endpoint.

## Purpose

A small, read-heavy JSON + HLS API served by axum. No authentication (intentional — LAN
appliance). Responses are cached in a moka LRU and carry `ETag` for client-side caching.

## Requirements

- All catalog responses are JSON, `Content-Type: application/json`.
- Keyset pagination for large lists (no deep `OFFSET`).
- The stream endpoint decides direct-play vs transcode using `media_files` metadata
  (see `01-db-schema.md`) and the decision table in `20-phase2-hwa-transcode.md`.
- Transcoded output is HLS (playlist + segments) produced by the `transcode` crate.
- Generated assets (`/config/previews`, `/config/trickplay`) are served by dedicated routes.

## Packages / crates

- `axum`, `tokio`, `tower`, `tower-http` (compression, CORS, `ServeDir` for static assets)
- `serde`, `serde_json`
- `moka` (async LRU response cache)

## Endpoints

| Method & Path | Purpose | Notes |
|---|---|---|
| `GET /api/library?cursor=&limit=&sort=` | Paginated unified catalog (movies + series cards) | Keyset cursor; `sort=sort_title\|added_at`. Cached. |
| `GET /api/movies/:id` | Movie detail + media_files + credits | Cached; ETag. |
| `GET /api/series/:id` | Series detail + seasons + episodes + credits | Cached; ETag. |
| `GET /api/stream/:file_id` | Playback decision for a media file | Returns `{ "mode": "direct" \| "hls", "url": ... }`. |
| `GET /api/direct/:file_id` | Direct-play byte-range stream of the source | Supports `Range`; no transcode. |
| `GET /api/hls/:session_id/index.m3u8` | HLS master/media playlist for a transcode session | Session created by `/api/stream`. |
| `GET /api/hls/:session_id/:segment.ts` | HLS media segment | Served as generated. |
| `GET /api/preview/:file_id` | 720p silent hover clip (mp4) | From `/config/previews`; 404 if not yet generated. |
| `GET /api/trickplay/:file_id` | Trickplay sprite (BIF or tiled JPG) + metadata | From `/config/trickplay`. |
| `GET /api/images/*path` | Posters / backdrops | `ServeDir` over allowed image roots. |
| `GET /api/health` | Liveness | For Docker healthcheck. |

## Representative response shapes

`GET /api/library`
```json
{
  "items": [
    { "kind": "movie",  "id": 12, "title": "Blade Runner 2049", "year": 2017,
      "poster": "/api/images/movies/12/poster.jpg", "hdr": "dolbyvision" },
    { "kind": "series", "id": 3,  "title": "Severance", "year": 2022,
      "poster": "/api/images/series/3/poster.jpg", "hdr": "hdr10" }
  ],
  "next_cursor": "eyJzb3J0X3RpdGxlIjoiQ..."   // null when exhausted
}
```

`GET /api/stream/:file_id`
```json
{
  "file_id": 88,
  "mode": "hls",                       // or "direct"
  "reason": "dv_profile_5_sdr_display",// why transcode was chosen (for debugging/logs)
  "url": "/api/hls/9f2c.../index.m3u8"
}
```

## Caching & error model

- Catalog GETs: store serialized body in moka keyed by path+query; attach `ETag`
  (hash of body). Honor `If-None-Match` → `304`. Invalidate cache keys on ingest write.
- Errors: JSON `{ "error": { "code": "not_found", "message": "..." } }` with proper status
  (`404`, `409` for busy transcode session, `503` while a scan holds a write lock rarely).
- No auth headers, no cookies. Bind to LAN; document that exposure to WAN is unsupported.

## Sub-tasks

1. Define axum router with the routes above; group catalog vs stream vs static.
2. Implement keyset pagination cursor (base64 of last sort key + id).
3. Implement the moka cache layer + ETag middleware for catalog routes.
4. `/api/stream` calls `transcode` crate's decision function; direct vs HLS.
5. Serve `/config/previews`, `/config/trickplay`, and image roots via `tower-http::ServeDir`.
6. Emit an OpenAPI/JSON description (optional) so `client/packages/api-client` types stay in sync.

## Verification

- `curl /api/health` → `200`.
- `curl /api/library?limit=50` returns items + `next_cursor`; second page differs.
- `curl -I` a catalog route twice → second with `If-None-Match` yields `304`.
- `curl /api/stream/<dv5_file>` returns `mode:"hls"`; `<h264_sdr_file>` returns `mode:"direct"`.
