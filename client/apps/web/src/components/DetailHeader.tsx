/**
 * `DetailHeader` (Task 81) — the shared hero block at the top of a detail page.
 *
 * A backdrop image (dimmed for legibility) with the title, year and overview laid over
 * it — the browse app's echo of the Apple TV-style feature banner. Reused verbatim by
 * both the movie and series pages so their headers stay identical and restyling happens
 * in one place. Degrades to a flat surface when there's no backdrop art.
 */

import { useState, type ReactNode } from 'react';
import { theme } from '../theme';
import { BackdropTrailer } from './BackdropTrailer';

export interface DetailHeaderProps {
  title: string;
  year?: number | null;
  overview?: string | null;
  /** Absolute backdrop URL from `client.imageUrl(...)`, or undefined. */
  backdropUrl?: string;
  /**
   * Absolute logo URL (a transparent-PNG title wordmark from fanart.tv, via
   * `client.imageUrl(...)`) — Task 94. When set, it replaces the text `<h1>` title on the
   * hero; the title text is still exposed as the image's `alt`. Movie pages pass this; the
   * series page omits it (series logos are deferred), so its header is unchanged. If the
   * logo file 404s, an `onError` hides the image and reveals the text title.
   */
  logoUrl?: string;
  /**
   * A metadata line rendered directly under the title (e.g. genre · runtime · year, plus
   * quality badges). Sits above `overview`/`children`.
   */
  meta?: ReactNode;
  /** Minimum banner height in px. Defaults to 240; the movie page uses a taller hero. */
  minHeight?: number;
  /**
   * Layout style. `'bottom'` (default) is the original bottom-anchored, full-width overlay
   * used by the series page. `'hero'` is the Apple-TV/Figma movie hero: the backdrop bleeds
   * to the right, a left→right black gradient keeps a solid backing on the left, and the
   * content column is left-aligned and vertically centered over it (year lives in `meta`,
   * not the title).
   */
  layout?: 'bottom' | 'hero';
  /** Optional badges / actions rendered under the overview (e.g. HDR, Play). */
  children?: ReactNode;
  /**
   * A shelf fused to the bottom of the hero card (Figma `trailers_scenes`): a translucent
   * `rgba(11,11,15,0.8)` band with bottom-rounded corners that shares the card's rounded
   * rectangle with the backdrop above it. The movie page passes its Trailers row here so the
   * strip reads as part of the banner rather than a detached section. Hero layout only.
   */
  footer?: ReactNode;
  /**
   * A YouTube trailer key (Task: Apple-TV hero). When set on the `'hero'` layout, the trailer
   * fades in behind the hero content after a short beat, plays muted, then fades back to the
   * still backdrop. Ignored on the `'bottom'` layout and when there's no backdrop.
   */
  trailerYoutubeKey?: string;
}

export function DetailHeader({
  title,
  year,
  overview,
  backdropUrl,
  logoUrl,
  meta,
  minHeight = 240,
  layout = 'bottom',
  children,
  footer,
  trailerYoutubeKey,
}: DetailHeaderProps) {
  const hero = layout === 'hero';
  // A logo that fails to load (deleted on disk → 404) falls back to the text title.
  const [logoBroken, setLogoBroken] = useState(false);
  const showLogo = logoUrl != null && !logoBroken;
  // The backdrop trailer plays only on the hero, only when we have both a backdrop and a key.
  // If the video can't be embedded, `trailerUnavailable` retires the layer for good.
  const [trailerUnavailable, setTrailerUnavailable] = useState(false);
  const showTrailer =
    hero && backdropUrl != null && trailerYoutubeKey != null && !trailerUnavailable;

  return (
    <header
      style={{
        // The rounded card that clips both the backdrop hero and the fused footer shelf.
        position: 'relative',
        borderRadius: 16,
        overflow: 'hidden',
        marginBottom: 32,
        background: backdropUrl ? '#000' : theme.colors.surface,
      }}
    >
      {/* Hero region: the backdrop, its scrim, and the overlaid content. Owns the 16:9
          aspect (fanart/TMDB backdrops are 16:9) so the art is never cropped top/bottom; the
          footer shelf sits below it and adds its own height to the card. */}
      <div
        style={{
          position: 'relative',
          display: 'flex',
          flexDirection: 'column',
          minHeight,
          ...(hero && backdropUrl ? { aspectRatio: '16 / 9' } : {}),
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
              // The hero relies on its side gradient for legibility, so the art stays full
              // strength; the bottom layout dims the whole image instead.
              opacity: hero ? 1 : 0.5,
            }}
          />
        )}
        {/* Backdrop trailer: sits above the still <img> (which stays mounted as the fade
            target) and below the scrim. Keyed by the video so a movie→movie navigation tears
            the old player down and builds a fresh one. */}
        {showTrailer && (
          <BackdropTrailer
            key={trailerYoutubeKey}
            youtubeKey={trailerYoutubeKey}
            onUnavailable={() => setTrailerUnavailable(true)}
          />
        )}
        {/* Gradient overlay: a full-bleed scrim over the hero region so the backdrop never
            shows through uncovered — a left→right black scrim for the hero (content sits on
            solid black at the left, art shows through on the right); a bottom-up scrim
            otherwise. */}
        {backdropUrl && (
          <div
            aria-hidden="true"
            style={{
              position: 'absolute',
              inset: 0,
              background: hero
                ? 'linear-gradient(90deg, rgba(0,0,0,0.92) 30%, rgba(0,0,0,0.55) 55%, rgba(0,0,0,0) 100%)'
                : 'linear-gradient(to top, rgba(11,11,15,0.95), rgba(11,11,15,0.35))',
            }}
          />
        )}
        {/* Content block: sits above the scrim, vertically centered, with a min height so
            short synopses still give the banner presence. */}
        <div
          style={{
            position: 'relative',
            // Extra bottom padding on the hero so the action row isn't cramped against the
            // card edge / trailer shelf.
            padding: hero ? '48px 48px 80px' : 32,
            display: 'flex',
            flexDirection: 'column',
            justifyContent: 'center',
            alignItems: 'flex-start',
            minHeight,
            flex: '1 1 auto',
          }}
        >
        <div style={{ maxWidth: hero ? 560 : undefined, width: '100%' }}>
          {showLogo ? (
            // The transparent-PNG wordmark stands in for the title text (Task 94). Constrained
            // by height with width:auto so a wide or narrow logo both sit left-aligned over the
            // gradient without letterboxing or overflow; `alt` carries the title for a11y.
            <div>
              <img
                src={logoUrl}
                alt={title}
                onError={() => setLogoBroken(true)}
                style={{
                  maxHeight: hero ? 120 : 88,
                  maxWidth: '100%',
                  width: 'auto',
                  objectFit: 'contain',
                  display: 'block',
                }}
              />
              {/* In the bottom layout the year normally lives in the <h1>; keep it as a caption
                  under the logo. The hero already shows the year in its `meta` line. */}
              {!hero && year != null && (
                <span
                  style={{
                    color: theme.colors.textMuted,
                    fontSize: 16,
                    marginTop: 8,
                    display: 'inline-block',
                  }}
                >
                  ({year})
                </span>
              )}
            </div>
          ) : (
            <h1 style={{ fontSize: hero ? 40 : 34, margin: 0, color: theme.colors.text, lineHeight: 1.1 }}>
              {title}
              {!hero && year != null && (
                <span style={{ color: theme.colors.textMuted, fontWeight: 400 }}> ({year})</span>
              )}
            </h1>
          )}
          {meta && (
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 10,
                flexWrap: 'wrap',
                marginTop: hero ? 16 : 12,
              }}
            >
              {meta}
            </div>
          )}
          {overview && (
            <p
              style={{
                maxWidth: hero ? undefined : 720,
                margin: hero ? '18px 0 0' : '12px 0 0',
                fontSize: 15,
                lineHeight: 1.5,
                color: theme.colors.text,
              }}
            >
              {overview}
            </p>
          )}
          {children && (
            <div
              style={{
                display: 'flex',
                flexDirection: 'column',
                alignItems: 'flex-start',
                // More breathing room between the summary and the action row on the hero.
                gap: hero ? 28 : 12,
                marginTop: hero ? 28 : 16,
              }}
            >
              {children}
            </div>
          )}
        </div>
        </div>
        {/* /content block */}
      </div>
      {/* /hero region */}
      {/* Footer shelf fused to the card's bottom (Figma `trailers_scenes`): a translucent
          dark band sharing the card's rounded rectangle with the backdrop above. */}
      {footer && (
        <div
          style={{
            position: 'relative',
            background: 'rgba(11,11,15,0.8)',
            padding: '34px 96px 44px',
          }}
        >
          {footer}
        </div>
      )}
    </header>
  );
}
