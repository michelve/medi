/**
 * Player screen — mounts the Phase 5 `@medi/player` `VideoScreen`.
 *
 * This screen's only job is to adapt the app's shared `ApiClient` into the two
 * injectables `VideoScreen` needs, and to pop the nav stack when the player asks
 * to exit (hardware back/menu or end-of-file):
 *
 *  - `resolveStream`: `GET /api/stream/:file_id` → an absolute source URL, with
 *    the direct-vs-HLS mode flattened to `isHls`. Any `ApiError` (incl. the `409`
 *    session-cap, which the player surfaces gracefully) propagates unchanged.
 *  - `resolveTrickplay`: best-effort grid metadata for scrub thumbnails. The
 *    backend metadata endpoint (`/api/trickplay/:file_id/meta`) is a known gap
 *    (see `@medi/player` README); until it lands this rejects and the player
 *    falls back to a plain scrub bar. No app change is needed when it ships.
 *
 * Unlike other screens it does not render inside `<Page>`: the player owns the
 * full frame and its own (non-spatial) input via `useTVEventHandler`.
 */

import React, { useCallback } from 'react';

import { VideoScreen, type ResolvedStream, type TrickplayMeta } from '../deps';
import { useApi } from '../api';
import { useNavigation } from '../navigation';

export function PlayerScreen({
  fileId,
  title,
  isActive,
}: {
  fileId: number;
  title: string;
  isActive: boolean;
}): React.JSX.Element {
  const api = useApi();
  const nav = useNavigation();

  const resolveStream = useCallback(
    async (id: number, signal: AbortSignal): Promise<ResolvedStream> => {
      const decision = await api.stream(id, {}, { signal });
      // `decision.url` is the m3u8 (hls) or `/api/direct/:id` (direct); absolutize
      // uniformly for the native player.
      return {
        uri: api.abs(decision.url),
        isHls: decision.mode === 'hls',
      };
    },
    [api],
  );

  const resolveTrickplay = useCallback(
    async (id: number, signal: AbortSignal): Promise<TrickplayMeta> => {
      // Backend metadata endpoint (grid dims + interval) — not yet served; see
      // @medi/player README. The URL shape is fixed so this "just works" later.
      const metaUrl = api.abs(`/api/trickplay/${id}/meta`);
      const res = await fetch(metaUrl, { method: 'GET', signal });
      if (!res.ok) throw new Error(`trickplay meta ${res.status}`);
      const m = (await res.json()) as {
        interval_ms: number;
        tile_w: number;
        tile_h: number;
        cols: number;
        rows: number;
      };
      return {
        url: api.trickplayUrl(id, 'jpg'),
        intervalMs: m.interval_ms,
        tileW: m.tile_w,
        tileH: m.tile_h,
        cols: m.cols,
        rows: m.rows,
      };
    },
    [api],
  );

  const onExit = useCallback(() => {
    if (nav.canGoBack) nav.pop();
  }, [nav]);

  // Only the active (top-of-stack) player owns the remote; a backgrounded one
  // must not intercept events. When inactive, render nothing focus-worthy.
  if (!isActive) return <></>;

  return (
    <VideoScreen
      fileId={fileId}
      title={title}
      resolveStream={resolveStream}
      resolveTrickplay={resolveTrickplay}
      onExit={onExit}
    />
  );
}
