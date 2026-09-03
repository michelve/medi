/**
 * `HScroll` — a horizontally-scrolling row that fades its content at the edges instead of
 * clipping it with a hard line, hides the native scrollbar, and shows a round arrow button
 * on whichever side has more content to scroll to.
 *
 * The `.medi-hscroll` class (see `installGlobalStyles`) applies the edge mask via
 * `--fade-l` / `--fade-r` CSS variables and hides the scrollbar; this component measures the
 * scroll position, fades + shows an arrow only on a side that actually has off-screen
 * content, and scrolls by roughly a viewport-width when an arrow is clicked. Used by the
 * detail-page rows (trailers, cast, poster strips) so they all scroll identically.
 */

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from 'react';

/** How wide the fade is at each edge, px. Matches the default in `.medi-hscroll`. */
const FADE = 40;

export function HScroll({
  children,
  gap,
  style,
}: {
  children: ReactNode;
  /** Gap between items, px. */
  gap: number;
  /** Extra styles merged onto the scroll container. */
  style?: CSSProperties;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [canLeft, setCanLeft] = useState(false);
  const [canRight, setCanRight] = useState(false);

  // Fade + arrow on a side only when there's overflow to reveal there.
  const update = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    const max = el.scrollWidth - el.clientWidth;
    const left = el.scrollLeft;
    const showLeft = left > 1;
    const showRight = left < max - 1;
    el.style.setProperty('--fade-l', showLeft ? `${FADE}px` : '0px');
    el.style.setProperty('--fade-r', showRight ? `${FADE}px` : '0px');
    setCanLeft(showLeft);
    setCanRight(showRight);
  }, []);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    // Measure now, and again next frame — on first mount the flex children (and any images)
    // may not have laid out yet, so `scrollWidth` can momentarily equal `clientWidth`.
    update();
    const raf = requestAnimationFrame(update);
    // Re-evaluate when the row or its content resizes (width changes, images load and grow),
    // and on window resize.
    const ro = new ResizeObserver(update);
    ro.observe(el);
    for (const child of Array.from(el.children)) ro.observe(child);
    window.addEventListener('resize', update);
    // Late-loading images can change the scroll width after paint.
    for (const img of Array.from(el.querySelectorAll('img'))) {
      if (!img.complete) img.addEventListener('load', update, { once: true });
    }
    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
      window.removeEventListener('resize', update);
    };
  }, [update, children]);

  // Scroll by ~85% of the visible width so a click reveals a fresh set of items with a
  // little overlap for orientation.
  const scrollByPage = (dir: 1 | -1) => {
    const el = ref.current;
    if (!el) return;
    el.scrollBy({ left: dir * el.clientWidth * 0.85, behavior: 'smooth' });
  };

  return (
    <div style={{ position: 'relative' }}>
      <div
        ref={ref}
        className="medi-hscroll"
        onScroll={update}
        style={{ display: 'flex', gap, ...style }}
      >
        {children}
      </div>

      {canLeft && (
        <Arrow side="left" onClick={() => scrollByPage(-1)} />
      )}
      {canRight && (
        <Arrow side="right" onClick={() => scrollByPage(1)} />
      )}
    </div>
  );
}

/** A round scroll-affordance button pinned to one edge, vertically centered on the row. */
function Arrow({ side, onClick }: { side: 'left' | 'right'; onClick: () => void }) {
  return (
    <button
      type="button"
      aria-label={side === 'left' ? 'Scroll left' : 'Scroll right'}
      onClick={onClick}
      className="medi-scroll-arrow"
      style={{
        position: 'absolute',
        // Centre vertically without `transform` so the hover rule owns `transform`.
        top: 'calc(50% - 20px)',
        [side]: 4,
        zIndex: 2,
      }}
    >
      <svg width="20" height="20" viewBox="0 0 24 24" fill="none" aria-hidden="true">
        <path
          d={side === 'left' ? 'M15 5l-7 7 7 7' : 'M9 5l7 7-7 7'}
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
    </button>
  );
}
