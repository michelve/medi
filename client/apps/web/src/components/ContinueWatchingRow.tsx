/**
 * `ContinueWatchingRow` (Task 98) — the landing page's real "Continue Watching" strip.
 *
 * Unlike a `CategoryRow`, each card links straight to `/play/:file_id` (which then resumes
 * from the saved position) rather than to a detail page, and carries a thin progress bar so
 * you can see how far in you are. It fetches `GET /api/continue-watching` itself and renders
 * nothing when there's nothing in progress — so the landing page shows it only when it's real.
 *
 * Poster framing matches `PosterCard` (same 2:3 tile from the shared `theme`); the strip
 * scrolls on its own x-axis via `HScroll`, like the other rows.
 */

import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import type { ContinueWatchingItem } from '@medi/api-client';
import { useApi } from '../api';
import { theme } from '../theme';
import { HScroll } from './HScroll';

export function ContinueWatchingRow() {
  const api = useApi();
  const [items, setItems] = useState<ContinueWatchingItem[]>([]);

  useEffect(() => {
    const controller = new AbortController();
    api
      .continueWatching(undefined, { signal: controller.signal })
      .then((rows) => {
        if (!controller.signal.aborted) setItems(rows);
      })
      .catch(() => {
        // Non-fatal — just don't show the row.
      });
    return () => controller.abort();
  }, [api]);

  // Nothing in progress → render nothing (the row is real data, not a fixed teaser).
  if (items.length === 0) return null;

  const tileWidth = theme.poster.width * 0.7;

  return (
    <section style={{ marginBottom: 28 }}>
      <h2 style={{ fontSize: 18, margin: '0 0 12px', color: theme.colors.text }}>Continue Watching</h2>
      <HScroll gap={theme.poster.gap}>
        {items.map((item) => (
          <div key={item.file_id} style={{ flex: '0 0 auto', width: tileWidth }}>
            <ContinueCard item={item} posterUrl={api.imageUrl(item.poster)} />
          </div>
        ))}
      </HScroll>
    </section>
  );
}

function ContinueCard({ item, posterUrl }: { item: ContinueWatchingItem; posterUrl?: string }) {
  // Fraction watched, for the progress bar. Guard a missing/zero duration → no bar fill.
  const pct =
    item.duration_ms > 0
      ? Math.min(100, Math.max(0, (item.position_ms / item.duration_ms) * 100))
      : 0;

  return (
    <Link
      to={`/play/${item.file_id}`}
      state={{ title: item.title }}
      style={{ textDecoration: 'none', color: 'inherit', display: 'block' }}
    >
      <div className="medi-poster-card">
        <div
          style={{
            position: 'relative',
            aspectRatio: `${theme.poster.width} / ${theme.poster.height}`,
            borderRadius: theme.poster.radius,
            overflow: 'hidden',
            background: theme.colors.surface,
          }}
        >
          {posterUrl ? (
            <img
              src={posterUrl}
              alt=""
              loading="lazy"
              style={{ width: '100%', height: '100%', objectFit: 'cover', display: 'block' }}
            />
          ) : (
            <div
              style={{
                position: 'absolute',
                inset: 0,
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                padding: 12,
                textAlign: 'center',
                color: theme.colors.textMuted,
                fontSize: 14,
                lineHeight: 1.3,
              }}
            >
              {item.title}
            </div>
          )}

          {/* Progress bar pinned to the bottom edge of the poster. */}
          <div
            style={{
              position: 'absolute',
              left: 0,
              right: 0,
              bottom: 0,
              height: 4,
              background: 'rgba(0,0,0,0.45)',
            }}
          >
            <div style={{ width: `${pct}%`, height: '100%', background: theme.colors.accent }} />
          </div>
        </div>
        <div style={{ marginTop: 8 }}>
          <div
            style={{
              fontSize: 14,
              fontWeight: 600,
              color: theme.colors.text,
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              whiteSpace: 'nowrap',
            }}
            title={item.title}
          >
            {item.title}
          </div>
        </div>
      </div>
    </Link>
  );
}
