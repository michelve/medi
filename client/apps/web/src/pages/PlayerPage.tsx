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
import {
  ApiError,
  type FileAudioTrack,
  type FileChapter,
  type FileSubtitleTrack,
  type StreamDecision,
  type SubtitleStream,
} from '@medi/api-client';
import { useApi } from '../api';
import { ResumeChip } from '../components/ResumeChip';
import { nextChapterMs, previousChapterMs } from '@medi/player/chapters';
import {
  autoSelectAudio,
  autoSelectSubtitle,
  readRememberedAudio,
  readRememberedSubtitle,
  rememberAudio,
  rememberSubtitle,
  subtitleLabel,
  subtitleTrackId,
} from '../lib/subtitleSelection';
import { usePlaybackProgress } from '../lib/usePlaybackProgress';
import { VideoPlayer, type WebTextTrack } from '../components/VideoPlayer';
import { PlayerEventLog } from '../components/PlayerEventLog';
import type { PlayerDiagnostics } from '../lib/playerDiagnostics';
import { PlayerControls, SubtitleMenu, type SubtitleMenuEntry } from '../components/PlayerControls';
import { SubtitleSettings } from '../components/SubtitleSettings';
import { readAppearance, writeAppearance, type SubtitleAppearance } from '../lib/subtitleAppearance';
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
  // The raw subtitle track list from GET /api/files/:id (`docs/.tasks/99`): drives the caption
  // menu (with codec/format for render-path + badges). `fileSubtitles` above stays for the
  // nav-state `<track>` path.
  const [subtitleTracks, setSubtitleTracks] = useState<FileSubtitleTrack[]>([]);
  // The active caption track id (`ext<id>` or a stream_index string), or `null` for Off.
  const [activeSubtitleId, setActiveSubtitleId] = useState<string | null>(null);
  // Subtitle sync offset in seconds (`docs/.tasks/99` C5); + = later, - = earlier.
  const [subtitleOffset, setSubtitleOffset] = useState(0);
  // A forced burn-in override (`docs/.tasks/99` C1 fallback): set when libass fails on an ASS
  // track, so we fall back to a server burn-in of that stream_index. Cleared on track change.
  const [forceBurnIn, setForceBurnIn] = useState<number | null>(null);
  // Guards the one-shot auto-select so it doesn't re-fire and override a manual choice.
  const autoSelectedRef = useRef(false);
  const autoSelectedAudioRef = useRef(false);
  // Embedded chapters (`docs/.tasks/99`) — scrub-bar ticks + prev/next nav.
  const [chapters, setChapters] = useState<FileChapter[]>([]);
  // Video frame rate (`docs/.tasks/99`) for libass `targetFps`; undefined until known.
  const [videoFps, setVideoFps] = useState<number | undefined>(undefined);
  // Subtitle appearance (`docs/.tasks/99` C4): loaded from localStorage, editable in a panel.
  const [subtitleAppearance, setSubtitleAppearance] = useState<SubtitleAppearance>(() => readAppearance());
  const [appearanceOpen, setAppearanceOpen] = useState(false);
  // The base decision's mode, so a non-default audio pick on a `direct` file forces a transcode
  // (a browser <video> can't switch an embedded audio track).
  const [baseMode, setBaseMode] = useState<StreamDecision['mode'] | null>(null);

  // A *stable* onDecision (identity never changes) so the memoized <VideoPlayer> isn't re-run
  // on every PlayerPage render. `audioTrack` is read through a ref rather than a closure so the
  // callback needn't be recreated when it changes — the player already re-resolves on an
  // `audioTrack` prop change, and an unstable callback here would spawn duplicate transcode
  // sessions (the very bug this fixes).
  const audioTrackRef = useRef(audioTrack);
  audioTrackRef.current = audioTrack;
  const onDecision = useCallback((d: StreamDecision) => {
    // Remember the *base* (default-track) decision mode so a non-default audio pick on a direct
    // file forces a transcode. Ignore decisions made while a switch is active.
    if (audioTrackRef.current == null) setBaseMode(d.mode);
  }, []);

  // Fetch the file's audio + subtitle tracks (deep-link menus, Part C).
  useEffect(() => {
    if (!Number.isFinite(id)) return;
    const controller = new AbortController();
    api
      .files(id, { signal: controller.signal })
      .then((tracks) => {
        if (controller.signal.aborted) return;
        setAudioTracks(tracks.audio);
        // Auto-select audio from the remembered cross-title choice (`docs/.tasks/99` C3), once.
        if (!autoSelectedAudioRef.current) {
          autoSelectedAudioRef.current = true;
          const pick = autoSelectAudio(tracks.audio, readRememberedAudio());
          if (pick != null) setAudioTrack(pick);
        }
        setChapters(tracks.chapters ?? []);
        setVideoFps(tracks.video_fps);
        setSubtitleTracks(tracks.subtitles);
        // Auto-select captions once (`docs/.tasks/99` C3): honor the remembered cross-title
        // choice, else the file's default/forced track. A manual pick later wins (guarded).
        if (!autoSelectedRef.current) {
          autoSelectedRef.current = true;
          const chosen = autoSelectSubtitle(tracks.subtitles, readRememberedSubtitle());
          setActiveSubtitleId(chosen ? subtitleTrackId(chosen) : null);
        }
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
          label: s.title ?? textTrackLabel(s),
          default: s.is_default || s.is_forced,
        };
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, id, navState?.subtitles, fileSubtitles]);

  // Caption menu entries (`docs/.tasks/99` A2) from the raw subtitle list — text tracks are
  // selectable now; image tracks are shown but flagged (client-side render lands in Phase 5).
  const subtitleEntries = useMemo<SubtitleMenuEntry[]>(
    () =>
      subtitleTracks.map((t, i) => ({
        id: subtitleTrackId(t),
        label: subtitleLabel(t, i),
        image: t.format === 'image',
      })),
    [subtitleTracks],
  );

  // Resolve the selected subtitle track (or null for Off).
  const activeTrack = useMemo(
    () =>
      activeSubtitleId == null
        ? null
        : subtitleTracks.find((t) => subtitleTrackId(t) === activeSubtitleId) ?? null,
    [activeSubtitleId, subtitleTracks],
  );

  // Client-render vs burn-in split (`docs/.tasks/99` C1). All image subs (PGS + VobSub) now
  // render client-side via libbitsub (VobSub uses the `.idx`+`.sub` raw pair). Server burn-in is
  // only the fallback when a client renderer fails (`forceBurnIn`).
  const burnInSubtitle = useMemo<number | null>(
    () => forceBurnIn, // null unless a libass/libbitsub failure fell back to burn-in
    [forceBurnIn],
  );

  // The active caption for the player's CLIENT-SIDE path: plain text (native `<track>`), ASS
  // (libass), or image PGS/VobSub (libbitsub). A failure-fallback burn-in contributes nothing.
  const activeSubtitle = useMemo(() => {
    if (!activeTrack || activeSubtitleId == null || forceBurnIn != null) return null;
    const codec = (activeTrack.codec ?? '').toLowerCase();
    // Plain-text (native <track>) vs client-rendered (ASS libass / image libbitsub).
    const isPlainText = activeTrack.format !== 'image' && codec !== 'ass' && codec !== 'ssa';
    return {
      track: activeTrack,
      id: activeSubtitleId,
      textTrackSrc: isPlainText ? api.subtitleUrl(id, activeSubtitleId) : null,
    };
  }, [api, id, activeSubtitleId, activeTrack, forceBurnIn]);

  // Select a caption track (or Off) and remember the choice for future titles.
  const selectSubtitle = useCallback(
    (selId: string | null) => {
      setActiveSubtitleId(selId);
      setSubtitleOffset(0); // A fresh track starts un-shifted.
      setForceBurnIn(null); // Clear any prior libass-failure burn-in fallback.
      const track = selId == null ? null : subtitleTracks.find((t) => subtitleTrackId(t) === selId);
      rememberSubtitle({
        off: selId == null,
        language: track?.language ?? null,
        title: track?.title ?? null,
        forced: track?.is_forced ?? false,
      });
    },
    [subtitleTracks],
  );

  // Select an audio track (by stream_index) and remember it for future titles (`docs/.tasks/99`).
  const selectAudio = useCallback(
    (streamIndex: number) => {
      setAudioTrack(streamIndex);
      const track = audioTracks.find((t) => t.stream_index === streamIndex);
      rememberAudio({ language: track?.language ?? null, title: track?.title ?? null });
    },
    [audioTracks],
  );

  // Persist subtitle appearance as it's edited (`docs/.tasks/99` C4).
  const changeAppearance = useCallback((next: SubtitleAppearance) => {
    setSubtitleAppearance(next);
    writeAppearance(next);
  }, []);

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

  // Resume + throttled progress persistence (`docs/.tasks/98`): reads the saved position on
  // mount (→ `resumeMs` seeds the player's initial seek), shows a non-blocking chip, and
  // persists the position as it plays + on pause / tab-hide / unmount.
  const progress = usePlaybackProgress(api, id, videoEl);

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

  // Chapter navigation (`docs/.tasks/99`): seek to the next / previous chapter based on the
  // live playback position. Read from the <video> directly so the target is always current.
  const goNextChapter = useCallback(() => {
    const posMs = Math.round((videoElRef.current?.currentTime ?? 0) * 1000);
    const target = nextChapterMs(chapters, posMs);
    if (target != null && videoElRef.current) videoElRef.current.currentTime = target / 1000;
  }, [chapters]);
  const goPrevChapter = useCallback(() => {
    const posMs = Math.round((videoElRef.current?.currentTime ?? 0) * 1000);
    const target = previousChapterMs(chapters, posMs);
    if (target != null && videoElRef.current) videoElRef.current.currentTime = target / 1000;
  }, [chapters]);

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
        case 'PageUp':
          // Next chapter (`docs/.tasks/99`), mirroring Jellyfin's PageUp/PageDown.
          if (chapters.length > 0) {
            e.preventDefault();
            goNextChapter();
          }
          break;
        case 'PageDown':
          if (chapters.length > 0) {
            e.preventDefault();
            goPrevChapter();
          }
          break;
        case 'g':
          // Subtitle sync (`docs/.tasks/99` C5): g = earlier, h = later (Jellyfin's keys).
          if (activeSubtitleId != null) setSubtitleOffset((o) => Math.round((o - 0.5) * 10) / 10);
          break;
        case 'h':
          if (activeSubtitleId != null) setSubtitleOffset((o) => Math.round((o + 0.5) * 10) / 10);
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
  }, [handleRemote, goBack, chapters, goNextChapter, goPrevChapter, activeSubtitleId]);

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
          initialResumeMs={progress.resumeMs}
          onDecision={onDecision}
          textTracks={textTracks}
          activeSubtitle={activeSubtitle}
          subtitleOffsetSeconds={subtitleOffset}
          videoFps={videoFps}
          subtitleAppearance={subtitleAppearance}
          burnInSubtitle={burnInSubtitle}
          onSubtitleBurnIn={(track) => {
            // A libass failure on an ASS track (image tracks already burn in via `burnInSubtitle`)
            // → fall back to a server burn-in of that track's embedded stream_index, if any.
            if (track.stream_index != null) setForceBurnIn(track.stream_index);
          }}
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

      {/* Subtitle sync indicator (`docs/.tasks/99` C5): shows the current offset while non-zero
          and the overlay is up, so g/h adjustments are visible. */}
      {overlayVisible && subtitleOffset !== 0 && (
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
            fontVariantNumeric: 'tabular-nums',
          }}
        >
          Subtitle offset {subtitleOffset > 0 ? '+' : ''}
          {subtitleOffset.toFixed(1)}s
        </div>
      )}

      {/* Resume chip (`docs/.tasks/98`): a non-blocking "Resuming from mm:ss / Start over" that
          auto-dismisses. Playback already resumes via `initialResumeMs`; the chip just offers
          the start-over escape hatch. */}
      {progress.showChip && (
        <ResumeChip
          label={progress.resumeLabel}
          onStartOver={progress.startOver}
          onDismiss={progress.dismissChip}
        />
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
          onSelectAudio={selectAudio}
          subtitlesMenu={
            subtitleEntries.length > 0 ? (
              <SubtitleMenu
                entries={subtitleEntries}
                active={activeSubtitleId}
                onSelect={selectSubtitle}
                onOpenSettings={() => setAppearanceOpen(true)}
              />
            ) : undefined
          }
          chapters={chapters}
          onNextChapter={goNextChapter}
          onPrevChapter={goPrevChapter}
        />
        {/* Subtitle appearance panel (`docs/.tasks/99` C4). */}
        {appearanceOpen && (
          <SubtitleSettings
            value={subtitleAppearance}
            onChange={changeAppearance}
            onClose={() => setAppearanceOpen(false)}
          />
        )}
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
 * A `<track>` label for a subtitle stream with no explicit title (`docs/.tasks/90`):
 * uppercased language, marked `(forced)` for a forced track, else a generic fallback. (The
 * richer caption-menu label lives in `lib/subtitleSelection.ts`.)
 */
function textTrackLabel(s: SubtitleStream): string {
  const lang = s.language ? s.language.toUpperCase() : 'Subtitles';
  return s.is_forced ? `${lang} (forced)` : lang;
}
