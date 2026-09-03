/**
 * Typed fetch client for the medi REST API (`docs/.tasks/02-api-contract.md`).
 *
 * The API is read-heavy, unauthenticated (LAN appliance), and every catalog
 * response carries an `ETag`. This client keeps a small in-memory ETag map and
 * replays `If-None-Match`, so a `304 Not Modified` transparently returns the
 * previously-parsed body without re-deserializing — matching the backend's moka
 * cache on the wire.
 *
 * URL builders (`imageUrl`, `previewUrl`, `trickplayUrl`, `directUrl`, `hlsUrl`)
 * produce absolute URLs against `baseUrl` for assets that are streamed by the
 * `<Image>` / `<Video>` native views rather than fetched here.
 */

import { ApiError } from './errors';
import type {
  ApiErrorBody,
  BackfillResponse,
  CreateLibraryRequest,
  FileTracks,
  GenreCount,
  Library,
  LibraryPage,
  LibraryRows,
  LibrarySort,
  MatchesResponse,
  MovieDetail,
  PatchLibraryRequest,
  PersonPage,
  ProbeFailuresPage,
  RefreshResponse,
  SeriesDetail,
  StreamDecision,
  StreamHints,
  SystemStatus,
  TrickplayMetaResponse,
  UnmatchedPage,
} from './types';

export interface ApiClientOptions {
  /**
   * Base URL of the backend, e.g. `http://192.168.1.10:8080`. No trailing slash
   * required (one is trimmed). On the LAN this is the appliance's address.
   */
  baseUrl: string;
  /** Injectable fetch (defaults to global `fetch`); handy for tests. */
  fetch?: typeof fetch;
  /**
   * Enable the built-in ETag / `If-None-Match` cache for catalog GETs.
   * Defaults to `true`.
   */
  etagCache?: boolean;
}

interface CacheEntry {
  etag: string;
  /** Parsed body, replayed verbatim on a `304`. */
  body: unknown;
}

export interface LibraryQuery {
  /** Keyset cursor from a previous page's `next_cursor`. */
  cursor?: string | null;
  /** Page size. */
  limit?: number;
  /** Sort key; defaults to the server default when omitted. */
  sort?: LibrarySort;
}

/** Per-request options common to every call. */
export interface RequestOptions {
  /** Abort signal — wired to the FSM teardown so a scroll-away cancels in-flight fetches. */
  signal?: AbortSignal;
}

export class ApiClient {
  private readonly baseUrl: string;
  private readonly doFetch: typeof fetch;
  private readonly etagCache: boolean;
  /** path+query → last ETag + parsed body. */
  private readonly cache = new Map<string, CacheEntry>();

  constructor(options: ApiClientOptions) {
    this.baseUrl = options.baseUrl.replace(/\/+$/, '');
    // Bind so a bare global `fetch` keeps the right `this`.
    const f = options.fetch ?? globalThis.fetch;
    this.doFetch = f.bind(globalThis);
    this.etagCache = options.etagCache ?? true;
  }

  // -- Catalog -------------------------------------------------------------

  /** `GET /api/library` — one page of the unified movie+series catalog. */
  library(query: LibraryQuery = {}, opts: RequestOptions = {}): Promise<LibraryPage> {
    const params = new URLSearchParams();
    if (query.cursor) params.set('cursor', query.cursor);
    if (query.limit != null) params.set('limit', String(query.limit));
    if (query.sort) params.set('sort', query.sort);
    const qs = params.toString();
    return this.getJson<LibraryPage>(`/api/library${qs ? `?${qs}` : ''}`, opts);
  }

  /**
   * `GET /api/library/rows` — the landing page's curated category rows ("Recently Added"
   * plus top genres) in one request (`docs/.tasks/91`). ETag-cached like the catalog.
   */
  libraryRows(opts: RequestOptions = {}): Promise<LibraryRows> {
    return this.getJson<LibraryRows>('/api/library/rows', opts);
  }

  /** `GET /api/genres` — genres with ≥1 title, ordered by count desc then name. */
  genres(opts: RequestOptions = {}): Promise<GenreCount[]> {
    return this.getJson<GenreCount[]>('/api/genres', opts);
  }

  /**
   * `GET /api/genres/:id` — one genre's keyset-paginated grid. **Same page shape as
   * `library()`** (`LibraryPage`), so a paging hook can point at either endpoint.
   */
  genreTitles(
    genreId: number,
    query: LibraryQuery = {},
    opts: RequestOptions = {},
  ): Promise<LibraryPage> {
    const params = new URLSearchParams();
    if (query.cursor) params.set('cursor', query.cursor);
    if (query.limit != null) params.set('limit', String(query.limit));
    if (query.sort) params.set('sort', query.sort);
    const qs = params.toString();
    return this.getJson<LibraryPage>(`/api/genres/${genreId}${qs ? `?${qs}` : ''}`, opts);
  }

  /** `GET /api/movies/:id` — full movie detail. */
  movie(id: number, opts: RequestOptions = {}): Promise<MovieDetail> {
    return this.getJson<MovieDetail>(`/api/movies/${id}`, opts);
  }

  /** `GET /api/series/:id` — full series detail. */
  series(id: number, opts: RequestOptions = {}): Promise<SeriesDetail> {
    return this.getJson<SeriesDetail>(`/api/series/${id}`, opts);
  }

  /**
   * `GET /api/people/:id` — a person page (`docs/.tasks/91` Phase B): the person's bio +
   * headshot and their in-library filmography. ETag-cached like the rest of the catalog.
   */
  person(id: number, opts: RequestOptions = {}): Promise<PersonPage> {
    return this.getJson<PersonPage>(`/api/people/${id}`, opts);
  }

  /**
   * `GET /api/stream/:file_id` — the direct-vs-HLS playback decision.
   * `hints` map to the query the backend honors: the video axis
   * (`?hdr=0&dv=0&sdr=1`) plus the Task 70 audio/quality axis
   * (`platform`, `max_channels`, `audio`, `max_bitrate`, `quality`).
   */
  stream(
    fileId: number,
    hints: StreamHints = {},
    opts: RequestOptions = {},
  ): Promise<StreamDecision> {
    const params = new URLSearchParams();
    // The backend reads these as "0" = false; only send the disabling hint.
    if (hints.hdr === false) params.set('hdr', '0');
    if (hints.dv === false) params.set('dv', '0');
    if (hints.sdr === true) params.set('sdr', '1');
    // Task 70 audio + quality hints (`docs/.tasks/70`).
    if (hints.platform) params.set('platform', hints.platform);
    if (hints.maxChannels != null) params.set('max_channels', String(hints.maxChannels));
    // `atmos` is sugar for the `eac3_joc` encoding token.
    const audio = [...(hints.audio ?? [])];
    if (hints.atmos && !audio.includes('eac3_joc')) audio.push('eac3_joc');
    if (audio.length) params.set('audio', audio.join(','));
    if (hints.maxBitrate != null) params.set('max_bitrate', String(hints.maxBitrate));
    if (hints.quality) params.set('quality', hints.quality);
    // Task 90 image-subtitle burn-in: send the selected track + the burn flag. A text
    // subtitle is never sent here — it is fetched as a `.vtt` sidecar via `subtitleUrl`.
    if (hints.subBurn && hints.sub != null) {
      params.set('sub', String(hints.sub));
      params.set('sub_burn', '1');
    }
    // Force a server transcode even when the file would direct-play — the web player's
    // fallback when a `direct` stream proved unplayable in the browser.
    if (hints.forceTranscode) params.set('force_transcode', '1');
    // Selected audio track (`docs/.tasks/97` Part C): the source `stream_index` the server
    // maps (`-map 0:a:<n>`). A distinct value yields a distinct transcode session.
    if (hints.audioTrack != null) params.set('audio_track', String(hints.audioTrack));
    const qs = params.toString();
    // Not cached: a stream decision may spin up a transcode session.
    return this.getJson<StreamDecision>(
      `/api/stream/${fileId}${qs ? `?${qs}` : ''}`,
      opts,
      /* cacheable */ false,
    );
  }

  /**
   * `GET /api/files/:file_id` — a file's audio + subtitle tracks (`docs/.tasks/97` Part C).
   * A deep link to `/play/:file_id` (with no router state) fetches this to populate the
   * player's audio-track and caption menus. Not ETag-cached — a tiny per-file read.
   */
  files(fileId: number, opts: RequestOptions = {}): Promise<FileTracks> {
    return this.getJson<FileTracks>(`/api/files/${fileId}`, opts, false);
  }

  /**
   * `GET /api/trickplay/:file_id/meta` — tiled-JPG mosaic geometry for scrub thumbnails.
   * Throws `ApiError` with `isNotFound` when the title has no croppable trickplay sheet
   * (BIF-only or none) — the caller treats that as "no thumbnails" and shows a plain bar.
   * Pair the geometry with `trickplayUrl(fileId, 'jpg')` for the mosaic image.
   */
  trickplayMeta(fileId: number, opts: RequestOptions = {}): Promise<TrickplayMetaResponse> {
    return this.getJson<TrickplayMetaResponse>(`/api/trickplay/${fileId}/meta`, opts, false);
  }

  // -- Metadata enrichment (Phase A, `docs/.tasks/60`) ---------------------

  /** `POST /api/movies/:id/refresh` — force re-enrichment of one movie. */
  refreshMovie(id: number, opts: RequestOptions = {}): Promise<RefreshResponse> {
    return this.sendJson<RefreshResponse>('POST', `/api/movies/${id}/refresh`, undefined, opts);
  }

  /**
   * `GET /api/movies/:id/matches?query=` — candidate provider matches to choose
   * from. `query` overrides the filename-parsed title (a corrected search term).
   */
  movieMatches(id: number, query?: string, opts: RequestOptions = {}): Promise<MatchesResponse> {
    const qs = query ? `?query=${encodeURIComponent(query)}` : '';
    return this.getJson<MatchesResponse>(`/api/movies/${id}/matches${qs}`, opts, false);
  }

  /** `POST /api/movies/:id/match` — pin a provider id and re-enrich against it. */
  matchMovie(id: number, providerId: string, opts: RequestOptions = {}): Promise<RefreshResponse> {
    return this.sendJson<RefreshResponse>(
      'POST',
      `/api/movies/${id}/match`,
      { provider_id: providerId },
      opts,
    );
  }

  // -- Libraries (Phase B, `docs/.tasks/60`) -------------------------------

  /** `GET /api/libraries` — all libraries with their folders. */
  libraries(opts: RequestOptions = {}): Promise<Library[]> {
    return this.getJson<Library[]>('/api/libraries', opts, false);
  }

  /** `POST /api/libraries` — create a library. Folders must be inside MEDIA_DIR. */
  createLibrary(body: CreateLibraryRequest, opts: RequestOptions = {}): Promise<Library> {
    return this.sendJson<Library>('POST', '/api/libraries', body, opts);
  }

  /** `PATCH /api/libraries/:id` — rename / add / remove folders. */
  patchLibrary(id: number, body: PatchLibraryRequest, opts: RequestOptions = {}): Promise<Library> {
    return this.sendJson<Library>('PATCH', `/api/libraries/${id}`, body, opts);
  }

  /** `DELETE /api/libraries/:id` — remove a library (cascades its titles). */
  async deleteLibrary(id: number, opts: RequestOptions = {}): Promise<void> {
    const res = await this.doFetch(this.abs(`/api/libraries/${id}`), {
      method: 'DELETE',
      signal: opts.signal,
    });
    if (!res.ok) throw await ApiError.fromResponse(res);
  }

  /** `POST /api/libraries/:id/scan` — trigger an immediate scan of one library. */
  async scanLibrary(id: number, opts: RequestOptions = {}): Promise<void> {
    const res = await this.doFetch(this.abs(`/api/libraries/${id}/scan`), {
      method: 'POST',
      signal: opts.signal,
    });
    if (!res.ok) throw await ApiError.fromResponse(res);
  }

  /**
   * `POST /api/metadata/backfill[?force=1]` — run a background pass over already-matched
   * titles, filling any newer detail fields / artwork (genres, collections, fanart logos +
   * wallpapers) that landed since they were matched. Returns immediately; `already_running`
   * is `true` when a backfill was already in flight. `force` re-fetches every matched title.
   * Throws a `501` `ApiError` when no metadata provider is configured.
   */
  backfillMetadata(force = false, opts: RequestOptions = {}): Promise<BackfillResponse> {
    const qs = force ? '?force=1' : '';
    return this.sendJson<BackfillResponse>('POST', `/api/metadata/backfill${qs}`, undefined, opts);
  }

  /**
   * `POST /api/metadata/enrich` — kick a background enrichment pass over pending/failed
   * titles (`docs/.tasks/96`), without waiting for the next scan or the periodic backstop.
   * Returns immediately (`202`); tallies land in `status()`. `501` when no provider is set.
   */
  enrichMetadata(opts: RequestOptions = {}): Promise<{ status: string }> {
    return this.sendJson<{ status: string }>('POST', '/api/metadata/enrich', undefined, opts);
  }

  // -- Status & observability (`docs/.tasks/96`) ---------------------------

  /** `GET /api/status` — enrichment/ingest status: counts, providers, last runs. */
  status(opts: RequestOptions = {}): Promise<SystemStatus> {
    // Not ETag-cached: status must be live.
    return this.getJson<SystemStatus>('/api/status', opts, false);
  }

  /** `GET /api/status/unmatched` — a keyset page of unmatched/failed titles. */
  unmatched(
    query: { kind?: 'movie' | 'series'; after?: number; limit?: number } = {},
    opts: RequestOptions = {},
  ): Promise<UnmatchedPage> {
    const params = new URLSearchParams();
    if (query.kind) params.set('kind', query.kind);
    if (query.after != null) params.set('after', String(query.after));
    if (query.limit != null) params.set('limit', String(query.limit));
    const qs = params.toString();
    return this.getJson<UnmatchedPage>(`/api/status/unmatched${qs ? `?${qs}` : ''}`, opts, false);
  }

  /** `GET /api/status/probe-failures` — a keyset page of ffprobe-failed files. */
  probeFailures(
    query: { after?: number; limit?: number } = {},
    opts: RequestOptions = {},
  ): Promise<ProbeFailuresPage> {
    const params = new URLSearchParams();
    if (query.after != null) params.set('after', String(query.after));
    if (query.limit != null) params.set('limit', String(query.limit));
    const qs = params.toString();
    return this.getJson<ProbeFailuresPage>(
      `/api/status/probe-failures${qs ? `?${qs}` : ''}`,
      opts,
      false,
    );
  }

  /** `GET /api/health` — liveness probe. Resolves to `true` on `200`. */
  async health(opts: RequestOptions = {}): Promise<boolean> {
    const res = await this.doFetch(this.abs('/api/health'), {
      method: 'GET',
      signal: opts.signal,
    });
    return res.ok;
  }

  // -- Asset URL builders (consumed by native <Image>/<Video>) --------------

  /** Absolute URL for a poster/backdrop. Accepts a stored path or a full `/api/images/...`. */
  imageUrl(pathOrUrl: string | null | undefined): string | undefined {
    if (!pathOrUrl) return undefined;
    if (pathOrUrl.startsWith('/api/images/')) return this.abs(pathOrUrl);
    return this.abs(`/api/images/${pathOrUrl.replace(/^\/+/, '')}`);
  }

  /** `GET /api/preview/:file_id` — 720p silent hover clip (mp4). */
  previewUrl(fileId: number): string {
    // Backend serves via filename convention `<file_id>.mp4` (see api-phasing).
    return this.abs(`/api/preview/${fileId}.mp4`);
  }

  /** `GET /api/trickplay/:file_id` — trickplay sprite (BIF or tiled JPG). */
  trickplayUrl(fileId: number, ext: 'bif' | 'jpg' = 'bif'): string {
    return this.abs(`/api/trickplay/${fileId}.${ext}`);
  }

  /** `GET /api/direct/:file_id` — direct-play byte-range source stream. */
  directUrl(fileId: number): string {
    return this.abs(`/api/direct/${fileId}`);
  }

  /**
   * `GET /api/subtitles/:file_id/:index.vtt` — a **text** subtitle track as WebVTT
   * (`docs/.tasks/90`). `index` is the embedded `stream_index`, or `ext<id>` for an
   * external sidecar (its `subtitle_streams` row id). Use as a react-native-video
   * `textTracks` URI so a direct-played file still shows subtitles without a transcode.
   * Image tracks are not served here — request a burn-in via `stream(..., { sub, subBurn })`.
   */
  subtitleUrl(fileId: number, index: number | string): string {
    return this.abs(`/api/subtitles/${fileId}/${index}.vtt`);
  }

  /**
   * Resolve an HLS/`index.m3u8` URL from a `StreamDecision`. The backend returns
   * a root-relative `url`; make it absolute for the native player.
   */
  hlsUrl(decision: StreamDecision): string {
    return this.abs(decision.url);
  }

  /** Turn a root-relative API path into an absolute URL against `baseUrl`. */
  abs(path: string): string {
    if (/^https?:\/\//i.test(path)) return path;
    return `${this.baseUrl}${path.startsWith('/') ? '' : '/'}${path}`;
  }

  // -- Internals -----------------------------------------------------------

  private async getJson<T>(
    path: string,
    opts: RequestOptions,
    cacheable = true,
  ): Promise<T> {
    const url = this.abs(path);
    const useCache = cacheable && this.etagCache;
    const cached = useCache ? this.cache.get(path) : undefined;

    const headers: Record<string, string> = { Accept: 'application/json' };
    if (cached) headers['If-None-Match'] = cached.etag;

    const res = await this.doFetch(url, {
      method: 'GET',
      headers,
      signal: opts.signal,
    });

    // 304: the body is unchanged — replay what we parsed last time.
    if (res.status === 304 && cached) {
      return cached.body as T;
    }

    if (!res.ok) {
      throw await ApiError.fromResponse(res);
    }

    const body = (await res.json()) as T;

    if (useCache) {
      const etag = res.headers.get('ETag');
      if (etag) this.cache.set(path, { etag, body });
    }
    return body;
  }

  /**
   * Send a mutating request (`POST`/`PATCH`) with an optional JSON body and parse
   * the JSON response. Never ETag-cached — these are writes. A write also drops the
   * local catalog cache, since the backend invalidates its own after enrichment /
   * library mutations and stale detail bodies would otherwise replay.
   */
  private async sendJson<T>(
    method: 'POST' | 'PATCH',
    path: string,
    body: unknown,
    opts: RequestOptions,
  ): Promise<T> {
    const headers: Record<string, string> = { Accept: 'application/json' };
    if (body !== undefined) headers['Content-Type'] = 'application/json';
    const res = await this.doFetch(this.abs(path), {
      method,
      headers,
      body: body !== undefined ? JSON.stringify(body) : undefined,
      signal: opts.signal,
    });
    if (!res.ok) throw await ApiError.fromResponse(res);
    this.clearCache();
    return (await res.json()) as T;
  }

  /** Drop all cached ETags/bodies (e.g. after a known ingest write). */
  clearCache(): void {
    this.cache.clear();
  }
}

export type { ApiErrorBody };
