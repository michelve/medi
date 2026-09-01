//! The off-peak asset worker main loop (`docs/.tasks/30` sub-task 4).
//!
//! Repeatedly: pull the next batch of probed files still missing a preview or trickplay
//! asset (`queries::list_pending_assets`, oldest-first for a resumable backfill), and
//! for each one — behind the [`Scheduler`] gate (off-peak window + GPU-idle guard +
//! concurrency throttle) — generate whichever assets it lacks, record the rows, and
//! stamp `scan_state.{preview,trickplay}_done_at` so a restart resumes rather than
//! restarts.
//!
//! ## Idempotency & resume
//! A file is "done" per-asset: `preview_done_at`/`trickplay_done_at` are stamped
//! independently, so a crash after the preview but before the trickplay leaves the
//! preview marked done and only the trickplay is retried. Because the pending query
//! filters on those columns, an already-done asset is never regenerated.
//!
//! ## GPU yielding
//! The scheduler is consulted *before each file*, not once per batch, so a live
//! transcode starting mid-batch pauses the next file. An in-flight ffmpeg (a ~15s
//! preview or a quick frame sample) is allowed to finish — it is short and killing it
//! would waste the work already done.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use medi_db::queries::{self, PendingAsset};
use medi_db::writes::{self, TrickplayKind};
use medi_db::Db;
use medi_transcode::HwCaps;

use crate::preview;
use crate::scheduler::Scheduler;
use crate::trickplay::{self, DEFAULT_INTERVAL_MS};

/// Configuration for one run of the asset worker.
#[derive(Clone)]
pub struct AssetWorkerConfig {
    /// Host capabilities, reused from the Phase 2 probe for HW-accelerated downscale.
    pub caps: HwCaps,
    /// How many pending files to pull per batch. Progress is checkpointed to the DB
    /// after each file, so a small batch keeps memory flat on a 10k-file backfill.
    pub batch_size: u32,
    /// Trickplay format to produce. Defaults to **tiled-JPG**: the medi TV client
    /// (`@medi/player`, `docs/.tasks/50` Part A) renders scrub thumbnails by cropping a
    /// cell out of a JPEG mosaic — it cannot parse the binary BIF index on-device — and
    /// `GET /api/trickplay/:id/meta` only serves grid geometry for the tiled-JPG kind.
    /// `docs/.tasks/30` allowed BIF as an option; it is still selectable here for a
    /// Roku-style consumer, but the shipping client needs `TiledJpg`.
    pub trickplay_kind: TrickplayKind,
    /// Frame-sampling interval for trickplay sprites, ms.
    pub trickplay_interval_ms: i64,
}

impl AssetWorkerConfig {
    /// A sensible default: tiled-JPG trickplay at the 10s interval, 32-file batches.
    /// Tiled-JPG (not BIF) so the TV client's scrub thumbnails work end-to-end.
    pub fn new(caps: HwCaps) -> Self {
        Self {
            caps,
            batch_size: 32,
            trickplay_kind: TrickplayKind::TiledJpg,
            trickplay_interval_ms: DEFAULT_INTERVAL_MS,
        }
    }
}

/// Run the asset worker forever: process pending files in batches, sleeping when the
/// library is fully covered (nothing pending). Consulting the [`Scheduler`] gate before
/// each file keeps generation inside the off-peak window and off the GPU when live
/// streams are running.
///
/// Returns only on an unrecoverable error (e.g. the DB is gone); transient per-file
/// ffmpeg failures are logged and skipped so one bad file never stalls the backfill.
pub async fn run(db: Db, scheduler: Scheduler, cfg: AssetWorkerConfig) -> anyhow::Result<()> {
    /// How long to sleep when there is nothing pending before checking again — a new
    /// file may be ingested at any time (the ingest watcher runs independently).
    const IDLE_SLEEP: Duration = Duration::from_secs(300);

    tracing::info!(
        batch_size = cfg.batch_size,
        trickplay = cfg.trickplay_kind.as_str(),
        "asset worker started",
    );

    loop {
        let batch = pending_batch(&db, cfg.batch_size).await?;
        if batch.is_empty() {
            tracing::debug!("asset worker: nothing pending; sleeping");
            tokio::time::sleep(IDLE_SLEEP).await;
            continue;
        }
        tracing::info!(count = batch.len(), "asset worker: processing batch");

        let mut progressed = 0usize;
        for pending in batch {
            // Gate: block until off-peak + GPU idle, then hold a throttle permit for the
            // duration of this file's generation.
            let _permit = scheduler.wait_until_runnable().await;
            match process_one(&db, &scheduler, &cfg, &pending).await {
                Ok(()) => progressed += 1,
                Err(err) => tracing::warn!(
                    media_file_id = pending.media_file_id,
                    path = %pending.path,
                    error = %err,
                    "asset generation failed; skipping file",
                ),
            }
        }

        // If the whole batch failed (e.g. a cluster of un-processable files that stay
        // pending), back off before retrying so we don't hot-spin the same failing set.
        // A batch with any success re-queries immediately to keep the backfill moving.
        if progressed == 0 {
            tracing::warn!("asset worker: batch made no progress; backing off");
            tokio::time::sleep(IDLE_SLEEP).await;
        }
    }
}

/// Pull the next batch of pending files off the DB on the blocking pool.
async fn pending_batch(db: &Db, batch_size: u32) -> anyhow::Result<Vec<PendingAsset>> {
    let db = db.clone();
    let batch = tokio::task::spawn_blocking(move || -> medi_db::DbResult<Vec<PendingAsset>> {
        let conn = db.conn()?;
        queries::list_pending_assets(&conn, batch_size)
    })
    .await??;
    Ok(batch)
}

/// Generate whichever assets `pending` still lacks and record them. Each asset is
/// stamped independently so a partial completion resumes cleanly.
async fn process_one(
    db: &Db,
    scheduler: &Scheduler,
    cfg: &AssetWorkerConfig,
    pending: &PendingAsset,
) -> anyhow::Result<()> {
    let input = PathBuf::from(&pending.path);
    if !input.is_file() {
        // The file vanished between ingest and asset generation; the ingest reaper will
        // clean the rows. Skip without stamping so we don't mark a missing file "done".
        anyhow::bail!("source file no longer exists");
    }

    // Read whether this source is HDR/DV so the preview can tone-map to SDR.
    let hdr = is_hdr(db, pending.media_file_id).await?;

    if !pending.preview_done {
        let out = preview::generate(
            &cfg.caps,
            &input,
            &scheduler.previews_dir(),
            pending.media_file_id,
            pending.duration_ms,
            hdr,
        )
        .await?;
        record_preview(db, pending.media_file_id, &pending.path, &out).await?;
    }

    if !pending.trickplay_done {
        let out = trickplay::generate(
            &input,
            &scheduler.trickplay_dir(),
            pending.media_file_id,
            cfg.trickplay_interval_ms,
            cfg.trickplay_kind,
        )
        .await?;
        record_trickplay(db, pending.media_file_id, &pending.path, &out).await?;
    }
    Ok(())
}

/// Is the media file an HDR/DV source (so its preview needs tone-mapping to SDR)?
async fn is_hdr(db: &Db, media_file_id: i64) -> anyhow::Result<bool> {
    let db = db.clone();
    let hdr = tokio::task::spawn_blocking(move || -> medi_db::DbResult<bool> {
        let conn = db.conn()?;
        let file = queries::get_media_file(&conn, media_file_id)?;
        // Any non-SDR transfer/HDR type warrants tone-mapping the preview.
        let is_hdr = file
            .hdr_type
            .as_deref()
            .map(|h| h != "none" && !h.is_empty())
            .unwrap_or(false)
            || file.dv_profile.is_some();
        Ok(is_hdr)
    })
    .await??;
    Ok(hdr)
}

/// Persist the `preview_clips` row and stamp `scan_state.preview_done_at`, in one
/// transaction so the row and the "done" flag commit together. `path` is stored
/// relative to nothing — the schema keeps the absolute `/config/previews/<id>.mp4`.
async fn record_preview(
    db: &Db,
    media_file_id: i64,
    scan_path: &str,
    out: &Path,
) -> anyhow::Result<()> {
    let db = db.clone();
    let out = out.to_string_lossy().into_owned();
    let scan_path = scan_path.to_string();
    tokio::task::spawn_blocking(move || -> medi_db::DbResult<()> {
        let mut conn = db.conn()?;
        let tx = conn.transaction()?;
        let now = now_secs();
        writes::upsert_preview_clip(&tx, media_file_id, &out, now)?;
        writes::mark_preview_done(&tx, &scan_path, now)?;
        tx.commit()?;
        Ok(())
    })
    .await??;
    Ok(())
}

/// Persist the `trickplay_assets` row and stamp `scan_state.trickplay_done_at`, in one
/// transaction so the row and the "done" flag commit together.
async fn record_trickplay(
    db: &Db,
    media_file_id: i64,
    scan_path: &str,
    out: &trickplay::TrickplayOutput,
) -> anyhow::Result<()> {
    let db = db.clone();
    let scan_path = scan_path.to_string();
    let out = out.clone();
    tokio::task::spawn_blocking(move || -> medi_db::DbResult<()> {
        let mut conn = db.conn()?;
        let tx = conn.transaction()?;
        let now = now_secs();
        let path = out.path.to_string_lossy().into_owned();
        writes::upsert_trickplay_asset(
            &tx,
            media_file_id,
            out.kind,
            &path,
            out.interval_ms,
            out.grid,
            now,
        )?;
        writes::mark_trickplay_done(&tx, &scan_path, now)?;
        tx.commit()?;
        Ok(())
    })
    .await??;
    Ok(())
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medi_db::writes::{FileOwner, MediaFileWrite};

    fn temp_db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = medi_db::open(dir.path().join("library.db"), 2).unwrap();
        (db, dir)
    }

    /// Insert a probed movie file + its scan_state row, returning the media_file id.
    fn seed_probed_file(db: &Db, path: &str, hdr: &str) -> i64 {
        let conn = db.conn().unwrap();
        let movie = writes::find_or_create_movie(&conn, "T", "t", None, 0).unwrap();
        let data = MediaFileWrite {
            container: Some("mkv".into()),
            width: Some(3840),
            height: Some(2160),
            duration_ms: Some(600_000),
            hdr_type: Some(hdr.into()),
            ..Default::default()
        };
        let id = writes::upsert_media_file(&conn, path, FileOwner::Movie(movie), &data).unwrap();
        writes::upsert_scan_state(
            &conn,
            path,
            writes::FileStat { mtime: 1, size_bytes: 1 },
        )
        .unwrap();
        writes::mark_probed(&conn, path, 100).unwrap();
        id
    }

    #[tokio::test]
    async fn pending_lists_probed_undone_files() {
        let (db, _dir) = temp_db();
        seed_probed_file(&db, "/media/a.mkv", "none");

        let batch = pending_batch(&db, 10).await.unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].path, "/media/a.mkv");
        assert!(!batch[0].preview_done);
        assert!(!batch[0].trickplay_done);
        assert_eq!(batch[0].duration_ms, Some(600_000));
    }

    #[tokio::test]
    async fn recording_marks_done_and_drops_from_pending() {
        let (db, _dir) = temp_db();
        let id = seed_probed_file(&db, "/media/a.mkv", "none");

        // Simulate a completed preview + trickplay by recording both.
        record_preview(&db, id, "/media/a.mkv", Path::new("/config/previews/1.mp4"))
            .await
            .unwrap();
        let tp = trickplay::TrickplayOutput {
            kind: TrickplayKind::Bif,
            path: PathBuf::from("/config/trickplay/1.bif"),
            interval_ms: 10_000,
            grid: None,
        };
        record_trickplay(&db, id, "/media/a.mkv", &tp).await.unwrap();

        // Now nothing is pending (both done stamps set).
        let batch = pending_batch(&db, 10).await.unwrap();
        assert!(batch.is_empty(), "fully-processed file drops from pending");

        // The rows landed.
        let conn = db.conn().unwrap();
        let pc: i64 = conn
            .query_row("SELECT COUNT(*) FROM preview_clips", [], |r| r.get(0))
            .unwrap();
        let ta: i64 = conn
            .query_row("SELECT COUNT(*) FROM trickplay_assets", [], |r| r.get(0))
            .unwrap();
        assert_eq!((pc, ta), (1, 1));
    }

    #[tokio::test]
    async fn partial_completion_keeps_only_the_undone_asset_pending() {
        let (db, _dir) = temp_db();
        let id = seed_probed_file(&db, "/media/a.mkv", "hdr10");

        // Only the preview finished (crash before trickplay).
        record_preview(&db, id, "/media/a.mkv", Path::new("/config/previews/1.mp4"))
            .await
            .unwrap();

        let batch = pending_batch(&db, 10).await.unwrap();
        assert_eq!(batch.len(), 1, "still pending: trickplay not done");
        assert!(batch[0].preview_done, "preview already done");
        assert!(!batch[0].trickplay_done);
    }

    #[tokio::test]
    async fn hdr_detection_reads_media_row() {
        let (db, _dir) = temp_db();
        let sdr = seed_probed_file(&db, "/media/sdr.mkv", "none");
        let hdr = seed_probed_file(&db, "/media/hdr.mkv", "dolbyvision");
        assert!(!is_hdr(&db, sdr).await.unwrap());
        assert!(is_hdr(&db, hdr).await.unwrap());
    }
}
