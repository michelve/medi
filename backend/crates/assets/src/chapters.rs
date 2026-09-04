//! Per-chapter poster frames (`docs/.tasks/99` Part C).
//!
//! For each embedded chapter of a file, seek to its start and grab a single downscaled JPEG,
//! written under `<chapter_images_dir>/<media_file_id>/<ordinal>.jpg`. The web player uses these
//! for the scrub-bar hover bubble (fallback when there's no trickplay sheet) and the in-player
//! scene-selection grid — mirroring how jellyfin extracts a frame per chapter for its scene view.
//!
//! One ffmpeg invocation per chapter (`-ss <start>` input seek → `-frames:v 1`), which is a
//! handful of quick seeks per title (typical chapter counts are 5–30) — cheaper than a full
//! decode pass and run under the same off-peak / GPU-idle gate as previews and trickplay. A
//! chapter whose extract fails is skipped (logged), so a partial set still lights up most scenes.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

use medi_transcode::caps::ffmpeg_bin;

/// Poster-frame width in pixels (height keeps aspect). ~400px matches jellyfin's chapter-image
/// `maxWidth` — large enough for a scene card, small enough to stay cheap on disk.
const FRAME_WIDTH: u32 = 400;

/// One chapter to extract a frame for: its `ordinal` (names the output file) and `start_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChapterFrame {
    pub ordinal: i64,
    pub start_ms: i64,
}

/// Errors from generating chapter poster frames.
#[derive(Debug, thiserror::Error)]
pub enum ChapterImageError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

/// The directory a file's chapter frames live in: `<chapter_images_dir>/<media_file_id>/`.
pub fn chapter_dir(chapter_images_dir: &Path, media_file_id: i64) -> PathBuf {
    chapter_images_dir.join(media_file_id.to_string())
}

/// The on-disk path of one chapter's frame: `<dir>/<media_file_id>/<ordinal>.jpg`. Stable across
/// regenerations so the `/api/chapters/<file_id>/image/<ordinal>` URL is stable.
pub fn chapter_image_path(chapter_images_dir: &Path, media_file_id: i64, ordinal: i64) -> PathBuf {
    chapter_dir(chapter_images_dir, media_file_id).join(format!("{ordinal}.jpg"))
}

/// Extract a poster frame for each chapter of `input`, writing `<dir>/<media_file_id>/<ordinal>.jpg`.
/// Returns the ordinals whose frame was written (a subset of `chapters` — a failed extract is
/// skipped, not fatal). An empty `chapters` slice is a no-op returning an empty vec, so the caller
/// can still stamp the file "done".
pub async fn generate(
    input: &Path,
    chapter_images_dir: &Path,
    media_file_id: i64,
    chapters: &[ChapterFrame],
) -> Result<Vec<i64>, ChapterImageError> {
    if chapters.is_empty() {
        return Ok(Vec::new());
    }
    let dir = chapter_dir(chapter_images_dir, media_file_id);
    std::fs::create_dir_all(&dir)?;

    let mut done = Vec::with_capacity(chapters.len());
    for c in chapters {
        match extract_one(input, &dir, c).await {
            Ok(()) => done.push(c.ordinal),
            Err(err) => tracing::warn!(
                media_file_id,
                ordinal = c.ordinal,
                error = %err,
                "chapter frame extract failed; skipping this chapter",
            ),
        }
    }
    tracing::info!(
        media_file_id,
        wanted = chapters.len(),
        wrote = done.len(),
        "generated chapter poster frames",
    );
    Ok(done)
}

/// Extract a single chapter's frame with one ffmpeg seek. Input `-ss` (before `-i`) is a fast
/// keyframe seek — poster-accurate is fine for a scene thumbnail. Writes atomically (temp +
/// rename) so an interrupted run never leaves a half-JPEG the client would try to draw.
async fn extract_one(input: &Path, dir: &Path, c: &ChapterFrame) -> Result<(), ChapterImageError> {
    let out = dir.join(format!("{}.jpg", c.ordinal));
    let tmp = dir.join(format!("{}.jpg.tmp", c.ordinal));
    let ss = format!("{:.3}", (c.start_ms.max(0) as f64) / 1000.0);
    let argv = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "warning".to_string(),
        "-nostdin".to_string(),
        "-y".to_string(),
        // Input seek to the chapter start (fast; keyframe-accurate is fine for a poster).
        "-ss".to_string(),
        ss,
        "-i".to_string(),
        input.to_string_lossy().into_owned(),
        "-frames:v".to_string(),
        "1".to_string(),
        "-vf".to_string(),
        format!("scale={FRAME_WIDTH}:-1"),
        "-q:v".to_string(),
        "4".to_string(),
        tmp.to_string_lossy().into_owned(),
    ];
    tracing::debug!(argv = ?argv, "chapter frame ffmpeg argv");

    let output = Command::new(ffmpeg_bin())
        .args(&argv)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&tmp);
        // A failed extract is non-fatal (caller skips the chapter); surface stderr for debugging.
        return Err(ChapterImageError::Io(std::io::Error::other(format!(
            "ffmpeg exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim(),
        ))));
    }
    // ffmpeg may exit 0 without writing when the seek lands past EOF; treat a missing temp as a
    // skip rather than renaming a nonexistent file.
    if !tmp.is_file() {
        return Err(ChapterImageError::Io(std::io::Error::other(
            "ffmpeg wrote no frame (seek past end?)",
        )));
    }
    std::fs::rename(&tmp, &out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_keyed_by_id_and_ordinal() {
        let base = PathBuf::from("/config/chapter-images");
        assert!(chapter_dir(&base, 7).ends_with("7"));
        assert!(chapter_image_path(&base, 7, 3).ends_with("7/3.jpg") || chapter_image_path(&base, 7, 3).ends_with("7\\3.jpg"));
    }

    #[tokio::test]
    async fn empty_chapters_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let done = generate(Path::new("/media/x.mkv"), dir.path(), 1, &[]).await.unwrap();
        assert!(done.is_empty(), "no chapters → nothing generated, no error");
    }
}
