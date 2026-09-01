/**
 * Minimal 10-foot UI theme. Kept tiny and centralized so tile/row sizing stays
 * consistent across `PosterGrid`, `Carousel`, and `HeroBanner`. Values are tuned
 * for a 1080p TV safe area; scale up for 4K by multiplying `scale`.
 */

export const theme = {
  colors: {
    background: '#0b0b0f',
    surface: '#1a1a1f',
    text: '#f2f2f7',
    textMuted: '#9a9aa2',
    focus: '#ffffff',
    accent: '#0a84ff',
  },
  /** Standard 2:3 movie poster tile. */
  poster: {
    width: 220,
    height: 330,
    gap: 24,
    radius: 8,
  },
  /** Horizontal safe-area padding (TV overscan margin). */
  screenPaddingH: 48,
  rowGap: 40,
} as const;

export type Theme = typeof theme;
