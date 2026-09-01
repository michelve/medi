/**
 * `FocusTrap` — confines D-pad focus to a modal / side-drawer while it is open,
 * so the remote can never highlight the obscured background (README §Spatial
 * Navigation → Focus Traps).
 *
 * Mechanism: while `isOpen`, the trap mounts its own `SpatialNavigationRoot`
 * (`isActive` = open) and, on mount, `lock()`s the *parent* navigator via
 * `useLockSpatialNavigation`; on close/unmount it `unlock()`s. Locking the parent
 * means directional presses cannot escape into the background tree even though it
 * is still rendered behind the overlay. `DefaultFocus` pulls focus onto the first
 * focusable inside the trap the moment it opens.
 */

import React, { useEffect } from 'react';
import {
  SpatialNavigationRoot,
  DefaultFocus,
  useLockSpatialNavigation,
} from 'react-tv-space-navigation';

/**
 * Inner body: only rendered while open, so its lock effect runs exactly for the
 * open lifetime. Must live under the PARENT navigator (that's whose lock we grab)
 * but wraps its children in a fresh, active root of its own.
 */
function TrapBody({ children }: { children: React.ReactNode }): React.JSX.Element {
  const { lock, unlock } = useLockSpatialNavigation();

  useEffect(() => {
    // Freeze the background navigator for the whole open lifetime.
    lock();
    return () => unlock();
  }, [lock, unlock]);

  return (
    <SpatialNavigationRoot isActive>
      <DefaultFocus>{children}</DefaultFocus>
    </SpatialNavigationRoot>
  );
}

export interface FocusTrapProps {
  /** When `true`, focus is trapped inside `children` and the background is locked. */
  isOpen: boolean;
  children: React.ReactNode;
}

export function FocusTrap({ isOpen, children }: FocusTrapProps): React.JSX.Element | null {
  if (!isOpen) return null;
  // Keyed so re-opening remounts the body and re-runs the lock/DefaultFocus.
  return <TrapBody>{children}</TrapBody>;
}
