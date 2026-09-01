# @medi/player

Full-length TV playback for the medi client — **Phase 5**, Part A
(`docs/.tasks/50-phase5-playback-packaging.md`).

Phase 4 shipped only the silent hover *preview* (`@medi/ui` `HoverPreview`). This
package is the deferred full player.

## What's here

| Export | Role |
|---|---|
| `VideoScreen` | Mounts `react-native-video` (AVPlayer on tvOS / ExoPlayer on Android TV), resolves `/api/stream/:file_id`, drives the overlay. |
| `PlayerOverlay` | Presentational transport (title, play/pause glyph, scrub bar, trickplay thumb). **No focusable nodes**, `pointerEvents="none"`. |
| `usePlayerControls` | The overlay/transport state machine. Consumes raw remote events; holds play/pause + scrub intent; auto-hides the overlay. |
| `tileForPosition` / `TrickplayMeta` | Map a playback position to a cell of the tiled-JPG trickplay mosaic. |

## The design rule (why the overlay bypasses spatial navigation)

On Android TV, absolutely-positioned Touchable/Pressable controls layered over
the video surface break D-pad routing and cause render lag (README §Video
Playback and Overlay Integration). So the overlay owns **no** focusable nodes and
does **not** participate in `react-tv-space-navigation`. Instead `VideoScreen`
intercepts raw remote events globally with **`useTVEventHandler`** and turns each
into a `usePlayerControls` action:

```
select | playPause  → toggle play/pause
left | right         → scrub (accumulate offset, commit after a settle)
up | down            → reveal overlay
menu                 → left to the host app's global back handler (avoids double-pop)
```

## Direct-play vs HLS

Both are handled uniformly: the injected `resolveStream` returns an absolute
`uri` plus `isHls`, and `VideoScreen` sets `type: 'm3u8'` only for HLS. A `409`
from the stream decision (transcode-session cap, `ApiError.isBusy`) is surfaced as
a non-fatal notice — no crash, no blank screen.

## Trickplay metadata (scrub thumbnails)

Scrub thumbnails need the tiled-JPG **grid geometry** — `interval_ms`, `tile_w`,
`tile_h`, `cols`, `rows` — to crop the right cell.

**Backend support is now implemented** (Phase 5):

```
GET /api/trickplay/:file_id/meta  →  200
{ "file_id": 88, "kind": "tiled_jpg",
  "interval_ms": 10000, "tile_w": 320, "tile_h": 180, "cols": 10, "rows": 6 }
```

- Served from the `trickplay_assets` row by `medi_api` (`queries::get_trickplay_asset`).
- Returns `404` when the asset is absent **or** is a BIF (no client-croppable grid);
  `PlayerScreen.resolveTrickplay` treats that as "no thumbnails" and the player
  falls back to a plain scrub bar (playback + seeking still work).
- The sprite image itself is still the static `/api/trickplay/<file_id>.jpg`.

This client renders the **tiled-JPG** variant only — BIF is a binary index that is
impractical to parse on-device. The asset worker's default is therefore now
`TrickplayKind::TiledJpg` (`medi_assets::AssetWorkerConfig::new`), and it records the
exact aspect-derived `tile_h` (probed from the finished mosaic) so rows crop cleanly.
