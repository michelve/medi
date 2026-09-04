/**
 * `VideoPlayer` (Task 82) — decision-driven in-browser playback.
 *
 * The server owns the playback path: on mount we call `client.stream(fileId)` and branch
 * on `decision.mode` — never guess a container client-side.
 *  - `direct` → a plain `<video src={client.directUrl(fileId)}>`; the browser does HTTP
 *    Range seeking itself.
 *  - `hls`    → Safari plays `decision.url` natively (`canPlayType('application/vnd.apple.mpegurl')`);
 *    every other browser gets an `hls.js` instance that we `attachMedia` and, crucially,
 *    `destroy()` on unmount. A fatal `hls.js` error surfaces as a retry state.
 *
 * The `<video>` is otherwise chrome-less: transport lives in `PlayerControls`, wired
 * through the shared `usePlayerControls` reducer. This component exposes the element and
 * the resolved decision via callbacks so the page can attach controls.
 */

import { useEffect, useMemo, useRef, useState } from 'react';
import { ApiError, type StreamDecision } from '@medi/api-client';
import { useApi } from '../api';
import { theme } from '../theme';
import { Loading } from './Status';
import { PlayerEventLog } from './PlayerEventLog';
import {
  PlayerDiagnostics,
  mediaErrorName,
  networkStateName,
  readyStateName,
} from '../lib/playerDiagnostics';

/** A WebVTT text subtitle rendered as a `<track>` child (`docs/.tasks/90`). */
export interface WebTextTrack {
  /** `GET /api/subtitles/:file_id/:index.vtt`. */
  src: string;
  /** BCP-47 / ISO-639 language tag for the `srclang` attribute. */
  srclang: string;
  /** Human label shown in the browser's caption menu. */
  label: string;
  /** Show this track by default (a forced/default subtitle). */
  default?: boolean;
}

export interface VideoPlayerProps {
  fileId: number;
  /** Receives the `<video>` element once mounted, so the page can bind transport controls. */
  onVideoRef?: (el: HTMLVideoElement | null) => void;
  /** Receives the server's resolved decision (for debugging / mode display). */
  onDecision?: (decision: StreamDecision) => void;
  /**
   * Fill the parent container (`docs/.tasks/97` Part A): drop the boxed `16/9` aspect frame
   * and stretch the `<video>` to `100% × 100%` with `object-fit: contain`, for the
   * full-viewport player. Default `false` keeps the rounded, aspect-boxed frame used elsewhere.
   */
  fill?: boolean;
  /**
   * Selected source audio track (`docs/.tasks/97` Part C): the ffprobe `stream_index` to
   * transcode. Changing it re-resolves the stream, tears down the current hls.js instance,
   * attaches the new track's playlist, and re-seeks to the position captured at the switch —
   * so the audio changes without losing the user's place. `undefined` = the server default.
   */
  audioTrack?: number;
  /**
   * Pair `audioTrack` with a forced transcode when the base decision was `direct` — a browser
   * `<video>` can't switch an embedded audio track, so a non-default selection must go through
   * the server transcode path (`docs/.tasks/97` Part C, "direct-play caveat").
   */
  forceTranscodeForAudio?: boolean;
  /** Fires with `true` while an audio-track switch is re-resolving + re-attaching. */
  onSwitchingAudio?: (switching: boolean) => void;
  /**
   * Resume position in ms (`docs/.tasks/98`): seek here once the first source is ready, so a
   * reopened title picks up where it left off. Seeded on mount into the same resume-seek path
   * an audio-track switch uses (the VOD playlist makes any offset immediately seekable).
   * `undefined`/`0` starts from the beginning. Only the initial value is read — later changes
   * are ignored (a running player isn't re-seeked out from under the viewer).
   */
  initialResumeMs?: number;
  /**
   * WebVTT text subtitles to attach as `<track>` children (`docs/.tasks/90`). Text tracks
   * ride alongside a direct-played or transcoded stream with no extra work; image tracks
   * are burned in server-side (via the stream decision) and never appear here.
   */
  textTracks?: WebTextTrack[];
  /**
   * Show the on-screen diagnostics event log below the player (default true). It logs the
   * stream decision, every hls.js event/error, and `<video>` state transitions — both to the
   * page and the browser console — so playback issues are visible without devtools. Set false
   * and use {@link onDiagnostics} to render the log elsewhere (e.g. outside the controls
   * overlay) while still capturing every event.
   */
  diagnostics?: boolean;
  /** Open the diagnostics panel expanded by default. */
  diagnosticsOpen?: boolean;
  /**
   * Receives the player's {@link PlayerDiagnostics} channel so a parent can render its own
   * `PlayerEventLog` (or read the log). Fires once on mount.
   */
  onDiagnostics?: (diag: PlayerDiagnostics) => void;
}

type Phase =
  | { kind: 'loading' }
  | { kind: 'ready'; decision: StreamDecision }
  | { kind: 'error'; message: string; busy: boolean };

/**
 * Kick off playback once a source is attached. Browsers block un-muted autoplay unless the
 * page has interaction; on a rejection we retry once muted (a muted autoplay is always
 * allowed) so the picture appears rather than sitting paused on a black first frame. A second
 * rejection is left alone — the user can press Play. A real decode error surfaces via the
 * element's `error` event, not here.
 */
async function startPlayback(video: HTMLVideoElement): Promise<void> {
  try {
    await video.play();
  } catch {
    try {
      video.muted = true;
      await video.play();
    } catch {
      // Autoplay still blocked — leave paused; the transport controls can start it.
    }
  }
}

/** A human message for a `<video>` MediaError code. */
function mediaErrorMessage(err: MediaError | null): string {
  switch (err?.code) {
    case MediaError.MEDIA_ERR_ABORTED:
      return 'Playback was aborted.';
    case MediaError.MEDIA_ERR_NETWORK:
      return 'A network error interrupted the download.';
    case MediaError.MEDIA_ERR_DECODE:
      return "This file's video or audio codec can't be decoded by your browser.";
    case MediaError.MEDIA_ERR_SRC_NOT_SUPPORTED:
      return 'This file format is not supported by your browser.';
    default:
      return 'The video could not be played.';
  }
}

export function VideoPlayer({
  fileId,
  onVideoRef,
  onDecision,
  fill = false,
  audioTrack,
  forceTranscodeForAudio = false,
  onSwitchingAudio,
  initialResumeMs,
  textTracks,
  diagnostics = true,
  diagnosticsOpen = false,
  onDiagnostics,
}: VideoPlayerProps) {
  const api = useApi();
  const videoRef = useRef<HTMLVideoElement | null>(null);
  // The playback position (ms) to restore once a freshly-attached source is ready — set when
  // an audio-track switch tears the current stream down, so the new track resumes in place.
  const resumeToRef = useRef<number | null>(null);
  // One-shot initial resume (`docs/.tasks/98`): the saved position to seek to on the FIRST
  // attach. Kept separate from `resumeToRef` so it does NOT trip the "Switching audio…" toast
  // (the resolve effect keys that off `resumeToRef`). Consumed once in the attach effect.
  const initialResumeRef = useRef<number | null>(
    initialResumeMs != null && initialResumeMs > 0 ? Math.round(initialResumeMs) : null,
  );
  // Remember the previous audioTrack so a *change* (a real switch) captures the current
  // position, while the first mount does not.
  const prevAudioTrackRef = useRef<number | undefined>(audioTrack);
  // One diagnostics channel per mount — a fresh log for each file/session.
  const diag = useMemo(() => new PlayerDiagnostics(), []);

  // Hand the channel to a parent that wants to render the log itself.
  useEffect(() => {
    onDiagnostics?.(diag);
  }, [diag, onDiagnostics]);
  const [phase, setPhase] = useState<Phase>({ kind: 'loading' });
  // Bumped to force a re-resolve on "retry".
  const [attempt, setAttempt] = useState(0);
  // Once a `direct` stream proves unplayable in this browser, we re-resolve with
  // `forceTranscode` so the server hands back an HLS stream instead. `forceTranscode` state
  // drives the re-resolve; the ref guards against looping (we only auto-fall-back once).
  const [forceTranscode, setForceTranscode] = useState(false);
  const didFallbackRef = useRef(false);

  // Reset the one-shot fallback whenever we switch files.
  useEffect(() => {
    didFallbackRef.current = false;
    setForceTranscode(false);
  }, [fileId]);

  // On an audio-track *change* (not the first mount), capture the current playback position
  // so the resolve effect below can re-seek there after the new track's source attaches.
  if (prevAudioTrackRef.current !== audioTrack) {
    prevAudioTrackRef.current = audioTrack;
    const t = videoRef.current?.currentTime;
    if (typeof t === 'number' && Number.isFinite(t) && t > 0) {
      resumeToRef.current = Math.round(t * 1000);
    }
  }

  // 1) Resolve the stream decision (direct vs HLS) from the server. Re-runs on an
  // `audioTrack` switch too (the new source is a distinct transcode session).
  useEffect(() => {
    const controller = new AbortController();
    const switching = resumeToRef.current !== null;
    setPhase({ kind: 'loading' });
    if (switching) onSwitchingAudio?.(true);
    // A non-default audio selection can't be switched by a browser `<video>` on a direct
    // stream — force the transcode path so the server maps the chosen track.
    const force = forceTranscode || (audioTrack != null && forceTranscodeForAudio);
    diag.info('decision', 'requesting /api/stream', { fileId, platform: 'web', forceTranscode: force, audioTrack, attempt });
    api
      // `platform: 'web'` selects the browser capability profile so the server transcodes
      // codecs a browser can't decode (HEVC / AC-3 / DTS) instead of handing back a direct
      // stream that would black-screen. `forceTranscode` is set by the fallback below after a
      // `direct` stream failed to play — it demands an HLS transcode hls.js can always play.
      .stream(fileId, { platform: 'web', forceTranscode: force, audioTrack }, { signal: controller.signal })
      .then((decision) => {
        if (controller.signal.aborted) return;
        diag.info('decision', `resolved: ${decision.mode}`, decision);
        onDecision?.(decision);
        setPhase({ kind: 'ready', decision });
      })
      .catch((err: unknown) => {
        if (controller.signal.aborted) return;
        const busy = err instanceof ApiError && err.isBusy;
        const message = err instanceof ApiError ? err.message : String(err);
        diag.error('decision', 'stream request failed', { message, busy, status: err instanceof ApiError ? err.status : undefined });
        setPhase({ kind: 'error', message, busy });
        if (switching) onSwitchingAudio?.(false);
      });
    return () => controller.abort();
  }, [api, fileId, attempt, forceTranscode, audioTrack, forceTranscodeForAudio, onDecision, onSwitchingAudio, diag]);

  // 2) Attach the resolved source to the <video> (direct src / native HLS / hls.js).
  useEffect(() => {
    if (phase.kind !== 'ready') return;
    const video = videoRef.current;
    if (!video) return;
    const { decision } = phase;

    // Instrument the raw <video> element's own lifecycle for every attach path. These fire
    // regardless of direct/HLS and are the ground truth for "why is it a black screen".
    const detachVideoEvents = attachVideoDiagnostics(video, diag);

    // On the very first attach with no pending audio-switch resume, seed the saved resume
    // position (`docs/.tasks/98`) so the title picks up where it left off. Consumed once, and
    // only when nothing else is already queued (an audio switch mid-mount takes precedence).
    if (resumeToRef.current === null && initialResumeRef.current !== null) {
      resumeToRef.current = initialResumeRef.current;
      initialResumeRef.current = null;
      diag.info('player', 'resuming from saved position', { ms: resumeToRef.current });
    }

    // Resume seek (`docs/.tasks/97` Part C audio switch + `98` initial resume): if a position is
    // queued, seek there once the new source is ready. The VOD playlist makes any offset
    // immediately seekable. One-shot: it fires on the first `loadedmetadata`, then clears.
    const detachResume = attachResumeSeek(video, resumeToRef, diag, () => onSwitchingAudio?.(false));

    if (decision.mode === 'direct') {
      const src = api.directUrl(fileId);
      diag.info('video', 'attach direct src', { src });
      video.src = src;
      void startPlayback(video);
      return () => {
        detachVideoEvents();
        detachResume();
        video.removeAttribute('src');
        video.load();
      };
    }

    // HLS. IMPORTANT: prefer hls.js (MSE) wherever it is supported, and only fall back to the
    // browser's *native* HLS when hls.js is NOT supported (real Safari / iOS).
    //
    // Chromium (Chrome/Edge) reports `canPlayType('application/vnd.apple.mpegurl') === 'maybe'`
    // but its "native HLS" is a lie: it hands the .m3u8 straight to the <video> element, which
    // tries to demux the PLAYLIST TEXT as a media file and fails with
    // `MEDIA_ERR_SRC_NOT_SUPPORTED / DEMUXER_ERROR_COULD_NOT_PARSE`. So `canPlayType` must NOT
    // gate the native path — `Hls.isSupported()` (MSE present) does. hls.js is the default on
    // every MSE browser; native HLS is the fallback only for Safari, which lacks MSE for fMP4.
    const hlsUrl = api.hlsUrl(decision);

    // hls.js is loaded on demand (dynamic import) so it stays OUT of the browse bundle —
    // only the player route pulls it. `cancelled`/`instance` bridge the async gap so the
    // effect cleanup can still destroy whatever got created.
    let cancelled = false;
    let instance: import('hls.js').default | null = null;
    let nativeSrcSet = false;
    diag.info('hls', 'loading hls.js', { url: hlsUrl });
    void import('hls.js').then(({ default: Hls }) => {
      if (cancelled) return;
      if (!Hls.isSupported()) {
        // No MSE — this is Safari/iOS. Use the browser's genuine native HLS support.
        const native = video.canPlayType('application/vnd.apple.mpegurl');
        if (native) {
          diag.info('hls', 'no MSE → native HLS (Safari)', { canPlayType: native, url: hlsUrl });
          video.src = hlsUrl;
          nativeSrcSet = true;
          void startPlayback(video);
          return;
        }
        diag.error('hls', 'no MSE and no native HLS — cannot play');
        setPhase({
          kind: 'error',
          message: 'This browser cannot play the transcoded (HLS) stream.',
          busy: false,
        });
        return;
      }
      diag.info('hls', `hls.js ${Hls.version} ready`, { workers: true });
      // The stream is transcoded ON THE FLY: when the decision starts a fresh session, the
      // first segment (and thus index.m3u8) takes several seconds to appear — a 4K HDR→SDR
      // first segment measured ~6s. hls.js's default manifest-load policy gives up after a
      // couple of quick retries, so it 404s before ffmpeg has written anything. Widen the
      // retry budget generously so it polls the playlist until the transcoder produces it.
      const hls = new Hls({
        enableWorker: true,
        // v1.x load policies: retry the manifest for ~20s (many attempts, capped backoff) so
        // an on-the-fly transcode's warm-up 404s are transient, not fatal.
        manifestLoadPolicy: {
          default: {
            maxTimeToFirstByteMs: 20_000,
            maxLoadTimeMs: 20_000,
            timeoutRetry: { maxNumRetry: 4, retryDelayMs: 1000, maxRetryDelayMs: 2000 },
            errorRetry: { maxNumRetry: 20, retryDelayMs: 1000, maxRetryDelayMs: 2000 },
          },
        },
        // Segments can also lag slightly behind the playlist edge; give them room too.
        playlistLoadPolicy: {
          default: {
            maxTimeToFirstByteMs: 20_000,
            maxLoadTimeMs: 20_000,
            timeoutRetry: { maxNumRetry: 4, retryDelayMs: 1000, maxRetryDelayMs: 2000 },
            errorRetry: { maxNumRetry: 20, retryDelayMs: 1000, maxRetryDelayMs: 2000 },
          },
        },
      });
      instance = hls;
      attachHlsDiagnostics(hls, Hls, diag);
      hls.loadSource(hlsUrl);
      hls.attachMedia(video);
      hls.on(Hls.Events.MEDIA_ATTACHED, () => {
        diag.info('hls', 'MEDIA_ATTACHED → play()');
        void startPlayback(video);
      });
      hls.on(Hls.Events.ERROR, (_evt, data) => {
        // Only fatal errors abort playback; hls.js recovers from the rest itself.
        if (data.fatal) {
          // A fatal HLS error on a *forced* transcode usually means the server couldn't
          // produce the stream (e.g. ffmpeg unavailable) rather than a browser limitation —
          // say so instead of blaming the browser.
          const message = didFallbackRef.current
            ? `Server transcode is unavailable (${data.details ?? data.type}). The file could not be converted for playback.`
            : `Stream error (${data.type}). ${data.details ?? ''}`.trim();
          setPhase({ kind: 'error', message, busy: false });
        }
      });
    });
    // Destroy the instance on unmount / re-resolve — the task's explicit requirement.
    return () => {
      cancelled = true;
      detachVideoEvents();
      detachResume();
      instance?.destroy();
      // Native-HLS (Safari) path: clear the src we set so a re-resolve starts clean.
      if (nativeSrcSet) {
        video.removeAttribute('src');
        video.load();
      }
    };
  }, [api, fileId, phase, diag, onSwitchingAudio]);

  const setRef = (el: HTMLVideoElement | null) => {
    videoRef.current = el;
    onVideoRef?.(el);
  };

  return (
    <>
    <div
      style={{
        position: 'relative',
        width: '100%',
        // Full-viewport player fills its (fixed) parent; the boxed use keeps the 16/9 frame.
        ...(fill
          ? { height: '100%' }
          : { aspectRatio: '16 / 9', borderRadius: 8 }),
        background: '#000',
        overflow: 'hidden',
      }}
    >
      <video
        ref={setRef}
        style={{
          width: '100%',
          height: '100%',
          display: 'block',
          background: '#000',
          // Letterbox the frame inside the viewport rather than cropping it.
          objectFit: fill ? 'contain' : undefined,
        }}
        crossOrigin="anonymous"
        playsInline
        // A decode / unsupported-source error would otherwise be a silent black screen — surface
        // it in the error phase with a Retry (`docs/.tasks/82`). Only report a real element error
        // (an aborted src swap during cleanup sets no `error`).
        onError={(e) => {
          const err = e.currentTarget.error;
          if (!err) return;
          diag.error('video', `element error: ${mediaErrorName(err.code)}`, {
            code: err.code,
            message: err.message || undefined,
            mode: phase.kind === 'ready' ? phase.decision.mode : phase.kind,
          });
          // A `direct` stream the browser can't open/decode: instead of dead-ending, re-resolve
          // once forcing a server transcode (H.264+AAC HLS) that hls.js can always play. This
          // self-heals when the server's direct guess was wrong for this browser. A second
          // failure (or a non-direct phase) falls through to the real error.
          const recoverable =
            err.code === MediaError.MEDIA_ERR_SRC_NOT_SUPPORTED ||
            err.code === MediaError.MEDIA_ERR_DECODE;
          if (
            recoverable &&
            phase.kind === 'ready' &&
            phase.decision.mode === 'direct' &&
            !didFallbackRef.current
          ) {
            diag.warn('player', 'direct stream failed → retrying with force_transcode');
            didFallbackRef.current = true;
            setForceTranscode(true);
            setPhase({ kind: 'loading' });
            return;
          }
          setPhase({ kind: 'error', message: mediaErrorMessage(err), busy: false });
        }}
        // Controls come from PlayerControls; keep the native ones off.
      >
        {/* Text subtitles as WebVTT tracks (`docs/.tasks/90`). `kind="subtitles"` needs a
            `srclang`; the browser's own caption menu toggles them. */}
        {textTracks?.map((t) => (
          <track
            key={t.src}
            kind="subtitles"
            src={t.src}
            srcLang={t.srclang}
            label={t.label}
            default={t.default}
          />
        ))}
      </video>
      {phase.kind === 'loading' && (
        <div style={overlayCenter}>
          <Loading label="Preparing playback…" />
        </div>
      )}
      {phase.kind === 'error' && (
        <div style={overlayCenter}>
          <div style={{ textAlign: 'center', maxWidth: 420, padding: 24 }}>
            <p style={{ color: theme.colors.text, fontSize: 15, margin: '0 0 16px' }}>
              {phase.busy
                ? 'All transcode sessions are busy right now. Try again in a moment.'
                : `Playback unavailable: ${phase.message}`}
            </p>
            <button
              type="button"
              onClick={() => {
                // A manual retry starts fresh: let the server re-decide (drop the forced
                // transcode) and re-arm the one-shot direct→HLS fallback.
                didFallbackRef.current = false;
                setForceTranscode(false);
                setAttempt((n) => n + 1);
              }}
              style={{
                padding: '8px 18px',
                borderRadius: 6,
                border: 'none',
                fontSize: 14,
                fontWeight: 600,
                cursor: 'pointer',
                color: '#fff',
                background: theme.colors.accent,
              }}
            >
              Retry
            </button>
          </div>
        </div>
      )}
    </div>
    {diagnostics && <PlayerEventLog diagnostics={diag} defaultOpen={diagnosticsOpen} />}
    </>
  );
}

const overlayCenter: React.CSSProperties = {
  position: 'absolute',
  inset: 0,
  display: 'flex',
  alignItems: 'center',
  justifyContent: 'center',
  background: 'rgba(0,0,0,0.6)',
};

/**
 * Subscribe to the raw `<video>` element's own media events and log each to diagnostics.
 * Returns a detach function. These are the ground-truth signals for whether the browser is
 * loading, stalling, buffering, or erroring — independent of hls.js.
 */
function attachVideoDiagnostics(
  video: HTMLVideoElement,
  diag: PlayerDiagnostics,
): () => void {
  const state = () => ({
    readyState: readyStateName(video.readyState),
    networkState: networkStateName(video.networkState),
    currentTime: Number(video.currentTime.toFixed(2)),
    paused: video.paused,
  });
  // A curated set: enough to see the load→play→stall lifecycle without flooding on timeupdate.
  const handlers: Record<string, (ev?: Event) => void> = {
    loadstart: () => diag.info('video', 'loadstart', state()),
    loadedmetadata: () =>
      diag.info('video', 'loadedmetadata', {
        ...state(),
        duration: Number.isFinite(video.duration) ? Number(video.duration.toFixed(1)) : video.duration,
        videoWidth: video.videoWidth,
        videoHeight: video.videoHeight,
      }),
    loadeddata: () => diag.info('video', 'loadeddata', state()),
    canplay: () => diag.info('video', 'canplay', state()),
    playing: () => diag.info('video', 'playing', state()),
    waiting: () => diag.warn('video', 'waiting (buffering)', state()),
    stalled: () => diag.warn('video', 'stalled (no data)', state()),
    suspend: () => diag.info('video', 'suspend', state()),
    pause: () => diag.info('video', 'pause', state()),
    ended: () => diag.info('video', 'ended', state()),
  };
  for (const [name, fn] of Object.entries(handlers)) video.addEventListener(name, fn);
  return () => {
    for (const [name, fn] of Object.entries(handlers)) video.removeEventListener(name, fn);
  };
}

/**
 * Seek a freshly-attached source back to a captured position once it's ready — the resume
 * half of an audio-track switch (`docs/.tasks/97` Part C). `resumeToRef` holds the target ms
 * (or `null` when there's nothing to resume). On the first `loadedmetadata` it seeks, clears
 * the ref, calls `onDone` (to drop the "switching audio…" state), and detaches itself. Returns
 * a detach function for the effect cleanup.
 */
function attachResumeSeek(
  video: HTMLVideoElement,
  resumeToRef: React.MutableRefObject<number | null>,
  diag: PlayerDiagnostics,
  onDone: () => void,
): () => void {
  const target = resumeToRef.current;
  if (target === null) return () => {};
  let done = false;
  const seek = () => {
    if (done) return;
    done = true;
    resumeToRef.current = null;
    video.currentTime = target / 1000;
    diag.info('player', 'audio switch: resumed position', { ms: target });
    void video.play().catch(() => undefined);
    onDone();
    video.removeEventListener('loadedmetadata', seek);
  };
  video.addEventListener('loadedmetadata', seek);
  return () => {
    video.removeEventListener('loadedmetadata', seek);
    // If we tore down before the seek landed (a rapid re-switch), keep the target so the next
    // attach still resumes — do NOT clear it. Drop the switching flag only once it lands.
  };
}

/**
 * Wire hls.js lifecycle + error events into diagnostics. `Hls` is the imported module (its
 * `Events`/`ErrorTypes` enums); `hls` is the instance. Logs manifest parsing, level switches,
 * fragment loads, and — crucially — every error (fatal AND non-fatal, since a run of
 * recovered errors is exactly what precedes a stall/black screen).
 */
function attachHlsDiagnostics(
  hls: import('hls.js').default,
  Hls: typeof import('hls.js').default,
  diag: PlayerDiagnostics,
): void {
  const E = Hls.Events;
  hls.on(E.MANIFEST_LOADING, () => diag.info('hls', 'MANIFEST_LOADING'));
  hls.on(E.MANIFEST_PARSED, (_e, d) =>
    diag.info('hls', 'MANIFEST_PARSED', { levels: d.levels?.length, firstLevel: d.firstLevel }),
  );
  hls.on(E.LEVEL_LOADED, (_e, d) =>
    diag.info('hls', 'LEVEL_LOADED', {
      live: d.details?.live,
      totalduration: d.details?.totalduration,
      fragments: d.details?.fragments?.length,
    }),
  );
  hls.on(E.LEVEL_SWITCHED, (_e, d) => diag.info('hls', 'LEVEL_SWITCHED', { level: d.level }));
  hls.on(E.FRAG_LOADED, (_e, d) =>
    diag.info('hls', `FRAG_LOADED #${d.frag?.sn}`, {
      sn: d.frag?.sn,
      duration: d.frag?.duration,
    }),
  );
  hls.on(E.ERROR, (_e, d) => {
    const detail = {
      type: d.type,
      details: d.details,
      fatal: d.fatal,
      // Network errors carry the HTTP response; expose it — a 404/5xx here is the smoking gun.
      httpStatus: (d.response as { code?: number } | undefined)?.code,
      url: (d as { frag?: { url?: string }; url?: string }).frag?.url ?? (d as { url?: string }).url,
      reason: (d as { reason?: string }).reason,
    };
    if (d.fatal) diag.error('hls', `FATAL ${d.type}: ${d.details}`, detail);
    else diag.warn('hls', `recovered ${d.type}: ${d.details}`, detail);
  });
}
