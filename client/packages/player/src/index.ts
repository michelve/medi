/**
 * `@medi/player` — Phase 5 (`docs/.tasks/50-phase5-playback-packaging.md`).
 *
 * This package will hold the full react-native-video wrapper and the custom
 * playback overlay driven by the low-level `useTVEventHandler` hook (README
 * §Video Playback and Overlay Integration — the overlay deliberately bypasses the
 * spatial-navigation focus engine to avoid Android TV focus thrashing), plus
 * trickplay-sprite scrubbing.
 *
 * Phase 4 ships only the hover *preview* (`@medi/ui` `HoverPreview`), which uses
 * react-native-video directly for the silent 720p clip. Full-length playback is
 * intentionally deferred; this placeholder keeps the workspace resolvable.
 */

export const PLAYER_PHASE = 5 as const;
