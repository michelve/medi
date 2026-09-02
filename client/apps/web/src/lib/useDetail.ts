/**
 * `useDetail` (Task 81) — fetch one detail resource by id with the standard lifecycle:
 * loading → (data | notFound | error), abortable on unmount / id change.
 *
 * Shared by the movie and series pages so both handle `ApiError.isNotFound` and teardown
 * identically. The caller passes a fetcher bound to the id (e.g. `(signal) =>
 * api.movie(id, { signal })`); we own the state machine.
 */

import { useEffect, useState } from 'react';
import { ApiError } from '@medi/api-client';

export type DetailState<T> =
  | { status: 'loading' }
  | { status: 'ready'; data: T }
  | { status: 'not_found' }
  | { status: 'error'; message: string };

/**
 * @param fetcher  Loads the resource; must forward the abort `signal`.
 * @param deps     Re-run when these change (typically `[id]`).
 */
export function useDetail<T>(
  fetcher: (signal: AbortSignal) => Promise<T>,
  deps: React.DependencyList,
): DetailState<T> {
  const [state, setState] = useState<DetailState<T>>({ status: 'loading' });

  useEffect(() => {
    const controller = new AbortController();
    setState({ status: 'loading' });
    (async () => {
      try {
        const data = await fetcher(controller.signal);
        if (!controller.signal.aborted) setState({ status: 'ready', data });
      } catch (err) {
        if (controller.signal.aborted) return;
        if (err instanceof ApiError && err.isNotFound) {
          setState({ status: 'not_found' });
          return;
        }
        setState({ status: 'error', message: err instanceof ApiError ? err.message : String(err) });
      }
    })();
    return () => controller.abort();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);

  return state;
}
