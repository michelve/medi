/**
 * `VideoScreen` — full-length playback (`docs/.tasks/50` Part A sub-tasks 1 & 4).
 *
 * Flow:
 *  1. Resolve the playback decision from `GET /api/stream/:file_id`
 *     (`resolveStream`). Direct-play and HLS are handled *uniformly* — both yield
 *     an absolute `uri` for the same `<Video>`; only `type: 'm3u8'` differs.
 *  2. Mount react-native-video (AVPlayer on tvOS, ExoPlayer on Android TV) with
 *     that source and start playing.
 *  3. Drive the overlay + transport entirely through `useTVEventHandler` →
 *     `usePlayerControls` (see that hook for why the overlay bypasses spatial nav).
 *
 * Graceful degradation:
 *  - A `409` from the stream decision means the transcode session cap is hit
 *    (README/api-client `ApiError.isBusy`). We surface a non-fatal notice and keep
 *    the screen up so the user can back out or retry — no crash, no blank screen.
 *  - If the native `react-native-video` module isn't present (e.g. running the JS
 *    bundle without a dev client), we degrade to a message instead of throwing —
 *    the same defensive `require` pattern `@medi/ui` `HoverPreview` uses.
 *
 * The player is intentionally decoupled from `@medi/api-client`'s transport: the
 * caller injects `resolveStream` and (optionally) `resolveTrickplay`, so this
 * package doesn't depend on how the app builds its client, and stays testable.
 */

import React, { useCallback, useEffect, useRef, useState } from 'react';
import {
  ActivityIndicator,
  StyleSheet,
  Text,
  View,
  useTVEventHandler,
} from 'react-native';

import { PlayerOverlay } from './PlayerOverlay';
import { usePlayerControls, type PlayerRemoteEvent } from './usePlayerControls';
import type { TrickplayMeta } from './trickplay';

// -- Soft dependency on react-native-video (same pattern as HoverPreview) -----
interface VideoSource {
  uri: string;
  /** `'m3u8'` for HLS; omitted for a direct progressive/byte-range source. */
  type?: 'm3u8';
}
interface VideoProgress {
  currentTime: number; // seconds
  seekableDuration?: number;
}
interface VideoLoad {
  duration: number; // seconds
}
interface VideoHandle {
  seek: (seconds: number) => void;
}
type VideoComponent = React.ComponentType<{
  ref?: React.Ref<VideoHandle>;
  source: VideoSource;
  style?: object;
  paused?: boolean;
  resizeMode?: 'cover' | 'contain' | 'stretch' | 'none';
  progressUpdateInterval?: number;
  onLoad?: (data: VideoLoad) => void;
  onProgress?: (data: VideoProgress) => void;
  onError?: (e: unknown) => void;
  onEnd?: () => void;
}>;

let Video: VideoComponent | null = null;
try {
  // eslint-disable-next-line @typescript-eslint/no-var-requires
  Video = (require('react-native-video') as { default: VideoComponent }).default;
} catch {
  Video = null;
}

/** What `resolveStream` yields — an absolute source plus its mode. */
export interface ResolvedStream {
  uri: string;
  isHls: boolean;
}

export interface VideoScreenProps {
  fileId: number;
  title: string;
  /**
   * Resolve `/api/stream/:file_id` to an absolute playable source. Throws the
   * api-client `ApiError` on failure; a `409` (`isBusy`) is surfaced as a notice.
   * Cancellable via the passed signal (mount/unmount races).
   */
  resolveStream: (fileId: number, signal: AbortSignal) => Promise<ResolvedStream>;
  /**
   * Optional: resolve the trickplay mosaic + grid metadata for scrub thumbnails.
   * Omit (or reject) to fall back to a plain scrub bar (e.g. asset not generated,
   * or the backend metadata endpoint isn't available yet — see README).
   */
  resolveTrickplay?: (fileId: number, signal: AbortSignal) => Promise<TrickplayMeta>;
  /**
   * Called when playback ends (`onEnd`) and the player should leave the screen.
   * The hardware back/menu button is intentionally NOT handled here — the host
   * app owns global back (it must pop *any* screen uniformly); a player-local
   * menu handler would double-pop. See `PlayerScreen` in the tv app.
   */
  onExit?: () => void;
}

type Phase =
  | { kind: 'loading' }
  | { kind: 'ready'; source: VideoSource }
  | { kind: 'error'; message: string; busy: boolean };

export function VideoScreen({
  fileId,
  title,
  resolveStream,
  resolveTrickplay,
  onExit,
}: VideoScreenProps): React.JSX.Element {
  const [phase, setPhase] = useState<Phase>({ kind: 'loading' });
  const [trickplay, setTrickplay] = useState<TrickplayMeta | undefined>(undefined);
  const videoRef = useRef<VideoHandle | null>(null);

  // Resolve the stream decision (direct vs HLS) on mount.
  useEffect(() => {
    const controller = new AbortController();
    setPhase({ kind: 'loading' });
    resolveStream(fileId, controller.signal)
      .then((s) => {
        setPhase({
          kind: 'ready',
          source: s.isHls ? { uri: s.uri, type: 'm3u8' } : { uri: s.uri },
        });
      })
      .catch((e: unknown) => {
        if (controller.signal.aborted) return;
        const busy =
          !!e && typeof e === 'object' && 'isBusy' in e
            ? Boolean((e as { isBusy: unknown }).isBusy)
            : false;
        const message = e instanceof Error ? e.message : 'playback unavailable';
        setPhase({ kind: 'error', message, busy });
      });
    return () => controller.abort();
  }, [fileId, resolveStream]);

  // Best-effort trickplay metadata (never blocks playback).
  useEffect(() => {
    if (!resolveTrickplay) return;
    const controller = new AbortController();
    resolveTrickplay(fileId, controller.signal)
      .then(setTrickplay)
      .catch(() => setTrickplay(undefined));
    return () => controller.abort();
  }, [fileId, resolveTrickplay]);

  const controls = usePlayerControls({
    onSeek: (positionMs) => videoRef.current?.seek(positionMs / 1000),
  });

  // Global remote interception — the whole point of Part A. No focusable overlay.
  // `menu`/back is deliberately left to the host app's global handler (see the
  // `onExit` doc); here we only route the transport keys.
  const { handleRemote, showOverlay } = controls;
  useTVEventHandler((evt: { eventType: string }) => {
    const known: PlayerRemoteEvent[] = ['select', 'playPause', 'left', 'right', 'up', 'down'];
    if ((known as string[]).includes(evt.eventType)) {
      handleRemote(evt.eventType as PlayerRemoteEvent);
    }
  });

  const onLoad = useCallback(
    (data: VideoLoad) => controls.reportDuration(Math.round(data.duration * 1000)),
    [controls],
  );
  const onProgress = useCallback(
    (data: VideoProgress) => controls.reportProgress(Math.round(data.currentTime * 1000)),
    [controls],
  );
  const onError = useCallback(() => {
    controls.showOverlay();
    setPhase({ kind: 'error', message: 'playback error', busy: false });
  }, [controls]);

  // Reveal the overlay whenever we surface an error/notice.
  useEffect(() => {
    if (phase.kind === 'error') showOverlay();
  }, [phase.kind, showOverlay]);

  return (
    <View style={styles.root}>
      {phase.kind === 'loading' ? (
        <View style={styles.centerFill}>
          <ActivityIndicator color="#fff" size="large" />
          <Text style={styles.hint}>Preparing “{title}”…</Text>
        </View>
      ) : null}

      {phase.kind === 'ready' && Video ? (
        <Video
          ref={videoRef}
          source={phase.source}
          style={StyleSheet.absoluteFill}
          paused={!controls.isPlaying}
          resizeMode="contain"
          progressUpdateInterval={500}
          onLoad={onLoad}
          onProgress={onProgress}
          onError={onError}
          onEnd={onExit}
        />
      ) : null}

      {phase.kind === 'ready' && !Video ? (
        <View style={styles.centerFill}>
          <Text style={styles.hint}>
            Video module unavailable in this build. Run a dev client / device build
            to play “{title}”.
          </Text>
        </View>
      ) : null}

      {/* The overlay renders for ready and error phases; it owns no focus. */}
      {phase.kind !== 'loading' ? (
        <PlayerOverlay
          visible={controls.overlayVisible || phase.kind === 'error'}
          isPlaying={controls.isPlaying}
          isScrubbing={controls.isScrubbing}
          positionMs={controls.displayPositionMs}
          durationMs={controls.durationMs}
          title={title}
          trickplay={trickplay}
          notice={
            phase.kind === 'error'
              ? phase.busy
                ? 'All transcode sessions are busy right now. Press back and try again in a moment.'
                : `Playback unavailable: ${phase.message}`
              : null
          }
        />
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1, backgroundColor: '#000' },
  centerFill: {
    ...StyleSheet.absoluteFillObject,
    alignItems: 'center',
    justifyContent: 'center',
    padding: 48,
  },
  hint: { color: '#f2f2f7', fontSize: 22, marginTop: 20, textAlign: 'center', lineHeight: 30 },
});
