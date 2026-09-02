/**
 * `FileList` (Task 81) — the per-title file table on a movie detail page.
 *
 * One row per `MediaFile`: container, video codec, resolution, HDR badge, size, and a
 * `PlayButton` (playback wired in Task 82). Every displayed field degrades gracefully
 * when the file hasn't been fully probed (nullable columns), via the `format` helpers.
 */

import type { MediaFile } from '@medi/api-client';
import { theme } from '../theme';
import { HdrBadge } from './HdrBadge';
import { PlayButton } from './PlayButton';
import { formatBytes, formatResolution, formatToken, basename } from '../lib/format';

export interface FileListProps {
  files: MediaFile[];
  /** Forwarded to each row's PlayButton; Task 82 supplies the real handler. */
  onPlay?: (fileId: number) => void;
}

export function FileList({ files, onPlay }: FileListProps) {
  if (files.length === 0) {
    return <p style={{ color: theme.colors.textMuted }}>No files on disk for this title.</p>;
  }

  return (
    <section>
      <h2 style={{ fontSize: 18, margin: '0 0 12px' }}>Files</h2>
      <div style={{ display: 'grid', gap: 8 }}>
        {files.map((file) => {
          const container = formatToken(file.container);
          const codec = formatToken(file.video_codec);
          const resolution = formatResolution(file.width, file.height);
          const size = formatBytes(file.size_bytes);

          return (
            <div
              key={file.id}
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
                  title={file.path}
                >
                  {basename(file.path)}
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
                    <span key={`${file.id}-meta-${i}`}>
                      {part}
                      {i < arr.length - 1 ? ' ·' : ''}
                    </span>
                  ))}
                  {file.hdr_type && <HdrBadge hdr={file.hdr_type} />}
                </div>
              </div>
              <PlayButton fileId={file.id} onPlay={onPlay} />
            </div>
          );
        })}
      </div>
    </section>
  );
}
