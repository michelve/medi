/**
 * `SuggestedRow` (Task 91 detail extensions) — "You might also like".
 *
 * Pools the in-library filmographies of a movie's top-billed actors **and its director(s)**
 * into one deduped poster row, excluding the movie itself — so it catches both "more with
 * this actor" and "from the director of…" matches. Reuses the person filmography endpoint
 * (`GET /api/people/:id`) — a handful of small fetches, merged. Renders nothing while loading
 * or when the pool is empty (a niche title whose people have nothing else in the library).
 */

import { useEffect, useState } from 'react';
import type { Credit, LibraryItem } from '@medi/api-client';
import { useApi } from '../api';
import { CategoryRow } from './CategoryRow';

/** How many top-billed actors to pool suggestions from (directors are always included). */
const ACTOR_COUNT = 3;
/** Cap the merged row so a prolific cast doesn't render dozens of tiles. */
const MAX_ITEMS = 10;

export function SuggestedRow({
  credits,
  excludeKind,
  excludeId,
  captionless = false,
}: {
  credits: Credit[];
  excludeKind: 'movie' | 'series';
  excludeId: number;
  /** Hide the poster title/year captions (detail-page rows, per the Figma comp). */
  captionless?: boolean;
}) {
  const api = useApi();
  const [items, setItems] = useState<LibraryItem[]>([]);

  // The first few billed actors (credits are in billing order) plus every director, deduped
  // (a director who also acts appears once).
  const actorIds = credits.filter((c) => c.role === 'actor').slice(0, ACTOR_COUNT).map((c) => c.person_id);
  const directorIds = credits.filter((c) => c.role === 'director').map((c) => c.person_id);
  const personIds = [...new Set([...actorIds, ...directorIds])];
  const personKey = personIds.join(',');

  useEffect(() => {
    if (personIds.length === 0) {
      setItems([]);
      return;
    }
    const controller = new AbortController();
    (async () => {
      try {
        const pages = await Promise.all(
          personIds.map((id) =>
            api.person(id, { signal: controller.signal }).then((p) => p.filmography, () => []),
          ),
        );
        if (controller.signal.aborted) return;
        // Merge, dedupe by kind+id, drop the current title, cap.
        const seen = new Set<string>([`${excludeKind}-${excludeId}`]);
        const merged: LibraryItem[] = [];
        for (const film of pages.flat()) {
          const k = `${film.kind}-${film.id}`;
          if (seen.has(k)) continue;
          seen.add(k);
          merged.push(film);
          if (merged.length >= MAX_ITEMS) break;
        }
        setItems(merged);
      } catch {
        if (!controller.signal.aborted) setItems([]);
      }
    })();
    return () => controller.abort();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [personKey, excludeKind, excludeId]);

  if (items.length === 0) return null;

  return (
    <CategoryRow
      captionless={captionless}
      row={{ key: 'suggested', title: 'You might also like', items }}
    />
  );
}
