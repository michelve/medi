/**
 * `useHoverPreview` — React binding for the hover-preview FSM.
 *
 * Owns one machine instance per poster and exposes a small, declarative surface
 * the `HoverPreview` component consumes: booleans for what to render, plus the
 * event senders the poster's focus handlers and the `<Image>`/`<Video>` fire.
 */

import { useMemo } from 'react';
import { useMachine } from '@xstate/react';

import {
  createHoverPreviewMachine,
  type HoverPreviewInput,
} from './machine';

export interface UseHoverPreviewResult {
  /** The resolved silent-preview URL, or `null` while none is mounted. */
  previewUrl: string | null;
  /** Mount the `<Video>` now? True once the 2s gate has opened and a src exists. */
  shouldMountVideo: boolean;
  /** Is the preview actively playing (as opposed to still loading)? */
  isPlaying: boolean;
  /** Last preview error (e.g. 404 — not generated yet), for optional UI. */
  error: string | null;

  /** Fire when the poster image finished loading (`<Image onLoad>`). */
  reportImageLoaded: () => void;
  /** Fire when focus/hover enters the poster. */
  onFocus: () => void;
  /** Fire when focus/hover leaves the poster (instant teardown). */
  onBlur: () => void;
  /** Fire from `<Video onLoad>` — the clip is ready and playing. */
  reportVideoLoaded: () => void;
  /** Fire from `<Video onError>` or a failed src resolve. */
  reportVideoError: (message: string) => void;
}

export function useHoverPreview(input: HoverPreviewInput): UseHoverPreviewResult {
  // Rebuild only if the target file changes; `resolvePreview` is stable enough
  // (the component memoizes it). fileId keying prevents cross-poster state bleed
  // when a virtualized row recycles a cell.
  const machine = useMemo(
    () => createHoverPreviewMachine(input),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [input.fileId],
  );

  const [state, send] = useMachine(machine);

  // The <Video> mounts once the gate has opened (`canMoveOn`) or we're already
  // playing, AND a src has resolved. During `cannotMoveOn` (the gate) nothing
  // is mounted, so fast scrolling never triggers a load.
  const inCanMoveOn = state.matches({
    showingVideo: { loadingVideoSrc: 'canMoveOn' },
  });
  const inPlaying = state.matches({ showingVideo: 'playing' });
  const shouldMountVideo = (inCanMoveOn || inPlaying) && state.context.previewUrl != null;

  return {
    previewUrl: state.context.previewUrl,
    shouldMountVideo,
    isPlaying: inPlaying,
    error: state.context.error,

    reportImageLoaded: () => send({ type: 'REPORT_IMAGE_LOADED' }),
    onFocus: () => send({ type: 'FOCUS' }),
    onBlur: () => send({ type: 'BLUR' }),
    reportVideoLoaded: () => send({ type: 'REPORT_VIDEO_LOADED' }),
    reportVideoError: (message: string) =>
      send({ type: 'REPORT_VIDEO_ERROR', message }),
  };
}
