/**
 * Best-file selection (Task 91, multi-resolution grouping).
 *
 * A movie/episode can carry several `media_files` at different resolutions (e.g. a 1080p and
 * a 4K rip of one title). The backend already returns them best-first, but the client ranks
 * independently too so any source (unordered fixtures, a future endpoint) yields the same
 * default and the ranking rule lives in one place.
 *
 * Ranking: resolution (height) → HDR tier (DV > HDR10+ > HDR10 > HLG > SDR) → bitrate. This
 * prefers a 4K copy even when a lower-res copy has a fancier HDR format. Mirrors the backend
 * `media_file_best_order` SQL.
 */

import type { MediaFile, HdrTier } from '@medi/api-client';

/** HDR tiers ranked strongest-first; anything absent (SDR / unknown) is 0. */
export const HDR_RANK: Record<string, number> = {
  dolbyvision: 4,
  hdr10plus: 3,
  hdr10: 2,
  hlg: 1,
};

/** The rank of an HDR tier (0 for SDR / null / unknown). */
export function hdrRank(hdr: HdrTier | null | undefined): number {
  return hdr ? (HDR_RANK[hdr] ?? 0) : 0;
}

/** Comparator sorting files best-first (resolution → HDR → bitrate). */
export function compareFilesBest(a: MediaFile, b: MediaFile): number {
  return (
    (b.height ?? 0) - (a.height ?? 0) ||
    hdrRank(b.hdr_type) - hdrRank(a.hdr_type) ||
    (b.bitrate ?? 0) - (a.bitrate ?? 0)
  );
}

/** The best file to play from a title's files, or `undefined` when there are none. */
export function pickBestFile(files: MediaFile[]): MediaFile | undefined {
  if (files.length === 0) return undefined;
  // Copy before sort so we never mutate the caller's array (React state).
  return [...files].sort(compareFilesBest)[0];
}
