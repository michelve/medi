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

import { memo, useEffect, useMemo, useRef, useState } from 'react';
import { ApiError, type FileSubtitleTrack, type StreamDecision } from '@medi/api-client';
import { useApi } from '../api';
import { theme } from '../theme';
import { Loading } from './Status';
import { PlayerEventLog } from './PlayerEventLog';
import { createSubtitleRenderer, type SubtitleRenderHandle } from '../lib/subtitles/renderer';
import { cueCss } from '../lib/subtitleAppearance';
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

/** The selected caption track (`docs/.tasks/99`): the full track row plus its addressing id
 * (`ext<id>` / `stream_index` string) and, for a plain-text track, its `<track>` src. */
export interface ActiveSubtitle {
  track: FileSubtitleTrack;
  /** `ext<id>` or the `stream_index` as a string — for `/raw` + `/api/subtitles` URLs. */
  id: string;
  /** The WebVTT `<track>` src for a plain-text track; `null` for ASS/image (client-rendered). */
  textTrackSrc: string | null;
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
   * The caption track to display (`docs/.tasks/99` A2). Since native `<video>` controls are off
   * (the browser's caption menu is hidden), the player renders the selection itself:
   *  - **plain text** (srt/vtt) → the matching `<track>`'s `mode` is set to `showing`
   *  - **ASS/SSA** → libass-wasm client renderer (native-res styling, zero transcode)
   *  - **image (PGS/VobSub)** → falls back to server burn-in via {@link onSubtitleBurnIn}
   * `null`/`undefined` shows none.
   */
  activeSubtitle?: ActiveSubtitle | null;
  /** Subtitle sync offset in seconds (`docs/.tasks/99` C5); applied to whichever renderer. */
  subtitleOffsetSeconds?: number;
  /** Video frame rate for libass `targetFps` (`docs/.tasks/99`); falls back to 24 when absent. */
  videoFps?: number;
  /** Viewer subtitle appearance (`docs/.tasks/99` C4) applied to the native `<track>` via a
   * scoped `::cue` stylesheet. Omit to use the browser defaults. */
  subtitleAppearance?: import('../lib/subtitleAppearance').SubtitleAppearance;
  /**
   * Called when the selected subtitle can only be shown by a server burn-in (an image track,
   * or a libass failure). The parent sets {@link burnInSubtitle} to the track's `stream_index`
   * in response, which re-resolves the stream with `sub`/`sub_burn`.
   */
  onSubtitleBurnIn?: (track: FileSubtitleTrack) => void;
  /**
   * Burn an image subtitle into the video (`docs/.tasks/99` C1 fallback): the embedded
   * `stream_index` to burn, or `null`/`undefined` for none. Setting it re-resolves the stream
   * with `sub`/`sub_burn` (a distinct transcode session), captures position, and re-seeks —
   * the same switch mechanic as an audio-track change.
   */
  burnInSubtitle?: number | null;
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

function VideoPlayerInner({
  fileId,
  onVideoRef,
  onDecision,
  fill = false,
  audioTrack,
  forceTranscodeForAudio = false,
  onSwitchingAudio,
  initialResumeMs,
  textTracks,
  activeSubtitle,
  subtitleOffsetSeconds = 0,
  videoFps,
  subtitleAppearance,
  onSubtitleBurnIn,
  burnInSubtitle,
  diagnostics = true,
  diagnosticsOpen = false,
  onDiagnostics,
}: VideoPlayerProps) {
  const api = useApi();
  const videoRef = useRef<HTMLVideoElement | null>(null);
  // The live client-side subtitle renderer (libass/libbitsub), so the sync-offset effect can
  // reach it without rebuilding (`docs/.tasks/99` C1/C5).
  const rendererRef = useRef<SubtitleRenderHandle | null>(null);
  // The subtitle offset (seconds) currently applied to native `<track>` cues, so a nudge shifts
  // by the delta rather than re-shifting from the original times (`docs/.tasks/99` C5).
  const appliedOffsetRef = useRef(0);
  // Latest known video FPS (read at renderer creation without forcing an effect re-run).
  const videoFpsRef = useRef(videoFps);
  videoFpsRef.current = videoFps;
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
  // A stable per-instance class so the `::cue` appearance stylesheet scopes to THIS video.
  const cueScopeClass = useMemo(
    () => `medi-cue-${Math.random().toString(36).slice(2, 10)}`,
    [],
  );

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

  // Keep the latest callback / hint in refs so the decision effect can read them WITHOUT
  // listing them in its dependency array. `onDecision` is an inline arrow recreated on every
  // parent render, and `forceTranscodeForAudio` is recomputed from the parent's `baseMode`
  // — and both change *as a result of* a resolved decision (the parent's `setBaseMode`). If
  // they were deps, resolving a decision would re-run the effect and fire ANOTHER
  // `/api/stream`, each one spinning up a fresh transcode session until the server's
  // capacity cap trips (HTTP 409). Refs break that self-retriggering loop.
  const onDecisionRef = useRef(onDecision);
  onDecisionRef.current = onDecision;
  const forceTranscodeForAudioRef = useRef(forceTranscodeForAudio);
  forceTranscodeForAudioRef.current = forceTranscodeForAudio;
  // The key of the decision request currently in flight or already resolved. Guards against a
  // duplicate fetch for an identical request — chiefly React StrictMode's dev double-mount,
  // which otherwise allocates two server sessions before the first abort lands.
  const decisionKeyRef = useRef<string | null>(null);

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

  // Same position-capture for a burn-in subtitle *change* (`docs/.tasks/99` C1 fallback): a
  // burn-in is a distinct transcode session, so we re-seek to the current spot after it attaches.
  const prevBurnInRef = useRef<number | null | undefined>(burnInSubtitle);
  if (prevBurnInRef.current !== burnInSubtitle) {
    prevBurnInRef.current = burnInSubtitle;
    const t = videoRef.current?.currentTime;
    if (typeof t === 'number' && Number.isFinite(t) && t > 0) {
      resumeToRef.current = Math.round(t * 1000);
    }
  }

  // 1) Resolve the stream decision (direct vs HLS) from the server. Re-runs on an
  // `audioTrack` switch too (the new source is a distinct transcode session).
  useEffect(() => {
    // A burn-in subtitle (`docs/.tasks/99`) is an image sub the server renders into the video —
    // it inherently requires a transcode. A non-default audio selection also can't be switched
    // by a browser `<video>` on a direct stream, so both force the transcode path.
    const burnIn = burnInSubtitle ?? null;
    const force =
      forceTranscode || burnIn != null || (audioTrack != null && forceTranscodeForAudioRef.current);
    // Identity of this exact request. If it matches the one already in flight / resolved,
    // don't fire again — this is what absorbs StrictMode's double-mount and any residual
    // re-render, so one playback allocates exactly one transcode session.
    const key = `${fileId}|${audioTrack ?? ''}|${force}|${burnIn ?? ''}|${attempt}`;
    if (decisionKeyRef.current === key) return;
    decisionKeyRef.current = key;

    const controller = new AbortController();
    // Once the request resolves it "owns" the key for good; until then, an abort (StrictMode's
    // unmount→remount, a fast fileId change) must release the key so the remount can re-issue
    // it — otherwise the guard would skip the re-fetch and leave the player stuck loading.
    let settled = false;
    const switching = resumeToRef.current !== null;
    setPhase({ kind: 'loading' });
    if (switching) onSwitchingAudio?.(true);
    diag.info('decision', 'requesting /api/stream', { fileId, platform: 'web', forceTranscode: force, audioTrack, burnIn, attempt });
    api
      // `platform: 'web'` selects the browser capability profile so the server transcodes
      // codecs a browser can't decode (HEVC / AC-3 / DTS) instead of handing back a direct
      // stream that would black-screen. `forceTranscode` is set by the fallback below after a
      // `direct` stream failed to play — it demands an HLS transcode hls.js can always play.
      // `sub`/`subBurn` (when set) ask the server to burn an image subtitle into the video.
      .stream(
        fileId,
        {
          platform: 'web',
          forceTranscode: force,
          audioTrack,
          ...(burnIn != null ? { sub: burnIn, subBurn: true } : {}),
        },
        { signal: controller.signal },
      )
      .then((decision) => {
        if (controller.signal.aborted) return;
        settled = true;
        diag.info('decision', `resolved: ${decision.mode}`, decision);
        onDecisionRef.current?.(decision);
        setPhase({ kind: 'ready', decision });
      })
      .catch((err: unknown) => {
        if (controller.signal.aborted) return;
        settled = true;
        // A failed request must clear the in-flight key so the identical request can be
        // retried (the Retry button also bumps `attempt`, but clearing here is belt-and-braces).
        if (decisionKeyRef.current === key) decisionKeyRef.current = null;
        const busy = err instanceof ApiError && err.isBusy;
        const message = err instanceof ApiError ? err.message : String(err);
        diag.error('decision', 'stream request failed', { message, busy, status: err instanceof ApiError ? err.status : undefined });
        setPhase({ kind: 'error', message, busy });
        if (switching) onSwitchingAudio?.(false);
      });
    return () => {
      controller.abort();
      // Aborted before it resolved → release the key so a remount re-issues the request.
      if (!settled && decisionKeyRef.current === key) decisionKeyRef.current = null;
    };
  }, [api, fileId, attempt, forceTranscode, audioTrack, burnInSubtitle, onSwitchingAudio, diag]);

  // The resolved decision, or null while loading/errored. Derived so the attach effect below
  // can key on the decision's *identity* (its URL/mode) rather than the `phase` object
  // reference — a re-resolve that yields the same stream must NOT tear down a working hls.js.
  const decision = phase.kind === 'ready' ? phase.decision : null;
  // A stable identity for the resolved stream: same mode+URL ⇒ same string ⇒ effect does not
  // re-run ⇒ the existing hls.js instance keeps playing. For `direct`, the source is
  // `directUrl(fileId)` (the decision carries no HLS url), so fold `fileId` in.
  const decisionId = decision ? `${decision.mode}|${decision.url}|${fileId}` : null;
  // Read the current decision inside the attach effect without listing `decision` (a new
  // object each resolve) as a dep — the effect keys on `decisionId`, which fully determines it.
  const decisionRef = useRef(decision);
  decisionRef.current = decision;

  // 2) Attach the resolved source to the <video> (direct src / native HLS / hls.js).
  useEffect(() => {
    const decision = decisionRef.current;
    if (!decision) return;
    const video = videoRef.current;
    if (!video) return;

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
        // The init segment (`init.mp4`) and the FIRST media segment load under this policy, and
        // that first fetch is what BLOCKS on the server while ffmpeg transcodes the opening
        // segment on the fly. A cold 4K HDR/DV→SDR start can take well over hls.js's DEFAULT 10s
        // fragment timeout — the server itself waits up to 15s (`ensure_segment`) for the segment
        // to appear. Without widening this, the first fragment aborts at 10s with
        // `fragLoadTimeOut` (reported as httpStatus:undefined — a client-side timeout, NOT a
        // server error) and the player loops forever even though the transcode is producing
        // output. Match the manifest/playlist budget so the first segment is polled, not dropped.
        fragLoadPolicy: {
          default: {
            maxTimeToFirstByteMs: 20_000,
            maxLoadTimeMs: 30_000,
            timeoutRetry: { maxNumRetry: 6, retryDelayMs: 1000, maxRetryDelayMs: 3000 },
            errorRetry: { maxNumRetry: 8, retryDelayMs: 1000, maxRetryDelayMs: 3000 },
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
      // Recover in place instead of tearing down (the jellyfin-web model,
      // `htmlMediaHelper.js` ERROR handler): a fatal network/media error is usually transient
      // — a warm-up 404 while ffmpeg writes the first segment, or an MSE buffer hiccup — and
      // `startLoad()` / `recoverMediaError()` fix it WITHOUT a `destroy()`. A teardown here
      // would clear the attach effect and re-resolve, spawning a brand-new transcode session,
      // which is exactly the storm we're fixing. Only give up after a couple of failed
      // recoveries, or on an error type hls.js can't recover from.
      let recoveries = 0;
      hls.on(Hls.Events.ERROR, (_evt, data) => {
        // Non-fatal errors are handled by hls.js itself.
        if (!data.fatal) return;
        if (recoveries < 3) {
          if (data.type === Hls.ErrorTypes.NETWORK_ERROR) {
            recoveries += 1;
            diag.info('hls', 'fatal network error → startLoad()', { details: data.details, recoveries });
            hls.startLoad();
            return;
          }
          if (data.type === Hls.ErrorTypes.MEDIA_ERROR) {
            recoveries += 1;
            diag.info('hls', 'fatal media error → recoverMediaError()', { details: data.details, recoveries });
            hls.recoverMediaError();
            return;
          }
        }
        // Unrecoverable (or recovery budget exhausted). A fatal HLS error on a *forced*
        // transcode usually means the server couldn't produce the stream (e.g. ffmpeg
        // unavailable) rather than a browser limitation — say so instead of blaming the browser.
        const message = didFallbackRef.current
          ? `Server transcode is unavailable (${data.details ?? data.type}). The file could not be converted for playback.`
          : `Stream error (${data.type}). ${data.details ?? ''}`.trim();
        diag.error('hls', 'fatal, unrecoverable', { type: data.type, details: data.details, recoveries });
        setPhase({ kind: 'error', message, busy: false });
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
  }, [api, fileId, decisionId, diag, onSwitchingAudio]);

  // 3) Apply the caption selection (`docs/.tasks/99` A2/C1). Native `<video>` controls are off,
  // so the in-player menu drives display. Three cases:
  //   - a plain-text track → set the matching `<track>`'s `mode` to `showing`, others disabled
  //   - an ASS/SSA (or image) track → build the client renderer (libass/libbitsub), disable
  //     every `<track>`; an image track / libass failure calls back to burn-in
  //   - nothing selected → all `<track>`s disabled, no client renderer
  // `<track>` elements map to `video.textTracks` in DOM order == `textTracks` array order.
  // Re-runs on selection change and after a (re)attach (decisionId) so a new source picks it up.
  const activeTextTrackSrc = activeSubtitle?.textTrackSrc ?? null;
  const clientRendered = activeSubtitle != null && activeSubtitle.textTrackSrc === null;
  const activeSubKey = activeSubtitle ? `${activeSubtitle.id}|${activeSubtitle.textTrackSrc ?? ''}` : '';
  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;

    // Native `<track>` modes (only relevant when a plain-text track is selected).
    const tt = video.textTracks;
    const applyModes = () => {
      if (!textTracks) return;
      for (let i = 0; i < textTracks.length && i < tt.length; i += 1) {
        const wanted = !clientRendered && activeTextTrackSrc != null && textTracks[i]?.src === activeTextTrackSrc;
        tt[i]!.mode = wanted ? 'showing' : 'disabled';
      }
    };
    applyModes();
    tt.addEventListener?.('addtrack', applyModes);
    // A freshly (re)shown native track has original cue times, so the applied-offset baseline
    // resets here; the offset effect below re-shifts from 0 for the new selection.
    appliedOffsetRef.current = 0;

    // Client renderer (ASS via libass; image → burn-in fallback).
    let handle: SubtitleRenderHandle | null = null;
    let disposed = false;
    if (clientRendered && activeSubtitle) {
      diag.info('subtitles', 'client render', { codec: activeSubtitle.track.codec, id: activeSubtitle.id });
      void createSubtitleRenderer({
        api,
        fileId,
        video,
        track: activeSubtitle.track,
        trackId: activeSubtitle.id,
        offsetSeconds: subtitleOffsetSeconds,
        videoFps: videoFpsRef.current,
        onUnsupported: (reason) => {
          diag.info('subtitles', 'client render unsupported → burn-in', { reason });
          onSubtitleBurnIn?.(activeSubtitle.track);
        },
      })
        .then((h) => {
          if (disposed) {
            h?.destroy();
            return;
          }
          handle = h;
          rendererRef.current = h;
        })
        .catch((err: unknown) => {
          diag.error('subtitles', 'renderer failed', { message: String(err) });
          onSubtitleBurnIn?.(activeSubtitle.track);
        });
    }

    return () => {
      disposed = true;
      tt.removeEventListener?.('addtrack', applyModes);
      handle?.destroy();
      if (rendererRef.current === handle) rendererRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, fileId, activeSubKey, textTracks, decisionId]);

  // Apply the sync offset (`docs/.tasks/99` C5). A live client renderer (libass) takes an
  // absolute offset. For a native `<track>` we shift each cue's start/end by the DELTA from the
  // last-applied offset (tracked in `appliedOffsetRef`) so repeated nudges accumulate correctly.
  // Maintain a scoped `::cue` stylesheet for the viewer's subtitle appearance (`docs/.tasks/99`
  // C4). One `<style>` per player instance, updated in place when the settings change.
  useEffect(() => {
    if (!subtitleAppearance) return;
    const el = document.createElement('style');
    el.textContent = cueCss(subtitleAppearance, `video.${cueScopeClass}`);
    document.head.appendChild(el);
    return () => {
      el.remove();
    };
  }, [subtitleAppearance, cueScopeClass]);

  useEffect(() => {
    // Client renderer: absolute offset.
    rendererRef.current?.setOffset(subtitleOffsetSeconds);

    // Native <track>: shift the showing track's cues by the delta.
    const video = videoRef.current;
    const delta = subtitleOffsetSeconds - appliedOffsetRef.current;
    if (video && delta !== 0) {
      const tt = video.textTracks;
      // Firefox keeps an already-active cue displayed at its old time after a start/end edit;
      // toggling the track's mode forces it to re-evaluate active cues. Harmless on Chromium.
      const isFirefox = /firefox/i.test(navigator.userAgent);
      for (let i = 0; i < tt.length; i += 1) {
        const track = tt[i];
        if (track && track.mode === 'showing' && track.cues) {
          for (let c = 0; c < track.cues.length; c += 1) {
            const cue = track.cues[c];
            if (cue) {
              cue.startTime = Math.max(0, cue.startTime + delta);
              cue.endTime = Math.max(0, cue.endTime + delta);
            }
          }
          if (isFirefox) {
            track.mode = 'hidden';
            track.mode = 'showing';
          }
        }
      }
    }
    appliedOffsetRef.current = subtitleOffsetSeconds;
  }, [subtitleOffsetSeconds]);

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
        className={cueScopeClass}
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
            `srclang`. Native controls are off, so the in-player caption menu (`99` A2) drives
            each track's `mode` via effect (3) above — we deliberately DON'T set the `default`
            attribute here (it would make the browser auto-show a track and fight that effect). */}
        {textTracks?.map((t) => (
          <track
            key={t.src}
            kind="subtitles"
            src={t.src}
            srcLang={t.srclang}
            label={t.label}
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

/**
 * Memoized so a parent re-render (e.g. PlayerPage's overlay show/hide, `setBaseMode`,
 * `switchingAudio`) does NOT re-run the player and its effects. Combined with the parent
 * passing a stable `onDecision`, this is what keeps one playback to one transcode session.
 */
export const VideoPlayer = memo(VideoPlayerInner);

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
