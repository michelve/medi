-- V11__playback_progress.sql — persisted playback position for resume + "Continue
-- Watching" (Task 98 Part A). The next free refinery version after V10__probe_failures.sql.
-- Additive DDL only (no PRAGMAs — see migrations/README.md).
--
-- Before this there was no playback-progress persistence anywhere: the player started at 0
-- every time and closing the tab lost your place, and the "Continue Watching" row was a
-- hardcoded label over a slice of the catalog. This table remembers where you left off,
-- keyed by media file.
--
-- Single-user LAN appliance (no auth): progress is GLOBAL, one row per file, so the primary
-- key is the `media_file_id` itself. `ON DELETE CASCADE` drops a file's progress when the
-- file is removed (matches audio_streams / subtitle_streams).

CREATE TABLE playback_progress (
    media_file_id INTEGER PRIMARY KEY REFERENCES media_files(id) ON DELETE CASCADE,
    position_ms   INTEGER NOT NULL,
    duration_ms   INTEGER NOT NULL,            -- snapshot at write time (for the % calc)
    updated_at    INTEGER NOT NULL,            -- unix seconds (match existing epoch columns)
    finished      INTEGER NOT NULL DEFAULT 0   -- set past ~95%; drops it from Continue Watching
);

-- "Continue Watching" lists in-progress titles newest-first, so index the ordering key.
CREATE INDEX idx_playback_progress_updated ON playback_progress(updated_at DESC);
