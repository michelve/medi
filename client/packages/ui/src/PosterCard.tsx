/**
 * `PosterCard` — a focusable poster tile WITHOUT the hover-preview video.
 *
 * Used wherever we have a poster but no media_file id to preview (the unified
 * `/api/library` browse grid, where cards carry no file id). It's the static
 * counterpart to `HoverPreview`: same focus behavior and framing, no FSM. When a
 * file id is available (detail screens), use `HoverPreview` instead.
 */

import React from 'react';
import { Image, StyleSheet, Text, View, type ImageSourcePropType } from 'react-native';
import { SpatialNavigationFocusableView } from 'react-tv-space-navigation';

import { theme } from './theme';

export interface PosterCardProps {
  posterUri?: string;
  /** Fallback label shown when there's no artwork. */
  title: string;
  onSelect?: () => void;
  width: number;
  height: number;
}

export function PosterCard({
  posterUri,
  title,
  onSelect,
  width,
  height,
}: PosterCardProps): React.JSX.Element {
  const source: ImageSourcePropType | undefined = posterUri ? { uri: posterUri } : undefined;

  return (
    <SpatialNavigationFocusableView onSelect={onSelect}>
      {({ isFocused }) => (
        <View style={[styles.tile, { width, height }, isFocused && styles.tileFocused]}>
          {source ? (
            <Image source={source} style={styles.fill} resizeMode="cover" />
          ) : (
            <View style={[styles.fill, styles.placeholder]}>
              <Text style={styles.placeholderText} numberOfLines={3}>
                {title}
              </Text>
            </View>
          )}
        </View>
      )}
    </SpatialNavigationFocusableView>
  );
}

const styles = StyleSheet.create({
  tile: {
    borderRadius: theme.poster.radius,
    overflow: 'hidden',
    backgroundColor: theme.colors.surface,
    borderWidth: 3,
    borderColor: 'transparent',
  },
  tileFocused: {
    borderColor: theme.colors.focus,
  },
  fill: {
    ...StyleSheet.absoluteFillObject,
  },
  placeholder: {
    alignItems: 'center',
    justifyContent: 'center',
    padding: 12,
  },
  placeholderText: {
    color: theme.colors.textMuted,
    fontSize: 16,
    textAlign: 'center',
  },
});
