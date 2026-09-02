/**
 * Browse UI state (Task 81) — the small shared state the header controls
 * (`SearchSortBar`, in `App`) and the grid (`LibraryPage`, in the `<Outlet/>`) both need.
 *
 * The search query and sort key live above both so the header can drive a page it doesn't
 * render. Kept as a tiny dedicated context (not a global store) so it's obvious what's
 * shared and easy to replace later; pages elsewhere in the app simply ignore it.
 */

import {
  createContext,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from 'react';
import type { LibrarySort } from '@medi/api-client';

/** Default catalog ordering when the user hasn't toggled sort. */
export const DEFAULT_SORT: LibrarySort = 'sort_title';

export interface BrowseState {
  /** Debounced, case-insensitive title filter over the loaded grid. */
  query: string;
  setQuery: (q: string) => void;
  /** Server-side sort key; changing it re-fetches from page one. */
  sort: LibrarySort;
  setSort: (s: LibrarySort) => void;
}

const BrowseContext = createContext<BrowseState | null>(null);

export function BrowseProvider({ children }: { children: ReactNode }) {
  const [query, setQuery] = useState('');
  const [sort, setSort] = useState<LibrarySort>(DEFAULT_SORT);
  const value = useMemo<BrowseState>(
    () => ({ query, setQuery, sort, setSort }),
    [query, sort],
  );
  return <BrowseContext.Provider value={value}>{children}</BrowseContext.Provider>;
}

export function useBrowseState(): BrowseState {
  const ctx = useContext(BrowseContext);
  if (!ctx) throw new Error('useBrowseState must be used within <BrowseProvider>.');
  return ctx;
}
