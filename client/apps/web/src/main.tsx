/**
 * ReactDOM entry point for the web SPA (Task 80). Wraps the router in the same-origin
 * `ApiProvider` and installs the shared theme's CSS variables.
 */

import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { RouterProvider } from 'react-router-dom';
import { ApiProvider } from './api';
import { router } from './router';
import { installThemeVars, installGlobalStyles } from './theme';

installThemeVars();
installGlobalStyles();

const container = document.getElementById('root');
if (!container) throw new Error('missing #root element');

createRoot(container).render(
  <StrictMode>
    <ApiProvider>
      <RouterProvider router={router} />
    </ApiProvider>
  </StrictMode>,
);
