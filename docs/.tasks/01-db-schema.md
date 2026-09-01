# 01 — Database Schema & SQLite Tuning

> Cross-cutting task. Defines the SQLite schema, the mandatory PRAGMA tuning, migrations,
> and indexing. Lives in the `backend/crates/db` crate. Referenced by `ingest`, `api`,
> `assets`. **Gap this closes:** the README/spec assert SQLite tuning but never give a schema.

## Purpose

Provide the normalized relational model for the library (movies, series/seasons/episodes,
media files, people/credits) plus the generated-asset references (preview clips, trickplay),
and configure SQLite for NVMe-optimized, read-heavy media-server workloads.

## Requirements

- Single embedded `library.db` at `/config/library.db`.
- **64 KB page size** and **WAL** are mandatory (README §Relational Database Engine).
- Store exact Dolby Vision profile data — it drives the transcode pipeline downstream.
- Schema and pragmas applied via idempotent migrations at container boot.
- Optimized for fast grid/scroll reads (covering indexes, pagination-friendly ordering).

## Packages / crates

- `rusqlite` (bundled SQLite; enable `bundled` + `serde_json` features as needed)
- `r2d2` + `r2d2_sqlite` — connection pool
- `refinery` (with the `rusqlite` feature) — embedded SQL migrations
- `serde` — for row → DTO mapping shared with `api`

## PRAGMA block (apply on every connection at pool-checkout, except page_size)

```sql
-- Applied ONCE, on a fresh database, BEFORE the first table is created:
PRAGMA page_size = 65536;          -- 64 KB pages, aligns to NVMe geometry

-- Applied on the database (persisted) at first migration:
PRAGMA journal_mode = WAL;         -- concurrent readers + single writer
PRAGMA auto_vacuum = INCREMENTAL;

-- Applied per-connection at checkout (r2d2 customizer):
PRAGMA synchronous = NORMAL;       -- safe with WAL, far faster than FULL
PRAGMA busy_timeout = 5000;        -- wait rather than error under write contention
PRAGMA foreign_keys = ON;
PRAGMA cache_size = -262144;       -- ~256 MB page cache (negative = KiB)
PRAGMA mmap_size = 1073741824;     -- 1 GiB memory-mapped I/O
PRAGMA temp_store = MEMORY;
```

> `page_size` cannot change after tables exist without a `VACUUM`. The migration runner
> must detect a fresh DB and set `page_size` first. Document this ordering in `db` crate.

## Schema (initial migration `V1__init.sql`)

```sql
CREATE TABLE movies (
    id            INTEGER PRIMARY KEY,
    title         TEXT NOT NULL,
    sort_title    TEXT NOT NULL,
    year          INTEGER,
    overview      TEXT,
    added_at      INTEGER NOT NULL,            -- unix epoch
    poster_path   TEXT,                        -- under /config or /media
    backdrop_path TEXT
);

CREATE TABLE series (
    id            INTEGER PRIMARY KEY,
    title         TEXT NOT NULL,
    sort_title    TEXT NOT NULL,
    year          INTEGER,
    overview      TEXT,
    added_at      INTEGER NOT NULL,
    poster_path   TEXT,
    backdrop_path TEXT
);

CREATE TABLE seasons (
    id            INTEGER PRIMARY KEY,
    series_id     INTEGER NOT NULL REFERENCES series(id) ON DELETE CASCADE,
    season_number INTEGER NOT NULL,
    UNIQUE(series_id, season_number)
);

CREATE TABLE episodes (
    id             INTEGER PRIMARY KEY,
    season_id      INTEGER NOT NULL REFERENCES seasons(id) ON DELETE CASCADE,
    episode_number INTEGER NOT NULL,
    title          TEXT,
    overview       TEXT,
    UNIQUE(season_id, episode_number)
);

-- One physical file. Belongs to exactly one movie OR one episode.
CREATE TABLE media_files (
    id                        INTEGER PRIMARY KEY,
    movie_id                  INTEGER REFERENCES movies(id)   ON DELETE CASCADE,
    episode_id                INTEGER REFERENCES episodes(id) ON DELETE CASCADE,
    path                      TEXT NOT NULL UNIQUE,           -- absolute, under /media
    container                 TEXT,                           -- mkv, mp4, ...
    size_bytes                INTEGER,
    duration_ms               INTEGER,
    -- video stream
    video_codec               TEXT,                           -- h264, hevc, av1
    video_profile             TEXT,                           -- e.g. "Main 10", "High 10"
    width                     INTEGER,
    height                    INTEGER,
    bit_depth                 INTEGER,                        -- 8 / 10
    bitrate                   INTEGER,
    -- HDR / color
    transfer_characteristics  TEXT,                           -- smpte2084(PQ), arib-std-b67(HLG), bt709
    color_space               TEXT,                           -- bt2020nc, bt709
    hdr_type                  TEXT,                           -- none, hdr10, hdr10plus, hlg, dolbyvision
    -- Dolby Vision (drives transcode path)
    dv_profile                INTEGER,                        -- 5, 7, 8 (NULL if not DV)
    dv_bl_compatible_id       INTEGER,                        -- 0,1,4,6 ; for P8: 1=HDR10, 4=SDR ...
    dv_level                  INTEGER,
    -- decode-fallback flag: TRUE for formats HW cannot decode (e.g. H.264 High 10)
    hw_decode_unsupported     INTEGER NOT NULL DEFAULT 0,
    CHECK ( (movie_id IS NOT NULL) <> (episode_id IS NOT NULL) )
    -- NOTE: audio is NOT stored here. A file has 1..N audio tracks (commentary, dubs,
    -- lossless+lossy), so audio lives in the child `audio_streams` table added by
    -- `V4__audio_streams.sql` (task `70-audio-quality-and-profiles.md`). media_files
    -- stays the 1:1 home for the single primary VIDEO stream. Do not add audio columns here.
);

CREATE TABLE people (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

CREATE TABLE credits (
    id         INTEGER PRIMARY KEY,
    person_id  INTEGER NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    movie_id   INTEGER REFERENCES movies(id)   ON DELETE CASCADE,
    series_id  INTEGER REFERENCES series(id)   ON DELETE CASCADE,
    role       TEXT,                                          -- 'actor','director',...
    character  TEXT,
    ord        INTEGER                                        -- billing order
);

-- Generated assets (written by assets crate to /config)
CREATE TABLE preview_clips (
    media_file_id INTEGER PRIMARY KEY REFERENCES media_files(id) ON DELETE CASCADE,
    path          TEXT NOT NULL,                              -- /config/previews/<id>.mp4
    generated_at  INTEGER NOT NULL
);

CREATE TABLE trickplay_assets (
    media_file_id INTEGER PRIMARY KEY REFERENCES media_files(id) ON DELETE CASCADE,
    kind          TEXT NOT NULL,                              -- 'bif' | 'tiled_jpg'
    path          TEXT NOT NULL,                              -- /config/trickplay/<id>.*
    interval_ms   INTEGER NOT NULL,
    tile_w        INTEGER, tile_h INTEGER, cols INTEGER, rows INTEGER,
    generated_at  INTEGER NOT NULL
);

-- Idempotent scan bookkeeping (used by ingest + assets workers)
CREATE TABLE scan_state (
    path            TEXT PRIMARY KEY,       -- file path
    mtime           INTEGER NOT NULL,
    size_bytes      INTEGER NOT NULL,
    probed_at       INTEGER,                -- ffprobe done
    preview_done_at INTEGER,               -- assets worker done
    trickplay_done_at INTEGER
);
```

## Indexes (fast grid / scroll / detail)

```sql
CREATE INDEX idx_movies_sort   ON movies(sort_title);
CREATE INDEX idx_series_sort   ON series(sort_title);
CREATE INDEX idx_movies_added  ON movies(added_at DESC);
CREATE INDEX idx_files_movie   ON media_files(movie_id);
CREATE INDEX idx_files_episode ON media_files(episode_id);
CREATE INDEX idx_episodes_season ON episodes(season_id, episode_number);
CREATE INDEX idx_credits_movie ON credits(movie_id, ord);
CREATE INDEX idx_credits_series ON credits(series_id, ord);
```

## Sub-tasks

1. `db` crate: build the r2d2 pool with a `ConnectionCustomizer` that runs the per-connection
   PRAGMAs; fresh-DB detection sets `page_size` before migrations.
2. Wire `refinery` embedded migrations from `backend/migrations/`.
3. Define Rust models + `serde` DTOs in `db` (or `core`) mirroring the tables; expose typed
   query functions (`list_movies(offset, limit)`, `get_movie(id)`, `get_series(id)`, …).
4. Add the `DvProfile` / `HdrType` enums to `core` so `ingest` writes them and `transcode` reads them.

## Scaling notes

- Keep list queries covering-index friendly and ordered by an indexed column for keyset
  pagination (avoid large `OFFSET`); `api` will paginate — see `02-api-contract.md`.
- Run all rusqlite calls under `tokio::task::spawn_blocking`; never block the async runtime.

## Verification

- Fresh boot creates `/config/library.db` with `PRAGMA page_size` → `65536` and
  `PRAGMA journal_mode` → `wal` (verify with `sqlite3`).
- Migrations are idempotent across restarts (refinery records applied versions).
- Insert a DV Profile 5 file → row has `dv_profile=5`, `hdr_type='dolbyvision'`.
