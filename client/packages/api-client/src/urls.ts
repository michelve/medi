/**
 * Client-side URL helpers for the SPA's pretty routes. The mapping from a title/genre to its
 * path lives here (not scattered across components) so a card, a row, and a detail chip all
 * build the same URL — and so the slug rule stays in lockstep with the backend's `genre_slug`
 * (`backend/crates/db/src/queries.rs`), which resolves these same slugs server-side.
 */

import type { LibraryItem, LibraryKind } from './types';

/**
 * URL-safe slug for a genre name, e.g. `"Science Fiction"` → `"science-fiction"`,
 * `"Action & Adventure"` → `"action-adventure"`. Must match the backend's `genre_slug`
 * byte-for-byte: lowercase ASCII, every run of non-alphanumerics collapses to one `-`, and
 * leading/trailing dashes are trimmed. Genres have no stored slug — both ends derive it from
 * the name, so a rename can't strand a stale slug.
 */
export function genreSlug(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
}

/**
 * The detail-page path for a poster tile: `/movie/:tmdbId` or `/series/:tmdbId` when the title
 * is matched, falling back to the internal `id` for an unmatched title. The backend's
 * `movie_detail` / `series_detail` handlers resolve a TMDB id first, then the internal id, so
 * either form loads.
 */
export function titlePath(item: Pick<LibraryItem, 'kind' | 'id' | 'tmdb_id'>): string {
  return `/${item.kind}/${item.tmdb_id ?? item.id}`;
}

/** The grid path for a genre, keyed by its name slug (`/genre/adventure`). */
export function genrePath(name: string): string {
  return `/genre/${genreSlug(name)}`;
}

export type { LibraryKind };
