//! HLS transcode session lifecycle (`docs/.tasks/20` sub-task 4).
//!
//! A *session* is one running `jellyfin-ffmpeg` process writing fMP4/CMAF HLS into
//! `/config/hls/<session_id>/`. [`SessionManager`] creates sessions (bounded by a
//! GPU-capacity cap → `409` past it), tracks them, serves their generated files to the
//! `/api/hls/:session_id/:file` route, and tears them down on idle or on demand
//! (killing the process and cleaning the directory).
//!
//! ## Lifecycle
//! 1. `/api/stream` runs `decision::decide`; on a transcode it calls [`SessionManager::start`].
//! 2. `start` allocates a random session id, spawns ffmpeg, and returns the playlist URL.
//! 3. The client polls `/api/hls/<id>/index.m3u8` and pulls `init.mp4` + `seg*.m4s`.
//! 4. An idle reaper drops sessions with no file access for [`IDLE_TIMEOUT`], killing
//!    the process and removing the directory.
//!
//! The manager is `Clone` (shares one `Arc` registry) so it lives in `AppState`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use crate::caps::{ffmpeg_bin, HwCaps};
use crate::command::{self, AudioTarget, PLAYLIST_NAME};
use crate::decision::TranscodeTarget;

/// How long a session may go without a file access before the reaper kills it.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Errors from the session layer.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("transcode capacity reached ({0} concurrent sessions)")]
    CapacityReached(usize),

    #[error("no such session")]
    NotFound,

    #[error("failed to spawn transcoder: {0}")]
    Spawn(#[source] std::io::Error),

    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

/// One live transcode session.
struct Session {
    /// The running ffmpeg child (killed on teardown).
    child: Child,
    /// Output directory `/config/hls/<id>`.
    dir: PathBuf,
    /// Last time a file of this session was requested — drives idle teardown.
    last_access: Instant,
}

/// Shared, cloneable manager of all live transcode sessions.
#[derive(Clone)]
pub struct SessionManager {
    inner: Arc<Manager>,
}

struct Manager {
    /// `/config/hls` — the root under which each session gets a subdir.
    hls_root: PathBuf,
    /// Max concurrent sessions (GPU-capacity cap). Past it, `start` returns
    /// [`SessionError::CapacityReached`] → the api layer maps to `409`.
    max_sessions: usize,
    /// Host capabilities (render node passed into the ffmpeg command).
    caps: HwCaps,
    sessions: Mutex<HashMap<String, Session>>,
}

impl SessionManager {
    /// Build a manager rooted at `hls_root` (usually `AppConfig::config_dir/hls`) with a
    /// concurrency cap. The cap should reflect GPU capacity (`docs/.tasks/20` §Scaling:
    /// UHD 770 ≈ 4–7 4K streams, Arc A380 ≈ 8–12).
    pub fn new(hls_root: PathBuf, max_sessions: usize, caps: HwCaps) -> Self {
        Self {
            inner: Arc::new(Manager {
                hls_root,
                max_sessions: max_sessions.max(1),
                caps,
                sessions: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Start a transcode session for `input` with `target`/`audio`, returning the
    /// session id (the caller builds `/api/hls/<id>/index.m3u8` from it).
    ///
    /// Rejects with [`SessionError::CapacityReached`] once the concurrency cap is hit.
    pub async fn start(
        &self,
        input: &Path,
        target: &TranscodeTarget,
        audio: AudioTarget,
    ) -> Result<String, SessionError> {
        let mut sessions = self.inner.sessions.lock().await;
        if sessions.len() >= self.inner.max_sessions {
            return Err(SessionError::CapacityReached(self.inner.max_sessions));
        }

        let id = new_session_id();
        let dir = self.inner.hls_root.join(&id);
        std::fs::create_dir_all(&dir)?;

        let argv = command::build_argv(
            target,
            audio,
            input,
            &dir,
            self.inner.caps.render_node.as_deref(),
        );
        tracing::info!(session = %id, input = %input.display(), "starting transcode session");
        tracing::debug!(session = %id, argv = ?argv, "ffmpeg argv");

        let child = Command::new(ffmpeg_bin())
            .args(&argv)
            .kill_on_drop(true)
            .spawn()
            .map_err(SessionError::Spawn)?;

        sessions.insert(
            id.clone(),
            Session {
                child,
                dir,
                last_access: Instant::now(),
            },
        );
        Ok(id)
    }

    /// Resolve `file` within `session_id`'s directory, refreshing its idle timer.
    ///
    /// Returns the on-disk path for the api layer to stream. The file may not exist yet
    /// (ffmpeg is still writing the first segment) — that is a normal `404`/retry for
    /// the client, distinct from [`SessionError::NotFound`] for an unknown session.
    pub async fn resolve_file(
        &self,
        session_id: &str,
        file: &str,
    ) -> Result<PathBuf, SessionError> {
        // Reject traversal: only a bare filename is allowed.
        if file.contains('/') || file.contains('\\') || file.contains("..") {
            return Err(SessionError::NotFound);
        }
        let mut sessions = self.inner.sessions.lock().await;
        let session = sessions.get_mut(session_id).ok_or(SessionError::NotFound)?;
        session.last_access = Instant::now();
        Ok(session.dir.join(file))
    }

    /// The playlist path (`index.m3u8`) for a session, if it exists.
    pub async fn playlist_path(&self, session_id: &str) -> Result<PathBuf, SessionError> {
        self.resolve_file(session_id, PLAYLIST_NAME).await
    }

    /// Tear down a session now: kill the process and remove its directory.
    pub async fn stop(&self, session_id: &str) -> Result<(), SessionError> {
        let mut sessions = self.inner.sessions.lock().await;
        if let Some(mut s) = sessions.remove(session_id) {
            teardown(&mut s).await;
            Ok(())
        } else {
            Err(SessionError::NotFound)
        }
    }

    /// Kill and remove every session whose last access is older than [`IDLE_TIMEOUT`],
    /// and reap any whose ffmpeg process has already exited. Returns how many were
    /// removed. Call periodically from a background task (see [`SessionManager::spawn_reaper`]).
    pub async fn reap_idle(&self) -> usize {
        let now = Instant::now();
        let mut sessions = self.inner.sessions.lock().await;
        let stale: Vec<String> = sessions
            .iter_mut()
            .filter_map(|(id, s)| {
                let idle = now.duration_since(s.last_access) >= IDLE_TIMEOUT;
                // `try_wait` returns Ok(Some(_)) once the process has exited.
                let exited = matches!(s.child.try_wait(), Ok(Some(_)));
                (idle || exited).then(|| id.clone())
            })
            .collect();
        for id in &stale {
            if let Some(mut s) = sessions.remove(id) {
                teardown(&mut s).await;
            }
        }
        stale.len()
    }

    /// Spawn a background task that reaps idle sessions on an interval, for the process
    /// lifetime. Call once at boot after building the manager.
    pub fn spawn_reaper(&self) {
        let mgr = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(15));
            loop {
                tick.tick().await;
                let n = mgr.reap_idle().await;
                if n > 0 {
                    tracing::info!(reaped = n, "reaped idle transcode sessions");
                }
            }
        });
    }

    /// Current number of live sessions (for metrics / tests).
    pub async fn active_count(&self) -> usize {
        self.inner.sessions.lock().await.len()
    }
}

/// Kill a session's process and remove its output directory. Best-effort — logs but
/// does not fail on a cleanup error, since the session is already being dropped.
async fn teardown(s: &mut Session) {
    if let Err(err) = s.child.start_kill() {
        tracing::debug!(error = %err, "transcoder already exited");
    }
    // Reap the process so it doesn't linger as a zombie.
    let _ = s.child.wait().await;
    if let Err(err) = std::fs::remove_dir_all(&s.dir) {
        tracing::warn!(dir = %s.dir.display(), error = %err, "failed to clean session dir");
    }
}

/// A random, URL-safe session id. Not security-sensitive (LAN appliance, no auth); just
/// needs to be unique and path-safe. Derived from a couple of entropy sources without a
/// new crate dependency.
fn new_session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // Mix in the address of a stack local for a little extra spread across concurrent
    // starts within the same nanosecond tick.
    let salt = &nanos as *const _ as usize as u128;
    let mixed = nanos ^ (salt.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    format!("{mixed:032x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::Vendor;
    use medi_core::VideoCodec;

    fn target() -> TranscodeTarget {
        TranscodeTarget {
            vendor: Some(Vendor::Intel),
            software_decode: false,
            tone_map: false,
            dv_tone_map: false,
            video_codec: VideoCodec::H264,
            audio_transcode_to: None,
        }
    }

    fn manager(dir: &Path, cap: usize) -> SessionManager {
        // Point ffmpeg at a shell no-op so `start` succeeds without a real ffmpeg:
        // FFMPEG_BIN is read by `ffmpeg_bin()`.
        SessionManager::new(dir.to_path_buf(), cap, HwCaps::software_only())
    }

    #[test]
    fn session_ids_are_unique_and_path_safe() {
        let a = new_session_id();
        let b = new_session_id();
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn resolve_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = manager(dir.path(), 2);
        // Even for an unknown session, traversal is rejected as NotFound.
        assert!(matches!(
            mgr.resolve_file("nope", "../secret").await,
            Err(SessionError::NotFound)
        ));
    }

    #[tokio::test]
    async fn capacity_cap_is_enforced() {
        // Use `true` (the shell builtin, always present on the CI image) as a fake
        // ffmpeg that exits 0 immediately; the session still registers before exit.
        std::env::set_var("FFMPEG_BIN", "true");
        let dir = tempfile::tempdir().unwrap();
        let mgr = manager(dir.path(), 1);

        let input = dir.path().join("in.mkv");
        std::fs::write(&input, b"x").unwrap();

        let first = mgr.start(&input, &target(), AudioTarget::Copy).await;
        assert!(first.is_ok(), "first session should start: {first:?}");

        // The cap is 1; the second start is rejected until the first is reaped.
        let second = mgr.start(&input, &target(), AudioTarget::Copy).await;
        assert!(matches!(second, Err(SessionError::CapacityReached(1))));

        // Reaping the (already-exited `true`) process frees the slot.
        let reaped = mgr.reap_idle().await;
        assert!(reaped >= 1);
        assert_eq!(mgr.active_count().await, 0);

        std::env::remove_var("FFMPEG_BIN");
    }
}
