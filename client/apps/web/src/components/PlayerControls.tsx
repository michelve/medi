/**
 * `PlayerControls` (Task 82; extended by `docs/.tasks/97` Part B) — the DOM transport overlay.
 *
 * A presentational layer over the `<video>`, driven by the shared `usePlayerControls`
 * reducer (from `@medi/player/usePlayerControls`, the same state machine the TV player
 * uses). It renders play/pause, ±10s skip, the current-time / duration clock, the `ScrubBar`,
 * a **volume slider + mute**, a **fullscreen** toggle, and **audio-track** + **subtitles** menu
 * buttons.
 *
 * ## Look — iOS-style "glass"
 * The control cluster sits on a translucent, blurred glass bar (`backdrop-filter: blur()
 * saturate()`) with hairline white borders, rather than flat solid fills — so the picture reads
 * through the chrome. Buttons are line-art SVG icons for a crisp, platform-neutral look.
 *
 * ## Web-local vs shared state (`docs/.tasks/97` design note)
 * Volume, mute, and fullscreen are **browser-only** concepts, so they live as component state
 * HERE — NOT in the cross-platform `usePlayerControls` reducer. Volume/mute bind directly to
 * `video.volume` / `video.muted` and persist in `localStorage`; fullscreen uses the real
 * Fullscreen API on the player container.
 *
 * The overlay auto-hides via the reducer's timer; we fade it with opacity so it can't trap
 * pointer events while hidden.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import type { PlayerControls as Controls } from '@medi/player/usePlayerControls';
import type { TrickplayMeta } from '@medi/player/trickplay';
import type { FileAudioTrack } from '@medi/api-client';
import { ScrubBar } from './ScrubBar';
import { Icon } from './PlayerIcons';
import { formatTime } from '../lib/format';
import { theme } from '../theme';

/** `localStorage` key for the persisted volume + muted flag (`docs/.tasks/97` Part B). */
const VOLUME_KEY = 'medi.player.volume';

/** Shared "glass" surface — translucent, blurred, hairline-bordered (iOS-style). */
const glass: React.CSSProperties = {
  background: 'rgba(28,28,32,0.42)',
  backdropFilter: 'blur(22px) saturate(160%)',
  WebkitBackdropFilter: 'blur(22px) saturate(160%)',
  border: '1px solid rgba(255,255,255,0.14)',
};

export interface PlayerControlsProps {
  controls: Controls;
  title: string;
  trickplay?: TrickplayMeta;
  /** Seek to an absolute position (ms) — from a ScrubBar click. */
  onSeek: (positionMs: number) => void;
  /** The `<video>` element, for binding volume / mute directly. */
  video: HTMLVideoElement | null;
  /** The player container to request fullscreen on (the fixed full-viewport root). */
  fullscreenTarget: HTMLElement | null;
  /** The file's audio tracks for the audio menu; a single track ⇒ the button is hidden. */
  audioTracks?: FileAudioTrack[];
  /** The active track's `stream_index` (highlighted in the menu). */
  activeAudioTrack?: number;
  /** Switch to a source audio track by its `stream_index`. */
  onSelectAudio?: (streamIndex: number) => void;
  /** A subtitles-menu slot (`docs/.tasks/99`); rendered as a disabled placeholder when absent. */
  subtitlesMenu?: React.ReactNode;
}

export function PlayerControls({
  controls,
  title,
  trickplay,
  onSeek,
  video,
  fullscreenTarget,
  audioTracks,
  activeAudioTrack,
  onSelectAudio,
  subtitlesMenu,
}: PlayerControlsProps) {
  const { overlayVisible, isPlaying, displayPositionMs, durationMs, handleRemote } = controls;

  return (
    <div
      style={{
        position: 'absolute',
        inset: 0,
        display: 'flex',
        flexDirection: 'column',
        justifyContent: 'flex-end',
        padding: 24,
        gap: 16,
        // A soft bottom scrim so the title/scrub read against a bright frame, on top of the
        // glass bar below.
        background: 'linear-gradient(to top, rgba(0,0,0,0.55), rgba(0,0,0,0) 40%)',
        opacity: overlayVisible ? 1 : 0,
        transition: 'opacity 220ms ease',
        pointerEvents: overlayVisible ? 'auto' : 'none',
      }}
    >
      <div
        style={{
          color: '#fff',
          fontSize: 18,
          fontWeight: 600,
          letterSpacing: 0.2,
          textShadow: '0 1px 8px rgba(0,0,0,0.5)',
        }}
      >
        {title}
      </div>

      {/* The whole transport cluster on one glass bar. */}
      <div
        style={{
          ...glass,
          borderRadius: 18,
          padding: '14px 18px',
          display: 'flex',
          flexDirection: 'column',
          gap: 14,
          boxShadow: '0 8px 32px rgba(0,0,0,0.38)',
        }}
      >
        <ScrubBar
          positionMs={displayPositionMs}
          durationMs={durationMs}
          onSeek={onSeek}
          trickplay={trickplay}
        />

        <div style={{ display: 'flex', alignItems: 'center', gap: 14 }}>
          <GlassButton label="Skip back 10 seconds" onClick={() => handleRemote('left')}>
            <Icon name="back10" />
          </GlassButton>

          <button
            type="button"
            onClick={() => handleRemote('playPause')}
            aria-label={isPlaying ? 'Pause' : 'Play'}
            style={{
              width: 48,
              height: 48,
              borderRadius: 24,
              border: 'none',
              cursor: 'pointer',
              color: '#fff',
              background: theme.colors.accent,
              display: 'inline-flex',
              alignItems: 'center',
              justifyContent: 'center',
              boxShadow: '0 4px 16px rgba(0,0,0,0.35)',
            }}
          >
            <Icon name={isPlaying ? 'pause' : 'play'} size={20} />
          </button>

          <GlassButton label="Skip forward 10 seconds" onClick={() => handleRemote('right')}>
            <Icon name="forward10" />
          </GlassButton>

          <span
            style={{
              color: 'rgba(255,255,255,0.92)',
              fontSize: 13,
              fontVariantNumeric: 'tabular-nums',
              marginLeft: 8,
              letterSpacing: 0.3,
            }}
          >
            {formatTime(displayPositionMs)}
            <span style={{ color: 'rgba(255,255,255,0.5)' }}>
              {' / '}
              {durationMs > 0 ? formatTime(durationMs) : '--:--'}
            </span>
          </span>

          {/* Right-aligned cluster: volume, audio, subtitles, fullscreen. */}
          <div style={{ marginLeft: 'auto', display: 'flex', alignItems: 'center', gap: 14 }}>
            <VolumeControl video={video} />

            {audioTracks && audioTracks.length > 1 && (
              <AudioMenu
                tracks={audioTracks}
                active={activeAudioTrack}
                onSelect={(idx) => onSelectAudio?.(idx)}
              />
            )}

            {/* Subtitles menu slot — `99` fills it; a placeholder keeps the bar layout stable. */}
            {subtitlesMenu ?? (
              <GlassButton label="Subtitles (not available yet)" onClick={() => undefined} disabled>
                <Icon name="subtitles" />
              </GlassButton>
            )}

            <FullscreenButton target={fullscreenTarget} />
          </div>
        </div>
      </div>
    </div>
  );
}

/** A translucent glass icon button — the standard control affordance. */
function GlassButton({
  label,
  onClick,
  disabled,
  active,
  children,
}: {
  label: string;
  onClick: () => void;
  disabled?: boolean;
  active?: boolean;
  children: React.ReactNode;
}) {
  const [hover, setHover] = useState(false);
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      disabled={disabled}
      style={{
        height: 40,
        minWidth: 40,
        padding: '0 10px',
        borderRadius: 12,
        border: `1px solid ${active ? 'rgba(255,255,255,0.5)' : 'rgba(255,255,255,0.16)'}`,
        background: active
          ? 'rgba(255,255,255,0.22)'
          : hover && !disabled
          ? 'rgba(255,255,255,0.14)'
          : 'rgba(255,255,255,0.06)',
        backdropFilter: 'blur(8px)',
        WebkitBackdropFilter: 'blur(8px)',
        color: disabled ? 'rgba(255,255,255,0.4)' : '#fff',
        cursor: disabled ? 'default' : 'pointer',
        display: 'inline-flex',
        alignItems: 'center',
        justifyContent: 'center',
        transition: 'background 120ms ease, border-color 120ms ease',
      }}
    >
      {children}
    </button>
  );
}

/**
 * Volume slider + mute toggle (`docs/.tasks/97` Part B). Web-local state, bound to
 * `video.volume` / `video.muted`, persisted in `localStorage` (restored on mount).
 */
function VolumeControl({ video }: { video: HTMLVideoElement | null }) {
  const [volume, setVolume] = useState(1);
  const [muted, setMuted] = useState(false);
  const restored = useRef(false);

  // Restore the persisted volume/mute once.
  useEffect(() => {
    if (restored.current) return;
    restored.current = true;
    try {
      const raw = localStorage.getItem(VOLUME_KEY);
      if (raw) {
        const saved = JSON.parse(raw) as { volume?: number; muted?: boolean };
        if (typeof saved.volume === 'number') setVolume(Math.min(1, Math.max(0, saved.volume)));
        if (typeof saved.muted === 'boolean') setMuted(saved.muted);
      }
    } catch {
      // Ignore malformed / unavailable storage — fall back to defaults.
    }
  }, []);

  // Apply state → element whenever either changes (or the element is (re)attached).
  useEffect(() => {
    if (!video) return;
    video.volume = volume;
    video.muted = muted;
  }, [video, volume, muted]);

  const persist = useCallback((v: number, m: boolean) => {
    try {
      localStorage.setItem(VOLUME_KEY, JSON.stringify({ volume: v, muted: m }));
    } catch {
      // Non-fatal: a private window / disabled storage just doesn't persist.
    }
  }, []);

  const onSlider = (v: number) => {
    const m = v === 0;
    setVolume(v);
    setMuted(m);
    persist(v, m);
  };
  const toggleMute = () => {
    const m = !muted;
    setMuted(m);
    persist(volume, m);
  };

  const iconName = muted || volume === 0 ? 'volumeMute' : volume < 0.5 ? 'volumeLow' : 'volumeHigh';

  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
      <GlassButton label={muted ? 'Unmute' : 'Mute'} onClick={toggleMute}>
        <Icon name={iconName} />
      </GlassButton>
      <input
        type="range"
        min={0}
        max={1}
        step={0.05}
        value={muted ? 0 : volume}
        aria-label="Volume"
        onChange={(e) => onSlider(Number(e.currentTarget.value))}
        style={{ width: 96, accentColor: theme.colors.accent, cursor: 'pointer' }}
      />
    </div>
  );
}

/**
 * The real Fullscreen API toggle (`docs/.tasks/97` Part B) — distinct from the Part A CSS
 * full-viewport layout. Requests fullscreen on the player container and reflects state from
 * the `fullscreenchange` event.
 */
function FullscreenButton({ target }: { target: HTMLElement | null }) {
  const [isFull, setIsFull] = useState(false);

  useEffect(() => {
    const onChange = () => setIsFull(document.fullscreenElement != null);
    document.addEventListener('fullscreenchange', onChange);
    onChange();
    return () => document.removeEventListener('fullscreenchange', onChange);
  }, []);

  const toggle = () => {
    if (document.fullscreenElement) {
      void document.exitFullscreen().catch(() => undefined);
    } else if (target) {
      void target.requestFullscreen().catch(() => undefined);
    }
  };

  return (
    <GlassButton label={isFull ? 'Exit fullscreen' : 'Enter fullscreen'} onClick={toggle} active={isFull}>
      <Icon name={isFull ? 'fullscreenExit' : 'fullscreen'} />
    </GlassButton>
  );
}

/**
 * Audio-track menu (`docs/.tasks/97` Part C): lists each track with a human label
 * (`title || language || "Track N"` + channel layout) and switches on click. A glass popover
 * anchored above the button; closes on outside click / Escape / a selection.
 */
function AudioMenu({
  tracks,
  active,
  onSelect,
}: {
  tracks: FileAudioTrack[];
  active?: number;
  onSelect: (streamIndex: number) => void;
}) {
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
    // Capture so we swallow Escape before the page's back-navigation handler sees it.
    document.addEventListener('keydown', onKey, true);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey, true);
    };
  }, [open]);

  return (
    <div ref={ref} style={{ position: 'relative' }}>
      <GlassButton label="Audio track" onClick={() => setOpen((v) => !v)} active={open}>
        <Icon name="audio" />
      </GlassButton>
      {open && (
        <div
          role="menu"
          style={{
            ...glass,
            position: 'absolute',
            bottom: 50,
            right: 0,
            minWidth: 220,
            maxHeight: 280,
            overflowY: 'auto',
            borderRadius: 14,
            padding: 8,
            boxShadow: '0 12px 40px rgba(0,0,0,0.5)',
          }}
        >
          <div
            style={{
              padding: '4px 12px 8px',
              color: 'rgba(255,255,255,0.55)',
              fontSize: 11,
              textTransform: 'uppercase',
              letterSpacing: 0.6,
            }}
          >
            Audio
          </div>
          {tracks.map((t, i) => {
            const isActive = active != null ? t.stream_index === active : t.is_default;
            return (
              <MenuItem
                key={t.stream_index}
                active={isActive}
                onClick={() => {
                  onSelect(t.stream_index);
                  setOpen(false);
                }}
                label={audioTrackLabel(t, i)}
              />
            );
          })}
        </div>
      )}
    </div>
  );
}

/** One selectable row in a glass popover menu. */
function MenuItem({ active, onClick, label }: { active: boolean; onClick: () => void; label: string }) {
  const [hover, setHover] = useState(false);
  return (
    <button
      type="button"
      role="menuitemradio"
      aria-checked={active}
      onClick={onClick}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      style={{
        display: 'flex',
        width: '100%',
        alignItems: 'center',
        gap: 10,
        padding: '9px 12px',
        borderRadius: 9,
        border: 'none',
        background: active ? 'rgba(255,255,255,0.14)' : hover ? 'rgba(255,255,255,0.08)' : 'transparent',
        color: '#fff',
        fontSize: 13,
        textAlign: 'left',
        cursor: 'pointer',
        transition: 'background 100ms ease',
      }}
    >
      <span style={{ width: 16, color: theme.colors.accent, display: 'inline-flex' }}>
        {active ? <Icon name="check" size={16} /> : null}
      </span>
      <span>{label}</span>
    </button>
  );
}

/**
 * A menu label for an audio track: `title || language || "Track N"`, with a channel-layout
 * suffix (`5.1`, `Stereo`) when known — e.g. `English · 5.1` (`docs/.tasks/97` Part C).
 */
function audioTrackLabel(t: FileAudioTrack, index: number): string {
  const name = t.title || languageName(t.language) || `Track ${index + 1}`;
  const layout = channelLabel(t.channel_layout, t.channels);
  return layout ? `${name} · ${layout}` : name;
}

/** Uppercase a bare ISO language tag as a fallback name (`eng` → `ENG`). */
function languageName(lang: string | null | undefined): string | undefined {
  return lang ? lang.toUpperCase() : undefined;
}

/** A friendly channel-layout string from a `channel_layout` and/or channel count. */
function channelLabel(layout: string | null | undefined, channels: number | null | undefined): string | undefined {
  if (layout) {
    const m = layout.match(/^\d(?:\.\d)?/);
    if (m) return m[0];
    if (layout === 'stereo') return 'Stereo';
    if (layout === 'mono') return 'Mono';
    return layout;
  }
  if (channels === 1) return 'Mono';
  if (channels === 2) return 'Stereo';
  if (channels === 6) return '5.1';
  if (channels === 8) return '7.1';
  return channels ? `${channels}ch` : undefined;
}
