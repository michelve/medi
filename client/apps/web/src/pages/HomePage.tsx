/**
 * Scaffold home page (Task 80). The full browse grid lands in `81`; for now this proves
 * the same-origin wiring end to end — it calls `/api/health` and `/api/library` through
 * the shared `ApiClient` and renders the result, so a green page here means the binary is
 * serving the SPA and the API on one origin with no CORS.
 */

import { useEffect, useState } from 'react';
import { ApiError, type LibraryItem } from '@medi/api-client';
import { useApi } from '../api';
import { theme } from '../theme';

type State =
  | { status: 'loading' }
  | { status: 'ready'; healthy: boolean; items: LibraryItem[] }
  | { status: 'error'; message: string };

export function HomePage() {
  const api = useApi();
  const [state, setState] = useState<State>({ status: 'loading' });

  useEffect(() => {
    const ctrl = new AbortController();
    (async () => {
      try {
        const healthy = await api.health({ signal: ctrl.signal });
        const page = await api.library({ limit: 60 }, { signal: ctrl.signal });
        setState({ status: 'ready', healthy, items: page.items });
      } catch (err) {
        if (ctrl.signal.aborted) return;
        const message = err instanceof ApiError ? err.message : String(err);
        setState({ status: 'error', message });
      }
    })();
    return () => ctrl.abort();
  }, [api]);

  if (state.status === 'loading') {
    return <p style={{ color: theme.colors.textMuted }}>Loading library…</p>;
  }
  if (state.status === 'error') {
    return <p style={{ color: '#ff6b6b' }}>Failed to reach the server: {state.message}</p>;
  }

  return (
    <section>
      <h1 style={{ fontSize: 24, margin: '0 0 8px' }}>Library</h1>
      <p style={{ color: theme.colors.textMuted, margin: '0 0 20px' }}>
        Server {state.healthy ? 'online' : 'unreachable'} · {state.items.length} title
        {state.items.length === 1 ? '' : 's'} on this page. The full browse grid arrives in
        the next task.
      </p>
      <ul style={{ listStyle: 'none', padding: 0, margin: 0, display: 'grid', gap: 8 }}>
        {state.items.map((item) => (
          <li key={`${item.kind}-${item.id}`} style={{ color: theme.colors.text }}>
            {item.title}
            {item.year ? ` (${item.year})` : ''}
          </li>
        ))}
      </ul>
    </section>
  );
}
