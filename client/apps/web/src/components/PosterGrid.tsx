/**
 * `PosterGrid` (Task 81) — a responsive CSS-grid poster wall.
 *
 * Presentational: it renders the cards it's given and an end-of-grid sentinel that
 * an `IntersectionObserver` watches, calling `onReachEnd` when it scrolls into view.
 * The owning page (`LibraryPage`) holds the paging state and decides what to do —
 * this component knows nothing about cursors or fetches (only whether more may exist,
 * via `hasMore`).
 *
 * Card min-width and gap come from `@medi/ui` `theme.ts` so the web wall matches the
 * TV app's poster sizing; `auto-fill` makes the column count responsive.
 */

import { useEffect, useRef } from 'react';
import type { LibraryItem } from '@medi/api-client';
import { useApi } from '../api';
import { theme } from '../theme';
import { PosterCard } from './PosterCard';

export interface PosterGridProps {
  items: LibraryItem[];
  /** More pages may exist (a non-null cursor); enables the sentinel. */
  hasMore: boolean;
  /** Invoked when the sentinel enters the viewport (request the next page). */
  onReachEnd: () => void;
}

export function PosterGrid({ items, hasMore, onReachEnd }: PosterGridProps) {
  const api = useApi();
  const sentinelRef = useRef<HTMLDivElement | null>(null);
  // Keep the latest callback without re-subscribing the observer each render.
  const onReachEndRef = useRef(onReachEnd);
  onReachEndRef.current = onReachEnd;

  useEffect(() => {
    const node = sentinelRef.current;
    if (!node || !hasMore) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) onReachEndRef.current();
      },
      // Prefetch a screen early so scrolling stays smooth.
      { rootMargin: '600px 0px' },
    );
    observer.observe(node);
    return () => observer.disconnect();
  }, [hasMore]);

  return (
    <div>
      <div
        style={{
          display: 'grid',
          gridTemplateColumns: `repeat(auto-fill, minmax(${theme.poster.width * 0.7}px, 1fr))`,
          gap: theme.poster.gap,
        }}
      >
        {items.map((item) => (
          <PosterCard
            key={`${item.kind}-${item.id}`}
            item={item}
            posterUrl={api.imageUrl(item.poster)}
          />
        ))}
      </div>
      {/* Sentinel: an IntersectionObserver target the page uses to fetch the next page. */}
      {hasMore && <div ref={sentinelRef} style={{ height: 1 }} aria-hidden />}
    </div>
  );
}
