/**
 * `@medi/player` — Phase 5 (`docs/.tasks/50-phase5-playback-packaging.md`).
 *
 * Full-length TV playback: a react-native-video wrapper (`VideoScreen`) plus a
 * custom transport overlay (`PlayerOverlay`) driven entirely by the low-level
 * `useTVEventHandler` hook via `usePlayerControls` — the overlay deliberately
 * bypasses the spatial-navigation focus engine to avoid Android TV focus
 * thrashing / render lag (README §Video Playback and Overlay Integration).
 *
 * Direct-play and HLS are handled uniformly; the transcode-session-cap `409` is
 * surfaced as a non-fatal notice. Timeline scrubbing renders trickplay thumbnails
 * from the tiled-JPG mosaic (see `trickplay.ts` and the package README for the
 * backend metadata endpoint this consumes).
 *
 * Phase 4 shipped only the hover *preview* (`@medi/ui` `HoverPreview`); this
 * package is the full player it deferred.
 */

export const PLAYER_PHASE = 5 as const;

export { VideoScreen } from './VideoScreen';
export type { VideoScreenProps, ResolvedStream, TextTrack } from './VideoScreen';

export { PlayerOverlay } from './PlayerOverlay';
export type { PlayerOverlayProps } from './PlayerOverlay';

export {
  usePlayerControls,
  SEEK_STEP_MS,
  HIDE_AFTER_MS,
  SEEK_SETTLE_MS,
} from './usePlayerControls';
export type {
  PlayerControls,
  PlayerRemoteEvent,
  UsePlayerControlsOptions,
} from './usePlayerControls';

export {
  tileForPosition,
  tileCount,
} from './trickplay';
export type { TrickplayMeta, TrickplayTile } from './trickplay';

export {
  chapterAt,
  nextChapterMs,
  previousChapterMs,
  PREVIOUS_CHAPTER_GRACE_MS,
} from './chapters';
