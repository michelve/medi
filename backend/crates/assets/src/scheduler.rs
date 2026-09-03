//! Off-peak scheduling + GPU-idle guard + concurrency throttle (`docs/.tasks/30`
//! §Off-peak scheduling & GPU guard, sub-task 1).
//!
//! Background asset generation must never compete with user-requested streaming, so
//! before every asset job the worker checks two gates and holds a semaphore:
//!
//! 1. **Off-peak window** — the current hour must fall inside the configured
//!    `[offpeak_start_hour, offpeak_end_hour)` window (`AppConfig::in_offpeak_window`).
//! 2. **GPU-idle guard** — there must be no live transcode session (or a count below a
//!    threshold). Live streams own the GPU; asset generation yields to them.
//! 3. **Throttle** — a [`tokio::sync::Semaphore`] bounds how many asset ffmpeg jobs run
//!    at once, keeping them low-priority relative to live streams.
//!
//! The guard is a *gate*, not a one-shot check: [`Scheduler::wait_until_runnable`]
//! sleeps and re-checks until both the window is open and the GPU is idle, so a job
//! that becomes eligible mid-sleep starts promptly, and a live transcode starting
//! mid-generation pauses the *next* job (an in-flight ffmpeg is not killed — it is
//! short, ~15s, and finishes).
//!
//! ## Time source
//!
//! Hour-of-day is derived from the system clock in **UTC** (no `chrono`/`time`
//! dependency; `docs/.tasks/00-architecture.md` keeps the crate set minimal). An
//! operator setting the window accounts for the container's clock; the Docker image
//! runs UTC unless `TZ` is set, in which case the kernel's `localtime` still reports
//! UTC seconds here. This is documented so the window is interpreted consistently.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use medi_core::AppConfig;
use medi_transcode::SessionManager;

/// How long to sleep between guard re-checks when a gate is closed (window not yet
/// open, or a live transcode is running). Short enough to react promptly, long enough
/// not to busy-spin.
const RECHECK_INTERVAL: Duration = Duration::from_secs(30);

/// The maximum number of live transcode sessions tolerated while still generating
/// assets. `0` = strict (any live stream pauses generation), the default per
/// `docs/.tasks/30`. Exposed as a constant so a future config could relax it.
const LIVE_SESSION_THRESHOLD: usize = 0;

/// Gates and throttles background asset jobs behind the off-peak window, the GPU-idle
/// guard, and a concurrency semaphore. Cheap to clone (shares one `Arc`'d semaphore and
/// the cloneable [`SessionManager`]).
#[derive(Clone)]
pub struct Scheduler {
    config: Arc<AppConfig>,
    transcode: SessionManager,
    /// Bounds concurrent asset ffmpeg jobs (`ASSET_MAX_CONCURRENCY`).
    permits: Arc<Semaphore>,
}

impl Scheduler {
    /// Build a scheduler from the resolved config and the live-session manager it must
    /// yield to. The semaphore is sized from `AppConfig::asset_max_concurrency`.
    pub fn new(config: Arc<AppConfig>, transcode: SessionManager) -> Self {
        let n = config.asset_max_concurrency.max(1) as usize;
        Self {
            config,
            transcode,
            permits: Arc::new(Semaphore::new(n)),
        }
    }

    /// Is the current hour inside the off-peak window?
    pub fn window_open(&self) -> bool {
        self.config.in_offpeak_window(current_hour_utc())
    }

    /// Is the GPU idle enough for background work — i.e. live transcode sessions at or
    /// below the threshold?
    ///
    /// `<=` (not `==`) is deliberate: `LIVE_SESSION_THRESHOLD` is currently `0` but is a
    /// constant so a future config can *relax* it to a positive count, at which point the
    /// inequality is the load-bearing comparison. clippy flags the `<= 0` case as absurd
    /// today (0 is `usize`'s minimum), so allow it here rather than hard-code `== 0` and
    /// silently break the relax-later intent.
    #[allow(clippy::absurd_extreme_comparisons)]
    pub async fn gpu_idle(&self) -> bool {
        self.transcode.active_count().await <= LIVE_SESSION_THRESHOLD
    }

    /// Both gates open right now?
    pub async fn runnable(&self) -> bool {
        self.window_open() && self.gpu_idle().await
    }

    /// Block until both gates are open (window in off-peak AND GPU idle), then acquire
    /// and return a throttle permit. Holding the returned permit bounds concurrency;
    /// drop it when the job finishes to free the slot.
    ///
    /// Re-checks every [`RECHECK_INTERVAL`] while a gate is closed. Because the permit
    /// is acquired *after* the gates pass, a burst of eligible files still serializes to
    /// `asset_max_concurrency` concurrent ffmpeg jobs.
    pub async fn wait_until_runnable(&self) -> OwnedSemaphorePermit {
        loop {
            if self.window_open() {
                if self.gpu_idle().await {
                    // Gates open — take a throttle slot. `acquire_owned` only fails if
                    // the semaphore is closed, which we never do.
                    return self
                        .permits
                        .clone()
                        .acquire_owned()
                        .await
                        .expect("assets semaphore open");
                }
                tracing::debug!("assets: live transcode in progress; yielding GPU");
            } else {
                tracing::trace!("assets: outside off-peak window; sleeping");
            }
            tokio::time::sleep(RECHECK_INTERVAL).await;
        }
    }

    /// The output directory for hover previews (`/config/previews`).
    pub fn previews_dir(&self) -> std::path::PathBuf {
        self.config.previews_dir()
    }

    /// The output directory for trickplay sprites (`/config/trickplay`).
    pub fn trickplay_dir(&self) -> std::path::PathBuf {
        self.config.trickplay_dir()
    }
}

/// Current hour of day (0–23), UTC, from the system clock. See the module time-source
/// note. Returns 0 if the clock is before the epoch (never in practice).
fn current_hour_utc() -> u8 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    ((secs / 3600) % 24) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use medi_transcode::HwCaps;

    fn scheduler_with(cfg: AppConfig, cap: usize) -> Scheduler {
        let mgr = SessionManager::new(
            std::env::temp_dir().join("medi-assets-test-hls"),
            cap,
            HwCaps::software_only(),
        );
        Scheduler::new(Arc::new(cfg), mgr)
    }

    #[test]
    fn current_hour_is_in_range() {
        let h = current_hour_utc();
        assert!(h < 24);
    }

    #[tokio::test]
    async fn gpu_idle_when_no_sessions() {
        let cfg = AppConfig::default();
        let sched = scheduler_with(cfg, 4);
        assert!(sched.gpu_idle().await, "no live sessions ⇒ GPU idle");
    }

    #[tokio::test]
    async fn window_gate_reflects_config() {
        // A degenerate window (start == end) is always open, so `window_open` is true
        // regardless of the wall clock — lets the test assert without mocking time.
        let mut cfg = AppConfig::default();
        cfg.offpeak_start_hour = 4;
        cfg.offpeak_end_hour = 4;
        let sched = scheduler_with(cfg, 1);
        assert!(sched.window_open());
        assert!(sched.runnable().await);
    }

    #[tokio::test]
    async fn permit_throttles_to_capacity() {
        let mut cfg = AppConfig::default();
        // Always-open window so the gate never blocks the throttle test.
        cfg.offpeak_start_hour = 0;
        cfg.offpeak_end_hour = 0;
        cfg.asset_max_concurrency = 1;
        let sched = scheduler_with(cfg, 1);

        let p1 = sched.wait_until_runnable().await;
        // With capacity 1, a second acquire cannot complete while p1 is held.
        let second =
            tokio::time::timeout(Duration::from_millis(100), sched.wait_until_runnable()).await;
        assert!(second.is_err(), "second permit blocked at capacity 1");

        drop(p1);
        // Now it proceeds.
        let p2 = tokio::time::timeout(Duration::from_secs(1), sched.wait_until_runnable()).await;
        assert!(p2.is_ok(), "permit available after the first is dropped");
    }
}
