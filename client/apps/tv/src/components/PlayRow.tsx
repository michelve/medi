/**
 * `PlayRow` — a focusable list row used on the Detail screen for each playable
 * file / episode. Thin wrapper over `SpatialNavigationFocusableView` with the
 * app's focus styling.
 */

import React from 'react';
import { StyleSheet, Text, View } from 'react-native';
import { SpatialNavigationFocusableView } from 'react-tv-space-navigation';

import { theme } from '@medi/ui';

export function PlayRow({
  label,
  onSelect,
  autoFocusHint,
}: {
  label: string;
  onSelect: () => void;
  /** Purely cosmetic hint that this is the primary action row. */
  autoFocusHint?: boolean;
}): React.JSX.Element {
  return (
    <SpatialNavigationFocusableView onSelect={onSelect}>
      {({ isFocused }) => (
        <View
          style={[
            styles.row,
            autoFocusHint && styles.primary,
            isFocused && styles.rowFocused,
          ]}
        >
          <Text style={[styles.label, isFocused && styles.labelFocused]}>{label}</Text>
        </View>
      )}
    </SpatialNavigationFocusableView>
  );
}

const styles = StyleSheet.create({
  row: {
    paddingVertical: 16,
    paddingHorizontal: 24,
    marginBottom: 12,
    borderRadius: 8,
    backgroundColor: theme.colors.surface,
    borderWidth: 3,
    borderColor: 'transparent',
  },
  primary: {
    backgroundColor: 'rgba(10,132,255,0.18)',
  },
  rowFocused: {
    backgroundColor: theme.colors.text,
    borderColor: theme.colors.focus,
  },
  label: {
    color: theme.colors.text,
    fontSize: 22,
    fontWeight: '600',
  },
  labelFocused: {
    color: theme.colors.background,
  },
});
