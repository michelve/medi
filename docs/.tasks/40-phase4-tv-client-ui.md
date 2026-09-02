# 40 — Phase 4: Unified TV Client UI

> Maps to README §Development Roadmap → Phase 4. Consumes the API from `02-api-contract.md`.
> Lives in `client/` (Yarn workspaces monorepo).

## Purpose

Build a single TypeScript codebase that runs on both Apple TV (tvOS) and Android TV, with
deterministic D-pad spatial navigation and the Netflix-style delayed hover-to-play trailer
mechanic governed by a strict finite state machine.

## Requirements

- One codebase, both platforms: **Expo SDK 54+** with **react-native-tvos** (version must
  strictly match the Expo SDK), using **Continuous Native Generation (CNG)** — no hand-managed
  Xcode/Android Studio projects.
- **Yarn workspaces** monorepo separating shared UI from app config (README §Organization).
- Deterministic focus via **react-tv-space-navigation** (not the flaky native nearest-neighbor
  heuristic); Apple TV focus bridged with **TVFocusGuideView**.
- Hover-to-play governed by an **xstate** machine with a strict **2-second gate**.

## Packages

`expo` (SDK 54+), `react-native-tvos` (matched to SDK), `react-tv-space-navigation`,
`xstate` + `@xstate/react`, `react-native-video` (Phase 5), TypeScript. Scaffold from the
`template-tv` community boilerplate. `react-native-mmkv`/simple store optional for UI prefs.

## File structure (where to save)

```
client/
├── package.json               # "workspaces": ["apps/*","packages/*"]
├── apps/tv/                    # Expo app; app.json/app.config with CNG TV config
│   ├── app.config.ts           # expo-tv plugin, isTV, appleTV + androidTV targets
│   └── src/screens/            # Home, Detail, Player screens
└── packages/
    ├── ui/src/                 # PosterGrid, Carousel, HeroBanner, HoverPreview
    ├── navigation/src/         # SpatialNavigation setup, focus traps, TVFocusGuideView helpers
    ├── player/src/             # (Phase 5) video wrapper + overlay
    └── api-client/src/         # typed fetch client for 02-api-contract.md endpoints
```

## Spatial navigation engineering (`packages/navigation`)

Implement the three paradigms from README §Spatial Navigation:
- **Focus traps** — modals/side-drawers trap focus; D-pad can't reach obscured background.
- **Directional overrides** — e.g. "Down" on hero banner jumps to first item of "Continue
  Watching" carousel regardless of geometry.
- **TVFocusGuideView** (Apple TV) — invisible bridges over empty/asymmetrical space using
  Apple's UIFocusGuide, so tvOS focus follows the designer's intent, not nearest-neighbor.

## Hover-to-play FSM (`packages/ui/HoverPreview`, xstate)

States (README §Deterministic State Management):
1. `awaitingBackgroundImageLoad` — no video logic runs until the poster image is fully
   loaded/cached/rendered (`REPORT_IMAGE_LOADED`). Prevents visual pop.
2. `idle` — after image load; waits for focus.
3. `waiting` → the **2-second gate** (`showingVideo.loadingVideoSrc.cannotMoveOn`) on
   `MOUSE_OVER`/D-pad focus. Prevents flooding the LAN while the user scrolls fast.
4. `playing` — after the 2s timer, `REPORT_VIDEO_LOADED` mounts `react-native-video` and
   plays the `/api/preview/:file_id` clip silently.
5. **Teardown** — on `MOUSE_OUT` at any instant (during gate or playback), abort the request,
   unmount the video, return to `idle`. No orphaned playback threads / concurrent audio.

## Sub-tasks

1. Scaffold the Yarn-workspace monorepo from `template-tv`; configure Expo CNG for both TV
   targets; verify `react-native-tvos` version matches the Expo SDK exactly.
2. `api-client`: typed client for the catalog/detail/preview/trickplay endpoints. This
   package owns the hand-written wire types (its `types.ts` is the single source of truth).
   The browser SPA (`client/apps/web`, `80-web-ui-client.md`) consumes `@medi/api-client`
   **unchanged** and must not fork these types — extend them here, in lockstep with the
   backend DTOs, so TV and web stay in sync.
3. `navigation`: wrap the app in react-tv-space-navigation; build focus-trap + directional
   override helpers + TVFocusGuideView wrappers.
4. `ui`: PosterGrid, horizontal Carousels, HeroBanner; wire the xstate HoverPreview FSM.
5. `apps/tv`: Home screen (rows from `/api/library`), Detail screen (`/api/movies|series/:id`).

## Scaling notes

- Virtualize long grids/carousels so scrolling 10,000 posters stays smooth on weak TV silicon.
- The 2-second gate is itself a scaling control — it throttles preview requests to the backend.
- Cache poster images aggressively on-device; only fetch preview clips after the gate fires.

## Verification

- Build and run on Apple TV simulator + Android TV emulator from the one codebase (CNG).
- D-pad navigation is deterministic on both: focus never traps or jumps to the wrong item;
  hero→carousel directional override works.
- Fast-scrolling past posters never starts a video (gate cancels); dwelling 2s starts a silent
  preview; moving away instantly tears it down (no lingering audio/second video).
