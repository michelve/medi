/**
 * `PlayerPage` (Task 82; full-viewport shell added by `docs/.tasks/97`) — `/play/:fileId`.
 *
 * A **full-viewport** player (Part A): a `position:fixed; inset:0` black surface holding just
 * the `<video>` and an auto-hiding control overlay — no nav chrome, no max-width box (it's a
 * sibling of `App` in the router, not a child). The only extra affordance is a small top-left
 * Back button that shares the overlay's auto-hide; `Esc` also navigates back (exiting real
 * fullscreen first).
 *
 * Composes `VideoPlayer` (server-decided direct/HLS) with `PlayerControls`, both driven by the
 * shared `usePlayerControls` reducer. DOM events feed the reducer: Space/click → play-pause,
 * ←/→ → seek, pointer-move → reveal overlay.
 *
 * Menus (audio tracks + subtitles) are populated from `GET /api/files/:id` (Part C) so a deep
 * link with no router state still shows them. Trickplay meta is best-effort — a 404 yields a
 * plain scrub bar.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useParams, useNavigate, useLocation } from 'react-router-dom';
import { usePlayerControls } from '@medi/player/usePlayerControls';
import type { TrickplayMeta } from '@medi/player/trickplay';
import { ApiError, type FileAudioTrack, type StreamDecision, type SubtitleStream } from '@medi/api-client';
import { useApi } from '../api';
import { VideoPlayer, type WebTextTrack } from '../components/VideoPlayer';
import { PlayerEventLog } from '../components/PlayerEventLog';
import type { PlayerDiagnostics } from '../lib/playerDiagnostics';
import { PlayerControls } from '../components/PlayerControls';
import { NotFound } from '../components/Status';

export function PlayerPage() {
  const { fileId } = useParams<{ fileId: string }>();
  const navigate = useNavigate();
  const location = useLocation();
  const api = useApi();
  const id = Number(fileId);
  // The player's diagnostics channel, lifted here so the (collapsed) event log renders as a
  // dismissible panel that never occupies the frame in the full-viewport layout.
  const [diag, setDiag] = useState<PlayerDiagnostics | null>(null);
  // The full-viewport container (fullscreen target) + the current <video> element.
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [videoEl, setVideoEl] = useState<HTMLVideoElement | null>(null);
  // Detail pages pass a friendly title + subtitle tracks through router state; both fall back
  // gracefully on a deep link (no state — menus then come from GET /api/files/:id).
  const navState = location.state as
    | { title?: string; subtitles?: SubtitleStream[] }
    | null;
  const navTitle = navState?.title;

  // Selected source audio track (`docs/.tasks/97` Part C). `undefined` = the server default.
  const [audioTrack, setAudioTrack] = useState<number | undefined>(undefined);
  const [switchingAudio, setSwitchingAudio] = useState(false);
  // The file's tracks (audio menu + subtitle list) from GET /api/files/:id — fixes deep links.
  const [audioTracks, setAudioTracks] = useState<FileAudioTrack[]>([]);
  const [fileSubtitles, setFileSubtitles] = useState<SubtitleStream[] | null>(null);
  // The base decision's mode, so a non-default audio pick on a `direct` file forces a transcode
  // (a browser <video> can't switch an embedded audio track).
  const [baseMode, setBaseMode] = useState<StreamDecision['mode'] | null>(null);

  // Fetch the file's audio + subtitle tracks (deep-link menus, Part C).
  useEffect(() => {
    if (!Number.isFinite(id)) return;
    const controller = new AbortController();
    api
      .files(id, { signal: controller.signal })
      .then((tracks) => {
        if (controller.signal.aborted) return;
        setAudioTracks(tracks.audio);
        // Prefer nav-state subtitles (already the full row shape); else map the file endpoint's
        // subtitle rows into the SubtitleStream shape the text-track builder consumes.
        if (!navState?.subtitles) {
          setFileSubtitles(
            tracks.subtitles.map((s) => ({
              id: s.id,
              media_file_id: id,
              stream_index: s.stream_index ?? null,
              codec: null,
              format: s.format,
              language: s.language ?? null,
              title: s.title ?? null,
              is_default: s.is_default,
              is_forced: s.is_forced,
              is_external: s.external,
              external_path: null,
            })),
          );
        }
      })
      .catch(() => {
        // Non-fatal: no menus (a fresh/unprobed file) — playback still works.
      });
    return () => controller.abort();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, id]);

  // Text subtitles → WebVTT `<track>`s (`docs/.tasks/90`). Nav state wins; else the file
  // endpoint's list. Image tracks are burned in server-side and never listed here.
  const textTracks = useMemo<WebTextTrack[]>(() => {
    const subs = navState?.subtitles ?? fileSubtitles ?? [];
    return subs
      .filter((s) => s.format === 'text')
      .map((s) => {
        const index = s.is_external ? `ext${s.id}` : String(s.stream_index);
        return {
          src: api.subtitleUrl(id, index),
          srclang: s.language ?? 'und',
          label: s.title ?? subtitleLabel(s),
          default: s.is_default || s.is_forced,
        };
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, id, navState?.subtitles, fileSubtitles]);

  const videoElRef = useRef<HTMLVideoElement | null>(null);
  const [trickplay, setTrickplay] = useState<TrickplayMeta | undefined>(undefined);

  // The reducer commits seeks / play / pause onto the <video> element.
  const controls = usePlayerControls({
    reflectFromEvents: true,
    onSeek: (positionMs) => {
      if (videoElRef.current) videoElRef.current.currentTime = positionMs / 1000;
    },
    onPlay: () => void videoElRef.current?.play().catch(() => undefined),
    onPause: () => videoElRef.current?.pause(),
  });
  const { handleRemote, reportProgress, reportDuration, setPlaying, showOverlay } = controls;

  // Best-effort trickplay geometry → map the api-client wire shape to the player's.
  useEffect(() => {
    if (!Number.isFinite(id)) return;
    const controller = new AbortController();
    api
      .trickplayMeta(id, { signal: controller.signal })
      .then((meta) => {
        if (controller.signal.aborted) return;
        setTrickplay({
          url: api.trickplayUrl(id, 'jpg'),
          intervalMs: meta.interval_ms,
          tileW: meta.tile_w,
          tileH: meta.tile_h,
          cols: meta.cols,
          rows: meta.rows,
        });
      })
      .catch((err: unknown) => {
        if (!(err instanceof ApiError) || !err.isNotFound) {
          // Non-404 is unexpected but non-fatal; leave trickplay undefined.
        }
        setTrickplay(undefined);
      });
    return () => controller.abort();
  }, [api, id]);

  // Bind the <video> element's playback events to the reducer, and surface it for the controls.
  const handleVideoRef = useCallback(
    (el: HTMLVideoElement | null) => {
      videoElRef.current = el;
      setVideoEl(el);
      if (!el) return;
      el.onloadedmetadata = () => reportDuration(Math.round(el.duration * 1000));
      el.ontimeupdate = () => reportProgress(Math.round(el.currentTime * 1000));
      el.onplay = () => setPlaying(true);
      el.onpause = () => setPlaying(false);
    },
    [reportDuration, reportProgress, setPlaying],
  );

  // Navigate back, exiting real fullscreen first if we're in it.
  const goBack = useCallback(() => {
    if (document.fullscreenElement) {
      void document.exitFullscreen().catch(() => undefined);
    }
    navigate(-1);
  }, [navigate]);

  // DOM keyboard transport + Esc-to-back (the RN remote's web analogue).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      switch (e.key) {
        case ' ':
        case 'k':
          e.preventDefault();
          handleRemote('playPause');
          break;
        case 'ArrowLeft':
          handleRemote('left');
          break;
        case 'ArrowRight':
          handleRemote('right');
          break;
        case 'ArrowUp':
        case 'ArrowDown':
          handleRemote('up');
          break;
        case 'Escape':
          // A menu's own capture-phase handler swallows Escape when it's open, so reaching here
          // means nothing is open: exit fullscreen (if any) and navigate back.
          if (document.fullscreenElement) {
            e.preventDefault();
            void document.exitFullscreen().catch(() => undefined);
          } else {
            goBack();
          }
          break;
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [handleRemote, goBack]);

  const seekTo = useCallback((positionMs: number) => {
    if (videoElRef.current) videoElRef.current.currentTime = positionMs / 1000;
  }, []);

  const title = useMemo(() => navTitle ?? `File ${id}`, [navTitle, id]);
  const overlayVisible = controls.overlayVisible;

  if (!Number.isFinite(id)) return <NotFound message="That isn't a valid file id." />;

  return (
    <div
      ref={containerRef}
      // Reveal the overlay on ANY pointer activity over the whole surface (not just the video
      // layer) so it reliably comes back after auto-hiding — regardless of which layer the
      // cursor is over. Hide the cursor while the overlay is hidden for a clean full-screen feel.
      onPointerMove={showOverlay}
      onPointerDown={showOverlay}
      style={{
        position: 'fixed',
        inset: 0,
        width: '100vw',
        height: '100dvh',
        background: '#000',
        overflow: 'hidden',
        cursor: overlayVisible ? 'default' : 'none',
      }}
    >
      {/* Video layer: a bare click toggles play/pause. */}
      <div
        style={{ position: 'absolute', inset: 0 }}
        onClick={() => handleRemote('playPause')}
      >
        <VideoPlayer
          fileId={id}
          fill
          onVideoRef={handleVideoRef}
          audioTrack={audioTrack}
          // A browser <video> can't switch an embedded audio track on a `direct` stream, so a
          // non-default pick must go through the transcode path. Force it unless we've positively
          // confirmed the base decision is already HLS (then the switch is a distinct session
          // anyway). Unknown base (fetch still racing) → force, the safe default.
          forceTranscodeForAudio={baseMode !== 'hls'}
          onSwitchingAudio={setSwitchingAudio}
          onDecision={(d) => {
            // Remember the *base* (default-track) decision mode so a non-default audio pick on
            // a direct file forces a transcode. Ignore decisions made while a switch is active.
            if (audioTrack == null) setBaseMode(d.mode);
          }}
          textTracks={textTracks}
          diagnostics={false}
          onDiagnostics={setDiag}
        />
      </div>

      {/* Top-left Back button — shares the overlay's auto-hide. It sits ABOVE the controls
          layer (`zIndex`) so the full-frame controls overlay can't intercept its clicks. */}
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          goBack();
        }}
        aria-label="Back"
        style={{
          position: 'absolute',
          top: 20,
          left: 20,
          zIndex: 20,
          display: 'inline-flex',
          alignItems: 'center',
          gap: 8,
          padding: '9px 16px 9px 12px',
          borderRadius: 12,
          border: '1px solid rgba(255,255,255,0.16)',
          background: 'rgba(28,28,32,0.42)',
          backdropFilter: 'blur(22px) saturate(160%)',
          WebkitBackdropFilter: 'blur(22px) saturate(160%)',
          color: '#fff',
          fontSize: 14,
          fontWeight: 500,
          cursor: 'pointer',
          opacity: overlayVisible ? 1 : 0,
          transition: 'opacity 200ms ease',
          pointerEvents: overlayVisible ? 'auto' : 'none',
          boxShadow: '0 4px 16px rgba(0,0,0,0.3)',
        }}
      >
        <svg width={18} height={18} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round" aria-hidden>
          <path d="M10.5 19.5 3 12m0 0 7.5-7.5M3 12h18" />
        </svg>
        Back
      </button>

      {/* "Switching audio…" toast during an audio-track change. */}
      {switchingAudio && (
        <div
          style={{
            position: 'absolute',
            top: 20,
            left: '50%',
            transform: 'translateX(-50%)',
            zIndex: 20,
            padding: '9px 18px',
            borderRadius: 12,
            background: 'rgba(28,28,32,0.42)',
            backdropFilter: 'blur(22px) saturate(160%)',
            WebkitBackdropFilter: 'blur(22px) saturate(160%)',
            border: '1px solid rgba(255,255,255,0.14)',
            color: '#fff',
            fontSize: 13,
            pointerEvents: 'none',
            boxShadow: '0 4px 16px rgba(0,0,0,0.3)',
          }}
        >
          Switching audio…
        </div>
      )}

      {/* Controls overlay: its buttons/scrub bar must not bubble a click up to the video-area
          play-toggle, so stop propagation at this boundary. When the overlay is hidden this
          layer is click-through (`pointerEvents:none`) so a bare click on the video still
          toggles play (and reveals the overlay via the pointer-move handler). */}
      <div
        style={{ position: 'absolute', inset: 0, pointerEvents: overlayVisible ? 'auto' : 'none' }}
        onClick={(e) => e.stopPropagation()}
      >
        <PlayerControls
          controls={controls}
          title={title}
          trickplay={trickplay}
          onSeek={seekTo}
          video={videoEl}
          fullscreenTarget={containerRef.current}
          audioTracks={audioTracks}
          activeAudioTrack={audioTrack}
          onSelectAudio={(idx) => setAudioTrack(idx)}
        />
      </div>

      {/* Diagnostics: collapsed by default and pinned bottom-left so it never occupies the
          frame (`docs/.tasks/97` Part A). Only rendered while the overlay is up. */}
      {diag && overlayVisible && (
        <div
          style={{ position: 'absolute', bottom: 84, left: 20, maxWidth: 420, zIndex: 5 }}
          onClick={(e) => e.stopPropagation()}
        >
          <PlayerEventLog diagnostics={diag} defaultOpen={false} />
        </div>
      )}
    </div>
  );
}

/**
 * A caption-menu label for a subtitle track with no explicit title (`docs/.tasks/90`):
 * uppercased language, marked `(forced)` for a forced track, else a generic fallback.
 */
function subtitleLabel(s: SubtitleStream): string {
  const lang = s.language ? s.language.toUpperCase() : 'Subtitles';
  return s.is_forced ? `${lang} (forced)` : lang;
}
