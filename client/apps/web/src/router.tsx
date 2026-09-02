/**
 * Route table for the web SPA. Task 81 fills in the browse routes; Task 82 adds
 * playback (`/watch/:fileId`) and admin (`/settings/...`).
 *
 * `/`                    — library poster wall (infinite scroll + search/sort)
 * `/movie/:id`           — movie detail (overview, credits, files)
 * `/series/:id`          — series detail (seasons → episodes)
 * `/play/:fileId`        — in-browser player (direct / HLS)
 * `/settings/libraries`  — library management (create/scan/edit/delete)
 * `*`                    — catch-all for deep links no page owns yet
 */

import { createBrowserRouter } from 'react-router-dom';
import { App } from './App';
import { LibraryPage } from './pages/LibraryPage';
import { MovieDetailPage } from './pages/MovieDetailPage';
import { SeriesDetailPage } from './pages/SeriesDetailPage';
import { PlayerPage } from './pages/PlayerPage';
import { LibrariesPage } from './pages/LibrariesPage';
import { NotFoundPage } from './pages/NotFoundPage';

export const router = createBrowserRouter([
  {
    path: '/',
    element: <App />,
    children: [
      { index: true, element: <LibraryPage /> },
      { path: 'movie/:id', element: <MovieDetailPage /> },
      { path: 'series/:id', element: <SeriesDetailPage /> },
      { path: 'play/:fileId', element: <PlayerPage /> },
      { path: 'settings/libraries', element: <LibrariesPage /> },
      { path: '*', element: <NotFoundPage /> },
    ],
  },
]);
