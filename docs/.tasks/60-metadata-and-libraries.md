# 60 — Metadata Enrichment & Plex-style Libraries

> **Status (2026-09-01): backend Phases A + B IMPLEMENTED and cargo-tested.** New crate
> `medi-metadata` (provider trait + TMDB/OMDb + matcher + enrichment + artwork reaping),
> migrations `V2__metadata.sql` / `V3__libraries.sql`, config keys, DB write/read helpers,
> ingest auto-enrichment + multi-library scan with kind-override, and the 8 API endpoints
> are all live. `medi-metadata` 22/22 tests pass; new db/ingest/api/core tests pass. The
> RN Settings→Libraries + richer Detail **client screens (sub-task 12)** are the deferred
> follow-up; `client/packages/api-client` types + methods for every new endpoint are done.

> New cross-cutting phase. Closes the gap between what the narrative docs assume and
> what any task actually specs. `README.md` promises a "background metadata scrape" and
> `AppConfig::images_dir()` documents an artwork "metadata pipeline" — but no `NN-*.md`
> task, schema table, or code delivers descriptive metadata or user-managed libraries.
> Depends on `00-architecture.md`, `01-db-schema.md`, `02-api-contract.md`, and the
> Phase 1 ingest pipeline (`10-phase1-foundation-data.md`). **Gap this closes:** the
> catalog's descriptive columns (`overview`, `poster_path`, `backdrop_path`, `people`,
> `credits`) exist but nothing fills them, and there is exactly one implicit library
> (the `/media` mount) with no way to define or manage more.

## Purpose

Give `medi` the two capabilities every Plex/Jellyfin-style server has and this one does
not yet:

1. **Metadata enrichment** — when a title enters the catalog, fetch its summary,
   cast/actors, poster, and backdrop from an online provider and write them to the rows
   the schema already reserves. This includes **auto-detection on add**: dropping a new
   movie into a watched folder fetches its metadata with no manual step.
2. **Library management** — Plex-style named libraries, each pointing at one or more
   folders with a type (Movies / TV), managed in-app rather than only via the `/media`
   Docker mount.

The work is split so the visible win ships first:

- **Phase A — Metadata provider + auto-enrichment** against the existing single `/media`
  root. This is the headline feature and drops into the *already-live* watch/scan loop.
- **Phase B — Multi-library folders + management API/UI**, layered on top once A is in.

## Requirements

- Provider access is **pluggable behind a `MetadataProvider` trait**; ship **TMDB first**
  (default) and **OMDb as a second impl** to prove the abstraction.
- Enrichment is **idempotent**: an already-matched title is not re-fetched unless a
  force-refresh is requested. A first-run scan of 10,000 files must not exceed provider
  rate limits (bounded concurrency, mirroring the ffprobe semaphore in
  `10-phase1-foundation-data.md` §Scaling notes).
- **Graceful degradation**: with no API key configured, ingestion behaves exactly as
  today (filename-only) — metadata is simply skipped, never an error.
- Enrichment runs off the request path; catalog GETs stay fast and the moka cache is
  invalidated after enrichment writes (reuse the existing `Invalidator` callback).
- Artwork is downloaded into `/config/images` (the root `AppConfig::images_dir()` already
  documents and `GET /api/images/*path` already serves); `poster_path`/`backdrop_path`
  store paths relative to it.
- **Security (Phase B):** every user-supplied library folder must canonicalize to a
  location **inside `MEDIA_DIR`**. `/media` remains the read-only trust boundary
  (`50-phase5-playback-packaging.md` §"/media Read-Only"); a `..`/symlink escape is a
  `400`. The UI must never be able to point a library at an arbitrary host path.
- No auth (LAN-appliance model, per `00-architecture.md`) — the new mutation endpoints
  inherit the same no-auth, LAN-only posture.

## Packages / crates

Adds to the Phase 1 set: `reqwest` (with `rustls-tls` + `json`, no OpenSSL) for the
provider HTTP client, and `async-trait` for the provider trait. `serde`/`serde_json`,
`tokio`, `tracing`, `anyhow`/`thiserror` are already in the workspace.

## File structure (where to save)

```
backend/
├── migrations/
│   ├── V2__metadata.sql          # Phase A: external ids + metadata_state
│   └── V3__libraries.sql         # Phase B: libraries + library_folders + scoping columns
└── crates/
    └── metadata/  (medi-metadata)  # NEW crate
        └── src/
            ├── lib.rs
            ├── provider.rs        # MetadataProvider trait + shared Match/Details types
            ├── tmdb.rs            # TmdbProvider (primary)
            ├── omdb.rs            # OmdbProvider (second impl)
            └── enrich.rs          # enrich_movie(...) orchestration + artwork download
```

Touched existing files (Phase A): `crates/core/src/config.rs`,
`crates/db/src/{models,writes,queries}.rs`, `crates/ingest/src/worker.rs`,
`crates/api/src/{main,routes,dto,error,state}.rs`, `crates/api/Cargo.toml`,
workspace `Cargo.toml`. Phase B additionally touches `crates/ingest/src/scanner.rs`,
`crates/db/src/writes.rs` (library scoping), `crates/api/src/routes.rs`, and `client/`.

## DB migrations

`V2__metadata.sql` (Phase A) — the schema already holds `overview`, `poster_path`,
`backdrop_path`, `people`, `credits`; this adds only external ids and match state:

```sql
ALTER TABLE movies ADD COLUMN tmdb_id        INTEGER;
ALTER TABLE movies ADD COLUMN imdb_id        TEXT;
ALTER TABLE movies ADD COLUMN metadata_state TEXT NOT NULL DEFAULT 'pending'; -- pending|matched|unmatched|failed
ALTER TABLE series ADD COLUMN tmdb_id        INTEGER;
ALTER TABLE series ADD COLUMN imdb_id        TEXT;
ALTER TABLE series ADD COLUMN metadata_state TEXT NOT NULL DEFAULT 'pending';
CREATE INDEX idx_movies_meta_state ON movies(metadata_state);
```

`V3__libraries.sql` (Phase B) — the Plex-style model, plus scoping existing rows to a
library so scans and reaps are per-library:

```sql
CREATE TABLE libraries (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL,
    kind       TEXT NOT NULL,                     -- 'movie' | 'series'
    created_at INTEGER NOT NULL
);
CREATE TABLE library_folders (
    id         INTEGER PRIMARY KEY,
    library_id INTEGER NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
    path       TEXT NOT NULL,                     -- absolute, must resolve under MEDIA_DIR
    UNIQUE(library_id, path)
);
ALTER TABLE movies ADD COLUMN library_id INTEGER REFERENCES libraries(id) ON DELETE CASCADE;
ALTER TABLE series ADD COLUMN library_id INTEGER REFERENCES libraries(id) ON DELETE CASCADE;
```

Migrations stay idempotent via refinery's version records (`01-db-schema.md`). On first
boot with **no** libraries defined, auto-seed one `movie` and one `series` library rooted
at `MEDIA_DIR`, so existing single-mount deployments keep working with no config change.

> **Version coordination.** This task reserves migration versions **V2** and **V3**;
> `70-audio-quality-and-profiles.md` uses **V4**. refinery versions must stay gapless and
> monotonic, so whichever of `60`/`70` ships later renumbers to keep the sequence
> contiguous — the ordering constraint is refinery's; the numbers are not load-bearing.

## Config additions

Extend `AppConfig` (`crates/core/src/config.rs`) and its `from_env()` `KEYS` allowlist:

| Env var             | Field               | Default   |
|---------------------|---------------------|-----------|
| `TMDB_API_KEY`      | `tmdb_api_key`      | *(unset)* |
| `OMDB_API_KEY`      | `omdb_api_key`      | *(unset)* |
| `METADATA_PROVIDER` | `metadata_provider` | `tmdb`    |
| `METADATA_ENABLED`  | `metadata_enabled`  | `true`    |
| `METADATA_LANGUAGE` | `metadata_language` | `en-US`   |

> **Prerequisite fix**: `crates/api/src/main.rs` currently constructs
> `AppConfig::default()` (a standing in-code TODO), not `AppConfig::from_env()`. Switch it
> to `from_env()` as part of this task, or the new keys never load and metadata silently
> stays off.

## Sub-tasks

### Phase A — metadata provider + auto-enrichment

1. **`metadata` crate + trait** (`provider.rs`): define
   ```rust
   #[async_trait]
   pub trait MetadataProvider: Send + Sync {
       async fn search(&self, title: &str, year: Option<i64>, kind: MediaKind)
           -> Result<Vec<Match>>;
       async fn details(&self, id: &ProviderId) -> Result<Details>;
   }
   ```
   with shared `Match { provider_id, title, year, score }` and
   `Details { overview, cast: Vec<CreditIn>, poster_url, backdrop_url, imdb_id, tmdb_id }`.
2. **TMDB impl** (`tmdb.rs`): `reqwest` against `api.themoviedb.org/3` — `/search/movie`,
   `/movie/{id}?append_to_response=credits`, and `/configuration` for the image base URL.
   Bound concurrency with a `tokio::sync::Semaphore` and respect TMDB rate limits.
3. **OMDb impl** (`omdb.rs`): `www.omdbapi.com` by title+year or imdb id. Second impl to
   validate the trait; TMDB stays the default `metadata_provider`.
4. **Enrichment orchestration** (`enrich.rs`): `enrich_movie(db, provider, movie_id, force)`
   → read parsed title+year → `search` → best-match (year exact + title-similarity
   threshold; below threshold ⇒ `metadata_state = 'unmatched'`) → `details` → download
   poster/backdrop into `/config/images/movies/<id>/{poster,backdrop}.jpg` → write
   `overview`, artwork paths, `people`/`credits` (billing order into `credits.ord`),
   `tmdb_id`/`imdb_id`, set `metadata_state = 'matched'`. Idempotent: skip `matched` rows
   unless `force`. **Downloads are atomic** (temp file + `rename`) and overwrite in place on
   refresh/re-match, per §Asset storage; re-download only the missing asset if the row is
   `matched` but a file is gone.
4a. **Asset reaping** (`db/src/writes.rs` + `ingest/src/worker.rs`): when a title's rows are
   deleted (reap or, in Phase B, a library-delete cascade), also remove its
   `/config/images/<kind>/<id>/` directory; add the opportunistic sweep that reconciles
   `/config/images` against surviving ids.
5. **DB write helpers** (`db/src/writes.rs`, mirrored in `models.rs`):
   `set_movie_metadata(...)`, `upsert_credit(person, role, character, ord)` (people already
   de-dupe on the unique `name`), `set_metadata_state(id, state)`.
6. **Wire into the live ingest loop** (`ingest/src/worker.rs`): extend `WorkerConfig` with
   an optional `Arc<dyn MetadataProvider>` + concurrency cap. After the writer task
   persists a **new** title in `run_scan`, enqueue its id on a bounded mpsc consumed by an
   enrichment worker calling `enrich_movie`; call the existing `invalidate()` after writes.
   Because `main.rs` already spawns `run_scan` then `watch` (debounced `notify` →
   incremental `run_scan`), **a newly dropped file auto-enriches with no extra wiring** —
   this is the auto-detect-on-add feature.
7. **Construct the provider** from config in `crates/api/src/main.rs`; add `medi-metadata`
   to `crates/api/Cargo.toml`; pass the provider into `WorkerConfig`.
8. **Manual controls** (`crates/api/src/routes.rs`, see §API additions): refresh, list
   candidate matches, pin a match. Invalidate the cache on each write.

### Phase B — libraries & folders

9. **`V3__libraries.sql`** + auto-seed of the default movie/series libraries on first boot.
10. **Scanner → multi-root** (`ingest/src/scanner.rs`, `worker.rs`): replace the single
    `WorkerConfig.media_dir` with `roots: Vec<{ library_id, kind, PathBuf }>`. `scan()`
    already takes `root: &Path`; loop per folder and tag each `DiscoveredFile` with its
    `library_id`/`kind`. The library `kind` **overrides** filename guessing (a stray
    `SxxEyy` in a Movies library stays a movie) — matches Plex, removes a misclassification
    class. `resolve_owner` writes `library_id`.
11. **Libraries CRUD API** (see §API additions) with the `MEDIA_DIR` path-containment
    check on every folder write.
12. **Client Settings UI** (`client/`): the client is presently an empty scaffold and the
    TV app is read-only, so this introduces its first mutation surface — a
    **Settings → Libraries** screen (list, add/remove folder rows via a picker restricted
    to `MEDIA_DIR`, per-library `kind`, "Scan now") plus a richer **Detail** screen
    (overview, cast row, poster/backdrop, "Refresh / Fix match", an `unmatched` badge).
    Extend `client/packages/api-client` types (type-sync owned by `40-phase4-tv-client-ui.md`).

## Asset storage, caching & lifecycle

Enrichment downloads binary artwork; this section is the contract for how those files are
written, served, cached, and reclaimed so the `/config` cache never truncates, leaks, or
serves stale images.

**On-disk layout.** Artwork lives under `AppConfig::images_dir()` (`/config/images`),
partitioned by kind and id so a title owns its own directory:
```
/config/images/
├── movies/<movie_id>/{poster,backdrop}.jpg
└── series/<series_id>/{poster,backdrop}.jpg
```
`movies.poster_path` / `backdrop_path` store the path **relative to** `images_dir()` (e.g.
`movies/12/poster.jpg`), which is exactly what `GET /api/images/*path`'s `ServeDir`
resolves. No absolute paths in the DB.

**Atomic writes.** Each download streams to a temp file in the same directory
(`poster.jpg.tmp`) and is `rename`d into place only after the full body is received —
`rename` within one filesystem is atomic, so a crash or a killed container never leaves a
half-written image that `ServeDir` would serve. The DB `poster_path` is set in the same
transaction that marks the title `matched`, so the row and the file commit together.

**Overwrite on re-match / refresh.** `POST /api/movies/:id/refresh` and
`POST /api/movies/:id/match` **replace** the title's images in place (same atomic
temp+rename over the existing file) rather than writing new filenames, so a corrected match
never accumulates a second poster. Stable filenames also mean the `/api/images` URL for a
title is stable across refreshes.

**Orphan reaping.** When a title is removed, its `/config/images/<kind>/<id>/` directory
must be deleted too. Phase 1's `reap_missing` (`ingest/src/worker.rs`) deletes only DB
rows; extend the reap path (and the Phase B `DELETE /api/libraries/:id` cascade) to also
remove the corresponding image directory, so deleting media does not leak artwork in the
cache. A periodic sweep (opportunistic, off the request path) reconciles `/config/images`
against surviving `movies`/`series` ids as a backstop.

**Cache layers — three, kept distinct.**
- *HTTP response cache* (moka, `crates/api`): catalog JSON. Enrichment writes call the
  existing `Invalidator` so the next `GET /api/library` / `GET /api/movies/:id` reflects new
  overview + artwork. Image bytes are **not** in moka — they are static files.
- *Image bytes on disk* (`/config/images`): the durable artwork cache. `GET /api/images`
  serves these via `ServeDir` with long-lived `Cache-Control` (immutable per stable path)
  so TV clients and any reverse proxy cache aggressively.
- *Provider-response cache* (in `medi-metadata`, in-memory, process-lifetime): the TMDB
  `/configuration` image-base URL is fetched once; short-TTL memoization of recent
  `search`/`details` results avoids duplicate round-trips during a burst scan. This is a
  courtesy cache to respect rate limits, not a source of truth.

**Idempotency & re-download avoidance.** A title already `matched` with both image files
present on disk is skipped on re-scan (no re-download) unless `force`. If the row is
`matched` but an image file is missing (manual deletion, partial `/config` restore), the
enrichment pass re-downloads just the missing asset.

## API additions

Extends `02-api-contract.md`. All JSON, no auth, cache-invalidating on write.

| Method & Path | Purpose | Phase |
|---|---|---|
| `POST /api/movies/:id/refresh` | Force re-enrichment of one title | A |
| `GET  /api/movies/:id/matches?query=` | Candidate provider matches to choose from | A |
| `POST /api/movies/:id/match` | Pin `{ provider_id }` and re-enrich | A |
| `GET  /api/libraries` | List libraries + their folders | B |
| `POST /api/libraries` | Create `{ name, kind, folders[] }` | B |
| `PATCH /api/libraries/:id` | Rename / add / remove folders | B |
| `DELETE /api/libraries/:id` | Remove a library (cascades its rows) | B |
| `POST /api/libraries/:id/scan` | Trigger an immediate scan of one library | B |

`GET /api/movies/:id` and `GET /api/series/:id` (already live) need no shape change — they
begin returning populated `overview`, cast (`credits`), and artwork once enrichment runs.
Follow the existing `dto.rs` / `error.rs` / `state.rs` patterns; folder-path validation
errors return the standard `{ "error": { "code": "bad_request", ... } }` with `400`.

## Scaling notes

- Bound enrichment concurrency (a `Semaphore`, as with ffprobe) so a first-run scan of
  10,000 titles respects TMDB/OMDb rate limits and never spawns a request per file at once.
- Keep enrichment on the write side only; the write path stays single-threaded (WAL = one
  writer) while catalog reads remain concurrent and cache-served.
- Cache provider `search`/`details` responses where cheap (e.g. the TMDB image-base config
  is fetched once per process) to avoid redundant round-trips.

## Verification

> Note: this dev machine has no Rust toolchain, so the crate tests below run on a machine
> with Rust installed; they cannot be `cargo test`ed where this doc was authored.

- **Migrations** (`cargo test -p medi-db`): fresh DB applies V2 (+ V3) once; restart is a
  no-op (refinery version records); an existing single-`/media` DB gets the auto-seeded
  libraries and keeps working.
- **Provider parsing** (`cargo test -p medi-metadata`): `TmdbProvider`/`OmdbProvider`
  `search`/`details` parse **recorded JSON fixtures** (no live network in tests);
  best-match scoring picks the right candidate and falls to `unmatched` below threshold.
- **Enrichment + scan** (`cargo test -p medi-ingest`): a newly written title is enqueued
  for enrichment; Phase B multi-root scan tags rows with the right `library_id` and the
  library `kind` overrides a filename mis-guess.
- **End-to-end** (Rust box, `TMDB_API_KEY` set): start the server; drop
  `Arrival (2016).mkv` into `/media`; the watch debounce → scan → ffprobe → enrichment
  writes overview + cast + poster; `GET /api/movies/:id` returns them;
  `POST /api/movies/:id/refresh` re-fetches; a wrong match is corrected via
  `POST /api/movies/:id/match`.
- **Asset lifecycle** (`cargo test -p medi-metadata` / `-p medi-ingest`): a download
  interrupted before completion leaves no `.jpg` (only a stale `.tmp`), and the next pass
  completes it atomically; a refresh overwrites the existing poster in place (no second
  file, stable URL); deleting a title (or, Phase B, its library) removes its
  `/config/images/<kind>/<id>/` directory; a `matched` row with a manually deleted image
  re-downloads only that file.
- **Graceful degradation**: with `TMDB_API_KEY` unset, the same drop ingests filename-only
  with no error, proving metadata is optional.
- **Security (Phase B)**: `POST /api/libraries` with a folder outside `MEDIA_DIR` (or a
  `..` escape) is rejected `400`.
