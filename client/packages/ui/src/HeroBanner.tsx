/**
 * `HeroBanner` — the large featured backdrop atop the Home screen.
 *
 * It is a single focusable node wrapped in a `DirectionalOverride` so that
 * pressing **Down** jumps focus straight to the first item of the row named by
 * `downTarget` (canonically "Continue Watching"), regardless of geometry — the
 * exact override the task calls out (README §Spatial Navigation → Directional
 * Overrides). The target row registers its name via `useFocusTarget`.
 */

import React from 'react';
import { ImageBackground, StyleSheet, Text, View } from 'react-native';
import {
  SpatialNavigationFocusableView,
  DirectionalOverride,
  DefaultFocus,
} from '@medi/navigation';

import { theme } from './theme';

export interface HeroBannerProps {
  title: string;
  overview?: string;
  /** Absolute backdrop image URL. */
  backdropUri?: string;
  /** Play / open the featured title. */
  onSelect: () => void;
  /**
   * Name of the focus target to jump to on "Down" (registered by a carousel via
   * `useFocusTarget`). Defaults to `"continue-watching"`.
   */
  downTarget?: string;
  /** Give the hero initial focus on mount (typical for Home). */
  defaultFocus?: boolean;
}

export function HeroBanner({
  title,
  overview,
  backdropUri,
  onSelect,
  downTarget = 'continue-watching',
  defaultFocus = true,
}: HeroBannerProps): React.JSX.Element {
  const focusable = (
    <SpatialNavigationFocusableView onSelect={onSelect}>
      {({ isFocused }) => (
        <ImageBackground
          source={backdropUri ? { uri: backdropUri } : undefined}
          style={styles.hero}
          resizeMode="cover"
        >
          <View style={styles.scrim} />
          <View style={styles.content}>
            <Text style={styles.title}>{title}</Text>
            {overview ? (
              <Text style={styles.overview} numberOfLines={3}>
                {overview}
              </Text>
            ) : null}
            <View style={[styles.cta, isFocused && styles.ctaFocused]}>
              <Text style={[styles.ctaText, isFocused && styles.ctaTextFocused]}>
                ▶  Play
              </Text>
            </View>
          </View>
        </ImageBackground>
      )}
    </SpatialNavigationFocusableView>
  );

  return (
    <DirectionalOverride to={{ down: downTarget }}>
      {defaultFocus ? <DefaultFocus>{focusable}</DefaultFocus> : focusable}
    </DirectionalOverride>
  );
}

const styles = StyleSheet.create({
  hero: {
    height: 520,
    justifyContent: 'flex-end',
    backgroundColor: theme.colors.surface,
  },
  scrim: {
    ...StyleSheet.absoluteFillObject,
    backgroundColor: 'rgba(0,0,0,0.35)',
  },
  content: {
    padding: theme.screenPaddingH,
    paddingBottom: 40,
    maxWidth: 900,
  },
  title: {
    color: theme.colors.text,
    fontSize: 56,
    fontWeight: '800',
    marginBottom: 12,
  },
  overview: {
    color: theme.colors.textMuted,
    fontSize: 20,
    lineHeight: 28,
    marginBottom: 24,
  },
  cta: {
    alignSelf: 'flex-start',
    paddingHorizontal: 28,
    paddingVertical: 14,
    borderRadius: 8,
    backgroundColor: 'rgba(255,255,255,0.2)',
    borderWidth: 3,
    borderColor: 'transparent',
  },
  ctaFocused: {
    backgroundColor: theme.colors.text,
    borderColor: theme.colors.focus,
  },
  ctaText: {
    color: theme.colors.text,
    fontSize: 22,
    fontWeight: '700',
  },
  ctaTextFocused: {
    // Dark text on the white focused CTA for contrast.
    color: theme.colors.background,
  },
});
