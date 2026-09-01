/**
 * `Carousel` — a horizontal, virtualized row of poster tiles (README §Home rows).
 *
 * Built on `SpatialNavigationVirtualizedList` so a row of 10,000 posters stays
 * smooth on weak TV silicon (task §Scaling notes): only on-screen tiles plus a
 * small overscan are mounted. Each tile is a `HoverPreview`, so dwelling on one
 * for 2s starts its silent preview and scrolling past cancels the gate.
 *
 * `data` MUST be a stable/memoized array (virtualized-list requirement) — the
 * screen that owns the row is responsible for memoizing it.
 */

import React, { useCallback, useRef } from 'react';
import { StyleSheet, Text, View } from 'react-native';
import {
  SpatialNavigationVirtualizedList,
  useRegisterFocusTarget,
  type SpatialNavigationVirtualizedListRef,
} from '@medi/navigation';

import { HoverPreview } from './hover-preview';
import { PosterCard } from './PosterCard';
import { theme } from './theme';
import type { PosterItem } from './types';

export interface CarouselProps {
  title: string;
  /** Memoized list of poster items for this row. */
  data: PosterItem[];
  /** Resolve a title's poster image URL. */
  posterUri: (item: PosterItem) => string | undefined;
  /** Resolve/verify a title's preview clip URL, cancellable via the signal. */
  resolvePreview: (item: PosterItem, signal: AbortSignal) => Promise<string>;
  /** D-pad select on a tile (open its detail screen). */
  onSelect: (item: PosterItem) => void;
  /** Total item count for infinite scroll alignment; defaults to `data.length`. */
  nbMaxOfItems?: number;
  /** Load the next page when the row nears its end. */
  onEndReached?: () => void;
  /**
   * Register this row's first item as a named directional-override focus target
   * (e.g. `"continue-watching"`), so a hero's "Down" press can jump here.
   */
  focusTargetName?: string;
}

const ITEM_STRIDE = theme.poster.width + theme.poster.gap;

export function Carousel({
  title,
  data,
  posterUri,
  resolvePreview,
  onSelect,
  nbMaxOfItems,
  onEndReached,
  focusTargetName,
}: CarouselProps): React.JSX.Element {
  const listRef = useRef<SpatialNavigationVirtualizedListRef>(null);
  // A hero "Down" override that names this row focuses its first tile.
  useRegisterFocusTarget(focusTargetName, () => listRef.current?.focus(0));

  const renderItem = useCallback(
    ({ item }: { item: PosterItem }) => (
      <View style={styles.itemWrap}>
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
    <View style={styles.row}>
      <Text style={styles.title}>{title}</Text>
      <SpatialNavigationVirtualizedList
        ref={listRef}
        orientation="horizontal"
        data={data}
        renderItem={renderItem}
        itemSize={ITEM_STRIDE}
        nbMaxOfItems={nbMaxOfItems ?? data.length}
        scrollBehavior="stick-to-start"
        onEndReached={onEndReached}
        onEndReachedThresholdItemsNumber={6}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  row: {
    marginBottom: theme.rowGap,
  },
  title: {
    color: theme.colors.text,
    fontSize: 24,
    fontWeight: '700',
    marginBottom: 16,
    paddingHorizontal: theme.screenPaddingH,
  },
  itemWrap: {
    marginRight: theme.poster.gap,
  },
});
