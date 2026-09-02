/**
 * `PlayerPage` (Task 82) — `/play/:fileId`.
 *
 * Composes `VideoPlayer` (server-decided direct/HLS) with `PlayerControls`, both driven by
 * the shared `usePlayerControls` reducer. DOM events feed the reducer (the task's "feed it
 * DOM events instead of RN remote events"): Space/click → play-pause, ←/→ → seek,
 * pointer-move → reveal overlay. Trickplay meta is best-effort — a 404 just yields a plain
 * scrub bar.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useParams, useNavigate, useLocation } from 'react-router-dom';
import { usePlayerControls } from '@medi/player/usePlayerControls';
import type { TrickplayMeta } from '@medi/player/trickplay';
import { ApiError } from '@medi/api-client';
import { useApi } from '../api';
import { VideoPlayer } from '../components/VideoPlayer';
import { PlayerControls } from '../components/PlayerControls';
import { NotFound } from '../components/Status';
import { theme } from '../theme';

export function PlayerPage() {
  const { fileId } = useParams<{ fileId: string }>();
  const navigate = useNavigate();
  const location = useLocation();
  const api = useApi();
  const id = Number(fileId);
  // Detail pages pass a friendly title through router state; fall back to the file id.
  const navTitle = (location.state as { title?: string } | null)?.title;

  const videoElRef = useRef<HTMLVideoElement | null>(null);
  const [trickplay, setTrickplay] = useState<TrickplayMeta | undefined>(undefined);

  // The reducer commits seeks / play / pause onto the <video> element.
  const controls = usePlayerControls({
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
        // 404 = no croppable sheet → plain bar. Anything else: also just skip thumbnails.
        if (!(err instanceof ApiError) || !err.isNotFound) {
          // Non-404 is unexpected but non-fatal; leave trickplay undefined.
        }
        setTrickplay(undefined);
      });
    return () => controller.abort();
  }, [api, id]);

  // Bind the <video> element's playback events to the reducer.
  const handleVideoRef = useCallback(
    (el: HTMLVideoElement | null) => {
      videoElRef.current = el;
      if (!el) return;
      el.onloadedmetadata = () => reportDuration(Math.round(el.duration * 1000));
      el.ontimeupdate = () => reportProgress(Math.round(el.currentTime * 1000));
      el.onplay = () => setPlaying(true);
      el.onpause = () => setPlaying(false);
    },
    [reportDuration, reportProgress, setPlaying],
  );

  // DOM keyboard transport (the RN remote's web analogue).
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
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [handleRemote]);

  const seekTo = useCallback(
    (positionMs: number) => {
      if (videoElRef.current) videoElRef.current.currentTime = positionMs / 1000;
    },
    [],
  );

  const title = useMemo(() => navTitle ?? `File ${id}`, [navTitle, id]);

  if (!Number.isFinite(id)) return <NotFound message="That isn't a valid file id." />;

  return (
    <section>
      <button
        type="button"
        onClick={() => navigate(-1)}
        style={{
          marginBottom: 16,
          padding: '6px 14px',
          borderRadius: 6,
          border: `1px solid ${theme.colors.surface}`,
          background: 'transparent',
          color: theme.colors.text,
          fontSize: 14,
          cursor: 'pointer',
        }}
      >
        ← Back
      </button>
      <div
        style={{ position: 'relative', maxWidth: 1100, margin: '0 auto' }}
        onPointerMove={showOverlay}
        onClick={() => handleRemote('playPause')}
      >
        <VideoPlayer fileId={id} onVideoRef={handleVideoRef} />
        {/* Controls overlay: its own buttons/scrub bar must not bubble a click up to the
            video-area play-toggle, so stop propagation at this boundary. */}
        <div style={{ position: 'absolute', inset: 0 }} onClick={(e) => e.stopPropagation()}>
          <PlayerControls controls={controls} title={title} trickplay={trickplay} onSeek={seekTo} />
        </div>
      </div>
    </section>
  );
}
