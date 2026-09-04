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

import { useRef, useState } from 'react';
import { useParams, useNavigate } from 'react-router-dom';
import { useApi } from '../api';
import { useDetail } from '../lib/useDetail';
import { DetailHeader } from '../components/DetailHeader';
import { PlaybackBadges } from '../components/PlaybackBadges';
import { BannerActions } from '../components/BannerActions';
import { CreditsList } from '../components/CreditsList';
import { CategoryRow } from '../components/CategoryRow';
import { TrailerSection } from '../components/TrailerSection';
import { SuggestedRow } from '../components/SuggestedRow';
import { InfoDialog } from '../components/InfoDialog';
import { AboutSection } from '../components/AboutSection';
import { MatchDialog } from '../components/MatchDialog';
import { Loading, ErrorState, NotFound } from '../components/Status';
import { formatRuntime } from '../lib/format';
import { pickBestFile } from '../lib/bestFile';
import { detail } from '../theme';
import type { MovieDetail } from '@medi/api-client';

export function MovieDetailPage() {
  const { id } = useParams<{ id: string }>();
  const api = useApi();
  const navigate = useNavigate();
  // The URL param: a TMDB id for a matched movie (`/movie/98641`) or the internal id for an
  // unmatched one. It's fine to fetch `api.movie(param)` with either — the backend resolves
  // tmdb→internal. The metadata match flow, however, must use the *internal* id (from the
  // loaded detail's `movie.id`), since `/api/movies/:id/match*` are keyed by the internal id.
  const movieParam = Number(id);

  // Bump to force a re-fetch after a metadata match/refresh.
  const [nonce, setNonce] = useState(0);
  const [matchOpen, setMatchOpen] = useState(false);
  const [infoOpen, setInfoOpen] = useState(false);

  // The Trailer banner icon scrolls to the Trailers section. Declared before any early
  // return so the hook order stays stable (React #310).
  const trailersRef = useRef<HTMLDivElement>(null);

  const state = useDetail<MovieDetail>(
    (signal) => api.movie(movieParam, { signal }),
    [movieParam, nonce],
  );

  if (!Number.isFinite(movieParam)) return <NotFound message="That isn't a valid movie id." />;
  if (state.status === 'loading') return <Loading label="Loading movie…" />;
  if (state.status === 'not_found') return <NotFound message="We couldn't find that movie." />;
  if (state.status === 'error') return <ErrorState message={state.message} />;

  const movie = state.data;
  // Defensive defaults: an older/partial backend can omit these collection fields entirely.
  // Coalescing here keeps the page (and every child section) from throwing on a missing array.
  const genres = movie.genres ?? [];
  const trailers = movie.trailers ?? [];
  const credits = movie.credits ?? [];
  const collectionMovies = movie.collection_movies ?? [];
  const mediaFiles = movie.media_files ?? [];
  // The best copy drives the banner Play button + the runtime (a movie may carry several
  // files at different resolutions; `pickBestFile` picks the highest quality).
  const primaryFile = pickBestFile(mediaFiles);
  const runtime = formatRuntime(primaryFile?.duration_ms);
  // The official trailer drives the hero's autoplay-behind-the-backdrop flourish: prefer a
  // "Trailer" (over a teaser/clip), else fall back to the first trailer. Mirrors the
  // TrailerSection card's kind check.
  const officialTrailer =
    trailers.find((t) => (t.kind || 'Trailer').toLowerCase() === 'trailer') ?? trailers[0];
  // Genre · runtime · year metadata line under the title.
  const genreText = genres.map((g) => g.name).join(', ') || undefined;
  const metaParts = [genreText, runtime, movie.year != null ? String(movie.year) : undefined].filter(
    Boolean,
  ) as string[];

  const scrollTo = (ref: React.RefObject<HTMLDivElement | null>) =>
    ref.current?.scrollIntoView({ behavior: 'smooth', block: 'start' });

  // Carry the title + the file's subtitle tracks into the player so its overlay shows a
  // name and can attach WebVTT text subtitles (`docs/.tasks/90`).
  const playFile = (fileId: number) => {
    const file = mediaFiles.find((f) => f.id === fileId);
    navigate(`/play/${fileId}`, {
      state: { title: movie.title, subtitles: file?.subtitle_streams ?? [] },
    });
  };

  return (
    // Figma "Movie Details": a #26262a→#131922 vertical gradient behind the whole page. It's
    // painted as a fixed full-viewport layer (not on this article) so it spans the entire body
    // even though the content column is capped at `detail.maxWidth` — otherwise the gradient
    // would only fill the centered column and leave the shell's flat bg showing on the sides.
    <article style={{ position: 'relative', minHeight: 'calc(100vh - 65px)' }}>
      <div
        aria-hidden="true"
        style={{
          position: 'fixed',
          inset: 0,
          zIndex: 0,
          background: detail.pageGradient,
          pointerEvents: 'none',
        }}
      />
      {/* A faint film-grain texture over the gradient (Figma), so the body reads as textured
          rather than a flat wash. */}
      <div
        aria-hidden="true"
        style={{
          position: 'fixed',
          inset: 0,
          zIndex: 0,
          backgroundImage: detail.grainUrl,
          opacity: detail.grainOpacity,
          pointerEvents: 'none',
        }}
      />
      {/* All page content sits above the fixed gradient + grain layers. */}
      <div style={{ position: 'relative', zIndex: 1 }}>
      {/* Banner: taller hero. Under the title sits a genre · runtime · year line plus quality
          badges, then Play / Trailer / Fix-match actions and the (expandable) summary. The
          Trailers row is fused to the card bottom as a footer shelf (Figma `trailers_scenes`),
          so the Trailer action scrolls to the banner rather than a detached section. */}
      <div ref={trailersRef}>
        <DetailHeader
          title={movie.title}
          /* Prefer the fanart.tv wallpaper on the hero (Task 95); fall back to the TMDB
             backdrop, then to nothing. */
          backdropUrl={api.imageUrl(movie.wallpaper_path) ?? api.imageUrl(movie.backdrop_path)}
          logoUrl={api.imageUrl(movie.logo_path)}
          trailerYoutubeKey={officialTrailer?.youtube_key}
          minHeight={520}
          layout="hero"
          meta={
            <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
              {metaParts.length > 0 && (
                <span style={{ fontSize: 14, lineHeight: '21px', color: detail.text.secondary }}>
                  {metaParts.join('   ·   ')}
                </span>
              )}
              <PlaybackBadges files={mediaFiles} />
            </div>
          }
          footer={trailers.length > 0 ? <TrailerSection trailers={trailers} /> : undefined}
        >
          {/* Full synopsis on the banner — Figma shows the whole thing, no "more" toggle. */}
          {movie.overview && (
            <p
              style={{
                margin: 0,
                maxWidth: 470,
                fontSize: 14,
                lineHeight: '21px',
                color: detail.text.primary,
              }}
            >
              {movie.overview}
            </p>
          )}
          <BannerActions
            canPlay={primaryFile != null}
            onPlay={() => primaryFile && playFile(primaryFile.id)}
            hasTrailer={trailers.length > 0}
            onTrailer={() => scrollTo(trailersRef)}
            onInfo={() => setInfoOpen(true)}
            onFixMatch={() => setMatchOpen(true)}
          />
        </DetailHeader>
      </div>

      {/* Section order (Task 91): collection → suggestions → cast & crew → about. Trailers
          live in the banner footer above. The file/version list lives in the banner's Info
          dialog, not the page body. `minWidth: 0` lets the scrolling rows shrink rather than
          push the page wide. Figma stacks the content sections 64px apart. */}
      <div
        style={{
          display: 'grid',
          gridTemplateColumns: 'minmax(0, 1fr)',
          gap: detail.sectionGap,
          minWidth: 0,
        }}
      >
        {movie.collection && collectionMovies.length > 0 && (
          <CategoryRow
            captionless
            row={{
              key: `collection:${movie.collection.id}`,
              title: movie.collection.name,
              items: collectionMovies,
            }}
          />
        )}

        <SuggestedRow credits={credits} excludeKind="movie" excludeId={movie.id} captionless />

        <CreditsList credits={credits} />

        <AboutSection movie={movie} />
      </div>
      </div>
      {/* /content above gradient */}

      {infoOpen && (
        <InfoDialog movie={movie} onPlay={playFile} onClose={() => setInfoOpen(false)} />
      )}

      {matchOpen && (
        <MatchDialog
          movieId={movie.id}
          initialQuery={movie.title}
          onClose={() => setMatchOpen(false)}
          onMatched={() => setNonce((n) => n + 1)}
        />
      )}
    </article>
  );
}
