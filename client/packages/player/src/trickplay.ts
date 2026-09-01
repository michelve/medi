/**
 * Trickplay scrub-thumbnail geometry (Phase 5, `docs/.tasks/50` Part A sub-task 3).
 *
 * The backend samples one frame every `interval_ms` and packs them either as a
 * Roku **BIF** binary or a **tiled-JPG** mosaic (`docs/.tasks/30` §Trickplay).
 * On a TV client the tiled-JPG mosaic is the only practical form to render — we
 * crop one cell out of a single `<Image>` — so this module models that grid and
 * maps a playback position to the tile that covers it.
 *
 * BIF is deliberately unsupported on the client: parsing its binary index and
 * slicing embedded JPEGs on-device is heavy and pointless when the server can
 * emit a mosaic. `@medi/player` therefore asks for the `jpg` variant.
 *
 * ## The metadata gap
 * To crop a tile the client needs the grid dims (`tile_w/h`, `cols`, `rows`) and
 * `interval_ms`. Those live in the backend `trickplay_assets` row but are NOT yet
 * served — `/api/trickplay/:file_id` is a bare static `ServeDir` over the image.
 * The API contract (`docs/.tasks/02` line 40) already promises "sprite + metadata";
 * a small `GET /api/trickplay/:file_id/meta` (or an `X-Trickplay-*` header set)
 * closes it. `TrickplayMeta` is the exact shape that endpoint should return, so
 * the player is complete the moment the backend serves it. See the package
 * README for the endpoint spec.
 */

/**
 * Grid geometry for a title's tiled-JPG trickplay sheet. Mirrors the
 * `trickplay_assets` row (`medi_db::models::TrickplayAsset`) for the `jpg` kind.
 */
export interface TrickplayMeta {
  /** Absolute URL of the mosaic image (`ApiClient.trickplayUrl(id, 'jpg')`). */
  url: string;
  /** Milliseconds between sampled frames (e.g. 10000 = one tile per 10s). */
  intervalMs: number;
  /** Width of a single thumbnail cell, px. */
  tileW: number;
  /** Height of a single thumbnail cell, px. */
  tileH: number;
  /** Columns in the mosaic. */
  cols: number;
  /** Rows in the mosaic. */
  rows: number;
}

/** A resolved crop into the mosaic for a given playback position. */
export interface TrickplayTile {
  /** Zero-based frame index that covers the position. */
  index: number;
  /** Pixel offset of the tile's left edge within the mosaic. */
  x: number;
  /** Pixel offset of the tile's top edge within the mosaic. */
  y: number;
  /** Tile width (= `meta.tileW`), px. */
  width: number;
  /** Tile height (= `meta.tileH`), px. */
  height: number;
}

/** Total frames the mosaic can address (`cols * rows`). */
export function tileCount(meta: TrickplayMeta): number {
  return Math.max(0, meta.cols * meta.rows);
}

/**
 * Map a playback position (ms) to the mosaic tile that covers it.
 *
 * The frame index is `floor(positionMs / intervalMs)`, clamped to the last
 * available tile (a slightly-past-end scrub still shows the final thumbnail
 * rather than nothing). Returns `null` only for a degenerate grid (no tiles or a
 * non-positive interval), so callers can fall back to a plain scrub bar.
 */
export function tileForPosition(
  meta: TrickplayMeta,
  positionMs: number,
): TrickplayTile | null {
  const count = tileCount(meta);
  if (count === 0 || meta.intervalMs <= 0 || meta.cols <= 0) return null;

  const raw = Math.floor(Math.max(0, positionMs) / meta.intervalMs);
  const index = Math.min(raw, count - 1);

  const col = index % meta.cols;
  const row = Math.floor(index / meta.cols);

  return {
    index,
    x: col * meta.tileW,
    y: row * meta.tileH,
    width: meta.tileW,
    height: meta.tileH,
  };
}
