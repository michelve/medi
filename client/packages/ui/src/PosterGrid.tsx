/**
 * `PosterGrid` — a virtualized 2-D grid of poster tiles for the full-library
 * browse view (task sub-task 4).
 *
 * Uses `SpatialNavigationVirtualizedGrid` so scrolling 10,000 posters stays
 * smooth on weak TV silicon (task §Scaling notes) — only the visible rows plus a
 * few overscan rows are mounted. D-pad navigation across the grid is deterministic
 * (react-tv-space-navigation), and each cell is a `HoverPreview` governed by the
 * 2s gate.
 */

import React, { useCallback } from 'react';
import { StyleSheet, View } from 'react-native';
import { SpatialNavigationVirtualizedGrid } from 'react-tv-space-navigation';

import { HoverPreview } from './hover-preview';
import { PosterCard } from './PosterCard';
import { theme } from './theme';
import type { PosterItem } from './types';

export interface PosterGridProps {
  /** Memoized poster items. */
  data: PosterItem[];
  numberOfColumns?: number;
  posterUri: (item: PosterItem) => string | undefined;
  resolvePreview: (item: PosterItem, signal: AbortSignal) => Promise<string>;
  onSelect: (item: PosterItem) => void;
  /** Fetch the next keyset page when nearing the end. */
  onEndReached?: () => void;
}

const ROW_HEIGHT = theme.poster.height + theme.poster.gap;

export function PosterGrid({
  data,
  numberOfColumns = 6,
  posterUri,
  resolvePreview,
  onSelect,
  onEndReached,
}: PosterGridProps): React.JSX.Element {
  const renderItem = useCallback(
    ({ item }: { item: PosterItem }) => (
      <View style={styles.cell}>
        {item.previewFileId != null ? (
          <HoverPreview
            fileId={item.previewFileId}
            posterUri={posterUri(item)}
            resolvePreview={(_fileId, signal) => resolvePreview(item, signal)}
            onSelect={() => onSelect(item)}
            width={theme.poster.width}
            height={theme.poster.height}
          />
        ) : (
          <PosterCard
            posterUri={posterUri(item)}
            title={item.title}
            onSelect={() => onSelect(item)}
            width={theme.poster.width}
            height={theme.poster.height}
          />
        )}
      </View>
    ),
    [posterUri, resolvePreview, onSelect],
  );

  return (
    <SpatialNavigationVirtualizedGrid
      data={data}
      renderItem={renderItem}
      numberOfColumns={numberOfColumns}
      itemHeight={ROW_HEIGHT}
      additionalRenderedRows={2}
      onEndReached={onEndReached}
      onEndReachedThresholdRowsNumber={2}
      rowContainerStyle={styles.gridRow}
    />
  );
}

const styles = StyleSheet.create({
  gridRow: {
    gap: theme.poster.gap,
    paddingHorizontal: theme.screenPaddingH,
  },
  cell: {
    // Cells are laid out by the grid; keep them centered within their slot.
    alignItems: 'center',
  },
});
