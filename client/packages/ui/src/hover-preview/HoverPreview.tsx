/**
 * `HoverPreview` — a focusable poster tile that plays a silent preview clip after
 * the deterministic 2-second hover gate (README §Netflix-Style Hover Experience).
 *
 * The FSM in `./machine.ts` governs *when* the `<Video>` exists; this component
 * only renders what each state permits:
 *  - always: the poster `<Image>` (fires `REPORT_IMAGE_LOADED` on load);
 *  - only once the gate opens AND a src resolved: the `<Video>` on top, silent,
 *    looping, cross-fading in when it reports loaded.
 *
 * Focus is a `SpatialNavigationFocusableView` (deterministic D-pad), whose
 * focus/blur drive the machine — a blur at any instant tears playback down.
 *
 * `react-native-video` is imported defensively: the full player lives in
 * `@medi/player` (Phase 5). If the native module isn't present, the preview
 * degrades to just the poster (no crash), which is the correct pre-Phase-5 state.
 */

import React, { useCallback } from 'react';
import { Image, StyleSheet, View, type ImageSourcePropType } from 'react-native';
import { SpatialNavigationFocusableView } from 'react-tv-space-navigation';

import { useHoverPreview } from './useHoverPreview';

// Soft dependency on react-native-video (Phase 5 owns it fully).
type VideoComponent = React.ComponentType<{
  source: { uri: string };
  style?: object;
  muted?: boolean;
  repeat?: boolean;
  resizeMode?: 'cover' | 'contain' | 'stretch' | 'none';
  paused?: boolean;
  onLoad?: () => void;
  onError?: (e: unknown) => void;
}>;

let Video: VideoComponent | null = null;
try {
  // eslint-disable-next-line @typescript-eslint/no-var-requires
  Video = (require('react-native-video') as { default: VideoComponent }).default;
} catch {
  Video = null;
}

export interface HoverPreviewProps {
  /** media_file id whose 720p silent preview to play. */
  fileId: number;
  /** Poster image source (already an absolute URL via `ApiClient.imageUrl`). */
  posterUri?: string;
  /** Resolve/verify the preview clip URL; cancellable via the passed signal. */
  resolvePreview: (fileId: number, signal: AbortSignal) => Promise<string>;
  /** D-pad select (open detail). */
  onSelect?: () => void;
  /** Tile dimensions. */
  width: number;
  height: number;
}

export function HoverPreview({
  fileId,
  posterUri,
  resolvePreview,
  onSelect,
  width,
  height,
}: HoverPreviewProps): React.JSX.Element {
  const preview = useHoverPreview({ fileId, resolvePreview });

  const posterSource: ImageSourcePropType | undefined = posterUri
    ? { uri: posterUri }
    : undefined;

  const handleVideoError = useCallback(
    (e: unknown) => {
      const message =
        e && typeof e === 'object' && 'error' in e
          ? String((e as { error: unknown }).error)
          : 'video_error';
      preview.reportVideoError(message);
    },
    [preview],
  );

  return (
    <SpatialNavigationFocusableView
      onSelect={onSelect}
      onFocus={preview.onFocus}
      onBlur={preview.onBlur}
    >
      {({ isFocused }) => (
        <View
          style={[
            styles.tile,
            { width, height },
            isFocused && styles.tileFocused,
          ]}
        >
          {posterSource && (
            <Image
              source={posterSource}
              style={styles.fill}
              resizeMode="cover"
              // Gate step 1: no video logic runs until this fires.
              onLoad={preview.reportImageLoaded}
            />
          )}

          {/* Mounted only after the 2s gate opens and a src resolved. */}
          {preview.shouldMountVideo && preview.previewUrl && Video && (
            <View
              style={[styles.fill, preview.isPlaying ? styles.videoShown : styles.videoHidden]}
              pointerEvents="none"
            >
              <Video
                source={{ uri: preview.previewUrl }}
                style={styles.fill}
                muted
                repeat
                resizeMode="cover"
                onLoad={preview.reportVideoLoaded}
                onError={handleVideoError}
              />
            </View>
          )}
        </View>
      )}
    </SpatialNavigationFocusableView>
  );
}

const styles = StyleSheet.create({
  tile: {
    borderRadius: 8,
    overflow: 'hidden',
    backgroundColor: '#1a1a1a',
    borderWidth: 3,
    borderColor: 'transparent',
  },
  tileFocused: {
    borderColor: '#ffffff',
    // A subtle lift; scale is applied by the parent row if desired.
  },
  fill: {
    ...StyleSheet.absoluteFillObject,
  },
  // Cross-fade: hidden until the clip reports loaded, then shown over the poster.
  videoHidden: {
    opacity: 0,
  },
  videoShown: {
    opacity: 1,
  },
});
