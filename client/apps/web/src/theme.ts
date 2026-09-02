/**
 * DOM theme for the web SPA (Task 80). Re-exports the shared `@medi/ui` tokens so the
 * browser UI matches the TV app's palette and poster sizing, and adds a couple of
 * CSS-friendly helpers. The `@medi/ui` root index pulls in react-native components, so we
 * import the pure `theme` submodule directly (aliased in tsconfig/vite).
 */

import { theme } from '@medi/ui/theme';

export { theme };
export type { Theme } from '@medi/ui/theme';

/** The shared color tokens, ready to drop into inline styles or CSS variables. */
export const colors = theme.colors;

/** Install the theme colors as `--medi-*` CSS custom properties on `:root`. */
export function installThemeVars(root: HTMLElement = document.documentElement): void {
  root.style.setProperty('--medi-bg', theme.colors.background);
  root.style.setProperty('--medi-surface', theme.colors.surface);
  root.style.setProperty('--medi-text', theme.colors.text);
  root.style.setProperty('--medi-text-muted', theme.colors.textMuted);
  root.style.setProperty('--medi-accent', theme.colors.accent);
}

/**
 * Install the handful of global rules the inline-styled components can't express
 * (`:hover`, `::placeholder`). Idempotent — guarded by an id so React StrictMode's
 * double-invoke or a hot reload won't stack duplicate `<style>` tags.
 */
export function installGlobalStyles(doc: Document = document): void {
  const id = 'medi-global-styles';
  if (doc.getElementById(id)) return;
  const style = doc.createElement('style');
  style.id = id;
  style.textContent = `
    .medi-poster-card { transition: transform 120ms ease; }
    .medi-poster-card:hover { transform: scale(1.04); }
    .medi-search-input::placeholder { color: ${theme.colors.textMuted}; }
  `;
  doc.head.appendChild(style);
}
