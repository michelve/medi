/**
 * Layout shell for the web SPA (Task 80). A thin header + an `<Outlet />` for the routed
 * page. Browse/playback/admin chrome lands in `81`/`82`; this establishes the themed
 * frame and the same-origin `ApiProvider`.
 */

import { Outlet, Link } from 'react-router-dom';
import { theme } from './theme';

export function App() {
  return (
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
          gap: 16,
          padding: '16px 24px',
          borderBottom: `1px solid ${theme.colors.surface}`,
        }}
      >
        <Link
          to="/"
          style={{ color: theme.colors.text, textDecoration: 'none', fontWeight: 700, fontSize: 20 }}
        >
          medi
        </Link>
      </header>
      <main style={{ padding: 24 }}>
        <Outlet />
      </main>
    </div>
  );
}
