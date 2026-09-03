# 94 — fanart.tv Title Logos (Web Client & Ops)

> **Status: SPEC (not started).** Client half of the fanart.tv integration; depends on
> `93-fanart-logos.md` (the `movies.logo_path` column, the enrichment/backfill that fills it,
> and the movie-detail API surfacing the logo). Build 93 first. **Movies only** (matches 93).
>
> This is a small, contained UI change: render a movie's cached transparent-PNG logo in place
> of the text `<h1>` title on the detail hero when one exists, falling back to the text title
> when it doesn't. Plus the ops wiring (Unraid template / README) for the `FANARTTV_API_KEY`
> the user already added.

## Why

93 downloads and stores each matched movie's logo at `/config/images/movies/<id>/logo.png`,
served by the existing `GET /api/images/*`. This task shows it: the *Titanic* detail page
renders the script-logo wordmark over the backdrop instead of the words "Titanic (1997)" — the
Plex/Jellyfin/Apple-TV look. When a movie has no logo (unmatched, no fanart art, or fanart
unconfigured), the header is **visually unchanged** from today (text title).

## Requirements

- **Logo replaces the text title on the detail hero, with a text fallback.** In
  `DetailHeader`, when a `logoUrl` is provided, render `<img>` (the transparent PNG) where the
  `<h1>` text sits; otherwise render the existing `<h1>{title}{year}</h1>`. The year still
  shows (as a small caption line under/next to the logo) so the header doesn't lose it.
- **Accessible.** The logo `<img>` carries `alt={title}` (the wordmark is the title), so screen
  readers and a broken-image state still convey the title. Never ship a logo with empty `alt`.
- **No layout shift / no letterbox.** The logo is a wide, variable-aspect transparent PNG.
  Constrain it by **height** (e.g. `max-height: ~120px` on the movie hero, smaller on compact
  views) with `width: auto` and `object-fit: contain`, left-aligned like the current `<h1>`,
  so a wide or narrow logo both sit correctly over the bottom-anchored gradient. Cap
  `max-width: 100%` so a huge logo never overflows the banner.
- **Movie pages only.** `DetailHeader` is shared by movie and series pages (`91` note). Only
  the **movie** page passes `logoUrl`; the series page passes nothing, so series headers are
  unchanged (series logos are a 93/§Out-of-scope follow-up). `logoUrl` is an **optional** prop
  — omitting it preserves the existing series behavior with zero change.
- **Reuse the existing image-URL convention.** The movie-detail response carries the logo the
  same way it carries the poster/backdrop (per 93 §API). The page builds the absolute URL with
  the same helper it already uses for backdrop/poster (`client.imageUrl(...)` /
  `api-client` `imageUrl`) — **no new client method, no hotlinking fanart**.
- **Graceful in every state.** Loading (no data yet) and error states are unchanged. A movie
  with `logo_path == null` renders the text title exactly as today. A logo whose file 404s
  (deleted on disk) shows a broken `<img>` briefly — acceptable, matches how a missing poster
  behaves; optionally add an `onError` that hides the `<img>` and reveals the text `<h1>`
  (nice-to-have, not required).

## Packages / crates

**No new dependencies.** The web app reuses its existing React Router + fetch + `theme` setup;
the api-client gains only a type field (below).

## File structure (where to save)

```
client/
├── packages/api-client/src/
│   └── types.ts                      # + logo_path?: string | null on Movie / MovieDetail (mirror poster_path)
└── apps/web/src/
    ├── components/DetailHeader.tsx    # optional logoUrl prop → <img> title, text fallback
    └── pages/MovieDetailPage.tsx      # pass logoUrl={movie.logo_path ? api.imageUrl(movie.logo_path) : undefined}

docker/
├── README.md                         # document FANARTTV_API_KEY (metadata art section)
└── unraid/<template>.xml             # ensure FANARTTV_API_KEY variable present + described
docs/
└── ... (this task; update 91's "Deferred"/roadmap cross-refs if desired)
```

## api-client types (`packages/api-client/src/types.ts`)

The api-client types are the single source of truth (owned by task 40; extend, never fork).
Add the field to `Movie` (the flattened head of `MovieDetail`) next to the existing art paths,
matching whatever 93 chose:

```ts
export interface Movie {
  // ... id, title, sort_title, year, overview, added_at ...
  poster_path: string | null;
  backdrop_path: string | null;
  logo_path?: string | null;   // relative art path; build a URL with imageUrl(...)
}
```

- If 93 surfaces the logo as a **pre-built `logo` URL** instead of a raw `logo_path` (check the
  93 API decision), name the field to match the wire exactly and skip the `imageUrl(...)` call
  in the page. **The wire shape 93 ships is authoritative** — mirror it verbatim.
- Keep it **optional** (`?`) so older cached responses / series detail (no logo) type-check.

## `DetailHeader` change (`components/DetailHeader.tsx`)

Add one optional prop and branch the title node:

```tsx
export interface DetailHeaderProps {
  title: string;
  year?: number | null;
  // ... existing overview / backdropUrl / meta / minHeight / children ...
  /** Absolute logo URL (transparent PNG) from imageUrl(...). When set, replaces the text title. */
  logoUrl?: string;
}
```

Replace the current `<h1>{title}{year}</h1>` block with:

```tsx
{logoUrl ? (
  <div>
    <img
      src={logoUrl}
      alt={title}
      style={{
        maxHeight: 120,          // tune per hero; smaller on compact
        maxWidth: '100%',
        width: 'auto',
        objectFit: 'contain',
        display: 'block',
      }}
    />
    {year != null && (
      <span style={{ color: theme.colors.textMuted, fontSize: 16, marginTop: 8, display: 'inline-block' }}>
        {year}
      </span>
    )}
  </div>
) : (
  <h1 style={{ fontSize: 34, margin: 0, color: theme.colors.text }}>
    {title}
    {year != null && (
      <span style={{ color: theme.colors.textMuted, fontWeight: 400 }}> ({year})</span>
    )}
  </h1>
)}
```

- Keep the existing `meta` / `overview` / `children` blocks untouched below the title node.
- The style values are a starting point; keep them inline (the component is inline-styled
  today) and consistent with `theme`. Optional `onError` to fall back to text is a nice-to-have.

## `MovieDetailPage` change (`pages/MovieDetailPage.tsx`)

The page already fetches `MovieDetail` and renders `DetailHeader`. Pass the logo URL:

```tsx
<DetailHeader
  title={movie.title}
  year={movie.year}
  overview={movie.overview}
  backdropUrl={movie.backdrop_path ? api.imageUrl(movie.backdrop_path) : undefined}
  logoUrl={movie.logo_path ? api.imageUrl(movie.logo_path) : undefined}
  /* ...existing meta / children... */
/>
```

- Use the **same** `api.imageUrl(...)` (or whatever helper the page already uses for
  `backdrop_path`) — do not introduce a new URL builder.
- The **series** page (`SeriesDetailPage.tsx`) is **not** changed — it keeps passing no
  `logoUrl`, so its header is identical to today.

## Ops / distribution wiring

The user already added `FANARTTV_API_KEY` to the Unraid template. Finish the loop so a fresh
install documents and passes it:

- **Unraid template XML** (`docker/unraid/…`): confirm a `FANARTTV_API_KEY` `<Config>` variable
  exists (env `FANARTTV_API_KEY`, empty default, `Display=always` to match the recent template
  convention), with a short description: *"fanart.tv personal API key — enables movie title
  logos on detail pages. Optional; leave blank to disable."* Mirror the existing TMDB/OMDb key
  entries added in commits `4473677` / `5ce6f13`.
- **`docker/README.md`**: add `FANARTTV_API_KEY` to the metadata/art env-var section — what it
  does (movie title logos), that it's optional, and where to get a key (fanart.tv account →
  API keys). Note it's independent of the TMDB/OMDb keys.
- No compose change is required beyond passing the env var through as the other metadata keys
  are.

## Testing

- **typecheck**: all client workspaces green (`tsc`), `vite build` green (the app has no
  vitest/testing-library harness yet — see `91` deferred note, so a render test is out of
  scope; keep the change small and type-safe).
- **manual smoke** (documented in the task, run when a server is available): a matched movie
  with a fanart logo shows the wordmark on `/movie/:id`; a movie without a logo shows the text
  title; the series page is unchanged; a broken logo URL degrades acceptably.
- **backend contract check**: `GET /api/movies/:id` returns the logo path/URL field 94 reads
  (this is 93's test; 94 just consumes it). If 93 renamed the field, the api-client type here
  must match the wire — verify against a live/response fixture before shipping.

## Out of scope (explicitly deferred)

- **Logos on poster-grid cards / category rows** — this task renders the logo only on the
  detail hero. Overlaying logos on grid tiles is a separate visual pass.
- **Series logos** — blocked on 93's deferred TVDB resolution.
- **A per-user "prefer text titles" toggle** — no user accounts (LAN model).
- **Animated/parallax logo treatments** — plain, correctly-sized `<img>` only.
