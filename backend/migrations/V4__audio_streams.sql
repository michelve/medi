-- V4__audio_streams.sql — one row per audio track of a media file (Task 70).
--
-- Canonical source: docs/.tasks/70-audio-quality-and-profiles.md §DB migration.
-- A file is 1:N in audio tracks (director's commentary, foreign dubs, a lossless +
-- lossy pair), so audio lives in a child table keyed by media_file_id rather than in
-- flat columns on media_files — flat columns could hold only one track and would force
-- a lossy "pick the first" choice at probe time, breaking selectedAudioTrack.
-- media_files remains the 1:1 home for the single primary VIDEO stream; audio joins the
-- same normalization discipline 01-db-schema.md already uses for credits/seasons/episodes.
--
-- Each track gets a stream_index matching ffprobe's ordering, which is exactly what
-- react-native-video's selectedAudioTrack selects by. Existing media_files rows simply
-- have no audio_streams children until re-probed; the scan_state-driven re-probe path
-- repopulates them. Additive DDL only (no PRAGMAs — see migrations/README.md); idempotent
-- via refinery version records.

CREATE TABLE audio_streams (
    id             INTEGER PRIMARY KEY,
    media_file_id  INTEGER NOT NULL REFERENCES media_files(id) ON DELETE CASCADE,
    stream_index   INTEGER NOT NULL,        -- ffprobe stream index (selectedAudioTrack)
    codec          TEXT,                    -- aac,ac3,eac3,dts,dtshd,truehd,flac,opus,pcm
    profile        TEXT,                    -- raw ffprobe profile, e.g. "DTS-HD MA"
    channels       INTEGER,                 -- 2, 6, 8
    channel_layout TEXT,                    -- "stereo", "5.1", "7.1", "5.1(side)"
    bitrate        INTEGER,                 -- bits/sec, NULL if lossless / unknown
    sample_rate    INTEGER,                 -- Hz
    language       TEXT,                    -- ISO-639-2 tag, e.g. "eng"
    title          TEXT,                    -- stream tag title, e.g. "Commentary"
    immersive      TEXT NOT NULL DEFAULT 'none',  -- none | dolby_atmos | dts_x
    is_default     INTEGER NOT NULL DEFAULT 0,    -- ffprobe DISPOSITION:default
    UNIQUE(media_file_id, stream_index)
);

CREATE INDEX idx_audio_streams_file ON audio_streams(media_file_id);
