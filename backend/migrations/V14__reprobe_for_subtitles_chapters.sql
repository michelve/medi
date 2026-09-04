-- V14__reprobe_for_subtitles_chapters.sql — force a one-time re-probe of every media file so
-- existing libraries pick up external subtitle sidecars and embedded chapters (Task 99).
--
-- Canonical source: docs/.tasks/99-subtitles-and-chapters.md §detection fix.
-- The ingest probe path GAINED capabilities after most libraries were already scanned:
-- external sidecar discovery landed in Task 90 and `-show_chapters` in Task 99. Because
-- `filter_changed` (ingest/worker.rs) only re-probes a file whose mtime/size changed or whose
-- `probed_at` is NULL, an already-probed file never picks up these newer probe results — so a
-- movie with a same-folder `.srt` shows a disabled subtitle button, and no file has chapter
-- rows. Clearing `probed_at` is the standard, refinery-tracked way to force a single re-probe
-- of the whole library without adding a probe-version column: the next scan sees every file as
-- `probed_at IS NULL`, re-probes it once, and repopulates subtitle_streams (external) +
-- chapters. Re-probing is itself idempotent (writes::replace_* are delete-then-insert), so the
-- only cost is one extra probe pass. Applied exactly once via the refinery version record.
--
-- (Going forward, sidecar drift alone also triggers a re-probe — see worker.rs::sidecars_drifted
-- — so this backfill is only needed for files probed before that logic existed.)
-- Additive/data-only DML (no schema change, no PRAGMAs — see migrations/README.md).

UPDATE scan_state SET probed_at = NULL;
