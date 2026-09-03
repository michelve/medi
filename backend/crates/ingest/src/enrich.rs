//! The ingest-side enrichment pass (`docs/.tasks/60` §Sub-tasks 6).
//!
//! Phase A drops metadata enrichment into the *already-live* watch/scan loop: after a
//! scan persists new titles, this pass finds every title still awaiting metadata
//! (`metadata_state` in `pending`/`failed`) and enriches it with **bounded concurrency**
//! (a semaphore, mirroring the ffprobe fan-out) so a first-run scan of a large library
//! respects the provider's rate limits. Because `main.rs` already runs an initial
//! `run_scan` then a debounced `watch` → incremental `run_scan`, a newly dropped file is
//! auto-enriched with no extra wiring — the auto-detect-on-add feature.
//!
//! The pass is idempotent: `enrich_movie`/`enrich_series` skip `matched` (and, without
//! force, `unmatched`) rows, so re-running is cheap and restart-resumable (pending rows
//! persist in the DB).

use std::sync::Arc;

use tokio::sync::Semaphore;

use medi_db::writes::TitleKind;
use medi_db::Db;
use medi_metadata::EnrichContext;

use crate::worker::Invalidator;

/// How many titles a single enrichment pass pulls per kind. Bounds memory and keeps each
/// pass checkpointed to the DB (the next pass resumes with whatever is still pending).
const BATCH: u32 = 500;

/// Enrich every pending title with bounded concurrency, then invalidate the response
/// cache if any title was matched. Called by [`crate::worker::run_scan`] after a scan
/// that wrote rows, and safe to call independently.
///
/// `concurrency` caps in-flight provider round-trips (the provider also bounds its own
/// HTTP fan-out); a small multiple keeps a 10k-title backfill from hammering the API.
pub async fn run_enrichment(
    db: &Db,
    ctx: &EnrichContext,
    concurrency: usize,
    invalidate: &Invalidator,
    status: Option<&crate::status::EnrichmentStatus>,
) -> anyhow::Result<()> {
    let mut any_matched = false;
    // Tallies for `GET /api/status` (`docs/.tasks/96`): how this pass resolved.
    let mut matched_total = 0u64;
    let mut unmatched_total = 0u64;
    let mut failed_total = 0u64;
    // Process movies then series. Each kind pulls one bounded batch per DB read and
    // loops until the batch is short (backlog drained). A batch that makes no forward
    // progress (every row stayed `pending`/`failed` — e.g. a persistent provider outage)
    // stops the loop so a transient failure never spins; those rows are retried on the
    // next scan-triggered pass.
    for kind in [TitleKind::Movie, TitleKind::Series] {
        loop {
            let pending = {
                let db = db.clone();
                tokio::task::spawn_blocking(move || -> medi_db::DbResult<Vec<_>> {
                    let conn = db.conn()?;
                    medi_db::queries::list_pending_metadata(&conn, kind, BATCH)
                })
                .await??
            };
            let batch_len = pending.len() as u32;
            if batch_len == 0 {
                break;
            }
            tracing::info!(count = batch_len, kind = ?kind, "enriching pending titles");

            let sem = Arc::new(Semaphore::new(concurrency.max(1)));
            let mut tasks = Vec::with_capacity(pending.len());
            for title in pending {
                let permit_sem = sem.clone();
                let ctx = ctx.clone();
                tasks.push(tokio::spawn(async move {
                    let _permit = permit_sem.acquire_owned().await.expect("semaphore open");
                    let res = match kind {
                        TitleKind::Movie => medi_metadata::enrich_movie(&ctx, title.id, false).await,
                        TitleKind::Series => medi_metadata::enrich_series(&ctx, title.id, false).await,
                    };
                    // Return an outcome tag so the caller can tally matched/unmatched/failed
                    // for `GET /api/status` (`docs/.tasks/96`): 0=matched, 1=unmatched,
                    // 2=skipped(terminal, no tally), 3=failed(no progress).
                    match res {
                        Ok(medi_metadata::EnrichOutcome::Matched { .. }) => 0u8,
                        Ok(medi_metadata::EnrichOutcome::Unmatched) => 1u8,
                        Ok(medi_metadata::EnrichOutcome::Skipped) => 2u8,
                        Err(err) => {
                            tracing::warn!(id = title.id, error = %err, "enrichment failed");
                            3u8 // stayed pending/failed → no progress
                        }
                    }
                }));
            }
            let mut progressed = 0u32;
            for t in tasks {
                match t.await {
                    Ok(tag) => {
                        match tag {
                            0 => {
                                any_matched = true;
                                matched_total += 1;
                                progressed += 1;
                            }
                            1 => {
                                unmatched_total += 1;
                                progressed += 1;
                            }
                            2 => progressed += 1, // skipped: terminal, not counted
                            _ => failed_total += 1, // no progress
                        }
                    }
                    Err(err) => tracing::error!(error = %err, "enrichment task panicked"),
                }
            }
            // Drained (short batch) or stalled (no row left the pending set) → stop.
            if batch_len < BATCH || progressed == 0 {
                break;
            }
        }
    }

    if any_matched {
        (**invalidate)();
    }
    if let Some(s) = status {
        s.enrichment_finished(matched_total, unmatched_total, failed_total);
    }
    Ok(())
}
