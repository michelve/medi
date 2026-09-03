/**
 * `PosterCard` (Task 81) — one poster tile in the browse grid, as fresh DOM.
 *
 * The DOM counterpart to `@medi/ui`'s react-native `PosterCard`: same 2:3 framing,
 * radius and placeholder behavior pulled from the shared `theme`, but a plain `<img>`
 * and CSS hover instead of the TV app's spatial focus. A missing poster degrades to a
 * titled placeholder tile (Task 81 requirement); the whole card is a router `<Link>`
 * to the movie/series detail page.
 */

import { Link } from 'react-router-dom';
import type { LibraryItem } from '@medi/api-client';
import { theme } from '../theme';
import { HdrBadge } from './HdrBadge';

export interface PosterCardProps {
  item: LibraryItem;
  /** Absolute poster URL from `client.imageUrl(item.poster)`, or undefined. */
  posterUrl?: string;
  /**
   * Show the title/year caption under the poster. The browse grid keeps it (default); the
   * detail page's collection/recommendation rows hide it to match the Figma comp, which
   * shows bare poster art.
   */
  showCaption?: boolean;
}

export function PosterCard({ item, posterUrl, showCaption = true }: PosterCardProps) {
  const to = `/${item.kind}/${item.id}`;

  return (
    <Link to={to} style={{ textDecoration: 'none', color: 'inherit', display: 'block' }}>
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
          {item.hdr && (
            <div style={{ position: 'absolute', top: 8, right: 8 }}>
              <HdrBadge hdr={item.hdr} />
            </div>
          )}
        </div>
        {showCaption && (
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
            {item.year != null && (
              <div style={{ fontSize: 13, color: theme.colors.textMuted }}>{item.year}</div>
            )}
          </div>
        )}
      </div>
    </Link>
  );
}
