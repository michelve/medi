/**
 * `LibraryPage` (Task 81) — the `/` poster wall.
 *
 * Thin wiring: `useLibraryPaging` (keyset infinite scroll over `GET /api/library`) plus
 * the shared `BrowseState` (search query + sort). Sort changes re-fetch from page one via
 * the hook; the search box filters the already-loaded items client-side (case-insensitive
 * title match) — the server has no text-search endpoint (see the task's follow-up note).
 */

import { useMemo } from 'react';
import { useBrowseState } from '../lib/browseState';
import { useLibraryPaging } from '../lib/useLibraryPaging';
import { PosterGrid } from '../components/PosterGrid';
import { Loading, ErrorState, EmptyState } from '../components/Status';

export function LibraryPage() {
  const { query, sort } = useBrowseState();
  const { items, initialLoading, hasMore, error, loadMore } = useLibraryPaging(sort);

  // Client-side filter over the loaded page(s). Whole-catalog search is a follow-up.
  const visible = useMemo(() => {
    const q = query.toLowerCase();
    if (!q) return items;
    return items.filter((item) => item.title.toLowerCase().includes(q));
  }, [items, query]);

  if (initialLoading && items.length === 0) return <Loading label="Loading library…" />;
  if (error && items.length === 0) return <ErrorState message={error} />;

  return (
    <section>
      {query && visible.length === 0 ? (
        <EmptyState>No loaded titles match “{query}”.</EmptyState>
      ) : (
        <PosterGrid
          items={visible}
          // Don't page while a search filter is active — it narrows loaded items only.
          hasMore={hasMore && !query}
          onReachEnd={loadMore}
        />
      )}
    </section>
  );
}
