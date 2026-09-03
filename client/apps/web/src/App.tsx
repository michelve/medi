/**
 * Layout shell for the web SPA. A themed header (home link + `SearchSortBar`) over a
 * routed `<Outlet/>`. The header controls and the library grid share `BrowseState`
 * (search query + sort), so the provider wraps the whole shell.
 *
 * Task 80 established the frame; Task 81 fills in the browse chrome. The search box and
 * sort toggle act on the library grid (`/`); on detail routes they're inert controls the
 * user can use to jump back and refine — harmless, and keeps the header stable.
 */

import { Outlet } from 'react-router-dom';
import { theme, detail } from './theme';
import { BrowseProvider } from './lib/browseState';
import { NavBar } from './components/NavBar';

export function App() {
  return (
    <BrowseProvider>
      <div
        style={{
          minHeight: '100vh',
          background: theme.colors.background,
          color: theme.colors.text,
          fontFamily: detail.fontFamily,
        }}
      >
        {/* Floating Apple-TV-style glass nav — a fixed, transparent-track overlay pinned to the
            top of the viewport. Content scrolls beneath it and the detail page's fixed gradient
            shows through the glass. */}
        <NavBar />
        {/* The fixed nav is out of flow, so the inner wrapper adds top padding to clear the
            floating pill. `minWidth: 0` lets flex/grid children shrink; `overflowX: hidden`
            keeps the body from scrolling sideways (wide rows scroll in their own container);
            the inner wrapper centers content and caps it at the max width on wide displays. */}
        <main style={{ minWidth: 0, overflowX: 'hidden' }}>
          <div style={{ maxWidth: detail.maxWidth, margin: '0 auto', padding: '108px 24px 24px' }}>
            <Outlet />
          </div>
        </main>
      </div>
    </BrowseProvider>
  );
}
