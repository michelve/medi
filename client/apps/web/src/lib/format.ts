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

/**
 * A short marketing resolution label (`4K` / `HD` / `SD` / …). Marketing classes are defined
 * by the **horizontal** size (3840=4K, 2560=1440p, 1920=HD, ≤1280=SD), so letterboxed scope
 * content (e.g. 1920×800) still reads as HD. Falls back to height when width is unknown.
 *
 * Consumer-facing wording: 1080p is shown as **HD**, and 720p and below as **SD** (per the
 * common streaming convention where "HD" means 1080). 4K and 1440p keep their technical
 * labels; HDR/DV is a *separate* badge, so a 4K HDR title reads as "4K" + "HDR" side by side.
 */
export function resolutionLabel(
  width: number | null | undefined,
  height?: number | null | undefined,
): string | undefined {
  if (width != null) {
    if (width >= 3200) return '4K';
    if (width >= 2200) return '1440p';
    if (width >= 1700) return 'HD'; // 1080p
    if (width >= 1100) return 'SD'; // 720p
    if (width >= 900) return 'SD'; // 576p
    return 'SD';
  }
  if (height == null) return undefined;
  if (height >= 2000) return '4K';
  if (height >= 1400) return '1440p';
  if (height >= 1000) return 'HD'; // 1080p
  if (height >= 700) return 'SD'; // 720p
  if (height >= 500) return 'SD'; // 576p
  return 'SD';
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

/** Common ISO-639-2/B (three-letter) codes ffprobe emits that `Intl.DisplayNames` misses. */
const LANG_FALLBACK: Record<string, string> = {
  eng: 'English',
  spa: 'Spanish',
  fre: 'French',
  fra: 'French',
  ger: 'German',
  deu: 'German',
  ita: 'Italian',
  por: 'Portuguese',
  jpn: 'Japanese',
  kor: 'Korean',
  chi: 'Chinese',
  zho: 'Chinese',
  rus: 'Russian',
  hin: 'Hindi',
  ara: 'Arabic',
  dut: 'Dutch',
  nld: 'Dutch',
  swe: 'Swedish',
  nor: 'Norwegian',
  dan: 'Danish',
  fin: 'Finnish',
  pol: 'Polish',
  tur: 'Turkish',
  und: 'Undetermined',
};

/** A display language name from an ISO code (`eng` → `English`), or the raw code as-is. */
export function languageName(code: string | null | undefined): string | undefined {
  if (!code) return undefined;
  const lc = code.toLowerCase();
  if (LANG_FALLBACK[lc]) return LANG_FALLBACK[lc];
  try {
    const dn = new Intl.DisplayNames([navigator.language || 'en'], { type: 'language' });
    const name = dn.of(lc);
    if (name && name.toLowerCase() !== lc) return name;
  } catch {
    // Intl.DisplayNames unavailable or threw on an odd code — fall through.
  }
  return code.toUpperCase();
}

/** A channel-count label, e.g. 6 → `5.1`, 8 → `7.1`, 2 → `Stereo`, 1 → `Mono`. */
export function channelLabel(channels: number | null | undefined): string | undefined {
  if (channels == null || channels <= 0) return undefined;
  switch (channels) {
    case 1:
      return 'Mono';
    case 2:
      return 'Stereo';
    case 6:
      return '5.1';
    case 8:
      return '7.1';
    default:
      return `${channels}ch`;
  }
}

/** Human runtime from a millisecond duration, e.g. `2h 43m` / `54m`. Undefined when unknown. */
export function formatRuntime(ms: number | null | undefined): string | undefined {
  if (ms == null || ms <= 0) return undefined;
  const totalMin = Math.round(ms / 60000);
  const h = Math.floor(totalMin / 60);
  const m = totalMin % 60;
  if (h > 0) return m > 0 ? `${h}h ${m}m` : `${h}h`;
  return `${m}m`;
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
