/**
 * Directional overrides — force focus to jump to a designer-chosen target on a
 * given D-pad direction, ignoring geometry (README §Spatial Navigation →
 * Directional Overrides).
 *
 * Canonical case: pressing **Down** on the hero banner must land on the first
 * item of the "Continue Watching" carousel, even though nothing is geometrically
 * below the hero.
 *
 * Implementation: a small imperative registry of named focus targets. A target
 * registers an imperative `focus()` (via `useFocusTarget` on a node's `ref`); an
 * override source wraps a `SpatialNavigationNode` and, on
 * `onDirectionHandledWithoutMovement` (fired when a press can't move within the
 * subtree), looks up the mapped target and focuses it. This keeps the geometry
 * engine intact for the common case and only overrides the specific edges the
 * designer names.
 */

import React, { useCallback, useMemo, useRef } from 'react';
import {
  SpatialNavigationRoot,
  type SpatialNavigationNodeRef,
} from 'react-tv-space-navigation';

type Direction = 'up' | 'down' | 'left' | 'right';

/** A registrable focus target: a stable name → an imperative focus fn. */
interface Registry {
  register(name: string, focus: () => void): () => void;
  focusByName(name: string): boolean;
}

const FocusRegistryContext = React.createContext<Registry | null>(null);

/**
 * Provider holding the app-wide map of named focus targets. Mount once high in
 * the tree (e.g. inside the Home screen) so hero and carousels share it.
 */
export function FocusTargetProvider({
  children,
}: {
  children: React.ReactNode;
}): React.JSX.Element {
  const targets = useRef(new Map<string, () => void>());

  const registry = useMemo<Registry>(
    () => ({
      register(name, focus) {
        targets.current.set(name, focus);
        return () => {
          // Only delete if we still own this name (avoid clobbering a remount).
          if (targets.current.get(name) === focus) targets.current.delete(name);
        };
      },
      focusByName(name) {
        const focus = targets.current.get(name);
        if (focus) {
          focus();
          return true;
        }
        return false;
      },
    }),
    [],
  );

  return (
    <FocusRegistryContext.Provider value={registry}>
      {children}
    </FocusRegistryContext.Provider>
  );
}

function useFocusRegistry(): Registry {
  const reg = React.useContext(FocusRegistryContext);
  if (!reg) {
    throw new Error(
      'useFocusTarget/DirectionalOverride must be used within a <FocusTargetProvider>.',
    );
  }
  return reg;
}

/**
 * Register a spatial node as a named focus target. Returns a ref to attach to a
 * `SpatialNavigationNode`/`SpatialNavigationFocusableView`; the node's imperative
 * `focus()` becomes reachable by an override that names it.
 */
export function useFocusTarget(name: string): React.RefObject<SpatialNavigationNodeRef | null> {
  const registry = useFocusRegistry();
  const ref = useRef<SpatialNavigationNodeRef | null>(null);

  React.useEffect(() => {
    return registry.register(name, () => ref.current?.focus());
  }, [registry, name]);

  return ref;
}

/**
 * Register an arbitrary imperative focus function under `name`. Use this when the
 * thing you want to focus isn't a bare node — e.g. a virtualized list whose ref
 * exposes `focus(index)` and you want the override to land on its first item. Pass
 * `undefined` to register nothing (convenient for conditional targets).
 */
export function useRegisterFocusTarget(
  name: string | undefined,
  focus: () => void,
): void {
  const registry = useFocusRegistry();
  // Keep the latest closure without re-registering every render.
  const focusRef = useRef(focus);
  focusRef.current = focus;

  React.useEffect(() => {
    if (!name) return;
    return registry.register(name, () => focusRef.current());
  }, [registry, name]);
}

export interface DirectionalOverrideProps {
  /**
   * Map of edge direction → target name. When focus is on this subtree and the
   * user presses a mapped direction that would otherwise do nothing (edge), we
   * jump to the named target instead.
   */
  to: Partial<Record<Direction, string>>;
  children: React.ReactNode;
}

/**
 * Wrap a component (e.g. the hero banner) so specific directional presses jump to
 * named targets. Only the edges you list are overridden; every other direction
 * falls through to normal geometric navigation.
 *
 * Mechanism: a nested `SpatialNavigationRoot` scoped to `children`. A root is the
 * one place the library reports an unhandled directional press
 * (`onDirectionHandledWithoutMovement`) — i.e. focus is at the subtree's edge in
 * that direction. When that direction is one we map, we imperatively focus the
 * named target instead of letting geometry decide. Nested roots are supported and
 * hand focus back to the parent for any direction we don't override.
 */
export function DirectionalOverride({
  to,
  children,
}: DirectionalOverrideProps): React.JSX.Element {
  const registry = useFocusRegistry();

  const handleEdge = useCallback(
    (direction: Direction): boolean => {
      const targetName = to[direction];
      if (targetName) return registry.focusByName(targetName);
      return false;
    },
    [registry, to],
  );

  return (
    <SpatialNavigationRoot
      isActive
      onDirectionHandledWithoutMovement={(d: string) => {
        handleEdge(d as Direction);
      }}
    >
      {children}
    </SpatialNavigationRoot>
  );
}

/**
 * Imperatively focus a registered target by name. Handy after closing a modal to
 * restore focus (e.g. back to `"hero"`). Returns `true` if the target existed.
 */
export function useFocusByName(): (name: string) => boolean {
  const registry = useFocusRegistry();
  return useCallback((name: string) => registry.focusByName(name), [registry]);
}
