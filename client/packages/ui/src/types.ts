/**
 * UI-layer view models. These are derived from `@medi/api-client` responses by
 * the app screens and handed to the presentational components, keeping the UI
 * package decoupled from wire shapes.
 */

import type { HdrTier, LibraryKind } from '@medi/api-client';

/** A poster tile in a grid or carousel. */
export interface PosterItem {
  /** `movie` / `series` — decides which detail route select opens. */
  kind: LibraryKind;
  /** Title id (movie id or series id). */
  id: number;
  title: string;
  year?: number;
  /** Poster image path/URL as returned by the API (may be absolutized by the app). */
  poster?: string;
  hdr?: HdrTier;
  /**
   * The media_file id used for the hover preview clip. For a movie this is its
   * primary file; for a series, a representative episode's file. The unified
   * `/api/library` card does NOT carry a file id — only the detail responses do —
   * so this is `undefined` in the browse grid, and a poster with no
   * `previewFileId` simply shows its still image with no hover preview. Detail
   * screens, which have the media_files, supply it.
   */
  previewFileId?: number;
}
