/**
 * `PersonPage` (Task 91 Phase B) — `/person/:id`.
 *
 * `client.person(id)` → a headshot (`/api/images/people/<id>/photo.jpg`, a titled
 * placeholder when the person has none), the name, a clamped biography with a "more" toggle,
 * then their in-library filmography as the existing `PosterGrid` (the response is a fixed
 * list, so `hasMore` is false — no paging). A 404 renders the shared `NotFound`.
 */

import { useState } from 'react';
import { useParams } from 'react-router-dom';
import type { PersonPage as PersonPageData } from '@medi/api-client';
import { useApi } from '../api';
import { useDetail } from '../lib/useDetail';
import { PosterGrid } from '../components/PosterGrid';
import { Loading, ErrorState, EmptyState, NotFound } from '../components/Status';
import { theme } from '../theme';

/** How many characters of a bio to show before the "more" toggle appears. */
const BIO_CLAMP = 320;

export function PersonPage() {
  const { id } = useParams<{ id: string }>();
  const api = useApi();
  const personId = Number(id);

  const state = useDetail<PersonPageData>((signal) => api.person(personId, { signal }), [personId]);

  if (!Number.isFinite(personId)) return <NotFound message="That isn't a valid person." />;
  if (state.status === 'loading') return <Loading label="Loading…" />;
  if (state.status === 'not_found') return <NotFound message="We couldn't find that person." />;
  if (state.status === 'error') return <ErrorState message={state.message} />;

  const person = state.data;
  const photoUrl = api.imageUrl(person.photo);

  return (
    <article>
      <header style={{ display: 'flex', gap: 24, marginBottom: 32, flexWrap: 'wrap' }}>
        <div
          style={{
            flex: '0 0 auto',
            width: 180,
            aspectRatio: '2 / 3',
            borderRadius: theme.poster.radius,
            overflow: 'hidden',
            background: theme.colors.surface,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
          }}
        >
          {photoUrl ? (
            <img
              src={photoUrl}
              alt=""
              style={{ width: '100%', height: '100%', objectFit: 'cover', display: 'block' }}
            />
          ) : (
            <span style={{ color: theme.colors.textMuted, fontSize: 14, padding: 12, textAlign: 'center' }}>
              {person.name}
            </span>
          )}
        </div>
        <div style={{ flex: '1 1 320px', minWidth: 0 }}>
          <h1 style={{ fontSize: 28, margin: '0 0 12px', color: theme.colors.text }}>{person.name}</h1>
          <Bio text={person.biography} />
        </div>
      </header>

      <section>
        <h2 style={{ fontSize: 20, margin: '0 0 16px', color: theme.colors.text }}>
          In this library
        </h2>
        {person.filmography.length === 0 ? (
          <EmptyState>No titles from {person.name} in your library yet.</EmptyState>
        ) : (
          // The filmography is a complete list from one response — no further pages.
          <PosterGrid items={person.filmography} hasMore={false} onReachEnd={() => {}} />
        )}
      </section>
    </article>
  );
}

/** A biography clamped to `BIO_CLAMP` chars with a "more/less" toggle. */
function Bio({ text }: { text?: string }) {
  const [expanded, setExpanded] = useState(false);
  if (!text) return null;

  const isLong = text.length > BIO_CLAMP;
  const shown = expanded || !isLong ? text : `${text.slice(0, BIO_CLAMP).trimEnd()}…`;

  return (
    <div style={{ fontSize: 15, lineHeight: 1.6, color: theme.colors.textMuted }}>
      <p style={{ margin: 0, whiteSpace: 'pre-line' }}>{shown}</p>
      {isLong && (
        <button
          type="button"
          onClick={() => setExpanded((v) => !v)}
          style={{
            marginTop: 8,
            background: 'none',
            border: 'none',
            padding: 0,
            color: theme.colors.accent,
            fontSize: 14,
            cursor: 'pointer',
          }}
        >
          {expanded ? 'Show less' : 'Show more'}
        </button>
      )}
    </div>
  );
}
