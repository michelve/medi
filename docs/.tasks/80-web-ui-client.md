# 80 — Web UI Client: Scaffolding, Serving & Packaging

> New cross-cutting phase, peer to `70-audio-quality-and-profiles.md`. Stands up the
> **browser** client that medi does not have. Depends on `00-architecture.md`,
> `02-api-contract.md` (the endpoints it consumes), and `40-phase4-tv-client-ui.md`
> (which owns `client/packages/api-client` type-sync). **Gap this closes:** the backend
> serves **only** `/api/*` — hitting `http://<host>:8096/` returns `404`, and the sole
> frontend is the native TV app (`client/apps/tv`, react-native-tvos). There is no
> Plex-style web page where a user can browse and play the library from a laptop or phone.
> This task is the **foundation**: a new `client/apps/web` SPA, the backend serving it at
> `/`, and the Docker image baking it in. Browse (`81`) and playback/admin (`82`) build on it.

## Purpose

Give medi a browser UI, served by the same binary on the same port as the API, with **zero
new backend dependencies** and **maximum reuse** of the existing client packages:

1. **`client/apps/web`** — a Vite + React + TypeScript SPA (fresh DOM components; **not**
   react-native-web). It reuses `@medi/api-client` **unchanged** and the framework-neutral
   logic modules; only the *visual* layer is new.
2. **Same-origin serving** — the Rust binary serves the built SPA at `/` via a static-file
   fallback, so `/api/*` and the UI share `0.0.0.0:8096`. No second container, no CORS.
3. **Baked into the image** — a Docker web build stage compiles the SPA and copies its
   `dist/` into the runtime image; the Unraid `WebUI` button finally opens a real page.

> **Why fresh DOM, not react-native-web.** The RN visual components
> (`PosterCard`/`PosterGrid`/`Carousel`/`HeroBanner`/`HoverPreview`, `VideoScreen`) are
> bound to `react-native` + `react-tv-space-navigation` + `react-native-video`. Rendering
> them in a browser would require solving spatial-navigation-on-web and swapping the video
> engine — heavier and more fragile than writing plain DOM components. The **logic** is
> already framework-neutral and *is* reused (see Reuse below).

## Requirements

- **No new backend crate and no new Cargo dependency.** `tower-http` already has the `fs`
  feature and `routes.rs` already imports `ServeDir`/`ServeFile`.
- The web assets ship **in the image**, never under `/config` (read-write appdata) or
  `/media` (read-only library). This preserves the `/config` vs `/media` contract from `00`.
- The static fallback **must not shadow** any `/api/*` route — it fires only when no API
  route matched.
- **No auth** — LAN-appliance invariant (`00`). The web UI is unauthenticated like the API.
- Same-origin: the SPA constructs `new ApiClient({ baseUrl: '' })` so requests are relative
  (`/api/...`) both when served by the binary and behind the Vite dev proxy.

## Packages / crates

- **New:** `client/apps/web` — Yarn workspace member `@medi/web` (Vite React-TS).
- **Touched:** `backend/crates/core` (add `web_dir()`), `backend/crates/api` (fallback
  service in `routes.rs`), `docker/Dockerfile` (web build stage + COPY), `.dockerignore`
  (stop excluding `client/`), `client/package.json` (web scripts).

## Reuse (do not reinvent — these are browser-safe today)

- **`@medi/api-client`** (`client/packages/api-client/src/*`) — pure `fetch`, zero deps.
  Drop in unchanged; it already exposes `library`/`movie`/`series`/`stream`/`refreshMovie`/
  `movieMatches`/`matchMovie`/`libraries`/`createLibrary`/`patchLibrary`/`deleteLibrary`/
  `scanLibrary`/`health` plus URL builders `imageUrl`/`directUrl`/`hlsUrl`/`trickplayUrl`/
  `previewUrl`, and `ApiError` (`isNotFound`/`isBusy`).
- **`@medi/ui` `theme.ts` + `types.ts`** — pure data/types (poster sizing, colors,
  `PosterItem`). Reuse the tokens for consistent look with the TV app.

## File structure (where to save)

```
client/
  apps/
    web/                       # NEW — @medi/web, Vite React-TS SPA
      index.html
      package.json             # name @medi/web; deps: react, react-dom, react-router;
                               #   workspace "@medi/api-client": "*"; scripts build/dev
      vite.config.ts           # dev proxy /api -> http://<server>:8096
      tsconfig.json            # extends ../../tsconfig.base.json; @medi/* path aliases
      src/
        main.tsx               # ReactDOM root
        App.tsx                # layout shell
        api.ts                 # new ApiClient({ baseUrl: '' }) + React context (useApi)
        router.tsx             # routes (fleshed out in 81/82)
        theme.ts               # re-export/adapt @medi/ui theme for DOM
        components/            # DOM components (81/82)
        pages/                 # route pages (81/82)
```

The runtime image path for the built assets is **`/usr/share/medi/web`** (see sub-task 3).

## Sub-tasks

1. **Scaffold `client/apps/web`** — create the Vite React-TS app as a Yarn workspace
   member (`"name": "@medi/web"`, `"private": true`). Add `web:dev` / `web:build` /
   `web:typecheck` scripts to `client/package.json` mirroring the existing `tv:*` pattern.
   In `apps/web/tsconfig.json` extend `../../tsconfig.base.json` and add `paths` aliases
   mapping `@medi/api-client` → `../../packages/api-client/src` (mirror `apps/tv/tsconfig.json`).
2. **API wiring** (`src/api.ts`) — construct `new ApiClient({ baseUrl: '' })` and expose it
   via a React context hook `useApi()` (mirror `apps/tv/src/api.tsx`). In `vite.config.ts`
   add a dev-server `server.proxy` forwarding `/api` → the server (`http://<server>:8096`)
   so dev and prod both use relative URLs.
3. **`web_dir()` in `backend/crates/core/src/config.rs`** — add a `web_dir` field defaulting
   to `/usr/share/medi/web`, add `"web_dir"` to the `KEYS` array so `WEB_DIR` /
   `MEDI_WEB_DIR` override it, extend `Default` and the env-var doc table. State the
   invariant in the doc comment: **web assets ship in the image, never under `config_dir`.**
4. **Serve at `/` in `backend/crates/api/src/routes.rs`** — build
   `ServeDir::new(config.web_dir()).not_found_service(ServeFile::new(<web_dir>/index.html))`
   and attach it as `.fallback_service(web)` immediately before `.with_state(state)`. The
   `not_found_service(index.html)` gives SPA history-fallback (deep links return the shell
   with `200`). No `Cargo.toml` change. **Verify `/api/*` still wins** — the fallback only
   fires when no route matched.
5. **Docker web build stage** (`docker/Dockerfile`) — add before the `runtime` stage:
   `FROM node:22-bookworm AS web` · `COPY client/ ./client/` ·
   `corepack enable && yarn install --frozen-lockfile && yarn workspace @medi/web build`
   (emits `client/apps/web/dist`). Then in `runtime`, next to the binary copy:
   `COPY --from=web /web/client/apps/web/dist/ /usr/share/medi/web/`.
6. **`.dockerignore`** — remove the `client` line so the web stage sees the source; keep
   `**/node_modules` and `**/target` so the context stays small (a clean `yarn install`
   runs in the image).
7. **Confirm the Unraid `WebUI`** — the template's `<WebUI>http://[IP]:[PORT:8096]/</WebUI>`
   now resolves to the SPA. No template change required beyond confirming it no longer 404s.

## Verification

> Note: this dev machine has no Rust/Node toolchain on PATH — build steps run in Docker
> (on the Unraid/Linux host) or CI.

- **Web build** (`yarn workspace @medi/web build`): emits `client/apps/web/dist/index.html`
  plus hashed asset bundles.
- **Serving** (container running): `curl -fsS http://localhost:8096/` → `200` and HTML.
- **SPA deep link**: `curl -fsS http://localhost:8096/movie/1` → `200` returning the app
  shell (history fallback), **not** a `404`.
- **API not clobbered**: `curl -fsS http://localhost:8096/api/health` → `ok`;
  `curl -fsS http://localhost:8096/api/library` → the JSON catalog page.
- **Same-origin**: opening `http://<unraid-ip>:8096/` in a browser loads the SPA and its
  `fetch('/api/library')` succeeds with no CORS error.

## Cross-references (edits required in lockstep)

- `00-architecture.md` — add `client/apps/web` to the repository/directory layout tree and
  note the binary now serves a browser UI at `/`.
- `02-api-contract.md` — note that `/` (and any non-`/api` path) now serves the SPA via a
  static fallback; the `/api/*` contract is unchanged.
- `40-phase4-tv-client-ui.md` — owns `client/packages/api-client` type-sync; the web app
  consumes those types and must not fork them.
