/**
 * `Page` — the per-screen spatial-navigation root.
 *
 * Wrap every full-screen route in a `Page`. It provides:
 *  - a `SpatialNavigationRoot` scoped to this screen, gated by `isActive` so an
 *    off-screen page's nodes never steal D-pad focus (README §Spatial Nav);
 *  - the `SpatialNavigationDeviceTypeProvider`, which lets focusable views also
 *    respond to pointer hover on web-TV builds (harmless on tvOS/Android TV);
 *  - an `onDirectionHandledWithoutMovement` hook so a screen can hand focus off
 *    to a sibling navigator (e.g. a side drawer) when focus hits its edge.
 */

import React from 'react';
import {
  SpatialNavigationRoot,
  SpatialNavigationDeviceTypeProvider,
} from 'react-tv-space-navigation';

export type EdgeDirection = 'up' | 'down' | 'left' | 'right';

export interface PageProps {
  /**
   * Whether this page owns D-pad focus right now. In a stack navigator, only the
   * top screen should be `true`; background screens pass `false` so their nodes
   * are inert. Defaults to `true` for single-screen usage.
   */
  isActive?: boolean;
  /**
   * Called when the user presses a direction that this navigator could not act
   * on (focus is already at the edge). Use it to activate a neighboring
   * navigator — e.g. a side menu on `left`. See `useEdgeHandoff`.
   */
  onEdge?: (direction: EdgeDirection) => void;
  children: React.ReactNode;
}

export function Page({ isActive = true, onEdge, children }: PageProps): React.JSX.Element {
  return (
    <SpatialNavigationDeviceTypeProvider>
      <SpatialNavigationRoot
        isActive={isActive}
        onDirectionHandledWithoutMovement={
          onEdge ? (direction) => onEdge(direction as EdgeDirection) : undefined
        }
      >
        {children}
      </SpatialNavigationRoot>
    </SpatialNavigationDeviceTypeProvider>
  );
}
