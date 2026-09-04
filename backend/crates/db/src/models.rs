//! Rust row models + serde DTOs mirroring the `01-db-schema.md` tables.
//!
//! Each struct maps 1:1 to a table and knows how to build itself from a
//! [`rusqlite::Row`] via `from_row`. Columns are read positionally against the
//! explicit `SELECT` lists in [`crate::queries`]; keep the two in lockstep.
//!
//! The `media_files` HDR/DV columns are stored as loose strings/ints for
//! ffprobe-fidelity; [`MediaFile::profile`] reconstructs the typed
//! [`MediaProfile`] from `medi-core` for the transcode decision.

use medi_core::{DvProfile, HdrType, MediaProfile, VideoCodec};
use rusqlite::Row;
use serde::{Deserialize, Serialize};

/// A row of `movies`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Movie {
    pub id: i64,
    pub title: String,
    pub sort_title: String,
    pub year: Option<i64>,
    pub overview: Option<String>,
    pub added_at: i64,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    /// The movie's transparent-PNG title logo from fanart.tv (Task 93), relative to
    /// `images_dir()`. The client maps it to a `/api/images/...` URL exactly like
    /// `poster_path`. `None` when the movie has no logo / fanart is unconfigured.
    pub logo_path: Option<String>,
    /// The movie's fanart.tv background wallpaper (Task 95), relative to `images_dir()`.
    /// Shown on the detail hero in place of the TMDB backdrop when present. `None` when the
    /// movie has no wallpaper / fanart is unconfigured.
    pub wallpaper_path: Option<String>,
}

impl Movie {
    /// Column order: id, title, sort_title, year, overview, added_at,
    /// poster_path, backdrop_path, logo_path, wallpaper_path.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            title: row.get(1)?,
            sort_title: row.get(2)?,
            year: row.get(3)?,
            overview: row.get(4)?,
            added_at: row.get(5)?,
            poster_path: row.get(6)?,
            backdrop_path: row.get(7)?,
            logo_path: row.get(8)?,
            wallpaper_path: row.get(9)?,
        })
    }
}

/// A row of `series`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Series {
    pub id: i64,
    pub title: String,
    pub sort_title: String,
    pub year: Option<i64>,
    pub overview: Option<String>,
    pub added_at: i64,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
}

impl Series {
    /// Column order: id, title, sort_title, year, overview, added_at,
    /// poster_path, backdrop_path.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            title: row.get(1)?,
            sort_title: row.get(2)?,
            year: row.get(3)?,
            overview: row.get(4)?,
            added_at: row.get(5)?,
            poster_path: row.get(6)?,
            backdrop_path: row.get(7)?,
        })
    }
}

/// A row of `seasons`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Season {
    pub id: i64,
    pub series_id: i64,
    pub season_number: i64,
}

impl Season {
    /// Column order: id, series_id, season_number.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            series_id: row.get(1)?,
            season_number: row.get(2)?,
        })
    }
}

/// A row of `episodes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Episode {
    pub id: i64,
    pub season_id: i64,
    pub episode_number: i64,
    pub title: Option<String>,
    pub overview: Option<String>,
}

impl Episode {
    /// Column order: id, season_id, episode_number, title, overview.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            season_id: row.get(1)?,
            episode_number: row.get(2)?,
            title: row.get(3)?,
            overview: row.get(4)?,
        })
    }
}

/// A row of `media_files`. Belongs to exactly one movie OR one episode.
// `frame_rate: Option<f64>` (Task 99) means no `Eq` (f64 isn't `Eq`); `PartialEq` is enough.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaFile {
    pub id: i64,
    pub movie_id: Option<i64>,
    pub episode_id: Option<i64>,
    pub path: String,
    pub container: Option<String>,
    pub size_bytes: Option<i64>,
    pub duration_ms: Option<i64>,
    // video stream
    pub video_codec: Option<String>,
    pub video_profile: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub bit_depth: Option<i64>,
    pub bitrate: Option<i64>,
    // HDR / color
    pub transfer_characteristics: Option<String>,
    pub color_space: Option<String>,
    pub hdr_type: Option<String>,
    // Dolby Vision
    pub dv_profile: Option<i64>,
    pub dv_bl_compatible_id: Option<i64>,
    pub dv_level: Option<i64>,
    pub hw_decode_unsupported: bool,
    /// Video frame rate (Task 99) — for the web player's libass `targetFps`. NULL until probed.
    pub frame_rate: Option<f64>,
    /// Audio tracks of this file (Task 70). A child table, not `media_files` columns —
    /// a file is 1:N in audio. Empty when unprobed or read without the audio join;
    /// [`MediaFile::from_row`] leaves it empty and the query layer fills it in.
    #[serde(default)]
    pub audio_streams: Vec<AudioStream>,
    /// Subtitle tracks of this file (Task 90) — embedded tracks + external sidecars. A
    /// child table for the same 1:N reason as `audio_streams`. Empty when unprobed or read
    /// without the subtitle join; the query layer fills it in.
    #[serde(default)]
    pub subtitle_streams: Vec<SubtitleStream>,
    /// Embedded chapter markers of this file (Task 99). A child table, 1:N like the stream
    /// lists. Empty when unprobed or read without the chapter join; the query layer fills it.
    #[serde(default)]
    pub chapters: Vec<Chapter>,
}

impl MediaFile {
    /// Column order matches [`crate::queries::MEDIA_FILE_COLUMNS`]. `audio_streams` is a
    /// child table filled in separately by the query layer, so it starts empty here.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            movie_id: row.get(1)?,
            episode_id: row.get(2)?,
            path: row.get(3)?,
            container: row.get(4)?,
            size_bytes: row.get(5)?,
            duration_ms: row.get(6)?,
            video_codec: row.get(7)?,
            video_profile: row.get(8)?,
            width: row.get(9)?,
            height: row.get(10)?,
            bit_depth: row.get(11)?,
            bitrate: row.get(12)?,
            transfer_characteristics: row.get(13)?,
            color_space: row.get(14)?,
            hdr_type: row.get(15)?,
            dv_profile: row.get(16)?,
            dv_bl_compatible_id: row.get(17)?,
            dv_level: row.get(18)?,
            // stored as 0/1
            hw_decode_unsupported: row.get::<_, i64>(19)? != 0,
            frame_rate: row.get(20)?,
            audio_streams: Vec::new(),
            subtitle_streams: Vec::new(),
            chapters: Vec::new(),
        })
    }

    /// Reconstruct the typed [`MediaProfile`] the transcode crate consumes.
    ///
    /// Returns `None` if the row has not been probed yet (no `width`/`height`),
    /// since a profile without dimensions is meaningless to the decision logic.
    pub fn profile(&self) -> Option<MediaProfile> {
        let width = self.width? as u32;
        let height = self.height? as u32;

        // Single source of truth for the ffprobe-name → typed-codec mapping (Task 90);
        // shared with `ingest` so the two never drift.
        let codec = VideoCodec::from_ffprobe(self.video_codec.as_deref().unwrap_or(""));

        let hdr = match self.hdr_type.as_deref() {
            Some("hdr10") => HdrType::Hdr10,
            Some("hdr10plus") => HdrType::Hdr10Plus,
            Some("hlg") => HdrType::Hlg,
            Some("dolbyvision") => HdrType::DolbyVision,
            _ => HdrType::None,
        };

        let dv = match self.dv_profile {
            Some(5) => Some(DvProfile::P5),
            Some(7) => Some(DvProfile::P7),
            Some(8) => Some(DvProfile::P8 {
                bl_compatible_id: self.dv_bl_compatible_id.unwrap_or(0) as u8,
            }),
            _ => None,
        };

        Some(MediaProfile {
            codec,
            width,
            height,
            bit_depth: self.bit_depth.unwrap_or(8) as u8,
            hdr,
            dv,
            hw_decode_unsupported: self.hw_decode_unsupported,
            // Drives the QualityProfile::Capped decision (Task 70); negatives (never
            // stored) are ignored.
            bitrate: self.bitrate.and_then(|b| u64::try_from(b).ok()),
            // Drives the HLS keyframe GOP so transcoded segments cut at every SEGMENT_SECONDS
            // boundary (Task 101). NULL until re-probed (V13/V14).
            frame_rate: self.frame_rate,
        })
    }
}

/// A row of `audio_streams` — one audio track of a media file (Task 70).
///
/// `codec` / `immersive` are the normalized strings the transcode decision reads back;
/// `stream_index` is what react-native-video's `selectedAudioTrack` selects by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioStream {
    pub id: i64,
    pub media_file_id: i64,
    pub stream_index: i64,
    pub codec: Option<String>,
    pub profile: Option<String>,
    pub channels: Option<i64>,
    pub channel_layout: Option<String>,
    pub bitrate: Option<i64>,
    pub sample_rate: Option<i64>,
    pub language: Option<String>,
    pub title: Option<String>,
    /// `none` | `dolby_atmos` | `dts_x`.
    pub immersive: String,
    pub is_default: bool,
}

impl AudioStream {
    /// Column order: id, media_file_id, stream_index, codec, profile, channels,
    /// channel_layout, bitrate, sample_rate, language, title, immersive, is_default.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            media_file_id: row.get(1)?,
            stream_index: row.get(2)?,
            codec: row.get(3)?,
            profile: row.get(4)?,
            channels: row.get(5)?,
            channel_layout: row.get(6)?,
            bitrate: row.get(7)?,
            sample_rate: row.get(8)?,
            language: row.get(9)?,
            title: row.get(10)?,
            immersive: row.get(11)?,
            is_default: row.get::<_, i64>(12)? != 0,
        })
    }
}

/// A row of `subtitle_streams` — one subtitle track of a media file (Task 90).
///
/// Either an embedded track (`stream_index` set, `external_path` None) or an external
/// sidecar (`external_path` set, `stream_index` None). `format` is `"text"` | `"image"`;
/// the client uses it to decide between a WebVTT sidecar and a burn-in request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubtitleStream {
    pub id: i64,
    pub media_file_id: i64,
    pub stream_index: Option<i64>,
    pub codec: Option<String>,
    /// `"text"` | `"image"`.
    pub format: String,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_default: bool,
    pub is_forced: bool,
    pub is_external: bool,
    pub external_path: Option<String>,
}

impl SubtitleStream {
    /// Column order: id, media_file_id, stream_index, codec, format, language, title,
    /// is_default, is_forced, is_external, external_path.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            media_file_id: row.get(1)?,
            stream_index: row.get(2)?,
            codec: row.get(3)?,
            format: row.get(4)?,
            language: row.get(5)?,
            title: row.get(6)?,
            is_default: row.get::<_, i64>(7)? != 0,
            is_forced: row.get::<_, i64>(8)? != 0,
            is_external: row.get::<_, i64>(9)? != 0,
            external_path: row.get(10)?,
        })
    }
}

/// A row of `chapters` — one embedded chapter marker of a media file (Task 99).
///
/// `ordinal` is the 0-based order; `start_ms`/`end_ms` are milliseconds. `end_ms` may be
/// `None` (some files omit chapter end times — the player bounds a chapter by the next
/// chapter's `start_ms` then). `title` is the chapter name, may be `None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chapter {
    pub id: i64,
    pub media_file_id: i64,
    pub ordinal: i64,
    pub start_ms: i64,
    pub end_ms: Option<i64>,
    pub title: Option<String>,
    /// Whether a poster frame has been generated for this chapter (`docs/.tasks/99` Part C).
    /// Set by the off-peak asset worker; the client shows the hover image / scene card only
    /// when true (Jellyfin's `ImageTag` gate).
    pub has_image: bool,
}

impl Chapter {
    /// Column order: id, media_file_id, ordinal, start_ms, end_ms, title, has_image.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            media_file_id: row.get(1)?,
            ordinal: row.get(2)?,
            start_ms: row.get(3)?,
            end_ms: row.get(4)?,
            title: row.get(5)?,
            has_image: row.get::<_, i64>(6)? != 0,
        })
    }
}

/// A row of `people`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    pub id: i64,
    pub name: String,
}

/// The enriched `people` row backing a person page (`docs/.tasks/91` Phase B): the base
/// identity plus the TMDB linkage / headshot / bio columns added in V6. `photo_path` is
/// relative to `images_dir()`; the API maps it to a `/api/images/...` URL. Nullable columns
/// are absent for a person not yet enriched (pre-backfill).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonMeta {
    pub id: i64,
    pub name: String,
    pub tmdb_id: Option<i64>,
    pub photo_path: Option<String>,
    pub biography: Option<String>,
}

impl PersonMeta {
    /// Column order: id, name, tmdb_id, photo_path, biography.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            tmdb_id: row.get(2)?,
            photo_path: row.get(3)?,
            biography: row.get(4)?,
        })
    }
}

/// A `trailers` row — a YouTube trailer/teaser of a movie (Task 91 detail extensions).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trailer {
    pub id: i64,
    pub youtube_key: String,
    pub name: Option<String>,
    /// TMDB `type`: `"Trailer"` | `"Teaser"` | `"Clip"` | …
    pub kind: Option<String>,
}

impl Trailer {
    /// Column order: id, youtube_key, name, kind.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            youtube_key: row.get(1)?,
            name: row.get(2)?,
            kind: row.get(3)?,
        })
    }
}

/// A `collections` row — a TMDB franchise a movie belongs to (Task 91 detail extensions).
/// `poster_path` is relative to `images_dir()`; the API maps it to a `/api/images/...` URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collection {
    pub id: i64,
    pub name: String,
    pub poster_path: Option<String>,
}

impl Collection {
    /// Column order: id, name, poster_path.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            poster_path: row.get(2)?,
        })
    }
}

/// A genre with the number of titles carrying it — the shape `GET /api/genres` returns
/// (`docs/.tasks/91` Phase A). `count` is movies + series; only genres with `count >= 1`
/// are listed, ordered by count desc then name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenreCount {
    pub id: i64,
    pub name: String,
    pub count: i64,
}

impl GenreCount {
    /// Column order: id, name, count.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            count: row.get(2)?,
        })
    }
}

/// A genre carried by a title (id + name), for the detail-page metadata line
/// (`docs/.tasks/91`). Unlike [`GenreCount`] this has no count — it's the genres
/// a single movie/series belongs to, in provider order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Genre {
    pub id: i64,
    pub name: String,
}

impl Genre {
    /// Column order: id, name.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
        })
    }
}

/// A row of `libraries` (`docs/.tasks/60` Phase B).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Library {
    pub id: i64,
    pub name: String,
    /// `"movie"` | `"series"`.
    pub kind: String,
    pub created_at: i64,
}

impl Library {
    /// Column order: id, name, kind, created_at.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            kind: row.get(2)?,
            created_at: row.get(3)?,
        })
    }
}

/// A library together with its folder paths — the shape `GET /api/libraries` returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryWithFolders {
    #[serde(flatten)]
    pub library: Library,
    pub folders: Vec<String>,
}

/// A joined `credits` + `people` row (billing entry for a title). `photo_path` is the
/// person's downloaded headshot (Task 91 Phase B), relative to `images_dir()`, so a detail
/// page can render a circular avatar; `None` for a person not yet enriched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credit {
    pub id: i64,
    pub person_id: i64,
    pub person_name: String,
    pub role: Option<String>,
    pub character: Option<String>,
    pub ord: Option<i64>,
    pub photo_path: Option<String>,
}

impl Credit {
    /// Column order: credits.id, credits.person_id, people.name,
    /// credits.role, credits.character, credits.ord, people.photo_path.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            person_id: row.get(1)?,
            person_name: row.get(2)?,
            role: row.get(3)?,
            character: row.get(4)?,
            ord: row.get(5)?,
            photo_path: row.get(6)?,
        })
    }
}

/// A row of `playback_progress` (Task 98) — where playback of one file was left off.
///
/// Single-user, so keyed by `media_file_id` (one row per file). `duration_ms` is a snapshot
/// taken at write time so the resume/Continue-Watching `%` can be computed without a second
/// read of `media_files`. `finished` is set once playback passes ~95% (drops the title from
/// Continue Watching). `updated_at` is unix seconds, like the other epoch columns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub media_file_id: i64,
    pub position_ms: i64,
    pub duration_ms: i64,
    pub updated_at: i64,
    pub finished: bool,
}

impl Progress {
    /// Column order: media_file_id, position_ms, duration_ms, updated_at, finished.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            media_file_id: row.get(0)?,
            position_ms: row.get(1)?,
            duration_ms: row.get(2)?,
            updated_at: row.get(3)?,
            // stored as 0/1
            finished: row.get::<_, i64>(4)? != 0,
        })
    }
}

/// One row of the "Continue Watching" list (Task 98) — an in-progress title's playback
/// position joined to the owning movie/episode so a card can render a poster + title and link
/// straight to `/play/:file_id`. `kind` is `"movie"` | `"episode"`; `poster_path` is relative
/// to `images_dir()` (the owning movie's, or the episode's series' poster), mapped by the API
/// layer to a `/api/images/...` URL like a [`LibraryCard`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinueItem {
    pub file_id: i64,
    /// `"movie"` | `"episode"`.
    pub kind: String,
    /// The movie id, or the episode's series id — what a poster tile links its detail page to.
    pub title_id: i64,
    pub title: String,
    pub poster_path: Option<String>,
    pub position_ms: i64,
    pub duration_ms: i64,
    pub updated_at: i64,
}

impl ContinueItem {
    /// Column order: file_id, kind_tag (0=movie,1=episode), title_id, title, poster_path,
    /// position_ms, duration_ms, updated_at.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let kind = match row.get::<_, i64>(1)? {
            1 => "episode",
            _ => "movie",
        };
        Ok(Self {
            file_id: row.get(0)?,
            kind: kind.to_string(),
            title_id: row.get(2)?,
            title: row.get(3)?,
            poster_path: row.get(4)?,
            position_ms: row.get(5)?,
            duration_ms: row.get(6)?,
            updated_at: row.get(7)?,
        })
    }
}

/// A row of `preview_clips`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewClip {
    pub media_file_id: i64,
    pub path: String,
    pub generated_at: i64,
}

impl PreviewClip {
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            media_file_id: row.get(0)?,
            path: row.get(1)?,
            generated_at: row.get(2)?,
        })
    }
}

/// A row of `trickplay_assets`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrickplayAsset {
    pub media_file_id: i64,
    pub kind: String,
    pub path: String,
    pub interval_ms: i64,
    pub tile_w: Option<i64>,
    pub tile_h: Option<i64>,
    pub cols: Option<i64>,
    pub rows: Option<i64>,
    pub generated_at: i64,
}

impl TrickplayAsset {
    /// Column order: media_file_id, kind, path, interval_ms, tile_w, tile_h,
    /// cols, rows, generated_at.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            media_file_id: row.get(0)?,
            kind: row.get(1)?,
            path: row.get(2)?,
            interval_ms: row.get(3)?,
            tile_w: row.get(4)?,
            tile_h: row.get(5)?,
            cols: row.get(6)?,
            rows: row.get(7)?,
            generated_at: row.get(8)?,
        })
    }
}

/// A row of `scan_state` (ingest/assets bookkeeping).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanState {
    pub path: String,
    pub mtime: i64,
    pub size_bytes: i64,
    pub probed_at: Option<i64>,
    pub preview_done_at: Option<i64>,
    pub trickplay_done_at: Option<i64>,
}

impl ScanState {
    /// Column order: path, mtime, size_bytes, probed_at, preview_done_at,
    /// trickplay_done_at.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            path: row.get(0)?,
            mtime: row.get(1)?,
            size_bytes: row.get(2)?,
            probed_at: row.get(3)?,
            preview_done_at: row.get(4)?,
            trickplay_done_at: row.get(5)?,
        })
    }
}

// ---------------------------------------------------------------------------
// DTOs — the composed shapes the `api` crate serializes to the TV client.
// (Response envelopes like /api/library live in the api crate; these are the
// reusable detail aggregates the queries return.)
// ---------------------------------------------------------------------------

/// Full movie detail: the movie plus its files and billed credits, and (Task 91 detail
/// extensions) its trailers + franchise collection. The **other** in-library movies of the
/// collection are assembled and shaped by the API layer (as poster tiles), not here. Backs
/// `GET /api/movies/:id` (see `02-api-contract.md`).
// No `Eq`: embeds `Vec<MediaFile>`, which carries `frame_rate: f64` (Task 99).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MovieDetail {
    #[serde(flatten)]
    pub movie: Movie,
    pub media_files: Vec<MediaFile>,
    pub credits: Vec<Credit>,
    /// YouTube trailers, best-first. Empty when the movie has none.
    #[serde(default)]
    pub trailers: Vec<Trailer>,
    /// The franchise this movie belongs to, or `null`.
    #[serde(default)]
    pub collection: Option<Collection>,
    /// Genres carried by this movie, in provider order. Empty when unmatched.
    #[serde(default)]
    pub genres: Vec<Genre>,
}

/// An episode together with its on-disk media files (each with audio tracks).
///
/// The `Episode` row flattens to the top level (so `id`, `episode_number`, `title`,
/// `overview` stay where clients expect them) and `media_files` sits alongside — the
/// same shape `MovieDetail` uses. Carrying the files here lets the web/TV clients play
/// an episode directly (Task 82): the primary file's `id` is the `file_id` handed to
/// `GET /api/stream/:file_id`. Empty until the episode's file is ingested/probed.
// No `Eq`: embeds `Vec<MediaFile>` (carries `frame_rate: f64`, Task 99).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EpisodeWithFiles {
    #[serde(flatten)]
    pub episode: Episode,
    pub media_files: Vec<MediaFile>,
}

/// A season together with its ordered episodes (each with its media files).
// No `Eq`: transitively embeds `MediaFile` (carries `frame_rate: f64`, Task 99).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeasonWithEpisodes {
    #[serde(flatten)]
    pub season: Season,
    pub episodes: Vec<EpisodeWithFiles>,
}

/// Full series detail: the series plus its seasons/episodes and billed credits.
/// Backs `GET /api/series/:id` (see `02-api-contract.md`).
// No `Eq`: transitively embeds `MediaFile` (carries `frame_rate: f64`, Task 99).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeriesDetail {
    #[serde(flatten)]
    pub series: Series,
    pub seasons: Vec<SeasonWithEpisodes>,
    pub credits: Vec<Credit>,
}

/// Whether a [`LibraryCard`] describes a movie or a series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LibraryKind {
    Movie,
    Series,
}

/// One card in the unified `/api/library` grid: a movie or a series, plus just the
/// fields the client needs to render a poster tile. The `api` crate maps this to the
/// public card shape (`kind`, `id`, `title`, `year`, `poster`, `hdr`).
///
/// `hdr` is the highest HDR tier found across the title's media files (a series shows
/// the strongest format any of its episodes carry), or `None` when unprobed / SDR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryCard {
    pub kind: LibraryKind,
    pub id: i64,
    pub title: String,
    pub sort_title: String,
    pub year: Option<i64>,
    pub added_at: i64,
    pub poster_path: Option<String>,
    pub hdr: Option<String>,
    /// Provider `tmdb_id` (movies/series), `None` for an unmatched title. Lets a poster tile
    /// link to the pretty `/movie/:tmdbId` / `/series/:tmdbId` URL (falling back to `id`).
    pub tmdb_id: Option<i64>,
}

impl LibraryCard {
    /// Column order: kind_tag (0=movie,1=series), id, title, sort_title, year,
    /// added_at, poster_path, hdr, tmdb_id.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let kind = match row.get::<_, i64>(0)? {
            1 => LibraryKind::Series,
            _ => LibraryKind::Movie,
        };
        Ok(Self {
            kind,
            id: row.get(1)?,
            title: row.get(2)?,
            sort_title: row.get(3)?,
            year: row.get(4)?,
            added_at: row.get(5)?,
            poster_path: row.get(6)?,
            hdr: row.get(7)?,
            tmdb_id: row.get(8)?,
        })
    }
}
