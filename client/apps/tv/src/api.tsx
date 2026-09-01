/**
 * App-wide `ApiClient` provider. Reads the backend base URL from the Expo config
 * `extra.apiBaseUrl` (set in `app.config.ts`, overridable via `MEDI_API_BASE_URL`)
 * and exposes a single shared client + a preview-availability resolver used by
 * the hover FSM.
 */

import React, { createContext, useContext, useMemo } from 'react';
import Constants from 'expo-constants';
import { ApiClient, ApiError, type PosterItem } from './deps';

const ApiContext = createContext<ApiClient | null>(null);

export function ApiProvider({ children }: { children: React.ReactNode }): React.JSX.Element {
  const client = useMemo(() => {
    const baseUrl =
      (Constants.expoConfig?.extra?.apiBaseUrl as string | undefined) ??
      'http://localhost:8080';
    return new ApiClient({ baseUrl });
  }, []);

  return <ApiContext.Provider value={client}>{children}</ApiContext.Provider>;
}

export function useApi(): ApiClient {
  const client = useContext(ApiContext);
  if (!client) throw new Error('useApi must be used within <ApiProvider>.');
  return client;
}

/**
 * Build the `resolvePreview` the hover FSM needs: verify the 720p clip exists
 * (a `HEAD`, cancellable via the FSM's abort signal) and return its URL. A 404
 * (not yet generated) rejects, so the machine tears down cleanly to `idle`.
 */
export function usePreviewResolver(): (item: PosterItem, signal: AbortSignal) => Promise<string> {
  const client = useApi();
  return useMemo(
    () => async (item: PosterItem, signal: AbortSignal) => {
      // Only items with a media_file id (detail rows) can preview; the browse
      // grid's library cards have none and render a static PosterCard instead.
      if (item.previewFileId == null) {
        throw new ApiError(0, 'no_preview', 'item has no previewable file');
      }
      const url = client.previewUrl(item.previewFileId);
      // Cheap existence check honoring the abort (scroll-away cancels it).
      const res = await fetch(url, { method: 'HEAD', signal });
      if (!res.ok) {
        throw new ApiError(res.status, 'preview_unavailable', 'preview not generated');
      }
      return url;
    },
    [client],
  );
}
