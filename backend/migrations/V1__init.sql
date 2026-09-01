-- V1__init.sql — initial schema + indexes for library.db
--
-- Canonical source: docs/.tasks/01-db-schema.md.
-- Applied by refinery at container boot. Idempotent across restarts (refinery
-- records applied versions in its own bookkeeping table).
--
-- NOTE ON PRAGMAS: `page_size = 65536` MUST be set on a fresh database BEFORE
-- this migration runs (it cannot change once tables exist without a VACUUM).
-- `journal_mode = WAL` and `auto_vacuum = INCREMENTAL` are persisted database
-- properties and are applied by the db crate around this migration; they are
-- intentionally NOT set here so the file stays a pure DDL migration.

-- ---------------------------------------------------------------------------
-- Catalog: movies
-- ---------------------------------------------------------------------------
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

-- ---------------------------------------------------------------------------
-- Catalog: series / seasons / episodes
-- ---------------------------------------------------------------------------
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

-- ---------------------------------------------------------------------------
-- Physical media files. Belongs to exactly one movie OR one episode.
-- ---------------------------------------------------------------------------
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
);

-- ---------------------------------------------------------------------------
-- People + credits
-- ---------------------------------------------------------------------------
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

-- ---------------------------------------------------------------------------
-- Generated assets (written by the assets crate to /config)
-- ---------------------------------------------------------------------------
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

-- ---------------------------------------------------------------------------
-- Idempotent scan bookkeeping (used by ingest + assets workers)
-- ---------------------------------------------------------------------------
CREATE TABLE scan_state (
    path              TEXT PRIMARY KEY,       -- file path
    mtime             INTEGER NOT NULL,
    size_bytes        INTEGER NOT NULL,
    probed_at         INTEGER,                -- ffprobe done
    preview_done_at   INTEGER,                -- assets worker done
    trickplay_done_at INTEGER
);

-- ---------------------------------------------------------------------------
-- Indexes (fast grid / scroll / detail)
-- ---------------------------------------------------------------------------
CREATE INDEX idx_movies_sort     ON movies(sort_title);
CREATE INDEX idx_series_sort     ON series(sort_title);
CREATE INDEX idx_movies_added    ON movies(added_at DESC);
CREATE INDEX idx_files_movie     ON media_files(movie_id);
CREATE INDEX idx_files_episode   ON media_files(episode_id);
CREATE INDEX idx_episodes_season ON episodes(season_id, episode_number);
CREATE INDEX idx_credits_movie   ON credits(movie_id, ord);
CREATE INDEX idx_credits_series  ON credits(series_id, ord);
