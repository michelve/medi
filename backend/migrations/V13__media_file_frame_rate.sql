-- V13__media_file_frame_rate.sql — the video stream's frame rate on media_files (Task 99).
--
-- Canonical source: docs/.tasks/99-subtitles-and-chapters.md §client-side rendering.
-- The web player's libass renderer times ASS animations against the video's real FPS
-- (`targetFps`); without it, animated/karaoke subtitles drift on non-24fps content. This is a
-- 1:1 property of the single primary video stream, so it lives as a flat column on media_files
-- (like width/height/bit_depth), not a child table. ffprobe reports it as `avg_frame_rate`
-- ("24000/1001"); the parser reduces it to a float. Existing rows are NULL until re-probed via
-- the scan_state path. Additive DDL only (no PRAGMAs — see migrations/README.md); idempotent
-- via refinery version records.

ALTER TABLE media_files ADD COLUMN frame_rate REAL;
