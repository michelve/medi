/**
 * `@medi/navigation` — the app's spatial-navigation layer over
 * react-tv-space-navigation. Implements the three README paradigms: focus traps,
 * directional overrides, and Apple TV `TVFocusGuideView` bridges.
 *
 * App code imports focus primitives from here (not directly from the library) so
 * the wiring (remote-control config, device-type provider) stays centralized.
 */

// One-time startup wiring.
export { configureTVRemoteControl } from './remoteControl';

// Per-screen root + edge handoff.
export { Page } from './Page';
export type { PageProps, EdgeDirection } from './Page';

// Focus traps for modals / drawers.
export { FocusTrap } from './FocusTrap';
export type { FocusTrapProps } from './FocusTrap';

// Directional overrides (hero → carousel jumps) + named focus targets.
export {
  FocusTargetProvider,
  DirectionalOverride,
  useFocusTarget,
  useRegisterFocusTarget,
  useFocusByName,
} from './directionalOverride';
export type { DirectionalOverrideProps } from './directionalOverride';

// Apple TV UIFocusGuide bridge.
export { FocusGuide } from './FocusGuide';
export type { FocusGuideProps } from './FocusGuide';

// Re-export the library primitives app/ui code composes with, so everything
// spatial-navigation flows through this one package.
export {
  SpatialNavigationNode,
  SpatialNavigationView,
  SpatialNavigationScrollView,
  SpatialNavigationFocusableView,
  SpatialNavigationVirtualizedList,
  SpatialNavigationVirtualizedGrid,
  DefaultFocus,
  useLockSpatialNavigation,
  Directions,
} from 'react-tv-space-navigation';
export type {
  SpatialNavigationNodeRef,
  SpatialNavigationVirtualizedListRef,
} from 'react-tv-space-navigation';
