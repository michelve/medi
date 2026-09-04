-- V12__chapters.sql — one row per embedded chapter marker of a media file (Task 99).
--
-- Canonical source: docs/.tasks/99-subtitles-and-chapters.md §Part B.
-- A file is 1:N in chapters, so chapters live in a child table keyed by media_file_id — the
-- same normalization discipline audio_streams (V4) / subtitle_streams (V5) use, never flat
-- columns on media_files. Rows come from ffprobe `-show_chapters` on the single probe pass;
-- `ordinal` preserves chapter order, `start_ms`/`end_ms` are milliseconds (ffprobe reports
-- fractional seconds), `title` is the chapter name (may be NULL). `end_ms` is nullable —
-- some files omit chapter end times, and the player bounds a chapter by the next chapter's
-- start then. Existing media_files rows simply have no chapters children until re-probed; the
-- scan_state-driven re-probe path repopulates them. Additive DDL only (no PRAGMAs — see
-- migrations/README.md); idempotent via refinery version records.

CREATE TABLE chapters (
    id             INTEGER PRIMARY KEY,
    media_file_id  INTEGER NOT NULL REFERENCES media_files(id) ON DELETE CASCADE,
    ordinal        INTEGER NOT NULL,           -- 0-based chapter order
    start_ms       INTEGER NOT NULL,           -- chapter start in milliseconds
    end_ms         INTEGER,                    -- chapter end in ms; NULL when the file omits it
    title          TEXT,                       -- ffprobe tags.title, may be NULL
    UNIQUE(media_file_id, ordinal)
);

CREATE INDEX idx_chapters_file ON chapters(media_file_id);
