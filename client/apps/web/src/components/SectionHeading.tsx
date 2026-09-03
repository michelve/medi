/**
 * `SectionHeading` (Figma "Movie Details" restyle) — the shared heading above each content
 * section on a detail page (Part of this collection / You might also like / Cast & crew /
 * Trailers and extras).
 *
 * One place for the design's section type scale: 24px Inter Medium in solid white, with the
 * 24px gap to its content owned by the parent section's layout. Keeping it centralized means
 * every poster row, the cast strip and the About block share exactly the same heading.
 */

import type { ReactNode } from 'react';
import { detail } from '../theme';

export function SectionHeading({
  children,
  as: Tag = 'h2',
}: {
  children: ReactNode;
  /** Element to render — `h2` for a page section, `h3`/`p` where nesting requires. */
  as?: 'h2' | 'h3';
}) {
  return (
    <Tag
      style={{
        margin: 0,
        fontSize: detail.sectionHeading.fontSize,
        fontWeight: detail.sectionHeading.fontWeight,
        lineHeight: detail.sectionHeading.lineHeight,
        color: detail.text.primary,
      }}
    >
      {children}
    </Tag>
  );
}
