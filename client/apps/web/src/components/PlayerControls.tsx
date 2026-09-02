/**
 * `PlayerControls` (Task 82) — the DOM transport overlay.
 *
 * A presentational layer over the `<video>`, driven by the shared `usePlayerControls`
 * reducer (from `@medi/player/usePlayerControls`, the same state machine the TV player
 * uses). It renders play/pause, the current-time / duration clock, and the `ScrubBar`; the
 * page feeds the reducer DOM events (Space/click → play-pause, ←/→ → seek, pointer-move →
 * reveal), so this component only reflects `controls` state and forwards intent back.
 *
 * The overlay auto-hides via the reducer's timer; we fade it with opacity so it can't trap
 * pointer events while hidden.
 */

import type { PlayerControls as Controls } from '@medi/player/usePlayerControls';
import type { TrickplayMeta } from '@medi/player/trickplay';
import { ScrubBar } from './ScrubBar';
import { formatTime } from '../lib/format';
import { theme } from '../theme';

export interface PlayerControlsProps {
  controls: Controls;
  title: string;
  trickplay?: TrickplayMeta;
  /** Seek to an absolute position (ms) — from a ScrubBar click. */
  onSeek: (positionMs: number) => void;
}

export function PlayerControls({ controls, title, trickplay, onSeek }: PlayerControlsProps) {
  const { overlayVisible, isPlaying, displayPositionMs, durationMs, handleRemote } = controls;

  return (
    <div
      style={{
        position: 'absolute',
        inset: 0,
        display: 'flex',
        flexDirection: 'column',
        justifyContent: 'flex-end',
        padding: 20,
        gap: 12,
        background: 'linear-gradient(to top, rgba(0,0,0,0.72), rgba(0,0,0,0) 45%)',
        opacity: overlayVisible ? 1 : 0,
        transition: 'opacity 200ms ease',
        pointerEvents: overlayVisible ? 'auto' : 'none',
      }}
    >
      <div style={{ color: theme.colors.text, fontSize: 15, fontWeight: 600 }}>{title}</div>

      <ScrubBar
        positionMs={displayPositionMs}
        durationMs={durationMs}
        onSeek={onSeek}
        trickplay={trickplay}
      />

      <div style={{ display: 'flex', alignItems: 'center', gap: 16 }}>
        <button
          type="button"
          onClick={() => handleRemote('playPause')}
          aria-label={isPlaying ? 'Pause' : 'Play'}
          style={{
            width: 40,
            height: 40,
            borderRadius: 20,
            border: 'none',
            cursor: 'pointer',
            color: '#fff',
            background: theme.colors.accent,
            fontSize: 16,
            lineHeight: 1,
          }}
        >
          {isPlaying ? '❚❚' : '▶'}
        </button>
        <span style={{ color: theme.colors.text, fontSize: 13, fontVariantNumeric: 'tabular-nums' }}>
          {formatTime(displayPositionMs)} / {durationMs > 0 ? formatTime(durationMs) : '--:--'}
        </span>
      </div>
    </div>
  );
}
