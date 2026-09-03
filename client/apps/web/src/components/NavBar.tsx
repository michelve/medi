/**
 * `NavBar` — the app's top navigation, styled after the Apple TV+ floating nav: a centered,
 * pill-shaped bar with a frosted-glass (backdrop-blur) translucent background. Nav items are
 * light text labels; the item for the current route is highlighted with a solid white pill.
 * A search icon at the right toggles the library search field (which drives `BrowseState`).
 *
 * The bar floats over the page content (sticky, transparent track) so a detail page's hero
 * backdrop shows through the glass, matching the reference. The search field only narrows the
 * already-loaded grid (see `SearchSortBar`); opening it here reuses that same state.
 */

import { useEffect, useRef, useState } from 'react';
import { Link, useLocation } from 'react-router-dom';
import { detail } from '../theme';
import { useBrowseState } from '../lib/browseState';
import { useDebouncedValue } from '../lib/useDebouncedValue';

/** Primary nav destinations. `match` decides which one owns the current route. */
const NAV_ITEMS: { label: string; to: string; match: (path: string) => boolean }[] = [
  { label: 'Home', to: '/', match: (p) => p === '/' || p.startsWith('/genre') || p.startsWith('/movie') || p.startsWith('/series') || p.startsWith('/person') },
  { label: 'Libraries', to: '/settings/libraries', match: (p) => p.startsWith('/settings/libraries') },
  { label: 'Status', to: '/settings/status', match: (p) => p.startsWith('/settings/status') },
];

export function NavBar() {
  const { pathname } = useLocation();

  return (
    // Fixed, transparent track — the glass pill floats over whatever scrolls beneath it and
    // stays pinned to the top of the viewport.
    <div
      style={{
        position: 'fixed',
        top: 0,
        left: 0,
        right: 0,
        zIndex: 20,
        display: 'flex',
        justifyContent: 'center',
        padding: '16px 24px',
        pointerEvents: 'none',
      }}
    >
      <nav
        aria-label="Primary"
        style={{
          pointerEvents: 'auto',
          display: 'flex',
          alignItems: 'center',
          gap: 4,
          padding: 6,
          borderRadius: 999,
          // Frosted glass: translucent dark fill + blur, a hairline ring and a soft shadow to
          // lift it off the content.
          background: 'rgba(24,24,28,0.55)',
          border: '1px solid rgba(255,255,255,0.12)',
          boxShadow: '0 8px 30px rgba(0,0,0,0.35)',
          backdropFilter: 'blur(20px) saturate(160%)',
          WebkitBackdropFilter: 'blur(20px) saturate(160%)',
        }}
      >
        {/* Brand pill — always the home affordance. */}
        <Link
          to="/"
          style={{
            display: 'inline-flex',
            alignItems: 'center',
            padding: '8px 14px',
            marginRight: 2,
            fontWeight: 700,
            fontSize: 16,
            letterSpacing: 0.2,
            color: detail.text.primary,
            textDecoration: 'none',
          }}
        >
          medi
        </Link>

        {NAV_ITEMS.map((item) => {
          const active = item.match(pathname);
          return (
            <Link
              key={item.to}
              to={item.to}
              aria-current={active ? 'page' : undefined}
              style={{
                display: 'inline-flex',
                alignItems: 'center',
                padding: '8px 16px',
                borderRadius: 999,
                fontSize: 15,
                fontWeight: 600,
                textDecoration: 'none',
                whiteSpace: 'nowrap',
                transition: 'background 120ms ease, color 120ms ease',
                // Active item: solid white pill with dark text (Apple TV+ highlight).
                color: active ? '#131316' : detail.text.secondary,
                background: active ? '#ffffff' : 'transparent',
                boxShadow: active ? '0 2px 8px rgba(0,0,0,0.25)' : 'none',
              }}
            >
              {item.label}
            </Link>
          );
        })}

        <SearchControl />
      </nav>
    </div>
  );
}

/**
 * The search affordance in the nav: a round icon button that expands into an inline text
 * field. Typing pushes a debounced query into `BrowseState` (the library grid filters on it).
 */
function SearchControl() {
  const { setQuery } = useBrowseState();
  const [open, setOpen] = useState(false);
  const [input, setInput] = useState('');
  const debounced = useDebouncedValue(input, 200);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    setQuery(debounced.trim());
  }, [debounced, setQuery]);

  // Focus the field when it opens; Escape closes and clears.
  useEffect(() => {
    if (open) inputRef.current?.focus();
  }, [open]);

  return (
    <div style={{ display: 'inline-flex', alignItems: 'center', gap: 4, marginLeft: 2 }}>
      {open && (
        <input
          ref={inputRef}
          className="medi-search-input"
          type="search"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Escape') {
              setInput('');
              setOpen(false);
            }
          }}
          placeholder="Search library…"
          aria-label="Search library"
          style={{
            width: 200,
            padding: '8px 12px',
            borderRadius: 999,
            border: '1px solid rgba(255,255,255,0.16)',
            background: 'rgba(255,255,255,0.08)',
            color: detail.text.primary,
            fontSize: 14,
            outline: 'none',
          }}
        />
      )}
      <button
        type="button"
        aria-label={open ? 'Close search' : 'Search'}
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        style={{
          display: 'inline-flex',
          alignItems: 'center',
          justifyContent: 'center',
          width: 36,
          height: 36,
          borderRadius: '50%',
          border: 'none',
          background: 'transparent',
          color: detail.text.secondary,
          cursor: 'pointer',
        }}
      >
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <circle cx="11" cy="11" r="7" stroke="currentColor" strokeWidth="2" />
          <path d="M20 20l-3.2-3.2" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
        </svg>
      </button>
    </div>
  );
}
