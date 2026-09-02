/**
 * Layout shell for the web SPA. A themed header (home link + `SearchSortBar`) over a
 * routed `<Outlet/>`. The header controls and the library grid share `BrowseState`
 * (search query + sort), so the provider wraps the whole shell.
 *
 * Task 80 established the frame; Task 81 fills in the browse chrome. The search box and
 * sort toggle act on the library grid (`/`); on detail routes they're inert controls the
 * user can use to jump back and refine — harmless, and keeps the header stable.
 */

import { Outlet, Link } from 'react-router-dom';
import { theme } from './theme';
import { BrowseProvider } from './lib/browseState';
import { SearchSortBar } from './components/SearchSortBar';

export function App() {
  return (
    <BrowseProvider>
      <div
        style={{
          minHeight: '100vh',
          background: theme.colors.background,
          color: theme.colors.text,
          fontFamily:
            '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif',
        }}
      >
        <header
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 24,
            padding: '16px 24px',
            borderBottom: `1px solid ${theme.colors.surface}`,
            position: 'sticky',
            top: 0,
            zIndex: 10,
            background: theme.colors.background,
          }}
        >
          <Link
            to="/"
            style={{
              color: theme.colors.text,
              textDecoration: 'none',
              fontWeight: 700,
              fontSize: 20,
              flex: '0 0 auto',
            }}
          >
            medi
          </Link>
          <div style={{ flex: '1 1 auto' }}>
            <SearchSortBar />
          </div>
          <Link
            to="/settings/libraries"
            style={{
              color: theme.colors.textMuted,
              textDecoration: 'none',
              fontSize: 14,
              flex: '0 0 auto',
            }}
          >
            Libraries
          </Link>
        </header>
        <main style={{ padding: 24 }}>
          <Outlet />
        </main>
      </div>
    </BrowseProvider>
  );
}
