/**
 * `FileList` (Task 81, multi-resolution grouping) — the "Versions" block on a movie detail
 * page.
 *
 * All of a movie's `media_files` belong to one title, so they're presented as a single group:
 * a primary entry describing the **best** copy (highest resolution / HDR / bitrate, via
 * `pickBestFile`) with a Play button that plays it. When more than one copy exists, a
 * resolution switcher lists every copy as a chip (e.g. `4K DV`, `HD`) so the user can play
 * a specific resolution; the best one is marked. Every field degrades gracefully for unprobed
 * files (nullable columns) via the `format` helpers.
 */

import type { MediaFile } from '@medi/api-client';
import { theme } from '../theme';
import { HdrBadge, hdrLabel } from './HdrBadge';
import { PlayButton } from './PlayButton';
import { pickBestFile } from '../lib/bestFile';
import {
  formatBytes,
  formatResolution,
  formatToken,
  resolutionLabel,
  basename,
} from '../lib/format';

export interface FileListProps {
  files: MediaFile[];
  /** Forwarded to the Play controls; the page supplies the real navigation handler. */
  onPlay?: (fileId: number) => void;
}

/** A short label for a version chip: `4K DV` / `HD` / the filename when unprobed. */
function versionLabel(file: MediaFile): string {
  const res = resolutionLabel(file.width, file.height);
  const hdr = hdrLabel(file.hdr_type);
  if (res) return [res, hdr].filter(Boolean).join(' ');
  return basename(file.path);
}

export function FileList({ files, onPlay }: FileListProps) {
  if (files.length === 0) {
    return <p style={{ color: theme.colors.textMuted }}>No files on disk for this title.</p>;
  }

  const best = pickBestFile(files)!;
  const multiple = files.length > 1;
  // Chips list the best first, then the rest in best-first order (the array already arrives
  // best-first from the backend, but sort defensively so the order is stable regardless).
  const ordered = [best, ...files.filter((f) => f.id !== best.id)];

  const container = formatToken(best.container);
  const codec = formatToken(best.video_codec);
  const resolution = formatResolution(best.width, best.height);
  const size = formatBytes(best.size_bytes);

  return (
    <section>
      <h2 style={{ fontSize: 18, margin: '0 0 12px' }}>{multiple ? 'Versions' : 'File'}</h2>

      {/* Primary entry — the best copy. */}
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 12,
          flexWrap: 'wrap',
          padding: '12px 14px',
          borderRadius: 8,
          background: theme.colors.surface,
        }}
      >
        <div style={{ flex: '1 1 220px', minWidth: 0 }}>
          <div
            style={{
              fontSize: 14,
              color: theme.colors.text,
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              whiteSpace: 'nowrap',
            }}
            title={best.path}
          >
            {basename(best.path)}
          </div>
          <div
            style={{
              marginTop: 4,
              display: 'flex',
              alignItems: 'center',
              gap: 8,
              flexWrap: 'wrap',
              fontSize: 13,
              color: theme.colors.textMuted,
            }}
          >
            {[container, codec, resolution, size].filter(Boolean).map((part, i, arr) => (
              <span key={`best-meta-${i}`}>
                {part}
                {i < arr.length - 1 ? ' ·' : ''}
              </span>
            ))}
            {best.hdr_type && <HdrBadge hdr={best.hdr_type} />}
          </div>
        </div>
        <PlayButton fileId={best.id} onPlay={onPlay} />
      </div>

      {/* Resolution switcher — only when there's more than one copy. */}
      {multiple && (
        <div style={{ marginTop: 12 }}>
          <div style={{ fontSize: 13, color: theme.colors.textMuted, marginBottom: 8 }}>
            Play another version
          </div>
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8 }}>
            {ordered.map((file) => {
              const isBest = file.id === best.id;
              return (
                <button
                  key={file.id}
                  type="button"
                  onClick={() => onPlay?.(file.id)}
                  title={file.path}
                  style={{
                    display: 'inline-flex',
                    alignItems: 'center',
                    gap: 8,
                    padding: '6px 12px',
                    borderRadius: 6,
                    border: `1px solid ${isBest ? theme.colors.accent : theme.colors.surface}`,
                    background: theme.colors.surface,
                    color: theme.colors.text,
                    fontSize: 13,
                    cursor: 'pointer',
                  }}
                >
                  <svg width="11" height="11" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
                    <path d="M8 5v14l11-7z" />
                  </svg>
                  {versionLabel(file)}
                  {isBest && (
                    <span style={{ fontSize: 11, fontWeight: 700, color: theme.colors.accent }}>
                      Best
                    </span>
                  )}
                  {formatBytes(file.size_bytes) && (
                    <span style={{ color: theme.colors.textMuted }}>
                      {formatBytes(file.size_bytes)}
                    </span>
                  )}
                </button>
              );
            })}
          </div>
        </div>
      )}
    </section>
  );
}
