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

/// One transcode session for a title — a seekable VOD stream. The manager serves a complete
/// synthesized playlist (`command::build_vod_playlist`) so the client sees the full runtime;
/// the actual `.m4s` segments are produced on demand by an ffmpeg that can be (re)started at
/// any segment offset when the player seeks (`command::build_argv`'s `start_segment`).
struct Session {
    /// The running ffmpeg child (killed on teardown / on a seek-restart).
    child: Child,
    /// Output directory `/config/hls/<id>`.
    dir: PathBuf,
    /// Last time a file of this session was requested — drives idle teardown.
    last_access: Instant,
    /// When the session was created — used to reap a session that was started but never
    /// consumed (the client got an error, or double-requested) before it wastes GPU/a slot.
    created: Instant,
    /// Whether any file of this session has been fetched yet. A session that has served at
    /// least one file is a real playback; one that never has is a candidate for early reaping.
    consumed: bool,
    /// Fingerprint of `(input, target, audio)` so an identical request reuses this session
    /// instead of spawning a second ffmpeg for the same output.
    key: String,
    // --- everything needed to (re)spawn ffmpeg at a seek target ---------------
    input: PathBuf,
    target: TranscodeTarget,
    audio: AudioTarget,
    /// The segment index the current ffmpeg process started producing from.
    producing_from: u32,
    /// Total media duration (drives the synthesized playlist + the last segment index).
    duration_ms: u64,
}

/// How far ahead of the current production point a requested segment may be while we simply
/// wait for the running ffmpeg to reach it, rather than restarting ffmpeg at that point. A
/// small forward jump (normal playback drift, a short skip) is cheaper to wait out; a larger
/// jump is a real seek that should restart the transcode at the target.
const SEEK_LOOKAHEAD_SEGMENTS: u32 = 3;

/// How long an unconsumed session (started, but no file ever fetched) may live before the
/// reaper drops it. Shorter than [`IDLE_TIMEOUT`] so a burst of failed/duplicate requests
/// (React StrictMode double-mount, rapid reloads) can't exhaust the capacity cap.
pub const UNCONSUMED_TIMEOUT: Duration = Duration::from_secs(20);

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

    /// Start (or reuse) a seekable VOD transcode session for `input` with `target`/`audio`,
    /// returning the session id (the caller builds `/api/hls/<id>/index.m3u8` from it).
    /// `duration_ms` is the media length, used to synthesize the full seekable playlist.
    ///
    /// Rejects with [`SessionError::CapacityReached`] once the concurrency cap is hit.
    pub async fn start(
        &self,
        input: &Path,
        target: &TranscodeTarget,
        audio: AudioTarget,
        duration_ms: u64,
    ) -> Result<String, SessionError> {
        let mut sessions = self.inner.sessions.lock().await;

        // Reuse an identical, still-running session instead of spawning a second ffmpeg for
        // the same output. This is what stops a burst of duplicate requests — a React
        // StrictMode double-mount, a reload, a retry — from consuming multiple capacity slots
        // (and multiple GPU pipelines) for one title. A session whose ffmpeg has already
        // exited is not reused (a failed encode should re-run, not hand back a dead session).
        let key = session_key(input, target, audio);
        let reusable = sessions
            .iter_mut()
            .find_map(|(id, s)| {
                let alive = !matches!(s.child.try_wait(), Ok(Some(_)));
                (s.key == key && alive).then(|| {
                    s.last_access = Instant::now();
                    id.clone()
                })
            });
        if let Some(id) = reusable {
            tracing::info!(session = %id, input = %input.display(), "reusing transcode session");
            return Ok(id);
        }

        if sessions.len() >= self.inner.max_sessions {
            return Err(SessionError::CapacityReached(self.inner.max_sessions));
        }

        let id = new_session_id();
        let dir = self.inner.hls_root.join(&id);
        std::fs::create_dir_all(&dir)?;

        // Start the transcode from segment 0. A seek later restarts ffmpeg at the target
        // segment via `ensure_segment` (no re-decode of the whole file up to the seek point).
        let child = self.spawn_ffmpeg(input, target, audio, &dir, 0)?;
        tracing::info!(session = %id, input = %input.display(), "starting transcode session");

        let now = Instant::now();
        sessions.insert(
            id.clone(),
            Session {
                child,
                dir,
                last_access: now,
                created: now,
                consumed: false,
                key,
                input: input.to_path_buf(),
                target: target.clone(),
                audio,
                producing_from: 0,
                duration_ms,
            },
        );
        Ok(id)
    }

    /// Spawn one ffmpeg producing segments from `start_segment` onward into `dir`.
    fn spawn_ffmpeg(
        &self,
        input: &Path,
        target: &TranscodeTarget,
        audio: AudioTarget,
        dir: &Path,
        start_segment: u32,
    ) -> Result<Child, SessionError> {
        let argv = command::build_argv(
            target,
            audio,
            input,
            dir,
            self.inner.caps.render_node.as_deref(),
            start_segment,
        );
        tracing::debug!(start_segment, argv = ?argv, "ffmpeg argv");
        Command::new(ffmpeg_bin())
            .args(&argv)
            .kill_on_drop(true)
            .spawn()
            .map_err(SessionError::Spawn)
    }

    /// The synthesized VOD playlist for a session (full runtime, all segments, seekable).
    /// This is what `/api/hls/<id>/index.m3u8` serves — NOT ffmpeg's own playlist.
    pub async fn vod_playlist(&self, session_id: &str) -> Result<String, SessionError> {
        let mut sessions = self.inner.sessions.lock().await;
        let s = sessions.get_mut(session_id).ok_or(SessionError::NotFound)?;
        s.last_access = Instant::now();
        s.consumed = true;
        Ok(command::build_vod_playlist(s.duration_ms))
    }

    /// Ensure the segment `seg_index` is being produced, restarting ffmpeg at that point on a
    /// real seek. Returns once the segment file exists on disk, or after a bounded wait (the
    /// caller then serves whatever's there — a 404 the client retries).
    ///
    /// Policy: if the segment already exists, done. If it's within [producing_from,
    /// producing_from+lookahead], the running ffmpeg will reach it soon → just wait. Otherwise
    /// it's a seek (backward, or a big jump forward) → kill ffmpeg and restart at `seg_index`.
    pub async fn ensure_segment(
        &self,
        session_id: &str,
        seg_index: u32,
    ) -> Result<PathBuf, SessionError> {
        let path;
        let need_restart;
        {
            let mut sessions = self.inner.sessions.lock().await;
            let s = sessions.get_mut(session_id).ok_or(SessionError::NotFound)?;
            s.last_access = Instant::now();
            s.consumed = true;
            path = s.dir.join(format!("seg{seg_index:05}.m4s"));

            if path.exists() {
                return Ok(path);
            }
            // Is the current ffmpeg producing toward this segment (still alive, and the segment
            // is at/after where it started, within a small lookahead)?
            let alive = !matches!(s.child.try_wait(), Ok(Some(_)));
            let in_window = seg_index >= s.producing_from
                && seg_index <= s.producing_from + SEEK_LOOKAHEAD_SEGMENTS;
            need_restart = !(alive && in_window);

            if need_restart {
                // Seek: kill the current ffmpeg and restart it at the requested segment.
                let _ = s.child.start_kill();
                let child = self.spawn_ffmpeg(
                    &s.input,
                    &s.target,
                    s.audio,
                    &s.dir,
                    seg_index,
                )?;
                s.child = child;
                s.producing_from = seg_index;
                tracing::info!(session = %session_id, seg_index, "seek: restarting transcode at segment");
            }
        }
        // Wait (outside the lock so other requests aren't blocked) for the segment to appear.
        wait_for_path(&path, Duration::from_secs(15)).await;
        Ok(path)
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
        session.consumed = true;
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
                // Reap the ffmpeg zombie of an exited process, but do NOT tear the session
                // down just because ffmpeg finished: a completed encode's segments stay on
                // disk and are still seekable (a seek restarts ffmpeg). Only idleness (no
                // access) or an unconsumed start drops a session.
                let _ = s.child.try_wait();
                let idle = now.duration_since(s.last_access) >= IDLE_TIMEOUT;
                // A session started but never consumed (client errored / duplicated the
                // request) is dropped after a short grace so it can't hold a capacity slot.
                let abandoned =
                    !s.consumed && now.duration_since(s.created) >= UNCONSUMED_TIMEOUT;
                (idle || abandoned).then(|| id.clone())
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

/// Wait for `path` to exist, up to `timeout`, polling briefly. Lets a just-started or
/// just-restarted (seek) transcode write the requested segment before we serve it.
async fn wait_for_path(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// Fingerprint a transcode request so identical `(input, target, audio)` calls reuse one
/// session. `TranscodeTarget` and `AudioTarget` are small, `Debug`-printable value types; a
/// debug string is a cheap, allocation-light stable key (no hashing crate needed) — two
/// requests match iff every decision-affecting field matches.
fn session_key(input: &Path, target: &TranscodeTarget, audio: AudioTarget) -> String {
    format!("{}|{:?}|{:?}", input.display(), target, audio)
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
    use medi_core::{AudioCodec, VideoCodec};

    fn target() -> TranscodeTarget {
        TranscodeTarget {
            vendor: Some(Vendor::Intel),
            software_decode: false,
            tone_map: false,
            dv_tone_map: false,
            video_codec: VideoCodec::H264,
            audio_transcode_to: None,
            max_bitrate: None,
            subtitle_burn_in: None,
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
    async fn identical_request_reuses_session() {
        // A second identical start must return the SAME session id (not a new ffmpeg), so a
        // duplicate request (StrictMode double-mount, reload) can't consume two slots. Use
        // `sleep` as a fake ffmpeg that stays alive long enough to be reused.
        std::env::set_var("FFMPEG_BIN", "cat");
        let dir = tempfile::tempdir().unwrap();
        let mgr = manager(dir.path(), 4);
        let input = dir.path().join("in.mkv");
        std::fs::write(&input, b"x").unwrap();

        let a = mgr.start(&input, &target(), AudioTarget::Copy { source: None }, 60_000).await.unwrap();
        let b = mgr.start(&input, &target(), AudioTarget::Copy { source: None }, 60_000).await.unwrap();
        assert_eq!(a, b, "identical request must reuse the session");
        assert_eq!(mgr.active_count().await, 1, "only one ffmpeg for one output");

        // A DIFFERENT audio target is a different output → a new session.
        let c = mgr
            .start(&input, &target(), AudioTarget::Transcode { codec: AudioCodec::Aac, channels: 2, source: None }, 60_000)
            .await
            .unwrap();
        assert_ne!(a, c, "different target must not reuse");
        assert_eq!(mgr.active_count().await, 2);

        std::env::remove_var("FFMPEG_BIN");
    }

    #[tokio::test]
    async fn vod_playlist_reflects_duration() {
        std::env::set_var("FFMPEG_BIN", "cat");
        let dir = tempfile::tempdir().unwrap();
        let mgr = manager(dir.path(), 4);
        let input = dir.path().join("in.mkv");
        std::fs::write(&input, b"x").unwrap();

        // 12s title @ 4s segments → 3 segments, VOD, ENDLIST.
        let id = mgr.start(&input, &target(), AudioTarget::Copy { source: None }, 12_000).await.unwrap();
        let m = mgr.vod_playlist(&id).await.unwrap();
        assert!(m.contains("#EXT-X-PLAYLIST-TYPE:VOD"));
        assert!(m.contains("#EXT-X-ENDLIST"));
        assert_eq!(m.matches(".m4s").count(), 3, "12s / 4s = 3 segments: {m}");

        std::env::remove_var("FFMPEG_BIN");
    }

    #[tokio::test]
    async fn seek_far_ahead_restarts_ffmpeg_at_segment() {
        // Requesting a segment well beyond the lookahead is a seek → the session restarts
        // ffmpeg at that segment (producing_from advances) rather than waiting for the
        // original process to encode the whole way there.
        std::env::set_var("FFMPEG_BIN", "cat");
        let dir = tempfile::tempdir().unwrap();
        let mgr = manager(dir.path(), 4);
        let input = dir.path().join("in.mkv");
        std::fs::write(&input, b"x").unwrap();

        // 600s title (150 segments). Start at 0, then "seek" to segment 100.
        let id = mgr.start(&input, &target(), AudioTarget::Copy { source: None }, 600_000).await.unwrap();
        // ensure_segment waits up to 15s for the file (which the fake ffmpeg never writes);
        // run it with a timeout and just assert the restart bookkeeping happened.
        let _ = tokio::time::timeout(
            Duration::from_millis(300),
            mgr.ensure_segment(&id, 100),
        )
        .await;
        // The session's producing_from should now be 100 (a real seek-restart).
        let from = {
            let sessions = mgr.inner.sessions.lock().await;
            sessions.get(&id).unwrap().producing_from
        };
        assert_eq!(from, 100, "a far-ahead segment request restarts ffmpeg at that segment");

        std::env::remove_var("FFMPEG_BIN");
    }

    #[tokio::test]
    async fn capacity_cap_is_enforced() {
        // A long-lived fake ffmpeg (`sleep`) so the first session stays alive and actually
        // holds the single slot. Two DIFFERENT inputs so session reuse never applies — this
        // test is about the capacity cap, not dedup.
        std::env::set_var("FFMPEG_BIN", "cat");
        let dir = tempfile::tempdir().unwrap();
        let mgr = manager(dir.path(), 1);

        let input = dir.path().join("in.mkv");
        std::fs::write(&input, b"x").unwrap();
        let other = dir.path().join("other.mkv");
        std::fs::write(&other, b"y").unwrap();

        let first = mgr.start(&input, &target(), AudioTarget::Copy { source: None }, 60_000).await;
        assert!(first.is_ok(), "first session should start: {first:?}");

        // The cap is 1; a start for a DIFFERENT output is rejected until the first is reaped.
        let second = mgr.start(&other, &target(), AudioTarget::Copy { source: None }, 60_000).await;
        assert!(matches!(second, Err(SessionError::CapacityReached(1))));

        // Tearing the first session down frees the slot deterministically (no reliance on
        // process-exit timing), after which a new start succeeds.
        assert_eq!(mgr.active_count().await, 1);
        mgr.stop(&first.unwrap()).await.unwrap();
        assert_eq!(mgr.active_count().await, 0);
        let third = mgr.start(&other, &target(), AudioTarget::Copy { source: None }, 60_000).await;
        assert!(third.is_ok(), "slot freed → a new session starts: {third:?}");

        std::env::remove_var("FFMPEG_BIN");
    }
}
