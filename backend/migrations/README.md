# migrations

refinery-embedded SQL migrations for `library.db`, embedded at compile time by the
`db` crate (`refinery::embed_migrations!("../../migrations")`).

- `V1__init.sql` — schema + indexes, from the canonical schema in
  `docs/.tasks/01-db-schema.md`.

## PRAGMA ordering

The `db` crate's `pool` module handles the file-level tuning that must precede table
creation: on a **fresh** database (detected via `PRAGMA page_count == 0`) it sets
`PRAGMA page_size = 65536` first, then establishes the persisted `journal_mode = WAL`
and `auto_vacuum = INCREMENTAL`, and only then does `migrate::run` apply the DDL here.
The remaining per-connection PRAGMAs (`synchronous`, `busy_timeout`, `foreign_keys`,
`cache_size`, `mmap_size`, `temp_store`) are applied on every pool checkout by the
connection customizer. Migration files stay pure DDL — they set no PRAGMAs.

## Adding a migration

Add `V2__<name>.sql` (sequential version, double underscore). refinery records applied
versions in `refinery_schema_history`, so `migrate::run` is idempotent across restarts.
