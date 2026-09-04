/**
 * `LibraryPage` (Task 81 + 91) — the `/` landing view.
 *
 * Two modes:
 *  - **Landing** (default, no search): the curated `CategoryRow`s from `GET /api/library/rows`
 *    ("Recently Added" + top genres) as a Netflix-style teaser, **followed by the full
 *    paginated `PosterGrid` of the whole catalog** so every title is browsable in one place
 *    (the rows alone are capped teasers — without the grid a large library looks truncated).
 *  - **Flat grid** (Task 81): a search query (or a non-default sort) drops the rows and shows
 *    just the paginated `PosterGrid` over `GET /api/library`. Sort re-fetches from page one via
 *    the hook; the search box filters the loaded items client-side.
 */

import { useMemo } from 'react';
import type { LibraryRows, LibrarySort } from '@medi/api-client';
import { useApi } from '../api';
import { useBrowseState, DEFAULT_SORT } from '../lib/browseState';
import { useDetail } from '../lib/useDetail';
import { useLibraryPaging } from '../lib/useLibraryPaging';
import { PosterGrid } from '../components/PosterGrid';
import { CategoryRow } from '../components/CategoryRow';
import { ContinueWatchingRow } from '../components/ContinueWatchingRow';
import { Loading, ErrorState, EmptyState } from '../components/Status';
import { theme } from '../theme';

const SORT_OPTIONS: { value: LibrarySort; label: string }[] = [
  { value: 'sort_title', label: 'A–Z' },
  { value: 'added_at', label: 'Recently added' },
];

/**
 * The library's sort toggle. Lived in the global header before the nav redesign; now it sits
 * on the library page itself, where the ordering it controls is relevant.
 */
function SortToggle() {
  const { sort, setSort } = useBrowseState();
  return (
    <div
      role="group"
      aria-label="Sort order"
      style={{
        display: 'inline-flex',
        borderRadius: 999,
        overflow: 'hidden',
        border: `1px solid ${theme.colors.surface}`,
        marginBottom: 24,
      }}
    >
      {SORT_OPTIONS.map((opt) => {
        const active = opt.value === sort;
        return (
          <button
            key={opt.value}
            type="button"
            onClick={() => setSort(opt.value)}
            aria-pressed={active}
            style={{
              padding: '8px 16px',
              border: 'none',
              fontSize: 13,
              fontWeight: 600,
              cursor: 'pointer',
              color: active ? '#ffffff' : theme.colors.textMuted,
              background: active ? theme.colors.accent : 'transparent',
            }}
          >
            {opt.label}
          </button>
        );
      })}
    </div>
  );
}

export function LibraryPage() {
  const { query, sort } = useBrowseState();
  // The landing view (curated rows + a full grid below) shows only when there's no active
  // search and the default ordering; a search or a non-default sort collapses to the flat grid.
  const showLanding = query.trim() === '' && sort === DEFAULT_SORT;

  return (
    <div>
      <SortToggle />
      {showLanding ? (
        <>
          <BrowseRows />
          {/* The full catalog below the teaser rows, so every title is browsable here and a
              large library never looks truncated to the capped rows. Heading separates it from
              the rows above. */}
          <AllTitlesGrid />
        </>
      ) : (
        <FlatGrid />
      )}
    </div>
  );
}

/** The full paginated catalog grid shown beneath the landing rows (all titles, A–Z). */
function AllTitlesGrid() {
  const { items, initialLoading, hasMore, error, loadMore } = useLibraryPaging(DEFAULT_SORT);

  // A heading anchors the grid under the horizontal rows; hidden until the first page lands so
  // it doesn't flash above an empty grid.
  return (
    <section style={{ marginTop: 8 }}>
      <h2 style={{ fontSize: 18, margin: '0 0 16px', color: theme.colors.text }}>All titles</h2>
      {initialLoading && items.length === 0 ? (
        <Loading label="Loading titles…" />
      ) : error && items.length === 0 ? (
        <ErrorState message={error} />
      ) : items.length === 0 ? (
        <EmptyState>No titles yet.</EmptyState>
      ) : (
        <PosterGrid items={items} hasMore={hasMore} onReachEnd={loadMore} />
      )}
    </section>
  );
}

/** The curated category rows for the default landing view (Task 91). */
function BrowseRows() {
  const api = useApi();
  const state = useDetail<LibraryRows>((signal) => api.libraryRows({ signal }), []);

  if (state.status === 'loading') return <Loading label="Loading library…" />;
  if (state.status === 'error') return <ErrorState message={state.message} />;
  if (state.status === 'not_found' || state.data.rows.length === 0) {
    return <EmptyState>Your library is empty. Add a library and drop in some media.</EmptyState>;
  }

  return (
    <section>
      {/* Real "Continue Watching" (Task 98) at the very top — renders nothing when there's
          nothing in progress, so it only appears when it's real data. */}
      <ContinueWatchingRow />
      {state.data.rows.map((row) => (
        <CategoryRow key={row.key} row={row} />
      ))}
    </section>
  );
}

/** The paginated flat grid for search / non-default sort (Task 81 behavior). */
function FlatGrid() {
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
