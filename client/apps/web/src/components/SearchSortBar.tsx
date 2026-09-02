/**
 * `SearchSortBar` (Task 81) — the header's search box + sort toggle.
 *
 * Reads/writes the shared `BrowseState`: the search box holds local input and pushes a
 * debounced value into `setQuery` (the grid filters loaded items client-side); the sort
 * control flips `LibraryQuery.sort` between `sort_title` and `added_at`, which the page
 * treats as a from-page-one re-fetch (cursor reset).
 *
 * NOTE: server-side text search (`GET /api/search?q=`, spanning the whole catalog) does
 * not exist yet — this box only narrows the already-loaded grid. Follow-up flagged in
 * `docs/.tasks/81-web-ui-browse.md`.
 */

import { useEffect, useState } from 'react';
import type { LibrarySort } from '@medi/api-client';
import { theme } from '../theme';
import { useBrowseState } from '../lib/browseState';
import { useDebouncedValue } from '../lib/useDebouncedValue';

const SORT_OPTIONS: { value: LibrarySort; label: string }[] = [
  { value: 'sort_title', label: 'A–Z' },
  { value: 'added_at', label: 'Recently added' },
];

export function SearchSortBar() {
  const { setQuery, sort, setSort } = useBrowseState();
  const [input, setInput] = useState('');
  const debounced = useDebouncedValue(input, 200);

  // Push the debounced value into shared state (the grid reads `query`).
  useEffect(() => {
    setQuery(debounced.trim());
  }, [debounced, setQuery]);

  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 12, flexWrap: 'wrap' }}>
      <input
        className="medi-search-input"
        type="search"
        value={input}
        onChange={(e) => setInput(e.target.value)}
        placeholder="Search library…"
        aria-label="Search library"
        style={{
          flex: '1 1 220px',
          minWidth: 160,
          maxWidth: 360,
          padding: '8px 12px',
          borderRadius: 8,
          border: `1px solid ${theme.colors.surface}`,
          background: theme.colors.surface,
          color: theme.colors.text,
          fontSize: 14,
          outline: 'none',
        }}
      />
      <div
        role="group"
        aria-label="Sort order"
        style={{
          display: 'inline-flex',
          borderRadius: 8,
          overflow: 'hidden',
          border: `1px solid ${theme.colors.surface}`,
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
                padding: '8px 14px',
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
    </div>
  );
}
