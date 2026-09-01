/**
 * Response types for the medi REST API.
 *
 * These mirror, field-for-field, the JSON the backend `api` crate serializes:
 *  - the envelope shapes in `docs/.tasks/02-api-contract.md`
 *    (`LibraryPage`, `StreamDecision`), and
 *  - the detail aggregates in `backend/crates/db/src/models.rs`
 *    (`MovieDetail`, `SeriesDetail`), which are `#[serde(flatten)]`-ed onto the
 *    base `Movie`/`Series` row.
 *
 * Keep this file in lockstep with those Rust definitions. Sub-task 6 of the API
 * contract (optional OpenAPI emission) is not done, so these hand-written types
 * are the source of truth on the client side.
 */

/** `"movie"` or `"series"` discriminant on a library card. */
export type LibraryKind = 'movie' | 'series';

/**
 * HDR tier string as stored on `media_files` / surfaced on a card. The backend
 * emits loose ffprobe-fidelity strings, so this is a union of the known values
 * plus a `string` escape hatch for anything new the prober learns to report.
 */
export type HdrTier =
  | 'dolbyvision'
  | 'hdr10'
  | 'hdr10plus'
  | 'hlg'
  | (string & {});

/** One poster tile in `GET /api/library`. */
export interface LibraryItem {
  kind: LibraryKind;
  id: number;
  title: string;
  /** Omitted by the backend for titles with no release year. */
  year?: number;
  /** Ready-to-fetch `/api/images/...` URL; omitted when the title has no art. */
  poster?: string;
  /** Highest HDR tier across the title's files; omitted for SDR / unprobed. */
  hdr?: HdrTier;
}

/** One page of the unified catalog (`GET /api/library`). */
export interface LibraryPage {
  items: LibraryItem[];
  /** Opaque keyset cursor for the next page, or `null` when exhausted. */
  next_cursor: string | null;
}

/** `sort` query values accepted by `GET /api/library`. */
export type LibrarySort = 'sort_title' | 'added_at';

// ---------------------------------------------------------------------------
// Detail aggregates. `Movie`/`Series` rows are flattened into their detail
// envelope by `#[serde(flatten)]`, so the row fields sit at the top level
// alongside `media_files` / `seasons` / `credits`.
// ---------------------------------------------------------------------------

/** A row of `movies` (also the flattened head of `MovieDetail`). */
export interface Movie {
  id: number;
  title: string;
  sort_title: string;
  year: number | null;
  overview: string | null;
  added_at: number;
  poster_path: string | null;
  backdrop_path: string | null;
}

/** A row of `series` (also the flattened head of `SeriesDetail`). */
export interface Series {
  id: number;
  title: string;
  sort_title: string;
  year: number | null;
  overview: string | null;
  added_at: number;
  poster_path: string | null;
  backdrop_path: string | null;
}

export interface Season {
  id: number;
  series_id: number;
  season_number: number;
}

export interface Episode {
  id: number;
  season_id: number;
  episode_number: number;
  title: string | null;
  overview: string | null;
}

/** A season together with its ordered episodes (`SeasonWithEpisodes`). */
export interface SeasonWithEpisodes extends Season {
  episodes: Episode[];
}

/**
 * A `media_files` row. Belongs to exactly one movie OR one episode. Mirrors
 * `medi_db::models::MediaFile`. Many fields are `null` until the file is probed.
 */
export interface MediaFile {
  id: number;
  movie_id: number | null;
  episode_id: number | null;
  path: string;
  container: string | null;
  size_bytes: number | null;
  duration_ms: number | null;
  video_codec: string | null;
  video_profile: string | null;
  width: number | null;
  height: number | null;
  bit_depth: number | null;
  bitrate: number | null;
  transfer_characteristics: string | null;
  color_space: string | null;
  hdr_type: HdrTier | null;
  dv_profile: number | null;
  dv_bl_compatible_id: number | null;
  dv_level: number | null;
  hw_decode_unsupported: boolean;
}

/** A joined `credits` + `people` billing entry. */
export interface Credit {
  id: number;
  person_id: number;
  person_name: string;
  role: string | null;
  character: string | null;
  ord: number | null;
}

/** `GET /api/movies/:id` — movie row (flattened) + its files and credits. */
export interface MovieDetail extends Movie {
  media_files: MediaFile[];
  credits: Credit[];
}

/** `GET /api/series/:id` — series row (flattened) + seasons/episodes + credits. */
export interface SeriesDetail extends Series {
  seasons: SeasonWithEpisodes[];
  credits: Credit[];
}

/** Playback mode returned by `GET /api/stream/:file_id`. */
export type StreamMode = 'direct' | 'hls';

/** The playback decision envelope from `GET /api/stream/:file_id`. */
export interface StreamDecision {
  file_id: number;
  mode: StreamMode;
  /** Stable slug explaining the decision, for logs/debugging. */
  reason: string;
  /** `/api/direct/:file_id` (direct) or an `index.m3u8` URL (hls). */
  url: string;
}

/** Client hints the stream decision honors (`?hdr=0&dv=0&sdr=1`). */
export interface StreamHints {
  /** Display cannot render HDR — force SDR tone-map. */
  hdr?: boolean;
  /** Client cannot decode Dolby Vision. */
  dv?: boolean;
  /** Explicitly request an SDR result. */
  sdr?: boolean;
}

/** Structured error body: `{ "error": { "code", "message" } }`. */
export interface ApiErrorBody {
  error: {
    code: string;
    message: string;
  };
}
