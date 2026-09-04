/**
 * Route table for the web SPA. Task 81 fills in the browse routes; Task 82 adds
 * playback (`/watch/:fileId`) and admin (`/settings/...`).
 *
 * `/`                    — landing: category rows (Task 91) / flat grid on search/sort
 * `/genre/:slug`         — one genre's keyset grid, keyed by the genre-name slug (`adventure`);
 *                          a numeric slug still resolves as the genre id (back-compat)
 * `/person/:id`          — person page: headshot + bio + filmography (Task 91)
 * `/movie/:id`           — movie detail. `:id` is the TMDB id for a matched title
 *                          (`/movie/98641`), falling back to the internal id when unmatched
 * `/series/:id`          — series detail. `:id` is the TMDB id, else the internal id
 * `/play/:fileId`        — in-browser player (direct / HLS). A SIBLING top-level route, NOT
 *                          a child of `App`: the player fills the whole viewport with no nav
 *                          chrome / max-width box (`docs/.tasks/97` Part A).
 * `/settings/libraries`  — library management (create/scan/edit/delete)
 * `*`                    — catch-all for deep links no page owns yet
 */

import { createBrowserRouter } from 'react-router-dom';
import { App } from './App';
import { LibraryPage } from './pages/LibraryPage';
import { GenrePage } from './pages/GenrePage';
import { PersonPage } from './pages/PersonPage';
import { MovieDetailPage } from './pages/MovieDetailPage';
import { SeriesDetailPage } from './pages/SeriesDetailPage';
import { PlayerPage } from './pages/PlayerPage';
import { LibrariesPage } from './pages/LibrariesPage';
import { StatusPage } from './pages/StatusPage';
import { NotFoundPage } from './pages/NotFoundPage';

export const router = createBrowserRouter([
  {
    path: '/',
    element: <App />,
    children: [
      { index: true, element: <LibraryPage /> },
      { path: 'genre/:slug', element: <GenrePage /> },
      { path: 'person/:id', element: <PersonPage /> },
      { path: 'movie/:id', element: <MovieDetailPage /> },
      { path: 'series/:id', element: <SeriesDetailPage /> },
      { path: 'settings/libraries', element: <LibrariesPage /> },
      { path: 'settings/status', element: <StatusPage /> },
      { path: '*', element: <NotFoundPage /> },
    ],
  },
  // The player is a sibling of `App`, not a child — it owns the whole viewport with no nav
  // bar / max-width box (`docs/.tasks/97` Part A).
  { path: '/play/:fileId', element: <PlayerPage /> },
]);
