/**
 * `InfoDialog` (Task 91) — the banner "info" modal listing a movie's files and quality
 * versions.
 *
 * Opened from the banner's info icon. Shows the grouped `FileList` (the best copy plus a
 * resolution switcher when several exist) inside a centered modal so the page body stays
 * clean — the files/versions live here instead of a standalone page section. Playing a
 * version closes the dialog and navigates via the supplied `onPlay`.
 */

import type { MovieDetail } from '@medi/api-client';
import { theme } from '../theme';
import { FileList } from './FileList';

export function InfoDialog({
  movie,
  onPlay,
  onClose,
}: {
  movie: MovieDetail;
  onPlay: (fileId: number) => void;
  onClose: () => void;
}) {
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={`${movie.title} — files and versions`}
      onClick={onClose}
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 1000,
        background: 'rgba(0,0,0,0.7)',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        padding: 24,
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          width: 'min(680px, 100%)',
          maxHeight: '85vh',
          overflowY: 'auto',
          background: theme.colors.background,
          border: `1px solid ${theme.colors.surface}`,
          borderRadius: 12,
          padding: 24,
          position: 'relative',
        }}
      >
        <button
          type="button"
          onClick={onClose}
          aria-label="Close"
          style={{
            position: 'absolute',
            top: 12,
            right: 12,
            width: 32,
            height: 32,
            borderRadius: '50%',
            border: 0,
            background: theme.colors.surface,
            color: theme.colors.text,
            fontSize: 18,
            lineHeight: 1,
            cursor: 'pointer',
          }}
        >
          ×
        </button>

        <h2 style={{ fontSize: 20, fontWeight: 700, margin: '0 0 4px', color: theme.colors.text }}>
          {movie.title}
        </h2>
        <p style={{ fontSize: 13, color: theme.colors.textMuted, margin: '0 0 20px' }}>
          Available files &amp; quality versions
        </p>

        <FileList
          files={movie.media_files}
          onPlay={(fileId) => {
            onClose();
            onPlay(fileId);
          }}
        />
      </div>
    </div>
  );
}
