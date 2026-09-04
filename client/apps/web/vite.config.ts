import { fileURLToPath, URL } from 'node:url';
import { defineConfig, loadEnv } from 'vite';
import react from '@vitejs/plugin-react';

/**
 * Vite config for the medi web SPA (Task 80).
 *
 * - Aliases `@medi/api-client` (and the two browser-safe `@medi/ui` submodules) straight
 *   to the workspace source, mirroring `apps/web/tsconfig.json` — no build step for the
 *   shared packages, and the api-client stays the single source of truth (Task 40).
 * - Dev proxy: forward `/api` to the running backend so the app uses **relative** URLs in
 *   both dev and prod (`new ApiClient({ baseUrl: '' })`). Point the proxy at a non-local
 *   backend (e.g. the Unraid box) with `MEDI_DEV_API` — set it in the shell, or drop it in
 *   an untracked `apps/web/.env.local` (`MEDI_DEV_API=http://192.168.5.242:8096`) so a bare
 *   `yarn web:dev` picks it up without a rebuild.
 */

export default defineConfig(({ mode }) => {
  // loadEnv reads .env / .env.local (the '' prefix loads every var, not just VITE_*).
  // A real shell env var still wins over the file.
  const env = loadEnv(mode, fileURLToPath(new URL('.', import.meta.url)), '');
  const apiTarget = process.env.MEDI_DEV_API ?? env.MEDI_DEV_API ?? 'http://localhost:8096';

  return {
  plugins: [react()],
  resolve: {
    alias: {
      '@medi/api-client': fileURLToPath(
        new URL('../../packages/api-client/src', import.meta.url),
      ),
      '@medi/ui/theme': fileURLToPath(new URL('../../packages/ui/src/theme', import.meta.url)),
      '@medi/ui/types': fileURLToPath(new URL('../../packages/ui/src/types', import.meta.url)),
      // Pure, RN-free player submodules (state reducer + trickplay math). The `@medi/player`
      // root re-exports react-native components, so import these directly — same rule as
      // `@medi/ui/theme`.
      '@medi/player/usePlayerControls': fileURLToPath(
        new URL('../../packages/player/src/usePlayerControls', import.meta.url),
      ),
      '@medi/player/trickplay': fileURLToPath(
        new URL('../../packages/player/src/trickplay', import.meta.url),
      ),
      '@medi/player/chapters': fileURLToPath(
        new URL('../../packages/player/src/chapters', import.meta.url),
      ),
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
  };
});
