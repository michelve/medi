/**
 * `PlayButton` (Task 81) — the single Play affordance used by movie file rows and
 * series episode rows.
 *
 * Playback is wired in Task 82. Deliberately centralized here so `82` has ONE place to
 * turn a `file_id` into a stream decision + player, and every Play button in the app
 * upgrades together. Until then it renders an enabled-looking control that reports the
 * requested `fileId` via `onPlay` (default: a console stub), so the wiring is visible
 * and the layout is final.
 */

import { theme } from '../theme';

export interface PlayButtonProps {
  /** The `media_files.id` to play. Undefined when a title has no probed file yet. */
  fileId: number | null | undefined;
  /** Task 82 injects the real handler; default logs the intent. */
  onPlay?: (fileId: number) => void;
  label?: string;
}

export function PlayButton({ fileId, onPlay, label = 'Play' }: PlayButtonProps) {
  const disabled = fileId == null;
  const handleClick = () => {
    if (fileId == null) return;
    // Task 82 replaces this default with the stream-decision + player flow.
    (onPlay ?? ((id: number) => console.info(`[medi] play requested for file ${id} (wired in 82)`)))(
      fileId,
    );
  };

  return (
    <button
      type="button"
      onClick={handleClick}
      disabled={disabled}
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: 6,
        padding: '6px 14px',
        borderRadius: 6,
        border: 'none',
        fontSize: 14,
        fontWeight: 600,
        cursor: disabled ? 'not-allowed' : 'pointer',
        color: disabled ? theme.colors.textMuted : '#ffffff',
        background: disabled ? theme.colors.surface : theme.colors.accent,
        opacity: disabled ? 0.6 : 1,
      }}
      title={disabled ? 'No playable file' : label}
    >
      <span aria-hidden>▶</span>
      {label}
    </button>
  );
}
