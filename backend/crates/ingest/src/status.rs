//! Shared, in-memory enrichment/scan status (`docs/.tasks/96` Part A).
//!
//! The scheduler + enrichment pipeline already run, but they were a black box: the only
//! signal was `tracing` logs an operator never reads. This module holds a small, cheaply
//! cloneable handle the worker **writes** (last scan/enrichment tallies, watcher liveness)
//! and the API **reads** to render `GET /api/status`. It is deliberately ephemeral —
//! durable counts (how many titles are matched/pending/…) come from the DB, and this
//! resets to "nothing has run yet" on restart, which is correct.
//!
//! One `Arc<RwLock<…>>` shared by every clone; updates are short, contended rarely, and
//! never on a hot request path.

use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// What the most recent ingest scan did (`docs/.tasks/96`). `None` fields mean "no scan has
/// finished yet this process".
#[derive(Debug, Clone, Copy, Default)]
pub struct LastScan {
    /// Unix seconds the scan started, or `None` if none has started.
    pub started_at: Option<i64>,
    /// Unix seconds the scan finished, or `None` while one is in flight / none has run.
    pub finished_at: Option<i64>,
    /// Rows written (new/changed files persisted) by that scan.
    pub written: u64,
    /// How many files that scan failed to ffprobe (skipped).
    pub probe_failures: u64,
}

/// What the most recent enrichment pass did (`docs/.tasks/96`).
#[derive(Debug, Clone, Copy, Default)]
pub struct LastEnrichment {
    /// Unix seconds the pass finished, or `None` if none has run.
    pub finished_at: Option<i64>,
    /// Titles matched (metadata written) in that pass.
    pub matched: u64,
    /// Titles that cleared no candidate → marked unmatched in that pass.
    pub unmatched: u64,
    /// Titles whose provider call errored (stayed pending/failed) in that pass.
    pub failed: u64,
}

/// The mutable inner state behind the shared lock.
#[derive(Debug, Clone, Copy, Default)]
struct Inner {
    last_scan: LastScan,
    last_enrichment: LastEnrichment,
    /// Set true once the fs-watch loop is running; a proxy for "the worker is alive".
    watcher_alive: bool,
}

/// A cheaply cloneable handle to the shared enrichment/scan status. Clones share one inner
/// `RwLock`, so an update through any clone is visible to all.
#[derive(Clone, Default)]
pub struct EnrichmentStatus {
    inner: Arc<RwLock<Inner>>,
}

/// A snapshot of the status for serialization by the API (`docs/.tasks/96`).
#[derive(Debug, Clone, Copy, Default)]
pub struct StatusSnapshot {
    pub last_scan: LastScan,
    pub last_enrichment: LastEnrichment,
    pub watcher_alive: bool,
}

impl EnrichmentStatus {
    /// A fresh status handle (nothing has run yet).
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that a scan just started (stamps `started_at`, clears the prior `finished_at`).
    pub fn scan_started(&self) {
        if let Ok(mut g) = self.inner.write() {
            g.last_scan.started_at = Some(now_secs());
            g.last_scan.finished_at = None;
        }
    }

    /// Record that a scan finished with the given tallies.
    pub fn scan_finished(&self, written: u64, probe_failures: u64) {
        if let Ok(mut g) = self.inner.write() {
            g.last_scan.finished_at = Some(now_secs());
            g.last_scan.written = written;
            g.last_scan.probe_failures = probe_failures;
        }
    }

    /// Record that an enrichment pass finished with the given tallies.
    pub fn enrichment_finished(&self, matched: u64, unmatched: u64, failed: u64) {
        if let Ok(mut g) = self.inner.write() {
            g.last_enrichment = LastEnrichment {
                finished_at: Some(now_secs()),
                matched,
                unmatched,
                failed,
            };
        }
    }

    /// Mark the fs-watch loop as running (called once when `watch` starts).
    pub fn watcher_started(&self) {
        if let Ok(mut g) = self.inner.write() {
            g.watcher_alive = true;
        }
    }

    /// Snapshot the current status for the API.
    pub fn snapshot(&self) -> StatusSnapshot {
        let g = self.inner.read().ok();
        match g {
            Some(g) => StatusSnapshot {
                last_scan: g.last_scan,
                last_enrichment: g.last_enrichment,
                watcher_alive: g.watcher_alive,
            },
            None => StatusSnapshot::default(),
        }
    }
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

    #[test]
    fn records_and_snapshots() {
        let s = EnrichmentStatus::new();
        let snap = s.snapshot();
        assert!(snap.last_scan.started_at.is_none());
        assert!(!snap.watcher_alive);

        s.scan_started();
        s.scan_finished(3, 1);
        s.enrichment_finished(2, 5, 0);
        s.watcher_started();

        let snap = s.snapshot();
        assert!(snap.last_scan.started_at.is_some());
        assert!(snap.last_scan.finished_at.is_some());
        assert_eq!(snap.last_scan.written, 3);
        assert_eq!(snap.last_scan.probe_failures, 1);
        assert_eq!(snap.last_enrichment.matched, 2);
        assert_eq!(snap.last_enrichment.unmatched, 5);
        assert!(snap.watcher_alive);
    }

    #[test]
    fn clones_share_state() {
        let a = EnrichmentStatus::new();
        let b = a.clone();
        a.scan_finished(7, 0);
        assert_eq!(b.snapshot().last_scan.written, 7, "clone sees the update");
    }
}
