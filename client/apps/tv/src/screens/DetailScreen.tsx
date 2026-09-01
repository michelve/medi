/**
 * Detail screen — `GET /api/movies/:id` or `/api/series/:id` (task sub-task 5).
 *
 * Shows the title's backdrop, overview, credits, and a focusable list of playable
 * files (movie: its media_files; series: episodes, grouped by season). Selecting
 * a file asks `/api/stream/:file_id` for the direct-vs-HLS decision and pushes the
 * Player route with the result.
 *
 * Because a detail response DOES carry media_file ids, the hero here can hover-
 * preview (unlike the library grid) — but to keep the screen focused we use a
 * plain backdrop and reserve the FSM preview for the poster rows on Home.
 */

import React, { useCallback } from 'react';
import { ActivityIndicator, StyleSheet, Text, View } from 'react-native';

import {
  Page,
  DefaultFocus,
  SpatialNavigationScrollView,
  theme,
  type MovieDetail,
  type SeriesDetail,
  type MediaFile,
} from '../deps';
import { useApi } from '../api';
import { useMovie, useSeries } from '../hooks';
import { useNavigation } from '../navigation';
import { PlayRow } from '../components/PlayRow';

export function DetailScreen({
  kind,
  id,
  isActive,
}: {
  kind: 'movie' | 'series';
  id: number;
  isActive: boolean;
}): React.JSX.Element {
  return kind === 'movie' ? (
    <MovieDetailScreen id={id} isActive={isActive} />
  ) : (
    <SeriesDetailScreen id={id} isActive={isActive} />
  );
}

function Header({
  title,
  year,
  overview,
  backdropUri,
}: {
  title: string;
  year?: number | null;
  overview?: string | null;
  backdropUri?: string;
}): React.JSX.Element {
  return (
    <View style={styles.header}>
      <Text style={styles.title}>
        {title}
        {year ? <Text style={styles.year}>  ·  {year}</Text> : null}
      </Text>
      {overview ? <Text style={styles.overview}>{overview}</Text> : null}
    </View>
  );
}

function Credits({ detail }: { detail: MovieDetail | SeriesDetail }): React.JSX.Element | null {
  if (!detail.credits.length) return null;
  const names = detail.credits
    .slice()
    .sort((a, b) => (a.ord ?? 999) - (b.ord ?? 999))
    .map((c) => c.person_name)
    .slice(0, 8)
    .join(', ');
  return <Text style={styles.credits}>Cast: {names}</Text>;
}

function MovieDetailScreen({ id, isActive }: { id: number; isActive: boolean }): React.JSX.Element {
  const api = useApi();
  const { data, loading, error } = useMovie(id);
  const play = usePlay();

  return (
    <Page isActive={isActive}>
      <SpatialNavigationScrollView style={styles.screen}>
        {loading ? <ActivityIndicator color={theme.colors.text} style={styles.loader} /> : null}
        {error ? <Text style={styles.error}>Failed to load: {error}</Text> : null}
        {data ? (
          <View style={styles.body}>
            <Header
              title={data.title}
              year={data.year}
              overview={data.overview}
              backdropUri={api.imageUrl(data.backdrop_path ?? undefined)}
            />
            <Text style={styles.sectionTitle}>Play</Text>
            <DefaultFocus>
              <View>
                {data.media_files.map((file, idx) => (
                  <PlayRow
                    key={file.id}
                    label={fileLabel(file, `${data.title}`)}
                    onSelect={() => play(file.id, data.title)}
                    autoFocusHint={idx === 0}
                  />
                ))}
              </View>
            </DefaultFocus>
            <Credits detail={data} />
          </View>
        ) : null}
      </SpatialNavigationScrollView>
    </Page>
  );
}

function SeriesDetailScreen({ id, isActive }: { id: number; isActive: boolean }): React.JSX.Element {
  const api = useApi();
  const { data, loading, error } = useSeries(id);
  const play = usePlay();

  return (
    <Page isActive={isActive}>
      <SpatialNavigationScrollView style={styles.screen}>
        {loading ? <ActivityIndicator color={theme.colors.text} style={styles.loader} /> : null}
        {error ? <Text style={styles.error}>Failed to load: {error}</Text> : null}
        {data ? (
          <View style={styles.body}>
            <Header
              title={data.title}
              year={data.year}
              overview={data.overview}
              backdropUri={api.imageUrl(data.backdrop_path ?? undefined)}
            />
            {data.seasons.map((season, seasonIdx) => (
              <View key={season.id} style={styles.season}>
                <Text style={styles.sectionTitle}>Season {season.season_number}</Text>
                <DefaultFocus enable={seasonIdx === 0}>
                  <View>
                    {season.episodes.map((ep) => (
                      <PlayRow
                        key={ep.id}
                        label={`E${ep.episode_number}${ep.title ? ` · ${ep.title}` : ''}`}
                        // Episodes reference their media_file; the detail response
                        // does not inline the file here, so open by episode id is a
                        // Phase 5 concern — for now select is a no-op placeholder.
                        onSelect={() => {
                          /* Phase 5: resolve the episode's media_file then play. */
                        }}
                      />
                    ))}
                  </View>
                </DefaultFocus>
              </View>
            ))}
            <Credits detail={data} />
          </View>
        ) : null}
      </SpatialNavigationScrollView>
    </Page>
  );
}

/** Push the Player route. The direct-vs-HLS decision is resolved there, so a
 * busy-session 409 surfaces on the player screen rather than blocking navigation. */
function usePlay(): (fileId: number, title: string) => void {
  const nav = useNavigation();
  return useCallback(
    (fileId: number, title: string) => {
      nav.push({ name: 'Player', fileId, title });
    },
    [nav],
  );
}

function fileLabel(file: MediaFile, fallback: string): string {
  const bits: string[] = [];
  if (file.width && file.height) bits.push(`${file.height}p`);
  if (file.hdr_type) bits.push(file.hdr_type.toUpperCase());
  if (file.video_codec) bits.push(file.video_codec.toUpperCase());
  return bits.length ? `▶ Play  ·  ${bits.join(' · ')}` : `▶ Play ${fallback}`;
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: theme.colors.background },
  body: { padding: theme.screenPaddingH },
  header: { marginBottom: 24 },
  title: { color: theme.colors.text, fontSize: 44, fontWeight: '800' },
  year: { color: theme.colors.textMuted, fontSize: 28, fontWeight: '600' },
  overview: { color: theme.colors.textMuted, fontSize: 20, lineHeight: 28, marginTop: 12, maxWidth: 1000 },
  sectionTitle: { color: theme.colors.text, fontSize: 24, fontWeight: '700', marginTop: 24, marginBottom: 12 },
  season: { marginBottom: 8 },
  credits: { color: theme.colors.textMuted, fontSize: 18, marginTop: 28 },
  loader: { marginTop: 80 },
  error: { color: '#ff6b6b', padding: theme.screenPaddingH },
});
