/**
 * Home screen — the featured hero over rows of posters (task sub-task 5:
 * "rows from `/api/library`").
 *
 * The unified library is paged with a keyset cursor and split into a hero (first
 * title) plus carousels. The hero's "Down" press is a directional override that
 * jumps to the first carousel, which registers the `"continue-watching"` focus
 * target (README §Directional Overrides). A `FocusTargetProvider` scopes those
 * named targets to this screen.
 *
 * `isActive` is forwarded to `<Page>` so this screen only owns D-pad focus while
 * it is the top of the navigation stack.
 */

import React, { useMemo } from 'react';
import { ActivityIndicator, StyleSheet, Text, View } from 'react-native';

import {
  Page,
  FocusTargetProvider,
  HeroBanner,
  Carousel,
  SpatialNavigationScrollView,
  theme,
  type PosterItem,
} from '../deps';
import { usePreviewResolver } from '../api';
import { useLibrary } from '../hooks';
import { useNavigation } from '../navigation';

/** Chunk a flat item list into pseudo-rows so the demo Home has several carousels. */
function intoRows(items: PosterItem[], rowSize = 12): PosterItem[][] {
  const rows: PosterItem[][] = [];
  for (let i = 0; i < items.length; i += rowSize) {
    rows.push(items.slice(i, i + rowSize));
  }
  return rows;
}

const ROW_LABELS = ['Continue Watching', 'Recently Added', 'Movies', 'Series', 'More'];

export function HomeScreen({ isActive }: { isActive: boolean }): React.JSX.Element {
  const nav = useNavigation();
  const resolvePreview = usePreviewResolver();
  const { items, loading, error, loadMore, exhausted } = useLibrary();

  const rows = useMemo(() => intoRows(items), [items]);
  const hero = items[0];

  // `useLibrary` already absolutized `poster` via `api.imageUrl`.
  const posterUri = (item: PosterItem) => item.poster;
  const openDetail = (item: PosterItem) =>
    nav.push({ name: 'Detail', kind: item.kind, id: item.id });

  return (
    <Page isActive={isActive}>
      <FocusTargetProvider>
        <SpatialNavigationScrollView style={styles.screen}>
          {hero ? (
            <HeroBanner
              title={hero.title}
              backdropUri={posterUri(hero)}
              onSelect={() => openDetail(hero)}
              downTarget="continue-watching"
            />
          ) : null}

          <View style={styles.rows}>
            {rows.map((row, idx) => (
              <Carousel
                // Rows are stable slices; index key is fine for this demo layout.
                key={idx}
                // First row is the hero's "Down" target.
                focusTargetName={idx === 0 ? 'continue-watching' : undefined}
                title={ROW_LABELS[idx] ?? `Row ${idx + 1}`}
                data={row}
                posterUri={posterUri}
                resolvePreview={resolvePreview}
                onSelect={openDetail}
                // The last visible row triggers the next page.
                onEndReached={idx === rows.length - 1 && !exhausted ? loadMore : undefined}
              />
            ))}
          </View>

          {loading ? <ActivityIndicator style={styles.loader} color={theme.colors.text} /> : null}
          {error ? <Text style={styles.error}>Failed to load library: {error}</Text> : null}
        </SpatialNavigationScrollView>
      </FocusTargetProvider>
    </Page>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: theme.colors.background,
  },
  rows: {
    paddingTop: theme.rowGap,
  },
  loader: {
    marginVertical: 24,
  },
  error: {
    color: '#ff6b6b',
    padding: theme.screenPaddingH,
  },
});
