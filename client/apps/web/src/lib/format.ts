/**
 * Presentation formatters (Task 81) — pure, framework-free helpers shared across
 * the detail components. Kept out of the components so the display logic is testable
 * and swappable independently of the DOM.
 */

/** Human byte size, e.g. `12.3 GB`. Returns undefined for null/unknown sizes. */
export function formatBytes(bytes: number | null | undefined): string | undefined {
  if (bytes == null || bytes < 0) return undefined;
  if (bytes === 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const exp = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** exp;
  // Whole numbers for bytes; one decimal for larger units.
  return `${exp === 0 ? value : value.toFixed(1)} ${units[exp]}`;
}

/** `1920×1080` from width/height, or a common label when it maps to one. */
export function formatResolution(
  width: number | null | undefined,
  height: number | null | undefined,
): string | undefined {
  if (width == null || height == null) return undefined;
  return `${width}×${height}`;
}

/** A short marketing resolution label (`4K` / `1080p` / …) from the vertical size. */
export function resolutionLabel(height: number | null | undefined): string | undefined {
  if (height == null) return undefined;
  if (height >= 2000) return '4K';
  if (height >= 1400) return '1440p';
  if (height >= 1000) return '1080p';
  if (height >= 700) return '720p';
  if (height >= 500) return '576p';
  return `${height}p`;
}

/** Upper-cased codec/container token, or undefined when absent. */
export function formatToken(token: string | null | undefined): string | undefined {
  if (!token) return undefined;
  return token.toUpperCase();
}

/** A file's display basename from its full path. */
export function basename(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/** `h:mm:ss` / `m:ss` clock from a millisecond position. */
export function formatTime(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const mm = h > 0 ? String(m).padStart(2, '0') : String(m);
  const ss = String(s).padStart(2, '0');
  return h > 0 ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}
