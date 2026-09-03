# 95 — fanart.tv Background Wallpapers (Backend, Data & Web)

> **Status: DONE (session 2026-09-03).** Extends the fanart.tv integration from `93`/`94` with
> the movie **background wallpaper** art type (fanart's `moviebackground`, the site's
> "wallpaper" section — e.g. <https://fanart.tv/movie/299534/avengers-endgame/?section=wallpaper>).
> **Movies only** (matches 93/94). Built directly on 93's plumbing — same fanart client, same
> `enrich_with_id`/backfill path, same local-download-and-serve model.

## Why

93 fetches a movie's **title logo** from fanart.tv. The same `/v3/movies/{id}` response also
carries **background wallpapers** (`moviebackground`, 1920×1080 key art). This task reads that
array too and shows it on the movie detail hero **in place of the TMDB backdrop** when present —
the higher-quality, community-curated background — falling back to the TMDB backdrop (then to a
flat surface) when there's no wallpaper.

The design mirror is deliberate and the marginal cost is tiny: the wallpaper is fetched from the
**same single fanart request** the logo already makes (no extra HTTP), stored and served the
same way as posters/backdrops/logos (atomic temp-file + `rename` into `/config/images/...`,
served by the existing `GET /api/images/*`). We never hotlink fanart.tv from the client.

## Render policy (decided)

**fanart wallpaper wins; TMDB backdrop is the fallback.** The movie hero's background is:

```
hero backdrop = fanart wallpaper (movie.wallpaper_path)   if present
              else TMDB backdrop  (movie.backdrop_path)    if present
              else flat surface
```

## What shipped

### One request, two art types (`metadata/src/fanart.rs`)

- The `LogoSource` trait was generalized to **`FanartArt`** with a single method
  `movie_art(tmdb_id) -> Result<Option<MovieArt>>`, returning `MovieArt { logo_url,
  wallpaper_url }` parsed from **one** `/v3/movies/{id}` response. A 404 (fanart has no art for
  the id) → `Ok(None)`; any real HTTP/parse failure errors so the caller logs-and-continues.
- New pure `parse_movie_wallpaper(&Value, preferred_lang)` reads the `moviebackground` array,
  reusing the shared `best_by_lang_and_likes` selection helper (extracted from
  `parse_movie_logo`). Wallpapers usually carry no `lang`, so selection collapses to
  highest-`likes` (fanart stores `likes` as a *string*; parsed to i64, default 0), array order
  as the final tie-break. Fixture-tested inline like `parse_movie_logo`.
- `build_fanart` now returns `Arc<dyn FanartArt>`.

### Storage & data

- **`V9__fanart_wallpapers.sql`** — `ALTER TABLE movies ADD COLUMN wallpaper_path TEXT` (next
  free refinery version after V8).
- `models.rs` — `Movie.wallpaper_path: Option<String>` (after `logo_path`), `from_row` index 9.
- `queries.rs` — `MOVIE_COLUMNS` gains `wallpaper_path`; `matched_movies_missing_logo` became
  **`matched_movies_missing_fanart`** (filter: `logo_path IS NULL OR wallpaper_path IS NULL`),
  so one worklist drives both art types in the single-request backfill pass.
- `writes.rs` — `set_movie_wallpaper(conn, movie_id, wallpaper_path)`, written unconditionally
  in the enrichment transaction like `set_movie_logo` (a re-match with no wallpaper clears it).

### Enrichment (`enrich.rs`)

- `EnrichContext.fanart` is now `Option<Arc<dyn FanartArt>>`.
- `download_logo` became **`download_fanart_art`**, returning `(logo_rel, wallpaper_rel)` from
  one fanart request: skip entirely when fanart is off / no TMDB id / **both** files already on
  disk; otherwise fetch once and download whichever asset is missing (logo `.png`, wallpaper
  `.jpg`) via `maybe_download` (atomic, skip-if-present). Both paths persist in the same match
  transaction. Any failure is non-fatal (writes the corresponding `NULL`).
- The backfill's fanart pass targets `matched_movies_missing_fanart` and fills both art types
  without re-downloading posters/backdrops already on disk.

### API & web

- `Movie.wallpaper_path` flattens into `GET /api/movies/:id` automatically (like `logo_path`);
  no route change. Cache invalidation rides the existing `movie/{id}` key.
- api-client `types.ts` — `wallpaper_path?: string | null` on `Movie`.
- `MovieDetailPage.tsx` — `backdropUrl={api.imageUrl(movie.wallpaper_path) ?? api.imageUrl(movie.backdrop_path)}`
  (fanart wins, TMDB fallback). `DetailHeader` is unchanged — it already takes a single
  `backdropUrl`; the wallpaper is just a better source for it.

### Ops

- `docker/README.md` + `unraid-templates/medi.xml` — the `FANARTTV_API_KEY` description now
  says "title logos **and background wallpapers**"; the same key enables both.

## Testing

- **fanart parsing**: `parse_movie_wallpaper` picks highest-likes, prefers a tagged language
  when present, returns `None` with no `moviebackground`. (`metadata/src/fanart.rs`)
- **enrichment**: a matched movie writes both `logo_path` + `wallpaper_path`, both files exist,
  one fanart lookup; both-present forced re-enrich re-queries neither; wallpaper-only and
  logo-only fanart responses write only the present art; `fanart = None` writes neither; a
  backfill fills both without re-downloading poster/backdrop. (`enrich.rs`)
- **db**: `matched_movies_missing_fanart` returns matched movies missing *either* art type;
  `get_movie` round-trips both columns. (`db/tests/schema.rs`)
- **client**: all workspaces `tsc` green; `vite build` green.

## Out of scope (explicitly deferred)

- **Wallpapers on grid cards / rows** — hero only, like logos (`94`).
- **`movie4kbackground`** (4K wallpapers) — only the standard `moviebackground` is read; a 4K
  preference is a small follow-up.
- **Series wallpapers** — blocked on 93's deferred TVDB resolution.
- **Other fanart art types** (`hdmovieclearart`, `movieposter`, `moviethumb`, `moviebanner`,
  `moviedisc`) — still deferred (`93` §Out of scope).
