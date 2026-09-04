/**
 * Minimal type declaration for `@jellyfin/libass-wasm` (SubtitlesOctopus) — the package ships
 * no `.d.ts`. Only the surface we use (`docs/.tasks/99`). See the upstream README for the full
 * option set.
 */
declare module '@jellyfin/libass-wasm' {
  export interface SubtitlesOctopusOptions {
    /** The `<video>` the overlay canvas tracks. */
    video: HTMLVideoElement;
    /** URL of the subtitle file (ASS/SSA) to render. */
    subUrl?: string;
    /** Font attachment URLs to make available to libass. */
    fonts?: string[];
    /** Fallback font URL used when a referenced font is missing. */
    fallbackFont?: string;
    /** URL of the wasm worker script (served asset). */
    workerUrl?: string;
    /** URL of the legacy (asm.js) worker for browsers without wasm. */
    legacyWorkerUrl?: string;
    /** Seconds to shift all subtitle timings (sync offset). */
    timeOffset?: number;
    /** Render mode; `wasm-blend` is the modern default. */
    renderMode?: 'wasm-blend' | 'lossy' | 'blend';
    dropAllAnimations?: boolean;
    libassMemoryLimit?: number;
    libassGlyphLimit?: number;
    targetFps?: number;
    prescaleFactor?: number;
    prescaleHeightLimit?: number;
    maxRenderHeight?: number;
    /** Called on a fatal render error. */
    onError?: (err: unknown) => void;
    onReady?: () => void;
  }

  export default class SubtitlesOctopus {
    constructor(options: SubtitlesOctopusOptions);
    /** Shift all timings by `seconds` (subtitle sync). */
    timeOffset: number;
    /** Replace the subtitle track by URL. */
    setTrackByUrl(url: string): void;
    /** Free the worker + canvas. */
    dispose(): void;
  }
}
