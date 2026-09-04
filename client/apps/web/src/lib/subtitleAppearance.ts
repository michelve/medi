/**
 * Subtitle appearance settings (`docs/.tasks/99` C4) — viewer-controlled caption styling for
 * the native `<track>` path, applied via an injected `::cue` stylesheet and persisted in
 * `localStorage`. (libass is authoritative on ASS styling; these primarily affect plain text.)
 */

export interface SubtitleAppearance {
  /** Font size as a percentage of the default cue size (50–200). */
  fontSizePct: number;
  /** Text color (CSS color). */
  textColor: string;
  /** Cue background opacity, 0–100 (0 = transparent, 100 = solid black box). */
  backgroundOpacity: number;
  /** Edge style for legibility. */
  edgeStyle: 'none' | 'dropShadow' | 'outline';
  /** Vertical position: distance of the cue block from the bottom, in vh (0–30). */
  bottomOffsetVh: number;
}

export const DEFAULT_APPEARANCE: SubtitleAppearance = {
  fontSizePct: 100,
  textColor: '#ffffff',
  backgroundOpacity: 55,
  edgeStyle: 'dropShadow',
  bottomOffsetVh: 8,
};

const KEY = 'medi.player.subtitleAppearance';

export function readAppearance(): SubtitleAppearance {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return DEFAULT_APPEARANCE;
    // Merge onto defaults so a stored older shape (missing keys) stays valid.
    return { ...DEFAULT_APPEARANCE, ...(JSON.parse(raw) as Partial<SubtitleAppearance>) };
  } catch {
    return DEFAULT_APPEARANCE;
  }
}

export function writeAppearance(a: SubtitleAppearance): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(a));
  } catch {
    /* storage disabled — non-fatal */
  }
}

/** The `::cue` edge treatment for the chosen style. */
function edgeCss(style: SubtitleAppearance['edgeStyle']): string {
  switch (style) {
    case 'dropShadow':
      return 'text-shadow: 0 2px 4px rgba(0,0,0,0.9), 0 0 2px rgba(0,0,0,0.9);';
    case 'outline':
      return 'text-shadow: -1px -1px 0 #000, 1px -1px 0 #000, -1px 1px 0 #000, 1px 1px 0 #000;';
    case 'none':
    default:
      return '';
  }
}

/**
 * Build a scoped `::cue` stylesheet for the given appearance, targeting the player's `<video>`
 * (scoped by `selector`, e.g. `.medi-video`). Vertical position is applied to the cue *box* via
 * the video element's `::cue` line isn't reliable across browsers, so we shift the whole cue
 * region with a translate on the pseudo-element where supported and rely on `line`-less cues.
 */
export function cueCss(a: SubtitleAppearance, selector: string): string {
  const bg =
    a.backgroundOpacity <= 0
      ? 'transparent'
      : `rgba(0,0,0,${(a.backgroundOpacity / 100).toFixed(2)})`;
  // `font-size` on `::cue` scales the cue text; color + background + shadow are widely honored.
  return `
${selector}::cue {
  font-size: ${a.fontSizePct}%;
  color: ${a.textColor};
  background-color: ${bg};
  ${edgeCss(a.edgeStyle)}
}
${selector}::-webkit-media-text-track-display {
  padding-bottom: ${a.bottomOffsetVh}vh;
}
`.trim();
}
