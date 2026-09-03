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

import { useEffect, useRef, useState } from 'react';
import { ApiError, type StreamDecision } from '@medi/api-client';
import { useApi } from '../api';
import { theme } from '../theme';
import { Loading } from './Status';

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
   * WebVTT text subtitles to attach as `<track>` children (`docs/.tasks/90`). Text tracks
   * ride alongside a direct-played or transcoded stream with no extra work; image tracks
   * are burned in server-side (via the stream decision) and never appear here.
   */
  textTracks?: WebTextTrack[];
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

export function VideoPlayer({ fileId, onVideoRef, onDecision, textTracks }: VideoPlayerProps) {
  const api = useApi();
  const videoRef = useRef<HTMLVideoElement | null>(null);
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

  // 1) Resolve the stream decision (direct vs HLS) from the server.
  useEffect(() => {
    const controller = new AbortController();
    setPhase({ kind: 'loading' });
    api
      // `platform: 'web'` selects the browser capability profile so the server transcodes
      // codecs a browser can't decode (HEVC / AC-3 / DTS) instead of handing back a direct
      // stream that would black-screen. `forceTranscode` is set by the fallback below after a
      // `direct` stream failed to play — it demands an HLS transcode hls.js can always play.
      .stream(fileId, { platform: 'web', forceTranscode }, { signal: controller.signal })
      .then((decision) => {
        if (controller.signal.aborted) return;
        onDecision?.(decision);
        setPhase({ kind: 'ready', decision });
      })
      .catch((err: unknown) => {
        if (controller.signal.aborted) return;
        const busy = err instanceof ApiError && err.isBusy;
        const message = err instanceof ApiError ? err.message : String(err);
        setPhase({ kind: 'error', message, busy });
      });
    return () => controller.abort();
  }, [api, fileId, attempt, forceTranscode, onDecision]);

  // 2) Attach the resolved source to the <video> (direct src / native HLS / hls.js).
  useEffect(() => {
    if (phase.kind !== 'ready') return;
    const video = videoRef.current;
    if (!video) return;
    const { decision } = phase;

    if (decision.mode === 'direct') {
      video.src = api.directUrl(fileId);
      void startPlayback(video);
      return () => {
        video.removeAttribute('src');
        video.load();
      };
    }

    // HLS. Prefer native (Safari); else hls.js.
    const hlsUrl = api.hlsUrl(decision);
    if (video.canPlayType('application/vnd.apple.mpegurl')) {
      video.src = hlsUrl;
      void startPlayback(video);
      return () => {
        video.removeAttribute('src');
        video.load();
      };
    }

    // hls.js is loaded on demand (dynamic import) so it stays OUT of the browse bundle —
    // only the player route pulls it. `cancelled`/`instance` bridge the async gap so the
    // effect cleanup can still destroy whatever got created.
    let cancelled = false;
    let instance: import('hls.js').default | null = null;
    void import('hls.js').then(({ default: Hls }) => {
      if (cancelled) return;
      if (!Hls.isSupported()) {
        setPhase({
          kind: 'error',
          message: 'This browser cannot play the transcoded (HLS) stream.',
          busy: false,
        });
        return;
      }
      const hls = new Hls({ enableWorker: true });
      instance = hls;
      hls.loadSource(hlsUrl);
      hls.attachMedia(video);
      hls.on(Hls.Events.MEDIA_ATTACHED, () => void startPlayback(video));
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
      instance?.destroy();
    };
  }, [api, fileId, phase]);

  const setRef = (el: HTMLVideoElement | null) => {
    videoRef.current = el;
    onVideoRef?.(el);
  };

  return (
    <div
      style={{
        position: 'relative',
        width: '100%',
        aspectRatio: '16 / 9',
        background: '#000',
        borderRadius: 8,
        overflow: 'hidden',
      }}
    >
      <video
        ref={setRef}
        style={{ width: '100%', height: '100%', display: 'block', background: '#000' }}
        crossOrigin="anonymous"
        playsInline
        // A decode / unsupported-source error would otherwise be a silent black screen — surface
        // it in the error phase with a Retry (`docs/.tasks/82`). Only report a real element error
        // (an aborted src swap during cleanup sets no `error`).
        onError={(e) => {
          const err = e.currentTarget.error;
          if (!err) return;
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
