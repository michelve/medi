# 93 — fanart.tv Title Logos (Backend & Data)

> **Status: SPEC (not started).** Backend + data half of the fanart.tv integration. The
> web-client + config/ops half is `94-fanart-logos-web.md`; build 93 first (94 depends on the
> `logo_path` column + API shape 93 lands). **Movies only** this task — series logos are an
> explicit follow-up (see §Out of scope).
>
> This task **extends the existing enrichment pipeline** (`60-metadata-and-libraries.md`,
> `91-genres-and-people-discovery.md`) — it does **not** add a second metadata provider behind
> `MetadataProvider`. fanart.tv is an *art-only* source keyed by the **TMDB id the pipeline has
> already resolved**, so it plugs in as a small extra art fetch inside `enrich_with_id`,
> exactly like the collection-poster and person-headshot downloads already there.

## Why

Today a movie's detail hero (`DetailHeader`) shows the title as plain `<h1>` text over the
backdrop. Plex/Jellyfin/Netflix show the film's **logo artwork** — the transparent-PNG
wordmark (e.g. the *Titanic* script logo at <https://fanart.tv/movie/597/titanic/>) — instead.
fanart.tv is the canonical community source for these. This task fetches, caches, and stores a
movie's logo locally so 94 can render it in place of the text title, falling back to text when
no logo exists.

The design mirror is deliberate: **logos are just one more downloaded, locally-served asset**,
stored and served the same way as posters/backdrops/headshots (atomic temp-file + `rename`
into `/config/images/...`, served by the existing `GET /api/images/*` `ServeDir`). We never
hotlink fanart.tv from the client.

## Requirements

- **Reuse the resolved TMDB id.** fanart.tv's `/movies/{id}` accepts a TMDB id directly. The
  movie already has `tmdb_id` after TMDB enrichment (stored on the row, `Details::tmdb_id`),
  so the fanart fetch needs **no new id resolution** — it runs only for movies that matched
  TMDB. A movie matched by OMDb (IMDb id, no TMDB id) is skipped for logos this task (its
  `imdb_id` *could* key fanart.tv too — deferred, see §Out of scope).
- **One extra HTTP request per matched movie, gated by a key.** With no `FANARTTV_API_KEY`
  set, the whole feature is inert: enrichment behaves exactly as today, no request, no error
  (graceful degradation, inherit `60`'s posture). This is a separate key from TMDB/OMDb.
- **Off the request path + bounded fan-out.** The fetch runs inside the enrichment worker like
  every other art download, bounded by its own `Semaphore` so a first-run scan or a backfill of
  a large library respects fanart.tv's rate limits (their personal-key limit is generous but
  finite). Never block a movie's `matched` write on the logo — a failed/absent logo logs and
  continues, same policy as a missing poster.
- **Idempotent.** A movie already carrying a `logo_path` whose file is on disk is skipped with
  no fanart request (like a present poster). A force-refresh / re-match re-fetches. The
  existing genres/people backfill is extended to fill logos for already-`matched` movies
  **without re-downloading posters/backdrops**.
- **Atomic local storage, served locally.** Download to
  `/config/images/movies/<movie_id>/logo.png` atomically (temp + `rename`), store the path
  relative to `images_dir()` in a new `movies.logo_path` column, and serve it through the
  existing images `ServeDir`. Logos are **PNG** (transparency), not JPEG — the only asset in
  the tree that isn't `.jpg`; keep the extension `.png`.
- **Pure, fixture-tested parsing.** fanart JSON parsing is a pure `parse_movie_logo(&Value)`
  function tested against a recorded/inline fixture with no live network, matching the
  `tmdb.rs` `parse_*` pattern. No new HTTP client type in the `metadata` crate beyond one
  small fanart client (it may reuse the same `reqwest::Client` construction helper).
- **No auth** on new surfaces (LAN-appliance model). No new mutation endpoints beyond reusing
  the existing backfill trigger.

## Packages / crates

**No new dependencies.** Everything is reachable with the workspace's current set (`reqwest` +
`rustls-tls`, `async-trait`, `serde`/`serde_json`, `rusqlite`, `tokio`, `tracing`, `figment`).

## File structure (where to save)

```
backend/
├── migrations/
│   └── V8__fanart_logos.sql          # NEW — movies.logo_path (next free version after V7)
└── crates/
    ├── core/src/
    │   └── config.rs                  # + fanarttv_api_key: Option<String> (FANARTTV_API_KEY)
    ├── metadata/src/
    │   ├── fanart.rs                  # NEW — FanartClient + parse_movie_logo (pure) + fixtures
    │   ├── lib.rs                     # build the fanart client alongside build_provider; re-exports
    │   ├── enrich.rs                  # fetch+download logo inside enrich_with_id (movies); backfill
    │   └── provider.rs               # (optional) LogoArt type if shared; else keep in fanart.rs
    ├── db/src/
    │   ├── writes.rs                  # set_movie_logo(conn, movie_id, logo_path)
    │   ├── queries.rs                 # SELECT logo_path in get_movie/movie detail; matched_movies_missing_logo
    │   └── models.rs                  # Movie.logo_path column (+ from_row position)
    └── api/src/
        └── dto.rs / routes.rs        # surface logo as a /api/images/... URL on the movie detail
```

## Config addition (`medi-core`)

Add one field + env var, following the exact pattern of `tmdb_api_key` in `config.rs`:

```rust
pub struct AppConfig {
    // ... existing metadata fields ...
    /// fanart.tv personal API key. Unset ⇒ title-logo fetching is disabled (graceful
    /// degradation); every other enrichment behaves exactly as today.
    pub fanarttv_api_key: Option<String>,
}
```

- Default: `None`.
- Env var: `FANARTTV_API_KEY` (and the `MEDI_`-prefixed form, via the existing `KEYS` list +
  figment merge). Add `"fanarttv_api_key"` to the `KEYS` array and a row to the doc table.
- A `fanart_enabled()` helper (or inline check) = `fanarttv_api_key.is_some() && !empty`.
- Extend the two config tests (`env_absent_yields_defaults`, `metadata_env_keys_load`) to
  assert the key is `None` by default and loads from `FANARTTV_API_KEY`.

> The Unraid template already exposes `FANARTTV_API_KEY` (the user added it). 94 documents the
> template/README wiring; this task only consumes the env var.

## DB migration

`V8__fanart_logos.sql` — the next free refinery version after `V7__collections_trailers.sql`.
Additive DDL only (no PRAGMAs — see `migrations/README.md`).

```sql
-- V8__fanart_logos.sql — fanart.tv title-logo artwork (Task 93).
--
-- A movie's transparent-PNG wordmark logo from fanart.tv, downloaded and served locally like
-- its poster/backdrop. One nullable column: the path (relative to images_dir()) of the cached
-- logo, or NULL when the movie has no logo / fanart is unconfigured / not yet backfilled.
ALTER TABLE movies ADD COLUMN logo_path TEXT;   -- relative to images_dir(): movies/<id>/logo.png
```

> **Version coordination.** Reserves migration version **V8**. refinery versions stay gapless
> and monotonic (`01-db-schema.md`); if another in-flight task also claims a new version,
> whichever ships later renumbers so the sequence stays contiguous. The ordering is
> load-bearing, not the number.

Update `models.rs`:

- Add `pub logo_path: Option<String>` to `Movie` (after `backdrop_path`).
- Extend `Movie::from_row` to read it at the next positional index, and update every
  `SELECT` in `queries.rs` that builds a `Movie` / `MovieDetail` to include `logo_path` in the
  column list (keep the positional `from_row` and the `SELECT` in lockstep — the module docs
  call this out explicitly).

## fanart.tv client (`metadata/src/fanart.rs`)

A small, self-contained art client — **not** a `MetadataProvider` (it has no search/details
semantics; it's an art lookup keyed by TMDB id).

**Endpoint (v3, stable):**

```
GET https://webservice.fanart.tv/v3/movies/{tmdb_id}?api_key={FANARTTV_API_KEY}
```

- `{tmdb_id}` accepts a TMDB **or** IMDb id; we pass the TMDB id.
- `api_key` is the personal key (query param). A `client_key` is optional and not used here.
- `404` means "fanart has no art for this id" — treat as "no logo", not an error.
- Response is a JSON object: top-level `name` / `tmdb_id` / `imdb_id` (all strings) plus one
  array per art type. **For logos we read, in priority order:**
  1. `hdmovielogo` — HD (higher-res) transparent PNG wordmark. **Preferred.**
  2. `movielogo` — standard-res fallback.

  Each entry is `{ "id": "3321", "url": "https://assets.fanart.tv/fanart/titanic-4fa587….png", "lang": "en", "likes": "11" }`
  — **`id`/`likes`/`lang` are strings** (parse `likes` to i64 for the tie-break, default 0
  on empty/missing), and the **`url` is already an absolute `https://assets.fanart.tv/…`
  URL — no base-URL join needed** (unlike TMDB's relative `poster_path`). Feed it straight to
  `maybe_download`.
  (Other arrays exist and are **ignored this task** — verified live against TMDB 597 "Titanic":
  `hdmovieclearart`, `moviebackground`, `movie4kbackground`, `movieposter`, `moviebanner`,
  `moviethumb`, `moviesquare`, `moviedisc` (the last carries extra `disc`/`disc_type` fields).
  See §Out of scope.)

> **Verified live (session 2026-09-02)** with the dev `FANARTTV_API_KEY` (in the git-ignored
> `docker/compose.dev.override.yml`) against `GET /v3/movies/597`: `hdmovielogo` (11 entries)
> and `movielogo` (3) both present, shape exactly as above. Record a trimmed copy of this real
> response as the `parse_movie_logo` fixture (a couple of entries per array is enough).

**Selection rule (in `parse_movie_logo`, pure):**

1. Prefer `hdmovielogo`, else `movielogo`.
2. Within the chosen array, prefer an entry whose `lang` matches the configured
   `metadata_language`'s language subtag (e.g. `en-US` → `en`); else prefer `lang == "en"`;
   else the first entry. Tie-break by highest `likes` (parse the string to i64; default 0).
3. Return `Option<String>` — the chosen absolute `url`, or `None` when neither array has a
   usable entry. Keep it a pure function of `(&Value, preferred_lang)` so tests need no network.

```rust
// fanart.rs — sketch
pub struct FanartClient {
    api_key: String,
    preferred_lang: String,        // e.g. "en" from metadata_language
    http: reqwest::Client,
    sem: Arc<Semaphore>,           // MAX_CONCURRENCY, mirrors TmdbProvider
}

impl FanartClient {
    pub fn new(api_key: impl Into<String>, metadata_language: &str) -> Result<Self> { /* … */ }

    /// Absolute URL of the best logo for a TMDB movie id, or None (incl. on 404).
    pub async fn movie_logo_url(&self, tmdb_id: i64) -> Result<Option<String>> {
        // GET /v3/movies/{tmdb_id}, 404 => Ok(None), then parse_movie_logo(&json, &lang)
    }
}

/// Pure: pick the best logo URL from a fanart /movies/{id} response.
pub fn parse_movie_logo(v: &Value, preferred_lang: &str) -> Option<String> { /* per rule above */ }
```

- Reuse the same `reqwest::Client` builder + `user_agent` as `TmdbProvider::new` /
  `HttpFetcher` (a small shared helper is fine, or duplicate the 3 lines — match whatever the
  crate already does).
- Bound concurrency with a `Semaphore(MAX_CONCURRENCY)` like `TmdbProvider`, so a backfill of a
  large library never bursts fanart.
- Errors map to the crate's existing `Error` (`Http` / `Provider` / `Parse`).

**`lib.rs`:** add a `build_fanart(cfg) -> Option<FanartClient>` mirroring `build_provider`
(returns `None` when the key is unset/empty), and re-export `FanartClient` + `parse_movie_logo`.

## Enrichment changes (`enrich.rs`)

The fanart client is optional context on `EnrichContext`:

```rust
pub struct EnrichContext {
    // ... existing db / provider / fetcher / images_dir ...
    /// fanart.tv art client, or None when FANARTTV_API_KEY is unset (feature inert).
    pub fanart: Option<Arc<FanartClient>>,
}
```

Inside `enrich_with_id`, **for movies only**, after `details()` resolves the TMDB id and the
poster/backdrop download block, add a logo step that mirrors `build_collection` /
`enrich_people` (best-effort, non-fatal, idempotent):

1. If `ctx.fanart` is `None` → skip (feature off). If `title_kind != Movie` → skip.
2. Resolve the movie's TMDB id (`details.tmdb_id`); if `None` (OMDb match) → skip.
3. Idempotency: if `movies/<id>/logo.png` already exists on disk (and not `force`) → keep it,
   record the path, no fanart request. (The `maybe_download` "already on disk" branch already
   does this if we route the logo through it — see below.)
4. Else call `ctx.fanart.movie_logo_url(tmdb_id)`; on `Some(url)`, download atomically to
   `movies/<id>/logo.png` via the existing `maybe_download` (which handles skip-if-present +
   atomic `write_atomic` + returns the relative path) — **note `.png`, not `.jpg`**.
5. Persist the returned relative path with a new `writes::set_movie_logo(&tx, id, logo_path)`
   inside the **same transaction** as `set_title_metadata` / `replace_credits` / genres, so a
   match commits atomically and invalidates the cache once. A movie with no logo writes
   `NULL` (a re-match with no logo clears a stale link, like the collection FK).
6. A fanart HTTP error logs `warn` and continues with `logo_path = None` — never fails the
   movie's enrichment.

**Backfill:** extend `backfill_genres_people` (or add the logo fetch to the movie branch of the
per-title work it already does) so re-processing an already-`matched` movie fills its
`logo_path` **without re-downloading its poster/backdrop** (the per-asset `maybe_download`
skips existing files; the logo is a new asset so it downloads once). Add a DB helper
`matched_movies_missing_logo(conn, force, limit)` mirroring `matched_titles_missing_genres` so
the backfill can target only movies still lacking a logo (resumable; a crash mid-backfill
re-runs cleanly). The existing `POST /api/metadata/backfill` trigger covers this — no new
endpoint.

> **`maybe_download` reuse.** It currently hard-codes nothing about the extension — the caller
> passes the file name (`"poster.jpg"`, `"photo.jpg"`). Pass `"logo.png"` and it works
> unchanged (atomic temp `.png.tmp` → `rename`). The `artwork_complete` idempotency gate keys
> on `poster.jpg` only and needn't change; the logo has its own on-disk skip via
> `maybe_download`.

## API changes (`crates/api`)

The movie detail already returns `MovieDetail { movie: Movie, … }` and `Movie` now carries
`logo_path`. Surface it as a **client-facing `/api/images/...` URL**, the same way the library
tile maps `poster_path` → `poster` via `dto::image_url`:

- In the movie-detail handler (or a thin DTO wrapper), expose `logo: Option<String>` =
  `movie.logo_path.map(image_url)`. Prefer adding a `logo` field to the movie-detail response
  shape rather than leaking the raw stored `logo_path` — mirror how `LibraryItem::from_card`
  turns `poster_path` into a `poster` URL. If the detail response currently serializes `Movie`
  via `#[serde(flatten)]` with raw `poster_path`/`backdrop_path` (check `MovieDetailPage.tsx` /
  `api-client` — it reads `poster_path` directly and calls `client.imageUrl(...)`), then follow
  that same convention: serialize `logo_path` (raw relative path) on the flattened `Movie` and
  let the client call `imageUrl(...)`. **Match the existing poster/backdrop convention exactly**
  so 94 has nothing new to learn.
- Whatever shape is chosen, ensure the movie-detail cache entry is invalidated by the same
  `invalidate_all` the refresh/match/backfill path already calls (a re-match can change the
  logo). No new cache key needed — it rides on `movie/{id}`.

## Testing

- **config**: `fanarttv_api_key` is `None` by default and loads from `FANARTTV_API_KEY`
  (extend the two existing config tests).
- **fanart parsing** (`fanart.rs`, pure, inline `serde_json::json!` fixtures like `tmdb.rs`):
  - `parse_movie_logo` prefers `hdmovielogo` over `movielogo`.
  - prefers the configured language, then `en`, then first; tie-breaks on `likes`.
  - returns `None` when both arrays are absent/empty.
- **enrichment** (extend the stub-provider tests in `enrich.rs`): give `EnrichContext` a stub
  fanart client (behind a small trait or an injected closure — mirror `ImageFetcher`) returning
  a logo URL; assert a matched movie writes `movies.logo_path = movies/<id>/logo.png`, the file
  exists on disk with no stray `.png.tmp`, and a forced re-enrich does **not** re-download the
  logo already present. Assert a movie whose fanart lookup yields `None` writes `logo_path =
  NULL` and still matches. Assert `ctx.fanart = None` ⇒ no logo write, movie still matches
  (feature inert).
  > To keep the fetch testable without a live network, introduce a tiny trait (e.g.
  > `LogoSource { async fn movie_logo_url(&self, tmdb_id) -> Result<Option<String>> }`)
  > implemented by `FanartClient`, and store `Option<Arc<dyn LogoSource>>` on `EnrichContext`.
  > The stub in tests then returns canned URLs, exactly like `StubProvider`/`StubFetcher`.
- **db**: `matched_movies_missing_logo` returns only matched movies with `logo_path IS NULL`;
  `get_movie` / movie-detail query round-trips `logo_path`.
- **api**: `GET /api/movies/:id` includes the logo path/URL in the documented shape after a
  match; absent for an unmatched or logo-less movie.
- **backfill**: a pre-seeded matched movie with a poster but no logo gets `logo_path` filled by
  a backfill run **without** re-downloading its poster (assert the poster fetch count is
  unchanged; assert the logo file now exists).

## Out of scope (explicitly deferred)

- **Series/TV logos** — fanart's `/v3/tv/{tvdb_id}` (`hdtvlogo`/`clearlogo`) needs a TVDB id
  medi doesn't resolve or store today. A follow-up task adds TVDB resolution + series logos.
- **Other fanart art types** — `hdmovieclearart`, `movieposter`, `moviethumb`, `moviebanner`,
  `moviedisc`. Logos only this task. (`moviebackground` wallpapers shipped as a follow-up —
  see `95-fanart-wallpapers.md`, which reuses this task's single fanart request.)
- **Keying fanart by IMDb id** for OMDb-matched movies (fanart accepts `tt…` too) — deferred;
  this task requires a resolved TMDB id.
- **A user setting to prefer text over logos** — 94 renders the logo when present with a text
  fallback; a per-user toggle is out of scope (no user accounts, LAN model).
- **Refreshing logos on a schedule** as fanart adds art — covered by the existing force-refresh
  / backfill, not a new cron.
