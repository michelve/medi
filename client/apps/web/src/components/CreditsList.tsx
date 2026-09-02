/**
 * `CreditsList` (Task 81) — the billing block on a detail page.
 *
 * Renders a `Credit[]` (joined `credits` + `people`) in billing order (`ord`, nulls
 * last), showing each person's name and, when present, their character or role. Renders
 * nothing when there are no credits.
 */

import type { Credit } from '@medi/api-client';
import { theme } from '../theme';

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
      <h2 style={{ fontSize: 18, margin: '0 0 12px' }}>Cast &amp; Crew</h2>
      <ul
        style={{
          listStyle: 'none',
          padding: 0,
          margin: 0,
          display: 'grid',
          gridTemplateColumns: 'repeat(auto-fill, minmax(200px, 1fr))',
          gap: 8,
        }}
      >
        {ordered.map((credit) => {
          const detail = credit.character ?? credit.role;
          return (
            <li key={credit.id} style={{ fontSize: 14, lineHeight: 1.4 }}>
              <span style={{ color: theme.colors.text }}>{credit.person_name}</span>
              {detail && <span style={{ color: theme.colors.textMuted }}> — {detail}</span>}
            </li>
          );
        })}
      </ul>
    </section>
  );
}
