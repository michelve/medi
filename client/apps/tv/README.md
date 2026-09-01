# @medi/tv

The unified TV app: a single Expo (react-native-tvos) codebase targeting Apple TV
and Android TV via Continuous Native Generation (CNG — no checked-in Xcode/Android
projects). Implemented in **Phase 4** (`docs/.tasks/40-phase4-tv-client-ui.md`);
playback overlays and packaging in **Phase 5**.

## Stack (pinned)

- **Expo SDK 54** (`~54.0.37`) with **CNG** — native `ios/`/`android/` are generated
  by `expo prebuild`, never committed (see root `.gitignore`).
- **react-native-tvos `0.81.5-2`**, aliased over `react-native` in `package.json`.
  Its `0.81` line strictly matches Expo SDK 54 (React Native 0.81), as the task
  requires. `expo install --check` will flag this alias as "expected 0.81.5" —
  that's benign: the tvos fork is a separate npm package on the same 0.81.5 base.
- **`@react-native-tvos/config-tv`** plugin with `isTV: true` — flips prebuild to
  tvOS + Android TV (leanback).
- **react-tv-space-navigation `5.2.0`** for deterministic D-pad focus.
- **xstate `5.32.6`** + **@xstate/react `6.1.0`** for the hover-to-play FSM.
- **react-native-video `6.19.2`** for the silent hover preview (full player: Phase 5).

## Workspaces consumed

| Package | Role |
|---|---|
| `@medi/api-client` | Typed fetch client for `docs/.tasks/02-api-contract.md` (ETag cache, keyset paging, asset URL builders). |
| `@medi/navigation` | Spatial-navigation layer: remote-control bridge, `Page` root, `FocusTrap`, `DirectionalOverride`, Apple TV `FocusGuide`. |
| `@medi/ui` | `HeroBanner`, `Carousel`, `PosterGrid`, `PosterCard`, and the xstate `HoverPreview`. |
| `@medi/player` | Phase 5 placeholder. |

## Screens

- **Home** — `HeroBanner` over virtualized `Carousel` rows fed by `/api/library`
  (keyset-paginated). Hero **Down** is a directional override that jumps to the
  first "Continue Watching" tile regardless of geometry.
- **Detail** — `/api/movies/:id` or `/api/series/:id`: backdrop, overview, credits,
  and focusable play rows.
- **Player** — Phase 4 stub: resolves `/api/stream/:file_id` (direct vs HLS) and
  shows the decision. Full playback is Phase 5.

## Run

From the monorepo root (`client/`):

```bash
yarn install
yarn tv            # expo start (dev)
yarn tv:prebuild   # generate native tvOS + Android TV projects (CNG)
yarn tv:ios        # build/run on the Apple TV simulator
yarn tv:android    # build/run on the Android TV emulator
```

Point the app at a backend with `MEDI_API_BASE_URL` (read at config-eval time
into `extra.apiBaseUrl`; defaults to `http://localhost:8080`).

## Type-checking

`yarn typecheck` (root) runs `tsc --noEmit` across every workspace.
