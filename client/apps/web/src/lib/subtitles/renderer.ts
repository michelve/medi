/**
 * Client-side subtitle render dispatcher (`docs/.tasks/99` C1) — mirrors jellyfin-web's
 * `renderTracksEvents`. Keyed on the track's codec:
 *
 *   - `ass` / `ssa`            → libass-wasm (SubtitlesOctopus): full ASS styling at native res
 *   - `pgssub` / vobsub        → libbitsub (Phase 5) — for now signals `unsupported` so the
 *                                caller falls back to server burn-in
 *   - plain text (srt/vtt/…)   → handled by the native `<track>` path in VideoPlayer, NOT here
 *
 * The heavy WASM libraries are dynamic-imported so they stay out of the browse bundle (like
 * hls.js). A renderer owns an overlay canvas over the `<video>`; `destroy()` tears it down.
 */

import type { ApiClient, FileSubtitleTrack } from '@medi/api-client';

/** Static asset URLs for the libass worker/wasm/fallback font, served from `public/libass/`. */
const LIBASS_WORKER_URL = '/libass/subtitles-octopus-worker.js';
const LIBASS_LEGACY_WORKER_URL = '/libass/subtitles-octopus-worker-legacy.js';
const LIBASS_FALLBACK_FONT_URL = '/libass/default.woff2';

/** Codecs libass renders (ASS/SSA). */
const ASS_CODECS = new Set(['ass', 'ssa']);
/** PGS image-subtitle codecs — rendered client-side by libbitsub from a single `.sup`. */
const PGS_CODECS = new Set(['pgssub', 'hdmv_pgs_subtitle', 'pgs']);
/** VobSub/DVD image codecs — need a `.idx`+`.sub` pair (backend raw pair not wired yet), so
 * these fall back to server burn-in for now. */
const VOBSUB_CODECS = new Set(['dvdsub', 'dvd_subtitle', 'vobsub']);
/** All bitmap image-subtitle codecs. */
const IMAGE_CODECS = new Set([...PGS_CODECS, ...VOBSUB_CODECS]);

/** Whether a track is handled by this client-side dispatcher (vs the native `<track>` path). */
export function usesClientRenderer(track: FileSubtitleTrack): boolean {
  const codec = (track.codec ?? '').toLowerCase();
  return ASS_CODECS.has(codec) || IMAGE_CODECS.has(codec) || track.format === 'image';
}

export interface SubtitleRenderHandle {
  /** Shift all subtitle timings by `seconds` (sync/offset). */
  setOffset(seconds: number): void;
  /** Tear down the renderer (worker, canvas). Safe to call once. */
  destroy(): void;
}

export interface CreateRendererOptions {
  api: ApiClient;
  fileId: number;
  video: HTMLVideoElement;
  track: FileSubtitleTrack;
  /** A stable id for the track (`ext<id>` or `stream_index`), for the `/raw` URL. */
  trackId: string;
  /** Initial sync offset in seconds. */
  offsetSeconds?: number;
  /** Video frame rate, for libass animation timing (`targetFps`). Falls back to 24. */
  videoFps?: number;
  /** Called when this renderer can't handle the track (e.g. image sub before Phase 5, or a
   * fatal WASM error) so the caller can fall back to server burn-in. */
  onUnsupported?: (reason: string) => void;
}

/**
 * Create a client-side renderer for `track`, or return `null` when the track should be handled
 * by the native `<track>` path (plain text). For image tracks (and any WASM failure) the
 * `onUnsupported` callback fires and the returned handle is a no-op, so the caller falls back
 * to burn-in.
 */
export async function createSubtitleRenderer(
  opts: CreateRendererOptions,
): Promise<SubtitleRenderHandle | null> {
  const codec = (opts.track.codec ?? '').toLowerCase();

  if (ASS_CODECS.has(codec)) {
    return createAssRenderer(opts);
  }

  if (PGS_CODECS.has(codec)) {
    return createImageRenderer(opts, 'pgs');
  }

  if (VOBSUB_CODECS.has(codec) || opts.track.format === 'image') {
    return createImageRenderer(opts, 'vobsub');
  }

  // Plain text → native <track> path owns it; nothing to render here.
  return null;
}

/** libbitsub renderer for image subtitles — PGS (single `.sup`) or VobSub (`.sub`+`.idx`).
 * Client-side, zero transcode; any failure calls back to burn-in. */
async function createImageRenderer(
  opts: CreateRendererOptions,
  kind: 'pgs' | 'vobsub',
): Promise<SubtitleRenderHandle> {
  const { createAutoSubtitleRenderer } = await import('libbitsub');

  let renderer: { timeOffset: number; dispose: () => void } | null = null;
  try {
    const onError = () => {
      try {
        renderer?.dispose();
      } catch {
        /* already gone */
      }
      renderer = null;
      opts.onUnsupported?.(`libbitsub-render-error:${kind}`);
    };
    renderer =
      kind === 'vobsub'
        ? createAutoSubtitleRenderer({
            video: opts.video,
            subUrl: opts.api.rawSubtitleUrl(opts.fileId, opts.trackId), // `.sub`
            idxUrl: opts.api.rawSubtitleIdxUrl(opts.fileId, opts.trackId), // `.idx`
            fileName: 'subtitle.idx',
            onError,
          })
        : createAutoSubtitleRenderer({
            video: opts.video,
            subUrl: opts.api.rawSubtitleUrl(opts.fileId, opts.trackId), // `.sup`
            fileName: 'subtitle.sup',
            onError,
          });
    if (opts.offsetSeconds) renderer.timeOffset = opts.offsetSeconds;
  } catch (err) {
    // WASM/init failure → burn-in.
    opts.onUnsupported?.(`libbitsub-init-failed:${String(err)}`);
    return noopHandle();
  }

  return {
    setOffset(seconds: number) {
      if (renderer) renderer.timeOffset = seconds;
    },
    destroy() {
      try {
        renderer?.dispose();
      } catch {
        /* already disposed */
      }
      renderer = null;
    },
  };
}

/** libass-wasm (SubtitlesOctopus) renderer for ASS/SSA. */
async function createAssRenderer(opts: CreateRendererOptions): Promise<SubtitleRenderHandle> {
  const { default: SubtitlesOctopus } = await import('@jellyfin/libass-wasm');

  // Embedded fonts (best-effort) so libass uses the file's real faces; failure → fallback font.
  let fonts: string[] = [];
  try {
    const res = await opts.api.fonts(opts.fileId);
    fonts = res.fonts.map((name) => opts.api.fontUrl(opts.fileId, name));
  } catch {
    // No fonts endpoint / none embedded — libass uses the fallback font below.
  }

  // Real frame rate (from ffprobe, via GET /api/files/:id) drives libass animation timing;
  // fall back to 24 when unknown (old files probed before the frame_rate column).
  const videoFps = opts.videoFps && opts.videoFps > 0 ? opts.videoFps : 24;

  let instance: InstanceType<typeof SubtitlesOctopus> | null = new SubtitlesOctopus({
    video: opts.video,
    subUrl: opts.api.rawSubtitleUrl(opts.fileId, opts.trackId),
    fonts,
    fallbackFont: LIBASS_FALLBACK_FONT_URL,
    workerUrl: LIBASS_WORKER_URL,
    legacyWorkerUrl: LIBASS_LEGACY_WORKER_URL,
    timeOffset: opts.offsetSeconds ?? 0,
    renderMode: 'wasm-blend',
    targetFps: videoFps,
    prescaleFactor: 0.8,
    prescaleHeightLimit: 1080,
    maxRenderHeight: 2160,
    onError: () => {
      // A fatal libass error → dispose and fall back to burn-in.
      try {
        instance?.dispose();
      } catch {
        /* already gone */
      }
      instance = null;
      opts.onUnsupported?.('libass-render-error');
    },
  });

  return {
    setOffset(seconds: number) {
      if (instance) instance.timeOffset = seconds;
    },
    destroy() {
      try {
        instance?.dispose();
      } catch {
        /* already disposed */
      }
      instance = null;
    },
  };
}

/** A do-nothing handle for the fall-back-to-burn-in case. */
function noopHandle(): SubtitleRenderHandle {
  return { setOffset() {}, destroy() {} };
}
