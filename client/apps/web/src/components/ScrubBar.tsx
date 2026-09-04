/**
 * `ScrubBar` (Task 82) — the seek bar with an optional trickplay thumbnail.
 *
 * A progress bar the user clicks/drags to seek. On hover it computes the hovered position
 * and, when trickplay geometry is available, crops the covering cell out of the tiled-JPG
 * mosaic (`tileForPosition` + `client.trickplayUrl(fileId, 'jpg')`) into a floating preview.
 * With no trickplay meta it falls back to the hovered chapter's poster frame (`docs/.tasks/99`
 * Part C), and with neither it's just a plain bar; no error is surfaced. This trickplay →
 * chapter-image → time-only order mirrors jellyfin's seek-bubble fallback.
 *
 * Geometry math is reused from `@medi/player/trickplay` — the same module the TV player
 * uses — so the two clients stay in lockstep.
 */

import { useRef, useState } from 'react';
import { tileForPosition, type TrickplayMeta } from '@medi/player/trickplay';
import type { FileChapter } from '@medi/api-client';
import { chapterAt } from '@medi/player/chapters';
import { useApi } from '../api';
import { theme } from '../theme';

/** Chapter-image fallback thumbnail width (px); matches the ~400px frames the backend writes,
 * shown at a scrub-preview size. Height follows the video aspect (16:9 assumed for the box). */
const CHAPTER_THUMB_W = 220;
const CHAPTER_THUMB_H = Math.round((CHAPTER_THUMB_W * 9) / 16);

export interface ScrubBarProps {
  /** The file being scrubbed — for building chapter-image URLs (`docs/.tasks/99` Part C). */
  fileId: number;
  positionMs: number;
  durationMs: number;
  /** Seek to an absolute position (ms) — commit on click. */
  onSeek: (positionMs: number) => void;
  /** Mosaic URL + grid geometry; undefined ⇒ plain bar, no thumbnails. */
  trickplay?: TrickplayMeta;
  /** Embedded chapters (`docs/.tasks/99`): ticks on the track + name in the hover bubble. */
  chapters?: FileChapter[];
}

export function ScrubBar({ fileId, positionMs, durationMs, onSeek, trickplay, chapters }: ScrubBarProps) {
  const api = useApi();
  const trackRef = useRef<HTMLDivElement | null>(null);
  // Hover position in ms (for the thumbnail), and its x within the track (for placement).
  const [hover, setHover] = useState<{ ms: number; x: number } | null>(null);

  const pct = durationMs > 0 ? Math.min(1, positionMs / durationMs) : 0;

  const msAtClientX = (clientX: number): number => {
    const track = trackRef.current;
    if (!track || durationMs <= 0) return 0;
    const rect = track.getBoundingClientRect();
    const ratio = Math.min(1, Math.max(0, (clientX - rect.left) / rect.width));
    return ratio * durationMs;
  };

  const tile = hover && trickplay ? tileForPosition(trickplay, hover.ms) : null;
  // The chapter under the hover point, for a label above the thumbnail (or above the bar when
  // there's no trickplay sheet). Only meaningful when the title actually has chapters.
  const hoverChapter =
    hover && chapters && chapters.length > 0 ? chapterAt(chapters, hover.ms) : null;
  // Chapter-image fallback: only when there's no trickplay tile AND the hovered chapter has a
  // generated frame (`image === true`). Trickplay stays the preferred hover preview.
  const chapterThumb = !tile && hoverChapter?.image ? hoverChapter : null;
  // The height the name label clears — whichever thumbnail is showing, if any.
  const thumbH = tile ? tile.height : chapterThumb ? CHAPTER_THUMB_H : 0;

  return (
    <div style={{ position: 'relative', width: '100%' }}>
      {/* Trickplay thumbnail, positioned above the hovered point. */}
      {tile && trickplay && hover && (
        <div
          style={{
            position: 'absolute',
            bottom: 20,
            left: Math.max(0, hover.x - tile.width / 2),
            width: tile.width,
            height: tile.height,
            borderRadius: 4,
            overflow: 'hidden',
            border: `1px solid ${theme.colors.surface}`,
            boxShadow: '0 4px 16px rgba(0,0,0,0.6)',
            backgroundImage: `url(${trickplay.url})`,
            backgroundRepeat: 'no-repeat',
            backgroundPosition: `-${tile.x}px -${tile.y}px`,
            pointerEvents: 'none',
          }}
        />
      )}
      {/* Chapter-image fallback thumbnail (no trickplay sheet, but the chapter has a frame). */}
      {chapterThumb && hover && (
        <img
          src={api.chapterImageUrl(fileId, chapterThumb.ordinal)}
          alt=""
          style={{
            position: 'absolute',
            bottom: 20,
            left: Math.max(0, hover.x - CHAPTER_THUMB_W / 2),
            width: CHAPTER_THUMB_W,
            height: CHAPTER_THUMB_H,
            objectFit: 'cover',
            borderRadius: 4,
            border: `1px solid ${theme.colors.surface}`,
            boxShadow: '0 4px 16px rgba(0,0,0,0.6)',
            pointerEvents: 'none',
          }}
        />
      )}
      {/* Chapter name above the hovered point — sits over the thumbnail when there is one, or
          just above the bar otherwise. `textContent` via a child string is XSS-safe in React. */}
      {hoverChapter?.title && hover && (
        <div
          style={{
            position: 'absolute',
            bottom: thumbH ? 20 + thumbH + 6 : 20,
            left: hover.x,
            transform: 'translateX(-50%)',
            maxWidth: 240,
            padding: '4px 10px',
            borderRadius: 8,
            background: 'rgba(20,20,24,0.82)',
            backdropFilter: 'blur(12px)',
            WebkitBackdropFilter: 'blur(12px)',
            border: '1px solid rgba(255,255,255,0.12)',
            color: '#fff',
            fontSize: 12,
            fontWeight: 500,
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            pointerEvents: 'none',
            boxShadow: '0 4px 16px rgba(0,0,0,0.5)',
          }}
        >
          {hoverChapter.title}
        </div>
      )}
      <div
        ref={trackRef}
        onClick={(e) => onSeek(msAtClientX(e.clientX))}
        onMouseMove={(e) => setHover({ ms: msAtClientX(e.clientX), x: e.clientX - (trackRef.current?.getBoundingClientRect().left ?? 0) })}
        onMouseLeave={() => setHover(null)}
        style={{
          position: 'relative',
          height: 8,
          borderRadius: 4,
          background: 'rgba(255,255,255,0.24)',
          cursor: 'pointer',
        }}
      >
        <div
          style={{
            position: 'absolute',
            left: 0,
            top: 0,
            bottom: 0,
            width: `${pct * 100}%`,
            borderRadius: 4,
            background: theme.colors.accent,
          }}
        />
        {/* Chapter ticks (`docs/.tasks/99`): a small mark at each chapter start (skipping the
            0ms opening edge). Watched ticks (before the playhead) dim; unwatched stay bright. */}
        {durationMs > 0 &&
          chapters?.map((c) =>
            c.start_ms <= 0 || c.start_ms >= durationMs ? null : (
              <div
                key={c.ordinal}
                aria-hidden
                style={{
                  position: 'absolute',
                  top: 0,
                  bottom: 0,
                  left: `${(c.start_ms / durationMs) * 100}%`,
                  width: 2,
                  marginLeft: -1,
                  borderRadius: 1,
                  background:
                    c.start_ms <= positionMs ? 'rgba(255,255,255,0.45)' : 'rgba(255,255,255,0.85)',
                  pointerEvents: 'none',
                }}
              />
            ),
          )}
      </div>
    </div>
  );
}
