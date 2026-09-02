/**
 * `DetailHeader` (Task 81) — the shared hero block at the top of a detail page.
 *
 * A backdrop image (dimmed for legibility) with the title, year and overview laid over
 * it — the browse app's echo of the Apple TV-style feature banner. Reused verbatim by
 * both the movie and series pages so their headers stay identical and restyling happens
 * in one place. Degrades to a flat surface when there's no backdrop art.
 */

import type { ReactNode } from 'react';
import { theme } from '../theme';

export interface DetailHeaderProps {
  title: string;
  year?: number | null;
  overview?: string | null;
  /** Absolute backdrop URL from `client.imageUrl(...)`, or undefined. */
  backdropUrl?: string;
  /** Optional badges / actions rendered under the overview (e.g. HDR, Play). */
  children?: ReactNode;
}

export function DetailHeader({ title, year, overview, backdropUrl, children }: DetailHeaderProps) {
  return (
    <header
      style={{
        position: 'relative',
        borderRadius: 12,
        overflow: 'hidden',
        marginBottom: 32,
        background: backdropUrl ? `#000` : theme.colors.surface,
        minHeight: 240,
      }}
    >
      {backdropUrl && (
        <img
          src={backdropUrl}
          alt=""
          style={{
            position: 'absolute',
            inset: 0,
            width: '100%',
            height: '100%',
            objectFit: 'cover',
            opacity: 0.5,
          }}
        />
      )}
      {/* Bottom-up gradient so overlaid text stays readable over any backdrop. */}
      <div
        style={{
          position: 'relative',
          padding: 32,
          display: 'flex',
          flexDirection: 'column',
          justifyContent: 'flex-end',
          minHeight: 240,
          background: backdropUrl
            ? 'linear-gradient(to top, rgba(11,11,15,0.95), rgba(11,11,15,0.35))'
            : 'transparent',
        }}
      >
        <h1 style={{ fontSize: 34, margin: 0, color: theme.colors.text }}>
          {title}
          {year != null && (
            <span style={{ color: theme.colors.textMuted, fontWeight: 400 }}> ({year})</span>
          )}
        </h1>
        {overview && (
          <p
            style={{
              maxWidth: 720,
              margin: '12px 0 0',
              fontSize: 15,
              lineHeight: 1.5,
              color: theme.colors.text,
            }}
          >
            {overview}
          </p>
        )}
        {children && (
          <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginTop: 16 }}>
            {children}
          </div>
        )}
      </div>
    </header>
  );
}
