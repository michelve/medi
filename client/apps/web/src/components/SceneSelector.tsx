/**
 * `SceneSelector` (`docs/.tasks/99` Part C) — the in-player scene-selection grid.
 *
 * A control-bar button that opens a popover grid of chapter cards (poster frame + title +
 * timestamp); clicking a card seeks to that chapter's start. Mirrors jellyfin's scene view and
 * reuses the same open/click-outside/Escape popover mechanics as `SubtitleMenu`.
 *
 * The parent renders this ONLY when at least one chapter has a generated image
 * (`FileChapter.image`), so the button never appears for a title without scene frames — matching
 * jellyfin, which hides the Scenes section when no chapter has an `ImageTag`. Cards for chapters
 * without a frame still show (title + time), just with a placeholder tile.
 */

import { useEffect, useRef, useState } from 'react';
import type { FileChapter } from '@medi/api-client';
import { useApi } from '../api';
import { Icon } from './PlayerIcons';
import { formatTime } from '../lib/format';

const glass: React.CSSProperties = {
  background: 'rgba(28,28,32,0.62)',
  backdropFilter: 'blur(22px) saturate(160%)',
  WebkitBackdropFilter: 'blur(22px) saturate(160%)',
  border: '1px solid rgba(255,255,255,0.14)',
};

export function SceneSelector({
  fileId,
  chapters,
  onSeek,
}: {
  fileId: number;
  chapters: FileChapter[];
  /** Seek to an absolute position (ms) — the chapter's `start_ms`. */
  onSeek: (positionMs: number) => void;
}) {
  const api = useApi();
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        setOpen(false);
      }
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey, true);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey, true);
    };
  }, [open]);

  return (
    <div ref={ref} style={{ position: 'relative' }}>
      <button
        type="button"
        aria-label="Scenes"
        title="Scenes"
        onClick={() => setOpen((v) => !v)}
        style={{
          ...glass,
          width: 40,
          height: 40,
          borderRadius: 12,
          display: 'inline-flex',
          alignItems: 'center',
          justifyContent: 'center',
          color: '#fff',
          cursor: 'pointer',
          opacity: open ? 1 : 0.92,
          outline: open ? '1px solid rgba(255,255,255,0.35)' : 'none',
        }}
      >
        <Icon name="scenes" />
      </button>

      {open && (
        <div
          role="menu"
          aria-label="Scene selection"
          style={{
            ...glass,
            position: 'absolute',
            bottom: 50,
            right: 0,
            width: 460,
            maxWidth: '78vw',
            maxHeight: 340,
            overflowY: 'auto',
            borderRadius: 14,
            padding: 12,
            boxShadow: '0 12px 40px rgba(0,0,0,0.5)',
          }}
        >
          <div
            style={{
              padding: '2px 4px 10px',
              color: 'rgba(255,255,255,0.55)',
              fontSize: 11,
              textTransform: 'uppercase',
              letterSpacing: 0.6,
            }}
          >
            Scenes
          </div>
          <div
            style={{
              display: 'grid',
              gridTemplateColumns: 'repeat(3, 1fr)',
              gap: 10,
            }}
          >
            {chapters.map((c) => (
              <button
                key={c.ordinal}
                type="button"
                onClick={() => {
                  onSeek(c.start_ms);
                  setOpen(false);
                }}
                style={{
                  display: 'flex',
                  flexDirection: 'column',
                  gap: 5,
                  padding: 0,
                  border: 'none',
                  background: 'transparent',
                  color: '#fff',
                  cursor: 'pointer',
                  textAlign: 'left',
                }}
              >
                <div
                  style={{
                    width: '100%',
                    aspectRatio: '16 / 9',
                    borderRadius: 8,
                    overflow: 'hidden',
                    background: 'rgba(255,255,255,0.08)',
                    border: '1px solid rgba(255,255,255,0.12)',
                  }}
                >
                  {c.image && (
                    <img
                      src={api.chapterImageUrl(fileId, c.ordinal)}
                      alt=""
                      loading="lazy"
                      style={{ width: '100%', height: '100%', objectFit: 'cover', display: 'block' }}
                    />
                  )}
                </div>
                <div style={{ fontSize: 12, fontWeight: 500, lineHeight: 1.25, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                  {c.title || `Chapter ${c.ordinal + 1}`}
                </div>
                <div style={{ fontSize: 11, color: 'rgba(255,255,255,0.55)', fontVariantNumeric: 'tabular-nums' }}>
                  {formatTime(c.start_ms)}
                </div>
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
