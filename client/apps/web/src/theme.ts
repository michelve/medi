/**
 * DOM theme for the web SPA (Task 80). Re-exports the shared `@medi/ui` tokens so the
 * browser UI matches the TV app's palette and poster sizing, and adds a couple of
 * CSS-friendly helpers. The `@medi/ui` root index pulls in react-native components, so we
 * import the pure `theme` submodule directly (aliased in tsconfig/vite).
 */

import { theme } from '@medi/ui/theme';

export { theme };
export type { Theme } from '@medi/ui/theme';

/** The shared color tokens, ready to drop into inline styles or CSS variables. */
export const colors = theme.colors;

/**
 * Figma "Movie Details" design tokens (node 8:158). The detail pages match this comp for
 * spacing, typography and color; centralized here so the hero, section headings, poster
 * rows and cast strip all read from one source. Colors are white-on-dark with alpha tints
 * rather than the flat TV palette, and the type scale is Inter (loaded as the app font).
 */
export const detail = {
  /**
   * Max content width. The Figma comp's content column is ~1350px; capping the app around
   * 1600px keeps the layout from stretching thin on ultra-wide displays while leaving the
   * rows room to breathe. The shell centers `<main>` (and the header) to this width.
   */
  maxWidth: 1600,
  /** Page backdrop gradient behind a movie/series detail page. */
  pageGradient: 'linear-gradient(180deg, #26262a 0%, #131922 100%)',
  /**
   * A subtle film-grain texture (Figma) layered over the page gradient. An inline SVG
   * `feTurbulence` noise as a data-URI — self-contained (no asset fetch), tiled, and kept
   * faint so it reads as texture, not static. Layer it above `pageGradient` with a low
   * opacity via `grainOpacity`.
   */
  grainUrl:
    "url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='160' height='160'%3E%3Cfilter id='n'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.8' numOctaves='2' stitchTiles='stitch'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23n)'/%3E%3C/svg%3E\")",
  /** Opacity of the grain overlay over the gradient. */
  grainOpacity: 0.1,
  /** Font stack — Inter first, with the system fallback the shell already uses. */
  fontFamily:
    'Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif',
  text: {
    /** Solid white — headings, primary values. */
    primary: '#ffffff',
    /** 80% white — metadata line, secondary values, badge text. */
    secondary: 'rgba(255,255,255,0.8)',
    /** 55% white — de-emphasised captions. */
    tertiary: 'rgba(255,255,255,0.55)',
  },
  /** Translucent grey fill used by quality badges and round secondary buttons. */
  badgeBg: 'rgba(255,255,255,0.2)',
  /** Section heading: 24px Inter Medium white (poster rows, cast, etc.). */
  sectionHeading: { fontSize: 24, fontWeight: 500 as const, lineHeight: '30px' },
  /** Vertical rhythm between the stacked content sections. */
  sectionGap: 64,
  /** Gap between a section's heading and its content. */
  headingGap: 24,
  /** Poster tile in the collection / recommendation rows (2:3, square corners). */
  posterTile: { width: 216, height: 324, gap: 32, radius: 8 },
  /** Circular cast avatar. */
  avatar: 100,
} as const;

/** Install the theme colors as `--medi-*` CSS custom properties on `:root`. */
export function installThemeVars(root: HTMLElement = document.documentElement): void {
  root.style.setProperty('--medi-bg', theme.colors.background);
  root.style.setProperty('--medi-surface', theme.colors.surface);
  root.style.setProperty('--medi-text', theme.colors.text);
  root.style.setProperty('--medi-text-muted', theme.colors.textMuted);
  root.style.setProperty('--medi-accent', theme.colors.accent);
}

/**
 * Install the handful of global rules the inline-styled components can't express
 * (`:hover`, `::placeholder`). Idempotent — guarded by an id so React StrictMode's
 * double-invoke or a hot reload won't stack duplicate `<style>` tags.
 */
export function installGlobalStyles(doc: Document = document): void {
  const id = 'medi-global-styles';
  if (doc.getElementById(id)) return;
  const style = doc.createElement('style');
  style.id = id;
  style.textContent = `
    .medi-poster-card { transition: transform 120ms ease; }
    .medi-poster-card:hover { transform: scale(1.04); }
    .medi-search-input::placeholder { color: ${theme.colors.textMuted}; }
    .medi-credit-link:hover { text-decoration: underline; }

    /* Themed scrollbars. Firefox uses the inline scrollbar-width/color; WebKit/Chromium
       needs these pseudo-elements — a slim, subtle track/thumb instead of the chunky OS
       default, brightening a touch on hover. */
    * { scrollbar-color: rgba(255,255,255,0.22) transparent; }
    ::-webkit-scrollbar { width: 10px; height: 10px; }
    ::-webkit-scrollbar-track { background: transparent; }
    ::-webkit-scrollbar-thumb {
      background: rgba(255,255,255,0.18);
      border-radius: 999px;
      border: 3px solid transparent;
      background-clip: padding-box;
    }
    ::-webkit-scrollbar-thumb:hover { background: rgba(255,255,255,0.32); background-clip: padding-box; }
    ::-webkit-scrollbar-corner { background: transparent; }

    /* A horizontal scroll row that fades its content out at the edges instead of hard-
       clipping, with the scrollbar hidden (arrow buttons in \`HScroll\` signal scrollability).
       \`--fade-l\`/\`--fade-r\` toggle each side (JS sets them from scroll position). */
    .medi-hscroll {
      overflow-x: auto;
      overflow-y: hidden;
      /* Never let the flex row expand its own box to fit its children — it must stay within
         its container so it actually scrolls (and \`scrollWidth > clientWidth\` can be true). */
      min-width: 0;
      max-width: 100%;
      scrollbar-width: none;                 /* Firefox */
      -ms-overflow-style: none;              /* legacy Edge */
      --fade-l: 0px;
      --fade-r: 0px;
      -webkit-mask-image: linear-gradient(to right, transparent 0, #000 var(--fade-l), #000 calc(100% - var(--fade-r)), transparent 100%);
      mask-image: linear-gradient(to right, transparent 0, #000 var(--fade-l), #000 calc(100% - var(--fade-r)), transparent 100%);
    }
    .medi-hscroll::-webkit-scrollbar { display: none; }  /* WebKit */

    /* Scroll arrow button: a round control floating over an \`HScroll\` edge, shown only when
       that side has more content. A near-opaque dark fill with a hairline ring so it reads on
       both dark placeholders and bright poster art; scales up slightly on hover. */
    .medi-scroll-arrow {
      display: inline-flex;
      align-items: center;
      justify-content: center;
      width: 40px;
      height: 40px;
      border: 1px solid rgba(255,255,255,0.25);
      border-radius: 50%;
      color: #fff;
      background: rgba(20,20,24,0.82);
      cursor: pointer;
      box-shadow: 0 2px 10px rgba(0,0,0,0.45);
      transition: transform 120ms ease, background 120ms ease;
      backdrop-filter: blur(6px);
    }
    .medi-scroll-arrow:hover { background: rgba(20,20,24,0.95); transform: scale(1.08); }
  `;
  doc.head.appendChild(style);
}
