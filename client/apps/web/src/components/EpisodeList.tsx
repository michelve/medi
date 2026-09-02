/**
 * `EpisodeList` (Task 81, playable in 82) — one season's episodes on a series detail page.
 *
 * Rows show episode number, title and overview with a per-episode `PlayButton`. Each
 * episode now carries its `media_files` (`EpisodeWithFiles`), so the button plays the
 * episode's primary file — its `id` is the `file_id` handed to `GET /api/stream`. An
 * episode with no probed file yet renders the button in its disabled state.
 */

import type { SeasonWithEpisodes } from '@medi/api-client';
import { theme } from '../theme';
import { PlayButton } from './PlayButton';

export interface EpisodeListProps {
  season: SeasonWithEpisodes;
  /** Forwarded to each row's PlayButton; Task 82 supplies the real handler. */
  onPlay?: (fileId: number) => void;
}

export function EpisodeList({ season, onPlay }: EpisodeListProps) {
  return (
    <section>
      <h2 style={{ fontSize: 18, margin: '0 0 12px' }}>Season {season.season_number}</h2>
      <div style={{ display: 'grid', gap: 8 }}>
        {season.episodes.map((ep) => {
          // The episode's primary file drives playback; undefined until probed.
          const fileId = ep.media_files[0]?.id;
          return (
            <div
              key={ep.id}
              style={{
                display: 'flex',
                alignItems: 'flex-start',
                gap: 16,
                padding: '12px 14px',
                borderRadius: 8,
                background: theme.colors.surface,
              }}
            >
              <div
                style={{
                  flex: '0 0 auto',
                  width: 32,
                  textAlign: 'right',
                  fontSize: 15,
                  fontWeight: 700,
                  color: theme.colors.textMuted,
                  lineHeight: 1.4,
                }}
              >
                {ep.episode_number}
              </div>
              <div style={{ flex: '1 1 auto', minWidth: 0 }}>
                <div style={{ fontSize: 15, color: theme.colors.text }}>
                  {ep.title ?? `Episode ${ep.episode_number}`}
                </div>
                {ep.overview && (
                  <p
                    style={{
                      margin: '4px 0 0',
                      fontSize: 13,
                      lineHeight: 1.45,
                      color: theme.colors.textMuted,
                    }}
                  >
                    {ep.overview}
                  </p>
                )}
              </div>
              <div style={{ flex: '0 0 auto' }}>
                <PlayButton fileId={fileId} onPlay={onPlay} />
              </div>
            </div>
          );
        })}
      </div>
    </section>
  );
}
