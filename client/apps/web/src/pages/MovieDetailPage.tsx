/**
 * `MovieDetailPage` (Task 81 + 82) — `/movie/:id`.
 *
 * `client.movie(id)` → shared `DetailHeader` (backdrop + title/year/overview), an HDR
 * badge for the file's tier, then `CreditsList` and `FileList` (each file row's Play button
 * navigates to `/play/:fileId` — Task 82). A 404 renders the shared `NotFound`.
 *
 * Task 82 adds the "Fix match" flow: a header button opens `MatchDialog`; on a successful
 * pin/refresh the page re-fetches (nonce bump) so the new poster/overview show.
 */

import { useState } from 'react';
import { useParams, useNavigate } from 'react-router-dom';
import { useApi } from '../api';
import { useDetail } from '../lib/useDetail';
import { DetailHeader } from '../components/DetailHeader';
import { HdrBadge } from '../components/HdrBadge';
import { CreditsList } from '../components/CreditsList';
import { FileList } from '../components/FileList';
import { MatchDialog } from '../components/MatchDialog';
import { Loading, ErrorState, NotFound } from '../components/Status';
import { theme } from '../theme';
import type { MovieDetail } from '@medi/api-client';

export function MovieDetailPage() {
  const { id } = useParams<{ id: string }>();
  const api = useApi();
  const navigate = useNavigate();
  const movieId = Number(id);

  // Bump to force a re-fetch after a metadata match/refresh.
  const [nonce, setNonce] = useState(0);
  const [matchOpen, setMatchOpen] = useState(false);

  const state = useDetail<MovieDetail>(
    (signal) => api.movie(movieId, { signal }),
    [movieId, nonce],
  );

  if (!Number.isFinite(movieId)) return <NotFound message="That isn't a valid movie id." />;
  if (state.status === 'loading') return <Loading label="Loading movie…" />;
  if (state.status === 'not_found') return <NotFound message="We couldn't find that movie." />;
  if (state.status === 'error') return <ErrorState message={state.message} />;

  const movie = state.data;
  // Highest HDR tier across the movie's files, for the header badge.
  const hdr = movie.media_files.find((f) => f.hdr_type)?.hdr_type ?? undefined;
  // Carry the title into the player so its overlay shows a name, not the file id.
  const playFile = (fileId: number) =>
    navigate(`/play/${fileId}`, { state: { title: movie.title } });

  return (
    <article>
      <DetailHeader
        title={movie.title}
        year={movie.year}
        overview={movie.overview}
        backdropUrl={api.imageUrl(movie.backdrop_path)}
      >
        {hdr && <HdrBadge hdr={hdr} />}
        <button
          type="button"
          onClick={() => setMatchOpen(true)}
          style={{
            padding: '6px 14px',
            borderRadius: 6,
            border: `1px solid ${theme.colors.textMuted}`,
            background: 'rgba(0,0,0,0.35)',
            color: theme.colors.text,
            fontSize: 13,
            fontWeight: 600,
            cursor: 'pointer',
          }}
        >
          Fix match
        </button>
      </DetailHeader>

      <div style={{ display: 'grid', gap: 32 }}>
        <FileList files={movie.media_files} onPlay={playFile} />
        <CreditsList credits={movie.credits} />
      </div>

      {matchOpen && (
        <MatchDialog
          movieId={movieId}
          initialQuery={movie.title}
          onClose={() => setMatchOpen(false)}
          onMatched={() => setNonce((n) => n + 1)}
        />
      )}
    </article>
  );
}
