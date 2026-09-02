/**
 * `SeriesDetailPage` (Task 81) — `/series/:id`.
 *
 * `client.series(id)` → shared `DetailHeader` like a movie, then each `SeasonWithEpisodes`
 * rendered as an `EpisodeList` (number, title, overview, per-episode Play — wired in 82),
 * followed by `CreditsList`. A 404 renders the shared `NotFound`.
 */

import { useParams, useNavigate } from 'react-router-dom';
import { useApi } from '../api';
import { useDetail } from '../lib/useDetail';
import { DetailHeader } from '../components/DetailHeader';
import { EpisodeList } from '../components/EpisodeList';
import { CreditsList } from '../components/CreditsList';
import { Loading, ErrorState, EmptyState, NotFound } from '../components/Status';
import type { SeriesDetail } from '@medi/api-client';

export function SeriesDetailPage() {
  const { id } = useParams<{ id: string }>();
  const api = useApi();
  const navigate = useNavigate();
  const seriesId = Number(id);

  const state = useDetail<SeriesDetail>(
    (signal) => api.series(seriesId, { signal }),
    [seriesId],
  );

  if (!Number.isFinite(seriesId)) return <NotFound message="That isn't a valid series id." />;
  if (state.status === 'loading') return <Loading label="Loading series…" />;
  if (state.status === 'not_found') return <NotFound message="We couldn't find that series." />;
  if (state.status === 'error') return <ErrorState message={state.message} />;

  const series = state.data;
  // Seasons in order; episodes are already ordered by the backend.
  const seasons = [...series.seasons].sort((a, b) => a.season_number - b.season_number);
  const playFile = (fileId: number) =>
    navigate(`/play/${fileId}`, { state: { title: series.title } });

  return (
    <article>
      <DetailHeader
        title={series.title}
        year={series.year}
        overview={series.overview}
        backdropUrl={api.imageUrl(series.backdrop_path)}
      />

      <div style={{ display: 'grid', gap: 32 }}>
        {seasons.length === 0 ? (
          <EmptyState>No seasons on disk for this series yet.</EmptyState>
        ) : (
          seasons.map((season) => (
            <EpisodeList key={season.id} season={season} onPlay={playFile} />
          ))
        )}
        <CreditsList credits={series.credits} />
      </div>
    </article>
  );
}
