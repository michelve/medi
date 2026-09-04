/**
 * `CategoryRow` (Task 91) — one horizontally-scrolling poster row on the landing page.
 *
 * Presentational: given a row's title, its poster tiles, and an optional "See all →"
 * target, it renders a heading and a horizontal strip of the existing `PosterCard`s (so
 * the tiles match the grid exactly). The strip scrolls on its own x-axis; the page body
 * never scrolls sideways. A row with no items renders nothing.
 */

import { Link } from 'react-router-dom';
import { genrePath, type CategoryRow as CategoryRowData } from '@medi/api-client';
import { useApi } from '../api';
import { theme, detail } from '../theme';
import { PosterCard } from './PosterCard';
import { SectionHeading } from './SectionHeading';
import { HScroll } from './HScroll';

export interface CategoryRowProps {
  row: CategoryRowData;
  /**
   * Hide each tile's title/year caption. The detail page's collection and recommendation
   * rows (Figma) show bare posters; the landing page keeps captions.
   */
  captionless?: boolean;
}

export function CategoryRow({ row, captionless = false }: CategoryRowProps) {
  const api = useApi();
  if (row.items.length === 0) return null;

  // Only genre rows have a "See all →" destination; "Recently Added" has none. A genre row's
  // `title` is the genre name, so the pretty `/genre/:slug` URL is derived from it.
  const seeAll = row.genre_id != null ? genrePath(row.title) : undefined;

  // The detail-page rows (captionless) follow the Figma comp — 216px tiles, 32px apart,
  // under a 24px-gap 24px heading. The landing-page rows keep their original, denser tiles.
  const tileWidth = captionless ? detail.posterTile.width : theme.poster.width * 0.7;
  const tileGap = captionless ? detail.posterTile.gap : theme.poster.gap;

  return (
    <section style={captionless ? undefined : { marginBottom: 28 }}>
      <div
        style={{
          display: 'flex',
          alignItems: 'baseline',
          justifyContent: 'space-between',
          marginBottom: captionless ? detail.headingGap : 12,
        }}
      >
        {captionless ? (
          <SectionHeading>{row.title}</SectionHeading>
        ) : (
          <h2 style={{ fontSize: 18, margin: 0, color: theme.colors.text }}>{row.title}</h2>
        )}
        {seeAll && (
          <Link
            to={seeAll}
            style={{
              fontSize: 14,
              color: theme.colors.accent,
              textDecoration: 'none',
              whiteSpace: 'nowrap',
            }}
          >
            See all →
          </Link>
        )}
      </div>
      {/* The row's own horizontal scroll container: wide content scrolls here, the page
          body stays put. */}
      <HScroll gap={tileGap}>
        {row.items.map((item) => (
          <div key={`${item.kind}-${item.id}`} style={{ flex: '0 0 auto', width: tileWidth }}>
            <PosterCard
              item={item}
              posterUrl={api.imageUrl(item.poster)}
              showCaption={!captionless}
            />
          </div>
        ))}
      </HScroll>
    </section>
  );
}
