/**
 * `BannerActions` (Task 91 hero banner) — the action row on a movie's hero banner, matching
 * the Figma design: a white "▶ Play Movie" pill, a round grey "versions/files" button (opens
 * the Info dialog), and a round grey "···" overflow menu (Watch trailer / Fix match). Icons
 * are inline SVG so there's no icon-font dependency.
 */

import { useEffect, useRef, useState } from 'react';
import { theme, detail } from '../theme';

export interface BannerActionsProps {
  canPlay: boolean;
  onPlay: () => void;
  hasTrailer: boolean;
  onTrailer: () => void;
  onInfo: () => void;
  onFixMatch: () => void;
}

/** A round, filled-grey icon button — the Figma hero's secondary control. */
function IconButton({
  label,
  onClick,
  children,
}: {
  label: string;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={label}
      title={label}
      style={{
        // Figma secondary control: 40px round button, 80%-white fill, dark glyph.
        width: 40,
        height: 40,
        borderRadius: '50%',
        border: 'none',
        background: detail.text.secondary,
        color: '#131922',
        display: 'inline-flex',
        alignItems: 'center',
        justifyContent: 'center',
        cursor: 'pointer',
        padding: 0,
      }}
    >
      {children}
    </button>
  );
}

export function BannerActions({
  canPlay,
  onPlay,
  hasTrailer,
  onTrailer,
  onInfo,
  onFixMatch,
}: BannerActionsProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  // Close the overflow menu on an outside click / Escape.
  useEffect(() => {
    if (!menuOpen) return;
    const onDown = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) setMenuOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && setMenuOpen(false);
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [menuOpen]);

  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 16, flexWrap: 'wrap' }}>
      {canPlay && (
        <button
          type="button"
          onClick={onPlay}
          style={{
            // Figma primary action: 40px-tall white pill, 16px radius, 14px semibold, black
            // text, 16px icon→label gap.
            display: 'inline-flex',
            alignItems: 'center',
            gap: 16,
            height: 40,
            padding: '10px 16px',
            borderRadius: 16,
            border: 'none',
            background: '#fff',
            color: '#000',
            fontSize: 14,
            fontWeight: 600,
            cursor: 'pointer',
          }}
        >
          <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
            <path d="M8 5v14l11-7z" />
          </svg>
          Play Movie
        </button>
      )}

      {/* Files & versions (filmstrip). */}
      <IconButton label="Files & versions" onClick={onInfo}>
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <rect x="4" y="4" width="16" height="16" rx="2" stroke="currentColor" strokeWidth="2" />
          <path d="M9 4v16M15 4v16" stroke="currentColor" strokeWidth="2" />
          <path d="M4 9h5M4 15h5M15 9h5M15 15h5" stroke="currentColor" strokeWidth="2" />
        </svg>
      </IconButton>

      {/* Overflow menu (···): trailer + fix match. */}
      <div ref={menuRef} style={{ position: 'relative' }}>
        <IconButton label="More" onClick={() => setMenuOpen((v) => !v)}>
          <svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
            <circle cx="5" cy="12" r="2" />
            <circle cx="12" cy="12" r="2" />
            <circle cx="19" cy="12" r="2" />
          </svg>
        </IconButton>
        {menuOpen && (
          <div
            role="menu"
            style={{
              position: 'absolute',
              top: 48,
              left: 0,
              minWidth: 180,
              background: theme.colors.surface,
              borderRadius: 10,
              padding: 6,
              boxShadow: '0 8px 30px rgba(0,0,0,0.5)',
              zIndex: 20,
            }}
          >
            {hasTrailer && (
              <MenuItem
                label="Watch trailer"
                onClick={() => {
                  setMenuOpen(false);
                  onTrailer();
                }}
              />
            )}
            <MenuItem
              label="Fix match"
              onClick={() => {
                setMenuOpen(false);
                onFixMatch();
              }}
            />
          </div>
        )}
      </div>
    </div>
  );
}

function MenuItem({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button
      type="button"
      role="menuitem"
      onClick={onClick}
      className="medi-credit-link"
      style={{
        display: 'block',
        width: '100%',
        textAlign: 'left',
        padding: '9px 12px',
        borderRadius: 6,
        border: 'none',
        background: 'transparent',
        color: theme.colors.text,
        fontSize: 14,
        cursor: 'pointer',
      }}
    >
      {label}
    </button>
  );
}
