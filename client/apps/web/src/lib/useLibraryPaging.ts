/**
 * `useLibraryPaging` (Task 81) — keyset infinite-scroll state over `GET /api/library`.
 *
 * Owns the accumulated items, the opaque `next_cursor`, and load lifecycle. Exposes
 * `loadMore()` (idempotent while a fetch is in flight) for the grid sentinel to call.
 * Changing `sort` resets to page one (cursor cleared, items dropped) — the from-page-one
 * re-fetch the spec requires. All fetches are abortable and cancelled on unmount / re-run.
 *
 * Kept as a hook (not inline in the page) so the paging contract is isolated and the page
 * is pure rendering — the modularity the task asks for.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import { ApiError, type LibraryItem, type LibrarySort } from '@medi/api-client';
import { useApi } from '../api';

const PAGE_SIZE = 60;

export interface LibraryPagingState {
  items: LibraryItem[];
  /** True while the first page is loading (grid is empty). */
  initialLoading: boolean;
  /** True while a subsequent page is loading. */
  loadingMore: boolean;
  /** A non-null cursor remains ⇒ more pages exist. */
  hasMore: boolean;
  error: string | null;
  /** Request the next page; a no-op while a fetch is in flight or exhausted. */
  loadMore: () => void;
}

export function useLibraryPaging(sort: LibrarySort): LibraryPagingState {
  const api = useApi();
  const [items, setItems] = useState<LibraryItem[]>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(true);
  const [initialLoading, setInitialLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // In-flight guard (survives the async gap; refs don't trigger re-renders).
  const inFlight = useRef(false);
  // Latest cursor readable inside the stable `loadMore` without re-creating it.
  const cursorRef = useRef<string | null>(null);
  cursorRef.current = cursor;
  const hasMoreRef = useRef(true);
  hasMoreRef.current = hasMore;

  // The abort controller for the current sort generation; a sort change aborts it.
  const controllerRef = useRef<AbortController | null>(null);

  const fetchPage = useCallback(
    async (nextCursor: string | null, isInitial: boolean) => {
      // Capture this generation's controller up front so a later sort change
      // (which swaps `controllerRef.current`) can't confuse this call's aborts.
      const controller = controllerRef.current;
      if (inFlight.current) return;
      inFlight.current = true;
      if (isInitial) setInitialLoading(true);
      else setLoadingMore(true);
      try {
        const page = await api.library(
          { cursor: nextCursor, limit: PAGE_SIZE, sort },
          { signal: controller?.signal },
        );
        // Superseded by a newer generation while awaiting — drop the result.
        if (controller?.signal.aborted) return;
        setItems((prev) => (isInitial ? page.items : [...prev, ...page.items]));
        setCursor(page.next_cursor);
        setHasMore(page.next_cursor !== null);
        setError(null);
      } catch (err) {
        if (controller?.signal.aborted) return;
        if (err instanceof DOMException && err.name === 'AbortError') return;
        setError(err instanceof ApiError ? err.message : String(err));
        // Stop the sentinel from hammering a failing endpoint.
        setHasMore(false);
      } finally {
        // Only the current generation may clear the shared flag; a stale call
        // that was aborted must not unblock the fresh generation's fetch.
        if (controller === controllerRef.current) inFlight.current = false;
        if (isInitial) setInitialLoading(false);
        else setLoadingMore(false);
      }
    },
    [api, sort],
  );

  // (Re)start from page one whenever the sort key changes.
  useEffect(() => {
    const controller = new AbortController();
    controllerRef.current = controller;
    // New generation: clear any flag left set by a now-aborted previous fetch.
    inFlight.current = false;
    setItems([]);
    setCursor(null);
    setHasMore(true);
    setError(null);
    void fetchPage(null, true);
    return () => controller.abort();
  }, [fetchPage]);

  const loadMore = useCallback(() => {
    if (inFlight.current || !hasMoreRef.current) return;
    void fetchPage(cursorRef.current, false);
  }, [fetchPage]);

  return { items, initialLoading, loadingMore, hasMore, error, loadMore };
}
