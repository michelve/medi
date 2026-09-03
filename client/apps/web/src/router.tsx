/**
 * Route table for the web SPA. Task 81 fills in the browse routes; Task 82 adds
 * playback (`/watch/:fileId`) and admin (`/settings/...`).
 *
 * `/`                    — landing: category rows (Task 91) / flat grid on search/sort
 * `/genre/:id`           — one genre's keyset grid (Task 91)
 * `/person/:id`          — person page: headshot + bio + filmography (Task 91)
 * `/movie/:id`           — movie detail (overview, credits, files)
 * `/series/:id`          — series detail (seasons → episodes)
 * `/play/:fileId`        — in-browser player (direct / HLS)
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
      { path: 'genre/:id', element: <GenrePage /> },
      { path: 'person/:id', element: <PersonPage /> },
      { path: 'movie/:id', element: <MovieDetailPage /> },
      { path: 'series/:id', element: <SeriesDetailPage /> },
      { path: 'play/:fileId', element: <PlayerPage /> },
      { path: 'settings/libraries', element: <LibrariesPage /> },
      { path: 'settings/status', element: <StatusPage /> },
      { path: '*', element: <NotFoundPage /> },
    ],
  },
]);
