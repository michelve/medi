/**
 * `usePlaybackProgress` (Task 98) — resume + throttled progress persistence for the web player.
 *
 * Two halves, both keyed off the `<video>` element the player owns:
 *
 *  - **Resume**: on mount it reads `GET /api/progress/:fileId`. If a resumable position exists
 *    (not finished, and past `MIN_RESUME_MS`), it exposes `resumeMs` for the player to seed the
 *    initial seek, plus a `showChip` flag + `resumeLabel` so a small non-blocking chip can offer
 *    "Start over". The chip auto-dismisses; if left alone the title simply resumes (the seek was
 *    already seeded). `startOver()` seeks to 0 and hides the chip.
 *  - **Persist**: while playing it `PUT`s the position every {@link WRITE_INTERVAL_MS} (a timer,
 *    not on every `timeupdate`), and flushes once more on `pause`, on `visibilitychange` (tab
 *    hidden), and on unmount — the hide/unmount flush uses `navigator.sendBeacon` so it isn't
 *    dropped as the page goes away. It never writes a scrub-in-flight value: it reads the
 *    element's committed `currentTime` at write time.
 *
 * The hook binds its own `addEventListener`s to the element, so it composes with the page's
 * existing `on*` property handlers (both fire) without clobbering them.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import type { ApiClient } from '@medi/api-client';

/** Below this many ms into a title, resume is skipped (mirrors the backend `MIN_RESUME_MS`). */
export const MIN_RESUME_MS = 30_000;
/** Throttle: persist the position at most this often while playing. */
export const WRITE_INTERVAL_MS = 12_000;
/** Auto-dismiss the resume chip after this long if the viewer doesn't touch it. */
export const RESUME_CHIP_MS = 8_000;

export interface UsePlaybackProgressResult {
  /** The saved position (ms) to seed the player's initial seek, or `undefined` for none. */
  resumeMs: number | undefined;
  /** Whether the "Resume / Start over" chip should be shown. */
  showChip: boolean;
  /** Label for the chip, e.g. `"Resuming from 5:00"`. */
  resumeLabel: string;
  /** Dismiss the chip and keep playing from the resumed position. */
  dismissChip: () => void;
  /** Seek to the beginning and dismiss the chip (the "Start over" action). */
  startOver: () => void;
}

/**
 * Wire resume + progress persistence for `fileId` against the current `<video>` element.
 * Pass the element from the page (it may change across audio-track re-attaches); the hook
 * re-binds its listeners whenever it does.
 */
export function usePlaybackProgress(
  api: ApiClient,
  fileId: number,
  videoEl: HTMLVideoElement | null,
): UsePlaybackProgressResult {
  const [resumeMs, setResumeMs] = useState<number | undefined>(undefined);
  const [showChip, setShowChip] = useState(false);
  const [resumeLabel, setResumeLabel] = useState('');

  // The last position we persisted, so the flush path can skip a redundant write.
  const lastWrittenRef = useRef<number>(-1);
  // Throttle gate: the timestamp of the last successful periodic write.
  const lastWriteAtRef = useRef<number>(0);

  // -- Resume: read saved progress once on mount / file change. --------------
  useEffect(() => {
    if (!Number.isFinite(fileId)) return;
    const controller = new AbortController();
    setResumeMs(undefined);
    setShowChip(false);
    api
      .progress(fileId, { signal: controller.signal })
      .then((p) => {
        if (controller.signal.aborted || !p) return;
        // Resume only when meaningfully into the film and not already finished.
        if (p.finished || p.position_ms < MIN_RESUME_MS) return;
        setResumeMs(p.position_ms);
        setResumeLabel(`Resuming from ${formatClock(p.position_ms)}`);
        setShowChip(true);
      })
      .catch(() => {
        // No saved progress / a transient error — start from the beginning, no chip.
      });
    return () => controller.abort();
  }, [api, fileId]);

  // Auto-dismiss the chip after a few seconds (the title keeps resuming either way).
  useEffect(() => {
    if (!showChip) return;
    const t = setTimeout(() => setShowChip(false), RESUME_CHIP_MS);
    return () => clearTimeout(t);
  }, [showChip]);

  // -- Persist: throttled writes + pause / hide / unmount flushes. -----------
  // Read the element's *committed* position + duration (never a scrub target — the DOM
  // `currentTime` is the settled value once a seek lands). Returns null when not worth writing.
  const snapshot = useCallback((): { position_ms: number; duration_ms: number } | null => {
    const el = videoEl;
    if (!el) return null;
    const position_ms = Math.round(el.currentTime * 1000);
    const duration_ms = Number.isFinite(el.duration) ? Math.round(el.duration * 1000) : 0;
    if (position_ms <= 0) return null;
    return { position_ms, duration_ms };
  }, [videoEl]);

  // A normal (keepalive) write via PUT — used on the throttle tick and on pause.
  const write = useCallback(() => {
    const snap = snapshot();
    if (!snap || snap.position_ms === lastWrittenRef.current) return;
    lastWrittenRef.current = snap.position_ms;
    lastWriteAtRef.current = Date.now();
    void api.putProgress(fileId, snap, { keepalive: true }).catch(() => undefined);
  }, [api, fileId, snapshot]);

  // A beacon flush for page-hide / unload — guaranteed-delivery even as the tab closes.
  const flushBeacon = useCallback(() => {
    const snap = snapshot();
    if (!snap || snap.position_ms === lastWrittenRef.current) return;
    lastWrittenRef.current = snap.position_ms;
    api.beaconProgress(fileId, snap);
  }, [api, fileId, snapshot]);

  useEffect(() => {
    const el = videoEl;
    if (!el) return;

    // Throttled tick off `timeupdate`: write at most once per interval while playing.
    const onTimeUpdate = () => {
      if (el.paused || el.seeking) return;
      if (Date.now() - lastWriteAtRef.current < WRITE_INTERVAL_MS) return;
      write();
    };
    // Flush the committed position immediately on a pause (the user stopped here).
    const onPause = () => write();

    el.addEventListener('timeupdate', onTimeUpdate);
    el.addEventListener('pause', onPause);

    // Tab hidden → beacon flush (the reliable moment before a mobile tab is frozen/closed).
    const onVisibility = () => {
      if (document.visibilityState === 'hidden') flushBeacon();
    };
    document.addEventListener('visibilitychange', onVisibility);
    // `pagehide` covers bfcache/navigation where `visibilitychange` may not fire.
    window.addEventListener('pagehide', flushBeacon);

    return () => {
      el.removeEventListener('timeupdate', onTimeUpdate);
      el.removeEventListener('pause', onPause);
      document.removeEventListener('visibilitychange', onVisibility);
      window.removeEventListener('pagehide', flushBeacon);
      // Unmount (navigating away from the player): a final beacon so the place is saved.
      flushBeacon();
    };
  }, [videoEl, write, flushBeacon]);

  const dismissChip = useCallback(() => setShowChip(false), []);
  const startOver = useCallback(() => {
    if (videoEl) videoEl.currentTime = 0;
    lastWrittenRef.current = -1;
    setShowChip(false);
  }, [videoEl]);

  return { resumeMs, showChip, resumeLabel, dismissChip, startOver };
}

/** Format ms as `h:mm:ss` (or `m:ss` under an hour) for the resume chip label. */
function formatClock(ms: number): string {
  const total = Math.floor(ms / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const mm = h > 0 ? String(m).padStart(2, '0') : String(m);
  const ss = String(s).padStart(2, '0');
  return h > 0 ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}
