//! The async ingestion worker: drives the scanner, bounds ffprobe concurrency with a
//! semaphore, writes rows via `medi-db`, watches `/media` for changes, and invalidates
//! the API's response cache after a write. Phase 1, sub-tasks 4, 5, 7.
//!
//! ## Concurrency & the write path
//!
//! ffprobe runs are CPU/IO heavy and each spawns a subprocess, so a first-run scan of
//! 10,000 files must not spawn 10,000 processes at once. A [`tokio::sync::Semaphore`]
//! caps how many probes run concurrently (`docs/.tasks/10` §Scaling notes). Probes fan
//! out; the DB **write path stays single-threaded** — WAL allows one writer — so
//! results funnel back through one mpsc channel to a single writer task.
//!
//! ## Cache invalidation
//!
//! The moka response cache lives in the `api` crate; to avoid a circular dependency
//! the worker holds an opaque [`Invalidator`] callback that `api` wires to
//! `ResponseCache::invalidate_all`. The worker calls it after each batch of writes so
//! the next catalog GET reflects freshly ingested titles.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::{mpsc, Semaphore};

use medi_db::writes::{
    self, AudioStreamWrite, ChapterWrite, FileOwner, FileStat, MediaFileWrite, SubtitleStreamWrite,
};
use medi_db::Db;

use crate::ffprobe;
use crate::scanner::{self, Classification, DiscoveredFile};

/// A callback the worker invokes after writing new/changed catalog rows, so the API
/// layer can drop its cached responses. `api` passes a closure over
/// `ResponseCache::invalidate_all`.
pub type Invalidator = Arc<dyn Fn() + Send + Sync>;

/// A handle the API uses to ask the ingest worker to run a scan **now**, out of band from
/// the filesystem watcher (`docs/.tasks/60` Phase B — the `POST /api/libraries/:id/scan`
/// and library-create triggers). The fs-watcher only fires on disk *changes*; a library
/// pointed at a folder whose files already exist would otherwise wait for the next restart
/// or fs event. Sending on this channel wakes the same debounced scan loop.
///
/// Cloneable and cheap; a send never blocks (unbounded) and a full/closed channel is a
/// no-op (the worker isn't running), so callers can `trigger()` fire-and-forget.
#[derive(Clone)]
pub struct ScanTrigger {
    tx: mpsc::UnboundedSender<()>,
}

impl ScanTrigger {
    /// Build a trigger + its receiver. The API keeps the [`ScanTrigger`]; the worker's
    /// [`watch`] loop takes the receiver.
    pub fn new() -> (Self, ScanSignal) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, ScanSignal { rx })
    }

    /// Ask the worker to scan as soon as the debounce settles. Never blocks; a closed
    /// receiver (worker not running) is silently ignored.
    pub fn trigger(&self) {
        let _ = self.tx.send(());
    }
}

/// The receiving half of a [`ScanTrigger`], handed to [`watch`]. Opaque to the API.
pub struct ScanSignal {
    rx: mpsc::UnboundedReceiver<()>,
}

/// Configuration for the ingestion worker.
#[derive(Clone)]
pub struct WorkerConfig {
    /// The read-only source root to scan (`AppConfig::media_dir`, usually `/media`).
    pub media_dir: PathBuf,
    /// Maximum concurrent ffprobe subprocesses. A small multiple of CPU cores keeps a
    /// 10k-file first scan from spawning thousands of processes.
    pub probe_concurrency: usize,
    /// Optional metadata enrichment context (`docs/.tasks/60` Phase A). `None` when no
    /// provider is configured — ingest then behaves filename-only (graceful degradation).
    pub enrich: Option<medi_metadata::EnrichContext>,
    /// Max concurrent enrichment provider round-trips, mirroring `probe_concurrency`.
    pub enrich_concurrency: usize,
    /// Shared status handle the API's `GET /api/status` reads (`docs/.tasks/96`). `None`
    /// when observability isn't wired (standalone / tests) — recording is then a no-op.
    pub status: Option<crate::status::EnrichmentStatus>,
}

impl WorkerConfig {
    pub fn new(media_dir: PathBuf) -> Self {
        Self {
            media_dir,
            // Conservative default; the caller may raise it based on core count.
            probe_concurrency: 4,
            enrich: None,
            enrich_concurrency: 4,
            status: None,
        }
    }

    /// Attach a metadata enrichment context so `run_scan` enriches newly-ingested titles.
    pub fn with_enrichment(mut self, ctx: medi_metadata::EnrichContext) -> Self {
        self.enrich = Some(ctx);
        self
    }

    /// Attach the shared status handle so scans/enrichment record their tallies for
    /// `GET /api/status` (`docs/.tasks/96`).
    pub fn with_status(mut self, status: crate::status::EnrichmentStatus) -> Self {
        self.status = Some(status);
        self
    }
}

/// Run one full ingestion pass: scan `media_dir`, probe every new/changed file with
/// bounded concurrency, write results, and invalidate the cache if anything changed.
///
/// Idempotent: unchanged, already-probed files are skipped via `scan_state`
/// (mtime + size), so a re-scan of an unchanged library re-probes nothing.
pub async fn run_scan(db: &Db, cfg: &WorkerConfig, invalidate: &Invalidator) -> anyhow::Result<()> {
    // Observability (`docs/.tasks/96`): stamp scan start so `GET /api/status` shows a run in
    // flight, and count ffprobe failures across the fan-out to report at the end.
    if let Some(s) = &cfg.status {
        s.scan_started();
    }
    let probe_failures = Arc::new(std::sync::atomic::AtomicU64::new(0));

    // Phase B: scan per library folder so files are scoped to a library and the library
    // `kind` overrides filename guessing. When no libraries are defined (a bare
    // pre-Phase-B DB), fall back to a single filename-guessed scan of `media_dir`.
    let roots = {
        let db = db.clone();
        tokio::task::spawn_blocking(move || -> medi_db::DbResult<Vec<_>> {
            let conn = db.conn()?;
            medi_db::queries::library_roots(&conn)
        })
        .await??
    };

    let discovered = if roots.is_empty() {
        let media_dir = cfg.media_dir.clone();
        tracing::info!(root = %media_dir.display(), "starting ingest scan (single root)");
        tokio::task::spawn_blocking(move || scanner::scan(&media_dir)).await?
    } else {
        tracing::info!(roots = roots.len(), "starting ingest scan (multi-library)");
        let roots2 = roots.clone();
        tokio::task::spawn_blocking(move || {
            let mut all = Vec::new();
            for r in &roots2 {
                let hint = match r.kind.as_str() {
                    "series" => scanner::KindHint::Series,
                    _ => scanner::KindHint::Movie,
                };
                all.extend(scanner::scan_root(
                    std::path::Path::new(&r.path),
                    Some(r.library_id),
                    hint,
                ));
            }
            all
        })
        .await?
    };
    tracing::info!(count = discovered.len(), "scan found candidate files");

    // Diff against scan_state to find what actually needs probing.
    let to_probe = filter_changed(db, discovered).await?;
    let changed = to_probe.len();
    tracing::info!(count = changed, "files new or changed since last scan");

    if changed == 0 {
        // Nothing to probe/write, but still a completed scan — record it so `GET /api/status`
        // shows a fresh "last scan" instead of looking stalled (`docs/.tasks/96`).
        if let Some(s) = &cfg.status {
            s.scan_finished(0, 0);
        }
        return Ok(());
    }

    // Fan out the probes under a semaphore; funnel results to a single writer task.
    let semaphore = Arc::new(Semaphore::new(cfg.probe_concurrency.max(1)));
    let (tx, mut rx) = mpsc::channel::<Probed>(cfg.probe_concurrency.max(1) * 2);

    // Writer task: the single WAL writer. Owns its own DB handle.
    //
    // It invalidates the response cache every `INVALIDATE_EVERY` rows, not just once
    // when the whole pass finishes. A first scan of a large library takes minutes-to-
    // hours (each ffprobe reads the file, slower still over an SMB mount); without
    // periodic invalidation the empty catalog page rendered at scan start stays cached
    // for the entire scan, so the UI grid looks empty even as thousands of rows land.
    // Busting the cache on a batch boundary makes the grid fill in progressively.
    let writer_db = db.clone();
    let writer_invalidate = invalidate.clone();
    let writer = tokio::spawn(async move {
        /// Rows between cache busts during a scan. Small enough that the grid fills in
        /// visibly, large enough that invalidation isn't per-row churn.
        const INVALIDATE_EVERY: usize = 25;
        let mut written = 0usize;
        while let Some(probed) = rx.recv().await {
            match write_one(&writer_db, probed).await {
                Ok(()) => {
                    written += 1;
                    if written % INVALIDATE_EVERY == 0 {
                        (*writer_invalidate)();
                    }
                }
                Err(err) => tracing::error!(error = %err, "failed to persist probed file"),
            }
        }
        written
    });

    // Spawn a bounded probe task per changed file.
    let mut probe_tasks = Vec::with_capacity(changed);
    for file in to_probe {
        let permit_sem = semaphore.clone();
        let tx = tx.clone();
        let probe_failures = probe_failures.clone();
        let probe_db = db.clone();
        probe_tasks.push(tokio::spawn(async move {
            // Acquire before spawning ffprobe so at most `concurrency` run at once.
            let _permit = permit_sem.acquire_owned().await.expect("semaphore open");
            match ffprobe::probe(&file.path).await {
                Ok((data, audio, mut subtitles, chapters)) => {
                    // External sidecars (`docs/.tasks/90`) are discovered by filename next
                    // to the video and merged with the embedded subtitle tracks.
                    subtitles.extend(scanner::discover_sidecars(&file.path));
                    tracing::info!(
                        path = %file.path.display(),
                        codec = data.video_codec.as_deref().unwrap_or("?"),
                        hdr = data.hdr_type.as_deref().unwrap_or("?"),
                        dv_profile = data.dv_profile.unwrap_or(-1),
                        hw_decode_unsupported = data.hw_decode_unsupported,
                        audio_tracks = audio.len(),
                        subtitle_tracks = subtitles.len(),
                        chapters = chapters.len(),
                        "probed file",
                    );
                    // A successful probe clears any prior recorded failure for this path
                    // (`docs/.tasks/96` Part C) so a re-encoded/replaced file drops off the
                    // status list. Best-effort — a clear failure is not fatal to the scan.
                    let path = file.path.to_string_lossy().into_owned();
                    let _ = clear_probe_failure(&probe_db, path).await;
                    // A closed receiver just means we're shutting down.
                    let _ = tx.send(Probed { file, data, audio, subtitles, chapters }).await;
                }
                Err(err) => {
                    tracing::warn!(path = %file.path.display(), error = %err, "ffprobe failed; skipping");
                    probe_failures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    // Persist the failure so `GET /api/status/probe-failures` can list it and
                    // the operator can find the bad file (`docs/.tasks/96` Part C).
                    let path = file.path.to_string_lossy().into_owned();
                    let _ = record_probe_failure(&probe_db, path, err.to_string()).await;
                }
            }
        }));
    }
    // Drop the sender clones so the writer's `recv` loop ends when probes finish.
    drop(tx);

    for t in probe_tasks {
        // A panicked probe task is logged, not fatal to the scan.
        if let Err(err) = t.await {
            tracing::error!(error = %err, "probe task panicked");
        }
    }

    let written = writer.await?;
    let failures = probe_failures.load(std::sync::atomic::Ordering::Relaxed);
    tracing::info!(written, probe_failures = failures, "ingest scan complete");
    if let Some(s) = &cfg.status {
        s.scan_finished(written as u64, failures);
    }

    if written > 0 {
        // `invalidate: &Arc<dyn Fn()>` — deref through the Arc to call the closure.
        (**invalidate)();

        // Auto-enrichment (`docs/.tasks/60` Phase A sub-task 6): a scan that wrote new
        // titles kicks a bounded enrichment pass over everything still `pending`. This is
        // what makes dropping a file into a watched folder fetch its metadata with no
        // manual step — `watch` → incremental `run_scan` → here. Enrichment invalidates
        // the cache again itself once art/overview land.
        if let Some(ctx) = &cfg.enrich {
            if let Err(err) = crate::enrich::run_enrichment(
                db,
                ctx,
                cfg.enrich_concurrency,
                invalidate,
                cfg.status.as_ref(),
            )
            .await
            {
                tracing::error!(error = %err, "metadata enrichment pass failed");
            }
        }
    }
    Ok(())
}

/// Record (or refresh) a persisted ffprobe failure for `path` (`docs/.tasks/96` Part C).
/// Best-effort: a DB error here is logged and swallowed so a probe failure never also fails
/// the scan. Runs on the blocking pool.
async fn record_probe_failure(db: &Db, path: String, error: String) -> anyhow::Result<()> {
    let db = db.clone();
    tokio::task::spawn_blocking(move || -> medi_db::DbResult<()> {
        let conn = db.conn()?;
        writes::upsert_probe_failure(&conn, &path, &error, now_secs())
    })
    .await?
    .map_err(|e| {
        tracing::warn!(error = %e, "failed to record probe failure");
        anyhow::anyhow!(e)
    })?;
    Ok(())
}

/// Clear a persisted ffprobe failure for `path` after a subsequent successful probe
/// (`docs/.tasks/96` Part C). Best-effort; a no-op if there was no recorded failure.
async fn clear_probe_failure(db: &Db, path: String) -> anyhow::Result<()> {
    let db = db.clone();
    tokio::task::spawn_blocking(move || -> medi_db::DbResult<()> {
        let conn = db.conn()?;
        writes::clear_probe_failure(&conn, &path)
    })
    .await??;
    Ok(())
}

/// A probed file plus its parsed metadata (video row + audio/subtitle/chapter children), on
/// its way to the writer task.
struct Probed {
    file: DiscoveredFile,
    data: MediaFileWrite,
    audio: Vec<AudioStreamWrite>,
    subtitles: Vec<SubtitleStreamWrite>,
    chapters: Vec<ChapterWrite>,
}

/// Diff discovered files against `scan_state`: keep only those never seen, changed
/// (mtime or size differs), or seen but never successfully probed. Runs on the
/// blocking pool.
async fn filter_changed(db: &Db, discovered: Vec<DiscoveredFile>) -> anyhow::Result<Vec<DiscoveredFile>> {
    let db = db.clone();
    let out = tokio::task::spawn_blocking(move || -> medi_db::DbResult<Vec<DiscoveredFile>> {
        let conn = db.conn()?;
        let mut keep = Vec::new();
        for f in discovered {
            let path = f.path.to_string_lossy();
            let prior = writes::get_scan_state(&conn, &path)?;
            let needs = match prior {
                None => true,
                Some((stat, probed_at)) => {
                    let current = FileStat {
                        mtime: f.mtime,
                        size_bytes: f.size_bytes,
                    };
                    // Re-probe on a changed stat / never-probed file, OR when the file's
                    // external subtitle sidecars have drifted (`docs/.tasks/99` detection
                    // fix): dropping a `.srt` next to an unchanged video doesn't touch the
                    // video's mtime/size, so without this the new sidecar is never picked up.
                    // The check is filename-only (one `read_dir`, no ffprobe), so it's cheap
                    // enough to run for every otherwise-skipped file.
                    stat != current
                        || probed_at.is_none()
                        || sidecars_drifted(&conn, &path, &f.path)
                }
            };
            if needs {
                keep.push(f);
            }
        }
        Ok(keep)
    })
    .await??;
    Ok(out)
}

/// Whether the external subtitle sidecars sitting next to `video_path` differ from the set
/// persisted for that file (`docs/.tasks/99` detection fix). Compares the fresh, filename-only
/// [`scanner::discover_sidecars`] scan against [`queries::external_subtitle_paths`] as sorted
/// sets of paths. `true` forces a re-probe so a newly-added (or removed) sidecar is detected
/// even when the video's own mtime/size are unchanged.
///
/// Best-effort: a DB read error here is treated as "no drift" (`false`) so a transient error
/// never spuriously re-probes the whole library — the changed-stat / never-probed checks still
/// cover the common cases.
fn sidecars_drifted(conn: &rusqlite::Connection, path_str: &str, video_path: &Path) -> bool {
    let persisted = match medi_db::queries::external_subtitle_paths(conn, path_str) {
        Ok(mut p) => {
            p.sort();
            p
        }
        Err(_) => return false,
    };
    let mut discovered: Vec<String> = scanner::discover_sidecars(video_path)
        .into_iter()
        .filter_map(|s| s.external_path)
        .collect();
    discovered.sort();
    persisted != discovered
}

/// Persist one probed file inside a single transaction: upsert the scan-state stat,
/// find-or-create the owning movie/episode, upsert the `media_files` row, then stamp
/// `probed_at`. Runs on the blocking pool (the single WAL writer).
async fn write_one(db: &Db, probed: Probed) -> anyhow::Result<()> {
    let db = db.clone();
    tokio::task::spawn_blocking(move || -> medi_db::DbResult<()> {
        let mut conn = db.conn()?;
        let tx = conn.transaction()?;

        let path = probed.file.path.to_string_lossy().into_owned();
        let now = now_secs();

        // Record the stat first so a crash mid-probe still leaves a scan_state row
        // (with probed_at NULL) that a re-scan will retry.
        writes::upsert_scan_state(
            &tx,
            &path,
            FileStat {
                mtime: probed.file.mtime,
                size_bytes: probed.file.size_bytes,
            },
        )?;

        let owner = resolve_owner(&tx, &probed.file.class, probed.file.library_id, now)?;
        let media_file_id = writes::upsert_media_file(&tx, &path, owner, &probed.data)?;
        // Audio + subtitle tracks are child tables (Tasks 70 / 90); overwrite them in the
        // same transaction so a re-probe replaces each whole set atomically.
        writes::replace_audio_streams(&tx, media_file_id, &probed.audio)?;
        writes::replace_subtitle_streams(&tx, media_file_id, &probed.subtitles)?;
        // Chapters (Task 99) — same atomic replace-on-reprobe contract.
        writes::replace_chapters(&tx, media_file_id, &probed.chapters)?;
        writes::mark_probed(&tx, &path, now)?;

        tx.commit()?;
        Ok(())
    })
    .await??;
    Ok(())
}

/// Find-or-create the owning movie/episode for a classification and return the
/// [`FileOwner`] the media_files row attaches to. When `library_id` is set (Phase B),
/// the owning movie/series is scoped to that library so scans and reaps are per-library.
fn resolve_owner(
    conn: &rusqlite::Connection,
    class: &Classification,
    library_id: Option<i64>,
    now: i64,
) -> medi_db::DbResult<FileOwner> {
    match class {
        Classification::Movie { title, year } => {
            let sort = scanner::sort_title(title);
            let id = writes::find_or_create_movie(conn, title, &sort, *year, now)?;
            if let Some(lib) = library_id {
                writes::set_movie_library(conn, id, lib)?;
            }
            Ok(FileOwner::Movie(id))
        }
        Classification::Episode {
            series_title,
            series_year,
            season,
            episode,
            title,
        } => {
            let sort = scanner::sort_title(series_title);
            let series_id =
                writes::find_or_create_series(conn, series_title, &sort, *series_year, now)?;
            if let Some(lib) = library_id {
                writes::set_series_library(conn, series_id, lib)?;
            }
            let season_id = writes::find_or_create_season(conn, series_id, *season)?;
            let episode_id =
                writes::find_or_create_episode(conn, season_id, *episode, title.as_deref())?;
            Ok(FileOwner::Episode(episode_id))
        }
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Watch mode (sub-task 5): notify → debounced incremental re-scan.
// ---------------------------------------------------------------------------

/// Watch `media_dir` for filesystem changes **and** manual scan triggers, running an
/// incremental re-scan whenever activity settles. Runs until the process exits.
///
/// `notify` fires many events for one logical change (a copy emits a burst), so events
/// are debounced: after the last event we wait `DEBOUNCE` quiet time before scanning,
/// coalescing a burst into a single pass. Because [`run_scan`] is idempotent, an
/// over-eager trigger only costs a `scan_state` diff, not re-probing.
///
/// `scan_signal` (from [`ScanTrigger::new`]) is a second event source: the API fires it on
/// library create / `POST /api/libraries/:id/scan`, so a library pointed at already-present
/// files is scanned immediately instead of waiting for the next fs change. Both sources feed
/// the same debounced loop, so a trigger during a copy burst just coalesces in.
pub async fn watch(
    db: Db,
    cfg: WorkerConfig,
    invalidate: Invalidator,
    mut scan_signal: ScanSignal,
) -> anyhow::Result<()> {
    use notify::{RecursiveMode, Watcher};

    /// Quiet period after the last fs event before an incremental scan fires.
    const DEBOUNCE: Duration = Duration::from_secs(3);

    let (evt_tx, mut evt_rx) = mpsc::unbounded_channel::<()>();

    // The notify callback runs on notify's own thread; forward a tick to our task.
    let notify_tx = evt_tx.clone();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        match res {
            Ok(event) if is_relevant(&event) => {
                let _ = notify_tx.send(());
            }
            Ok(_) => {}
            Err(err) => tracing::warn!(error = %err, "watch error"),
        }
    })?;

    watcher.watch(&cfg.media_dir, RecursiveMode::Recursive)?;
    tracing::info!(root = %cfg.media_dir.display(), "watching /media for changes");
    // The watcher is live — surface it in `GET /api/status` (`docs/.tasks/96`).
    if let Some(s) = &cfg.status {
        s.watcher_started();
    }

    // Keep the watcher alive for the lifetime of this task.
    let _watcher = watcher;

    // Forward manual scan triggers (API) into the same event channel as fs events, so the
    // one debounced loop below handles both. This task ends when either the trigger senders
    // are all dropped or `evt_tx` is closed (worker teardown).
    tokio::spawn(async move {
        while scan_signal.rx.recv().await.is_some() {
            tracing::info!("manual scan trigger received");
            if evt_tx.send(()).is_err() {
                break; // watch loop gone
            }
        }
    });

    loop {
        // Block until the first event of a burst.
        if evt_rx.recv().await.is_none() {
            break; // all senders dropped → shutting down
        }
        // Drain the burst: keep resetting the debounce timer while events keep coming.
        loop {
            match tokio::time::timeout(DEBOUNCE, evt_rx.recv()).await {
                Ok(Some(())) => continue,        // another event; keep waiting
                Ok(None) => return Ok(()),       // channel closed
                Err(_) => break,                 // quiet for DEBOUNCE → scan now
            }
        }
        tracing::info!("running incremental scan (fs change or manual trigger)");
        if let Err(err) = run_scan(&db, &cfg, &invalidate).await {
            tracing::error!(error = %err, "incremental scan failed");
        }
    }
    Ok(())
}

/// Only data-changing events warrant a re-scan; access/metadata-only events do not.
fn is_relevant(event: &notify::Event) -> bool {
    use notify::EventKind;
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

/// Reap `scan_state`/`media_files` rows for files that no longer exist under
/// `media_dir`. Called opportunistically (e.g. before a watch-triggered scan) so the
/// catalog does not accumulate ghosts. Left unused by the hot path in Phase 1 but
/// exposed for the worker/tests.
pub async fn reap_missing(db: &Db, present: &HashSet<PathBuf>) -> anyhow::Result<usize> {
    let db = db.clone();
    let present: HashSet<String> = present
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let removed = tokio::task::spawn_blocking(move || -> medi_db::DbResult<usize> {
        let conn = db.conn()?;
        let known: Vec<String> = {
            let mut stmt = conn.prepare("SELECT path FROM scan_state")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        let mut removed = 0;
        for path in known {
            if !present.contains(&path) {
                writes::delete_file(&conn, &path)?;
                removed += 1;
            }
        }
        Ok(removed)
    })
    .await??;
    Ok(removed)
}

/// Ensure a directory path is watchable — used by callers to fail fast if `/media` is
/// missing at boot rather than silently never ingesting.
pub fn media_dir_exists(media_dir: &Path) -> bool {
    media_dir.is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = medi_db::open(dir.path().join("library.db"), 2).unwrap();
        (db, dir)
    }

    fn probed_movie(path: &str, hdr: &str, dv: Option<i64>) -> Probed {
        Probed {
            file: DiscoveredFile {
                path: PathBuf::from(path),
                mtime: 10,
                size_bytes: 1000,
                class: Classification::Movie {
                    title: "Arrival".into(),
                    year: Some(2016),
                },
                library_id: None,
            },
            data: MediaFileWrite {
                container: Some("mkv".into()),
                video_codec: Some("hevc".into()),
                width: Some(3840),
                height: Some(2160),
                bit_depth: Some(10),
                hdr_type: Some(hdr.into()),
                dv_profile: dv,
                ..Default::default()
            },
            audio: vec![AudioStreamWrite {
                stream_index: 1,
                codec: Some("eac3".into()),
                channels: Some(6),
                channel_layout: Some("5.1".into()),
                immersive: "none".into(),
                is_default: true,
                ..Default::default()
            }],
            subtitles: Vec::new(),
            chapters: Vec::new(),
        }
    }

    #[tokio::test]
    async fn write_then_filter_is_idempotent() {
        let (db, _dir) = temp_db();

        // Persist one probed movie file.
        write_one(&db, probed_movie("/media/arrival.mkv", "dolbyvision", Some(5)))
            .await
            .unwrap();

        // The movie + media file + scan_state now exist.
        {
            let conn = db.conn().unwrap();
            let movies = medi_db::queries::list_movies(&conn, None, 10).unwrap();
            assert_eq!(movies.len(), 1);
            assert_eq!(movies[0].title, "Arrival");
            let files = medi_db::queries::media_files_for_movie(&conn, movies[0].id).unwrap();
            assert_eq!(files.len(), 1);
            assert_eq!(files[0].dv_profile, Some(5));
            // The default audio track was persisted and read back (Task 70).
            assert_eq!(files[0].audio_streams.len(), 1);
            assert_eq!(files[0].audio_streams[0].codec.as_deref(), Some("eac3"));
            assert!(files[0].audio_streams[0].is_default);
            let (_stat, probed_at) =
                writes::get_scan_state(&conn, "/media/arrival.mkv").unwrap().unwrap();
            assert!(probed_at.is_some(), "probed_at stamped after write");
        }

        // A re-scan of the same, unchanged file re-probes nothing (idempotent).
        let same = DiscoveredFile {
            path: PathBuf::from("/media/arrival.mkv"),
            mtime: 10,
            size_bytes: 1000,
            class: Classification::Movie {
                title: "Arrival".into(),
                year: Some(2016),
            },
            library_id: None,
        };
        let changed = filter_changed(&db, vec![same]).await.unwrap();
        assert!(changed.is_empty(), "unchanged probed file is skipped");

        // A changed size marks it for re-probe.
        let bigger = DiscoveredFile {
            path: PathBuf::from("/media/arrival.mkv"),
            mtime: 10,
            size_bytes: 2000,
            class: Classification::Movie {
                title: "Arrival".into(),
                year: Some(2016),
            },
            library_id: None,
        };
        // The stat has to be recorded (as a fresh scan would) before the diff sees it.
        {
            let conn = db.conn().unwrap();
            writes::upsert_scan_state(
                &conn,
                "/media/arrival.mkv",
                FileStat { mtime: 10, size_bytes: 2000 },
            )
            .unwrap();
        }
        let changed2 = filter_changed(&db, vec![bigger]).await.unwrap();
        assert_eq!(changed2.len(), 1, "changed file is re-probed");
    }

    #[tokio::test]
    async fn added_sidecar_forces_reprobe_of_unchanged_video() {
        // The detection fix (`docs/.tasks/99`): an already-probed, unchanged video whose
        // sibling folder gains a `.srt` sidecar is re-probed even though its own mtime/size
        // are untouched — otherwise the new subtitle would never be discovered.
        let (db, _dir) = temp_db();
        let media = tempfile::tempdir().unwrap();
        let video = media.path().join("Arrival (2016).mkv");
        std::fs::write(&video, b"x").unwrap();

        // Probe + persist the video (no sidecars yet).
        let mut probed = probed_movie(video.to_str().unwrap(), "hdr10", None);
        probed.file.mtime = 10;
        probed.file.size_bytes = 1;
        write_one(&db, probed).await.unwrap();

        let file = DiscoveredFile {
            path: video.clone(),
            mtime: 10,
            size_bytes: 1,
            class: Classification::Movie { title: "Arrival".into(), year: Some(2016) },
            library_id: None,
        };

        // No sidecar on disk → unchanged, already-probed file is skipped.
        let none = filter_changed(&db, vec![file.clone()]).await.unwrap();
        assert!(none.is_empty(), "no sidecar drift → not re-probed");

        // Drop a matching sidecar next to the (still unchanged) video → drift → re-probe.
        std::fs::write(media.path().join("Arrival (2016).en.srt"), b"1\n").unwrap();
        let drifted = filter_changed(&db, vec![file]).await.unwrap();
        assert_eq!(drifted.len(), 1, "added sidecar forces a re-probe of the unchanged video");
    }

    #[tokio::test]
    async fn episode_write_builds_series_chain() {
        let (db, _dir) = temp_db();
        let probed = Probed {
            file: DiscoveredFile {
                path: PathBuf::from("/media/Severance/S01E01.mkv"),
                mtime: 1,
                size_bytes: 500,
                class: Classification::Episode {
                    series_title: "Severance".into(),
                    series_year: Some(2022),
                    season: 1,
                    episode: 1,
                    title: Some("Good News About Hell".into()),
                },
                library_id: None,
            },
            data: MediaFileWrite {
                video_codec: Some("hevc".into()),
                hdr_type: Some("hdr10".into()),
                width: Some(3840),
                height: Some(2160),
                ..Default::default()
            },
            audio: Vec::new(),
            subtitles: Vec::new(),
            chapters: Vec::new(),
        };
        write_one(&db, probed).await.unwrap();

        let conn = db.conn().unwrap();
        let series = medi_db::queries::list_series(&conn, None, 10).unwrap();
        assert_eq!(series.len(), 1);
        let detail = medi_db::queries::get_series_detail(&conn, series[0].id).unwrap();
        assert_eq!(detail.seasons.len(), 1);
        assert_eq!(detail.seasons[0].episodes.len(), 1);
        assert_eq!(
            detail.seasons[0].episodes[0].episode.title.as_deref(),
            Some("Good News About Hell")
        );
    }

    #[tokio::test]
    async fn reap_removes_absent_files() {
        let (db, _dir) = temp_db();
        write_one(&db, probed_movie("/media/gone.mkv", "none", None))
            .await
            .unwrap();

        // Nothing is present → the file is reaped.
        let present = HashSet::new();
        let removed = reap_missing(&db, &present).await.unwrap();
        assert_eq!(removed, 1);

        let conn = db.conn().unwrap();
        assert!(writes::get_scan_state(&conn, "/media/gone.mkv").unwrap().is_none());
    }

    #[tokio::test]
    async fn write_one_scopes_owner_to_library() {
        // A discovered file tagged with a library_id scopes its owning movie to that
        // library (Phase B). No ffprobe needed — write_one takes an already-probed file.
        let (db, _dir) = temp_db();
        let lib = {
            let conn = db.conn().unwrap();
            writes::create_library(&conn, "Films", writes::TitleKind::Movie, 0).unwrap()
        };
        let mut probed = probed_movie("/media/movies/arrival.mkv", "hdr10", None);
        probed.file.library_id = Some(lib);
        write_one(&db, probed).await.unwrap();

        let conn = db.conn().unwrap();
        let movies = medi_db::queries::list_movies(&conn, None, 10).unwrap();
        assert_eq!(movies.len(), 1);
        let scoped: Option<i64> = conn
            .query_row(
                "SELECT library_id FROM movies WHERE id = ?1",
                rusqlite::params![movies[0].id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(scoped, Some(lib), "movie scoped to its library");
    }

    #[tokio::test]
    async fn scan_root_tags_library_and_applies_kind_override() {
        // A Movies-library scan of a real temp tree tags every file with the library id
        // and forces movie classification even for an episode-marked filename.
        let tmp = tempfile::tempdir().unwrap();
        let movies_dir = tmp.path().join("movies");
        std::fs::create_dir_all(&movies_dir).unwrap();
        // An episode-marked file that must still classify as a movie under a Movies lib.
        std::fs::write(movies_dir.join("Weird S01E01 (2020).mkv"), b"x").unwrap();

        let discovered = scanner::scan_root(&movies_dir, Some(42), scanner::KindHint::Movie);
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].library_id, Some(42));
        assert!(
            matches!(discovered[0].class, Classification::Movie { .. }),
            "kind override forces movie: {:?}",
            discovered[0].class
        );
    }

    #[tokio::test]
    async fn scan_trigger_delivers_and_no_ops_after_drop() {
        // A trigger delivers a tick to the signal receiver, and after the receiver is
        // dropped (worker not running) `trigger()` is a harmless no-op — the API can fire
        // it fire-and-forget without caring whether ingestion is up.
        let (trigger, mut signal) = ScanTrigger::new();
        trigger.trigger();
        assert!(signal.rx.recv().await.is_some(), "trigger delivers a tick");

        drop(signal);
        // No panic, no error — the closed-channel send is swallowed.
        trigger.trigger();
    }
}
