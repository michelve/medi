//! Recursively walks `/media`, classifies each video file as a movie or a
//! series/episode from its path and naming, diffs it against `scan_state`, and yields
//! the set of new/changed files the worker should probe. Phase 1, sub-task 3.
//!
//! Classification is filename-driven (Phase 1 has no external metadata provider):
//! a `SxxEyy` (or `1x02`) marker anywhere in the path means an episode; otherwise the
//! file is a movie, its title/year parsed from the filename in the common
//! `Title (YEAR)` convention. Series/season/episode numbers and titles come from the
//! path components around the marker.
//!
//! The walk is a plain synchronous `std::fs` recursion (no extra crate) and runs on
//! the blocking pool from the worker — `/media` is read-only and never written.

use std::path::{Path, PathBuf};

/// Video file extensions we ingest. Anything else (subtitles, artwork, `.nfo`) is
/// skipped by the walk.
const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "m4v", "mov", "avi", "ts", "m2ts", "webm", "wmv", "mpg", "mpeg",
    // NEW (`docs/.tasks/90`) — mainstream containers. No RealMedia (rm/rmvb) or raw disc
    // images (.iso / VIDEO_TS) — those are out of scope.
    "flv", "ogv", "ogm", "3gp", "3g2", "vob", "mk3d",
];

/// External subtitle sidecar extensions the scanner discovers next to a video file
/// (`docs/.tasks/90` §External sidecar discovery). `.sub` is VobSub (image, paired with a
/// `.idx`); the rest are text.
const SUBTITLE_EXTENSIONS: &[&str] = &["srt", "ass", "ssa", "vtt", "sub"];

/// How the scanner classified a discovered file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classification {
    Movie {
        title: String,
        year: Option<i64>,
    },
    Episode {
        series_title: String,
        series_year: Option<i64>,
        season: i64,
        episode: i64,
        /// Episode title parsed from the filename, if any.
        title: Option<String>,
    },
}

/// One file the scanner found on `/media`, with the stat the worker diffs against
/// `scan_state` and the classification the worker persists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredFile {
    pub path: PathBuf,
    pub mtime: i64,
    pub size_bytes: i64,
    pub class: Classification,
    /// The library this file was found under (Phase B). `None` for a legacy single-root
    /// scan; `Some(id)` when scanned via [`scan_root`] with a library root, so the worker
    /// can scope the owning movie/series to that library.
    pub library_id: Option<i64>,
}

/// A hint that forces classification when a library declares its `kind`, overriding the
/// filename guess (`docs/.tasks/60` §Sub-tasks 10) — a stray `SxxEyy` in a Movies
/// library stays a movie, matching Plex and removing a misclassification class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindHint {
    /// Classify by filename (legacy single-root behavior).
    Guess,
    /// Force a movie regardless of episode markers.
    Movie,
    /// Force a series/episode; if no marker is present, treat the whole file as S01E01
    /// of a series named from the file/folder (a loose episode in a TV library).
    Series,
}

/// Recursively collect every video file under `root`, classified and stat'd, using
/// filename-only classification and no library scoping (legacy single-root scan).
///
/// Errors reading an individual directory or entry are logged and skipped rather than
/// aborting the whole scan — one unreadable folder should not stop ingest. Returns the
/// full set; the worker diffs each against `scan_state` to decide what to (re)probe.
pub fn scan(root: &Path) -> Vec<DiscoveredFile> {
    scan_root(root, None, KindHint::Guess)
}

/// Recursively collect every video file under `root`, tagging each with `library_id` and
/// classifying under `hint` (`docs/.tasks/60` §Sub-tasks 10). The worker calls this once
/// per library folder so a file is scoped to its library and the library `kind` overrides
/// filename guessing.
pub fn scan_root(root: &Path, library_id: Option<i64>, hint: KindHint) -> Vec<DiscoveredFile> {
    let mut out = Vec::new();
    walk(root, library_id, hint, &mut out);
    out
}

fn walk(dir: &Path, library_id: Option<i64>, hint: KindHint, out: &mut Vec<DiscoveredFile>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            tracing::warn!(dir = %dir.display(), error = %err, "skipping unreadable directory");
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "skipping unstat-able entry");
                continue;
            }
        };

        if file_type.is_dir() {
            walk(&path, library_id, hint, out);
        } else if file_type.is_file() && is_video(&path) {
            match discover(&path, library_id, hint) {
                Some(f) => out.push(f),
                None => tracing::debug!(path = %path.display(), "could not stat file; skipping"),
            }
        }
    }
}

/// Is this a video file we ingest (by extension, case-insensitive)?
fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| VIDEO_EXTENSIONS.contains(&e.as_str()))
}

/// Discover external subtitle sidecars sibling to `video_path` (`docs/.tasks/90`
/// §External sidecar discovery). A sidecar matches when its filename stem starts with the
/// video's stem, so `Movie (2020).mkv` picks up `Movie (2020).srt`,
/// `Movie (2020).en.srt`, and `Movie (2020).en.forced.srt`. The trailing tokens after the
/// video stem are parsed for an optional ISO-639 `.<lang>` and a `.forced` marker.
///
/// Discovery is **filename-only** — no ffprobe pass on the sidecar — and sidecars are
/// never written (they live under `/media`, read-only). Returns one
/// [`SubtitleStreamWrite`] per matched sidecar (`is_external = true`), ready to persist
/// alongside the file's embedded subtitle tracks.
pub fn discover_sidecars(
    video_path: &Path,
) -> Vec<medi_db::writes::SubtitleStreamWrite> {
    let Some(dir) = video_path.parent() else {
        return Vec::new();
    };
    let Some(video_stem) = video_path.file_stem().and_then(|s| s.to_str()) else {
        return Vec::new();
    };

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        let ext = ext.to_ascii_lowercase();
        if !SUBTITLE_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }
        let Some(sub_stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // The sidecar must belong to *this* video: its stem starts with the video stem.
        // The remainder (`.en`, `.en.forced`, or "") carries the language / forced tokens.
        let Some(rest) = sub_stem.strip_prefix(video_stem) else {
            continue;
        };
        let (language, is_forced) = parse_sidecar_tokens(rest);
        // `.sub` is VobSub (image, paired with a `.idx`); the text formats become WebVTT.
        let format = if ext == "sub" {
            medi_core::SubtitleFormat::Image
        } else {
            medi_core::SubtitleFormat::Text
        };
        out.push(medi_db::writes::SubtitleStreamWrite {
            stream_index: None,
            codec: Some(sidecar_codec(&ext).to_string()),
            format: format.as_str().to_string(),
            language,
            title: None,
            is_default: false,
            is_forced,
            is_external: true,
            external_path: Some(path.to_string_lossy().into_owned()),
        });
    }
    // Stable ordering so a re-scan writes the same set in the same order.
    out.sort_by(|a, b| a.external_path.cmp(&b.external_path));
    out
}

/// The ffprobe-style codec name for a sidecar extension (so the serving route knows how
/// to convert it): `.srt` → `subrip`, `.vtt` → `webvtt`, `.sub` → VobSub image.
fn sidecar_codec(ext: &str) -> &'static str {
    match ext {
        "srt" => "subrip",
        "ass" => "ass",
        "ssa" => "ssa",
        "vtt" => "webvtt",
        "sub" => "dvd_subtitle",
        _ => "subrip",
    }
}

/// Parse the tokens trailing a sidecar's video-stem prefix into `(language, is_forced)`.
/// `rest` is e.g. `""`, `".en"`, `".en.forced"`, `".forced"`. A 2–3 char alpha token is
/// taken as an ISO-639 language; a `forced` token sets the forced flag.
fn parse_sidecar_tokens(rest: &str) -> (Option<String>, bool) {
    let mut language = None;
    let mut is_forced = false;
    for tok in rest.split('.').filter(|t| !t.is_empty()) {
        let lower = tok.to_ascii_lowercase();
        if lower == "forced" {
            is_forced = true;
        } else if (2..=3).contains(&lower.len()) && lower.chars().all(|c| c.is_ascii_alphabetic()) {
            language = Some(lower);
        }
    }
    (language, is_forced)
}

/// Stat and classify one file into a [`DiscoveredFile`]. `None` if it cannot be
/// stat'd (e.g. it vanished between the directory listing and here).
fn discover(path: &Path, library_id: Option<i64>, hint: KindHint) -> Option<DiscoveredFile> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Some(DiscoveredFile {
        path: path.to_path_buf(),
        mtime,
        size_bytes: meta.len() as i64,
        class: classify_with_hint(path, hint),
        library_id,
    })
}

/// Classify a file path as a movie or an episode.
///
/// The filename stem is the primary signal. If it (or a parent folder) contains a
/// `SxxEyy` / `xEyy` / `NxMM` episode marker, it is an episode; the series title is
/// taken from the text before the marker (or the show folder), and the episode title
/// from the text after it. Otherwise it is a movie.
pub fn classify(path: &Path) -> Classification {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    if let Some(ep) = parse_episode(path, &stem) {
        return ep;
    }

    let (title, year) = parse_title_year(&stem);
    Classification::Movie { title, year }
}

/// Classify honoring a library [`KindHint`] (`docs/.tasks/60` §Sub-tasks 10). The library
/// kind wins over the filename guess:
/// - `Movie`: always a movie — an episode marker in a Movies library is ignored, and the
///   whole `Title (YEAR)` is parsed from the filename.
/// - `Series`: always an episode — if the filename has a marker it is used; if not, the
///   file is treated as S01E01 of a series named from the file/folder (a loose episode).
/// - `Guess`: the legacy filename-driven [`classify`].
pub fn classify_with_hint(path: &Path, hint: KindHint) -> Classification {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    match hint {
        KindHint::Guess => classify(path),
        KindHint::Movie => {
            let (title, year) = parse_title_year(&stem);
            Classification::Movie { title, year }
        }
        KindHint::Series => {
            // Prefer a real marker; otherwise synthesize S01E01 for a loose file so a TV
            // library never silently drops a movie-named episode.
            parse_episode(path, &stem).unwrap_or_else(|| {
                let folder = show_folder_name(path).unwrap_or_default();
                let (series_title, series_year) = if folder.trim().is_empty() {
                    parse_title_year(&stem)
                } else {
                    parse_title_year(&folder)
                };
                Classification::Episode {
                    series_title,
                    series_year,
                    season: 1,
                    episode: 1,
                    title: None,
                }
            })
        }
    }
}

/// Try to read a `SxxEyy`-style marker from the filename, falling back to the parent
/// folders for the series title / season number. Returns `None` if no marker is found
/// (→ the file is a movie).
fn parse_episode(path: &Path, stem: &str) -> Option<Classification> {
    let (marker_start, season, episode) = find_episode_marker(stem)?;

    // Series title: the filename text before the marker, cleaned; if that is empty
    // (e.g. the file is just "S01E01.mkv"), fall back to the show folder name — the
    // grandparent of the file (…/Show/Season 01/S01E01.mkv) or its parent.
    // `.get()` keeps a non-ASCII stem (rare) from panicking on a byte-index slice.
    let before = clean_title(stem.get(..marker_start).unwrap_or(""));
    let (series_title, series_year) = if before.trim().is_empty() {
        let folder = show_folder_name(path).unwrap_or_default();
        parse_title_year(&folder)
    } else {
        parse_title_year(&before)
    };

    // Episode title: text after the marker, if any (e.g. "S01E01 - Pilot").
    let after_start = marker_start + marker_len(stem, marker_start);
    let ep_title = {
        let t = clean_title(stem.get(after_start..).unwrap_or(""));
        if t.trim().is_empty() {
            None
        } else {
            Some(t)
        }
    };

    Some(Classification::Episode {
        series_title,
        series_year,
        season,
        episode,
        title: ep_title,
    })
}

/// The show folder: prefer the grandparent (…/Show/Season N/file), else the parent.
fn show_folder_name(path: &Path) -> Option<String> {
    let parent = path.parent()?;
    let parent_name = parent.file_name().and_then(|n| n.to_str()).unwrap_or("");
    // A "Season 01" / "Specials" style parent means the show is one level up.
    if looks_like_season_folder(parent_name) {
        parent
            .parent()
            .and_then(|g| g.file_name())
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
    } else {
        Some(parent_name.to_string())
    }
}

fn looks_like_season_folder(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("season") || lower == "specials" || lower.starts_with("series ")
}

/// Find a `SxxEyy` / `sxxeyy` / `NxMM` episode marker in `stem`, returning its byte
/// start offset plus the parsed `(season, episode)`.
fn find_episode_marker(stem: &str) -> Option<(usize, i64, i64)> {
    let bytes = stem.as_bytes();
    let lower = stem.to_ascii_lowercase();
    let lb = lower.as_bytes();

    // Form 1: S<season>E<episode>, e.g. "S01E02".
    for i in 0..lb.len() {
        if lb[i] == b's' {
            if let Some((s, next)) = take_number(lb, i + 1) {
                if next < lb.len() && lb[next] == b'e' {
                    if let Some((e, _)) = take_number(lb, next + 1) {
                        return Some((i, s, e));
                    }
                }
            }
        }
    }

    // Form 2: <season>x<episode>, e.g. "1x02". Require a digit before 'x'.
    for i in 0..bytes.len() {
        if lb[i] == b'x' && i > 0 && bytes[i - 1].is_ascii_digit() {
            // Walk back to the start of the leading number.
            let mut start = i;
            while start > 0 && bytes[start - 1].is_ascii_digit() {
                start -= 1;
            }
            if let (Some((s, _)), Some((e, _))) =
                (take_number(bytes, start), take_number(bytes, i + 1))
            {
                return Some((start, s, e));
            }
        }
    }

    None
}

/// The length in bytes of the marker beginning at `start` in `stem`, so callers can
/// find where the episode title begins. Recomputed rather than threaded through so the
/// two marker forms share one exit path.
fn marker_len(stem: &str, start: usize) -> usize {
    let lower = stem.to_ascii_lowercase();
    let lb = lower.as_bytes();
    if start < lb.len() && lb[start] == b's' {
        // S<num>E<num>
        if let Some((_, after_s)) = take_number(lb, start + 1) {
            if after_s < lb.len() && lb[after_s] == b'e' {
                if let Some((_, after_e)) = take_number(lb, after_s + 1) {
                    return after_e - start;
                }
            }
        }
    }
    // <num>x<num>
    if let Some((_, after_a)) = take_number(lb, start) {
        if after_a < lb.len() && lb[after_a] == b'x' {
            if let Some((_, after_b)) = take_number(lb, after_a + 1) {
                return after_b - start;
            }
        }
    }
    0
}

/// Parse a run of ASCII digits starting at `from`, returning `(value, index_after)`.
/// `None` if there is no digit at `from`.
fn take_number(bytes: &[u8], from: usize) -> Option<(i64, usize)> {
    let mut i = from;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == from {
        return None;
    }
    let n: i64 = std::str::from_utf8(&bytes[from..i]).ok()?.parse().ok()?;
    Some((n, i))
}

/// Parse a `Title (YEAR)` filename stem into `(title, year)`. The year is a 19xx/20xx
/// in parentheses if present; the title is everything before it, cleaned of the
/// separators (`.`, `_`) rips use in place of spaces and of trailing quality tags.
fn parse_title_year(stem: &str) -> (String, Option<i64>) {
    let year = extract_year(stem);
    let title = if let Some(y) = year {
        // Cut the title at the year token so "Movie (2017) 2160p BluRay" → "Movie".
        let needle = y.to_string();
        match stem.find(&needle) {
            Some(idx) => clean_title(&stem[..idx]),
            None => clean_title(stem),
        }
    } else {
        clean_title(stem)
    };
    let title = if title.trim().is_empty() {
        stem.to_string()
    } else {
        title
    };
    (title, year)
}

/// Extract a plausible release year (1900–2099) from a stem, preferring one in
/// parentheses/brackets. Returns the first match.
fn extract_year(stem: &str) -> Option<i64> {
    let bytes = stem.as_bytes();
    // Prefer (YYYY) / [YYYY].
    for open in ['(', '['] {
        if let Some(pos) = stem.find(open) {
            let rest = &stem[pos + 1..];
            if let Some((y, _)) = take_number(rest.as_bytes(), 0) {
                if is_year(y) {
                    return Some(y);
                }
            }
        }
    }
    // Fall back to any standalone 4-digit run that looks like a year.
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            if let Some((y, next)) = take_number(bytes, i) {
                if next - i == 4 && is_year(y) {
                    return Some(y);
                }
                i = next;
                continue;
            }
        }
        i += 1;
    }
    None
}

fn is_year(y: i64) -> bool {
    (1900..=2099).contains(&y)
}

/// Normalize a raw title fragment: turn `.`/`_` separators into spaces, drop bracketed
/// groups and trailing scene/quality tags, and collapse whitespace.
fn clean_title(raw: &str) -> String {
    // Replace separators with spaces.
    let mut s: String = raw
        .chars()
        .map(|c| match c {
            '.' | '_' => ' ',
            _ => c,
        })
        .collect();

    // Strip anything from the first bracket/paren onward (year + tags live there).
    if let Some(idx) = s.find(['(', '[', '{']) {
        s.truncate(idx);
    }

    // Drop dashes used as separators on either end ("Show - " → "Show",
    // " - Half Loop" → "Half Loop"). A dash here is always a leftover separator
    // between the marker and the title, never meaningful.
    let s = s.trim().trim_matches('-').trim();

    // Collapse internal whitespace.
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Case-insensitive alphabetical sort key for a title (mirrors `sort_title` in the
/// schema). Lowercased, whitespace-normalized; a leading English article is dropped so
/// "The Matrix" files under "matrix".
pub fn sort_title(title: &str) -> String {
    let lower = title.trim().to_lowercase();
    for article in ["the ", "a ", "an "] {
        if let Some(rest) = lower.strip_prefix(article) {
            return rest.to_string();
        }
    }
    lower
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn classify_str(p: &str) -> Classification {
        classify(&PathBuf::from(p))
    }

    #[test]
    fn movie_library_kind_overrides_stray_episode_marker() {
        // A file with an SxxEyy marker in a *Movies* library stays a movie (Plex parity).
        let c = classify_with_hint(
            &PathBuf::from("/media/Movies/Weird S01E01 Title (2020).mkv"),
            KindHint::Movie,
        );
        assert!(matches!(c, Classification::Movie { .. }), "movie hint forces a movie: {c:?}");
    }

    #[test]
    fn series_library_kind_forces_episode_even_without_marker() {
        // A movie-named file in a TV library becomes S01E01 of a series (no silent drop).
        let c = classify_with_hint(
            &PathBuf::from("/media/TV/Some Show/Pilot.mkv"),
            KindHint::Series,
        );
        match c {
            Classification::Episode { season, episode, series_title, .. } => {
                assert_eq!((season, episode), (1, 1));
                assert_eq!(series_title, "Some Show");
            }
            other => panic!("series hint must force an episode, got {other:?}"),
        }
    }

    #[test]
    fn series_hint_still_honors_a_real_marker() {
        let c = classify_with_hint(
            &PathBuf::from("/media/TV/Severance/Season 01/Severance S01E02 - Half Loop.mkv"),
            KindHint::Series,
        );
        match c {
            Classification::Episode { season, episode, .. } => assert_eq!((season, episode), (1, 2)),
            other => panic!("expected episode, got {other:?}"),
        }
    }

    #[test]
    fn movie_with_year_and_tags() {
        let c = classify_str("/media/Movies/Blade Runner 2049 (2017) 2160p BluRay.mkv");
        assert_eq!(
            c,
            Classification::Movie {
                title: "Blade Runner 2049".to_string(),
                year: Some(2017),
            }
        );
    }

    #[test]
    fn movie_dotted_scene_name() {
        let c = classify_str("/media/Arrival.2016.1080p.BluRay.x264.mkv");
        assert_eq!(
            c,
            Classification::Movie {
                title: "Arrival".to_string(),
                year: Some(2016),
            }
        );
    }

    #[test]
    fn movie_without_year() {
        let c = classify_str("/media/Home Video.mkv");
        assert_eq!(
            c,
            Classification::Movie {
                title: "Home Video".to_string(),
                year: None,
            }
        );
    }

    #[test]
    fn episode_sxxeyy_with_title() {
        let c = classify_str("/media/Severance/Season 01/Severance S01E02 - Half Loop.mkv");
        assert_eq!(
            c,
            Classification::Episode {
                series_title: "Severance".to_string(),
                series_year: None,
                season: 1,
                episode: 2,
                title: Some("Half Loop".to_string()),
            }
        );
    }

    #[test]
    fn episode_marker_only_uses_show_folder() {
        // Filename is just the marker; the show name comes from the grandparent folder.
        let c = classify_str("/media/Severance (2022)/Season 1/S01E01.mkv");
        assert_eq!(
            c,
            Classification::Episode {
                series_title: "Severance".to_string(),
                series_year: Some(2022),
                season: 1,
                episode: 1,
                title: None,
            }
        );
    }

    #[test]
    fn episode_nxmm_form() {
        let c = classify_str("/media/Show/Show 1x02.mkv");
        assert_eq!(
            c,
            Classification::Episode {
                series_title: "Show".to_string(),
                series_year: None,
                season: 1,
                episode: 2,
                title: None,
            }
        );
    }

    #[test]
    fn sort_title_drops_article() {
        assert_eq!(sort_title("The Matrix"), "matrix");
        assert_eq!(sort_title("Arrival"), "arrival");
        assert_eq!(sort_title("An Education"), "education");
    }

    #[test]
    fn is_video_by_extension() {
        assert!(is_video(&PathBuf::from("/media/a.MKV")));
        assert!(is_video(&PathBuf::from("/media/a.mp4")));
        assert!(!is_video(&PathBuf::from("/media/a.srt")));
        assert!(!is_video(&PathBuf::from("/media/a.nfo")));
    }

    #[test]
    fn widened_containers_are_ingested() {
        // Task 90 container widening — .flv / .vob / .3gp etc. are now discovered.
        assert!(is_video(&PathBuf::from("/media/a.flv")));
        assert!(is_video(&PathBuf::from("/media/a.vob")));
        assert!(is_video(&PathBuf::from("/media/a.3gp")));
        assert!(is_video(&PathBuf::from("/media/a.mk3d")));
        // Out of scope: RealMedia / disc images stay unrecognized.
        assert!(!is_video(&PathBuf::from("/media/a.rmvb")));
        assert!(!is_video(&PathBuf::from("/media/a.iso")));
    }

    #[test]
    fn parse_sidecar_tokens_reads_lang_and_forced() {
        assert_eq!(parse_sidecar_tokens(""), (None, false));
        assert_eq!(parse_sidecar_tokens(".en"), (Some("en".into()), false));
        assert_eq!(parse_sidecar_tokens(".forced"), (None, true));
        assert_eq!(parse_sidecar_tokens(".en.forced"), (Some("en".into()), true));
        assert_eq!(parse_sidecar_tokens(".eng"), (Some("eng".into()), false));
    }

    #[test]
    fn discover_sidecars_matches_stem_and_parses_tokens() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        // The video and three matching sidecars, plus one unrelated sidecar.
        std::fs::write(dir.join("Movie (2020).mkv"), b"x").unwrap();
        std::fs::write(dir.join("Movie (2020).srt"), b"x").unwrap();
        std::fs::write(dir.join("Movie (2020).en.forced.srt"), b"x").unwrap();
        std::fs::write(dir.join("Movie (2020).sub"), b"x").unwrap();
        std::fs::write(dir.join("Other Movie.srt"), b"x").unwrap();

        let subs = discover_sidecars(&dir.join("Movie (2020).mkv"));
        assert_eq!(subs.len(), 3, "only the three matching sidecars: {subs:?}");
        assert!(subs.iter().all(|s| s.is_external && s.stream_index.is_none()));

        let forced = subs
            .iter()
            .find(|s| s.external_path.as_deref().unwrap().ends_with("en.forced.srt"))
            .unwrap();
        assert_eq!(forced.language.as_deref(), Some("en"));
        assert!(forced.is_forced);
        assert_eq!(forced.format, "text");
        assert_eq!(forced.codec.as_deref(), Some("subrip"));

        // The VobSub .sub sidecar is classified image.
        let vobsub = subs
            .iter()
            .find(|s| s.external_path.as_deref().unwrap().ends_with(".sub"))
            .unwrap();
        assert_eq!(vobsub.format, "image");
    }
}
