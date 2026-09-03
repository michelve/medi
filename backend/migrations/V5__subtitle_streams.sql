-- V5__subtitle_streams.sql — one row per subtitle track of a media file (Task 90).
--
-- Canonical source: docs/.tasks/90-format-coverage-and-subtitles.md §DB migration.
-- A file is 1:N in subtitles (commentary / foreign / forced tracks + external sidecars),
-- so subtitles live in a child table keyed by media_file_id — the same normalization
-- discipline audio_streams (V4) uses, and never flat columns on media_files (which stays
-- the 1:1 home for the single primary VIDEO stream). A row describes either an EMBEDDED
-- track (stream_index set, external_path NULL) or an EXTERNAL sidecar file (external_path
-- set, stream_index NULL).
--
-- `format` ('text' | 'image') drives serving: text subtitles convert to WebVTT and ride
-- as a client sidecar without a video transcode; image subtitles (PGS / VobSub) can only
-- be burned into the video via a forced re-encode. Existing media_files rows simply have
-- no subtitle_streams children until re-probed; the scan_state-driven re-probe path
-- repopulates them. Additive DDL only (no PRAGMAs — see migrations/README.md); idempotent
-- via refinery version records.

CREATE TABLE subtitle_streams (
    id             INTEGER PRIMARY KEY,
    media_file_id  INTEGER NOT NULL REFERENCES media_files(id) ON DELETE CASCADE,
    stream_index   INTEGER,                 -- ffprobe stream index for embedded; NULL for external
    codec          TEXT,                    -- subrip, ass, ssa, mov_text, webvtt, hdmv_pgs_subtitle, dvd_subtitle
    format         TEXT NOT NULL,           -- 'text' | 'image'  (drives passthrough-vtt vs burn-in)
    language       TEXT,                    -- ISO-639-2 tag, e.g. "eng"
    title          TEXT,                    -- stream tag title
    is_default     INTEGER NOT NULL DEFAULT 0,   -- ffprobe DISPOSITION:default
    is_forced      INTEGER NOT NULL DEFAULT 0,   -- ffprobe DISPOSITION:forced (or ".forced." sidecar)
    is_external    INTEGER NOT NULL DEFAULT 0,   -- 0 embedded, 1 sidecar file
    external_path  TEXT,                    -- absolute path under /media for a sidecar; NULL if embedded
    UNIQUE(media_file_id, stream_index, external_path)
);

CREATE INDEX idx_subtitle_streams_file ON subtitle_streams(media_file_id);
