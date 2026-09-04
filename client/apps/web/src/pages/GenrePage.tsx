/**
 * `GenrePage` (Task 91) — `/genre/:slug`.
 *
 * One genre's keyset grid: `useLibraryPaging` pointed at `GET /api/genres/:slug` (same
 * `LibraryPage` shape as the main library, so the hook is reused verbatim) feeding the
 * existing `PosterGrid` with infinite scroll. `:slug` is the genre-name slug (`adventure`);
 * the header name is resolved from the cached `GET /api/genres` list by slugging each genre's
 * name (no dedicated name endpoint). An unknown slug renders the shared `NotFound`.
 */

import { useMemo } from 'react';
import { useParams } from 'react-router-dom';
import { genreSlug, type GenreCount } from '@medi/api-client';
import { useApi } from '../api';
import { useDetail } from '../lib/useDetail';
import { useLibraryPaging } from '../lib/useLibraryPaging';
import { PosterGrid } from '../components/PosterGrid';
import { Loading, ErrorState, EmptyState, NotFound } from '../components/Status';
import { theme } from '../theme';

export function GenrePage() {
  const { slug = '' } = useParams<{ slug: string }>();
  const api = useApi();

  // The genre name comes from the (cached) genres list — there's no per-genre name route.
  // Match by slugging each genre's name so the URL slug (`science-fiction`) maps to its genre.
  const genresState = useDetail<GenreCount[]>((signal) => api.genres({ signal }), [slug]);
  const genre = useMemo(
    () =>
      genresState.status === 'ready'
        ? genresState.data.find((g) => genreSlug(g.name) === slug.toLowerCase())
        : undefined,
    [genresState, slug],
  );

  // Default sort for a genre view is alphabetical (matches the library default).
  const paging = useLibraryPaging('sort_title', { kind: 'genre', slug });

  if (!slug) return <NotFound message="That isn't a valid genre." />;
  // A genre with no titles isn't listed by /api/genres, so a slug absent from a loaded list
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
