/**
 * `PlayerOverlay` — the presentational transport UI drawn over the video.
 *
 * CRITICAL (`docs/.tasks/50` Part A sub-task 2): this layer contains NO focusable
 * nodes and is `pointerEvents="none"`. It never participates in spatial
 * navigation — on Android TV a focusable overlay over the ExoPlayer surface
 * breaks D-pad routing and causes render lag. All input is handled upstream by
 * `useTVEventHandler` → `usePlayerControls`; this component only *renders* the
 * resulting state (visibility, play/pause, scrub position, trickplay thumbnail).
 */

import React from 'react';
import { Image, StyleSheet, Text, View } from 'react-native';

import { tileForPosition, type TrickplayMeta } from './trickplay';

export interface PlayerOverlayProps {
  visible: boolean;
  isPlaying: boolean;
  isScrubbing: boolean;
  /** Position to render on the scrub bar / thumbnail (ms). */
  positionMs: number;
  /** Media duration (ms); 0 hides the elapsed/total readout. */
  durationMs: number;
  title: string;
  /** Trickplay mosaic + grid dims; when present a scrub shows a thumbnail. */
  trickplay?: TrickplayMeta;
  /** Non-fatal notice (e.g. the 409 session-cap fallback message). */
  notice?: string | null;
}

function fmt(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const mm = String(m).padStart(2, '0');
  const ss = String(s).padStart(2, '0');
  return h > 0 ? `${h}:${mm}:${ss}` : `${m}:${ss}`;
}

export function PlayerOverlay({
  visible,
  isPlaying,
  isScrubbing,
  positionMs,
  durationMs,
  title,
  trickplay,
  notice,
}: PlayerOverlayProps): React.JSX.Element | null {
  if (!visible) return null;

  const progress = durationMs > 0 ? Math.min(1, positionMs / durationMs) : 0;
  const tile =
    isScrubbing && trickplay ? tileForPosition(trickplay, positionMs) : null;

  return (
    <View style={styles.root} pointerEvents="none">
      {/* Top scrim: title. */}
      <View style={styles.top}>
        <Text style={styles.title} numberOfLines={1}>
          {title}
        </Text>
        {notice ? <Text style={styles.notice}>{notice}</Text> : null}
      </View>

      {/* Center: play/pause glyph, shown briefly on toggle. */}
      <View style={styles.center}>
        <View style={styles.glyphWrap}>
          <Text style={styles.glyph}>{isPlaying ? '❚❚' : '▶'}</Text>
        </View>
      </View>

      {/* Bottom scrim: trickplay thumb (while scrubbing) + scrub bar + times. */}
      <View style={styles.bottom}>
        {tile ? (
          <View
            style={[
              styles.thumbFrame,
              // Position the thumb horizontally over the scrub handle.
              { alignSelf: 'center' },
            ]}
          >
            <View style={[styles.thumbClip, { width: tile.width, height: tile.height }]}>
              <Image
                source={{ uri: trickplay!.url }}
                style={{
                  width: trickplay!.tileW * trickplay!.cols,
                  height: trickplay!.tileH * trickplay!.rows,
                  // Shift the mosaic so the wanted cell sits in the clip window.
                  transform: [
                    { translateX: -tile.x },
                    { translateY: -tile.y },
                  ],
                }}
                resizeMode="cover"
              />
            </View>
          </View>
        ) : null}

        <View style={styles.barRow}>
          <Text style={styles.time}>{fmt(positionMs)}</Text>
          <View style={styles.track}>
            <View style={[styles.fill, { width: `${progress * 100}%` }]} />
            <View style={[styles.handle, { left: `${progress * 100}%` }]} />
          </View>
          <Text style={styles.time}>{durationMs > 0 ? fmt(durationMs) : '--:--'}</Text>
        </View>
      </View>
    </View>
  );
}

const SCRIM = 'rgba(0,0,0,0.55)';

const styles = StyleSheet.create({
  root: {
    ...StyleSheet.absoluteFillObject,
    justifyContent: 'space-between',
  },
  top: {
    paddingTop: 40,
    paddingHorizontal: 48,
    paddingBottom: 60,
    backgroundColor: SCRIM,
  },
  title: { color: '#fff', fontSize: 34, fontWeight: '800' },
  notice: { color: '#ffd479', fontSize: 18, marginTop: 8 },
  center: { alignItems: 'center', justifyContent: 'center' },
  glyphWrap: {
    width: 96,
    height: 96,
    borderRadius: 48,
    backgroundColor: 'rgba(0,0,0,0.45)',
    alignItems: 'center',
    justifyContent: 'center',
  },
  glyph: { color: '#fff', fontSize: 40, fontWeight: '700' },
  bottom: {
    paddingBottom: 48,
    paddingHorizontal: 48,
    paddingTop: 60,
    backgroundColor: SCRIM,
  },
  thumbFrame: {
    marginBottom: 16,
    padding: 4,
    borderRadius: 8,
    backgroundColor: '#000',
    borderWidth: 2,
    borderColor: '#fff',
  },
  thumbClip: {
    overflow: 'hidden',
    borderRadius: 4,
  },
  barRow: { flexDirection: 'row', alignItems: 'center' },
  time: {
    color: '#fff',
    fontSize: 18,
    fontVariant: ['tabular-nums'],
    width: 88,
    textAlign: 'center',
  },
  track: {
    flex: 1,
    height: 6,
    borderRadius: 3,
    marginHorizontal: 16,
    backgroundColor: 'rgba(255,255,255,0.3)',
    justifyContent: 'center',
  },
  fill: {
    height: 6,
    borderRadius: 3,
    backgroundColor: '#0a84ff',
  },
  handle: {
    position: 'absolute',
    width: 16,
    height: 16,
    borderRadius: 8,
    marginLeft: -8,
    backgroundColor: '#fff',
  },
});
