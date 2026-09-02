/**
 * Route table for the web SPA (Task 80 scaffold). Browse (`81`) and playback/admin (`82`)
 * flesh these out; for now `/` renders a minimal home that proves same-origin fetch works,
 * and a catch-all keeps deep links from erroring before their pages exist.
 */

import { createBrowserRouter } from 'react-router-dom';
import { App } from './App';
import { HomePage } from './pages/HomePage';
import { NotFoundPage } from './pages/NotFoundPage';

export const router = createBrowserRouter([
  {
    path: '/',
    element: <App />,
    children: [
      { index: true, element: <HomePage /> },
      // 81/82 add: /movie/:id, /series/:id, /watch/:fileId, /settings/libraries …
      { path: '*', element: <NotFoundPage /> },
    ],
  },
]);
