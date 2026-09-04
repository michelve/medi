/**
 * Line-art player-control icons (`docs/.tasks/97` Part B) — inlined **Heroicons** (MIT, by
 * Tailwind Labs) outline/solid path data, so we get crisp, consistent SVG glyphs without adding
 * a runtime dependency (the CDN allowlist doesn't apply to this app, and inlining keeps the
 * bundle self-contained). `currentColor` inherits the button's text color.
 *
 * Where Heroicons has no exact match (10s skip, mute), we use the closest Heroicon base with a
 * small tweak. Sizes default to 20px.
 */

export type IconName =
  | 'play'
  | 'pause'
  | 'back10'
  | 'forward10'
  | 'volumeHigh'
  | 'volumeLow'
  | 'volumeMute'
  | 'fullscreen'
  | 'fullscreenExit'
  | 'audio'
  | 'subtitles'
  | 'nextChapter'
  | 'prevChapter'
  | 'scenes'
  | 'check';

export interface IconProps {
  name: IconName;
  /** Pixel size (square). Default 20. */
  size?: number;
}

/** Render a single control icon. Solid glyphs for play/pause; outline for the rest. */
export function Icon({ name, size = 20 }: IconProps) {
  const common = {
    width: size,
    height: size,
    viewBox: '0 0 24 24',
    'aria-hidden': true,
    focusable: false as const,
    style: { display: 'block' as const },
  };

  switch (name) {
    // Solid (filled) — Heroicons `play` / `pause` solid.
    case 'play':
      return (
        <svg {...common} fill="currentColor">
          <path d="M4.5 5.653c0-1.427 1.529-2.33 2.779-1.643l11.54 6.347c1.295.712 1.295 2.573 0 3.286L7.28 19.99c-1.25.687-2.779-.217-2.779-1.643V5.653Z" />
        </svg>
      );
    case 'pause':
      return (
        <svg {...common} fill="currentColor">
          <path
            fillRule="evenodd"
            d="M6.75 5.25a.75.75 0 0 1 .75-.75H9a.75.75 0 0 1 .75.75v13.5a.75.75 0 0 1-.75.75H7.5a.75.75 0 0 1-.75-.75V5.25Zm7.5 0A.75.75 0 0 1 15 4.5h1.5a.75.75 0 0 1 .75.75v13.5a.75.75 0 0 1-.75.75H15a.75.75 0 0 1-.75-.75V5.25Z"
            clipRule="evenodd"
          />
        </svg>
      );

    // Outline — Heroicons `arrow-uturn-left` / `-right` (skip), with a "10" numeral overlaid.
    case 'back10':
      return (
        <SkipIcon dir="back" size={size} />
      );
    case 'forward10':
      return (
        <SkipIcon dir="forward" size={size} />
      );

    // Heroicons outline `speaker-wave` (high), a trimmed variant (low), `speaker-x-mark` (mute).
    case 'volumeHigh':
      return (
        <svg {...common} fill="none" stroke="currentColor" strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round">
          <path d="M9.75 8.25 6 8.25a.75.75 0 0 0-.75.75v6c0 .414.336.75.75.75h3.75l4.5 3.75V4.5l-4.5 3.75Z" />
          <path d="M17.25 9a4 4 0 0 1 0 6M19.5 6.75a7.5 7.5 0 0 1 0 10.5" />
        </svg>
      );
    case 'volumeLow':
      return (
        <svg {...common} fill="none" stroke="currentColor" strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round">
          <path d="M9.75 8.25 6 8.25a.75.75 0 0 0-.75.75v6c0 .414.336.75.75.75h3.75l4.5 3.75V4.5l-4.5 3.75Z" />
          <path d="M17.25 9a4 4 0 0 1 0 6" />
        </svg>
      );
    case 'volumeMute':
      return (
        <svg {...common} fill="none" stroke="currentColor" strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round">
          <path d="M9.75 8.25 6 8.25a.75.75 0 0 0-.75.75v6c0 .414.336.75.75.75h3.75l4.5 3.75V4.5l-4.5 3.75Z" />
          <path d="m16.5 9 4 4m0-4-4 4" />
        </svg>
      );

    // Heroicons outline `arrows-pointing-out` / `arrows-pointing-in`.
    case 'fullscreen':
      return (
        <svg {...common} fill="none" stroke="currentColor" strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round">
          <path d="M3.75 3.75v4.5m0-4.5h4.5m-4.5 0L9 9M20.25 3.75v4.5m0-4.5h-4.5m4.5 0L15 9M3.75 20.25v-4.5m0 4.5h4.5m-4.5 0L9 15M20.25 20.25v-4.5m0 4.5h-4.5m4.5 0L15 15" />
        </svg>
      );
    case 'fullscreenExit':
      return (
        <svg {...common} fill="none" stroke="currentColor" strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round">
          <path d="M9 9V4.5M9 9H4.5M9 9 3.75 3.75M9 15v4.5M9 15H4.5M9 15l-5.25 5.25M15 9h4.5M15 9V4.5M15 9l5.25-5.25M15 15h4.5M15 15v4.5m0-4.5 5.25 5.25" />
        </svg>
      );

    // Heroicons outline `speaker-wave` is used for volume; for audio-track use `musical-note`.
    case 'audio':
      return (
        <svg {...common} fill="none" stroke="currentColor" strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round">
          <path d="M9 9l10.5-3m-10.5 3v7.5a2.25 2.25 0 1 1-1.5-2.122M9 9l-3 .857M19.5 6v7.5a2.25 2.25 0 1 1-1.5-2.122M19.5 6l-1.5.429" />
        </svg>
      );

    // Heroicons outline `chat-bubble-bottom-center-text` for captions/subtitles.
    case 'subtitles':
      return (
        <svg {...common} fill="none" stroke="currentColor" strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round">
          <path d="M8.25 9h7.5m-7.5 3h4.5m-4.72 6.03 1.72-1.72h6.75A2.25 2.25 0 0 0 18.75 14V6.75A2.25 2.25 0 0 0 16.5 4.5h-9A2.25 2.25 0 0 0 5.25 6.75V14a2.25 2.25 0 0 0 2.03 2.239v2.008c0 .46.556.69.882.362Z" />
        </svg>
      );

    // Heroicons solid `forward` / `backward` (skip to next / previous chapter).
    case 'nextChapter':
      return (
        <svg {...common} fill="currentColor">
          <path d="M5.055 7.06C3.805 6.347 2.25 7.25 2.25 8.69v6.62c0 1.44 1.555 2.343 2.805 1.63L9 14.688V15.31c0 1.44 1.555 2.343 2.805 1.63l5.856-3.345a1.875 1.875 0 0 0 0-3.256L11.805 7.06C10.555 6.347 9 7.25 9 8.69v.622L5.055 7.06Zm14.445.44a.75.75 0 0 0-.75.75v7.5a.75.75 0 0 0 1.5 0v-7.5a.75.75 0 0 0-.75-.75Z" />
        </svg>
      );
    case 'prevChapter':
      return (
        <svg {...common} fill="currentColor">
          <path d="M18.945 7.06c1.25-.713 2.805.19 2.805 1.63v6.62c0 1.44-1.555 2.343-2.805 1.63L15 14.688V15.31c0 1.44-1.555 2.343-2.805 1.63L6.34 13.595a1.875 1.875 0 0 1 0-3.256L12.195 7.06C13.445 6.347 15 7.25 15 8.69v.622l3.945-2.252ZM4.5 7.5a.75.75 0 0 1 .75.75v7.5a.75.75 0 0 1-1.5 0v-7.5A.75.75 0 0 1 4.5 7.5Z" />
        </svg>
      );

    // Heroicons outline `squares-2x2` (a 2×2 grid — the scene-selection affordance).
    case 'scenes':
      return (
        <svg {...common} fill="none" stroke="currentColor" strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round">
          <path d="M3.75 6A2.25 2.25 0 0 1 6 3.75h2.25A2.25 2.25 0 0 1 10.5 6v2.25a2.25 2.25 0 0 1-2.25 2.25H6a2.25 2.25 0 0 1-2.25-2.25V6ZM3.75 15.75A2.25 2.25 0 0 1 6 13.5h2.25a2.25 2.25 0 0 1 2.25 2.25V18a2.25 2.25 0 0 1-2.25 2.25H6A2.25 2.25 0 0 1 3.75 18v-2.25ZM13.5 6a2.25 2.25 0 0 1 2.25-2.25H18A2.25 2.25 0 0 1 20.25 6v2.25A2.25 2.25 0 0 1 18 10.5h-2.25a2.25 2.25 0 0 1-2.25-2.25V6ZM13.5 15.75a2.25 2.25 0 0 1 2.25-2.25H18a2.25 2.25 0 0 1 2.25 2.25V18A2.25 2.25 0 0 1 18 20.25h-2.25A2.25 2.25 0 0 1 13.5 18v-2.25Z" />
        </svg>
      );

    // Heroicons outline `check`.
    case 'check':
      return (
        <svg {...common} fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
          <path d="m4.5 12.75 6 6 9-13.5" />
        </svg>
      );
  }
}

/**
 * A skip glyph — Heroicons `arrow-uturn-{left,right}` with a small "10" numeral, since Heroicons
 * has no dedicated "skip 10s" icon. The numeral sits inside the loop of the arrow.
 */
function SkipIcon({ dir, size }: { dir: 'back' | 'forward'; size: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      aria-hidden
      focusable={false}
      style={{ display: 'block' }}
      fill="none"
      stroke="currentColor"
      strokeWidth={1.6}
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      {dir === 'back' ? (
        // Curved arrow sweeping counter-clockwise (rewind).
        <path d="M9 5 5 9m0 0 4 4M5 9h7a6 6 0 1 1-5.19 9" />
      ) : (
        // Curved arrow sweeping clockwise (fast-forward).
        <path d="m15 5 4 4m0 0-4 4m4-4h-7a6 6 0 1 0 5.19 9" />
      )}
      <text
        x="12"
        y="15.5"
        textAnchor="middle"
        fontSize="7"
        fontWeight="700"
        fill="currentColor"
        stroke="none"
      >
        10
      </text>
    </svg>
  );
}
