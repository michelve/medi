/**
 * `ResumeChip` (Task 98) — a small non-blocking chip the player shows when a title resumes
 * from a saved position. It states where it resumed from and offers "Start over"; if the
 * viewer ignores it, it auto-dismisses and playback simply continues from the resumed spot
 * (the seek is already seeded by the player). It never blocks playback.
 *
 * Styled to match the player's glass control affordances (the Back button / toast).
 */

interface ResumeChipProps {
  /** Label, e.g. `"Resuming from 5:00"`. */
  label: string;
  /** Restart from the beginning. */
  onStartOver: () => void;
  /** Dismiss the chip (keep playing from the resumed position). */
  onDismiss: () => void;
}

export function ResumeChip({ label, onStartOver, onDismiss }: ResumeChipProps) {
  return (
    <div
      style={{
        position: 'absolute',
        bottom: 108,
        left: '50%',
        transform: 'translateX(-50%)',
        zIndex: 20,
        display: 'inline-flex',
        alignItems: 'center',
        gap: 14,
        padding: '10px 12px 10px 18px',
        borderRadius: 14,
        background: 'rgba(28,28,32,0.5)',
        backdropFilter: 'blur(22px) saturate(160%)',
        WebkitBackdropFilter: 'blur(22px) saturate(160%)',
        border: '1px solid rgba(255,255,255,0.14)',
        color: '#fff',
        fontSize: 14,
        boxShadow: '0 4px 16px rgba(0,0,0,0.3)',
      }}
    >
      <span style={{ opacity: 0.92 }}>{label}</span>
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          onStartOver();
        }}
        style={{
          appearance: 'none',
          border: '1px solid rgba(255,255,255,0.2)',
          background: 'rgba(255,255,255,0.06)',
          color: '#fff',
          fontSize: 13,
          fontWeight: 600,
          padding: '6px 12px',
          borderRadius: 9,
          cursor: 'pointer',
        }}
      >
        Start over
      </button>
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          onDismiss();
        }}
        aria-label="Dismiss"
        style={{
          appearance: 'none',
          border: 'none',
          background: 'transparent',
          color: 'rgba(255,255,255,0.7)',
          fontSize: 18,
          lineHeight: 1,
          padding: '2px 6px',
          cursor: 'pointer',
        }}
      >
        ×
      </button>
    </div>
  );
}
