/**
 * Chapter helpers shared by the web and TV players (`docs/.tasks/99` Part B / C6).
 *
 * Pure functions over the `FileChapter[]` list from `GET /api/files/:id`. No React / RN deps,
 * so both `@medi/player` (TV) and the web app import them directly (the web app aliases this
 * submodule the same way it aliases `trickplay`).
 */

import type { FileChapter } from '@medi/api-client';

/** How far back a "previous chapter" press first rewinds within the current chapter before it
 * counts as jumping to the actual previous chapter (Jellyfin's grace window). */
export const PREVIOUS_CHAPTER_GRACE_MS = 10_000;

/** The chapter covering `positionMs` — the last chapter whose `start_ms <= positionMs`, or
 * `null` before the first chapter / when there are none. Assumes ordinal order. */
export function chapterAt(chapters: FileChapter[], positionMs: number): FileChapter | null {
  let current: FileChapter | null = null;
  for (const c of chapters) {
    if (c.start_ms <= positionMs) current = c;
    else break;
  }
  return current;
}

/** The start (ms) of the next chapter after `positionMs`, or `null` if none remain. */
export function nextChapterMs(chapters: FileChapter[], positionMs: number): number | null {
  const next = chapters.find((c) => c.start_ms > positionMs);
  return next ? next.start_ms : null;
}

/** The seek target (ms) for a "previous chapter" press: within `PREVIOUS_CHAPTER_GRACE_MS` of
 * the current chapter's start it goes to the previous chapter; otherwise it restarts the
 * current one. `null` only when there are no chapters. */
export function previousChapterMs(chapters: FileChapter[], positionMs: number): number | null {
  const first = chapters[0];
  if (!first) return null;
  const adjusted = positionMs - PREVIOUS_CHAPTER_GRACE_MS;
  let target = first.start_ms;
  for (const c of chapters) {
    if (c.start_ms <= adjusted) target = c.start_ms;
    else break;
  }
  return target;
}
