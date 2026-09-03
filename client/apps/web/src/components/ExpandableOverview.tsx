/**
 * `ExpandableOverview` (Task 91 detail banner) — the movie synopsis under the banner
 * actions, clamped to a few lines with an inline "more" toggle that expands to the full text.
 *
 * Collapsed, the paragraph is line-clamped (CSS `-webkit-line-clamp`) so a long overview
 * doesn't blow out the hero; the toggle only appears when the text actually overflows the
 * clamp, so short synopses show in full with no dangling "more". Renders nothing when the
 * movie has no overview.
 */

import { useEffect, useRef, useState } from 'react';
import { theme } from '../theme';

export function ExpandableOverview({
  text,
  lines = 2,
  size = 15,
}: {
  text?: string | null;
  lines?: number;
  /** Body font size in px (Figma About uses 16). */
  size?: number;
}) {
  const [expanded, setExpanded] = useState(false);
  // Whether the collapsed text is actually clipped (drives showing the toggle at all).
  const [overflows, setOverflows] = useState(false);
  const ref = useRef<HTMLParagraphElement>(null);

  // Measure after layout: the paragraph overflows if its full scroll height exceeds the
  // clamped client height. Re-check on text change and on resize.
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const check = () => setOverflows(el.scrollHeight - el.clientHeight > 1);
    check();
    window.addEventListener('resize', check);
    return () => window.removeEventListener('resize', check);
  }, [text, lines]);

  if (!text) return null;

  return (
    <div style={{ maxWidth: 720, marginTop: 16 }}>
      <p
        ref={ref}
        style={{
          margin: 0,
          fontSize: size,
          lineHeight: size >= 16 ? '24px' : 1.5,
          color: theme.colors.text,
          ...(expanded
            ? {}
            : {
                display: '-webkit-box',
                WebkitLineClamp: lines,
                WebkitBoxOrient: 'vertical',
                overflow: 'hidden',
              }),
        }}
      >
        {text}
      </p>
      {(overflows || expanded) && (
        <button
          type="button"
          onClick={() => setExpanded((v) => !v)}
          style={{
            marginTop: 4,
            padding: 0,
            border: 'none',
            background: 'none',
            color: theme.colors.textMuted,
            fontSize: 14,
            fontWeight: 600,
            cursor: 'pointer',
          }}
        >
          {expanded ? 'less' : 'more'}
        </button>
      )}
    </div>
  );
}
