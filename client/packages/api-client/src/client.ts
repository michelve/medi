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
  CreateLibraryRequest,
  Library,
  LibraryPage,
  LibrarySort,
  MatchesResponse,
  MovieDetail,
  PatchLibraryRequest,
  RefreshResponse,
  SeriesDetail,
  StreamDecision,
  StreamHints,
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

  /** `GET /api/movies/:id` — full movie detail. */
  movie(id: number, opts: RequestOptions = {}): Promise<MovieDetail> {
    return this.getJson<MovieDetail>(`/api/movies/${id}`, opts);
  }

  /** `GET /api/series/:id` — full series detail. */
  series(id: number, opts: RequestOptions = {}): Promise<SeriesDetail> {
    return this.getJson<SeriesDetail>(`/api/series/${id}`, opts);
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
    const qs = params.toString();
    // Not cached: a stream decision may spin up a transcode session.
    return this.getJson<StreamDecision>(
      `/api/stream/${fileId}${qs ? `?${qs}` : ''}`,
      opts,
      /* cacheable */ false,
    );
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
