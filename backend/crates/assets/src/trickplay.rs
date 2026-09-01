//! Trickplay scrub sprites (`docs/.tasks/30` §Trickplay sprites, sub-task 3).
//!
//! Samples one small frame every `interval_ms` from the source and packs them into one
//! of two formats for the client's timeline scrubber:
//!
//! - **BIF** (Roku Base Index Frames) — the default. A single binary file: an 8-byte
//!   magic, a header carrying the frame count and a timestamp multiplier, an index of
//!   `(timestamp, byte-offset)` pairs, then every JPEG concatenated. The client seeks
//!   the index by scrub position and reads the one JPEG it needs.
//! - **tiled JPG** — a single JPEG mosaic of `cols`×`rows` thumbnails plus grid
//!   metadata (`tile_w/h`, `cols`, `rows`). The client computes the tile from scrub
//!   position and crops it out.
//!
//! Both start from the same step: extract evenly-spaced small JPEGs with one ffmpeg
//! pass (`fps=1/interval`, `scale`). BIF then packs them in-process; tiled-JPG asks
//! ffmpeg's `tile` filter to lay them into a mosaic.
//!
//! ## BIF binary layout (Roku spec)
//! ```text
//! magic          8 bytes  0x89 'B' 'I' 'F' 0x0d 0x0a 0x1a 0x0a
//! version        u32-le   0
//! image count    u32-le   N
//! ts multiplier  u32-le   milliseconds per timestamp unit (= interval_ms)
//! reserved       44 bytes 0
//! index          (N+1) * 8 bytes:
//!                  per entry: frame timestamp (u32-le), absolute file offset (u32-le)
//!                  the final sentinel entry is (0xFFFFFFFF, offset-past-last-image)
//! images         concatenated JPEG bytes
//! ```

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

use medi_db::writes::{TrickplayGrid, TrickplayKind};
use medi_transcode::caps::ffmpeg_bin;

/// Default sampling interval — one sprite frame every 10 seconds.
pub const DEFAULT_INTERVAL_MS: i64 = 10_000;
/// Sprite thumbnail width in pixels (height keeps aspect). Small: it is a scrub preview.
const THUMB_WIDTH: u32 = 320;
/// Columns in a tiled-JPG mosaic. Rows follow from the frame count.
const TILE_COLS: u32 = 10;

/// The 8-byte BIF magic (`0x89 BIF \r \n 0x1a \n`), mirroring the PNG-style signature.
const BIF_MAGIC: [u8; 8] = [0x89, 0x42, 0x49, 0x46, 0x0d, 0x0a, 0x1a, 0x0a];
/// Sentinel timestamp terminating the BIF index.
const BIF_INDEX_SENTINEL: u32 = 0xFFFF_FFFF;

/// Errors from generating trickplay sprites.
#[derive(Debug, thiserror::Error)]
pub enum TrickplayError {
    #[error("failed to spawn ffmpeg: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("ffmpeg exited with status {status}: {stderr}")]
    NonZeroExit { status: String, stderr: String },
    #[error("no frames were sampled from the source")]
    NoFrames,
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

/// The result of a successful trickplay generation: the on-disk path plus the metadata
/// the `trickplay_assets` row records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrickplayOutput {
    pub kind: TrickplayKind,
    pub path: PathBuf,
    pub interval_ms: i64,
    /// Grid geometry for a tiled-JPG sheet; `None` for a BIF.
    pub grid: Option<TrickplayGrid>,
}

/// Where a title's trickplay file is written. BIF → `<id>.bif`, tiled → `<id>.jpg`.
/// Stable across regenerations so the `/api/trickplay/<file_id>.<ext>` URL is stable.
pub fn trickplay_path(trickplay_dir: &Path, media_file_id: i64, kind: TrickplayKind) -> PathBuf {
    let ext = match kind {
        TrickplayKind::Bif => "bif",
        TrickplayKind::TiledJpg => "jpg",
    };
    trickplay_dir.join(format!("{media_file_id}.{ext}"))
}

/// Generate trickplay sprites for `input` in the requested `kind`, sampling one frame
/// every `interval_ms`. Returns the output path + metadata for the DB row.
///
/// Frames are first extracted to a temp directory (auto-cleaned) as numbered JPEGs;
/// BIF packs them in-process, tiled-JPG re-runs ffmpeg's `tile` filter into a mosaic.
pub async fn generate(
    input: &Path,
    trickplay_dir: &Path,
    media_file_id: i64,
    interval_ms: i64,
    kind: TrickplayKind,
) -> Result<TrickplayOutput, TrickplayError> {
    std::fs::create_dir_all(trickplay_dir)?;
    let interval_ms = interval_ms.max(1000);

    match kind {
        TrickplayKind::Bif => generate_bif(input, trickplay_dir, media_file_id, interval_ms).await,
        TrickplayKind::TiledJpg => {
            generate_tiled(input, trickplay_dir, media_file_id, interval_ms).await
        }
    }
}

/// Extract evenly-spaced JPEG frames into `dir` as `f%05d.jpg`. Returns the sorted list
/// of written frame paths. `fps = 1/interval_seconds` samples one frame per interval.
async fn extract_frames(
    input: &Path,
    dir: &Path,
    interval_ms: i64,
) -> Result<Vec<PathBuf>, TrickplayError> {
    let fps = format!("1/{}", (interval_ms as f64 / 1000.0).max(1.0));
    let pattern = dir.join("f%05d.jpg");
    let argv = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "warning".to_string(),
        "-nostdin".to_string(),
        "-y".to_string(),
        "-i".to_string(),
        input.to_string_lossy().into_owned(),
        "-vf".to_string(),
        // Sample at the interval, then downscale. `-vsync vfr` keeps the frame count
        // exactly to the sampled cadence.
        format!("fps={fps},scale={THUMB_WIDTH}:-1"),
        "-vsync".to_string(),
        "vfr".to_string(),
        // JPEG quality (2 = high, 31 = low). Scrub thumbnails favor small; 5 is a
        // reasonable middle.
        "-q:v".to_string(),
        "5".to_string(),
        pattern.to_string_lossy().into_owned(),
    ];
    tracing::debug!(argv = ?argv, "trickplay frame-extract ffmpeg argv");

    let output = Command::new(ffmpeg_bin())
        .args(&argv)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(TrickplayError::Spawn)?;
    if !output.status.success() {
        return Err(TrickplayError::NonZeroExit {
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    let mut frames: Vec<PathBuf> = std::fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "jpg").unwrap_or(false))
        .collect();
    frames.sort();
    if frames.is_empty() {
        return Err(TrickplayError::NoFrames);
    }
    Ok(frames)
}

/// Generate a BIF: extract frames, then pack them into the binary index format.
async fn generate_bif(
    input: &Path,
    trickplay_dir: &Path,
    media_file_id: i64,
    interval_ms: i64,
) -> Result<TrickplayOutput, TrickplayError> {
    let scratch = tempfile::tempdir()?;
    let frames = extract_frames(input, scratch.path(), interval_ms).await?;

    let out = trickplay_path(trickplay_dir, media_file_id, TrickplayKind::Bif);
    let bytes = pack_bif(&frames, interval_ms as u32)?;
    write_atomic(&out, &bytes)?;

    tracing::info!(
        media_file_id,
        frames = frames.len(),
        path = %out.display(),
        "generated BIF trickplay",
    );
    Ok(TrickplayOutput {
        kind: TrickplayKind::Bif,
        path: out,
        interval_ms,
        grid: None,
    })
}

/// Pack a list of JPEG frame files into BIF bytes (see the module layout comment).
fn pack_bif(frames: &[PathBuf], interval_ms: u32) -> Result<Vec<u8>, TrickplayError> {
    let count = frames.len() as u32;

    // Read every JPEG up front so we know each length for the offset index.
    let mut images: Vec<Vec<u8>> = Vec::with_capacity(frames.len());
    for f in frames {
        images.push(std::fs::read(f)?);
    }

    // Fixed header: magic(8) + version(4) + count(4) + ts-multiplier(4) + reserved(44).
    const HEADER_LEN: usize = 8 + 4 + 4 + 4 + 44;
    // Index: (count + 1) entries of 8 bytes each (the +1 is the trailing sentinel).
    let index_len = (count as usize + 1) * 8;
    let images_start = HEADER_LEN + index_len;

    let total: usize = images_start + images.iter().map(|i| i.len()).sum::<usize>();
    let mut buf: Vec<u8> = Vec::with_capacity(total);

    buf.extend_from_slice(&BIF_MAGIC);
    buf.extend_from_slice(&0u32.to_le_bytes()); // version
    buf.extend_from_slice(&count.to_le_bytes()); // image count
    buf.extend_from_slice(&interval_ms.to_le_bytes()); // ts multiplier (ms per unit)
    buf.extend_from_slice(&[0u8; 44]); // reserved

    // Index entries: timestamp = frame ordinal (multiplied by interval_ms at read time),
    // offset = absolute byte offset of that image in the file.
    let mut offset = images_start as u32;
    for (i, img) in images.iter().enumerate() {
        buf.extend_from_slice(&(i as u32).to_le_bytes());
        buf.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    // Sentinel entry terminates the index; its offset points past the last image.
    buf.extend_from_slice(&BIF_INDEX_SENTINEL.to_le_bytes());
    buf.extend_from_slice(&offset.to_le_bytes());

    // Concatenated JPEGs.
    for img in &images {
        buf.extend_from_slice(img);
    }
    Ok(buf)
}

/// Generate a tiled-JPG mosaic: extract frames, count them, then lay them into a
/// `cols`×`rows` grid with ffmpeg's `tile` filter in a second pass.
async fn generate_tiled(
    input: &Path,
    trickplay_dir: &Path,
    media_file_id: i64,
    interval_ms: i64,
) -> Result<TrickplayOutput, TrickplayError> {
    let scratch = tempfile::tempdir()?;
    let frames = extract_frames(input, scratch.path(), interval_ms).await?;
    let n = frames.len() as u32;

    let cols = TILE_COLS.min(n.max(1));
    let rows = (n + cols - 1) / cols; // ceil

    // Second pass: read the numbered frames back in and tile them. Using the frame
    // glob keeps this independent of the source seek cost.
    let out = trickplay_path(trickplay_dir, media_file_id, TrickplayKind::TiledJpg);
    let input_glob = scratch.path().join("f%05d.jpg");
    let argv = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "warning".to_string(),
        "-nostdin".to_string(),
        "-y".to_string(),
        // Read the numbered frames as an image sequence.
        "-i".to_string(),
        input_glob.to_string_lossy().into_owned(),
        "-vf".to_string(),
        format!("tile={cols}x{rows}"),
        "-frames:v".to_string(),
        "1".to_string(),
        "-q:v".to_string(),
        "5".to_string(),
        out.to_string_lossy().into_owned(),
    ];
    tracing::debug!(argv = ?argv, "trickplay tile ffmpeg argv");

    let output = Command::new(ffmpeg_bin())
        .args(&argv)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(TrickplayError::Spawn)?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&out);
        return Err(TrickplayError::NonZeroExit {
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    // Derive the *exact* per-tile height from the finished mosaic instead of assuming a
    // square tile. Frames are scaled `320:-1`, so tile height is aspect-dependent (≈180
    // for 16:9); the client offsets rows by `row * tile_h`, so a wrong value misaligns
    // every row after the first. One cheap ffprobe of the sheet gives the truth.
    let tile_h = match probe_sheet_dimensions(&out).await {
        // The mosaic is exactly `rows` cells tall, so the per-tile height is the sheet
        // height divided by the row count. (Tile width stays THUMB_WIDTH — fixed by our
        // `scale=320:-1` filter.)
        Some((_sheet_w, sheet_h)) if rows > 0 => (sheet_h / rows).max(1) as i64,
        // Fallback: assume 16:9 from the known width. Better than a square proxy.
        _ => ((THUMB_WIDTH as f64) * 9.0 / 16.0).round() as i64,
    };

    tracing::info!(
        media_file_id,
        frames = n,
        cols,
        rows,
        tile_h,
        path = %out.display(),
        "generated tiled-JPG trickplay",
    );
    Ok(TrickplayOutput {
        kind: TrickplayKind::TiledJpg,
        path: out,
        interval_ms,
        grid: Some(TrickplayGrid {
            // Tile width is fixed by our `scale=320:-1` filter; tile height is the exact
            // measured cell height (aspect-derived), so the client crops each row cleanly.
            tile_w: THUMB_WIDTH as i64,
            tile_h,
            cols: cols as i64,
            rows: rows as i64,
        }),
    })
}

/// The ffprobe binary. jellyfin-ffmpeg installs it alongside ffmpeg on PATH inside the
/// container (`docs/.tasks/50`); overridable via `FFPROBE_BIN` for tests / dev.
fn ffprobe_bin() -> String {
    std::env::var("FFPROBE_BIN").unwrap_or_else(|_| "ffprobe".to_string())
}

/// ffprobe the finished mosaic for its pixel `(width, height)`. Returns `None` on any
/// failure — the caller then falls back to an aspect estimate, so a missing/odd ffprobe
/// never fails trickplay generation (the sprite is already written).
async fn probe_sheet_dimensions(sheet: &Path) -> Option<(u32, u32)> {
    let sheet_arg = sheet.to_string_lossy().into_owned();
    let argv = [
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "stream=width,height",
        "-of",
        "csv=s=x:p=0",
        &sheet_arg,
    ];
    let output = Command::new(ffprobe_bin())
        .args(argv)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    // Expect a single line `WxH` (e.g. `3200x180`).
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.trim();
    let (w, h) = line.split_once('x')?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

/// Write `bytes` to `path` atomically: to a `.tmp` sibling then rename into place, so an
/// interrupted write never leaves a half-file the client would try to parse.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_extension_matches_kind() {
        let dir = PathBuf::from("/config/trickplay");
        assert!(trickplay_path(&dir, 3, TrickplayKind::Bif).ends_with("3.bif"));
        assert!(trickplay_path(&dir, 3, TrickplayKind::TiledJpg).ends_with("3.jpg"));
    }

    #[test]
    fn bif_packs_valid_header_and_index() {
        // Two tiny fake "JPEG" payloads (content is opaque to the packer).
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("f00000.jpg");
        let b = dir.path().join("f00001.jpg");
        std::fs::write(&a, b"AAAA").unwrap(); // 4 bytes
        std::fs::write(&b, b"BBBBBB").unwrap(); // 6 bytes

        let bytes = pack_bif(&[a, b], 10_000).unwrap();

        // Magic.
        assert_eq!(&bytes[0..8], &BIF_MAGIC);
        // Version 0.
        assert_eq!(read_u32(&bytes, 8), 0);
        // Image count = 2.
        assert_eq!(read_u32(&bytes, 12), 2);
        // Timestamp multiplier = interval.
        assert_eq!(read_u32(&bytes, 16), 10_000);

        const HEADER_LEN: usize = 8 + 4 + 4 + 4 + 44;
        // Index has (2 + 1) entries of 8 bytes.
        let index_start = HEADER_LEN;
        let images_start = HEADER_LEN + 3 * 8;

        // Entry 0: ts 0, offset = images_start.
        assert_eq!(read_u32(&bytes, index_start), 0);
        assert_eq!(read_u32(&bytes, index_start + 4), images_start as u32);
        // Entry 1: ts 1, offset = images_start + 4 (len of first image).
        assert_eq!(read_u32(&bytes, index_start + 8), 1);
        assert_eq!(read_u32(&bytes, index_start + 12), (images_start + 4) as u32);
        // Sentinel: ts 0xFFFFFFFF, offset = images_start + 4 + 6 (past both images).
        assert_eq!(read_u32(&bytes, index_start + 16), BIF_INDEX_SENTINEL);
        assert_eq!(read_u32(&bytes, index_start + 20), (images_start + 10) as u32);

        // The image bytes follow, concatenated in order.
        assert_eq!(&bytes[images_start..images_start + 4], b"AAAA");
        assert_eq!(&bytes[images_start + 4..images_start + 10], b"BBBBBB");
        // Total length is exactly header + index + images.
        assert_eq!(bytes.len(), images_start + 10);
    }

    #[test]
    fn write_atomic_leaves_no_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("5.bif");
        write_atomic(&out, b"hello").unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"hello");
        assert!(!out.with_extension("tmp").exists(), "tmp cleaned by rename");
    }

    fn read_u32(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
    }
}
