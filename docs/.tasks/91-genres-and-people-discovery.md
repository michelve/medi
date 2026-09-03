# 91 — Genres & People Discovery (Netflix-style browsing)

> **Status (2026-09-02): Phase A (Genres) + Phase B (Person pages) SHIPPED.** Depends on the
> shipped metadata enrichment (`60-metadata-and-libraries.md` Phase A: `medi-metadata`
> provider trait, TMDB provider, `enrich.rs` pipeline, `people`/`credits` tables) and the web
> SPA (`80/81/82`). This task **extends the existing hand-written `TmdbProvider`** rather than
> adopting an external TMDB client crate — the provider stays behind the `MetadataProvider`
> abstraction and keeps its pure-function/recorded-fixture test pattern.
>
> **Phase A delivered:** `V6__genres_people.sql` (genre tables + person columns);
> `TmdbProvider` genre parsing (`parse_genres`) + `CreditIn::person_tmdb_id` capture;
> genre writes folded into the enrichment transaction (`replace_title_genres`) +
> `backfill_genres_people`; DB `list_genres` / `list_by_genre` (library page shape) /
> `matched_titles_missing_genres`; API `GET /api/genres`, `GET /api/genres/:id`,
> `GET /api/library/rows`, `POST /api/metadata/backfill`; web `CategoryRow`, `GenrePage`
> (`/genre/:id`), and `LibraryPage` landing rows.
>
> **Phase B delivered:** `PersonDetails` provider type + `person_details()` trait method
> (default `Ok(None)`; TMDB `parse_person` + `/person/{id}` + cached `profile_sizes` base);
> person enrichment folded into `enrich_with_id` (`enrich_people`: headshot download to
> `people/<id>/photo.jpg` + `upsert_person_meta`, idempotent skip when already enriched,
> best-effort/non-fatal), so `backfill_genres_people` backfills people too; DB `get_person`
> / `person_filmography` / `get_person_enrichment_state`; API `GET /api/people/:id`
> (`PersonPage` DTO); web `CreditsList` links → `/person/:id`, `PersonPage` (headshot + bio
> "show more" + filmography grid). All new endpoints cached + ETag'd and dropped by the
> existing `invalidate_all` on refresh/match/backfill.
>
> **Deferred:** web render tests for `CategoryRow` / credit links (the web app has no test
> harness — no vitest/testing-library yet); recorded-JSON fixtures under
> `crates/metadata/src/fixtures/` (the provider tests use inline `serde_json::json!`
> fixtures, matching the existing `tmdb.rs` test style).

> New cross-cutting phase. Today the catalog is one flat, alphabetical/recency grid
> (`GET /api/library`), cast is a plain name list on the detail page, and **genres do not
> exist anywhere** — no table, no provider parsing, no API, no UI. This task adds the two
> capabilities every Plex/Jellyfin/Netflix-style server has that `medi` lacks:
>
> 1. **Genre categorization + category rows** — titles carry TMDB genres; the browse page
>    shows horizontally-scrolling rows ("Action", "Drama", "Recently Added", …) instead of
>    only one flat grid, and a genre is its own filtered view.
> 2. **Person pages** — cast/crew names become clickable and open a person page with a
>    photo, bio, and their filmography *within this library* (plus their full TMDB
>    filmography as context).

## Purpose

Turn the catalog from a filing cabinet into something you browse. Concretely:

- A viewer opening `medi` lands on **rows of categories**, not a wall of posters.
- Clicking a genre chip opens **that genre's grid**, keyset-paginated like the main library.
- Clicking an actor or director opens **their page** — headshot, short bio, and every title
  of theirs in this library, newest first.

The work splits so the visible win ships first and each phase is independently shippable:

- **Phase A — Genres**: schema + provider parsing + backfill + browse rows + genre view.
  This is the headline "Netflix rows" feature and reuses the live enrichment pipeline.
- **Phase B — Person pages**: enrich people with TMDB ids/photos/bios, add the person
  endpoints, make cast clickable, build the person page.

Phase B depends on Phase A only for the enrichment plumbing it extends, not for its data —
they can be built in either order, but A is recommended first for user-visible payoff.

## Requirements

- **Reuse the existing provider abstraction.** All new TMDB access goes through new methods
  on `MetadataProvider` / `TmdbProvider`; the crate never gains a second HTTP client and the
  rest of the code never learns which provider answered. Keep JSON parsing as pure functions
  (`parse_*(&Value) -> …`) tested against recorded fixtures with no live network, matching
  `tmdb.rs`'s existing `parse_search`/`parse_details`.
- **Genres come essentially free from TMDB details.** `/movie/{id}` and `/tv/{id}` responses
  already include a `genres: [{id, name}]` array — parse it in the *same* `details()` call
  the pipeline already makes. **No extra TMDB request per title for genres.**
- **Idempotent + graceful degradation** (inherit `60`'s rules): with no `TMDB_API_KEY`,
  ingestion behaves exactly as today — no genres, no person data, never an error. A
  re-enrichment/force-refresh replaces a title's genres and re-fetches person data.
- **Enrichment stays off the request path**; genre and person writes reuse the same
  transaction + moka-cache invalidation as the existing metadata write.
- **Backfill without a rescan.** Libraries enriched before this task have `matched` rows
  with no genres/person ids. Provide a one-shot backfill (see §Backfill) that re-fetches
  `details()` for already-`matched` titles and fills the new tables, so existing users get
  rows without deleting and re-adding their library.
- **Person photos are downloaded and served locally**, exactly like posters/backdrops:
  atomic temp-file + `rename` into `/config/images/people/<person_id>/photo.jpg`, served by
  the existing `GET /api/images/*` `ServeDir`. Never hotlink TMDB in the client.
- **No auth** (LAN-appliance model, `00-architecture.md`); new read endpoints are plain GETs
  and inherit the same posture. No new mutation endpoints are required beyond the backfill
  trigger (which reuses the existing refresh authorization posture).
- **Bounded TMDB fan-out**: person enrichment reuses the provider's existing
  `Semaphore(MAX_CONCURRENCY)`; the backfill respects it so re-processing a 10k library
  does not exceed TMDB rate limits.

## Packages / crates

No new dependencies. Everything is reachable with the workspace's current set
(`reqwest` + `rustls-tls`, `async-trait`, `serde`/`serde_json`, `rusqlite`, `tokio`,
`tracing`). The web client adds no dependency — new pages reuse the existing React Router
+ fetch setup.

## File structure (where to save)

```
backend/
├── migrations/
│   └── V6__genres_people.sql        # NEW — see §DB migrations (next free version after V5)
└── crates/
    ├── metadata/  (medi-metadata)
    │   └── src/
    │       ├── provider.rs           # + Genre, PersonRef on Details; PersonDetails; new trait methods
    │       ├── tmdb.rs               # + parse genres in parse_details; person_details(); person photo base
    │       ├── enrich.rs             # write genres + person ids/photos; backfill_genres_people(...)
    │       └── fixtures/             # NEW — recorded TMDB JSON for /movie, /tv, /person tests
    ├── db/src/
    │   ├── writes.rs                 # replace_title_genres(...), upsert_person_meta(...)
    │   ├── queries.rs                # list_genres, list_by_genre (keyset), person + person_filmography
    │   └── models.rs                 # Genre, PersonMeta, PersonFilmographyEntry
    └── api/src/
        ├── routes.rs                 # /api/genres, /api/genres/:id, /api/people/:id (+ router wiring)
        └── dto.rs                    # GenreList, GenreRow, PersonPage DTOs

client/
├── packages/api-client/src/
│   ├── types.ts                      # Genre, PersonPage, filmography types
│   └── (client methods for the 3 new endpoints)
└── apps/web/src/
    ├── router.tsx                    # + /genre/:id and /person/:id routes
    ├── pages/GenrePage.tsx           # NEW — one genre's keyset grid (reuses PosterGrid)
    ├── pages/PersonPage.tsx          # NEW — headshot + bio + filmography grid
    ├── components/CategoryRow.tsx     # NEW — one horizontal poster row
    ├── components/CreditsList.tsx     # make each credit a <Link to=/person/:id>
    └── pages/LibraryPage.tsx          # render category rows above/instead-of the flat grid
```

## DB migrations

`V6__genres_people.sql` — the next free refinery version after `V5__subtitle_streams.sql`.
Adds a canonical genre table keyed by TMDB's own genre ids (stable across the API), M:N
join tables for movies and series, and person-enrichment columns on the existing `people`
table (whose only column today is `name`).

```sql
-- Canonical genres, keyed by TMDB genre id so re-enrichment is a stable upsert and two
-- providers could map onto the same rows. `name` is display text ("Science Fiction").
CREATE TABLE genres (
    id   INTEGER PRIMARY KEY,          -- TMDB genre id (NOT autoincrement)
    name TEXT NOT NULL UNIQUE
);

-- M:N: a title has many genres, a genre has many titles. Separate movie/series joins keep
-- the FKs simple and cascade cleanly on title delete (reap / library-delete).
CREATE TABLE movie_genres (
    movie_id INTEGER NOT NULL REFERENCES movies(id) ON DELETE CASCADE,
    genre_id INTEGER NOT NULL REFERENCES genres(id) ON DELETE CASCADE,
    PRIMARY KEY (movie_id, genre_id)
);
CREATE TABLE series_genres (
    series_id INTEGER NOT NULL REFERENCES series(id) ON DELETE CASCADE,
    genre_id  INTEGER NOT NULL REFERENCES genres(id) ON DELETE CASCADE,
    PRIMARY KEY (series_id, genre_id)
);

-- Person enrichment. `people` already exists (id, name UNIQUE); add TMDB linkage + art/bio.
ALTER TABLE people ADD COLUMN tmdb_id     INTEGER;   -- TMDB person id (nullable: pre-backfill)
ALTER TABLE people ADD COLUMN photo_path  TEXT;      -- relative to images_dir(): people/<id>/photo.jpg
ALTER TABLE people ADD COLUMN biography   TEXT;

-- Fast "titles in this genre, newest first" and "genres with a nonzero count".
CREATE INDEX idx_movie_genres_genre  ON movie_genres(genre_id);
CREATE INDEX idx_series_genres_genre ON series_genres(genre_id);
CREATE UNIQUE INDEX idx_people_tmdb  ON people(tmdb_id) WHERE tmdb_id IS NOT NULL;
```

> **Version coordination.** This task reserves migration version **V6**. refinery versions
> must stay gapless and monotonic (`01-db-schema.md`); if `90-format-coverage-and-subtitles.md`
> or any other in-flight task also claims a new version, whichever ships later renumbers so
> the sequence stays contiguous. The number is not load-bearing; the ordering is.

> **Note on person id vs `people.id`.** `people` rows de-dupe on `UNIQUE(name)` today and
> the `photo_path` in this migration is `people/<people.id>/photo.jpg` — i.e. keyed by our
> internal row id, not TMDB's — so a person with no TMDB match still has a stable art path.
> `tmdb_id` is the *link out*, unique when present. Backfill sets it; new enrichment sets it
> inline.

## Provider changes (`medi-metadata`)

Extend the shared types and the trait in `provider.rs`, then implement in `tmdb.rs`. The
`omdb.rs` provider returns empty genres and `None` person data (OMDb has neither in the
shape we use) — the trait methods have default-ish impls so only TMDB does real work.

```rust
// provider.rs — additive.
pub struct Genre { pub tmdb_id: i64, pub name: String }

// A cast/crew ref now carries the provider person id so enrichment can fetch the person.
pub struct CreditIn { /* existing */ pub person_tmdb_id: Option<i64> }

// Details gains genres (parsed from the SAME details() response — no extra request).
pub struct Details { /* existing */ pub genres: Vec<Genre> }

pub struct PersonDetails {
    pub tmdb_id: i64,
    pub name: String,
    pub biography: Option<String>,
    pub photo_url: Option<String>,      // absolute, provider-resolved (profile image base)
}

#[async_trait]
pub trait MetadataProvider: Send + Sync {
    // ... existing search() / details() ...
    /// Fetch a person's bio + headshot. Default: `Ok(None)` for providers without people.
    async fn person_details(&self, person_tmdb_id: i64) -> Result<Option<PersonDetails>> {
        let _ = person_tmdb_id; Ok(None)
    }
}
```

TMDB specifics (`tmdb.rs`, all as pure `parse_*` functions with fixtures):

1. **Genres** — in `parse_details`, read the top-level `genres: [{id, name}]` array. Zero
   extra HTTP. Add `genres` to the returned `Details`.
2. **Cast person ids** — in `parse_cast`, capture each member's `id` into
   `CreditIn::person_tmdb_id` (already present in the `credits` block the pipeline appends).
3. **Person details** — new `GET /person/{id}` (bounded by the existing semaphore), parsed by
   a new `parse_person(&Value, profile_base) -> PersonDetails`. `profile_url` uses the
   `/configuration` `profile_sizes` base (a second cached width alongside the poster base).

Record real (trimmed) TMDB JSON under `crates/metadata/src/fixtures/` for `/movie`, `/tv`,
and `/person`, and assert genre/person parsing against them, mirroring the existing
`details_extracts_overview_art_cast_and_ids` test.

## Enrichment changes (`enrich.rs`)

Fold the new writes into the existing `enrich_with_id` transaction so a match still writes
atomically and invalidates the cache once:

- After `details()`, upsert each `Details::genres` into `genres` and replace the title's
  rows in `movie_genres`/`series_genres` (`replace_title_genres` — delete-then-insert like
  `replace_credits`, so a re-match never leaves a stale genre).
- When persisting credits, for each `CreditIn` with a `person_tmdb_id`, call
  `provider.person_details(id)`; on `Some`, download the headshot atomically to
  `people/<people.id>/photo.jpg` (reuse `write_atomic` / `maybe_download`) and
  `upsert_person_meta(person_id, tmdb_id, photo_path, biography)`. A person already carrying
  a photo + tmdb_id is skipped (idempotent, no re-fetch) unless `force`. A failed person
  fetch logs and continues — a missing headshot must never fail the whole enrichment, same
  policy as a missing poster.
- **Backfill** — new `backfill_genres_people(ctx, force) -> BackfillReport`: iterate all
  `matched` movies/series lacking genres (or, with `force`, all matched titles), and for each
  run the *details + genre/person write* half of the pipeline **without** re-downloading
  posters/backdrops that already exist. Bounded by the provider semaphore; resumable (it
  only touches titles still missing data), so a crash mid-backfill re-runs cleanly.

## API additions (`crates/api`)

Three read endpoints, all cached + ETag'd via the existing `get_or_render`, plus a backfill
trigger that reuses the `require_enrich` 501-when-unconfigured guard.

| Method | Path | Returns |
|--------|------|---------|
| `GET`  | `/api/genres` | `[{ id, name, count }]` — genres with ≥1 title, count = movies+series, ordered by count desc then name. Backs the browse rows and genre chips. |
| `GET`  | `/api/genres/:id?cursor=&limit=&sort=` | Keyset-paginated grid of titles in one genre — **identical page shape to `/api/library`** (reuse `LibraryPage`/`LibraryItem` + the same cursor codec) so the client's paging hook is reused verbatim. |
| `GET`  | `/api/people/:id` | `PersonPage { id, name, photo_path, biography, tmdb_id, filmography: [LibraryItem] }` — the person's titles present in this library, newest first. |
| `POST` | `/api/metadata/backfill` | Kicks `backfill_genres_people` on the enrichment worker; `202` + a small status, or `501` if no provider configured. Idempotent to re-hit. |

Wire the routes in `router()` next to the existing catalog routes; genre/person detail
handlers mirror `movie_detail` (blocking DB read inside `get_or_render`). Cache keys:
`genres`, `genre/{id}?…`, `person/{id}`. **Invalidate these** in the same place the
enrichment/refresh path already invalidates `library` + `movie/{id}` (a re-match can change a
title's genres and a person's filmography), and after a backfill run completes.

`GET /api/library` gains an optional **`rows=true`** mode (or a sibling `/api/library/rows`)
returning a small set of curated category rows for the landing page:
`{ rows: [{ key, title, items: [LibraryItem] }] }` — e.g. "Recently Added" (existing
`added_at` sort, capped) plus the top-N genres by count, each capped to ~20 items. This is a
convenience aggregation so the landing page is one request instead of N.

## Web client (`client/apps/web`)

- **`LibraryPage`**: when no search/filter is active, render `CategoryRow`s from the rows
  endpoint (horizontal scroll, poster cards reused from `PosterGrid`/`PosterCard`); fall back
  to the existing flat grid for search/sort. Each row has a "See all →" link to
  `/genre/:id`.
- **`GenrePage`** (`/genre/:id`): reuse `useLibraryPaging` pointed at `/api/genres/:id` (same
  page shape) → the existing `PosterGrid` with infinite scroll. Header shows the genre name.
- **`CreditsList`**: wrap each cast/crew entry in `<Link to={/person/${id}}>` when the credit
  has a person id; non-linked (pre-backfill) names stay plain text.
- **`PersonPage`** (`/person/:id`): headshot (`/api/images/people/<id>/photo.jpg`), name, bio
  (clamped with a "more" toggle), then their filmography as a `PosterGrid`.
- Add both routes to `router.tsx`; add `Genre`, `PersonPage`, and the filmography types to
  `packages/api-client/src/types.ts` with a client method per endpoint (mirror the existing
  `getMovie`/`getLibrary` methods).

## Testing

- **metadata**: `parse_details` extracts genres from a recorded `/movie` + `/tv` fixture;
  `parse_cast` captures `person_tmdb_id`; `parse_person` extracts bio + photo url from a
  `/person` fixture. Enrichment test (extend the existing stub-provider tests): a match writes
  `genres` + join rows and, given a person id, upserts `people.tmdb_id`/`photo_path` and
  downloads the headshot atomically (assert file exists, no `.tmp`). Backfill test: a
  pre-seeded `matched` movie with no genres gets them without re-downloading its poster
  (assert download count).
- **db**: `list_genres` returns only nonzero-count genres ordered correctly; `list_by_genre`
  keyset-paginates identically to `list_library`; `person_filmography` returns the person's
  in-library titles newest-first and excludes titles they are not credited on.
- **api**: `GET /api/genres`, `/api/genres/:id` (first page + cursor), `/api/people/:id`
  return the documented shapes and ETag; `/api/metadata/backfill` returns `501` with no
  provider configured.
- **web**: minimal render tests for `CategoryRow` and that a `CreditsList` entry with an id
  links to `/person/:id`.

## Out of scope (explicitly deferred)

- Personalized recommendations / "Because you watched" (no user accounts — LAN model).
- Watch history / continue-watching rows (no playback-progress store yet).
- Full TMDB person filmography *outside* this library beyond what a single `/person` call
  returns; the person page lists in-library titles as the primary content.
- Collections/franchises, keywords, and studio/network browsing (a later discovery task).
- Multi-provider genre mapping — genres are TMDB-keyed; OMDb contributes none.
