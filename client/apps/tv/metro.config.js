/**
 * Metro config tuned for the Yarn-workspaces monorepo (README §Organization).
 *
 * Two things a monorepo needs that the default config doesn't do:
 *  1. `watchFolders`: watch the repo root so edits to `packages/*` hot-reload.
 *  2. `nodeModulesPaths`: resolve modules from BOTH the app and the workspace
 *     root, so hoisted deps (react, react-native, xstate, …) are found once.
 *
 * The `@medi/*` packages are consumed straight from their `src/` (their
 * package.json `main` points at `src/index.ts`), so Metro transpiles them like
 * app code — no build step, which keeps the workspaces fast to iterate on.
 */

const { getDefaultConfig } = require('expo/metro-config');
const path = require('path');

const projectRoot = __dirname;
// apps/tv → client (the workspace root).
const workspaceRoot = path.resolve(projectRoot, '../..');

const config = getDefaultConfig(projectRoot);

config.watchFolders = [workspaceRoot];

config.resolver.nodeModulesPaths = [
  path.resolve(projectRoot, 'node_modules'),
  path.resolve(workspaceRoot, 'node_modules'),
];

// Prefer a single copy of these singletons from the workspace root to avoid the
// "two Reacts"/"two react-native" class of errors in a hoisted monorepo.
config.resolver.disableHierarchicalLookup = false;

module.exports = config;
