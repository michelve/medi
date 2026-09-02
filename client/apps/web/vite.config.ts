import { fileURLToPath, URL } from 'node:url';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

/**
 * Vite config for the medi web SPA (Task 80).
 *
 * - Aliases `@medi/api-client` (and the two browser-safe `@medi/ui` submodules) straight
 *   to the workspace source, mirroring `apps/web/tsconfig.json` — no build step for the
 *   shared packages, and the api-client stays the single source of truth (Task 40).
 * - Dev proxy: forward `/api` to the running backend so the app uses **relative** URLs in
 *   both dev and prod (`new ApiClient({ baseUrl: '' })`). Override the target with
 *   `MEDI_DEV_API` (e.g. a real Unraid box) when the backend isn't on localhost.
 */
const apiTarget = process.env.MEDI_DEV_API ?? 'http://localhost:8096';

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '@medi/api-client': fileURLToPath(
        new URL('../../packages/api-client/src', import.meta.url),
      ),
      '@medi/ui/theme': fileURLToPath(new URL('../../packages/ui/src/theme', import.meta.url)),
      '@medi/ui/types': fileURLToPath(new URL('../../packages/ui/src/types', import.meta.url)),
    },
  },
  server: {
    proxy: {
      '/api': {
        target: apiTarget,
        changeOrigin: true,
      },
    },
  },
  build: {
    // Emitted into the image by the Docker web stage; copied to /usr/share/medi/web.
    outDir: 'dist',
    sourcemap: false,
  },
});
