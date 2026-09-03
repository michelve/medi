/**
 * `GenrePage` (Task 91) — `/genre/:id`.
 *
 * One genre's keyset grid: `useLibraryPaging` pointed at `GET /api/genres/:id` (same
 * `LibraryPage` shape as the main library, so the hook is reused verbatim) feeding the
 * existing `PosterGrid` with infinite scroll. The header shows the genre's name, resolved
 * from the cached `GET /api/genres` list (no dedicated name endpoint). An unknown/invalid
 * id renders the shared `NotFound`.
 */

import { useMemo } from 'react';
import { useParams } from 'react-router-dom';
import type { GenreCount } from '@medi/api-client';
import { useApi } from '../api';
import { useDetail } from '../lib/useDetail';
import { useLibraryPaging } from '../lib/useLibraryPaging';
import { PosterGrid } from '../components/PosterGrid';
import { Loading, ErrorState, EmptyState, NotFound } from '../components/Status';
import { theme } from '../theme';

export function GenrePage() {
  const { id } = useParams<{ id: string }>();
  const api = useApi();
  const genreId = Number(id);

  // The genre name comes from the (cached) genres list — there's no per-genre name route.
  const genresState = useDetail<GenreCount[]>((signal) => api.genres({ signal }), [genreId]);
  const genre = useMemo(
    () => (genresState.status === 'ready' ? genresState.data.find((g) => g.id === genreId) : undefined),
    [genresState, genreId],
  );

  // Default sort for a genre view is alphabetical (matches the library default).
  const paging = useLibraryPaging('sort_title', { kind: 'genre', id: genreId });

  if (!Number.isFinite(genreId)) return <NotFound message="That isn't a valid genre." />;
  // A genre with no titles isn't listed by /api/genres, so an id absent from a loaded list
  // is a real 404 (the grid would be empty anyway).
  if (genresState.status === 'ready' && !genre) {
    return <NotFound message="We couldn't find that genre." />;
  }

  if (paging.initialLoading && paging.items.length === 0) return <Loading label="Loading titles…" />;
  if (paging.error && paging.items.length === 0) return <ErrorState message={paging.error} />;

  return (
    <section>
      <h1 style={{ fontSize: 26, margin: '0 0 20px', color: theme.colors.text }}>
        {genre?.name ?? 'Genre'}
        {genre && (
          <span style={{ fontSize: 16, color: theme.colors.textMuted, fontWeight: 400 }}>
            {' '}· {genre.count} {genre.count === 1 ? 'title' : 'titles'}
          </span>
        )}
      </h1>
      {paging.items.length === 0 ? (
        <EmptyState>No titles in this genre yet.</EmptyState>
      ) : (
        <PosterGrid items={paging.items} hasMore={paging.hasMore} onReachEnd={paging.loadMore} />
      )}
    </section>
  );
}
