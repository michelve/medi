/**
 * `TrailerSection` (Task 91, Apple TV-style refresh) — a movie's YouTube trailers as a
 * horizontal row of cards.
 *
 * Each trailer is a 16:9 thumbnail card with a bottom overlay: the trailer name, a ▶ + its
 * kind (Trailer / Teaser / Clip), and a `···` affordance. Clicking a card opens that trailer
 * in a modal `youtube-nocookie` embed (so YouTube isn't loaded until the user asks). The
 * heading carries a `›` chevron to echo the browse rows. Renders nothing with no trailers.
 */

import { useState } from 'react';
import type { Trailer } from '@medi/api-client';
import { theme, detail } from '../theme';
import { HScroll } from './HScroll';

export function TrailerSection({ trailers }: { trailers: Trailer[] }) {
  const [activeKey, setActiveKey] = useState<string | null>(null);

  if (trailers.length === 0) return null;

  const active = trailers.find((t) => t.youtube_key === activeKey);

  return (
    <section>
      {/* Figma: "Trailers and extras" — 18px Inter Medium white, 16px above the row. */}
      <h2
        style={{
          fontSize: 18,
          fontWeight: 500,
          lineHeight: '21px',
          margin: '0 0 16px',
          color: detail.text.primary,
        }}
      >
        Trailers and extras
      </h2>

      <HScroll gap={16}>
        {trailers.map((t) => (
          <TrailerCard key={t.id} trailer={t} onOpen={() => setActiveKey(t.youtube_key)} />
        ))}
      </HScroll>

      {active && (
        <TrailerModal trailer={active} onClose={() => setActiveKey(null)} />
      )}
    </section>
  );
}

function TrailerCard({ trailer, onOpen }: { trailer: Trailer; onOpen: () => void }) {
  const key = trailer.youtube_key;
  // Figma: the first card reads "Play Trailer"; a teaser/clip label falls out to "Play".
  const isTrailer = (trailer.kind || 'Trailer').toLowerCase() === 'trailer';
  return (
    <button
      type="button"
      onClick={onOpen}
      aria-label={`Play ${trailer.name ?? 'trailer'}`}
      style={{
        flex: '0 0 auto',
        // Figma trailer card: 275×149, 16px radius.
        width: 275,
        height: 149,
        maxWidth: '80vw',
        border: 0,
        padding: 0,
        cursor: 'pointer',
        borderRadius: 16,
        overflow: 'hidden',
        position: 'relative',
        background: `#000 url(https://i.ytimg.com/vi/${key}/hqdefault.jpg) center/cover no-repeat`,
        color: '#fff',
        textAlign: 'left',
      }}
    >
      {/* Diagonal bottom-left scrim (Figma) so the play label reads over any thumbnail. */}
      <div
        style={{
          position: 'absolute',
          inset: 0,
          background: 'linear-gradient(215deg, rgba(0,0,0,0) 40%, rgba(0,0,0,1) 81%)',
        }}
      />
      {/* Bottom-left play affordance: ▶ + "Play Trailer" / "Play". */}
      <div
        style={{
          position: 'absolute',
          left: 13,
          bottom: 12,
          display: 'flex',
          alignItems: 'center',
          gap: 7,
        }}
      >
        <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
          <path d="M8 5v14l11-7z" />
        </svg>
        <span style={{ fontSize: 16, fontWeight: 400, lineHeight: '24px' }}>
          {isTrailer ? 'Play Trailer' : 'Play'}
        </span>
      </div>
    </button>
  );
}

function TrailerModal({ trailer, onClose }: { trailer: Trailer; onClose: () => void }) {
  return (
    <div
      role="dialog"
      aria-modal="true"
      onClick={onClose}
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 1000,
        background: 'rgba(0,0,0,0.8)',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        padding: 24,
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          width: 'min(1000px, 100%)',
          aspectRatio: '16 / 9',
          borderRadius: theme.poster.radius,
          overflow: 'hidden',
          background: '#000',
          position: 'relative',
        }}
      >
        <button
          type="button"
          onClick={onClose}
          aria-label="Close trailer"
          style={{
            position: 'absolute',
            top: 8,
            right: 8,
            zIndex: 1,
            width: 36,
            height: 36,
            borderRadius: '50%',
            border: 0,
            background: 'rgba(0,0,0,0.6)',
            color: '#fff',
            fontSize: 20,
            lineHeight: 1,
            cursor: 'pointer',
          }}
        >
          ×
        </button>
        <iframe
          title={trailer.name ?? 'Trailer'}
          src={`https://www.youtube-nocookie.com/embed/${trailer.youtube_key}?autoplay=1&rel=0`}
          allow="accelerometer; autoplay; encrypted-media; picture-in-picture"
          allowFullScreen
          style={{ width: '100%', height: '100%', border: 0, display: 'block' }}
        />
      </div>
    </div>
  );
}
