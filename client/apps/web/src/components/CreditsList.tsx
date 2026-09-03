/**
 * `CreditsList` (Task 81 + 91) — the Cast & Crew block on a detail page.
 *
 * A horizontally-scrolling row of circular headshots (Plex/Spotify style): each credit is a
 * round avatar with the person's name below and their character/role under that. A person
 * with a downloaded headshot (`photo_path`, Task 91 Phase B) shows their photo; one without
 * shows a neutral circle with their initials. The whole avatar links to `/person/:id` — every
 * credit carries a `person_id`. Renders nothing when there are no credits.
 */

import { Link } from 'react-router-dom';
import type { Credit } from '@medi/api-client';
import { useApi } from '../api';
import { theme, detail } from '../theme';
import { SectionHeading } from './SectionHeading';
import { HScroll } from './HScroll';

/** Diameter of an avatar circle, px (Figma: 100). */
const AVATAR = detail.avatar;
/** Fixed cast-member column width (Figma: 108) so names wrap under the 100px avatar. */
const COLUMN = 108;

export function CreditsList({ credits }: { credits: Credit[] }) {
  if (credits.length === 0) return null;

  // Billing order: `ord` ascending, entries without an `ord` sorted to the end.
  const ordered = [...credits].sort((a, b) => {
    if (a.ord == null && b.ord == null) return 0;
    if (a.ord == null) return 1;
    if (b.ord == null) return -1;
    return a.ord - b.ord;
  });

  return (
    <section>
      <div style={{ marginBottom: detail.headingGap }}>
        <SectionHeading>Cast &amp; crew</SectionHeading>
      </div>
      {/* One horizontal strip: the row scrolls sideways, the page never does. Figma spaces
          the cast members 32px apart. */}
      <HScroll gap={32}>
        {ordered.map((credit) => (
          <CreditAvatar key={credit.id} credit={credit} />
        ))}
      </HScroll>
    </section>
  );
}

/** One circular, linked credit: photo-or-initials, name, and character/role. */
function CreditAvatar({ credit }: { credit: Credit }) {
  const api = useApi();
  const photoUrl = api.imageUrl(credit.photo_path);
  const creditDetail = credit.character ?? credit.role;

  return (
    <Link
      to={`/person/${credit.person_id}`}
      className="medi-credit-link"
      style={{
        flex: '0 0 auto',
        // Figma: a fixed 108px column, avatar 16px above a centered two-line label.
        width: COLUMN,
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        gap: 16,
        textDecoration: 'none',
        textAlign: 'center',
      }}
    >
      <div
        style={{
          width: AVATAR,
          height: AVATAR,
          borderRadius: '50%',
          overflow: 'hidden',
          background: theme.colors.surface,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          flex: '0 0 auto',
        }}
      >
        {photoUrl ? (
          <img
            src={photoUrl}
            alt=""
            loading="lazy"
            style={{ width: '100%', height: '100%', objectFit: 'cover', display: 'block' }}
          />
        ) : (
          <span style={{ color: theme.colors.textMuted, fontSize: 28, fontWeight: 600, letterSpacing: 1 }}>
            {initials(credit.person_name)}
          </span>
        )}
      </div>
      <div>
        <div
          style={{
            fontSize: 16,
            fontWeight: 500,
            color: detail.text.primary,
            lineHeight: '21px',
          }}
          title={credit.person_name}
        >
          {credit.person_name}
        </div>
        {creditDetail && (
          <div
            style={{
              fontSize: 14,
              fontWeight: 500,
              color: detail.text.secondary,
              lineHeight: '21px',
            }}
            title={creditDetail}
          >
            {creditDetail}
          </div>
        )}
      </div>
    </Link>
  );
}

/** Up to two initials from a name (first + last word), for the no-photo placeholder. */
function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return '?';
  const first = parts[0]?.charAt(0) ?? '';
  const last = parts.length > 1 ? parts[parts.length - 1]?.charAt(0) ?? '' : '';
  return (first + last).toUpperCase() || '?';
}
