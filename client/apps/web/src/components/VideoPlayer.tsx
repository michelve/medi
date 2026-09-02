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

export interface VideoPlayerProps {
  fileId: number;
  /** Receives the `<video>` element once mounted, so the page can bind transport controls. */
  onVideoRef?: (el: HTMLVideoElement | null) => void;
  /** Receives the server's resolved decision (for debugging / mode display). */
  onDecision?: (decision: StreamDecision) => void;
}

type Phase =
  | { kind: 'loading' }
  | { kind: 'ready'; decision: StreamDecision }
  | { kind: 'error'; message: string; busy: boolean };

export function VideoPlayer({ fileId, onVideoRef, onDecision }: VideoPlayerProps) {
  const api = useApi();
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const [phase, setPhase] = useState<Phase>({ kind: 'loading' });
  // Bumped to force a re-resolve on "retry".
  const [attempt, setAttempt] = useState(0);

  // 1) Resolve the stream decision (direct vs HLS) from the server.
  useEffect(() => {
    const controller = new AbortController();
    setPhase({ kind: 'loading' });
    api
      .stream(fileId, {}, { signal: controller.signal })
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
  }, [api, fileId, attempt, onDecision]);

  // 2) Attach the resolved source to the <video> (direct src / native HLS / hls.js).
  useEffect(() => {
    if (phase.kind !== 'ready') return;
    const video = videoRef.current;
    if (!video) return;
    const { decision } = phase;

    if (decision.mode === 'direct') {
      video.src = api.directUrl(fileId);
      return () => {
        video.removeAttribute('src');
        video.load();
      };
    }

    // HLS. Prefer native (Safari); else hls.js.
    const hlsUrl = api.hlsUrl(decision);
    if (video.canPlayType('application/vnd.apple.mpegurl')) {
      video.src = hlsUrl;
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
      hls.on(Hls.Events.ERROR, (_evt, data) => {
        // Only fatal errors abort playback; hls.js recovers from the rest itself.
        if (data.fatal) {
          setPhase({
            kind: 'error',
            message: `Stream error (${data.type}). ${data.details ?? ''}`.trim(),
            busy: false,
          });
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
        playsInline
        // Controls come from PlayerControls; keep the native ones off.
      />
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
              onClick={() => setAttempt((n) => n + 1)}
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
