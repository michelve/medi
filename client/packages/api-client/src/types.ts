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

/** `sort` query values accepted by `GET /api/library` and `GET /api/genres/:id`. */
export type LibrarySort = 'sort_title' | 'added_at';

// ---------------------------------------------------------------------------
// Genres & discovery (`docs/.tasks/91` Phase A)
// ---------------------------------------------------------------------------

/** A genre carried by a title (id + name), e.g. on a movie detail's metadata line. */
export interface Genre {
  id: number;
  name: string;
}

/** One entry in `GET /api/genres` — a genre with its title count (movies + series). */
export interface GenreCount extends Genre {
  /** Number of titles carrying this genre; always ≥ 1 (empty genres are excluded). */
  count: number;
}

/**
 * One horizontal category row on the landing page (`GET /api/library/rows`). `key` is a
 * stable machine id (`recently_added` or `genre:878`); `title` is the display heading;
 * `genre_id` is present for a genre row (so "See all →" can link to `/genre/:id`) and
 * omitted for the synthetic "Recently Added" row.
 */
export interface CategoryRow {
  key: string;
  title: string;
  items: LibraryItem[];
  genre_id?: number;
}

/** `GET /api/library/rows` — the landing page's curated rows in one request. */
export interface LibraryRows {
  rows: CategoryRow[];
}

/**
 * `GET /api/people/:id` — a person page (`docs/.tasks/91` Phase B): the enriched person
 * plus their in-library filmography (poster tiles, newest first). `photo` is a ready-to-fetch
 * `/api/images/people/<id>/photo.jpg` URL, omitted before the person is enriched; `biography`
 * / `tmdb_id` are likewise omitted pre-enrichment. `filmography` is always present (it comes
 * from credits, not enrichment) though it may be empty.
 */
export interface PersonPage {
  id: number;
  name: string;
  photo?: string;
  biography?: string;
  tmdb_id?: number;
  filmography: LibraryItem[];
}

// ---------------------------------------------------------------------------
// Detail aggregates. `Movie`/`Series` rows are flattened into their detail
// envelope by `#[serde(flatten)]`, so the row fields sit at the top level
// alongside `media_files` / `seasons` / `credits`.
// ---------------------------------------------------------------------------

/**
 * Metadata match lifecycle on a `movies`/`series` row (`docs/.tasks/60`).
 * `pending` → not yet enriched, `matched` → provider details written,
 * `unmatched` → no candidate cleared the threshold, `failed` → provider error.
 */
export type MetadataState = 'pending' | 'matched' | 'unmatched' | 'failed';

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
  /**
   * The movie's transparent-PNG title logo from fanart.tv (Task 93). A stored path relative
   * to the images root — resolve with `client.imageUrl(...)`, exactly like `poster_path`.
   * `null`/absent when the movie has no logo or fanart is unconfigured; render the text title.
   */
  logo_path?: string | null;
  /**
   * The movie's fanart.tv background wallpaper (Task 95). A stored path relative to the
   * images root — resolve with `client.imageUrl(...)`. When present it's shown on the detail
   * hero in place of `backdrop_path` (fanart wins, TMDB backdrop is the fallback).
   * `null`/absent when the movie has no wallpaper or fanart is unconfigured.
   */
  wallpaper_path?: string | null;
  /** External ids + match state (Phase A, `docs/.tasks/60`). */
  tmdb_id?: number | null;
  imdb_id?: string | null;
  metadata_state?: MetadataState;
  /** Owning library (Phase B). Null until scoped. */
  library_id?: number | null;
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
  tmdb_id?: number | null;
  imdb_id?: string | null;
  metadata_state?: MetadataState;
  library_id?: number | null;
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

/**
 * An episode together with its on-disk media files (`EpisodeWithFiles`). The `Episode`
 * row is flattened in, so its fields sit at the top level alongside `media_files` — the
 * same shape as `MovieDetail`. The primary file's `id` is the `file_id` passed to
 * `GET /api/stream/:file_id` to play the episode. Empty until the file is ingested/probed.
 */
export interface EpisodeWithFiles extends Episode {
  media_files: MediaFile[];
}

/** A season together with its ordered episodes (`SeasonWithEpisodes`). */
export interface SeasonWithEpisodes extends Season {
  episodes: EpisodeWithFiles[];
}

/**
 * Immersive-audio marker on an audio track (`docs/.tasks/70`). Mirrors the backend
 * `audio_streams.immersive` string.
 */
export type ImmersiveAudio = 'none' | 'dolby_atmos' | 'dts_x';

/**
 * One audio track of a media file (`audio_streams`, Task 70). Mirrors
 * `medi_db::models::AudioStream`. `stream_index` is what react-native-video's
 * `selectedAudioTrack` selects by.
 */
export interface AudioStream {
  id: number;
  media_file_id: number;
  stream_index: number;
  codec: string | null;
  profile: string | null;
  channels: number | null;
  channel_layout: string | null;
  bitrate: number | null;
  sample_rate: number | null;
  language: string | null;
  title: string | null;
  immersive: ImmersiveAudio;
  is_default: boolean;
}

/**
 * How a subtitle track can be served (`docs/.tasks/90`). `text` tracks convert to WebVTT
 * and ride as a react-native-video `textTracks` sidecar (no video transcode); `image`
 * tracks (PGS / VobSub) can only be burned in via a forced transcode.
 */
export type SubtitleFormat = 'text' | 'image';

/**
 * One subtitle track of a media file (`subtitle_streams`, Task 90). Mirrors
 * `medi_db::models::SubtitleStream`. Either an embedded track (`stream_index` set,
 * `external_path` null) or an external sidecar (`external_path` set, `stream_index` null).
 */
export interface SubtitleStream {
  id: number;
  media_file_id: number;
  stream_index: number | null;
  codec: string | null;
  format: SubtitleFormat;
  language: string | null;
  title: string | null;
  is_default: boolean;
  is_forced: boolean;
  is_external: boolean;
  external_path: string | null;
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
  /** Audio tracks of this file (Task 70). Empty until probed. Drives `selectedAudioTrack`. */
  audio_streams: AudioStream[];
  /**
   * Subtitle tracks of this file (Task 90) — embedded + external sidecars. Empty until
   * probed. Text tracks attach as a `textTracks` sidecar; image tracks force a burn-in.
   */
  subtitle_streams: SubtitleStream[];
}

/** A joined `credits` + `people` billing entry. */
export interface Credit {
  id: number;
  person_id: number;
  person_name: string;
  role: string | null;
  character: string | null;
  ord: number | null;
  /**
   * The person's headshot path (Task 91 Phase B), relative to the images root — resolve
   * with `client.imageUrl(...)`. `null` for a person not yet enriched (show initials).
   */
  photo_path: string | null;
}

/** A YouTube trailer/teaser of a movie (Task 91 detail extensions). */
export interface Trailer {
  id: number;
  youtube_key: string;
  name: string | null;
  /** TMDB `type`: "Trailer" | "Teaser" | "Clip" | … */
  kind: string | null;
}

/** A TMDB collection (franchise) a movie belongs to (Task 91 detail extensions). */
export interface Collection {
  id: number;
  name: string;
  /** Resolve with `client.imageUrl(...)`; `null` when the collection has no art. */
  poster_path: string | null;
}

/**
 * `GET /api/movies/:id` — movie row (flattened) + its files, credits, trailers, and
 * franchise collection (Task 91 detail extensions). `collection_movies` is the other
 * in-library movies of the same franchise (this movie excluded), as poster tiles.
 */
export interface MovieDetail extends Movie {
  media_files: MediaFile[];
  credits: Credit[];
  trailers: Trailer[];
  collection: Collection | null;
  collection_movies: LibraryItem[];
  /** Genres this movie belongs to, in name order. Empty when unmatched. */
  genres: Genre[];
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

/**
 * `GET /api/trickplay/:file_id/meta` — grid geometry of a title's tiled-JPG scrub-thumbnail
 * mosaic. Mirrors the backend `TrickplayMeta` (snake_case wire shape). The mosaic image
 * itself is fetched from `ApiClient.trickplayUrl(file_id, 'jpg')`. A `404` from this route
 * means "no croppable thumbnails" (BIF-only or none) — callers fall back to a plain bar.
 */
export interface TrickplayMetaResponse {
  file_id: number;
  /** Always `"tiled_jpg"` when 200 (the only client-croppable kind). */
  kind: string;
  /** Milliseconds between sampled frames. */
  interval_ms: number;
  /** Width of one thumbnail cell, px. */
  tile_w: number;
  /** Height of one thumbnail cell, px. */
  tile_h: number;
  /** Columns in the mosaic. */
  cols: number;
  /** Rows in the mosaic. */
  rows: number;
}

// ---------------------------------------------------------------------------
// Per-file tracks (`docs/.tasks/97` Part C) — `GET /api/files/:file_id`
// ---------------------------------------------------------------------------

/**
 * One audio track in `GET /api/files/:file_id` (`docs/.tasks/97` Part C) — the subset of an
 * `audio_streams` row the player's audio menu needs. `stream_index` is the value passed back
 * as `audioTrack` on `stream(...)` to switch to this track.
 */
export interface FileAudioTrack {
  stream_index: number;
  codec?: string;
  channels?: number;
  channel_layout?: string;
  language?: string;
  title?: string;
  is_default: boolean;
}

/**
 * One subtitle track in `GET /api/files/:file_id` (`docs/.tasks/97` Part C, consumed by
 * `99`). `id` is the `subtitle_streams` row id (an external sidecar is addressed as
 * `ext<id>`); `stream_index` is the embedded ffprobe index (absent for external tracks).
 */
export interface FileSubtitleTrack {
  id: number;
  stream_index?: number;
  external: boolean;
  format: SubtitleFormat;
  language?: string;
  title?: string;
  is_default: boolean;
  is_forced: boolean;
}

/**
 * `GET /api/files/:file_id` — a file's audio + subtitle tracks (`docs/.tasks/97` Part C).
 * Lets a deep link to `/play/:file_id` (no router state) populate the player's menus.
 */
export interface FileTracks {
  file_id: number;
  audio: FileAudioTrack[];
  subtitles: FileSubtitleTrack[];
}

/** The client platform selecting a static per-device capability default (`docs/.tasks/70`). */
export type StreamPlatform = 'appletv' | 'shield' | 'androidtv' | 'web';

/** "Best available quality" control sent to `/api/stream` (`docs/.tasks/70`). */
export type QualityProfile = 'original' | 'auto' | 'capped';

/**
 * Client hints the stream decision honors. The video hints (`hdr`/`dv`/`sdr`) are
 * unchanged; Task 70 adds the audio + quality axis: `platform` picks the static device
 * default, and the rest overlay a detected `AudioCapabilities` payload.
 */
export interface StreamHints {
  /** Display cannot render HDR — force SDR tone-map. */
  hdr?: boolean;
  /** Client cannot decode Dolby Vision. */
  dv?: boolean;
  /** Explicitly request an SDR result. */
  sdr?: boolean;
  /** Selects the static per-platform capability default (`docs/.tasks/70`). */
  platform?: StreamPlatform;
  /** `EXTRA_MAX_CHANNEL_COUNT` — max audio channels the sink accepts. */
  maxChannels?: number;
  /**
   * ExoPlayer `EXTRA_ENCODINGS` tokens (e.g. `['eac3', 'ac3', 'aac', 'eac3_joc']`).
   * `eac3_joc` present ⇒ lossy Atmos passthrough. Overlays the platform default.
   */
  audio?: string[];
  /** Convenience flag: adds `eac3_joc` to the audio set when true. */
  atmos?: boolean;
  /** `MaxStreamingBitrate` in bits/sec (uncapped when omitted). */
  maxBitrate?: number;
  /** `original` | `auto` | `capped`. */
  quality?: QualityProfile;
  /**
   * Selected subtitle track for burn-in (`docs/.tasks/90`): the embedded `stream_index`
   * of an **image** track. Only meaningful with `subBurn: true`; a text track never sets
   * this — it is fetched as a `.vtt` sidecar and the video can still direct-play.
   */
  sub?: number;
  /** `true` ⇒ burn the selected image subtitle into the video (forces a transcode). */
  subBurn?: boolean;
  /**
   * `true` ⇒ force a server transcode even when the file would direct-play. The web player
   * sends this to recover when a `direct` stream turns out to be unplayable in the browser
   * (a `<video>` `MEDIA_ERR_SRC_NOT_SUPPORTED` / `MEDIA_ERR_DECODE`) — the server then returns
   * an H.264+AAC HLS stream hls.js can always play.
   */
  forceTranscode?: boolean;
  /**
   * Selected audio track (`docs/.tasks/97` Part C): the ffprobe `stream_index` of one of the
   * file's `audio_streams`. Switches the source audio the server transcodes and yields a
   * distinct HLS session per track. A browser `<video>` can't switch an embedded track, so
   * pair a non-default selection with `forceTranscode: true` when the base decision was
   * `direct`.
   */
  audioTrack?: number;
}

// ---------------------------------------------------------------------------
// Metadata enrichment (Phase A, `docs/.tasks/60`)
// ---------------------------------------------------------------------------

/** Outcome of a refresh / match request. */
export type EnrichOutcome = 'matched' | 'unmatched' | 'skipped';

/**
 * `POST /api/movies/:id/refresh` and `POST /api/movies/:id/match` response.
 * `provider_id` is the pinned provider token (`tmdb:movie:603`) when matched.
 */
export interface RefreshResponse {
  id: number;
  outcome: EnrichOutcome;
  provider_id?: string;
}

/**
 * `POST /api/metadata/backfill` response. The backfill runs in the background, so this only
 * acknowledges acceptance; `already_running` is `true` when a backfill was already in flight
 * (a re-hit is idempotent, not queued twice).
 */
export interface BackfillResponse {
  status: 'accepted';
  already_running: boolean;
}

/** One candidate from `GET /api/movies/:id/matches`. */
export interface MatchCandidate {
  /** Opaque provider token to pass to `POST /api/movies/:id/match`. */
  provider_id: string;
  title: string;
  year?: number;
  /** Match confidence in `[0,1]`. */
  score: number;
}

/** `GET /api/movies/:id/matches` response (candidates, best-first). */
export interface MatchesResponse {
  id: number;
  candidates: MatchCandidate[];
}

// ---------------------------------------------------------------------------
// Libraries (Phase B, `docs/.tasks/60`)
// ---------------------------------------------------------------------------

/** A library's kind — matches the catalog `TitleKind`. */
export type LibraryTypeKind = 'movie' | 'series';

/** A library row with its folder paths (`GET /api/libraries`). */
export interface Library {
  id: number;
  name: string;
  kind: LibraryTypeKind;
  created_at: number;
  folders: string[];
}

/** `POST /api/libraries` body. */
export interface CreateLibraryRequest {
  name: string;
  kind: LibraryTypeKind;
  folders: string[];
}

/** `PATCH /api/libraries/:id` body — all fields optional. */
export interface PatchLibraryRequest {
  name?: string;
  add_folders?: string[];
  remove_folders?: string[];
}

/** Structured error body: `{ "error": { "code", "message" } }`. */
export interface ApiErrorBody {
  error: {
    code: string;
    message: string;
  };
}

// -- Enrichment & ingest status (`docs/.tasks/96`) -------------------------

/** Per-`metadata_state` title counts for one kind. */
export interface StateCounts {
  total: number;
  matched: number;
  pending: number;
  unmatched: number;
  failed: number;
}

/** Whether a provider is configured (+ its name for the metadata provider). */
export interface ProviderStatus {
  name?: string;
  configured: boolean;
}

/** The full `GET /api/status` envelope. */
export interface SystemStatus {
  version: string;
  media_dir_present: boolean;
  counts: { movies: StateCounts; series: StateCounts };
  providers: { metadata: ProviderStatus; fanart: ProviderStatus };
  last_scan: {
    started_at: number | null;
    finished_at: number | null;
    written: number;
    probe_failures: number;
  };
  last_enrichment: {
    finished_at: number | null;
    matched: number;
    unmatched: number;
    failed: number;
  };
  workers: { watcher_alive: boolean; backfill_interval_hours: number };
}

/** One `unmatched`/`failed` title the operator can act on. */
export interface UnmatchedItem {
  id: number;
  kind: 'movie' | 'series';
  title: string;
  year?: number;
  state: string;
  path?: string;
}

/** A keyset page of unmatched titles (`next_cursor` is the last id, or null). */
export interface UnmatchedPage {
  items: UnmatchedItem[];
  next_cursor: number | null;
}

/** One ffprobe-failed file. */
export interface ProbeFailureItem {
  path: string;
  error: string;
  last_attempt_at: number;
}

/** A keyset page of probe failures (`next_cursor` is the last `last_attempt_at`). */
export interface ProbeFailuresPage {
  items: ProbeFailureItem[];
  next_cursor: number | null;
}
