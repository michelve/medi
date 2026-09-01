/**
 * `FocusGuide` — a thin, typed wrapper over react-native-tvos'
 * `TVFocusGuideView` (README §Spatial Navigation → TVFocusGuideView).
 *
 * On **Apple TV** this maps to Apple's native `UIFocusGuide`: an invisible bridge
 * that redirects the tvOS focus engine across empty or asymmetrical space so
 * focus follows the designer's intent rather than nearest-neighbor geometry
 * (e.g. guiding focus over a gap between an off-center hero and a carousel).
 *
 * On **Android TV / web** `TVFocusGuideView` degrades to a plain `View` — the
 * deterministic routing there is handled entirely by react-tv-space-navigation,
 * so this component is inert but safe to render on every platform.
 *
 * Two modes:
 *  - `destinations`: hand the guide explicit target components; focus entering the
 *    guide's frame is redirected to them.
 *  - `autoFocus`: the guide remembers and restores the last-focused child.
 */

import React from 'react';
import { Platform, View, type ViewProps } from 'react-native';

// `TVFocusGuideView` is exported by react-native-tvos. Import lazily/defensively
// so a non-TV RN build (or type resolution without the fork) still compiles.
type FocusGuideNativeProps = ViewProps & {
  destinations?: Array<React.Component | null>;
  autoFocus?: boolean;
  trapFocusUp?: boolean;
  trapFocusDown?: boolean;
  trapFocusLeft?: boolean;
  trapFocusRight?: boolean;
};

// eslint-disable-next-line @typescript-eslint/no-var-requires
const RN = require('react-native') as {
  TVFocusGuideView?: React.ComponentType<FocusGuideNativeProps>;
};

const NativeFocusGuide: React.ComponentType<FocusGuideNativeProps> =
  RN.TVFocusGuideView ?? (View as React.ComponentType<FocusGuideNativeProps>);

export interface FocusGuideProps extends ViewProps {
  /**
   * Explicit focus destinations. When the tvOS focus engine enters this guide's
   * frame, it is redirected to (one of) these. Ignored off tvOS.
   */
  destinations?: Array<React.Component | null>;
  /** Let the guide auto-remember and restore its last-focused child. */
  autoFocus?: boolean;
  /** Trap focus within the guide on the given edges (rarely needed; prefer FocusTrap). */
  trap?: Partial<Record<'up' | 'down' | 'left' | 'right', boolean>>;
  children?: React.ReactNode;
}

export function FocusGuide({
  destinations,
  autoFocus,
  trap,
  children,
  ...viewProps
}: FocusGuideProps): React.JSX.Element {
  // Only pass tvOS-specific props on Apple TV to avoid noisy warnings elsewhere.
  const tvProps: FocusGuideNativeProps =
    Platform.isTV && Platform.OS === 'ios'
      ? {
          destinations,
          autoFocus,
          trapFocusUp: trap?.up,
          trapFocusDown: trap?.down,
          trapFocusLeft: trap?.left,
          trapFocusRight: trap?.right,
        }
      : {};

  return (
    <NativeFocusGuide {...viewProps} {...tvProps}>
      {children}
    </NativeFocusGuide>
  );
}
