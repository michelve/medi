/**
 * App-wide `ApiClient` provider for the web SPA (Task 80).
 *
 * Same-origin: the client is built with `baseUrl: ''`, so every request is relative
 * (`/api/...`). In production the medi binary serves both the SPA and the API on one
 * port; in dev, Vite's `server.proxy` forwards `/api` to the backend. Either way the app
 * code is identical and there is no CORS.
 *
 * Mirrors `apps/tv/src/api.tsx`, minus the Expo config plumbing (there is no per-device
 * base URL on the web — the origin *is* the server).
 */

import { createContext, useContext, useMemo, type ReactNode, createElement } from 'react';
import { ApiClient } from '@medi/api-client';

const ApiContext = createContext<ApiClient | null>(null);

/** A single shared, same-origin client. Memoized so the ETag cache persists per session. */
export function ApiProvider({ children }: { children: ReactNode }) {
  const client = useMemo(() => new ApiClient({ baseUrl: '' }), []);
  return createElement(ApiContext.Provider, { value: client }, children);
}

/** Access the shared `ApiClient`. Throws if used outside `<ApiProvider>`. */
export function useApi(): ApiClient {
  const client = useContext(ApiContext);
  if (!client) throw new Error('useApi must be used within <ApiProvider>.');
  return client;
}
