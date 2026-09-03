/**
 * `usePlayerControls` — the entire overlay/transport state machine for the TV
 * player, driven exclusively by raw remote events (`docs/.tasks/50` Part A
 * sub-task 2).
 *
 * ## Why not spatial navigation
 * On Android TV, absolutely-positioned Touchable/Pressable controls layered over
 * the ExoPlayer surface break D-pad routing and cause render lag (README §Video
 * Playback and Overlay Integration). So the overlay owns NO focusable nodes: it
 * is a `pointerEvents="none"` presentational layer, and every button press is
 * intercepted globally via `useTVEventHandler` and turned into a state change
 * here. The spatial navigator (react-tv-space-navigation) is not involved on the
 * player screen at all.
 *
 * ## Behaviour
 * - Any key press reveals the overlay and re-arms the auto-hide timer.
 * - `select` / `playPause` toggle play/pause (and keep the overlay up).
 * - `left` / `right` enter *seek mode*: repeated presses accumulate a seek offset
 *   (a scrub), shown live with a trickplay thumbnail; the seek is committed to the
 *   player after a short settle so we don't thrash the decoder on every tap.
 * - The overlay auto-hides after `HIDE_AFTER_MS` of no input while playing.
 *
 * The hook is deliberately player-agnostic: it holds *intent* (isPlaying, the
 * pending seek target) and calls back out; `VideoScreen` binds those to the
 * `<Video>` ref. This keeps it unit-testable without a native module.
 */

import { useCallback, useEffect, useMemo, useReducer, useRef } from 'react';

/** Raw remote keys the player reacts to (subset of react-native-tvos events). */
export type PlayerRemoteEvent =
  | 'select'
  | 'playPause'
  | 'left'
  | 'right'
  | 'up'
  | 'down';

/** Coarse-grained step per D-pad tap while scrubbing (ms). Long-press repeats accumulate. */
export const SEEK_STEP_MS = 10_000;
/** Auto-hide the overlay after this idle time while playing (ms). */
export const HIDE_AFTER_MS = 4_000;
/** Commit an accumulated scrub to the player after this quiet period (ms). */
export const SEEK_SETTLE_MS = 450;

interface State {
  isPlaying: boolean;
  overlayVisible: boolean;
  /**
   * When scrubbing, the *pending* target position (ms) the user has dialed with
   * left/right taps but not yet committed. `null` when not scrubbing.
   */
  scrubTargetMs: number | null;
  /** Latest known playback position from the player (ms). */
  positionMs: number;
  /** Total media duration (ms); 0 until the player reports it. */
  durationMs: number;
}

type Action =
  | { type: 'SHOW_OVERLAY' }
  | { type: 'HIDE_OVERLAY' }
  | { type: 'TOGGLE_PLAY' }
  | { type: 'SET_PLAYING'; playing: boolean }
  | { type: 'SCRUB'; deltaMs: number }
  | { type: 'COMMIT_SCRUB' }
  | { type: 'CANCEL_SCRUB' }
  | { type: 'PROGRESS'; positionMs: number }
  | { type: 'DURATION'; durationMs: number };

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(Math.max(v, lo), hi);
}

function reducer(state: State, action: Action): State {
  switch (action.type) {
    case 'SHOW_OVERLAY':
      return state.overlayVisible ? state : { ...state, overlayVisible: true };
    case 'HIDE_OVERLAY':
      // Never hide mid-scrub.
      if (state.scrubTargetMs !== null) return state;
      return { ...state, overlayVisible: false };
    case 'TOGGLE_PLAY':
      return { ...state, isPlaying: !state.isPlaying, overlayVisible: true };
    case 'SET_PLAYING':
      return { ...state, isPlaying: action.playing };
    case 'SCRUB': {
      const base = state.scrubTargetMs ?? state.positionMs;
      const upper = state.durationMs > 0 ? state.durationMs : Number.MAX_SAFE_INTEGER;
      return {
        ...state,
        overlayVisible: true,
        scrubTargetMs: clamp(base + action.deltaMs, 0, upper),
      };
    }
    case 'COMMIT_SCRUB':
      if (state.scrubTargetMs === null) return state;
      return {
        ...state,
        positionMs: state.scrubTargetMs,
        scrubTargetMs: null,
      };
    case 'CANCEL_SCRUB':
      return state.scrubTargetMs === null ? state : { ...state, scrubTargetMs: null };
    case 'PROGRESS':
      // A live progress tick must not fight an in-flight scrub.
      if (state.scrubTargetMs !== null) return state;
      return { ...state, positionMs: action.positionMs };
    case 'DURATION':
      return { ...state, durationMs: action.durationMs };
    default:
      return state;
  }
}

export interface PlayerControls {
  isPlaying: boolean;
  overlayVisible: boolean;
  /** True while the user is actively scrubbing (overlay shows the trickplay thumb). */
  isScrubbing: boolean;
  /**
   * The position to *display* — the pending scrub target while scrubbing, else the
   * live playback position (ms).
   */
  displayPositionMs: number;
  durationMs: number;
  /** Feed the remote event here (from the screen's `useTVEventHandler`). */
  handleRemote: (event: PlayerRemoteEvent) => void;
  /** Player → hook: report the current playback position (from `onProgress`). */
  reportProgress: (positionMs: number) => void;
  /** Player → hook: report the media duration (from `onLoad`). */
  reportDuration: (durationMs: number) => void;
  /** Player → hook: reflect an externally-driven play/pause (e.g. buffering). */
  setPlaying: (playing: boolean) => void;
  /** Manually reveal the overlay (e.g. on mount / on error). */
  showOverlay: () => void;
}

export interface UsePlayerControlsOptions {
  /**
   * Commit a seek to the player (called after the scrub settles). `positionMs` is
   * the absolute target. Wire this to the `<Video>` ref's `seek`.
   */
  onSeek: (positionMs: number) => void;
  /** Start playing (video paused=false). */
  onPlay?: () => void;
  /** Pause (video paused=true). */
  onPause?: () => void;
  /** Start playing immediately (autoplay). Defaults to `true`. */
  autoplay?: boolean;
  /**
   * Web only: let the element's own `play`/`pause` events (via [`setPlaying`]) drive the
   * play state instead of the hook commanding it on mount. When `true`, the reducer starts
   * `isPlaying: false` and the "reflect intent" effect is primed so the first real
   * transition fires `onPlay`/`onPause` — this keeps the DOM `<video>` and the UI in lockstep
   * (the browser autostarts via `video.play()`, then its `onplay` event flips the state). The
   * RN/TV client leaves this unset: it binds `paused={!isPlaying}` declaratively and relies on
   * `autoplay` seeding `isPlaying: true`.
   */
  reflectFromEvents?: boolean;
}

export function usePlayerControls(options: UsePlayerControlsOptions): PlayerControls {
  const { onSeek, onPlay, onPause, autoplay = true, reflectFromEvents = false } = options;

  // When the element drives the state (web), start paused so the first real `play` event
  // flips `isPlaying` and keeps the UI in lockstep; otherwise (TV) seed from `autoplay`.
  const initialPlaying = reflectFromEvents ? false : autoplay;

  const [state, dispatch] = useReducer(reducer, {
    isPlaying: initialPlaying,
    overlayVisible: true, // visible on mount, then auto-hides
    scrubTargetMs: null,
    positionMs: 0,
    durationMs: 0,
  });

  // Timers live in refs so re-renders don't lose them.
  const hideTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const seekTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const clearHide = useCallback(() => {
    if (hideTimer.current) {
      clearTimeout(hideTimer.current);
      hideTimer.current = null;
    }
  }, []);

  const armHide = useCallback(() => {
    clearHide();
    hideTimer.current = setTimeout(() => dispatch({ type: 'HIDE_OVERLAY' }), HIDE_AFTER_MS);
  }, [clearHide]);

  // Reflect play/pause intent out to the player whenever it changes.
  const wasPlaying = useRef(state.isPlaying);
  useEffect(() => {
    if (state.isPlaying === wasPlaying.current) return;
    wasPlaying.current = state.isPlaying;
    if (state.isPlaying) onPlay?.();
    else onPause?.();
  }, [state.isPlaying, onPlay, onPause]);

  // Re-arm auto-hide whenever the overlay is shown while playing; keep it up while
  // paused or scrubbing (the user is clearly interacting).
  useEffect(() => {
    if (state.overlayVisible && state.isPlaying && state.scrubTargetMs === null) {
      armHide();
    } else {
      clearHide();
    }
    return clearHide;
  }, [state.overlayVisible, state.isPlaying, state.scrubTargetMs, armHide, clearHide]);

  const handleRemote = useCallback((event: PlayerRemoteEvent) => {
    switch (event) {
      case 'select':
      case 'playPause':
        dispatch({ type: 'TOGGLE_PLAY' });
        return;
      case 'left':
        dispatch({ type: 'SCRUB', deltaMs: -SEEK_STEP_MS });
        return;
      case 'right':
        dispatch({ type: 'SCRUB', deltaMs: SEEK_STEP_MS });
        return;
      case 'up':
      case 'down':
        // No vertical action on the player; just reveal the overlay.
        dispatch({ type: 'SHOW_OVERLAY' });
        return;
    }
  }, []);

  // When a scrub target is set, (re)start the settle timer; on quiet, commit the
  // seek to the player and drop out of scrub mode.
  useEffect(() => {
    if (state.scrubTargetMs === null) return;
    if (seekTimer.current) clearTimeout(seekTimer.current);
    const target = state.scrubTargetMs;
    seekTimer.current = setTimeout(() => {
      onSeek(target);
      dispatch({ type: 'COMMIT_SCRUB' });
    }, SEEK_SETTLE_MS);
    return () => {
      if (seekTimer.current) clearTimeout(seekTimer.current);
    };
  }, [state.scrubTargetMs, onSeek]);

  const reportProgress = useCallback(
    (positionMs: number) => dispatch({ type: 'PROGRESS', positionMs }),
    [],
  );
  const reportDuration = useCallback(
    (durationMs: number) => dispatch({ type: 'DURATION', durationMs }),
    [],
  );
  const setPlaying = useCallback(
    (playing: boolean) => dispatch({ type: 'SET_PLAYING', playing }),
    [],
  );
  const showOverlay = useCallback(() => dispatch({ type: 'SHOW_OVERLAY' }), []);

  return useMemo<PlayerControls>(
    () => ({
      isPlaying: state.isPlaying,
      overlayVisible: state.overlayVisible,
      isScrubbing: state.scrubTargetMs !== null,
      displayPositionMs: state.scrubTargetMs ?? state.positionMs,
      durationMs: state.durationMs,
      handleRemote,
      reportProgress,
      reportDuration,
      setPlaying,
      showOverlay,
    }),
    [state, handleRemote, reportProgress, reportDuration, setPlaying, showOverlay],
  );
}
