# 81 — Web UI Client: Library Grid, Detail Pages, Search & Filter

> Depends on `80-web-ui-client.md` (the `client/apps/web` scaffold, `useApi()`, and the
> `/`-served SPA). Consumes the read endpoints in `02-api-contract.md`. **Gap this closes:**
> `80` stands up an empty shell — this task delivers the actual **browse** experience: a
> poster wall, movie/series detail pages, and search/sort, so the web UI is usable as a
> catalog even before in-browser playback (`82`) lands.

## Purpose

Build the read/browse surface of the web UI as fresh DOM components:

1. **Library grid** — a responsive poster wall over `GET /api/library`, with keyset
   infinite scroll and HDR badges.
2. **Detail pages** — movie and series pages showing overview, artwork, credits, and the
   per-title file list (with Play affordances wired in `82`).
3. **Search & filter** — a client-side search box over the loaded catalog plus a sort
   toggle wired to the server's `sort` param.

## Requirements

- Use `@medi/api-client` exclusively for data; never hand-roll `fetch` or hard-code
  `/api/...` paths — use the client methods and URL builders.
- Poster/backdrop `src` come from the client URL builders (`imageUrl`), which already
  return `/api/images/...`; a missing poster degrades to a titled placeholder tile.
- Pagination is **keyset** via the opaque `next_cursor`; never assume offset paging.
- Reuse `@medi/ui` `theme.ts` sizing/colors so the web grid matches the TV app's look.

## Packages / crates

- **Touched (all under `client/apps/web`):** `src/router.tsx`, `src/App.tsx`,
  `src/components/*`, `src/pages/*`. New dep: `react-router` (routing only).

## File structure (where to save)

```
client/apps/web/src/
  router.tsx                 # routes: "/", "/movie/:id", "/series/:id"
  App.tsx                    # shell: header (search + sort), <Outlet/>
  components/
    PosterGrid.tsx           # CSS-grid poster wall + IntersectionObserver sentinel
    PosterCard.tsx           # single tile: <img>, title/year, HDR badge
    HdrBadge.tsx             # maps item.hdr -> label (DV / HDR10 / HDR10+ / HLG)
    CreditsList.tsx          # billing list from Credit[]
    FileList.tsx             # MediaFile[] rows: codec / resolution / HDR / size
    SearchSortBar.tsx        # search box + sort toggle
  pages/
    LibraryPage.tsx          # "/"     — grid + infinite scroll
    MovieDetailPage.tsx      # "/movie/:id"
    SeriesDetailPage.tsx     # "/series/:id"
```

## Sub-tasks

1. **Routing & shell** — add `react-router`; define routes `/`, `/movie/:id`,
   `/series/:id` in `router.tsx`; `App.tsx` renders a header (title, `SearchSortBar`) and
   an `<Outlet/>`. A `LibraryItem`'s `kind` selects the detail route on click.
2. **`PosterGrid` + `PosterCard` (DOM)** — CSS `grid` of cards; each card is an `<img>`
   (`client.imageUrl(item.poster)`), title, optional year, and an `HdrBadge` when
   `item.hdr` is set. Pull card width/gap/radius from `@medi/ui` `theme.ts`.
3. **Infinite scroll** — `LibraryPage` calls `client.library({ cursor, limit, sort })`,
   appends `items`, and stores `next_cursor`. An `IntersectionObserver` sentinel at the
   grid's end fetches the next page until `next_cursor === null`. Guard against overlapping
   fetches (in-flight flag) and abort on unmount (`AbortSignal` via `RequestOptions`).
4. **Movie detail** (`MovieDetailPage`) — `client.movie(id)`: backdrop
   (`client.imageUrl(backdrop_path)`), title/year, `overview`, `CreditsList` from `credits`,
   and `FileList` from `media_files` (container, `video_codec`, `width×height`, `hdr_type`,
   `size_bytes`). Each file row carries a **Play** button (handler stubbed here; wired in `82`).
   Handle `ApiError.isNotFound` → a friendly "not found" state.
5. **Series detail** (`SeriesDetailPage`) — `client.series(id)`: header like movies, then
   `seasons` → each `SeasonWithEpisodes` rendered as an episode list (number, title,
   overview) with a per-episode **Play** button (wired in `82`).
6. **Search & filter** (`SearchSortBar`) — a text box filters the currently-loaded grid
   items client-side (case-insensitive title match); a sort toggle switches
   `LibraryQuery.sort` between `sort_title` and `added_at` and **re-fetches from the first
   page** (cursor reset). Debounce the search input.

> **Server has no text-search endpoint.** Search is client-side over loaded items only. A
> future `GET /api/search?q=` (server-side, spanning the whole catalog) is **out of scope**
> here — flag it as follow-up so a later spec can add it to `02-api-contract.md`.

## Verification

- **Grid** — with a scanned server, `/` renders real posters; titles with no art show a
  placeholder; HDR badges appear on `hdr10`/`dolbyvision`/… items.
- **Infinite scroll** — scrolling loads additional pages and stops cleanly when
  `next_cursor` is `null`; no duplicate or overlapping fetches (check network panel).
- **Movie detail** — `/movie/:id` shows overview, credits, and one `FileList` row per
  `media_files` entry with correct codec/resolution/HDR.
- **Series detail** — `/series/:id` lists seasons and episodes in order.
- **Search/sort** — typing narrows the visible grid; toggling sort re-orders from page one
  (verify `added_at` puts newest first vs. `sort_title` alphabetical).

## Cross-references (edits required in lockstep)

- `80-web-ui-client.md` — this task fills the `components/` and `pages/` directories `80`
  scaffolds; the Play buttons it stubs are completed in `82`.

## Follow-ups (out of scope here)

- **Server-side text search.** The `SearchSortBar` box filters only the already-loaded
  grid items client-side. A whole-catalog `GET /api/search?q=` belongs in a later spec that
  adds it to `02-api-contract.md` (route + `search()` client method) and points the box at
  it. Flagged in `SearchSortBar.tsx` / `LibraryPage.tsx`.
- ~~**Per-episode playable file.**~~ **Done** (2026-09-02): `SeriesDetail` now hydrates each
  episode's `media_files` via the new `EpisodeWithFiles` aggregate (`medi-db` model + query,
  mirrored in `@medi/api-client`), so `EpisodeList` resolves a real `file_id`. `82` only
  needs to inject the `onPlay` handler.
