# medi

A self-hosted media server. Rust backend, hardware-accelerated transcode, browser + TV clients.

## What it does

- Scans your movie/TV libraries and enriches them with metadata (TMDB / fanart.tv).
- Streams to a browser (React SPA), Apple TV, and Android TV / Shield.
- Direct-plays when the client can, transcodes when it can't — HLS with NVENC / QSV / VA-API or software fallback.
- Client-rendered subtitles (SRT/VTT/ASS/PGS/VobSub), chapters, scene selection, and resume.

## Run (Docker)

```bash
docker compose -f docker/compose.dev.yml up --build -d
```

Then open <http://localhost:8096>.

Point it at your media with a bind mount (edit `docker/compose.dev.yml`, or set `MEDI_MEDIA`):

```bash
MEDI_MEDIA=/path/to/movies docker compose -f docker/compose.dev.yml up --build -d
```

## Develop

Backend (Rust workspace under `backend/`):

```bash
cargo build   --manifest-path backend/Cargo.toml
cargo test    --manifest-path backend/Cargo.toml
```

Web SPA (`client/apps/web`), proxies `/api` to the running backend:

```bash
cd client && yarn web:dev     # http://localhost:5173
```

## Layout

| Path | What |
|---|---|
| `backend/crates/` | api · ingest · transcode · assets · metadata · db · core |
| `backend/migrations/` | refinery SQL migrations (V1…) |
| `client/apps/web` | browser SPA (Vite + React) |
| `client/apps/tv` | Apple TV / Android TV app |
| `docker/` | dev + release images, compose files |
| `docs/.tasks/` | numbered task specs (the design record) |

## License

[AGPL-3.0](LICENSE). If you run a modified medi as a network service, you must make your source available to its users.
