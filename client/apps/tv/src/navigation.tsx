/**
 * A tiny stack navigator built on React state.
 *
 * The app only has three routes (Home → Detail → Player), so a full navigation
 * library is overkill and would fight the spatial-navigation `Page isActive`
 * gating. This keeps a route stack and hands each screen an `isActive` flag =
 * "am I the top of the stack", which each screen forwards to its `<Page>` so only
 * the visible screen owns D-pad focus (react-tv-space-navigation requirement).
 *
 * The hardware "menu"/back button pops the stack (wired in `App.tsx`).
 */

import React, { createContext, useCallback, useContext, useMemo, useState } from 'react';

export type Route =
  | { name: 'Home' }
  | { name: 'Detail'; kind: 'movie' | 'series'; id: number }
  | { name: 'Player'; fileId: number; title: string };

interface Navigation {
  stack: Route[];
  current: Route;
  push: (route: Route) => void;
  pop: () => void;
  reset: (route: Route) => void;
  /** True when the stack has something to pop (back is meaningful). */
  canGoBack: boolean;
}

const NavContext = createContext<Navigation | null>(null);

export function NavigationProvider({
  children,
  initial = { name: 'Home' },
}: {
  children: React.ReactNode;
  initial?: Route;
}): React.JSX.Element {
  const [stack, setStack] = useState<Route[]>([initial]);

  const push = useCallback((route: Route) => setStack((s) => [...s, route]), []);
  const pop = useCallback(
    () => setStack((s) => (s.length > 1 ? s.slice(0, -1) : s)),
    [],
  );
  const reset = useCallback((route: Route) => setStack([route]), []);

  const value = useMemo<Navigation>(() => {
    const current = stack[stack.length - 1] ?? initial;
    return { stack, current, push, pop, reset, canGoBack: stack.length > 1 };
    // `initial` is stable enough for a fallback.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stack, push, pop, reset]);

  return <NavContext.Provider value={value}>{children}</NavContext.Provider>;
}

export function useNavigation(): Navigation {
  const nav = useContext(NavContext);
  if (!nav) throw new Error('useNavigation must be used within <NavigationProvider>.');
  return nav;
}
