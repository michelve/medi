/**
 * Data hooks over `@medi/api-client`. Minimal fetch-on-mount + keyset paging;
 * no external data library (the API's own ETag/moka cache does the heavy lifting,
 * and `ApiClient` replays 304s from its in-memory cache).
 */

import { useCallback, useEffect, useRef, useState } from 'react';

import { useApi } from './api';
import type {
  LibraryItem,
  MovieDetail,
  SeriesDetail,
  PosterItem,
} from './deps';

/** Map a `/api/library` card to the UI `PosterItem`. */
function toPosterItem(item: LibraryItem, imageUrl: (p?: string) => string | undefined): PosterItem {
  return {
    kind: item.kind,
    id: item.id,
    title: item.title,
    year: item.year,
    poster: imageUrl(item.poster),
    hdr: item.hdr,
    // Library cards carry no media_file id, so no hover preview in browse.
    previewFileId: undefined,
  };
}

export interface UseLibraryResult {
  items: PosterItem[];
  loading: boolean;
  error: string | null;
  /** Fetch the next keyset page; no-op when exhausted or already loading. */
  loadMore: () => void;
  exhausted: boolean;
}

/** Page the unified catalog with a keyset cursor (`docs/.tasks/02-api-contract.md`). */
export function useLibrary(limit = 60): UseLibraryResult {
  const api = useApi();
  const [items, setItems] = useState<PosterItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const cursor = useRef<string | null | undefined>(undefined);
  const exhausted = useRef(false);
  const inFlight = useRef(false);

  const loadMore = useCallback(() => {
    if (inFlight.current || exhausted.current) return;
    inFlight.current = true;
    setLoading(true);
    api
      .library({ cursor: cursor.current ?? undefined, limit })
      .then((page) => {
        setItems((prev) => [
          ...prev,
          ...page.items.map((it) => toPosterItem(it, (p) => api.imageUrl(p))),
        ]);
        cursor.current = page.next_cursor;
        if (page.next_cursor == null) exhausted.current = true;
        setError(null);
      })
      .catch((e: unknown) => setError(e instanceof Error ? e.message : 'load failed'))
      .finally(() => {
        inFlight.current = false;
        setLoading(false);
      });
  }, [api, limit]);

  // Initial page.
  useEffect(() => {
    loadMore();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return { items, loading, error, loadMore, exhausted: exhausted.current };
}

interface UseDetailResult<T> {
  data: T | null;
  loading: boolean;
  error: string | null;
}

/** Fetch full movie detail (`/api/movies/:id`). */
export function useMovie(id: number): UseDetailResult<MovieDetail> {
  const api = useApi();
  const [data, setData] = useState<MovieDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    setLoading(true);
    api
      .movie(id, { signal: controller.signal })
      .then((d) => {
        setData(d);
        setError(null);
      })
      .catch((e: unknown) => {
        if (controller.signal.aborted) return;
        setError(e instanceof Error ? e.message : 'load failed');
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false);
      });
    return () => controller.abort();
  }, [api, id]);

  return { data, loading, error };
}

/** Fetch full series detail (`/api/series/:id`). */
export function useSeries(id: number): UseDetailResult<SeriesDetail> {
  const api = useApi();
  const [data, setData] = useState<SeriesDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    setLoading(true);
    api
      .series(id, { signal: controller.signal })
      .then((d) => {
        setData(d);
        setError(null);
      })
      .catch((e: unknown) => {
        if (controller.signal.aborted) return;
        setError(e instanceof Error ? e.message : 'load failed');
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false);
      });
    return () => controller.abort();
  }, [api, id]);

  return { data, loading, error };
}
