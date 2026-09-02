/**
 * Catch-all page (Task 80 scaffold). Deep links resolve to the app shell (the backend's
 * history-fallback returns index.html with 200), then the client router lands here for a
 * path no page owns yet. `81`/`82` replace the `*` route with real pages.
 */

import { Link } from 'react-router-dom';
import { theme } from '../theme';

export function NotFoundPage() {
  return (
    <section>
      <h1 style={{ fontSize: 24, margin: '0 0 8px' }}>Not found</h1>
      <p style={{ color: theme.colors.textMuted, margin: '0 0 16px' }}>
        There's nothing here yet.
      </p>
      <Link to="/" style={{ color: theme.colors.accent }}>
        Back to the library
      </Link>
    </section>
  );
}
